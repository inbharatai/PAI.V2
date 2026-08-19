package com.unoone.agent.core.model

/**
 * Parameter type for a tool argument, mirroring the LiteRT-LM `@ToolParam` allowed types
 * (String, Int, Boolean, Float, Double, or a List of these). Kept dependency-free in `:core` so
 * the canonical schema is visible to the brain (validation), safety, and test layers without any
 * of them depending on LiteRT-LM.
 */
enum class ToolParamType { STRING, INT, BOOLEAN, FLOAT, DOUBLE, STRING_LIST }

/**
 * One argument of a tool: its name, type, and whether it is required. A parameter is optional when
 * the UnoOne `@Tool` declaration gives it a nullable type or a default value.
 */
data class ToolParamSchema(
    val name: String,
    val type: ToolParamType,
    val required: Boolean,
    /** Human-readable description of this parameter, used in tool descriptors sent to the model. */
    val description: String? = null
)

/**
 * The full schema for one UnoOne tool: its name, description, and ordered argument schemas. This is
 * the single canonical description the brain validates a model-proposed tool call against before the
 * call is ever handed to [com.unoone.agent.execution.ActionExecutor] or
 * [com.unoone.agent.safety.SafetyGuard].
 *
 * Tool names are **never renamed** here: they match the `@Tool` method names in
 * [com.unoone.agent.localbrain.UnoOneToolSet], the `when` branches in `ActionExecutor`, the
 * `riskRules` keys in `SafetyGuard`, and the entries in `ToolPermissionRegistry`. Renaming would
 * break trained tool-call datasets and the existing tests.
 */
data class ToolSchema(
    val name: String,
    /** Human-readable description of this tool, used in tool descriptors sent to the model. */
    val description: String,
    val params: List<ToolParamSchema>
) {
    fun param(name: String): ToolParamSchema? = params.firstOrNull { it.name == name }
    val requiredParams: List<ToolParamSchema> get() = params.filter { it.required }
}

/**
 * The one authoritative list of UnoOne tools. The brain ([com.unoone.agent.localbrain.GemmaPlanner])
 * rejects any tool call whose name is not in [names] and validates that every required argument is
 * present and type-compatible before forwarding the call. Tests enforce that `SafetyGuard`,
 * `UnoOneToolSet`, `ActionExecutor`, and `ToolPermissionRegistry` all agree with this list.
 *
 * Descriptions are the single source of truth for [DynamicToolProvider], which constructs
 * LiteRT-LM tool descriptors from [ToolSchema] and these descriptions — bypassing the reflection-based
 * [com.google.ai.edge.litertlm.ToolSet] mechanism that would expose all 42 tools to the model at once.
 */
object CanonicalToolRegistry {

    val create_note = ToolSchema("create_note", "Create a local note", listOf(
        ToolParamSchema("title", ToolParamType.STRING, required = true, description = "Short title for the note"),
        ToolParamSchema("content", ToolParamType.STRING, required = true, description = "Full note content"),
        ToolParamSchema("tags", ToolParamType.STRING, required = false, description = "Optional comma-separated tags")
    ))
    val search_notes = ToolSchema("search_notes", "Search saved notes by keyword", listOf(
        ToolParamSchema("query", ToolParamType.STRING, true, description = "Search query")
    ))
    val summarize_text = ToolSchema("summarize_text", "Summarize a block of text", listOf(
        ToolParamSchema("text", ToolParamType.STRING, true, description = "Text to summarize")
    ))
    val speak_response = ToolSchema("speak_response", "Speak a response to the user through offline TTS", listOf(
        ToolParamSchema("text", ToolParamType.STRING, true, description = "Text to speak")
    ))
    val voice_recording = ToolSchema("voice_recording", "Record a short voice memo, transcribe it offline, and save it as a note", listOf(
        ToolParamSchema("duration_seconds", ToolParamType.INT, required = false, description = "Max recording duration in seconds (1-30, default 5)"),
        ToolParamSchema("title", ToolParamType.STRING, required = false, description = "Optional note title; defaults to the start of the transcription")
    ))
    val web_search = ToolSchema("web_search", "Search the web for an answer (online only; returns nothing offline). Uses a privacy-respecting scrape, never auto-opens links", listOf(
        ToolParamSchema("query", ToolParamType.STRING, true, description = "Search query")
    ))
    val open_chrome = ToolSchema("open_chrome", "Open the Chrome browser", emptyList())
    val open_app = ToolSchema("open_app", "Open any installed app by package name or friendly name", listOf(
        ToolParamSchema("app_name", ToolParamType.STRING, required = true, description = "Human-readable app name, e.g. WhatsApp"),
        ToolParamSchema("package_name", ToolParamType.STRING, required = false, description = "Exact Android package name if known")
    ))
    val open_url = ToolSchema("open_url", "Open a web URL after safety sanitization", listOf(
        ToolParamSchema("url", ToolParamType.STRING, true, description = "URL to open")
    ))
    val open_camera = ToolSchema("open_camera", "Launch the device camera app", emptyList())
    val system_control = ToolSchema("system_control", "Control the device UI via AccessibilityService", listOf(
        ToolParamSchema("action", ToolParamType.STRING, required = true, description = "Action: click, type, fill, scroll_up, scroll_down, swipe, go_back, go_home, open_notifications, open_recents, find_and_click, long_press, read_screen"),
        ToolParamSchema("target", ToolParamType.STRING, required = false, description = "Target text or direction"),
        ToolParamSchema("value", ToolParamType.STRING, required = false, description = "Value for fill/type actions")
    ))

