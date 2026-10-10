package com.unoone.agent.core.model

import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Test

class IntegratedToolAliasTest {
    private fun call(name: String, vararg args: Pair<String, String>) =
        ToolCall(name, JsonObject(args.associate { it.first to JsonPrimitive(it.second) }))

    @Test fun all42CanonicalSchemasKeepDescriptionsAndStableLegacyNames() {
        assertEquals(42, CanonicalToolRegistry.tools.size)
        assertTrue(CanonicalToolRegistry.tools.all { it.description.isNotBlank() })
        assertTrue(CanonicalToolRegistry.isKnown("send_prepared_whatsapp"))
        assertTrue(CanonicalToolRegistry.isKnown("create_calendar_event"))
    }
    @Test fun navigationAliasesAreTypedAndCannotSmuggleTargetsOrCoordinates() {
        for (name in listOf("go_back", "go_home", "open_notifications", "open_recents")) {
            assertNull(ToolCallValidator.rejection(call(name)))
            assertNotNull(ToolCallValidator.rejection(call(name, "target" to "password")))
        }
        assertNull(ToolCallValidator.rejection(call("scroll", "direction" to "down")))
        assertNotNull(ToolCallValidator.rejection(call("scroll", "direction" to "left")))
        assertNotNull(ToolCallValidator.rejection(call("system_control", "action" to "click", "target" to "Send")))
    }
    @Test fun oldNodeIdsNeverBecomeNativeReviewOrSnapshotAuthority() {
        for (name in listOf("click_accessibility_node", "long_press_accessibility_node")) {
            assertTrue(ToolCallValidator.rejection(call(name, "node_id" to "android:id/button1"))!!.contains("Manual handover"))
        }
        assertTrue(ToolCallValidator.rejection(call("type_into_accessibility_node", "node_id" to "password", "text" to "secret"))!!.contains("Manual handover"))
    }
    @Test fun legacySkillAdapterRetainsExactNonemptyStepsAndCanonicalArraySchema() {
        val adapted = ToolCallValidator.adaptLegacySkill(call("create_skill", "name" to "Routine", "steps" to "go back|read screen"))
        assertNull(ToolCallValidator.rejection(adapted))
        assertEquals(listOf("go back", "read screen"), (adapted.args.getValue("steps") as JsonArray).map { it.jsonPrimitive.content })
        try { ToolCallValidator.adaptLegacySkill(call("create_skill", "name" to "Bad", "steps" to "go back||read screen")); fail() }
        catch (_: IllegalArgumentException) { }
    }
}
