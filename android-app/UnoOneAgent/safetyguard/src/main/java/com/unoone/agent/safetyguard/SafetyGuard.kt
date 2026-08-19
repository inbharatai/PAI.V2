package com.unoone.agent.safetyguard

import com.unoone.agent.core.model.RiskLevel
import com.unoone.agent.core.util.Logger

class SafetyGuard {

    private val riskRules = mapOf(
        // Risk 0 — Direct execution (no confirmation needed)
        "create_note" to RiskLevel.DIRECT,
        "search_notes" to RiskLevel.DIRECT,
        "summarize_text" to RiskLevel.DIRECT,
        "speak_response" to RiskLevel.DIRECT,
        "open_chrome" to RiskLevel.DIRECT,
        "open_app" to RiskLevel.DIRECT,
        "deactivate_blind_aid" to RiskLevel.DIRECT,
        "prepare_document_fill" to RiskLevel.DIRECT,
        "check_calendar" to RiskLevel.DIRECT,
        "open_calendar" to RiskLevel.DIRECT,
        // Atomic accessibility — navigation actions are direct (safe, reversible)
        "go_home" to RiskLevel.DIRECT,
        "go_back" to RiskLevel.DIRECT,
        "scroll" to RiskLevel.DIRECT,
        "open_notifications" to RiskLevel.DIRECT,
        "open_recents" to RiskLevel.DIRECT,
        // Contact resolution is direct (read-only lookup)
        "resolve_contact" to RiskLevel.DIRECT,
        // Calendar conflict check is direct (read-only)
        "check_calendar_conflict" to RiskLevel.DIRECT,

        // Risk 1 — Single confirmation
        "open_url" to RiskLevel.CONFIRM,
        "open_calendar_insert" to RiskLevel.CONFIRM,
        "create_calendar_event" to RiskLevel.CONFIRM,
        "open_dialer" to RiskLevel.CONFIRM,
        "share_text" to RiskLevel.CONFIRM,
        "read_screen" to RiskLevel.STRONG_CONFIRM,
        "ocr_screen" to RiskLevel.STRONG_CONFIRM,
        "open_camera" to RiskLevel.CONFIRM,
        "create_skill" to RiskLevel.CONFIRM,
        "click_accessibility_node" to RiskLevel.CONFIRM,
        "type_into_accessibility_node" to RiskLevel.CONFIRM,
        "long_press_accessibility_node" to RiskLevel.CONFIRM,
        // Mic capture + online lookup both touch privacy-sensitive surfaces → single confirmation.
        "voice_recording" to RiskLevel.CONFIRM,
        "web_search" to RiskLevel.CONFIRM,
        // Drives the Secure Browser (PageAgent) on an approved origin — the user confirms opening the
        // session. In-browser sensitivity (passwords/OTP/payments/legal) is still gated by the
        // BrowserSafetyPolicy per-action confirm/takeover inside the session; this tier only authorizes
        // opening the automated browser session.
        "secure_browser_task" to RiskLevel.CONFIRM,
        // WhatsApp draft — user reviews before send
        "draft_whatsapp_message" to RiskLevel.STRONG_CONFIRM,

        // Risk 2 — Strong confirmation (must type "confirm")
        "delete_notes" to RiskLevel.STRONG_CONFIRM,
        "delete_all_notes" to RiskLevel.STRONG_CONFIRM,
        "export_data" to RiskLevel.STRONG_CONFIRM,
        "detect_objects" to RiskLevel.STRONG_CONFIRM,
        "draft_email" to RiskLevel.STRONG_CONFIRM,
        "send_whatsapp" to RiskLevel.STRONG_CONFIRM,
        "send_prepared_whatsapp" to RiskLevel.STRONG_CONFIRM,
        // system_control actions are all classified as STRONG_CONFIRM at the tool level. Atomic tools have their own entries above.
        "system_control" to RiskLevel.STRONG_CONFIRM,
        // Captures + analyzes the whole screen, which can read sensitive content (passwords, OTP,
        // banking) → strong confirmation, never silent.
        "describe_scene" to RiskLevel.STRONG_CONFIRM,

        // Risk 3 — Block (never executed)
        "send_message" to RiskLevel.BLOCK,
        "make_payment" to RiskLevel.BLOCK,
        "install_app" to RiskLevel.BLOCK,
        "access_passwords" to RiskLevel.BLOCK,
        "silent_control" to RiskLevel.BLOCK
    )

