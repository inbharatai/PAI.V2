package com.unoone.agent.core.model

/**
 * UnoOne V2 on-device planning brains. E2B is the Lite tier; E4B is the Medium tier.
 * The active profile is selected before each task and never switches mid-task.
 * If the active model fails, the task stops and the orchestrator offers a retry with a different tier.
 */
enum class BrainModelId { GEMMA_4_E2B, GEMMA_4_E4B }

/** Model family used by prompt construction. */
enum class ModelFamily { GEMMA_4 }

/** Hardware backend preference. LiteRT-LM backend mapping lives in `:localbrain`. */
enum class BackendPreference { GPU_FIRST, CPU_ONLY, ANY }

/**
 * Authoritative specification of the UnoOne planning brain.
 *
 * Integrity, performance and device support are deliberately not inferred from a download URL.
 * `isDeviceVerified` may only become true after the physical-device matrix is completed and the
 * result is committed to the repository.
 */
data class BrainModelSpec(
    val id: BrainModelId,
    val manifestId: String,
    val displayName: String,
    val modelFamily: ModelFamily,
    val modelFolder: String,
    val fileName: String,
    val fileExtension: String,
    val preferredBackend: BackendPreference,
    val minimumRamMb: Int,
    val recommendedRamMb: Int,
    val maximumContextTokens: Int,
    val defaultContextTokens: Int,
    val supportsNativeSystemRole: Boolean,
    val isLegacy: Boolean,
    val isDeviceVerified: Boolean,
    val experimentalLabel: String?,
    val description: String
)

/** Authoritative registry for all UnoOne planning brains. */
object BrainModelRegistry {

    val GEMMA_4_E2B: BrainModelSpec = BrainModelSpec(
        id = BrainModelId.GEMMA_4_E2B,
        manifestId = "gemma-4-e2b",
        displayName = "Gemma 4 E2B",
        modelFamily = ModelFamily.GEMMA_4,
        modelFolder = "brain/gemma-4-e2b",
        fileName = "gemma-4-E2B-it.litertlm",
        fileExtension = ".litertlm",
        preferredBackend = BackendPreference.GPU_FIRST,
        minimumRamMb = 6_144,
        recommendedRamMb = 8_192,
        maximumContextTokens = 32_768,
        defaultContextTokens = 4_096,
        supportsNativeSystemRole = true,
        isLegacy = false,
        isDeviceVerified = false,
        experimentalLabel = "Device qualification required",
        description = "UnoOne Lite planning brain. The generic Android LiteRT-LM artifact must pass integrity, tool-call, memory, thermal and real-device tests before production release."
    )

    /**
     * Gemma 4 E4B — UnoOne Medium planning brain.
     *
     * Better reasoning for compound commands. Requires ≥ 8 GB available RAM.
     * SHA-256: 0b2a8980ce155fd97673d8e820b4d29d9c7d99b8fa6806f425d969b145bd52e0
     * File size: ~3.66 GB on disk.
     */
    val GEMMA_4_E4B: BrainModelSpec = BrainModelSpec(
        id = BrainModelId.GEMMA_4_E4B,
        manifestId = "gemma-4-e4b",
        displayName = "Gemma 4 E4B",
        modelFamily = ModelFamily.GEMMA_4,
        modelFolder = "brain/gemma-4-e4b",
        fileName = "gemma-4-E4B-it.litertlm",
        fileExtension = ".litertlm",
        preferredBackend = BackendPreference.GPU_FIRST,
        minimumRamMb = 8_192,
        recommendedRamMb = 10_240,
        maximumContextTokens = 32_768,
        defaultContextTokens = 4_096,
        supportsNativeSystemRole = true,
        isLegacy = false,
        isDeviceVerified = false,
        experimentalLabel = "Device qualification required",
        description = "UnoOne Medium planning brain. Better reasoning for compound commands. Requires ≥ 8 GB available RAM."
    )

    val all: List<BrainModelSpec> = listOf(GEMMA_4_E2B, GEMMA_4_E4B)
    val defaultProfile: BrainModelSpec = GEMMA_4_E2B

    private val byIdMap: Map<BrainModelId, BrainModelSpec> = mapOf(
        BrainModelId.GEMMA_4_E2B to GEMMA_4_E2B,
        BrainModelId.GEMMA_4_E4B to GEMMA_4_E4B
    )

    fun byId(id: BrainModelId): BrainModelSpec =
        byIdMap[id] ?: GEMMA_4_E2B

    fun byManifestId(manifestId: String): BrainModelSpec? =
        all.firstOrNull { it.manifestId == manifestId }

    fun byFolder(folder: String): BrainModelSpec? =
        all.firstOrNull { it.modelFolder == folder }

    /** Resolve a persisted manifest id to a spec, falling back to E2B for unknown values. */
    fun resolveOrDefault(manifestId: String?): BrainModelSpec =
        manifestId?.let { byManifestId(it) } ?: GEMMA_4_E2B
}
