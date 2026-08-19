package com.unoone.agent.core.agent

import com.unoone.agent.core.model.ActionResult

/**
 * Verifies action results against expected outcomes and builds structured evidence.
 *
 * The orchestrator uses this to decide whether a tool execution was *verified* successful
 * (the foreground package matches, the database confirms the write, etc.) or merely *attempted*.
 * The model may only announce success when [ActionResult.verified] is true.
 *
 * This is the "Observe" half of the ReAct loop — it produces the [ActionResult] that gets
 * fed back to the model as a tool response.
 */
object ActionVerifier {

    /**
     * Foreground package verification: confirms that the expected app actually came to the
     * foreground after an app launch action.
     *
     * @param expectedPackages the set of packages that indicate success (e.g., "com.whatsapp",
     *   "com.whatsapp.w4b" for a WhatsApp launch).
     * @param actualForeground the actual foreground package observed by AccessibilityService.
     * @return an [ActionResult] with verified=true if the foreground matches, verified=false otherwise.
     */
    fun verifyForegroundLaunch(
        tool: String,
        userMessage: String,
        expectedPackages: Set<String>,
        actualForeground: String?
    ): ActionResult {
        val foreground = actualForeground ?: ""
        // Exact package match or sub-package match (com.whatsapp matches com.whatsapp.w4b
        // but NOT com.whatsappspyware). Simple prefix matching would allow com.whatsapp.spyware.
        val verified = expectedPackages.any { expected ->
            foreground == expected || foreground.startsWith("$expected.")
        }

        return if (verified) {
            ActionResult.success(
                tool = tool,
                userMessage = userMessage,
                evidence = mapOf(
                    "foregroundPackage" to foreground,
                    "expectedPackages" to expectedPackages.joinToString(",")
                )
            )
        } else {
            ActionResult.unverified(
                tool = tool,
                userMessage = "$userMessage (could not verify: expected ${expectedPackages.joinToString(" or ")}, saw $foreground)",
                evidence = mapOf(
                    "foregroundPackage" to foreground,
                    "expectedPackages" to expectedPackages.joinToString(",")
                )
            )
        }
    }

    /**
     * Verify a simple action that always succeeds deterministically (go_home, go_back, etc.).
     * These don't need foreground package verification — the AccessibilityService call itself
     * confirms success or failure.
     */
    fun verifyDeterministicAction(
        tool: String,
        userMessage: String,
        actionSucceeded: Boolean
    ): ActionResult {
        return if (actionSucceeded) {
            ActionResult.success(
                tool = tool,
                userMessage = userMessage,
                evidence = mapOf("actionPerformed" to tool)
            )
        } else {
            ActionResult.failed(
                tool = tool,
                userMessage = "Failed: $userMessage"
            )
        }
    }

    /**
     * Build an observation string from an [ActionResult] for the model's next ReAct step.
     * Delegates to [ObservationBuilder.buildDetailed] to maintain a single source of truth.
     */
    fun buildObservation(result: ActionResult): String = ObservationBuilder.buildDetailed(result)

    /**
     * Build a concise observation from an [ActionResult].
     * Delegates to [ObservationBuilder.buildConcise] to maintain a single source of truth.
     */
    fun buildConciseObservation(result: ActionResult): String = ObservationBuilder.buildConcise(result)

    /**
     * Map of tools that require foreground package verification after execution.
     * All others use [verifyDeterministicAction] or [ActionResult.unverified].
     */
    val FOREGROUND_VERIFICATION_TOOLS: Set<String> = setOf(
        "open_app",
        "draft_whatsapp_message",
        "send_prepared_whatsapp",
        "create_calendar_event",
        "open_calendar",
        "open_calendar_insert",  // legacy calendar tool, still dispatchable
        "open_chrome",
        "open_url",
        "draft_email",
        "open_camera",
        "open_dialer",
        "open_settings"
    )

    /**
     * Map of tools where the action itself is deterministic (AccessibilityService
     * confirms success/failure directly). No foreground check needed.
     */
    val DETERMINISTIC_TOOLS: Set<String> = setOf(
        "go_home",
        "go_back",
        "scroll",
        "click_accessibility_node",
        "type_into_accessibility_node",
        "long_press_accessibility_node",
        "open_notifications",
        "open_recents",
        "speak_response"
    )

    const val MAX_OBSERVATION_LENGTH: Int = 1_500
}