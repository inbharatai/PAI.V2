package com.unoone.agent.core.modeladmission

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.io.File
import java.io.FileOutputStream
import java.nio.file.Files
import java.nio.file.StandardCopyOption

/**
 * Device-local evidence that THIS device actually ran a native load plus a bounded real smoke
 * generation for one exact model identity. A receipt is "Works here" evidence — unsigned, local,
 * never uploaded — and is distinct from a signed physical-device qualification.
 *
 * Receipts are produced only by the native adapters (boot load, explicit load, staged activation)
 * after a real `loadLlmModel` + smoke. UI, sync, WorkManager and manifest data can never mint one.
 */
@Serializable
data class NativeLoadReceipt(
    val schemaVersion: Int = 1,
    val modelId: String,
    /** SHA-256 of the catalogue descriptor (same identity the bundle store binds). */
    val identity: String,
    /** Local device fingerprint hash (build fingerprint + total RAM + ABI). Never uploaded. */
    val deviceFingerprint: String,
    val outcome: Outcome,
    /** Free-form native reason on failure (OOM / BAD_LOAD / BAD_SMOKE / CANCELLED). */
    val reason: String = "",
    val backend: String = "",
    val loadMs: Long = 0,
    val smokeMs: Long = 0,
    val smokeTokens: Int = 0,
    /** Measured total/available RAM at record time, when the probe measured them. */
    val totalRamBytes: Long? = null,
    val availableRamBytesBefore: Long? = null,
    val recordedAtMs: Long,
    /** Which lane produced it: BOOT, EXPLICIT_LOAD, ACTIVATION, SELF_TEST. */
    val lane: String
) {
    enum class Outcome { PASSED, FAILED }
    val passed: Boolean get() = outcome == Outcome.PASSED
}

/** Evidence available to [LoadAdmission.decide]. */
data class LoadEvidence(
    /** Most recent receipt for (modelId, identity, this device fingerprint), or null. */
    val latestReceipt: NativeLoadReceipt? = null,
    /** Any PASSED receipt exists for this exact identity on this device (even if a later one failed). */
    val everPassedHere: Boolean = false,
    /** Model resolves from the pre-receipt legacy flat layout (installed before receipts existed). */
    val legacyInstalled: Boolean = false,
    /** Signed physical-device qualification record matched (production trust set is currently empty). */
    val signedQualification: Boolean = false
) {
    companion object { val NONE = LoadEvidence() }
}

interface ReceiptStore {
    fun record(receipt: NativeLoadReceipt)
    fun receipts(modelId: String, identity: String, deviceFingerprint: String): List<NativeLoadReceipt>
    fun evidence(modelId: String, identity: String, deviceFingerprint: String, legacyInstalled: Boolean = false): LoadEvidence {
        val rows = receipts(modelId, identity, deviceFingerprint)
        return LoadEvidence(
            latestReceipt = rows.maxByOrNull { it.recordedAtMs },
            everPassedHere = rows.any { it.passed },
            legacyInstalled = legacyInstalled
        )
    }
}

/**
 * Append-only JSON-lines receipt log under `<root>/.receipts-v1/<modelId>/<identity>.jsonl`.
 * Writes are fsync'ed and atomically replaced; a corrupt line is skipped, never "repaired".
 */
class FileReceiptStore(private val root: File) : ReceiptStore {
    private val json = Json { ignoreUnknownKeys = true }
    private val idPattern = Regex("[A-Za-z0-9][A-Za-z0-9._-]{0,159}")
    private val hexPattern = Regex("[a-f0-9]{64}")

    private fun file(modelId: String, identity: String): File {
        require(idPattern.matches(modelId)) { "Bad model id" }
        require(hexPattern.matches(identity)) { "Bad identity" }
        val dir = File(root, ".receipts-v1/$modelId")
        check(dir.isDirectory || dir.mkdirs())
        require(dir.canonicalPath.startsWith(root.canonicalPath + File.separator))
        return File(dir, "$identity.jsonl")
    }

    @Synchronized
    override fun record(receipt: NativeLoadReceipt) {
        require(hexPattern.matches(receipt.deviceFingerprint)) { "Bad fingerprint" }
        val target = file(receipt.modelId, receipt.identity)
        require(!Files.isSymbolicLink(target.toPath()))
        val existing = if (target.isFile) target.readText() else ""
        val tmp = File(target.parentFile, "${target.name}.${System.nanoTime()}.tmp")
        FileOutputStream(tmp).use {
            it.write((existing + json.encodeToString(NativeLoadReceipt.serializer(), receipt) + "\n").toByteArray(Charsets.UTF_8))
            it.fd.sync()
        }
        Files.move(tmp.toPath(), target.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
    }

    @Synchronized
    override fun receipts(modelId: String, identity: String, deviceFingerprint: String): List<NativeLoadReceipt> {
        val target = file(modelId, identity)
        if (!target.isFile || Files.isSymbolicLink(target.toPath())) return emptyList()
        return target.readLines().filter { it.isNotBlank() }.mapNotNull { line ->
            runCatching { json.decodeFromString(NativeLoadReceipt.serializer(), line) }.getOrNull()
        }.filter { it.modelId == modelId && it.identity == identity && it.deviceFingerprint == deviceFingerprint }
    }
}
