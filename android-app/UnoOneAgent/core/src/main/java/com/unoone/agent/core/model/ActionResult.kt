package com.unoone.agent.core.model

/**
 * Verified result of a tool execution. Replaces the previous `Result<String>` return type
 * in [com.unoone.agent.execution.ActionExecutor] with structured evidence that the orchestrator
 * and model can reason about.
 *
 * Key principle: **the model may announce success only when [verified] is true.**
 * An unverified result (e.g. a WhatsApp launch where we couldn't confirm the foreground package)
 * must be reported as "attempted but unverified" — never as "done".
 */
data class ActionResult(
    /** Whether the tool execution succeeded, partially succeeded, or failed. */
    val status: Status,

    /** The canonical tool name that was executed. */
    val tool: String,

    /**
     * True only if the orchestrator independently verified the outcome:
     * - App launches: `foregroundPackage == expectedPackage`
     * - Note operations: confirmed by database query
     * - Calendar operations: confirmed by content resolver query
     * - Accessibility actions: confirmed by accessibility tree diff
     *
     * When false, the model must report "attempted but could not verify" rather than "done".
     */
    val verified: Boolean,

    /**
     * Machine-readable evidence backing the result. Keys are tool-specific:
     * - App launches: `"foregroundPackage"`, `"timestamp"`
     * - Notes: `"noteId"`, `"rowCount"`
     * - Calendar: `"eventId"`, `"conflictCount"`
     * - Accessibility: `"nodeId"`, `"actionPerformed"`
     */
    val evidence: Map<String, String>,

    /**
     * Human-readable message suitable for speak_response. The orchestrator
     * uses this as the observation text fed back to the model.
     */
    val userMessage: String,

    /**
     * If the tool failed but the user can retry (e.g. contact not found, calendar conflict),
     * this contains a short machine-readable error code. The model may propose an alternative.
     * Null for successful or non-recoverable failures.
     */
    val recoverableError: String? = null
) {
    /** Status of a tool execution. */
    enum class Status {
        /** Tool executed and verified successfully. */
        SUCCESS,
        /** Tool executed but some parts failed or could not be verified. */
        PARTIAL,
        /** Tool execution failed. */
        FAILED
    }

    /** Convenience: true when status is SUCCESS and verified is true. */
    val isVerifiedSuccess: Boolean get() = status == Status.SUCCESS && verified

    companion object {
        /** Create a verified success result. */
        fun success(
            tool: String,
            userMessage: String,
            evidence: Map<String, String> = emptyMap()
        ): ActionResult = ActionResult(
            status = Status.SUCCESS,
            tool = tool,
            verified = true,
            evidence = evidence,
            userMessage = userMessage
        )

        /** Create an unverified success — tool ran but verification was not possible. */
        fun unverified(
            tool: String,
            userMessage: String,
            evidence: Map<String, String> = emptyMap()
        ): ActionResult = ActionResult(
            status = Status.SUCCESS,
            tool = tool,
            verified = false,
            evidence = evidence,
            userMessage = userMessage
        )

        /** Create a verified failure result with a recoverable error code. */
        fun failed(
            tool: String,
            userMessage: String,
            recoverableError: String? = null,
            evidence: Map<String, String> = emptyMap()
        ): ActionResult = ActionResult(
            status = Status.FAILED,
            tool = tool,
            verified = true,
            evidence = evidence,
            userMessage = userMessage,
            recoverableError = recoverableError
        )

        /** Create a partial result — some sub-operations succeeded, others failed. */
        fun partial(
            tool: String,
            userMessage: String,
            evidence: Map<String, String> = emptyMap()
        ): ActionResult = ActionResult(
            status = Status.PARTIAL,
            tool = tool,
            verified = false,
            evidence = evidence,
            userMessage = userMessage
        )
    }
}