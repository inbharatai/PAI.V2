package com.unoone.agent.core.agent

import com.unoone.agent.core.model.StructuredNode

/**
 * Pure-JVM inputs to [SceneDescriptionBuilder]: the structured signals available about the current
 * screen without any vision model. All fields are optional/nullable so the builder degrades
 * gracefully when a signal is absent (e.g. no accessibility context, blank OCR).
 */
data class SceneInput(
    val currentPackage: String = "",
    val currentActivity: String = "",
    val ocrText: String = "",
    /** What the user asked the scene to focus on, e.g. "buttons", "the total amount", "any OTP". */
    val aspect: String = "",
    /** Structured accessibility nodes with IDs, text, type, clickability, bounds, and depth. */
    val structuredNodes: List<StructuredNode> = emptyList()
)

/**
 * Pure-JVM scene-description builder: turns the non-vision signals about the current screen (the
 * foreground app/activity + OCR text) into a compact, human-readable scene description string.
 *
 * This is the JVM-tested, always-available control half of the `describe_scene` tool
 * ([com.unoone.agent.execution.ActionExecutor]). It works with the shipped **text-only** Gemma
 * models — no vision weights required — so `describe_scene` is useful today. The LiteRT-LM
 * multimodal path ([com.google.ai.edge.litertlm.Content.ImageBytes] +
 * `EngineConfig.visionBackend`) is wired in [com.unoone.agent.localbrain.GemmaPlanner] but is
 * INACTIVE until a vision-capable `.litertlm` artifact ships; when vision is unavailable the
 * orchestrator falls back to this builder. Nothing here touches Android, LiteRT-LM, or Room, so the
 * description contract is fully unit-testable.
 *
 * Honesty: this is a structured description built from OCR + foreground context, not a true visual
 * understanding of objects/layout. It never fabricates screen content — when OCR is blank and no
 * app context is known, it says so explicitly rather than inventing a scene.
 */
object SceneDescriptionBuilder {

    /** Max OCR lines / chars surfaced so the spoken description stays brief. */
    const val MAX_OCR_LINES: Int = 8
    const val MAX_OCR_CHARS: Int = 500
    /** Max nodes included in the structured tree section. */
    const val MAX_STRUCTURED_NODES: Int = 30
    /** Max chars for the structured tree section to fit in the LLM context window. */
    const val MAX_STRUCTURED_CHARS: Int = 1500

    /**
     * Builds the description. Deterministic and side-effect-free. Output shape:
     * "Scene: <app>. <aspect focus>. Visible text: <lines>." or an explicit "no readable content"
     * line when nothing was visible. Never returns blank — a scene with no signals still reports
     * that the screen content could not be read.
     *
     * When [SceneInput.structuredNodes] is non-empty, a compact node list is appended so the
     * LLM can identify specific nodes for click_accessibility_node / type_into_accessibility_node.
     */
    fun build(input: SceneInput): String {
        val aspect = input.aspect.trim()
        val app = appLabel(input)
        val lines = ocrLines(input.ocrText)
        val nodes = input.structuredNodes

        // A focus directive ("aspect") is NOT scene content — if we have no foreground app and no
        // readable text and no structured nodes, honestly say so (mentioning what we were asked to
        // look for) rather than inventing a scene.
        if (app.isBlank() && lines.isEmpty() && nodes.isEmpty()) {
            return if (aspect.isNotBlank())
                "I could not read any text on the screen to look for $aspect."
            else
                "I could not read any text on the screen."
        }

        val parts = mutableListOf<String>()
        if (app.isNotBlank()) parts += "Scene: $app"
        if (aspect.isNotBlank()) parts += "Looking for: $aspect"
        if (lines.isNotEmpty()) {
            parts += "Visible text: ${lines.joinToString("; ").take(MAX_OCR_CHARS)}"
        }
        if (nodes.isNotEmpty()) {
            parts += formatStructuredNodes(nodes)
        }
        return parts.joinToString(" ")
    }

    /**
     * Formats structured nodes as a compact multi-line block the LLM can reference.
     * Each line: "[nodeId] type(text) clickable bounds @depth"
     * nodeId is omitted when null. Output is capped to [MAX_STRUCTURED_CHARS] characters.
     */
    fun formatStructuredNodes(nodes: List<StructuredNode>): String {
        val capped = nodes.take(MAX_STRUCTURED_NODES)
        val sb = StringBuilder("UI nodes:\n")
        for (node in capped) {
            val id = node.nodeId?.let { "[$it] " } ?: ""
            val textPart = if (node.text.isNotBlank()) " \"${node.text.take(60)}\"" else ""
            val line = "${id}${node.type}${textPart} ${if (node.clickable) "clickable" else ""} ${node.bounds} @${node.depth}\n"
            if (sb.length + line.length > MAX_STRUCTURED_CHARS) break
            sb.append(line)
        }
        return sb.toString().trimEnd()
    }

    /** A short, human label for the foreground context: "App (activity)" or just one if only known. */
    private fun appLabel(input: SceneInput): String {
        val pkg = input.currentPackage.trim()
        val act = input.currentActivity.trim()
        return when {
            pkg.isNotBlank() && act.isNotBlank() -> "$pkg ($act)"
            pkg.isNotBlank() -> pkg
            act.isNotBlank() -> act
            else -> ""
        }
    }

    /**
     * Extracts up to [MAX_OCR_LINES] meaningful, distinct lines from raw OCR text: drops blank lines,
     * collapses duplicates, and trims each. OCR arrives as one big string (possibly newline- or
     * space-separated); we split on newlines and fall back to the whole blob.
     */
    private fun ocrLines(rawOcr: String): List<String> {
        val text = rawOcr.trim()
        if (text.isBlank()) return emptyList()
        val seen = LinkedHashSet<String>()
        for (line in text.split("\n")) {
            val cleaned = line.trim()
            if (cleaned.isNotBlank()) seen.add(cleaned)
        }
        return seen.toList().take(MAX_OCR_LINES)
    }
}