    fun classify(toolName: String): RiskLevel {
        val level = riskRules[toolName] ?: RiskLevel.STRONG_CONFIRM
        Logger.d("SafetyGuard classified $toolName as ${level.name}")
        return level
    }

    /**
     * Input-level risk classification. Scans the raw user input for dangerous keywords that might
     * indicate higher risk than the tool name alone suggests. Used as a secondary check after
     * tool-level classification; the pipeline takes the max of the two.
     *
     * Contextual model (review 2026-06-26): previously a blanket `"send " || "message" → BLOCK`
     * hard-blocked even draft-style requests that route to an already-STRONG_CONFIRM tool
     * (e.g. "send a WhatsApp message to mom" hit "send "/"message" → BLOCK, overriding
     * `send_whatsapp`'s STRONG_CONFIRM and making the headline assistant task unusable). The
     * classifier now distinguishes **draft** paths from **auto-send** paths:
     *
     * - BLOCK (never executed): money/pay/bank/credit-card/wire-transfer, passwords/OTP,
     *   install, factory reset, and explicit auto-send ("auto send", "send automatically",
     *   "send it now/for me").
     * - STRONG_CONFIRM (draft, user still presses send): "draft …", or "send/… message" when the
     *   target is WhatsApp/email — so `send_whatsapp` / `draft_email` proceed with strong
     *   confirmation instead of being hard-blocked.
     * - BLOCK: generic "send …" / "message" with no draft/app context (treated as auto-send intent).
     *
     * Auto-send of a final message is always blocked; only draft creation with confirmation is allowed.
     */
    fun classifyFromInput(input: String): RiskLevel {
        val lowered = input.lowercase()
        return when {
            // 1. Always-block: money, credentials, destructive system, explicit auto-send.
            lowered.contains("payment") || lowered.contains("pay ") -> RiskLevel.BLOCK
            lowered.contains("password") || lowered.contains("otp") || lowered.contains("one time password") -> RiskLevel.BLOCK
            lowered.contains("install") -> RiskLevel.BLOCK
            lowered.contains("money") || lowered.contains("transfer money") || lowered.contains("wire transfer") -> RiskLevel.BLOCK
            lowered.contains("bank") || lowered.contains("credit card") -> RiskLevel.BLOCK
            lowered.contains("format") || lowered.contains("factory reset") -> RiskLevel.BLOCK
            lowered.contains("auto send") || lowered.contains("send automatically") ||
                lowered.contains("send it now") || lowered.contains("send it for me") -> RiskLevel.BLOCK

            // 2. Draft paths — allow with strong confirmation (the user still reviews + presses send).
            lowered.contains("draft") -> RiskLevel.STRONG_CONFIRM
            (lowered.contains("whatsapp") || lowered.contains("email")) &&
                (lowered.contains("send") || lowered.contains("message")) &&
                !lowered.contains("read") && !lowered.contains("check") &&
                !lowered.contains("search") && !lowered.contains("show") &&
                !lowered.contains("look") -> RiskLevel.STRONG_CONFIRM

            // 3. Generic send/message with no draft/app context — block the auto-send intent.
            // "message" alone is too broad (blocks "read the message on screen" etc.),
            // so require compound phrases like "send message" or "message to <contact>".
            // Exempt read-only contexts: read, search, check, show, look, "what does".
            lowered.contains("send ") ||
                (lowered.contains("message") &&
                    !lowered.contains("read") &&
                    !lowered.contains("search") &&
                    !lowered.contains("check") &&
                    !lowered.contains("show") &&
                    !lowered.contains("look") &&
                    !lowered.contains("what ")) -> RiskLevel.BLOCK

            // 4. Destructive-but-recoverable.
            lowered.contains("delete all") -> RiskLevel.STRONG_CONFIRM
            lowered.contains("erase") || lowered.contains("wipe") -> RiskLevel.STRONG_CONFIRM
            lowered.contains("remove account") -> RiskLevel.STRONG_CONFIRM
            lowered.contains("delete") -> RiskLevel.CONFIRM

            else -> RiskLevel.DIRECT
        }
    }
}
