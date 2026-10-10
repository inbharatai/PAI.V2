package com.unoone.agent.modelmanager

import android.content.Context
import com.unoone.agent.core.model.BrainModelRegistry
import com.unoone.agent.core.model.BrainModelSpec
import com.unoone.agent.core.modeladmission.LoadAdmission
import com.unoone.agent.core.modeladmission.NativeLoadReceipt
import com.unoone.agent.core.util.Logger
import com.unoone.agent.storage.dao.ModelMetadataDao
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.security.MessageDigest
import kotlinx.serialization.json.Json

/**
 * Source-of-truth model filesystem facade.
 *
 * UnoOne stores models below the app-private models root using typed subdirectories. The bundled
 * manifest is the only install catalogue. If it cannot be parsed, model detection and installation
 * fail closed instead of inventing fallback descriptors or directories.
 */
class ModelManager(
    private val context: Context,
    private val modelMetadataDao: ModelMetadataDao? = null
) {

    private val manifestLoader = ModelManifestLoader()
    private val artifactVerifier = ArtifactVerifier(PreferencesVerificationRecordStore(context))
    private val installer: ModelInstaller by lazy {
        ModelInstaller(appPrivateModelPath, modelMetadataDao) { name ->
            runCatching { context.assets.open(name) }.getOrNull()
        }
    }

    private val appPrivateModelPath: String
        get() = context.getExternalFilesDir("models")?.absolutePath
            ?: context.filesDir.resolve("models").absolutePath

    private val bundles get() = ModelBundleStore(File(appPrivateModelPath))
    private val admission by lazy { AndroidModelAdmission(context, File(appPrivateModelPath)) }

    /** v1 pointer reader with read-only legacy-layout fallback; never rewrites standalone .bin files.
     * A pointer that exists but whose bundle fails verification resolves to a non-existent folder:
     * health reports missing and the legacy flat layout is NOT silently substituted.
     */
    private fun resolvedFolder(descriptor: ModelDescriptor): File {
        val store = bundles
        if (!store.hasPointer(descriptor.id)) return File(appPrivateModelPath, descriptor.folder)
        return runCatching { store.active(descriptor.id)?.root }.getOrNull()
            ?: File(appPrivateModelPath, ".bundles-v1/${descriptor.id}/.integrity-failed").also {
                Logger.w("ModelManager: active bundle for ${descriptor.id} failed integrity; reporting missing")
            }
    }
    fun deviceCheck(id: String, loading: Boolean = false): AndroidModelAdmission.Check =
        admission.check(requireNotNull(findModel(id)) { "Unknown model id" }, loading)
    fun approveManualInstall(id: String, allowMetered: Boolean): Boolean =
        admission.approveManual(requireNotNull(findModel(id)), allowMetered)
    fun revokeManualInstall(id: String) { admission.revoke(requireNotNull(findModel(id))) }
    /** True when the model resolves from the pre-receipt legacy flat layout (no v1 pointer). */
    fun isLegacyInstalled(id: String): Boolean {
        val descriptor = findModel(id) ?: return false
        if (bundles.hasPointer(descriptor.id)) return false
        val folder = File(appPrivateModelPath, descriptor.folder)
        return folder.isDirectory && folder.walkTopDown().any { it.isFile && !it.name.endsWith(".part") && it.length() > 0L }
    }
    /** Single decision function for explicit load, boot auto-load and staged activation. */
    fun loadDecision(id: String, purpose: LoadAdmission.Purpose = LoadAdmission.Purpose.EXPLICIT_LOAD): LoadAdmission.Decision =
        admission.decide(requireNotNull(findModel(id)) { "Unknown model id" }, purpose, isLegacyInstalled(id))
    fun beforeNativeLoad(id: String, purpose: LoadAdmission.Purpose = LoadAdmission.Purpose.EXPLICIT_LOAD): String? = runCatching {
        loadDecision(id, purpose).let { if (it.allowed) null else it.reason }
    }.getOrElse { "Native Device Check unavailable; paused" }
    /** Three honest states for the UI: QUALIFIED / WORKS_HERE / UNKNOWN. Unknown is not refused. */
    fun evidenceState(id: String): LoadAdmission.EvidenceState = runCatching {
        val d = requireNotNull(findModel(id)); LoadAdmission.evidenceState(admission.evidence(d, isLegacyInstalled(id)))
    }.getOrDefault(LoadAdmission.EvidenceState.UNKNOWN)
    fun latestReceipt(id: String): NativeLoadReceipt? = runCatching {
        val d = requireNotNull(findModel(id)); admission.evidence(d, false).latestReceipt
    }.getOrNull()
    /** Only native adapters call this, after a real load (+ smoke). Never from UI/sync/worker data. */
    fun recordNativeLoad(id: String, outcome: NativeLoadReceipt.Outcome, lane: String, reason: String = "",
        backend: String = "", loadMs: Long = 0, smokeMs: Long = 0, smokeTokens: Int = 0): NativeLoadReceipt? = runCatching {
        val d = requireNotNull(findModel(id))
        admission.record(d, outcome, lane, reason, backend, loadMs, smokeMs, smokeTokens, runCatching { admission.probe() }.getOrNull())
    }.onFailure { Logger.w("ModelManager: receipt not recorded for $id: ${it.message}") }.getOrNull()
    fun stagedBundle(id: String): ModelBundleStore.Staged? = findModel(id)?.let {
        bundles.latestStaged(id, admission.identity(it))
    }
    /** Returns the exact runtime load path inside a candidate bundle root, or null when absent. */
    fun loadPathWithin(root: File, spec: BrainModelSpec): String? = when (spec.runtime) {
        com.unoone.agent.core.model.BrainRuntime.LITERT_LM -> File(root, spec.fileName).takeIf { it.isFile }?.absolutePath
        com.unoone.agent.core.model.BrainRuntime.MNN -> File(root, "config.json").takeIf { it.isFile }?.absolutePath
        com.unoone.agent.core.model.BrainRuntime.LLAMA_CPP -> root.takeIf { it.isDirectory }?.absolutePath
    }
    /** Promotion path. Fresh admission uses the ACTIVATION purpose (transactional smoke may proceed
     * on a borderline available-RAM estimate; physically impossible / lowMemory-now still refuse). */
    fun activateStaged(bundle: ModelBundleStore.Staged, loader: ModelBundleStore.NativeLoader): Boolean {
        val descriptor = requireNotNull(findModel(bundle.modelId))
        check(bundle.identity == admission.identity(descriptor)) { "Staged profile no longer matches catalogue" }
        return bundles.activate(bundle, object : ModelBundleStore.NativeLoader by loader {
            override fun freshAdmissionError(): String? =
                beforeNativeLoad(bundle.modelId, LoadAdmission.Purpose.ACTIVATION) ?: loader.freshAdmissionError()
        })
    }
    fun cleanupPartial(id: String) { findModel(id)?.let { bundles.cleanupPartial(id, admission.identity(it)) } }

    fun loadManifest(): ModelManifest = manifestLoader.load(context)

    fun findModel(id: String): ModelDescriptor? = manifestLoader.find(context, id)

    /** Verifies every declared file against its exact size and SHA-256. */
    suspend fun modelHealth(id: String, forceVerify: Boolean = false): HealthResult = withContext(Dispatchers.IO) {
        val descriptor = findModel(id)
            ?: return@withContext HealthResult(
                modelId = id,
                healthy = false,
                verified = false,
                missing = emptyList(),
                sizeMismatch = emptyList(),
                checksumMismatch = emptyList(),
                unverified = emptyList(),
                message = "Unknown model id"
            )

        val folder = resolvedFolder(descriptor)
        val missing = mutableListOf<String>()
        val sizeMismatch = mutableListOf<String>()
        val checksumMismatch = mutableListOf<String>()
        val unverified = mutableListOf<String>()

        for (file in descriptor.files) {
            if (file.archive) {
                val extractedName = file.extractsTo?.takeIf { it.isNotBlank() }
                    ?: file.name.substringBeforeLast('.')
                val extracted = File(folder, extractedName)
                if (!extracted.exists() || !extracted.isDirectory || extracted.listFiles().isNullOrEmpty()) {
                    missing += file.name
                } else if (!installer.archiveAlreadyExtracted(file, folder)) {
                    checksumMismatch += file.name
                }
                continue
            }

            val target = File(folder, file.name)
            if (!target.exists() || target.length() == 0L) {
                missing += file.name
                continue
            }
            if (file.sizeBytes == 0L || file.sha256.isBlank()) {
                unverified += file.name
                continue
            }
            if (target.length() != file.sizeBytes) {
                sizeMismatch += file.name
                continue
            }
            val verification = artifactVerifier.verify(
                descriptor,
                file,
                target,
                loadManifest().manifestVersion,
                force = forceVerify
            )
            if (!verification.verified) checksumMismatch += file.name
        }

        val healthy = missing.isEmpty() && sizeMismatch.isEmpty() && checksumMismatch.isEmpty()
        val verified = healthy && unverified.isEmpty()
        val message = when {
            !healthy -> "Needs repair (missing/size/hash mismatch)"
            unverified.isNotEmpty() -> "Present — integrity metadata incomplete; release blocked"
            else -> "Verified"
        }
        HealthResult(
            modelId = id,
            healthy = healthy,
            verified = verified,
            missing = missing,
            sizeMismatch = sizeMismatch,
            checksumMismatch = checksumMismatch,
            unverified = unverified,
            message = message
        )
    }

    suspend fun installModel(
        id: String,
        shouldCancel: () -> Boolean = { false },
        onProgress: ((
            modelId: String,
            fileIndex: Int,
            totalFiles: Int,
            file: String,
            downloaded: Long,
            total: Long
        ) -> Unit)? = null
    ): ModelInstaller.InstallResult {
        val descriptor = findModel(id)
            ?: return ModelInstaller.InstallResult.Failure("Unknown model id: $id")
        // Manager is a second boundary: direct callers cannot bypass the worker's Device Check.
        val gate = { runCatching {
            admission.check(descriptor).decision.let { if (it.allowed) null else it.reason }
        }.getOrElse { "Device Check unavailable; paused" } }
        gate()?.let { return ModelInstaller.InstallResult.Failure(it) }
        val result = installer.install(
            descriptor,
            onProgress?.let { callback ->
                ModelInstaller.ProgressListener { mid, fileIndex, totalFiles, file, downloaded, total ->
                    callback(mid, fileIndex, totalFiles, file, downloaded, total)
                }
            },
            shouldCancel = shouldCancel,
            admission = gate
        )
        // Staged is deliberately NOT active and must not seed the live verification cache.
        return result
    }

    /** Deletes only a manifest-resolved folder beneath the app-private models root. */
    suspend fun uninstallModel(id: String) = withContext(Dispatchers.IO) {
        val descriptor = findModel(id)
            ?: run {
                Logger.w("ModelManager: refusing uninstall for unknown model '$id'")
                return@withContext
            }
        val base = File(appPrivateModelPath)
        val folder = File(base, descriptor.folder)
        if (!isSafeChild(base, folder)) {
            Logger.w("ModelManager: refusing to uninstall '$id' outside models root (${folder.canonicalPath})")
            return@withContext
        }
        deleteDirectoryContents(folder)
        // Explicit user uninstall also removes every versioned bundle of this id (pointer first).
        runCatching { bundles.uninstallAll(id) }.onFailure { Logger.w("ModelManager: bundle uninstall incomplete for '$id': ${it.message}") }
        runCatching { admission.revoke(descriptor) }
        artifactVerifier.invalidate(id)
        modelMetadataDao?.deleteByName(id)
        Logger.i("ModelManager: uninstalled $id")
    }

    /**
     * Preserves the legacy E2B artifact and reports whether it is present.
     *
     * This deprecated compatibility API always returns removed=false: checksum verification alone
     * is not deletion qualification and cannot authorize removing the legacy fallback.
     */
    @Deprecated("Integrity alone is not a deletion qualification")
    suspend fun removeLegacyE2BIfE4BVerified(): LegacyCleanupResult = withContext(Dispatchers.IO) {
        LegacyCleanupResult(
            removed = false,
            legacyPresent = legacyE2BPresent(),
            message = "Legacy cleanup is qualification-gated; checksum verification alone cannot remove E2B"
        )
    }

    suspend fun removeLegacyE2BIfQualified(userApproved: Boolean): LegacyCleanupResult = withContext(Dispatchers.IO) {
        val rejection = E4bCleanupGate.rejectionReason(readQualificationRecord(), userApproved)
        if (rejection != null) {
            return@withContext LegacyCleanupResult(
                removed = false,
                legacyPresent = legacyE2BFolder().exists(),
                message = "$rejection; legacy brain was preserved"
            )
        }

        // Qualification is not yet backed by release-device evidence; preserve the legacy fallback.
        LegacyCleanupResult(false, legacyE2BPresent(), "Legacy deletion disabled pending device qualification")
    }

    fun saveQualificationRecord(record: E4bQualificationRecord) {
        val json = Json.encodeToString(E4bQualificationRecord.serializer(), record)
        context.getSharedPreferences(QUALIFICATION_PREFS, Context.MODE_PRIVATE)
            .edit().putString(KEY_E4B_QUALIFICATION, json).apply()
    }

    fun readQualificationRecord(): E4bQualificationRecord? {
        val raw = context.getSharedPreferences(QUALIFICATION_PREFS, Context.MODE_PRIVATE)
            .getString(KEY_E4B_QUALIFICATION, null) ?: return null
        return runCatching { Json.decodeFromString(E4bQualificationRecord.serializer(), raw) }
            .getOrNull()
    }

    fun legacyE2BPresent(): Boolean = legacyE2BFolder().let { folder ->
        folder.isDirectory && folder.walkTopDown().any { it.isFile && it.length() > 0L }
    }

    suspend fun detectModels(): List<ModelStatus> = withContext(Dispatchers.IO) {
        val base = File(appPrivateModelPath)
        if (!base.exists()) base.mkdirs()

        val manifest = loadManifest()
        manifest.models.map { descriptor ->
            val folder = resolvedFolder(descriptor)
            val hasRealFile = folder.walkTopDown().any { file ->
                file.isFile && !file.name.endsWith(".part") && file.length() > 0L
            }
            val present = folder.exists() && folder.isDirectory && hasRealFile
            val sizeMb = if (present) {
                folder.walkTopDown().filter { it.isFile }.sumOf { it.length() } / (1024 * 1024)
            } else 0L
            val health = if (present) modelHealth(descriptor.id) else null
            ModelStatus(
                name = descriptor.folder,
                type = descriptor.type.name,
                present = present,
                loaded = false,
                sizeMb = sizeMb,
                version = descriptor.version,
                expectedSha256 = descriptor.files.firstOrNull()?.sha256.orEmpty(),
                healthy = health?.healthy ?: false,
                verified = health?.verified ?: false
            )
        }.also { models ->
            Logger.d("Detected ${models.count { it.present }} models present out of ${models.size}")
        }
    }

    suspend fun verifyChecksum(path: String, expected: String): Boolean = withContext(Dispatchers.IO) {
        val file = File(path)
        expected.matches(Regex("^[a-fA-F0-9]{64}$")) &&
            file.exists() &&
            computeSha256(path) == expected.lowercase()
    }

    private fun computeSha256(path: String): String? {
        return try {
            val file = File(path)
            if (!file.exists()) {
                null
            } else {
                val digest = MessageDigest.getInstance("SHA-256")
                file.inputStream().use { input ->
                    val buffer = ByteArray(8192)
                    while (true) {
                        val read = input.read(buffer)
                        if (read <= 0) break
                        digest.update(buffer, 0, read)
                    }
                }
                digest.digest().joinToString("") { "%02x".format(it) }
            }
        } catch (e: Exception) {
            Logger.e("Checksum computation failed for $path", e)
            null
        }
    }

    fun getStorageUsageMb(): Long {
        val base = File(appPrivateModelPath)
        if (!base.exists()) return 0L
        return base.walkTopDown().filter { it.isFile }.sumOf { it.length() } / (1024 * 1024)
    }

    fun getModelFolderPath(modelName: String): String =
        loadManifest().findByFolder(modelName)?.let { resolvedFolder(it).absolutePath }
            ?: File(appPrivateModelPath, modelName).absolutePath

    /** Creates only declared model folders plus non-model runtime directories. */
    fun ensureModelDirectories() {
        val base = File(appPrivateModelPath)
        val manifestFolders = loadManifest().models.map { it.folder }
        (manifestFolders + RUNTIME_DIRECTORIES).distinct().forEach { File(base, it).mkdirs() }
    }

    /**
     * Materializes the optional wake/VAD folder from the verified English ASR files when the
     * manifest declares both models as the exact same artifacts. This avoids a redundant network
     * download while still leaving the dedicated model folder independently verifiable.
     */
    suspend fun repairKwsFromVerifiedEnglishAsr(): Boolean = withContext(Dispatchers.IO) {
        val source = findModel("sherpa-asr-en") ?: return@withContext false
        val target = findModel("sherpa-kws-en") ?: return@withContext false
        val manifestsIdentical = KwsAliasRepair.manifestsIdentical(source, target)
        if (!manifestsIdentical) {
            Logger.w("ModelManager: refusing KWS alias repair because manifests differ")
            return@withContext false
        }
        if (modelHealth(target.id).verified) return@withContext true
        if (!modelHealth(source.id).verified) {
            Logger.w("ModelManager: cannot repair KWS; English ASR source is not verified")
            return@withContext false
        }

        // Bundle-pointer targets need the transactional bundle contract; the legacy flat KWS folder
        // uses the per-file verified copy (.part → size+SHA-256 → rename) exactly as standalone did.
        if (bundles.hasPointer(target.id)) {
            Logger.w("ModelManager: KWS alias repair skipped; target already uses a versioned bundle pointer")
            return@withContext false
        }
        val targetFolder = File(appPrivateModelPath, target.folder)
        val base = File(appPrivateModelPath)
        if (!isSafeChild(base, targetFolder)) return@withContext false
        val copied = KwsAliasRepair.copyVerified(resolvedFolder(source), targetFolder, target.files)
        if (!copied) {
            Logger.w("ModelManager: KWS alias repair did not complete; partial files removed, verified files retained")
            return@withContext false
        }
        val repaired = modelHealth(target.id, forceVerify = true).verified
        if (repaired) Logger.i("ModelManager: installed verified KWS wake pack from English ASR")
        repaired
    }

    suspend fun getLlmModelPath(): String? = getLlmModelPath(BrainModelRegistry.defaultProfile)

    /**
     * Returns only the exact, manifest-declared, integrity-verified model file.
     *
     * There is deliberately no "largest .litertlm" fallback: a stale E2B file, web artifact,
     * incomplete copy or manually dropped model must never be selected as UnoOne's brain.
     */
    suspend fun getLlmModelPath(spec: BrainModelSpec, forceVerify: Boolean = false): String? {
        if (spec.runtime != com.unoone.agent.core.model.BrainRuntime.LITERT_LM) return null
        val descriptor = findModel(spec.manifestId) ?: return null
        val artifact = descriptor.files.singleOrNull { file ->
            !file.archive && file.name.equals(spec.fileName, ignoreCase = false)
        } ?: return null
        if (descriptor.folder != spec.modelFolder || artifact.sha256.isBlank() || artifact.sizeBytes <= 0L) {
            return null
        }

        val folder = resolvedFolder(descriptor)
        val exact = File(folder, spec.fileName)
        if (!exact.isFile || exact.name.endsWith(".part") || exact.length() != artifact.sizeBytes) return null
        val verified = artifactVerifier.verify(
            descriptor,
            artifact,
            exact,
            loadManifest().manifestVersion,
            force = forceVerify
        )
        if (!verified.verified) return null
        return exact.absolutePath
    }

    /** MNN consumes a complete verified folder, never a single LiteRT model path. */
    suspend fun getMnnModelFolder(spec: BrainModelSpec): String? {
        if (spec.runtime != com.unoone.agent.core.model.BrainRuntime.MNN) return null
        val descriptor = findModel(spec.manifestId) ?: return null
        if (descriptor.folder != spec.modelFolder || !modelHealth(spec.manifestId).verified) return null
        return resolvedFolder(descriptor).absolutePath
    }

    data class VerifiedGgufArtifacts(val root: String, val decoderPath: String, val projectorPath: String)

    /** Exact pinned pair only; no glob discovery, partial activation or single-file readiness. */
    suspend fun getGgufModelArtifacts(spec: BrainModelSpec, forceVerify: Boolean = false): VerifiedGgufArtifacts? {
        if (spec != BrainModelRegistry.GUI_OWL_1_5_4B_INSTRUCT) return null
        val identity = com.unoone.agent.core.model.GuiOwlArtifact
        val descriptor = findModel(spec.manifestId) ?: return null
        if (descriptor.folder != identity.FOLDER || descriptor.files.size != 2) return null
        val expected = listOf(
            Triple(identity.DECODER, identity.DECODER_BYTES, identity.DECODER_SHA256),
            Triple(identity.PROJECTOR, identity.PROJECTOR_BYTES, identity.PROJECTOR_SHA256)
        )
        val root = resolvedFolder(descriptor)
        if (!isSafeChild(File(appPrivateModelPath), root)) return null
        for ((name, size, hash) in expected) {
            val entry = descriptor.files.singleOrNull { it.name == name } ?: return null
            if (entry.archive || entry.asset != null || entry.sizeBytes != size || entry.sha256 != hash ||
                entry.url != identity.url(name)) return null
            val file = File(root, name)
            if (!isSafeChild(root, file) || !file.isFile || file.length() != size) return null
        }
        if (!modelHealth(spec.manifestId, forceVerify = forceVerify).verified) return null
        return VerifiedGgufArtifacts(root.absolutePath, File(root, identity.DECODER).absolutePath,
            File(root, identity.PROJECTOR).absolutePath)
    }

    /** User/developer explicit verification always performs a complete SHA-256 pass. */
    suspend fun verifyLlmArtifact(spec: BrainModelSpec = BrainModelRegistry.defaultProfile): HealthResult {
        return modelHealth(spec.manifestId, forceVerify = true)
    }

    private fun legacyE2BFolder(): File = File(appPrivateModelPath, LEGACY_E2B_RELATIVE_FOLDER)

    private fun isSafeChild(base: File, candidate: File): Boolean {
        val baseCanonical = base.canonicalFile
        val candidateCanonical = candidate.canonicalFile
        return candidateCanonical != baseCanonical &&
            candidateCanonical.path.startsWith(baseCanonical.path + File.separator)
    }

    private fun deleteDirectoryContents(folder: File) {
        if (!folder.exists()) return
        folder.walkTopDown().sortedByDescending { it.path }.forEach { file ->
            runCatching { file.delete() }
        }
        runCatching { folder.delete() }
    }

    data class ModelStatus(
        val name: String,
        val type: String,
        val present: Boolean,
        val loaded: Boolean,
        val sizeMb: Long,
        val version: String = "",
        val expectedSha256: String = "",
        val healthy: Boolean = false,
        val verified: Boolean = false
    )

    data class HealthResult(
        val modelId: String,
        val healthy: Boolean,
        val verified: Boolean = false,
        val missing: List<String>,
        val sizeMismatch: List<String>,
        val checksumMismatch: List<String>,
        val unverified: List<String> = emptyList(),
        val message: String
    )

    data class LegacyCleanupResult(
        val removed: Boolean,
        val legacyPresent: Boolean,
        val message: String
    )

    companion object {
        private const val LEGACY_E2B_RELATIVE_FOLDER = "brain/gemma-4-e2b"
        private const val QUALIFICATION_PREFS = "e4b_device_qualification_v1"
        private const val KEY_E4B_QUALIFICATION = "qualified_record"
        private val RUNTIME_DIRECTORIES: List<String> = listOf(
            "vision/blind-aid",
            "staging"
        )
    }
}