    // --- Atomic accessibility tools (replacing system_control) ---

    /** Navigate to the device home screen. */
    val go_home = ToolSchema("go_home", "Navigate to the device home screen", emptyList())

    /** Navigate back to the previous screen. */
    val go_back = ToolSchema("go_back", "Navigate back to the previous screen", emptyList())

    /** Scroll in a direction. */
    val scroll = ToolSchema("scroll", "Scroll in a direction: up, down, left, or right", listOf(
        ToolParamSchema("direction", ToolParamType.STRING, required = true, description = "Direction to scroll: up, down, left, or right")
    ))

    /** Click an accessibility node by its node ID. */
    val click_accessibility_node = ToolSchema("click_accessibility_node", "Click an accessibility node by its node ID for precise UI interaction", listOf(
        ToolParamSchema("node_id", ToolParamType.STRING, required = true, description = "The accessibility node ID to click")
    ))

    /** Type text into an accessibility node by its node ID. */
    val type_into_accessibility_node = ToolSchema("type_into_accessibility_node", "Type text into an accessibility node by its node ID", listOf(
        ToolParamSchema("node_id", ToolParamType.STRING, required = true, description = "The accessibility node ID to type into"),
        ToolParamSchema("text", ToolParamType.STRING, required = true, description = "The text to type")
    ))

    /** Open the notification shade. */
    val open_notifications = ToolSchema("open_notifications", "Open the notification shade", emptyList())

    /** Open the recent apps switcher. */
    val open_recents = ToolSchema("open_recents", "Open the recent apps switcher", emptyList())

    /** Long-press an accessibility node by its node ID. */
    val long_press_accessibility_node = ToolSchema("long_press_accessibility_node", "Long-press an accessibility node by its node ID", listOf(
        ToolParamSchema("node_id", ToolParamType.STRING, required = true, description = "The accessibility node ID to long-press")
    ))
    val read_screen = ToolSchema("read_screen", "Read visible text from the current screen", emptyList())
    val ocr_screen = ToolSchema("ocr_screen", "Capture a screenshot and run OCR for apps without accessibility labels", emptyList())
    val create_skill = ToolSchema("create_skill", "Create a reusable multi-step skill", listOf(
        ToolParamSchema("name", ToolParamType.STRING, required = true, description = "Skill name"),
        ToolParamSchema("steps", ToolParamType.STRING_LIST, required = true, description = "List of step descriptions")
    ))
    val draft_email = ToolSchema("draft_email", "Draft an email; the user must review and press send", listOf(
        ToolParamSchema("to", ToolParamType.STRING, required = true, description = "Recipient email address"),
        ToolParamSchema("subject", ToolParamType.STRING, required = true, description = "Email subject"),
        ToolParamSchema("body", ToolParamType.STRING, required = true, description = "Email body")
    ))
    val send_whatsapp = ToolSchema("send_whatsapp", "Open WhatsApp with a pre-filled message; the user must press send", listOf(
        ToolParamSchema("number", ToolParamType.STRING, required = true, description = "Phone number with optional country code"),
        ToolParamSchema("message", ToolParamType.STRING, required = true, description = "Message text")
    ))

    // --- Messaging tools (replacing send_whatsapp with resolve-draft-send sequence) ---

    /** Resolve a contact name to a WhatsApp-reachable phone number. */
    val resolve_contact = ToolSchema("resolve_contact", "Resolve a contact name to a WhatsApp-reachable phone number. Use this first before drafting a WhatsApp message.", listOf(
        ToolParamSchema("query", ToolParamType.STRING, required = true, description = "Contact name or query to look up")
    ))

    /** Open WhatsApp with a pre-filled message draft for the user to review and send. */
    val draft_whatsapp_message = ToolSchema("draft_whatsapp_message", "Open WhatsApp with a pre-filled message draft for the specified contact. The user must review and press send.", listOf(
        ToolParamSchema("contact_name", ToolParamType.STRING, required = true, description = "Contact name or phone number"),
        ToolParamSchema("message", ToolParamType.STRING, required = true, description = "Message text to pre-fill")
    ))

