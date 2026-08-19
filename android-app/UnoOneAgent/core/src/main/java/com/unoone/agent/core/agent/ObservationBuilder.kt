package com.unoone.agent.core.agent

import com.unoone.agent.core.model.ActionResult
import com.unoone.agent.core.model.Result

/**
 * Builds structured observation strings from action results for the model's ReAct loop.
 *
 * The model sees tool results as observations in its next inference step. This builder
 * formats [ActionResult] into concise, informative text that the model can reason over,
 * capped to [MAX_OBSERVATION_CHARS] to fit the on-device context window.
 *
 * Two formatting modes:
 * - [buildDetailed] — full structured observation with status, evidence, and recovery info.
 *   Used for the first observation in a multi-step ReAct loop.
 * - [buildConcise] — single-line observation for subsequent steps.
 *   Used when the loop is deep and context budget is tight.
 */
object ObservationBuilder {

    /**
     * Maximum characters for an observation string fed back to the model.
     * Matches [com.unoone.agent.localbrain.GemmaPlanner.MAX_OBSERVATION_CHARS].
     */
    const val MAX_OBSERVATION_CHARS: Int = 1_500

    /**
     * Build a detailed observation from an [ActionResult].
     *
     * Format:
     * ```
     * Tool: <name>
     * Status: SUCCESS/PARTIAL/FAILED
     * Verified: true/false
     * Evidence:
     *   key1: value1
     *   key2: value2
     * Message: <user message>
     * Recoverable: <error code> (if applicable)
     * ```
     */
    fun buildDetailed(result: ActionResult): String {
        val sb = StringBuilder()
        sb.appendLine("Tool: ${result.tool}")
        sb.appendLine("Status: ${result.status.name}")
        sb.appendLine("Verified: ${result.verified}")
        if (result.evidence.isNotEmpty()) {
            sb.appendLine("Evidence:")
            for ((key, value) in result.evidence) {
                sb.appendLine("  $key: $value")
            }
        }
        sb.appendLine("Message: ${result.userMessage}")
        if (result.recoverableError != null) {
            sb.appendLine("Recoverable: ${result.recoverableError}")
        }
        return sb.toString().take(MAX_OBSERVATION_CHARS)
    }

    /**
     * Build a concise single-line observation from an [ActionResult].
     *
     * Uses emoji-like symbols for quick scanning:
     * - ✓ verified success
     * - ⚠ partial/unverified
     * - ✗ failed
     * - ? unverified attempt
     */
    fun buildConcise(result: ActionResult): String {
        val prefix = when {
            result.isVerifiedSuccess -> "✓"
            result.status == ActionResult.Status.PARTIAL -> "⚠"
            result.status == ActionResult.Status.FAILED -> "✗"
            !result.verified -> "?"
            else -> "→"
        }
        val suffix = result.recoverableError?.let { " [recoverable: $it]" } ?: ""
        return "$prefix ${result.tool}: ${result.userMessage}$suffix".take(MAX_OBSERVATION_CHARS)
    }

    /**
     * Build an observation from a legacy [Result<String>] return type.
     * For backward compatibility with ActionExecutor paths that haven't been
     * migrated to [ActionResult] yet.
     */
    fun fromLegacyResult(tool: String, result: Result<String>): String {
        return when (result) {
            is Result.Success -> "✓ $tool: ${result.data.take(MAX_OBSERVATION_CHARS)}"
            is Result.Error -> "✗ $tool: ${result.message.take(200)}"
        }
    }

    /**
     * Build a recovery suggestion from a [ActionResult.recoverableError].
     * Returns null if the error is not recoverable.
     */
    fun buildRecoveryHint(result: ActionResult): String? {
        if (result.recoverableError == null) return null
        return when (result.recoverableError) {
            "CONTACT_NOT_FOUND" -> "Try a different name or spell it differently."
            "CALENDAR_CONFLICT" -> "There's a conflict at that time. Try a different time."
            "APP_NOT_INSTALLED" -> "The app is not installed. Would you like to open a browser instead?"
            "PERMISSION_DENIED" -> "Permission was denied. You can grant it in Settings."
            else -> "Would you like to try again or try a different approach?"
        }
    }

    /**
     * Truncate an observation string to [MAX_OBSERVATION_CHARS] with a truncation marker.
     */
    fun truncate(observation: String): String {
        return if (observation.length <= MAX_OBSERVATION_CHARS) {
            observation
        } else {
            observation.take(MAX_OBSERVATION_CHARS - 20) + "\n... [truncated]"
        }
    }
}