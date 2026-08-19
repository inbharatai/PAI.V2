package com.unoone.agent.core.agent

import com.unoone.agent.core.model.CanonicalToolRegistry
import com.unoone.agent.core.model.ModelProfile
import com.unoone.agent.core.model.ToolSchema

/**
 * Selects the minimal set of candidate tools for a given task and model tier.
 *
 * The model never sees all 29+ tools at once. Instead, the orchestrator:
 * 1. Routes deterministic commands (wake, language, blind mode, app launch, accessibility) — no model.
 * 2. For model-routed commands, selects only the tools relevant to the task, capped by
 *    [ModelProfile.maxCandidateTools].
 *
 * This prevents hallucinated tool calls, reduces token usage, and makes E2B's small
 * context window viable for tool selection.
 */
object CandidateToolSelector {

    /**
     * Semantic intent categories that map to different tool sets.
     * These correspond to the output of [IntentClassifier] but are coarser-grained
     * to allow deterministic tool selection without model involvement.
     */
    enum class TaskIntent {
        /** Messaging: WhatsApp, SMS, email drafts. */
        MESSAGING,
        /** Calendar: check, create, conflict-check events. */
        CALENDAR,
        /** Note-taking: create, search, summarize, delete notes. */
        NOTES,
        /** Screen understanding: read, OCR, describe scene. */
        SCREEN,
        /** Web browsing or search. */
        WEB,
        /** Accessibility actions: scroll, click, type, navigate. */
        ACCESSIBILITY,
        /** Phone/dialer actions. */
        PHONE,
        /** Document fill/form completion. */
        DOCUMENT,
        /** Skill creation/management. */
        SKILL,
        /** Camera/vision actions. */
        CAMERA,
        /** General chat — no tools needed. */
        CHAT,
        /** Unknown/ambiguous — give the model a reasonable default set. */
        UNKNOWN
    }

    /**
     * The "always included" tools that are useful in almost any agent task.
     * Delegates to [CanonicalToolRegistry.ALWAYS_INCLUDED] so the single source of truth
     * is [CanonicalToolRegistry]. Added to every candidate set unless the intent is CHAT (no tools).
     */
    private val ALWAYS_INCLUDED: List<String> = CanonicalToolRegistry.ALWAYS_INCLUDED.toList()

    /**
     * Tool sets by intent. Each intent maps to the tools the model might need,
     * listed in priority order. The [ModelProfile.maxCandidateTools] cap is applied
     * after merging with [ALWAYS_INCLUDED].
     */
    private val TOOLS_BY_INTENT: Map<TaskIntent, List<String>> = mapOf(
        TaskIntent.MESSAGING to listOf(
            "resolve_contact",
            "draft_whatsapp_message",
            "send_prepared_whatsapp",
            "draft_email",
            "speak_response"
        ),
        TaskIntent.CALENDAR to listOf(
            "check_calendar_conflict",
            "create_calendar_event",
            "check_calendar",
            "open_calendar",
            "speak_response"
        ),
        TaskIntent.NOTES to listOf(
            "create_note",
            "search_notes",
            "summarize_text",
            "delete_notes",
            "speak_response"
        ),
        TaskIntent.SCREEN to listOf(
            "read_screen",
            "ocr_screen",
            "describe_scene",
            "detect_objects",
            "speak_response"
        ),
        TaskIntent.WEB to listOf(
            "web_search",
            "open_chrome",
            "open_url",
            "secure_browser_task",
            "speak_response"
        ),
        TaskIntent.ACCESSIBILITY to listOf(
            "go_home",
            "go_back",
            "scroll",
            "click_accessibility_node",
            "type_into_accessibility_node",
            "open_notifications",
            "open_recents",
            "speak_response"
        ),
        TaskIntent.PHONE to listOf(
            "open_dialer",
            "resolve_contact",
            "speak_response"
        ),
        TaskIntent.DOCUMENT to listOf(
            "prepare_document_fill",
            "speak_response"
        ),
        TaskIntent.SKILL to listOf(
            "create_skill",
            "speak_response"
        ),
        TaskIntent.CAMERA to listOf(
            "open_camera",
            "detect_objects",
            "speak_response"
        ),
        TaskIntent.CHAT to emptyList(),
        TaskIntent.UNKNOWN to listOf(
            "speak_response",
            "search_notes",
            "web_search",
            "open_app"
        )
    )

