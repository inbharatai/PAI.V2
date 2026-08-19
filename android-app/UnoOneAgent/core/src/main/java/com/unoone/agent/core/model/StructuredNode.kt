package com.unoone.agent.core.model

/**
 * A structured representation of a single node in the accessibility tree.
 * Unlike flat text dumps, this carries identity and hierarchy info so the LLM
 * can target specific nodes with [click_accessibility_node] and [type_into_accessibility_node].
 *
 * Designed to be compact (no child references — the tree is represented as a flat list
 * ordered by depth-first traversal, with [depth] encoding nesting).
 */
data class StructuredNode(
    /** The view ID resource name, e.g. "com.whatsapp:id/send_button". Null if the node has no ID. */
    val nodeId: String?,
    /** Visible text content (text or contentDescription). Blank if none. */
    val text: String,
    /** Node type inferred from className: button, edit, text, image, switch, checkbox, etc. */
    val type: String,
    /** Whether this node is clickable. */
    val clickable: Boolean,
    /** Bounding rectangle in screen coordinates: "left,top-right,bottom". */
    val bounds: String,
    /** Nesting depth in the accessibility tree (0 = root). */
    val depth: Int
)