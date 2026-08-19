package com.unoone.agent.core.agent

import com.unoone.agent.core.model.ActionResult
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for [ActionVerifier].
 *
 * Verifies foreground launch verification, deterministic action verification,
 * observation building, and tool classification.
 */
class ActionVerifierTest {

    // ── Foreground launch verification ──────────────────────────────────

    @Test
    fun verifyForegroundLaunch_match_verifiedSuccess() {
        val result = ActionVerifier.verifyForegroundLaunch(
            tool = "open_app",
            userMessage = "WhatsApp opened",
            expectedPackages = setOf("com.whatsapp", "com.whatsapp.w4b"),
            actualForeground = "com.whatsapp"
        )
        assertTrue(result.isVerifiedSuccess)
        assertEquals("com.whatsapp", result.evidence["foregroundPackage"])
    }

    @Test
    fun verifyForegroundLaunch_w4b_verifiedSuccess() {
        val result = ActionVerifier.verifyForegroundLaunch(
            tool = "open_app",
            userMessage = "WhatsApp Business opened",
            expectedPackages = setOf("com.whatsapp", "com.whatsapp.w4b"),
            actualForeground = "com.whatsapp.w4b"
        )
        assertTrue(result.isVerifiedSuccess)
    }

    @Test
    fun verifyForegroundLaunch_mismatch_unverified() {
        val result = ActionVerifier.verifyForegroundLaunch(
            tool = "open_app",
            userMessage = "WhatsApp may have opened",
            expectedPackages = setOf("com.whatsapp"),
            actualForeground = "com.google.android.apps.messaging"
        )
        assertFalse(result.verified)
        assertFalse(result.isVerifiedSuccess)
        assertTrue(result.evidence["foregroundPackage"]?.contains("messaging") == true)
    }

    @Test
    fun verifyForegroundLaunch_nullForeground_unverified() {
        val result = ActionVerifier.verifyForegroundLaunch(
            tool = "open_app",
            userMessage = "App may have opened",
            expectedPackages = setOf("com.whatsapp"),
            actualForeground = null
        )
        assertFalse(result.verified)
    }

    @Test
    fun verifyForegroundLaunch_partialMatch_verified() {
        // Foreground package starts with expected package name
        val result = ActionVerifier.verifyForegroundLaunch(
            tool = "open_app",
            userMessage = "Calendar opened",
            expectedPackages = setOf("com.google.android.calendar"),
            actualForeground = "com.google.android.calendar"
        )
        assertTrue(result.isVerifiedSuccess)
    }

    // ── Deterministic action verification ────────────────────────────────

    @Test
    fun verifyDeterministicAction_success() {
        val result = ActionVerifier.verifyDeterministicAction(
            tool = "go_home",
            userMessage = "Went home",
            actionSucceeded = true
        )
        assertTrue(result.isVerifiedSuccess)
        assertEquals("go_home", result.evidence["actionPerformed"])
    }

    @Test
    fun verifyDeterministicAction_failure() {
        val result = ActionVerifier.verifyDeterministicAction(
            tool = "go_back",
            userMessage = "Could not go back",
            actionSucceeded = false
        )
        assertFalse(result.isVerifiedSuccess)
        assertEquals(ActionResult.Status.FAILED, result.status)
    }

    @Test
    fun verifyDeterministicAction_scrollDown_success() {
        val result = ActionVerifier.verifyDeterministicAction(
            tool = "scroll",
            userMessage = "Scrolled down",
            actionSucceeded = true
        )
        assertTrue(result.isVerifiedSuccess)
    }

    // ── Observation building ────────────────────────────────────────────

    @Test
    fun buildObservation_verifiedSuccess() {
        val result = ActionResult.success(
            "open_app",
            "WhatsApp opened",
            mapOf("foregroundPackage" to "com.whatsapp")
        )
        val obs = ActionVerifier.buildObservation(result)
        assertTrue(obs.contains("Tool: open_app"))
        assertTrue(obs.contains("Status: SUCCESS"))
        assertTrue(obs.contains("Verified: true"))
        assertTrue(obs.contains("foregroundPackage: com.whatsapp"))
    }

    @Test
    fun buildObservation_failed() {
        val result = ActionResult.failed(
            "resolve_contact",
            "Contact not found",
            "CONTACT_NOT_FOUND"
        )
        val obs = ActionVerifier.buildObservation(result)
        assertTrue(obs.contains("Status: FAILED"))
        assertTrue(obs.contains("Recoverable: CONTACT_NOT_FOUND"))
    }

    @Test
    fun buildObservation_partial() {
        val result = ActionResult.partial(
            "draft_whatsapp_message",
            "Draft created but not verified"
        )
        val obs = ActionVerifier.buildObservation(result)
        assertTrue(obs.contains("Status: PARTIAL"))
        assertTrue(obs.contains("Verified: false"))
    }

    // ── Concise observation ─────────────────────────────────────────────

    @Test
    fun buildConciseObservation_verifiedSuccess() {
        val result = ActionResult.success("go_home", "Went home")
        val concise = ActionVerifier.buildConciseObservation(result)
        assertTrue(concise.startsWith("✓"))
        assertTrue(concise.contains("go_home"))
    }

    @Test
    fun buildConciseObservation_failed() {
        val result = ActionResult.failed("resolve_contact", "Not found", "CONTACT_NOT_FOUND")
        val concise = ActionVerifier.buildConciseObservation(result)
        assertTrue(concise.startsWith("✗"))
        assertTrue(concise.contains("recoverable"))
    }

    @Test
    fun buildConciseObservation_unverified() {
        val result = ActionResult.unverified("open_app", "May have opened")
        val concise = ActionVerifier.buildConciseObservation(result)
        assertTrue(concise.startsWith("?"))
    }

    @Test
    fun buildConciseObservation_partial() {
        val result = ActionResult.partial("draft_whatsapp_message", "Draft created")
        val concise = ActionVerifier.buildConciseObservation(result)
        assertTrue(concise.startsWith("⚠"))
    }

    // ── Tool classification ─────────────────────────────────────────────

    @Test
    fun foregroundVerificationTools_containsAppLaunch() {
        assertTrue(ActionVerifier.FOREGROUND_VERIFICATION_TOOLS.contains("open_app"))
        assertTrue(ActionVerifier.FOREGROUND_VERIFICATION_TOOLS.contains("open_chrome"))
        assertTrue(ActionVerifier.FOREGROUND_VERIFICATION_TOOLS.contains("draft_whatsapp_message"))
    }

    @Test
    fun deterministicTools_containsAccessibilityShortcuts() {
        assertTrue(ActionVerifier.DETERMINISTIC_TOOLS.contains("go_home"))
        assertTrue(ActionVerifier.DETERMINISTIC_TOOLS.contains("go_back"))
        assertTrue(ActionVerifier.DETERMINISTIC_TOOLS.contains("scroll"))
        assertTrue(ActionVerifier.DETERMINISTIC_TOOLS.contains("speak_response"))
    }

    @Test
    fun foregroundVerificationTools_doesNotContainAccessibility() {
        assertFalse(ActionVerifier.FOREGROUND_VERIFICATION_TOOLS.contains("go_home"))
        assertFalse(ActionVerifier.FOREGROUND_VERIFICATION_TOOLS.contains("scroll"))
    }

    @Test
    fun deterministicTools_doesNotContainAppLaunch() {
        assertFalse(ActionVerifier.DETERMINISTIC_TOOLS.contains("open_app"))
        assertFalse(ActionVerifier.DETERMINISTIC_TOOLS.contains("open_chrome"))
    }
}