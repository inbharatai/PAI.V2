package com.unoone.agent.core.model

/**
 * Result of a model engine initialization attempt.
 *
 * Records which hardware backend the model actually loaded on (GPU, NPU, or CPU),
 * how long initialization took, and whether the load succeeded. This is the
 * device-time companion to [BrainModelSpec] — the spec says what we *want*,
 * [ModelLoadResult] says what *happened*.
 *
 * The orchestrator reads [backend] to decide whether E4B is viable on this device:
 * - GPU backend → full speed, E4B usable for compound commands
 * - NPU backend → good speed, E4B usable
 * - CPU backend → slow, E4B may time out; prefer Lite
 */
data class ModelLoadResult(
    /** Which brain model this result is for. */
    val modelId: BrainModelId,

    /** The hardware backend that the engine actually loaded on: "GPU", "NPU", or "CPU". */
    val backend: String,

    /** Wall-clock milliseconds from Engine() constructor to initialize() completion. */
    val loadTimeMs: Long,

    /** Whether the model loaded successfully. If false, [backend] and [loadTimeMs] are best-effort. */
    val success: Boolean,

    /**
     * Error message if [success] is false, empty string on success.
     * Includes the backend that failed, e.g. "GPU backend failed: delegate error; fell back to CPU".
     */
    val errorMessage: String = "",

    /**
     * Available device RAM in MB at the time of load, as reported by ActivityManager.MemoryInfo.
     * Used by [ModelTierSelector] to decide whether E4B is viable.
     */
    val availableRamMb: Int = -1,

    /**
     * The profile that was used for this load attempt. Null if the load failed before
     * a profile could be determined.
     */
    val profile: ModelProfile? = null
) {
    /** True if the model loaded on a hardware-accelerated backend (GPU or NPU). */
    val isHardwareAccelerated: Boolean get() = success && (backend == "GPU" || backend == "NPU")

    /** True if the model loaded on CPU — functional but slow. */
    val isCpuFallback: Boolean get() = success && backend == "CPU"

    companion object {
        /** Create a successful load result. */
        fun loaded(
            modelId: BrainModelId,
            backend: String,
            loadTimeMs: Long,
            availableRamMb: Int,
            profile: ModelProfile
        ): ModelLoadResult = ModelLoadResult(
            modelId = modelId,
            backend = backend,
            loadTimeMs = loadTimeMs,
            success = true,
            errorMessage = "",
            availableRamMb = availableRamMb,
            profile = profile
        )

        /** Create a failed load result. */
        fun failed(
            modelId: BrainModelId,
            errorMessage: String,
            availableRamMb: Int = -1
        ): ModelLoadResult = ModelLoadResult(
            modelId = modelId,
            backend = "",
            loadTimeMs = 0,
            success = false,
            errorMessage = errorMessage,
            availableRamMb = availableRamMb,
            profile = null
        )
    }
}