package com.unoone.agent.core.agent

import com.unoone.agent.core.model.ToolCall
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Routes commands through deterministic parsers before invoking the model.
 *
 * Deterministic routing handles commands that don't need model reasoning:
 * - Wake commands ("uno on", "uno start", "listen")
 * - Blind mode commands ("start blind", "stop blind")
 * - Simple app launches ("open whatsapp", "open gmail")
 * - Accessibility shortcuts ("go home", "go back", "read screen", "scroll down")
 *
 * Only when NO deterministic match is found does the orchestrator invoke the model pipeline.
 * This saves inference time, reduces model load, and eliminates hallucinated tool calls
 * for commands that have a single obvious interpretation.
 *
 * Each parser returns a [DeterministicResult]:
 * - [DeterministicResult.Matched] — a tool call was determined without the model.
 * - [DeterministicResult.NoMatch] — the command needs model reasoning.
 */
object DeterministicIntentRouter {

    /**
     * Result of attempting deterministic command routing.
     */
    sealed class DeterministicResult {
        /**
         * A deterministic match was found. The command can be executed without model inference.
         * [call] is the resolved tool call. [intent] is the classified intent for logging/analytics.
         */
        data class Matched(val call: ToolCall, val intent: String) : DeterministicResult()

        /** No deterministic match — the command should go to the model pipeline. */
        data object NoMatch : DeterministicResult()
    }

    /**
     * Route a command through deterministic parsers.
     *
     * The order matters: more specific patterns are checked first to avoid false matches.
     * For example, "open whatsapp and send message to Rahul" should NOT match the
     * simple app-launch pattern — it needs model reasoning for the compound intent.
     *
     * @param normalizedCommand the language-normalized, cleaned transcript from [LanguageNormalizer].
     * @return [DeterministicResult.Matched] if a deterministic route was found,
     *   [DeterministicResult.NoMatch] if the command needs model reasoning.
     */
    fun route(normalizedCommand: String): DeterministicResult {
        val lower = normalizedCommand.lowercase().trim()

        // ── Wake commands (no model, no tool) ──────────────────────────────
        if (isWakeCommand(lower)) {
            return DeterministicResult.NoMatch // Wake is handled by the voice layer, not tools
        }

        // ── Language commands are NOT handled here ──────────────────────────
        // Language switching is handled by RuleBasedParser which calls
        // VoiceModule.reinitForLanguage(). Returning a speak_response here
        // would claim the language switch was done without actually reconfiguring
        // STT/TTS, so language commands must fall through to the orchestrator.

        // ── Blind mode commands ─────────────────────────────────────────────
        val blindResult = routeBlindModeCommand(lower)
        if (blindResult != null) return blindResult

        // ── Simple app launches ─────────────────────────────────────────────
        val appResult = routeAppLaunch(lower)
        if (appResult != null) return appResult

        // ── Accessibility shortcuts (no model needed) ─────────────────────
        val accessResult = routeAccessibilityShortcut(lower)
        if (accessResult != null) return accessResult

        // ── Simple voice fast-replies ──────────────────────────────────────
        val fastReplyResult = routeVoiceFastReply(lower)
        if (fastReplyResult != null) return fastReplyResult

        // ── No deterministic match ─────────────────────────────────────────
        return DeterministicResult.NoMatch
    }

    // ── Wake commands ────────────────────────────────────────────────────

    private val WAKE_PHRASES = setOf(
        "uno on", "uno start", "hey uno", "ok uno", "listen",
        "uno suno", "uno shuru", "hello uno"
    )

    private fun isWakeCommand(lower: String): Boolean =
        WAKE_PHRASES.any { lower == it || lower.startsWith("$it ") && !lower.contains(" and ") && !lower.contains(" message ") }

    // ── Language commands are handled by RuleBasedParser / VoiceModule ───
    // Removed from DeterministicIntentRouter because returning a speak_response
    // here did not actually reconfigure STT/TTS. Language switching must go
    // through the orchestrator's voice-language handling path which calls
    // VoiceModule.reinitForLanguage().

    // ── Blind mode commands ──────────────────────────────────────────────

    /** Negation words that should prevent blind mode activation/deactivation. */
    private val NEGATION_WORDS = setOf("don't", "dont", "don't", "do not", "never", "not", "no")

    private fun routeBlindModeCommand(lower: String): DeterministicResult? {
        // Check for negation before matching blind mode commands.
        // "don't start blind mode" should NOT activate; "never stop blind mode" should NOT deactivate.
        val hasNegation = NEGATION_WORDS.any { lower.contains(it) }

        return when {
            !hasNegation && (lower.contains("start blind") || lower.contains("blind mode on") ||
                lower.contains("andha mode shuru")) -> DeterministicResult.Matched(
                ToolCall("speak_response", JsonObject(mapOf(
                    "text" to JsonPrimitive("Blind aid mode activated")
                ))),
                intent = "BLIND_MODE_ON"
            )
            !hasNegation && (lower.contains("stop blind") || lower.contains("blind mode off") ||
                lower.contains("andha mode band")) -> DeterministicResult.Matched(
                ToolCall("speak_response", JsonObject(mapOf(
                    "text" to JsonPrimitive("Blind aid mode deactivated")
                ))),
                intent = "BLIND_MODE_OFF"
            )
            else -> null
        }
    }

