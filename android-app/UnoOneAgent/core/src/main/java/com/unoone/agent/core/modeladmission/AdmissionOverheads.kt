package com.unoone.agent.core.modeladmission

/**
 * Working-set overhead constants mirrored from the shared Rust admission core
 * (`packages/model-admission`, schema 1). The Rust core carries these as candidate memory fields and
 * the golden vector `tests/fixtures/admission-v1.json` is the single source of truth for their
 * values; `AdmissionOverheadsTest` fails if this file and the fixture ever disagree.
 *
 * Nothing here is a measured device peak. Every number is an ESTIMATE used only when no native
 * load+smoke receipt exists for this device (see [LoadAdmission] — evidence beats estimate).
 *
 * Decision semantics also follow the Rust core (`admission.rs`):
 *  - `PERMANENT_MEMORY_MISFIT` is budgeted against TOTAL RAM: `peak + os_reserve > total`.
 *  - `MEMORY_PRESSURE` is budgeted against AVAILABLE RAM: `peak + max(available_reserve, lowMemoryThreshold) > available`
 *    and is `SUPPORTED_WITH_LIMITS`, i.e. a soft, retryable condition, never a permanent refusal.
 */
object AdmissionOverheads {
    private const val MIB = 1024L * 1024L

    /** `speech_ram_bytes`: resident sherpa ASR/TTS/KWS while a brain is loaded. */
    const val SPEECH_RAM_BYTES: Long = 256L * MIB
    /** `runtime_ram_bytes`: native runtime (LiteRT-LM / MNN / llama.cpp) allocator + graph overhead. */
    const val RUNTIME_RAM_BYTES: Long = 512L * MIB
    /** `per_agent_ram_bytes`: one agent working set; Android runs exactly one. */
    const val PER_AGENT_RAM_BYTES: Long = 64L * MIB
    /** `heap_bytes`: JVM heap the app needs beside the native allocation. */
    const val HEAP_BYTES: Long = 128L * MIB
    /** `os_reserve_bytes`: total-RAM budget reserve (Rust `total_need = peak + os_reserve`). */
    const val OS_RESERVE_BYTES: Long = 1024L * MIB
    /** `available_reserve_bytes`: available-RAM budget reserve, `max`-ed with the low-memory threshold. */
    const val AVAILABLE_RESERVE_BYTES: Long = 512L * MIB
    /** `comfortable_headroom_bytes`: above this the core reports RECOMMENDED instead of LIMITED_HEADROOM. */
    const val COMFORTABLE_HEADROOM_BYTES: Long = 512L * MIB
    /** `disk_headroom_bytes`: storage reservation headroom beside download + installed bytes. */
    const val DISK_HEADROOM_BYTES: Long = 1024L * MIB

    /** Golden-vector KV reference: `kv_ram_bytes` 1,879,048,192 at `context_tokens` 8192. */
    const val KV_REFERENCE_BYTES: Long = 1_879_048_192L
    const val KV_REFERENCE_CONTEXT_TOKENS: Int = 8192
    /** Rust core has exactly one agent slot on phones. */
    const val ANDROID_PARALLEL_AGENTS: Long = 1L

    /** KV cache ESTIMATE: linear in context tokens using the golden-vector reference ratio. */
    fun kvEstimateBytes(contextTokens: Int): Long {
        require(contextTokens >= 0)
        return Math.multiplyExact(KV_REFERENCE_BYTES / KV_REFERENCE_CONTEXT_TOKENS, contextTokens.toLong())
    }

    /**
     * Peak working-set ESTIMATE for one Android brain, mirroring Rust `peak_ram()`:
     * weights(+projector, both inside [artifactBytes]) + kv + speech + runtime + per_agent × 1.
     * Vision RAM is zero on Android until a measured profile exists (no inferred GPU/NPU/VRAM).
     */
    fun workingSetEstimateBytes(artifactBytes: Long, contextTokens: Int): Long {
        require(artifactBytes > 0)
        var sum = Math.addExact(artifactBytes, kvEstimateBytes(contextTokens))
        sum = Math.addExact(sum, SPEECH_RAM_BYTES)
        sum = Math.addExact(sum, RUNTIME_RAM_BYTES)
        sum = Math.addExact(sum, Math.multiplyExact(PER_AGENT_RAM_BYTES, ANDROID_PARALLEL_AGENTS))
        return sum
    }

    /** Rust `total_need`: budget against TOTAL RAM. */
    fun totalRequiredBytes(workingSet: Long): Long = Math.addExact(workingSet, OS_RESERVE_BYTES)

    /** Rust `avail_need`: budget against AVAILABLE RAM. */
    fun availableRequiredBytes(workingSet: Long, lowMemoryThreshold: Long): Long =
        Math.addExact(workingSet, maxOf(AVAILABLE_RESERVE_BYTES, lowMemoryThreshold))
}
