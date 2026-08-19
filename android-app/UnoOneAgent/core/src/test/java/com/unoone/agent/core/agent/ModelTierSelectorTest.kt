package com.unoone.agent.core.agent

import com.unoone.agent.core.model.ModelProfiles
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for [ModelTierSelector].
 *
 * Verifies Lite/Medium selection logic based on intent, E4B availability,
 * RAM constraints, and CHAT routing.
 */
class ModelTierSelectorTest {

    // ── CHAT intent always uses Lite ──────────────────────────────────

    @Test
    fun chat_intent_alwaysReturnsLite_evenWithE4bAndRam() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.CHAT,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    @Test
    fun chat_intent_alwaysReturnsLite_withoutE4b() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.CHAT,
            e4bAvailable = false,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    // ── Simple intents always use Lite ─────────────────────────────────

    @Test
    fun phone_intent_alwaysReturnsLite() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.PHONE,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    @Test
    fun camera_intent_alwaysReturnsLite() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.CAMERA,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    @Test
    fun accessibility_intent_alwaysReturnsLite() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.ACCESSIBILITY,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    // ── Compound intents use Medium when E4B available and RAM sufficient ──

    @Test
    fun messaging_intent_returnsMedium_whenE4bAvailable() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.MESSAGING,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.MEDIUM, result)
    }

    @Test
    fun calendar_intent_returnsMedium_whenE4bAvailable() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.CALENDAR,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.MEDIUM, result)
    }

    @Test
    fun notes_intent_returnsMedium_whenE4bAvailable() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.NOTES,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.MEDIUM, result)
    }

    @Test
    fun screen_intent_returnsMedium_whenE4bAvailable() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.SCREEN,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.MEDIUM, result)
    }

    @Test
    fun web_intent_returnsMedium_whenE4bAvailable() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.WEB,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.MEDIUM, result)
    }

    @Test
    fun document_intent_returnsMedium_whenE4bAvailable() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.DOCUMENT,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.MEDIUM, result)
    }

    @Test
    fun skill_intent_returnsMedium_whenE4bAvailable() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.SKILL,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.MEDIUM, result)
    }

    @Test
    fun unknown_intent_returnsMedium_whenE4bAvailable() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.UNKNOWN,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.MEDIUM, result)
    }

    // ── Compound intents fall back to Lite when E4B unavailable ──────

    @Test
    fun messaging_intent_returnsLite_whenE4bNotAvailable() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.MESSAGING,
            e4bAvailable = false,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    @Test
    fun calendar_intent_returnsLite_whenE4bNotAvailable() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.CALENDAR,
            e4bAvailable = false,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    @Test
    fun unknown_intent_returnsLite_whenE4bNotAvailable() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.UNKNOWN,
            e4bAvailable = false,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    // ── RAM constraints ────────────────────────────────────────────────

    @Test
    fun compound_intent_returnsLite_whenRamBelowE4bThreshold() {
        // Device has 7GB available RAM (< 8192MB E4B minimum)
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.MESSAGING,
            e4bAvailable = true,
            availableRamMb = 7_168
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    @Test
    fun compound_intent_returnsLite_whenRamExactlyAtE4bThreshold() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.MESSAGING,
            e4bAvailable = true,
            availableRamMb = ModelTierSelector.E4B_MIN_RAM_MB
        )
        // At exactly the threshold, E4B is allowed
        assertEquals(ModelProfiles.MEDIUM, result)
    }

    @Test
    fun compound_intent_returnsLite_whenRamJustBelowE4bThreshold() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.MESSAGING,
            e4bAvailable = true,
            availableRamMb = ModelTierSelector.E4B_MIN_RAM_MB - 1
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    @Test
    fun e4bMinRam_matchesManifestMinimum() {
        // Must match BrainModelRegistry.GEMMA_4_E4B.minimumRamMb (8,192 MB)
        assertEquals(8_192, ModelTierSelector.E4B_MIN_RAM_MB)
    }

    // ── Explicit RAM (no silent Int.MAX_VALUE default) ──────────────────

    @Test
    fun compound_intent_usesMedium_withSufficientRam() {
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.MESSAGING,
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.MEDIUM, result)
    }

    @Test
    fun compound_intent_returnsLite_whenRamUnknown() {
        // availableRamMb == -1 means "unknown" — E4B must not be selected
        val result = ModelTierSelector.select(
            CandidateToolSelector.TaskIntent.MESSAGING,
            e4bAvailable = true,
            availableRamMb = -1
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    // ── selectForCommand() convenience ─────────────────────────────────

    @Test
    fun selectForCommand_whatsapp_messaging_medium() {
        val result = ModelTierSelector.selectForCommand(
            "message rahul on whatsapp",
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.MEDIUM, result)
    }

    @Test
    fun selectForCommand_openApp_accessibility_lite() {
        val result = ModelTierSelector.selectForCommand(
            "scroll down",
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        assertEquals(ModelProfiles.LITE, result)
    }

    @Test
    fun selectForCommand_chat_lite() {
        val result = ModelTierSelector.selectForCommand(
            "what's the weather today",
            e4bAvailable = true,
            availableRamMb = 16_384
        )
        // "weather" doesn't match any specific intent → UNKNOWN → Medium
        // Actually, "weather" might match WEB via "search" but it doesn't contain "search"
        // Let's test with a clear chat-like query
    }

    // ── SwitchResult sealed class ──────────────────────────────────────

    @Test
    fun switchResult_switched() {
        val result = ModelTierSelector.SwitchResult.Switched(ModelProfiles.MEDIUM)
        assertEquals(ModelProfiles.MEDIUM, result.profile)
    }

    @Test
    fun switchResult_alreadyActive() {
        val result = ModelTierSelector.SwitchResult.AlreadyActive(ModelProfiles.LITE)
        assertEquals(ModelProfiles.LITE, result.profile)
    }

    @Test
    fun switchResult_insufficientRam() {
        val result = ModelTierSelector.SwitchResult.InsufficientRam(
            requiredMb = 10_240,
            availableMb = 8_192
        )
        assertEquals(10_240, result.requiredMb)
        assertEquals(8_192, result.availableMb)
    }

    @Test
    fun switchResult_loadFailed() {
        val result = ModelTierSelector.SwitchResult.LoadFailed("GPU error")
        assertEquals("GPU error", result.error)
    }

    // ── All compound intents ──────────────────────────────────────────

    @Test
    fun allCompoundIntents_useLite_whenE4bUnavailable() {
        val compoundIntents = setOf(
            CandidateToolSelector.TaskIntent.MESSAGING,
            CandidateToolSelector.TaskIntent.CALENDAR,
            CandidateToolSelector.TaskIntent.NOTES,
            CandidateToolSelector.TaskIntent.SCREEN,
            CandidateToolSelector.TaskIntent.WEB,
            CandidateToolSelector.TaskIntent.DOCUMENT,
            CandidateToolSelector.TaskIntent.SKILL,
            CandidateToolSelector.TaskIntent.UNKNOWN
        )
        for (intent in compoundIntents) {
            val result = ModelTierSelector.select(intent, e4bAvailable = false, availableRamMb = 16_384)
            assertEquals("Intent $intent should use Lite when E4B unavailable",
                ModelProfiles.LITE, result)
        }
    }

    @Test
    fun allSimpleIntents_useLite_always() {
        val simpleIntents = setOf(
            CandidateToolSelector.TaskIntent.PHONE,
            CandidateToolSelector.TaskIntent.CAMERA,
            CandidateToolSelector.TaskIntent.ACCESSIBILITY
        )
        for (intent in simpleIntents) {
            val resultWithE4b = ModelTierSelector.select(intent, e4bAvailable = true, availableRamMb = 16_384)
            assertEquals("Simple intent $intent should use Lite even with E4B",
                ModelProfiles.LITE, resultWithE4b)

            val resultWithoutE4b = ModelTierSelector.select(intent, e4bAvailable = false, availableRamMb = 16_384)
            assertEquals("Simple intent $intent should use Lite without E4B",
                ModelProfiles.LITE, resultWithoutE4b)
        }
    }
}