    /** Send a prepared WhatsApp message after user confirmation. */
    val send_prepared_whatsapp = ToolSchema("send_prepared_whatsapp", "Send a prepared WhatsApp message after user confirmation", listOf(
        ToolParamSchema("contact_name", ToolParamType.STRING, required = true, description = "Contact name or phone number"),
        ToolParamSchema("message", ToolParamType.STRING, required = true, description = "Message text to send")
    ))
    val check_calendar = ToolSchema("check_calendar", "Check today's calendar events", emptyList())
    val open_calendar = ToolSchema("open_calendar", "Open the device's default calendar app", emptyList())
    val open_calendar_insert = ToolSchema("open_calendar_insert", "Open the calendar event creator", listOf(
        ToolParamSchema("title", ToolParamType.STRING, required = true, description = "Event title"),
        ToolParamSchema("start_time", ToolParamType.STRING, required = false, description = "Optional start time ISO string"),
        ToolParamSchema("end_time", ToolParamType.STRING, required = false, description = "Optional end time ISO string")
    ))

    // --- Calendar tools (replacing open_calendar_insert with check-then-create) ---

    /** Check for calendar conflicts at a proposed time. */
    val check_calendar_conflict = ToolSchema("check_calendar_conflict", "Check for calendar conflicts at a proposed time before creating an event", listOf(
        ToolParamSchema("date", ToolParamType.STRING, required = false, description = "Date for the event (e.g. '2026-07-22')"),
        ToolParamSchema("start_time", ToolParamType.STRING, required = false, description = "Start time (e.g. '15:00')"),
        ToolParamSchema("end_time", ToolParamType.STRING, required = false, description = "End time (e.g. '16:00')")
    ))

    /** Create a calendar event (opens calendar insert intent after conflict check). */
    val create_calendar_event = ToolSchema("create_calendar_event", "Create a calendar event. Always check for conflicts first using check_calendar_conflict.", listOf(
        ToolParamSchema("title", ToolParamType.STRING, required = true, description = "Event title"),
        ToolParamSchema("date", ToolParamType.STRING, required = false, description = "Date for the event (e.g. '2026-07-22')"),
        ToolParamSchema("start_time", ToolParamType.STRING, required = false, description = "Start time (e.g. '15:00')"),
        ToolParamSchema("end_time", ToolParamType.STRING, required = false, description = "End time (e.g. '16:00')")
    ))
    val open_dialer = ToolSchema("open_dialer", "Open the phone dialer with an optional number", listOf(
        ToolParamSchema("number", ToolParamType.STRING, required = false, description = "Optional phone number")
    ))
    val share_text = ToolSchema("share_text", "Share text through the system share sheet", listOf(
        ToolParamSchema("text", ToolParamType.STRING, true, description = "Text to share")
    ))
    val delete_notes = ToolSchema("delete_notes", "Delete notes matching a query", listOf(
        ToolParamSchema("query", ToolParamType.STRING, true, description = "Query to match notes for deletion")
    ))
    val delete_all_notes = ToolSchema("delete_all_notes", "Delete all saved notes", emptyList())
    val export_data = ToolSchema("export_data", "Export user data", emptyList())
    val detect_objects = ToolSchema("detect_objects", "Activate blind-aid camera and obstacle detection", emptyList())
    val deactivate_blind_aid = ToolSchema("deactivate_blind_aid", "Deactivate blind-aid camera and obstacle detection", emptyList())
    /**
     * Describe the current screen as a short scene: foreground app + OCR text (always available with
     * the shipped text-only models), optionally augmented by Gemma multimodal vision when a
     * vision-capable `.litertlm` artifact is loaded (device-time, inactive until then). Capturing +
     * analyzing the screen can read sensitive content, so the safety tier is STRONG_CONFIRM.
     */
    val describe_scene = ToolSchema("describe_scene", "Describe the current screen as a short scene (foreground app + visible text); optionally narrow what to look for with aspect. Sensitive: captures the screen.", listOf(
        ToolParamSchema("aspect", ToolParamType.STRING, required = false, description = "Optional what to focus on, e.g. 'buttons', 'the total', 'any OTP'")
    ))
    /**
     * Drive the UnoOne Secure Browser (Page Agent on a hardened WebView) and run a task.
     * Standard mode resolves and gates the target to an approved origin. Explicit Prototype/Off
     * admits arbitrary public HTTPS targets. Transport restrictions and the session-bound bridge
     * remain enforced. Risk tier CONFIRM in Standard: it drives a browser.
     */
    val secure_browser_task = ToolSchema("secure_browser_task", "Open the UnoOne Secure Browser and run a PageAgent task. Standard accepts approved origins such as unigurus, uniassist, testsprep and inbharat. Explicit Prototype/Off accepts any public HTTPS URL. Standard keeps confirmations and sensitive-step gates enabled.", listOf(
        ToolParamSchema("origin", ToolParamType.STRING, required = true, description = "Approved origin: friendly name ('unigurus'), bare host ('unigurus.com'), or full HTTPS URL"),
        ToolParamSchema("task", ToolParamType.STRING, required = true, description = "What UnoOne should do on the page, e.g. 'fill the profile form and stop before final submission'")
    ))
    /** Opens the offline Document Agent picker for a fillable PDF or DOCX template. */
    val prepare_document_fill = ToolSchema("prepare_document_fill", "Open the fully offline Document Agent to fill a PDF AcroForm or DOCX template and save a new copy", listOf(
        ToolParamSchema("format", ToolParamType.STRING, required = true, description = "Document format: pdf or docx")
    ))

