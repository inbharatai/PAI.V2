package com.unoone.agent.model

import com.unoone.agent.AgentOrchestrator
import com.unoone.agent.core.model.BrainModelId
import com.unoone.agent.core.model.BrainModelSpec
import com.unoone.agent.core.model.Result
import com.unoone.agent.core.modeladmission.NativeLoadReceipt
import com.unoone.agent.core.util.Logger
import com.unoone.agent.modelmanager.ModelManager
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.withTimeout

/**
 * Production [StagedActivation.NativePort] over the existing LiteRT-LM / MNN / llama.cpp load path
 * in [AgentOrchestrator]. The smoke is a real bounded generation: one planning turn for Gemma/Qwen
 * (`planLlmToolCall`, which parses an actual model response) or the synthetic Owl self-test probes.
 * It proves "this device loaded and generated with this exact artifact" — nothing more.
 */
class NativeBrainPort(
    private val orchestrator: AgentOrchestrator,
    private val spec: BrainModelSpec,
    private val smokeTimeoutMs: Long = SMOKE_TIMEOUT_MS
) : StagedActivation.NativePort {
    override fun currentLoadedPath(): String? = orchestrator.loadedBrainPath()

    override suspend fun unloadCurrent(): Boolean =
        if (!orchestrator.isPhoneBrainResident()) true else runCatching { orchestrator.unloadLlmModel() }.getOrDefault(false)

    override suspend fun load(path: String): StagedActivation.LoadOutcome {
        val result = runCatching { orchestrator.loadLlmModel(path, spec) }
            .getOrElse { if (it is kotlinx.coroutines.CancellationException) throw it; Result.Error(it.message ?: it.javaClass.simpleName, it) }
        return when (result) {
            is Result.Success -> if (orchestrator.isLlmLoaded()) StagedActivation.LoadOutcome.Loaded(orchestrator.loadedBrainBackend().ifBlank { "unknown" })
                else StagedActivation.LoadOutcome.Failed(orchestrator.lastBrainLoadError().ifBlank { "runtime reported success but no model is resident" })
            is Result.Error -> StagedActivation.LoadOutcome.Failed(result.message, looksLikeOom(result.message) || looksLikeOom(orchestrator.lastBrainLoadError()))
        }
    }

    override suspend fun smoke(): StagedActivation.SmokeOutcome = try {
        withTimeout(smokeTimeoutMs) {
            if (spec.id == BrainModelId.GUI_OWL_1_5_4B_INSTRUCT) {
                val owl = orchestrator.runOwlSelfTest(spec)
                if (owl.loaded && owl.toolAccepted) StagedActivation.SmokeOutcome.Passed(owl.probes.size, owl.message)
                else StagedActivation.SmokeOutcome.Failed(owl.message)
            } else {
                when (val plan = orchestrator.planLlmToolCall(SMOKE_PROMPT)) {
                    is Result.Success -> StagedActivation.SmokeOutcome.Passed(1, "tool ${plan.data.tool}")
                    is Result.Error -> StagedActivation.SmokeOutcome.Failed(plan.message)
                }
            }
        }
    } catch (timeout: TimeoutCancellationException) {
        orchestrator.cancelLlmInference("smoke timeout")
        StagedActivation.SmokeOutcome.Failed("smoke generation exceeded ${smokeTimeoutMs} ms")
    }

    companion object {
        const val SMOKE_TIMEOUT_MS = 45_000L
        /** Bypasses the rule-based parser so the model itself must answer. */
        const val SMOKE_PROMPT = "Open WhatsApp"

        fun looksLikeOom(message: String?): Boolean {
            val m = message?.lowercase() ?: return false
            return "outofmemory" in m || "out of memory" in m || "oom" in m || "failed to allocate" in m || "enomem" in m
        }

        /**
         * Shared explicit-load / boot-load helper: real load, bounded smoke, receipt. A failed smoke
         * leaves the model loaded (legacy behaviour) but records FAILED so the UI stays honest.
         */
        suspend fun loadAndRecord(
            orchestrator: AgentOrchestrator,
            modelManager: ModelManager,
            spec: BrainModelSpec,
            path: String,
            lane: String,
            smoke: Boolean = true,
            /** Boot smokes run under app-start contention; a failed boot smoke is logged, not recorded. */
            recordSmokeFailure: Boolean = true
        ): Result<Unit> {
            val port = NativeBrainPort(orchestrator, spec)
            val t0 = System.currentTimeMillis()
            val load = port.load(path)
            val loadMs = System.currentTimeMillis() - t0
            if (load is StagedActivation.LoadOutcome.Failed) {
                modelManager.recordNativeLoad(spec.manifestId, NativeLoadReceipt.Outcome.FAILED, lane,
                    reason = (if (load.outOfMemory) "OOM:" else "BAD_LOAD:") + load.message, loadMs = loadMs)
                return Result.Error(load.message)
            }
            val backend = (load as StagedActivation.LoadOutcome.Loaded).backend
            if (!smoke) return Result.Success(Unit)
            val s0 = System.currentTimeMillis()
            val outcome = port.smoke()
            val smokeMs = System.currentTimeMillis() - s0
            when (outcome) {
                is StagedActivation.SmokeOutcome.Passed -> modelManager.recordNativeLoad(spec.manifestId,
                    NativeLoadReceipt.Outcome.PASSED, lane, backend = backend, loadMs = loadMs, smokeMs = smokeMs, smokeTokens = outcome.tokens)
                is StagedActivation.SmokeOutcome.Failed -> {
                    Logger.w("NativeBrainPort: ${spec.displayName} loaded but smoke failed ($lane): ${outcome.message}")
                    if (recordSmokeFailure) modelManager.recordNativeLoad(spec.manifestId, NativeLoadReceipt.Outcome.FAILED, lane,
                        reason = "BAD_SMOKE:${outcome.message}", backend = backend, loadMs = loadMs, smokeMs = smokeMs)
                }
            }
            return Result.Success(Unit)
        }
    }
}
