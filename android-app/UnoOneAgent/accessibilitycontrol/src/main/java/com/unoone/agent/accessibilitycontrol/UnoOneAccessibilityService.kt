package com.unoone.agent.accessibilitycontrol

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.graphics.Path
import android.os.Bundle
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import com.unoone.agent.core.model.StructuredNode
import com.unoone.agent.core.util.Logger
import com.unoone.agent.core.runtime.AgentRuntimeGate

class UnoOneAccessibilityService : AccessibilityService() {

    @Volatile var currentPackage: String? = null
        private set
    @Volatile var currentActivity: String? = null
        private set

    override fun onAccessibilityEvent(event: AccessibilityEvent) {
        if (!AgentRuntimeGate.isEnabled()) return
        if (event.eventType == AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED) {
            currentPackage = event.packageName?.toString()
            currentActivity = event.className?.toString()
        }
    }

    override fun onInterrupt() {
        Logger.w("Accessibility Service Interrupted")
    }

    override fun onServiceConnected() {
        super.onServiceConnected()
        Logger.i("Accessibility Service Connected - UnoOne is now in control")
        instance = this
    }

    override fun onDestroy() {
        super.onDestroy()
        instance = null
    }

    fun clickAt(x: Float, y: Float): Boolean {
        val path = Path()
        path.moveTo(x, y)
        val builder = GestureDescription.Builder()
        builder.addStroke(GestureDescription.StrokeDescription(path, 0, 100))
        return dispatchGesture(builder.build(), null, null)
    }

    fun clickNodeWithText(text: String): Boolean {
        val rootNode = rootInActiveWindow ?: return false
        try {
            val nodes = rootNode.findAccessibilityNodeInfosByText(text)
            try {
                for (node in nodes) {
                    if (node.isClickable) {
                        return node.performAction(AccessibilityNodeInfo.ACTION_CLICK)
                    }
                    var parent = node.parent
                    while (parent != null) {
                        if (parent.isClickable) {
                            val clicked = parent.performAction(AccessibilityNodeInfo.ACTION_CLICK)
                            parent.recycle()
                            return clicked
                        }
                        val grandParent = parent.parent
                        parent.recycle()
                        parent = grandParent
                    }
                }
                return false
            } finally {
                nodes.forEach { it.recycle() }
            }
        } finally {
            rootNode.recycle()
        }
    }