    // ── App launches ────────────────────────────────────────────────────

    /** App launch phrases mapped to (packageName, friendlyName). */
    private val APP_LAUNCHES = mapOf(
        "open whatsapp" to ("com.whatsapp" to "WhatsApp"),
        "open gmail" to ("com.google.android.gm" to "Gmail"),
        "open google" to ("com.google.android.googlequicksearchbox" to "Google"),
        "open maps" to ("com.google.android.apps.maps" to "Maps"),
        "open youtube" to ("com.google.android.youtube" to "YouTube"),
        "open calendar" to ("com.google.android.calendar" to "Calendar"),
        "open chrome" to ("com.android.chrome" to "Chrome"),
        "open settings" to ("com.android.settings" to "Settings"),
        "open camera" to ("com.android.camera" to "Camera"),
        "open phone" to ("com.google.android.dialer" to "Phone"),
        "open play store" to ("com.android.vending" to "Play Store"),
        "open clock" to ("com.google.android.deskclock" to "Clock")
    )

    private fun routeAppLaunch(lower: String): DeterministicResult? {
        // Only match simple, single-intent app launches.
        // Compound commands ("open whatsapp and send message") should fall through to the model.
        // Also block patterns like "open whatsapp message to rahul" where a messaging intent
        // follows the app name without "and" as a connector.
        val compoundIndicators = listOf(" and ", " send ", " message ", " text ", " draft ",
            " compose ", " write ", " call ", " email ", " note ", " remind ", " schedule ",
            " add ", " create ", " fill ")
        for ((phrase, _) in APP_LAUNCHES) {
            if (lower == phrase || lower == "$phrase app" ||
                (lower.startsWith("$phrase ") && compoundIndicators.none { lower.contains(it) })) {
                val (packageName, friendlyName) = APP_LAUNCHES[phrase]!!
                return DeterministicResult.Matched(
                    ToolCall("open_app", JsonObject(mapOf(
                        "app_name" to JsonPrimitive(friendlyName),
                        "package_name" to JsonPrimitive(packageName)
                    ))),
                    intent = "APP_LAUNCH"
                )
            }
        }
        return null
    }

    // ── Accessibility shortcuts ─────────────────────────────────────────

    private fun routeAccessibilityShortcut(lower: String): DeterministicResult? {
        return when {
            lower == "go home" || lower == "go home please" -> DeterministicResult.Matched(
                ToolCall("go_home", JsonObject(emptyMap())), "ACCESSIBILITY"
            )
            lower == "go back" || lower == "go back please" -> DeterministicResult.Matched(
                ToolCall("go_back", JsonObject(emptyMap())), "ACCESSIBILITY"
            )
            lower == "read screen" || lower == "read the screen" || lower == "what's on screen" ||
                lower == "what is on screen" || lower == "what's on my screen" -> DeterministicResult.Matched(
                ToolCall("read_screen", JsonObject(emptyMap())), "ACCESSIBILITY"
            )
            lower == "scroll down" || lower == "scroll down please" -> DeterministicResult.Matched(
                ToolCall("scroll", JsonObject(mapOf("direction" to JsonPrimitive("down")))), "ACCESSIBILITY"
            )
            lower == "scroll up" || lower == "scroll up please" -> DeterministicResult.Matched(
                ToolCall("scroll", JsonObject(mapOf("direction" to JsonPrimitive("up")))), "ACCESSIBILITY"
            )
            lower == "open notifications" || lower == "show notifications" -> DeterministicResult.Matched(
                ToolCall("open_notifications", JsonObject(emptyMap())), "ACCESSIBILITY"
            )
            lower == "open recents" || lower == "show recents" || lower == "recent apps" -> DeterministicResult.Matched(
                ToolCall("open_recents", JsonObject(emptyMap())), "ACCESSIBILITY"
            )
            else -> null
        }
    }

    // ── Voice fast replies ──────────────────────────────────────────────

    // "yes" and "no" are intentionally excluded from fast replies because they are
    // context-dependent (confirmation/denial in multi-step flows) and must reach the
    // model pipeline for proper interpretation.
    private val FAST_REPLIES = mapOf(
        "thank you" to "You're welcome!",
        "thanks" to "You're welcome!",
        "stop" to "Okay, stopping.",
        "cancel" to "Cancelled.",
        "never mind" to "Okay, no problem.",
        "ok" to "Okay."
    )

    private fun routeVoiceFastReply(lower: String): DeterministicResult? {
        val reply = FAST_REPLIES[lower.trim()] ?: return null
        return DeterministicResult.Matched(
            ToolCall("speak_response", JsonObject(mapOf("text" to JsonPrimitive(reply)))),
            intent = "VOICE_FAST_REPLY"
        )
    }
}