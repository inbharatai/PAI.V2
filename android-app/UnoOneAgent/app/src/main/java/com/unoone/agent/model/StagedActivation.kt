package com.unoone.agent.model

import com.unoone.agent.core.modeladmission.NativeLoadReceipt
import com.unoone.agent.modelmanager.ModelBundleStore
import kotlinx.coroutines.runBlocking
import java.io.File

/**
 * Host-testable Download → Staged → Verifying → Active coordinator. It owns no Android state: the
 * native runtime is reached through [NativePort], the pointer switch through [ModelBundleStore.activate]
 * (or `ModelManager.activateStaged`, which adds fresh admission), and evidence through [record].
 *
 * Android's brain runtimes are single-resident, so "retains previous instance" means: the previous
 * ACTIVE BUNDLE and routing pointer are retained, the previously resident model path is remembered,
 * and on any failure the candidate is unloaded and the previous model is reloaded (best effort).
 * The pointer is only switched after a real native load AND a bounded real smoke generation.
 */
object StagedActivation {
    enum class Phase { STAGED, VERIFYING, ACTIVE, FAILED }
    data class State(val modelId: String, val phase: Phase, val detail: String)

    sealed class LoadOutcome {
        data class Loaded(val backend: String) : LoadOutcome()
        data class Failed(val message: String, val outOfMemory: Boolean = false) : LoadOutcome()
    }
    sealed class SmokeOutcome {
        data class Passed(val tokens: Int, val detail: String = "") : SmokeOutcome()
        data class Failed(val message: String) : SmokeOutcome()
    }

    /** Native runtime seam. Production: AgentOrchestrator. Tests: fakes. */
    interface NativePort {
        fun currentLoadedPath(): String?
        suspend fun unloadCurrent(): Boolean
        suspend fun load(path: String): LoadOutcome
        /** Short, bounded, REAL generation on the model loaded by [load]. */
        suspend fun smoke(): SmokeOutcome
    }

    class Receipt(val outcome: NativeLoadReceipt.Outcome, val reason: String, val backend: String,
        val loadMs: Long, val smokeMs: Long, val smokeTokens: Int)

    /** Transactional [ModelBundleStore.NativeLoader] built on a [NativePort]. */
    class Loader(
        private val port: NativePort,
        private val resolveLoadPath: (File) -> String?,
        private val record: (Receipt) -> Unit,
        private val onProgress: (String) -> Unit = {}
    ) : ModelBundleStore.NativeLoader {
        override val retainsPreviousInstance: Boolean = true
        private var previousPath: String? = null
        private var candidateLoaded = false
        private var pendingReceipt: Receipt? = null
        var lastDetail: String = ""
            private set

        override fun freshAdmissionError(): String? = null

        override fun loadAndSmoke(candidateRoot: File): ModelBundleStore.NativeResult = runBlocking {
            previousPath = port.currentLoadedPath()
            val path = resolveLoadPath(candidateRoot)
            if (path == null) {
                lastDetail = "candidate bundle has no runtime artifact"
                pendingReceipt = Receipt(NativeLoadReceipt.Outcome.FAILED, "BAD_LOAD:missing-artifact", "", 0, 0, 0)
                return@runBlocking ModelBundleStore.NativeResult.BAD_LOAD
            }
            onProgress("Unloading previous model")
            if (!port.unloadCurrent()) {
                lastDetail = "previous model did not acknowledge unload"
                pendingReceipt = Receipt(NativeLoadReceipt.Outcome.FAILED, "CANCELLED:unload-refused", "", 0, 0, 0)
                return@runBlocking ModelBundleStore.NativeResult.CANCELLED
            }
            onProgress("Native load")
            val t0 = System.currentTimeMillis()
            val load = port.load(path)
            val loadMs = System.currentTimeMillis() - t0
            when (load) {
                is LoadOutcome.Failed -> {
                    lastDetail = "native load failed: ${load.message}"
                    val code = if (load.outOfMemory) "OOM" else "BAD_LOAD"
                    pendingReceipt = Receipt(NativeLoadReceipt.Outcome.FAILED, "$code:${load.message}", "", loadMs, 0, 0)
                    return@runBlocking if (load.outOfMemory) ModelBundleStore.NativeResult.OOM else ModelBundleStore.NativeResult.BAD_LOAD
                }
                is LoadOutcome.Loaded -> {
                    candidateLoaded = true
                    onProgress("Smoke generation on ${load.backend}")
                    val s0 = System.currentTimeMillis()
                    val smoke = port.smoke()
                    val smokeMs = System.currentTimeMillis() - s0
                    when (smoke) {
                        is SmokeOutcome.Failed -> {
                            lastDetail = "smoke generation failed: ${smoke.message}"
                            pendingReceipt = Receipt(NativeLoadReceipt.Outcome.FAILED, "BAD_SMOKE:${smoke.message}", load.backend, loadMs, smokeMs, 0)
                            ModelBundleStore.NativeResult.BAD_SMOKE
                        }
                        is SmokeOutcome.Passed -> {
                            lastDetail = "loaded on ${load.backend} in ${loadMs} ms; smoke ${smoke.tokens} tokens in ${smokeMs} ms"
                            pendingReceipt = Receipt(NativeLoadReceipt.Outcome.PASSED, "", load.backend, loadMs, smokeMs, smoke.tokens)
                            ModelBundleStore.NativeResult.PASSED
                        }
                    }
                }
            }
        }

        override fun commitRouting() {
            // Pointer already switched atomically; the candidate stays resident as the active model.
            pendingReceipt?.let(record)
            pendingReceipt = null
        }

        override fun rollbackCandidate() = runBlocking {
            pendingReceipt?.let(record)
            pendingReceipt = null
            if (candidateLoaded) {
                runCatching { port.unloadCurrent() }
                candidateLoaded = false
            }
            val previous = previousPath
            if (previous != null) {
                onProgress("Restoring previous model")
                runCatching { port.load(previous) }
            }
        }
    }

    /**
     * Runs one activation. [activate] is `ModelManager.activateStaged` in production (adds fresh
     * ACTIVATION admission) or `ModelBundleStore.activate` in host tests.
     */
    suspend fun run(
        bundle: ModelBundleStore.Staged,
        port: NativePort,
        resolveLoadPath: (File) -> String?,
        record: (Receipt) -> Unit,
        activate: (ModelBundleStore.Staged, ModelBundleStore.NativeLoader) -> Boolean,
        onState: (State) -> Unit = {}
    ): State {
        val id = bundle.modelId
        onState(State(id, Phase.STAGED, "Staged bundle ${bundle.bundleId}"))
        onState(State(id, Phase.VERIFYING, "Fresh admission + native load + smoke"))
        val loader = Loader(port, resolveLoadPath, record) { onState(State(id, Phase.VERIFYING, it)) }
        val outcome = try {
            activate(bundle, loader)
        } catch (failure: IllegalStateException) {
            return State(id, Phase.FAILED, failure.message ?: "activation refused").also(onState)
        }
        val state = if (outcome) State(id, Phase.ACTIVE, loader.lastDetail)
            else State(id, Phase.FAILED, "previous active model retained — ${loader.lastDetail}")
        onState(state)
        return state
    }
}