    /**
     * Select candidate tools for a given task intent and model profile.
     *
     * Returns a list of [ToolSchema] objects, limited to [profile.maxCandidateTools],
     * always including [ALWAYS_INCLUDED] tools. If the intent is CHAT, returns an empty
     * list (no tools needed for free-form conversation).
     *
     * @param intent the semantic task intent, derived from [IntentClassifier] or
     *   deterministic routing.
     * @param profile the active model profile governing tool-set size.
     * @return the candidate tool schemas to register with the planning conversation.
     */
    fun select(intent: TaskIntent, profile: ModelProfile): List<ToolSchema> {
        if (intent == TaskIntent.CHAT) return emptyList()

        val intentTools = TOOLS_BY_INTENT[intent] ?: TOOLS_BY_INTENT[TaskIntent.UNKNOWN]!!

        // speak_response is always included regardless of cap — it's the model's escape hatch.
        // Reserve exactly one slot for speak_response; take intent tools up to (maxCandidateTools - 1).
        // Guard against zero/negative profiles: if maxCandidateTools <= 1, only speak_response survives.
        val nonSpeakTools = intentTools.filterNot { it == "speak_response" }
        val cap = (profile.maxCandidateTools - 1).coerceAtLeast(0)
        val cappedIntent = nonSpeakTools.take(cap)

        // Always include speak_response as the last tool.
        val merged = cappedIntent + ALWAYS_INCLUDED

        // Resolve to schemas, silently dropping unknown names
        return merged.mapNotNull { CanonicalToolRegistry.schemaFor(it) }
    }

    /**
     * Select candidate tools when a deterministic parser already produced a tool call.
     * In this case the model only needs the one tool (+ speak_response for follow-up).
     *
     * @param toolName the tool name from deterministic parsing.
     * @param profile the active model profile.
     * @return the minimal candidate set for the model to confirm or follow up.
     */
    fun selectForDeterministic(toolName: String, profile: ModelProfile): List<ToolSchema> {
        val tools = mutableListOf<String>()
        tools.add(toolName)
        if (toolName != "speak_response") tools.add("speak_response")
        val capped = tools.take(profile.maxCandidateTools)
        return capped.mapNotNull { CanonicalToolRegistry.schemaFor(it) }
    }

    /**
     * Infer [TaskIntent] from a raw user command string.
     * Uses simple keyword heuristics — no model involvement.
     * Falls back to [TaskIntent.UNKNOWN] for ambiguous commands.
     */
    fun inferIntent(command: String): TaskIntent {
        val lower = command.lowercase().trim()

        // Priority-ordered intent detection. More-specific patterns are checked first
        // to avoid misclassification of compound phrases (e.g. "search notes" → NOTES not WEB).

        // Messaging (compound intent — needs model for content drafting)
        if (lower.contains("whatsapp") || lower.contains("message") || lower.contains("send message") ||
            lower.contains("draft email") || lower.contains("compose email") || lower.contains("sms")) {
            return TaskIntent.MESSAGING
        }

        // Calendar (compound intent — needs model for date/time parsing)
        if (lower.contains("calendar") || lower.contains("meeting") || lower.contains("schedule") ||
            lower.contains("appointment") || lower.contains("event")) {
            return TaskIntent.CALENDAR
        }

        // Notes — checked before WEB so "search notes" routes to NOTES not WEB
        if (lower.contains("note") || lower.contains("memo") || lower.contains("remember") ||
            lower.contains("remind") || lower.contains("summarize")) {
            return TaskIntent.NOTES
        }

        // Screen understanding
        if (lower.contains("screen") || lower.contains("read screen") || lower.contains("what's on") ||
            lower.contains("describe") || lower.contains("ocr")) {
            return TaskIntent.SCREEN
        }

        // Web — "search" without "note" context
        if ((lower.contains("search") && !lower.contains("note")) || lower.contains("browse") ||
            lower.contains("website") || lower.contains("url") || lower.contains("internet")) {
            return TaskIntent.WEB
        }

        // Accessibility (deterministic — rarely needs model)
        if (lower.contains("scroll") || lower.contains("click") || lower.contains("go home") ||
            lower.contains("go back") || lower.contains("navigate") || lower.contains("type in")) {
            return TaskIntent.ACCESSIBILITY
        }

        // Phone (deterministic)
        if (lower.contains("call") || lower.contains("dial") || lower.contains("phone")) {
            return TaskIntent.PHONE
        }

        // Document (compound — form filling needs model)
        if (lower.contains("document") || lower.contains("form") || lower.contains("fill") ||
            lower.contains("pdf")) {
            return TaskIntent.DOCUMENT
        }

        // Camera (deterministic)
        if (lower.contains("camera") || lower.contains("photo") || lower.contains("detect")) {
            return TaskIntent.CAMERA
        }

        // Skill (compound — needs model for step parsing)
        if (lower.contains("skill") || lower.contains("teach") || lower.contains("automation")) {
            return TaskIntent.SKILL
        }

        return TaskIntent.UNKNOWN
    }
}