    fun typeTextIntoFocused(text: String): Boolean {
        val rootNode = rootInActiveWindow ?: return false
        try {
            val focusedNode = rootNode.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return false
            try {
                val arguments = Bundle()
                arguments.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
                return focusedNode.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments)
            } finally {
                focusedNode.recycle()
            }
        } finally {
            rootNode.recycle()
        }
    }

    fun fillFieldWithText(hint: String, text: String): Boolean {
        val rootNode = rootInActiveWindow ?: return false
        try {
            val nodes = rootNode.findAccessibilityNodeInfosByText(hint)
            try {
                for (node in nodes) {
                    if (node.isEditable) {
                        val arguments = Bundle()
                        arguments.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
                        return node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments)
                    }
                }
                return false
            } finally {
                nodes.forEach { it.recycle() }
            }
        } finally {
            rootNode.recycle()
        }
    }

    @Suppress("DEPRECATION")
    fun captureVisibleText(): List<String> {
        val rootNode = rootInActiveWindow ?: return emptyList()
        val texts = mutableListOf<String>()
        try {
            fun traverse(node: AccessibilityNodeInfo) {
                if (node.isVisibleToUser) {
                    val text = node.text?.toString() ?: node.contentDescription?.toString()
                    if (!text.isNullOrBlank()) {
                        texts.add(text)
                    }
                }
                for (i in 0 until node.childCount) {
                    node.getChild(i)?.let { child ->
                        traverse(child)
                        child.recycle() // Always recycle child nodes to prevent memory leaks
                    }
                }
            }
            traverse(rootNode)
        } finally {
            rootNode.recycle()
        }
        return texts.distinct()
    }

    fun scrollDown(): Boolean {
        val rootNode = rootInActiveWindow ?: return false
        try {
            val bounds = android.graphics.Rect()
            rootNode.getBoundsInScreen(bounds)
            val centerX = bounds.exactCenterX()
            val startY = bounds.exactCenterY() + bounds.height() * 0.25f
            val endY = bounds.exactCenterY() - bounds.height() * 0.25f
            return performSwipe(centerX, startY, centerX, endY, 500L)
        } finally {
            rootNode.recycle()
        }
    }

    fun scrollUp(): Boolean {
        val rootNode = rootInActiveWindow ?: return false
        try {
            val bounds = android.graphics.Rect()
            rootNode.getBoundsInScreen(bounds)
            val centerX = bounds.exactCenterX()
            val startY = bounds.exactCenterY() - bounds.height() * 0.25f
            val endY = bounds.exactCenterY() + bounds.height() * 0.25f
            return performSwipe(centerX, startY, centerX, endY, 500L)
        } finally {
            rootNode.recycle()
        }
    }

    fun swipe(startX: Float, startY: Float, endX: Float, endY: Float, durationMs: Long = 300): Boolean {
        val path = Path()
        path.moveTo(startX, startY)
        path.lineTo(endX, endY)
        val stroke = GestureDescription.StrokeDescription(path, 0, durationMs)
        return dispatchGesture(GestureDescription.Builder().addStroke(stroke).build(), null, null)
    }

    fun longPress(x: Float, y: Float): Boolean {
        val path = Path()
        path.moveTo(x, y)
        val stroke = GestureDescription.StrokeDescription(path, 0, 1000L)
        return dispatchGesture(GestureDescription.Builder().addStroke(stroke).build(), null, null)
    }

    fun goBack(): Boolean = performGlobalAction(GLOBAL_ACTION_BACK)
    fun goHome(): Boolean = performGlobalAction(GLOBAL_ACTION_HOME)
    fun openRecents(): Boolean = performGlobalAction(GLOBAL_ACTION_RECENTS)
    fun openNotifications(): Boolean = performGlobalAction(GLOBAL_ACTION_NOTIFICATIONS)
    fun openQuickSettings(): Boolean = performGlobalAction(GLOBAL_ACTION_QUICK_SETTINGS)

    /**
     * Click a specific accessibility node by its view ID resource name.
     * This is more precise than text-based clicking — the model proposes a node_id
     * that was identified in a previous read_screen or ocr_screen step.
     */
    fun clickNodeById(nodeId: String): Boolean {
        val rootNode = rootInActiveWindow ?: return false
        try {
            val nodes = rootNode.findAccessibilityNodeInfosByViewId(nodeId)
            try {
                for (node in nodes) {
                    if (node.isClickable) {
                        return node.performAction(AccessibilityNodeInfo.ACTION_CLICK)
                    }
                    // Walk up to find a clickable ancestor
                    var parent = node.parent
                    while (parent != null) {
                        if (parent.isClickable) {
                            val clicked = parent.performAction(AccessibilityNodeInfo.ACTION_CLICK)
                            parent.recycle()
                            return clicked
                        }
                        val grandParent = parent.parent
                        parent.recycle()
                        parent = grandParent
                    }
                }
                return false
            } finally {
                nodes.forEach { it.recycle() }
            }
        } finally {
            rootNode.recycle()
        }
    }

    /**
     * Type text into a specific accessibility node by its view ID resource name.
     * This is more precise than text-based filling — the model proposes a node_id
     * that was identified in a previous read_screen or ocr_screen step.
     */
    fun typeIntoNodeById(nodeId: String, text: String): Boolean {
        val rootNode = rootInActiveWindow ?: return false
        try {
            val nodes = rootNode.findAccessibilityNodeInfosByViewId(nodeId)
            try {
                for (node in nodes) {
                    if (node.isEditable) {
                        val args = Bundle()
                        args.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
                        // Focus the node first
                        node.performAction(AccessibilityNodeInfo.ACTION_FOCUS)
                        return node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
                    }
                    // Try parent if the node itself isn't editable
                    var parent = node.parent
                    while (parent != null) {
                        if (parent.isEditable) {
                            val args = Bundle()
                            args.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
                            parent.performAction(AccessibilityNodeInfo.ACTION_FOCUS)
                            val result = parent.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
                            parent.recycle()
                            return result
                        }
                        val grandParent = parent.parent
                        parent.recycle()
                        parent = grandParent
                    }
                }
                return false
            } finally {
                nodes.forEach { it.recycle() }
            }
        } finally {
            rootNode.recycle()
        }
    }

    /**
     * Long-press a specific accessibility node by its view ID resource name.
     */
    fun longPressNodeById(nodeId: String): Boolean {
        val rootNode = rootInActiveWindow ?: return false
        try {
            val nodes = rootNode.findAccessibilityNodeInfosByViewId(nodeId)
            try {
                for (node in nodes) {
                    val bounds = android.graphics.Rect()
                    node.getBoundsInScreen(bounds)
                    if (!bounds.isEmpty) {
                        val x = bounds.exactCenterX()
                        val y = bounds.exactCenterY()
                        // Delegate to the coordinate-based longPress
                        return longPress(x, y)
                    }
                }
                // Fall back to text-based long press if view ID lookup fails
                return false
            } finally {
                nodes.forEach { it.recycle() }
            }
        } finally {
            rootNode.recycle()
        }
    }

    /**
     * Find a node by text and click it. Used by the legacy find_and_click system_control action
     * and the rule-based parser for accessibility commands.
     */
    fun findAndClick(text: String): Boolean = clickNodeWithText(text)

    private fun performSwipe(sx: Float, sy: Float, ex: Float, ey: Float, duration: Long): Boolean {
        return swipe(sx, sy, ex, ey, duration)
    }

    /**
     * Walks the accessibility tree and returns a flat list of [StructuredNode]s
     * ordered by depth-first traversal. Only visible nodes are included. Skips
     * nodes that carry no useful signal (no text, no ID, not clickable, not editable)
     * to keep the output compact for the LLM context window.
     *
     * The caller is responsible for capping the list size and total character length.
     */
    fun captureStructuredTree(): List<StructuredNode> {
        val rootNode = rootInActiveWindow ?: return emptyList()
        val nodes = mutableListOf<StructuredNode>()
        try {
            fun traverse(node: AccessibilityNodeInfo, depth: Int) {
                if (!node.isVisibleToUser) return

                val nodeId = node.viewIdResourceName
                val text = node.text?.toString()?.ifBlank { null }
                    ?: node.contentDescription?.toString()?.ifBlank { null }
                    ?: node.hintText?.toString()?.ifBlank { null }
                val clickable = node.isClickable || node.isEditable
                val editable = node.isEditable

                // Skip nodes that carry no useful signal for the LLM:
                // no ID, no text, not clickable, not editable, not a checkable/important container.
                if (nodeId != null || text != null || clickable || editable || node.isCheckable) {
                    val boundsRect = android.graphics.Rect()
                    node.getBoundsInScreen(boundsRect)
                    val boundsStr = "${boundsRect.left},${boundsRect.top}-${boundsRect.right},${boundsRect.bottom}"
                    nodes.add(
                        StructuredNode(
                            nodeId = nodeId,
                            text = text ?: "",
                            type = inferNodeType(node),
                            clickable = node.isClickable || editable,
                            bounds = boundsStr,
                            depth = depth
                        )
                    )
                }

                for (i in 0 until node.childCount) {
                    val child = node.getChild(i)
                    if (child != null) {
                        traverse(child, depth + 1)
                        child.recycle()
                    }
                }
            }
            traverse(rootNode, 0)
        } finally {
            rootNode.recycle()
        }
        return nodes
    }

    /** Infers a short human-readable node type from the class name. */
    private fun inferNodeType(node: AccessibilityNodeInfo): String {
        val cls = node.className?.toString() ?: return "view"
        return when {
            cls.contains("Button", ignoreCase = true) -> "button"
            cls.contains("EditText", ignoreCase = true) -> "edit"
            cls.contains("TextView", ignoreCase = true) -> "text"
            cls.contains("ImageView", ignoreCase = true) -> "image"
            cls.contains("CheckBox", ignoreCase = true) -> "checkbox"
            cls.contains("Switch", ignoreCase = true) || cls.contains("Toggle", ignoreCase = true) -> "switch"
            cls.contains("RadioButton", ignoreCase = true) -> "radio"
            cls.contains("Spinner", ignoreCase = true) -> "spinner"
            cls.contains("RecyclerView", ignoreCase = true) -> "recycler"
            cls.contains("ListView", ignoreCase = true) -> "list"
            cls.contains("ScrollView", ignoreCase = true) -> "scroll"
            cls.contains("ViewPager", ignoreCase = true) -> "pager"
            cls.contains("Toolbar", ignoreCase = true) || cls.contains("ActionBar", ignoreCase = true) -> "toolbar"
            cls.contains("ImageView", ignoreCase = true) -> "image"
            cls.contains("ImageButton", ignoreCase = true) -> "button"
            cls.contains("WebView", ignoreCase = true) -> "web"
            cls.contains("SeekBar", ignoreCase = true) || cls.contains("ProgressBar", ignoreCase = true) -> "progress"
            cls.contains("CardView", ignoreCase = true) -> "card"
            cls.contains("FrameLayout", ignoreCase = true)
                || cls.contains("LinearLayout", ignoreCase = true)
                || cls.contains("ConstraintLayout", ignoreCase = true)
                || cls.contains("RelativeLayout", ignoreCase = true) -> "container"
            else -> "view"
        }
    }

    companion object {
        @Volatile
        private var instance: UnoOneAccessibilityService? = null
        fun getInstance(): UnoOneAccessibilityService? =
            instance?.takeIf { AgentRuntimeGate.isEnabled() }
        fun isEnabled(): Boolean = instance != null && AgentRuntimeGate.isEnabled()
    }
}
