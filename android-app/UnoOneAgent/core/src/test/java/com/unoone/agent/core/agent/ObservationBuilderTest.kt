package com.unoone.agent.core.agent

import com.unoone.agent.core.model.ActionResult
import com.unoone.agent.core.model.Result
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for [ObservationBuilder].
 *
 * Verifies detailed and concise observation formatting, truncation,
 * legacy Result<String> compatibility, and recovery hints.
 */
class ObservationBuilderTest {

    // ── Detailed observations ───────────────────────────────────────────

    @Test
    fun buildDetailed_verifiedSuccess() {
        val result = ActionResult.success(
            "open_app",
            "WhatsApp opened",
            mapOf("foregroundPackage" to "com.whatsapp")
        )
        val obs = ObservationBuilder.buildDetailed(result)
        assertTrue(obs.contains("Tool: open_app"))
        assertTrue(obs.contains("Status: SUCCESS"))
        assertTrue(obs.contains("Verified: true"))
        assertTrue(obs.contains("foregroundPackage: com.whatsapp"))
        assertTrue(obs.contains("Message: WhatsApp opened"))
    }

    @Test
    fun buildDetailed_failedWithRecoverable() {
        val result = ActionResult.failed(
            "resolve_contact",
            "Contact not found",
            "CONTACT_NOT_FOUND"
        )
        val obs = ObservationBuilder.buildDetailed(result)
        assertTrue(obs.contains("Status: FAILED"))
        assertTrue(obs.contains("Recoverable: CONTACT_NOT_FOUND"))
    }

    @Test
    fun buildDetailed_partialWithoutEvidence() {
        val result = ActionResult.partial("draft_whatsapp_message", "Draft created")
        val obs = ObservationBuilder.buildDetailed(result)
        assertTrue(obs.contains("Status: PARTIAL"))
        assertTrue(obs.contains("Verified: false"))
    }

    // ── Concise observations ────────────────────────────────────────────

    @Test
    fun buildConcise_verifiedSuccess() {
        val result = ActionResult.success("go_home", "Went home")
        val concise = ObservationBuilder.buildConcise(result)
        assertEquals("✓ go_home: Went home", concise)
    }

    @Test
    fun buildConcise_failed() {
        val result = ActionResult.failed("resolve_contact", "Not found", "CONTACT_NOT_FOUND")
        val concise = ObservationBuilder.buildConcise(result)
        assertTrue(concise.startsWith("✗"))
        assertTrue(concise.contains("recoverable: CONTACT_NOT_FOUND"))
    }

    @Test
    fun buildConcise_unverified() {
        val result = ActionResult.unverified("open_app", "May have opened")
        val concise = ObservationBuilder.buildConcise(result)
        assertTrue(concise.startsWith("?"))
    }

    @Test
    fun buildConcise_partial() {
        val result = ActionResult.partial("draft_whatsapp_message", "Draft created")
        val concise = ObservationBuilder.buildConcise(result)
        assertTrue(concise.startsWith("⚠"))
    }

    // ── Truncation ──────────────────────────────────────────────────────

    @Test
    fun truncate_shortString_unchanged() {
        val short = "Tool: go_home\nStatus: SUCCESS"
        assertEquals(short, ObservationBuilder.truncate(short))
    }

    @Test
    fun truncate_longString_truncated() {
        val long = "x".repeat(2000)
        val truncated = ObservationBuilder.truncate(long)
        assertTrue(truncated.length <= ObservationBuilder.MAX_OBSERVATION_CHARS)
        assertTrue(truncated.contains("truncated"))
    }

    @Test
    fun maxObservationChars_is1500() {
        assertEquals(1_500, ObservationBuilder.MAX_OBSERVATION_CHARS)
    }

    // ── Legacy Result<String> compatibility ──────────────────────────────

    @Test
    fun fromLegacyResult_success() {
        val result = Result.Success("WhatsApp opened")
        val obs = ObservationBuilder.fromLegacyResult("open_app", result)
        assertTrue(obs.startsWith("✓"))
        assertTrue(obs.contains("open_app"))
        assertTrue(obs.contains("WhatsApp opened"))
    }

    @Test
    fun fromLegacyResult_error() {
        val result = Result.Error("App not installed")
        val obs = ObservationBuilder.fromLegacyResult("open_app", result)
        assertTrue(obs.startsWith("✗"))
        assertTrue(obs.contains("App not installed"))
    }

    // ── Recovery hints ──────────────────────────────────────────────────

    @Test
    fun buildRecoveryHint_contactNotFound() {
        val result = ActionResult.failed("resolve_contact", "Not found", "CONTACT_NOT_FOUND")
        val hint = ObservationBuilder.buildRecoveryHint(result)
        assertEquals("Try a different name or spell it differently.", hint)
    }

    @Test
    fun buildRecoveryHint_calendarConflict() {
        val result = ActionResult.failed("create_calendar_event", "Conflict", "CALENDAR_CONFLICT")
        val hint = ObservationBuilder.buildRecoveryHint(result)
        assertEquals("There's a conflict at that time. Try a different time.", hint)
    }

    @Test
    fun buildRecoveryHint_appNotInstalled() {
        val result = ActionResult.failed("open_app", "Not installed", "APP_NOT_INSTALLED")
        val hint = ObservationBuilder.buildRecoveryHint(result)
        assertTrue(hint?.contains("browser") == true)
    }

    @Test
    fun buildRecoveryHint_nullForNonRecoverable() {
        val result = ActionResult.success("go_home", "Went home")
        val hint = ObservationBuilder.buildRecoveryHint(result)
        assertEquals(null, hint)
    }

    @Test
    fun buildRecoveryHint_unknownRecoverableError() {
        val result = ActionResult.failed("open_app", "Error", "UNKNOWN_ERROR")
        val hint = ObservationBuilder.buildRecoveryHint(result)
        assertTrue(hint?.contains("try again") == true || hint?.contains("different approach") == true)
    }
}