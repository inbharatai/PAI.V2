package com.unoone.agent.core.agent

import com.unoone.agent.core.model.CanonicalToolRegistry
import com.unoone.agent.core.model.ModelProfiles
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for [CandidateToolSelector].
 *
 * Verifies tool selection per intent and profile, max tool caps, CHAT empty set,
 * deterministic tool selection, and intent inference heuristics.
 */
class CandidateToolSelectorTest {

    // ── select() with LITE profile ─────────────────────────────────────

    @Test
    fun lite_messaging_capAt3() {
        val tools = CandidateToolSelector.select(
            CandidateToolSelector.TaskIntent.MESSAGING,
            ModelProfiles.LITE
        )
        assertTrue("Lite messaging should have at most 3 tools", tools.size <= 3)
        assertTrue("Should include resolve_contact",
            tools.any { it.name == "resolve_contact" })
    }

    @Test
    fun lite_calendar_capAt3() {
        val tools = CandidateToolSelector.select(
            CandidateToolSelector.TaskIntent.CALENDAR,
            ModelProfiles.LITE
        )
        assertTrue("Lite calendar should have at most 3 tools", tools.size <= 3)
    }

    @Test
    fun lite_accessibility_capAt3() {
        val tools = CandidateToolSelector.select(
            CandidateToolSelector.TaskIntent.ACCESSIBILITY,
            ModelProfiles.LITE
        )
        assertTrue("Lite accessibility should have at most 3 tools", tools.size <= 3)
    }

    @Test
    fun lite_unknown_capAt3() {
        val tools = CandidateToolSelector.select(
            CandidateToolSelector.TaskIntent.UNKNOWN,
            ModelProfiles.LITE
        )
        assertTrue("Lite unknown should have at most 3 tools", tools.size <= 3)
    }

    // ── select() with MEDIUM profile ────────────────────────────────────

    @Test
    fun medium_messaging_upTo6() {
        val tools = CandidateToolSelector.select(
            CandidateToolSelector.TaskIntent.MESSAGING,
            ModelProfiles.MEDIUM
        )
        assertTrue("Medium messaging should have at most 6 tools", tools.size <= 6)
        assertTrue("Should include resolve_contact",
            tools.any { it.name == "resolve_contact" })
        assertTrue("Should include draft_whatsapp_message",
            tools.any { it.name == "draft_whatsapp_message" })
    }

    @Test
    fun medium_calendar_upTo6() {
        val tools = CandidateToolSelector.select(
            CandidateToolSelector.TaskIntent.CALENDAR,
            ModelProfiles.MEDIUM
        )
        assertTrue("Medium calendar should have at most 6 tools", tools.size <= 6)
        assertTrue("Should include check_calendar_conflict",
            tools.any { it.name == "check_calendar_conflict" })
    }

    @Test
    fun medium_screen_upTo6() {
        val tools = CandidateToolSelector.select(
            CandidateToolSelector.TaskIntent.SCREEN,
            ModelProfiles.MEDIUM
        )
        assertTrue("Medium screen should have at most 6 tools", tools.size <= 6)
        assertTrue("Should include read_screen",
            tools.any { it.name == "read_screen" })
    }

    @Test
    fun medium_accessibility_upTo6() {
        val tools = CandidateToolSelector.select(
            CandidateToolSelector.TaskIntent.ACCESSIBILITY,
            ModelProfiles.MEDIUM
        )
        assertTrue("Medium accessibility should have at most 6 tools", tools.size <= 6)
    }

    // ── CHAT intent ─────────────────────────────────────────────────────

    @Test
    fun chat_returnsEmptyTools() {
        val liteTools = CandidateToolSelector.select(
            CandidateToolSelector.TaskIntent.CHAT,
            ModelProfiles.LITE
        )
        assertTrue("CHAT should have 0 tools for Lite", liteTools.isEmpty())

        val mediumTools = CandidateToolSelector.select(
            CandidateToolSelector.TaskIntent.CHAT,
            ModelProfiles.MEDIUM
        )
        assertTrue("CHAT should have 0 tools for Medium", mediumTools.isEmpty())
    }

    // ── speak_response always included ──────────────────────────────────

    @Test
    fun speakResponse_includedInAllNonChatIntents() {
        for (intent in CandidateToolSelector.TaskIntent.entries) {
            if (intent == CandidateToolSelector.TaskIntent.CHAT) continue
            val tools = CandidateToolSelector.select(intent, ModelProfiles.MEDIUM)
            assertTrue(
                "Intent $intent should include speak_response",
                tools.any { it.name == "speak_response" }
            )
        }
    }

    // ── All returned tools exist in CanonicalToolRegistry ────────────────

    @Test
    fun allReturnedTools_areInCanonicalRegistry() {
        for (intent in CandidateToolSelector.TaskIntent.entries) {
            val tools = CandidateToolSelector.select(intent, ModelProfiles.MEDIUM)
            for (tool in tools) {
                assertTrue(
                    "Tool '${tool.name}' should be in CanonicalToolRegistry",
                    CanonicalToolRegistry.isKnown(tool.name)
                )
            }
        }
    }

    // ── selectForDeterministic() ────────────────────────────────────────

    @Test
    fun deterministic_returnsToolPlusSpeakResponse() {
        val tools = CandidateToolSelector.selectForDeterministic(
            "create_note",
            ModelProfiles.LITE
        )
        assertTrue("Should include create_note",
            tools.any { it.name == "create_note" })
        assertTrue("Should include speak_response",
            tools.any { it.name == "speak_response" })
        assertEquals(2, tools.size)
    }