    /** All canonical tools, in declaration order. New atomic tools are included alongside legacy ones. */
    val tools: List<ToolSchema> = listOf(
        create_note, search_notes, summarize_text, speak_response, voice_recording, web_search,
        open_chrome, open_app, open_url, open_camera, system_control, read_screen, ocr_screen,
        create_skill, draft_email, send_whatsapp, check_calendar, open_calendar, open_calendar_insert,
        open_dialer, share_text, delete_notes, delete_all_notes, export_data, detect_objects,
        deactivate_blind_aid, describe_scene, secure_browser_task, prepare_document_fill,
        // Atomic accessibility tools (prefer over system_control)
        go_home, go_back, scroll, click_accessibility_node, type_into_accessibility_node,
        open_notifications, open_recents, long_press_accessibility_node,
        // Messaging tools (prefer over send_whatsapp)
        resolve_contact, draft_whatsapp_message, send_prepared_whatsapp,
        // Calendar tools (prefer over open_calendar_insert)
        check_calendar_conflict, create_calendar_event
    )

    /** The set of tool names a model is allowed to propose. Anything else is rejected by the brain. */
    val names: Set<String> = tools.map { it.name }.toSet()

    /** Schema for a tool name, or null if the name is not a canonical UnoOne tool. */
    fun schemaFor(name: String): ToolSchema? = tools.firstOrNull { it.name == name }

    /** True iff [name] is a canonical UnoOne tool the model may propose. */
    fun isKnown(name: String): Boolean = name in names

    /**
     * Legacy tool names that have been decomposed into finer-grained atomic tools.
     * The old names are still accepted for backward compatibility with model outputs and
     * stored conversation state, but new code should use the replacement tools.
     */
    val LEGACY_TOOL_NAMES: Set<String> = setOf("system_control", "send_whatsapp", "open_calendar_insert")

    /**
     * Maps a legacy tool name + action to the new atomic tool name.
     * For system_control, the action parameter determines the replacement.
     * Returns null if the tool name is not a legacy tool.
     */
    fun migrateToolName(legacyName: String, action: String? = null): String? = when (legacyName) {
        "system_control" -> when (action) {
            "go_home" -> "go_home"
            "go_back" -> "go_back"
            "scroll_up", "scroll_down" -> "scroll"
            "click", "find_and_click" -> "click_accessibility_node"
            "type", "fill" -> "type_into_accessibility_node"
            "open_notifications" -> "open_notifications"
            "open_recents" -> "open_recents"
            "long_press" -> "long_press_accessibility_node"
            else -> "system_control" // unknown action, keep legacy
        }
        "send_whatsapp" -> "draft_whatsapp_message" // default migration
        "open_calendar_insert" -> "create_calendar_event" // default migration
        else -> null // not a legacy tool
    }

    /** Tool names that are considered "atomic" replacements for system_control. */
    val ATOMIC_ACCESSIBILITY_TOOLS: Set<String> = setOf(
        "go_home", "go_back", "scroll", "click_accessibility_node",
        "type_into_accessibility_node", "open_notifications", "open_recents",
        "long_press_accessibility_node"
    )

    /** Tool names for the messaging resolve-draft-send sequence. */
    val MESSAGING_TOOLS: Set<String> = setOf(
        "resolve_contact", "draft_whatsapp_message", "send_prepared_whatsapp"
    )

    /** Tool names for the calendar check-then-create sequence. */
    val CALENDAR_TOOLS: Set<String> = setOf(
        "check_calendar_conflict", "create_calendar_event"
    )

    /** Tools that must always be registered regardless of the candidate set. */
    val ALWAYS_INCLUDED: Set<String> = setOf("speak_response")
}