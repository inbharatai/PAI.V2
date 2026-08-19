package com.unoone.agent.core.model

/**
 * Per-model configuration that governs how the agent orchestrator behaves when
 * a particular brain model is active. Each [BrainModelId] maps to exactly one profile;
 * the orchestrator reads limits from the active profile instead of using hardcoded
 * constants, so E2B (Lite) and E4B (Medium) share the same deterministic pipeline
 * with only numeric/behavioural differences.
 *
 * The profile is selected **before** a task starts and **never switches mid-task**.
 * If the active model fails, the task stops and the orchestrator offers a retry
 * with a different tier.
 */
data class ModelProfile(
    /** Machine-readable id matching the [BrainModelId]. */
    val id: String,

    /** Human-visible name: "Lite" for E2B, "Medium" for E4B. */
    val displayName: String,

    /**
     * Maximum number of candidate tools the model may be offered for a single task.
     * E2B (Lite): 3 — a small model can only pick accurately from a small menu.
     * E4B (Medium): 6 — a larger model can reason over more options.
     */
    val maxCandidateTools: Int,

    /**
     * Maximum agent (Reason→Act→Observe) steps per task, **including** the first call.
     * E2B (Lite): 2 — one action, one follow-up.
     * E4B (Medium): 4 — enough for a genuine multi-step chain.
     */
    val maxAgentSteps: Int,

    /**
     * Maximum browser-step sub-iterations inside a single tool execution.
     * E2B (Lite): 0 — no browser capability.
     * E4B (Medium): 8 — compound browser tasks need more room.
     */
    val maxBrowserSteps: Int,

    /**
     * How many times the orchestrator may ask the model to repair a failed tool call
     * before giving up and surfacing the error to the user. Fixed at 1 for both tiers.
     */
    val maxRepairAttempts: Int,

    /**
     * Sampler temperature for action/tool-calling turns. Low temperature yields
     * deterministic, repeatable tool selections — essential for reliability.
     * E2B (Lite): 0.1, E4B (Medium): 0.1
     */
    val actionTemperature: Float,

    /**
     * Sampler temperature for free-form chat turns. A higher temperature produces
     * more varied conversational responses.
     * E2B (Lite): 0.3 — constrained but not robotic.
     * E4B (Medium): 0.7 — natural conversational feel.
     */
    val chatTemperature: Float,

    /**
     * Default context window (in tokens) for a planning conversation.
     * E2B (Lite): 2 048 — keeps inference fast on a small model.
     * E4B (Medium): 4 096 — enough for compound reasoning.
     */
    val contextTokens: Int,

    /**
     * Rough classification of the commands this profile should handle.
     * "simple" — single-action, deterministic-routing commands.
     * "compound" — multi-step, ambiguous, or novel commands.
     */
    val complexityThreshold: String
)

/** Pre-built profiles matching the UnoOne model tiers. */
object ModelProfiles {

    /** Lite profile — Gemma 4 E2B. Fast, conservative, 2-3 tools, 2 steps. */
    val LITE: ModelProfile = ModelProfile(
        id = BrainModelId.GEMMA_4_E2B.name,
        displayName = "Lite",
        maxCandidateTools = 3,
        maxAgentSteps = 2,
        maxBrowserSteps = 0,
        maxRepairAttempts = 1,
        actionTemperature = 0.1f,
        chatTemperature = 0.3f,
        contextTokens = 2_048,
        complexityThreshold = "simple"
    )

    /** Medium profile — Gemma 4 E4B. Better reasoning, 3-6 tools, 4 steps. */
    val MEDIUM: ModelProfile = ModelProfile(
        id = BrainModelId.GEMMA_4_E4B.name,
        displayName = "Medium",
        maxCandidateTools = 6,
        maxAgentSteps = 4,
        maxBrowserSteps = 8,
        maxRepairAttempts = 1,
        actionTemperature = 0.1f,
        chatTemperature = 0.7f,
        contextTokens = 4_096,
        complexityThreshold = "compound"
    )

    /** All profiles, keyed by [BrainModelId]. */
    private val byId: Map<String, ModelProfile> = mapOf(
        BrainModelId.GEMMA_4_E2B.name to LITE,
        BrainModelId.GEMMA_4_E4B.name to MEDIUM
    )

    /** Look up a profile by [BrainModelId]. Falls back to [LITE] if unknown. */
    fun forId(id: BrainModelId): ModelProfile =
        byId[id.name] ?: LITE

    /** Look up a profile by string id. Falls back to [LITE] if unknown. */
    fun forId(id: String): ModelProfile =
        byId[id] ?: LITE
}