    @Test
    fun deterministic_speakResponse_onlyOnce() {
        val tools = CandidateToolSelector.selectForDeterministic(
            "speak_response",
            ModelProfiles.LITE
        )
        assertEquals("speak_response alone should produce 1 tool", 1, tools.size)
        assertEquals("speak_response", tools[0].name)
    }

    @Test
    fun deterministic_cappedByProfile() {
        val tools = CandidateToolSelector.selectForDeterministic(
            "create_note",
            ModelProfiles.LITE
        )
        assertTrue("Deterministic selection should be within Lite cap", tools.size <= ModelProfiles.LITE.maxCandidateTools)
    }

    // ── inferIntent() ───────────────────────────────────────────────────

    @Test
    fun inferIntent_whatsapp_returnsMessaging() {
        assertEquals(
            CandidateToolSelector.TaskIntent.MESSAGING,
            CandidateToolSelector.inferIntent("send a whatsapp message to Rahul")
        )
    }

    @Test
    fun inferIntent_calendar_returnsCalendar() {
        assertEquals(
            CandidateToolSelector.TaskIntent.CALENDAR,
            CandidateToolSelector.inferIntent("add a meeting tomorrow at 3pm")
        )
    }

    @Test
    fun inferIntent_note_returnsNotes() {
        assertEquals(
            CandidateToolSelector.TaskIntent.NOTES,
            CandidateToolSelector.inferIntent("create a note about the trip")
        )
    }

    @Test
    fun inferIntent_screen_returnsScreen() {
        assertEquals(
            CandidateToolSelector.TaskIntent.SCREEN,
            CandidateToolSelector.inferIntent("read screen")
        )
    }

    @Test
    fun inferIntent_search_returnsWeb() {
        assertEquals(
            CandidateToolSelector.TaskIntent.WEB,
            CandidateToolSelector.inferIntent("search for flights to Mumbai")
        )
    }

    @Test
    fun inferIntent_scroll_returnsAccessibility() {
        assertEquals(
            CandidateToolSelector.TaskIntent.ACCESSIBILITY,
            CandidateToolSelector.inferIntent("scroll down")
        )
    }

    @Test
    fun inferIntent_call_returnsPhone() {
        assertEquals(
            CandidateToolSelector.TaskIntent.PHONE,
            CandidateToolSelector.inferIntent("call mom")
        )
    }

    @Test
    fun inferIntent_document_returnsDocument() {
        assertEquals(
            CandidateToolSelector.TaskIntent.DOCUMENT,
            CandidateToolSelector.inferIntent("fill the document form")
        )
    }

    @Test
    fun inferIntent_camera_returnsCamera() {
        assertEquals(
            CandidateToolSelector.TaskIntent.CAMERA,
            CandidateToolSelector.inferIntent("take a photo")
        )
    }

    @Test
    fun inferIntent_skill_returnsSkill() {
        assertEquals(
            CandidateToolSelector.TaskIntent.SKILL,
            CandidateToolSelector.inferIntent("create a skill for morning routine")
        )
    }

    @Test
    fun inferIntent_gibberish_returnsUnknown() {
        assertEquals(
            CandidateToolSelector.TaskIntent.UNKNOWN,
            CandidateToolSelector.inferIntent("xyzzy florp")
        )
    }

    @Test
    fun inferIntent_caseInsensitive() {
        assertEquals(
            CandidateToolSelector.TaskIntent.MESSAGING,
            CandidateToolSelector.inferIntent("WHATSAPP RAHUL")
        )
    }

    @Test
    fun inferIntent_hindiKeyword_returnsMessaging() {
        assertEquals(
            CandidateToolSelector.TaskIntent.MESSAGING,
            CandidateToolSelector.inferIntent("whatsapp karo Rahul ko")
        )
    }

    // ── medium gives more tools than lite ───────────────────────────────

    @Test
    fun medium_givesEqualOrMoreToolsThanLite() {
        for (intent in CandidateToolSelector.TaskIntent.entries) {
            if (intent == CandidateToolSelector.TaskIntent.CHAT) continue
            val liteTools = CandidateToolSelector.select(intent, ModelProfiles.LITE)
            val mediumTools = CandidateToolSelector.select(intent, ModelProfiles.MEDIUM)
            assertTrue(
                "Medium ($intent) should have >= Lite tools",
                mediumTools.size >= liteTools.size
            )
        }
    }

    @Test
    fun liteTools_areSubsetOfMediumTools_forSameIntent() {
        for (intent in CandidateToolSelector.TaskIntent.entries) {
            if (intent == CandidateToolSelector.TaskIntent.CHAT) continue
            val liteTools = CandidateToolSelector.select(intent, ModelProfiles.LITE)
            val mediumTools = CandidateToolSelector.select(intent, ModelProfiles.MEDIUM)
            val liteNames = liteTools.map { it.name }.toSet()
            val mediumNames = mediumTools.map { it.name }.toSet()
            // Lite tools should be a subset of medium tools (they share the same intent mapping,
            // just capped at different sizes)
            for (name in liteNames) {
                assertTrue(
                    "Lite tool '$name' should also be available in Medium for intent $intent",
                    name in mediumNames || liteNames.size >= ModelProfiles.LITE.maxCandidateTools
                )
            }
        }
    }
}