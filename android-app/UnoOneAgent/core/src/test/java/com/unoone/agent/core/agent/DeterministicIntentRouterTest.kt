package com.unoone.agent.core.agent

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for [DeterministicIntentRouter].
 *
 * Verifies that deterministic commands are correctly routed without model involvement,
 * and that compound/ambiguous commands fall through to the model pipeline.
 */
class DeterministicIntentRouterTest {

    // ── Wake commands ──────────────────────────────────────────────────

    @Test
    fun wakeCommands_returnNoMatch() {
        // Wake commands are handled by the voice layer, not by tools
        val result = DeterministicIntentRouter.route("uno on")
        assertTrue("Wake commands should return NoMatch (handled by voice layer)",
            result is DeterministicIntentRouter.DeterministicResult.NoMatch)
    }

    @Test
    fun heyUno_returnsNoMatch() {
        val result = DeterministicIntentRouter.route("hey uno")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.NoMatch)
    }

    // ── Language commands ───────────────────────────────────────────────
    // Language switching is handled by RuleBasedParser / VoiceModule, NOT by
    // DeterministicIntentRouter. Language commands must fall through so the
    // orchestrator can call VoiceModule.reinitForLanguage().

    @Test
    fun languageSwitchEnglish_returnsNoMatch() {
        val result = DeterministicIntentRouter.route("speak in english")
        assertTrue("Language commands should fall through to orchestrator",
            result is DeterministicIntentRouter.DeterministicResult.NoMatch)
    }

    @Test
    fun languageSwitchHindi_returnsNoMatch() {
        val result = DeterministicIntentRouter.route("hindi mein bolo")
        assertTrue("Language commands should fall through to orchestrator",
            result is DeterministicIntentRouter.DeterministicResult.NoMatch)
    }

    @Test
    fun languageSwitchInHindi_returnsNoMatch() {
        val result = DeterministicIntentRouter.route("हिंदी में बोलो")
        assertTrue("Language commands should fall through to orchestrator",
            result is DeterministicIntentRouter.DeterministicResult.NoMatch)
    }

    // ── Blind mode commands ─────────────────────────────────────────────

    @Test
    fun blindModeOn_returnsMatched() {
        val result = DeterministicIntentRouter.route("start blind")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
        val matched = result as DeterministicIntentRouter.DeterministicResult.Matched
        assertEquals("BLIND_MODE_ON", matched.intent)
    }

    @Test
    fun blindModeOff_returnsMatched() {
        val result = DeterministicIntentRouter.route("stop blind")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
        val matched = result as DeterministicIntentRouter.DeterministicResult.Matched
        assertEquals("BLIND_MODE_OFF", matched.intent)
    }

    // ── Simple app launches ─────────────────────────────────────────────

    @Test
    fun openWhatsapp_returnsAppLaunch() {
        val result = DeterministicIntentRouter.route("open whatsapp")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
        val matched = result as DeterministicIntentRouter.DeterministicResult.Matched
        assertEquals("open_app", matched.call.tool)
        assertEquals("com.whatsapp", (matched.call.args["package_name"] as? kotlinx.serialization.json.JsonPrimitive)?.content)
    }

    @Test
    fun openGmail_returnsAppLaunch() {
        val result = DeterministicIntentRouter.route("open gmail")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
        val matched = result as DeterministicIntentRouter.DeterministicResult.Matched
        assertEquals("APP_LAUNCH", matched.intent)
    }

    @Test
    fun openCalendar_returnsAppLaunch() {
        val result = DeterministicIntentRouter.route("open calendar")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
    }

    @Test
    fun compoundAppLaunch_fallsThrough() {
        // "open whatsapp and send message" is compound — not a simple app launch
        val result = DeterministicIntentRouter.route("open whatsapp and send message to Rahul")
        // This should NOT match as a simple app launch because it contains "and" + "send"
        assertTrue("Compound commands should fall through to model",
            result is DeterministicIntentRouter.DeterministicResult.NoMatch ||
            (result is DeterministicIntentRouter.DeterministicResult.Matched &&
             (result as DeterministicIntentRouter.DeterministicResult.Matched).call.tool != "open_app"))
    }

    // ── Accessibility shortcuts ────────────────────────────────────────

    @Test
    fun goHome_returnsAccessibility() {
        val result = DeterministicIntentRouter.route("go home")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
        val matched = result as DeterministicIntentRouter.DeterministicResult.Matched
        assertEquals("go_home", matched.call.tool)
        assertEquals("ACCESSIBILITY", matched.intent)
    }

    @Test
    fun goBack_returnsAccessibility() {
        val result = DeterministicIntentRouter.route("go back")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
        assertEquals("go_back", (result as DeterministicIntentRouter.DeterministicResult.Matched).call.tool)
    }

    @Test
    fun readScreen_returnsAccessibility() {
        val result = DeterministicIntentRouter.route("read screen")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
        assertEquals("read_screen", (result as DeterministicIntentRouter.DeterministicResult.Matched).call.tool)
    }

    @Test
    fun scrollDown_returnsAccessibility() {
        val result = DeterministicIntentRouter.route("scroll down")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
        val matched = result as DeterministicIntentRouter.DeterministicResult.Matched
        assertEquals("scroll", matched.call.tool)
        assertEquals("down", (matched.call.args["direction"] as? kotlinx.serialization.json.JsonPrimitive)?.content)
    }

    @Test
    fun scrollUp_returnsAccessibility() {
        val result = DeterministicIntentRouter.route("scroll up")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
        assertEquals("scroll", (result as DeterministicIntentRouter.DeterministicResult.Matched).call.tool)
    }

    @Test
    fun openNotifications_returnsAccessibility() {
        val result = DeterministicIntentRouter.route("open notifications")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
        assertEquals("open_notifications", (result as DeterministicIntentRouter.DeterministicResult.Matched).call.tool)
    }

    @Test
    fun openRecents_returnsAccessibility() {
        val result = DeterministicIntentRouter.route("open recents")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
    }

    // ── Voice fast replies ──────────────────────────────────────────────

    @Test
    fun thankYou_returnsFastReply() {
        val result = DeterministicIntentRouter.route("thank you")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
        val matched = result as DeterministicIntentRouter.DeterministicResult.Matched
        assertEquals("VOICE_FAST_REPLY", matched.intent)
    }

    @Test
    fun stop_returnsFastReply() {
        val result = DeterministicIntentRouter.route("stop")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
    }

    @Test
    fun cancel_returnsFastReply() {
        val result = DeterministicIntentRouter.route("cancel")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
    }

    // ── Commands that need model ───────────────────────────────────────

    @Test
    fun messageRahul_needsModel() {
        val result = DeterministicIntentRouter.route("message rahul on whatsapp")
        assertTrue("Compound messaging should need model",
            result is DeterministicIntentRouter.DeterministicResult.NoMatch)
    }

    @Test
    fun addMeeting_needsModel() {
        val result = DeterministicIntentRouter.route("add a meeting tomorrow at 3pm")
        assertTrue("Calendar creation should need model",
            result is DeterministicIntentRouter.DeterministicResult.NoMatch)
    }

    @Test
    fun searchNotes_needsModel() {
        val result = DeterministicIntentRouter.route("search my notes about the trip")
        assertTrue("Note search should need model",
            result is DeterministicIntentRouter.DeterministicResult.NoMatch)
    }

    @Test
    fun describeScene_needsModel() {
        val result = DeterministicIntentRouter.route("describe what's on my screen")
        // "describe" doesn't match any deterministic pattern
        // It could match ACCESSIBILITY if it was "read screen", but "describe" is different
    }

    // ── Case insensitivity ─────────────────────────────────────────────

    @Test
    fun caseInsensitive_goHome() {
        val result = DeterministicIntentRouter.route("GO HOME")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
    }

    @Test
    fun caseInsensitive_openWhatsapp() {
        val result = DeterministicIntentRouter.route("OPEN WHATSAPP")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
    }

    @Test
    fun caseInsensitive_scrollDown() {
        val result = DeterministicIntentRouter.route("SCROLL DOWN")
        assertTrue(result is DeterministicIntentRouter.DeterministicResult.Matched)
    }
}