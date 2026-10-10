package com.unoone.agent.ui.viewmodel

import android.content.Context
import android.net.ConnectivityManager
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.Observer
import androidx.work.Constraints
import androidx.work.BackoffPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.workDataOf
import java.util.concurrent.TimeUnit
import com.unoone.agent.resolveBrainLoadPath
import com.unoone.agent.owlLoadAdmissionError
import com.unoone.agent.core.model.BrainRuntime
import com.unoone.agent.core.model.BrainExperimentalConsent
import com.unoone.agent.core.model.experimentalConsent
import com.unoone.agent.UnoOneApplication
import com.unoone.agent.core.model.BrainModelSpec
import com.unoone.agent.AgentOrchestrator
import com.unoone.agent.brain.BrainSelfTest
import com.unoone.agent.brain.BrainSelfTestResult
import com.unoone.agent.core.model.BrainModelRegistry
import com.unoone.agent.core.model.Result
import com.unoone.agent.modelmanager.ModelManager
import com.unoone.agent.model.ModelDownloadWorker
import com.unoone.agent.model.NativeBrainPort
import com.unoone.agent.model.StagedActivation
import com.unoone.agent.core.modeladmission.LoadAdmission
import com.unoone.agent.core.modeladmission.NativeLoadReceipt
import java.io.File
import com.unoone.agent.core.runtime.AgentRuntimeGate
import com.unoone.agent.storage.dao.ModelMetadataDao
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Drives model installation, health and the selected Gemma brain card. */
class ModelStatusViewModel(
    context: Context,
    modelMetadataDao: ModelMetadataDao? = null,
    private val orchestrator: AgentOrchestrator? = null
) : ViewModel() {

    private val appContext = context.applicationContext
    private val application = appContext as UnoOneApplication
    private val modelManager = ModelManager(appContext, modelMetadataDao)
    private val brainSelfTest = orchestrator?.let { BrainSelfTest(it, modelManager) }
    private val workManager = WorkManager.getInstance(appContext)

    data class ModelRow(
        val id: String,
        val folder: String,
        val type: String,
        val version: String,
        val present: Boolean,
        val healthy: Boolean,
        val verified: Boolean,
        val sizeMb: Long,
        val backend: String,
        val minRamMb: Int,
        val language: String,
        val sha256Preview: String,
        val healthMessage: String,
        /** A sealed-but-not-active bundle exists for this id. */
        val staged: Boolean = false,
        /** Three honest states: Qualified / Works here / Unknown. Unknown is NOT refused. */
        val evidence: LoadAdmission.EvidenceState = LoadAdmission.EvidenceState.UNKNOWN,
        val evidenceLabel: String = ""
    )

    data class BrainStatusRow(
        val manifestId: String,
        val displayName: String,
        val isDeviceVerified: Boolean,
        val minimumRamMb: Int,
        val recommendedRamMb: Int,
        val installed: Boolean,
        val isLoaded: Boolean,
        val backend: String,
        val lastLoadError: String,
        val description: String,
        val runtime: String,
        val artifactSummary: String,
        val runtimeStatus: String,
        val evidence: LoadAdmission.EvidenceState = LoadAdmission.EvidenceState.UNKNOWN,
        val evidenceLabel: String = "",
        val lastReceipt: String = ""
    )

    data class InstallProgress(
        val modelId: String,
        val file: String,
        val fileIndex: Int,
        val totalFiles: Int,
        val percent: Int,
        val active: Boolean,
        val message: String
    )

    private val _rows = MutableStateFlow<List<ModelRow>>(emptyList())
    val rows: StateFlow<List<ModelRow>> = _rows.asStateFlow()

    private val _profiles = MutableStateFlow<List<BrainStatusRow>>(emptyList())
    val profiles: StateFlow<List<BrainStatusRow>> = _profiles.asStateFlow()

    private val _brainStatus = MutableStateFlow<BrainStatusRow?>(null)
    val brainStatus: StateFlow<BrainStatusRow?> = _brainStatus.asStateFlow()

    private val _selfTest = MutableStateFlow<BrainSelfTestResult?>(null)
    val selfTest: StateFlow<BrainSelfTestResult?> = _selfTest.asStateFlow()

    private val _brainBusy = MutableStateFlow(false)
    val brainBusy: StateFlow<Boolean> = _brainBusy.asStateFlow()

    private val _verifying = MutableStateFlow(false)
    val verifying: StateFlow<Boolean> = _verifying.asStateFlow()

    private val _progress = MutableStateFlow<InstallProgress?>(null)
    val progress: StateFlow<InstallProgress?> = _progress.asStateFlow()

    private val _storageUsageMb = MutableStateFlow(0L)
    val storageUsageMb: StateFlow<Long> = _storageUsageMb.asStateFlow()

    private val _resultMessage = MutableStateFlow<String?>(null)
    val resultMessage: StateFlow<String?> = _resultMessage.asStateFlow()

    private val _busy = MutableStateFlow(false)
    val busy: StateFlow<Boolean> = _busy.asStateFlow()

    private val _pendingExperimentalSelection = MutableStateFlow<String?>(null)
    val pendingExperimentalSelection: StateFlow<String?> = _pendingExperimentalSelection.asStateFlow()

    fun dismissExperimentalSelection() { _pendingExperimentalSelection.value = null }
    fun confirmExperimentalSelection() {
        val id = _pendingExperimentalSelection.value ?: return
        _pendingExperimentalSelection.value = null
        selectBrain(id, experimentalConsent = true)
    }

    private val _pendingMeteredInstall = MutableStateFlow<String?>(null)
    val pendingMeteredInstall: StateFlow<String?> = _pendingMeteredInstall.asStateFlow()

    private val downloadObserver = Observer<List<WorkInfo>> { infos ->
        val active = infos.lastOrNull { !it.state.isFinished }
        val latest = active ?: infos.lastOrNull()
        _busy.value = active != null
        if (latest == null) return@Observer
        val modelId = latest.progress.getString(ModelDownloadWorker.KEY_MODEL_ID)
            ?: latest.outputData.getString(ModelDownloadWorker.KEY_MODEL_ID)
            ?: latest.tags.firstOrNull { it.startsWith(ModelDownloadWorker.MODEL_TAG_PREFIX) }
                ?.removePrefix(ModelDownloadWorker.MODEL_TAG_PREFIX)
            ?: "model"
        when (latest.state) {
            WorkInfo.State.ENQUEUED, WorkInfo.State.BLOCKED -> {
                _progress.value = InstallProgress(modelId, "", 0, 1, 0, true, "Waiting for allowed network…")
            }
            WorkInfo.State.RUNNING -> {
                val percent = latest.progress.getInt(ModelDownloadWorker.KEY_PERCENT, 0)
                val file = latest.progress.getString(ModelDownloadWorker.KEY_FILE).orEmpty()
                val fileIndex = latest.progress.getInt(ModelDownloadWorker.KEY_FILE_INDEX, 0)
                val totalFiles = latest.progress.getInt(ModelDownloadWorker.KEY_TOTAL_FILES, 1)
                _progress.value = InstallProgress(
                    modelId, file, fileIndex, totalFiles, percent, true,
                    "Downloading $file ($percent%) — file ${fileIndex + 1}/$totalFiles"
                )
            }
            WorkInfo.State.SUCCEEDED -> {
                _progress.value = null
                _resultMessage.value = "Staged: $modelId — verifying with a real native load + smoke before activation; existing model retained."
                if (latest.id != lastActivatedWork) { lastActivatedWork = latest.id; activateStaged(modelId) }
                refresh()
            }
            WorkInfo.State.FAILED -> {
                _progress.value = null
                _resultMessage.value = "Install failed: ${latest.outputData.getString(ModelDownloadWorker.KEY_ERROR) ?: "unknown error"}"
            }
            WorkInfo.State.CANCELLED -> {
                _progress.value = null
                _resultMessage.value = "Download cancelled; partial data kept for resume."
            }
        }
    }

    private var lastActivatedWork: java.util.UUID? = null
    private val _activation = MutableStateFlow<StagedActivation.State?>(null)
    /** Download → Staged → Verifying → Active (or Failed with the previous model retained). */
    val activation: StateFlow<StagedActivation.State?> = _activation.asStateFlow()
    fun dismissActivation() { _activation.value = null }

    /**
     * Production caller of `activateStaged`: real native load of the staged bundle, bounded real
     * smoke generation, then the atomic pointer switch. Failure keeps the old active bundle and the
     * previously resident model; the reason is shown, never hidden behind "Staged".
     */
    fun activateStaged(modelId: String) {
        if (_brainBusy.value) { _resultMessage.value = "Brain is busy; activation deferred."; return }
        val spec = BrainModelRegistry.byManifestId(modelId)
        if (spec == null) {
            _activation.value = StagedActivation.State(modelId, StagedActivation.Phase.STAGED,
                "Staged. Speech packs activate through Offline Languages; no native brain smoke applies.")
            return
        }
        val runtime = orchestrator
        if (runtime == null) {
            _activation.value = StagedActivation.State(modelId, StagedActivation.Phase.STAGED, "Staged; native runtime unavailable in this screen.")
            return
        }
        if (!application.brainProviderPreferences.hasConsent(spec)) {
            _pendingExperimentalSelection.value = spec.manifestId
            _activation.value = StagedActivation.State(modelId, StagedActivation.Phase.STAGED, "Staged; experimental consent required before the native smoke.")
            return
        }
        _brainBusy.value = true
        viewModelScope.launch {
            val state = withContext(Dispatchers.IO) {
                val bundle = modelManager.stagedBundle(modelId)
                    ?: return@withContext StagedActivation.State(modelId, StagedActivation.Phase.FAILED, "No sealed staged bundle found.")
                val port = NativeBrainPort(runtime, spec)
                StagedActivation.run(
                    bundle = bundle,
                    port = port,
                    resolveLoadPath = { root: File -> modelManager.loadPathWithin(root, spec) },
                    record = { r -> modelManager.recordNativeLoad(modelId, r.outcome, "ACTIVATION", r.reason, r.backend, r.loadMs, r.smokeMs, r.smokeTokens) },
                    activate = { staged, loader -> modelManager.activateStaged(staged, loader) },
                    onState = { _activation.value = it }
                )
            }
            _activation.value = state
            _brainBusy.value = false
            _resultMessage.value = when (state.phase) {
                StagedActivation.Phase.ACTIVE -> "${spec.displayName} is Active — ${state.detail}"
                StagedActivation.Phase.FAILED -> "${spec.displayName} activation failed; previous model retained — ${state.detail}"
                else -> state.detail
            }
            refresh()
        }
    }

    init {
        workManager.getWorkInfosByTagLiveData(ModelDownloadWorker.TAG).observeForever(downloadObserver)
        refresh()
    }

    fun refresh() {
        viewModelScope.launch {
            _rows.value = withContext(Dispatchers.IO) { buildRows() }
            val selected = application.resolveSelectedBrain()
            _profiles.value = withContext(Dispatchers.IO) { BrainModelRegistry.all.map { buildBrainStatus(it) } }
            _brainStatus.value = _profiles.value.first { it.manifestId == selected.manifestId }
            _storageUsageMb.value = withContext(Dispatchers.IO) { modelManager.getStorageUsageMb() }
        }
    }

    private val _pendingDeviceCheck = MutableStateFlow<com.unoone.agent.modelmanager.AndroidModelAdmission.Check?>(null)
    val pendingDeviceCheck = _pendingDeviceCheck.asStateFlow()
    private var pendingLoadCheck = false
    fun dismissDeviceCheck() { _pendingDeviceCheck.value = null; pendingLoadCheck = false }
    fun installModel(id: String) {
        if (_busy.value) return
        viewModelScope.launch {
            pendingLoadCheck = false
            _pendingDeviceCheck.value = withContext(Dispatchers.IO) { runCatching { modelManager.deviceCheck(id) }.getOrNull() }
            if (_pendingDeviceCheck.value == null) _resultMessage.value = "Device Check unavailable; download paused."
        }
    }
    fun confirmDeviceCheck() {
        val check = _pendingDeviceCheck.value ?: return
        val id = check.profile.modelId
        _pendingDeviceCheck.value = null
        if (!check.physicalPreview.allowed) { _resultMessage.value = "Device Check denied: ${check.physicalPreview.reason}"; return }
        if (pendingLoadCheck) {
            pendingLoadCheck = false
            if (modelManager.approveManualInstall(id, false)) loadBrain()
            else _resultMessage.value = "Risk policy not saved; load paused."
            return
        }
        val connectivity = appContext.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        if (connectivity.isActiveNetworkMetered) { _pendingMeteredInstall.value = id; return }
        if (!modelManager.approveManualInstall(id, false)) { _resultMessage.value = "Risk policy not saved; paused."; return }
        enqueueModelInstall(id, allowMetered = false)
    }

    fun confirmMeteredInstall() {
        val id = _pendingMeteredInstall.value ?: return
        _pendingMeteredInstall.value = null
        if (!modelManager.approveManualInstall(id, true)) { _resultMessage.value = "Risk policy not saved; paused."; return }
        enqueueModelInstall(id, allowMetered = true)
    }

    fun dismissMeteredInstall() {
        _pendingMeteredInstall.value = null
        _resultMessage.value = "Download not started. Connect to Wi-Fi and try again."
    }

    fun cancelInstall() {
        _progress.value?.modelId?.let { runCatching { modelManager.revokeManualInstall(it) } }
        workManager.cancelUniqueWork(ModelDownloadWorker.UNIQUE_WORK)
    }

    private fun enqueueModelInstall(id: String, allowMetered: Boolean) {
        val decision = runCatching { modelManager.deviceCheck(id).decision }.getOrNull()
        if (decision?.allowed != true) { _resultMessage.value = "Download paused: ${decision?.reason ?: "unknown probe"}"; return }
        _resultMessage.value = null
        val request = OneTimeWorkRequestBuilder<ModelDownloadWorker>()
            .setInputData(workDataOf(
                ModelDownloadWorker.KEY_MODEL_ID to id,
                ModelDownloadWorker.KEY_ALLOW_METERED to allowMetered
            ))
            .setConstraints(
                Constraints.Builder()
                    .setRequiredNetworkType(if (allowMetered) NetworkType.CONNECTED else NetworkType.UNMETERED)
                    .build()
            )
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS)
            .addTag(ModelDownloadWorker.TAG)
            .addTag("${ModelDownloadWorker.MODEL_TAG_PREFIX}$id")
            .build()
        workManager.enqueueUniqueWork(
            ModelDownloadWorker.UNIQUE_WORK,
            ExistingWorkPolicy.KEEP,
            request
        )
    }

    fun uninstallModel(id: String) {
        if (_busy.value) return
        _busy.value = true
        _resultMessage.value = null
        viewModelScope.launch {
            withContext(Dispatchers.IO) { modelManager.uninstallModel(id) }
            _busy.value = false
            _resultMessage.value = "Uninstalled: $id"
            refresh()
        }
    }

    fun selectBrain(manifestId: String, experimentalConsent: Boolean = false) {
        if (_brainBusy.value || _verifying.value) return
        val spec = BrainModelRegistry.byManifestId(manifestId) ?: return
        if (spec.experimentalConsent() != BrainExperimentalConsent.NONE && !experimentalConsent) {
            _pendingExperimentalSelection.value = manifestId
            return
        }
        _brainBusy.value = true
        viewModelScope.launch {
            val result = application.selectBrainProfile(spec, experimentalConsent)
            _brainBusy.value = false
            _selfTest.value = null
            _resultMessage.value = when (result) {
                is Result.Success -> "Selected ${spec.displayName}. Load it explicitly; other profiles are retained."
                is Result.Error -> result.message
            }
            refresh()
        }
    }

    fun loadBrain() {
        if (_brainBusy.value) return
        val spec = _brainStatus.value?.manifestId?.let(BrainModelRegistry::byManifestId) ?: return
        if (!application.brainProviderPreferences.hasConsent(spec)) {
            _pendingExperimentalSelection.value = spec.manifestId
            return
        }
        val admissionError = modelManager.beforeNativeLoad(spec.manifestId)
        if (admissionError != null) {
            pendingLoadCheck = true
            _pendingDeviceCheck.value = runCatching { modelManager.deviceCheck(spec.manifestId, loading = true) }.getOrNull()
            _resultMessage.value = "Load paused: $admissionError"
            return
        }
        if (orchestrator == null) {
            _resultMessage.value = "${spec.displayName} will load automatically when the app starts and the artifact is healthy."
            return
        }
        _brainBusy.value = true
        _resultMessage.value = null
        viewModelScope.launch {
            val path = withContext(Dispatchers.IO) { modelManager.resolveBrainLoadPath(spec) }
            if (path == null) {
                _brainBusy.value = false
                _resultMessage.value = "${spec.displayName} is not installed. Add the integrity-verified artifact first."
                refresh()
                return@launch
            }
            appContext.owlLoadAdmissionError(spec)?.let {
                _brainBusy.value = false
                _resultMessage.value = it
                return@launch
            }
            val result = withContext(Dispatchers.IO) {
                val fresh = modelManager.beforeNativeLoad(spec.manifestId, LoadAdmission.Purpose.EXPLICIT_LOAD)
                if (fresh != null) Result.Error("Load paused: $fresh")
                else NativeBrainPort.loadAndRecord(orchestrator, modelManager, spec, path, lane = "EXPLICIT_LOAD")
            }
            _brainBusy.value = false
            _resultMessage.value = if (result is Result.Success) {
                "${spec.displayName} loaded on ${orchestrator.loadedBrainBackend()}."
            } else {
                "${spec.displayName} failed to load: ${(result as? Result.Error)?.message}"
            }
            refresh()
        }
    }

    fun runBrainSelfTest() {
        val selected = _brainStatus.value?.manifestId?.let(BrainModelRegistry::byManifestId) ?: return
        if (!application.brainProviderPreferences.hasConsent(selected)) {
            _pendingExperimentalSelection.value = selected.manifestId
            return
        }
        val test = brainSelfTest
        if (test == null || _brainBusy.value) return
        val spec = _brainStatus.value?.manifestId?.let(BrainModelRegistry::byManifestId) ?: return
        _brainBusy.value = true
        _resultMessage.value = null
        _selfTest.value = null
        viewModelScope.launch {
            val result = try {
                withContext(Dispatchers.IO) {
                    check(modelManager.beforeNativeLoad(spec.manifestId) == null) { "Fresh Device Check denied self-test load" }
                    val outcome = if (spec.id == com.unoone.agent.core.model.BrainModelId.GUI_OWL_1_5_4B_INSTRUCT)
                        checkNotNull(orchestrator) { "Owl runtime unavailable" }.runOwlSelfTest(spec)
                    else test.run(spec)
                    // A self-test is a real native load + real generation: record it as device evidence.
                    if (outcome.installed) modelManager.recordNativeLoad(spec.manifestId,
                        if (outcome.loaded && outcome.toolAccepted) NativeLoadReceipt.Outcome.PASSED else NativeLoadReceipt.Outcome.FAILED,
                        lane = "SELF_TEST",
                        reason = when { !outcome.loaded -> "BAD_LOAD:${outcome.loadError}"; !outcome.toolAccepted -> "BAD_SMOKE:probes rejected"; else -> "" },
                        backend = outcome.backend, loadMs = outcome.elapsedMs, smokeTokens = outcome.probes.count { it.passed })
                    outcome
                }
            } catch (cancel: kotlinx.coroutines.CancellationException) { throw cancel }
            catch (_: Exception) { _resultMessage.value = "Self-test unavailable: load the selected brain and ensure no exclusive mode is active."; return@launch }
            finally { _brainBusy.value = false }
            _selfTest.value = result
            _brainBusy.value = false
            _resultMessage.value = result.message
            refresh()
        }
    }

    fun verifyBrainArtifact() {
        if (_brainBusy.value || _verifying.value) return
        val spec = _brainStatus.value?.manifestId?.let(BrainModelRegistry::byManifestId) ?: return
        _verifying.value = true
        _resultMessage.value = "Verifying the complete ${spec.displayName} artifact set…"
        viewModelScope.launch {
            val health = withContext(Dispatchers.IO) {
                modelManager.verifyLlmArtifact(spec)
            }
            _verifying.value = false
            _resultMessage.value = if (health.verified) {
                "${spec.displayName} size and SHA-256 are verified."
            } else {
                "${spec.displayName} verification failed: ${health.message}"
            }
            refresh()
        }
    }

    fun consumeResultMessage() {
        _resultMessage.value = null
    }

    override fun onCleared() {
        workManager.getWorkInfosByTagLiveData(ModelDownloadWorker.TAG).removeObserver(downloadObserver)
        super.onCleared()
    }

    private suspend fun buildRows(): List<ModelRow> {
        val manifest = modelManager.loadManifest()
        val statuses = modelManager.detectModels().associateBy { it.name }
        return manifest.models.map { descriptor ->
            val status = statuses[descriptor.folder]
            val health = modelManager.modelHealth(descriptor.id)
            val staged = modelManager.stagedBundle(descriptor.id)
            val evidence = modelManager.evidenceState(descriptor.id)
            ModelRow(
                id = descriptor.id,
                folder = descriptor.folder,
                type = descriptor.type.name,
                version = descriptor.version,
                present = status?.present == true,
                healthy = status?.present == true && health.healthy,
                verified = status?.present == true && health.verified,
                sizeMb = status?.sizeMb ?: 0L,
                backend = descriptor.backend.name,
                minRamMb = descriptor.minRamMb,
                language = descriptor.defaultLanguage,
                sha256Preview = descriptor.files.firstOrNull { it.sha256.isNotBlank() }
                    ?.sha256?.take(12)?.let { "$it…" } ?: "—",
                healthMessage = if (staged != null)
                    "Staged — not yet Active. Verify & activate runs a real native load + smoke; existing model retained. ${health.message}"
                    else health.message,
                staged = staged != null,
                evidence = evidence,
                evidenceLabel = evidenceLabel(evidence, modelManager.latestReceipt(descriptor.id))
            )
        }
    }

    private suspend fun buildBrainStatus(spec: BrainModelSpec): BrainStatusRow {
        val loaded = orchestrator?.loadedBrainProfile()
        val isLoaded = loaded?.manifestId == spec.manifestId
        val descriptor = modelManager.findModel(spec.manifestId)
        val bytes = descriptor?.files?.sumOf { it.sizeBytes } ?: 0L
        val evidence = modelManager.evidenceState(spec.manifestId)
        val receipt = modelManager.latestReceipt(spec.manifestId)
        return BrainStatusRow(
            manifestId = spec.manifestId,
            displayName = spec.displayName,
            isDeviceVerified = spec.isDeviceVerified,
            minimumRamMb = spec.minimumRamMb,
            recommendedRamMb = spec.recommendedRamMb,
            installed = modelManager.resolveBrainLoadPath(spec) != null,
            isLoaded = isLoaded,
            backend = if (isLoaded) orchestrator?.loadedBrainBackend().orEmpty() else "",
            lastLoadError = orchestrator?.lastBrainLoadError().orEmpty(),
            description = spec.description,
            runtime = when (spec.runtime) {
                BrainRuntime.MNN -> "MNN (experimental CPU)"
                BrainRuntime.LITERT_LM -> "LiteRT-LM"
                BrainRuntime.LLAMA_CPP -> "llama.cpp (experimental CPU; browser unsupported)"
            },
            artifactSummary = "${descriptor?.files?.size ?: 0} files · $bytes bytes (${bytes / 1_000_000} MB download)",
            runtimeStatus = if (isLoaded) "Loaded: ${orchestrator?.loadedBrainBackend().orEmpty()}. Load success is not physical-device qualification."
                else "Not loaded.",
            evidence = evidence,
            evidenceLabel = evidenceLabel(evidence, receipt),
            lastReceipt = receipt?.let { r ->
                "${r.outcome} ${r.lane} ${if (r.passed) "on ${r.backend}, load ${r.loadMs} ms, smoke ${r.smokeTokens} tok/${r.smokeMs} ms" else r.reason}"
            }.orEmpty()
        )
    }

    private fun evidenceLabel(state: LoadAdmission.EvidenceState, receipt: NativeLoadReceipt?): String = when (state) {
        LoadAdmission.EvidenceState.QUALIFIED -> "Qualified — signed physical-device record"
        LoadAdmission.EvidenceState.WORKS_HERE -> "Works here — native load + smoke recorded on this device (unsigned)"
        LoadAdmission.EvidenceState.UNKNOWN ->
            if (receipt != null && !receipt.passed) "Unknown — last native attempt here failed (${receipt.reason}); not a policy refusal"
            else "Unknown — no load evidence on this device yet; unsigned is not refused"
    }
}
