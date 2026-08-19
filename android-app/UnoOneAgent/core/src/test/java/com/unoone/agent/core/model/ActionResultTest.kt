package com.unoone.agent.core.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for [ActionResult].
 *
 * Verifies factory methods, status transitions, verified flag semantics,
 * and the isVerifiedSuccess convenience property.
 */
class ActionResultTest {

    // ── Factory: success() ──────────────────────────────────────────────

    @Test
    fun success_createsVerifiedSuccess() {
        val result = ActionResult.success("open_app", "Opened WhatsApp",
            mapOf("foregroundPackage" to "com.whatsapp"))
        assertEquals(ActionResult.Status.SUCCESS, result.status)
        assertTrue(result.verified)
        assertEquals("open_app", result.tool)
        assertEquals("Opened WhatsApp", result.userMessage)
        assertEquals("com.whatsapp", result.evidence["foregroundPackage"])
        assertTrue(result.isVerifiedSuccess)
        assertNull(result.recoverableError)
    }

    @Test
    fun success_withoutEvidence_isStillVerified() {
        val result = ActionResult.success("go_home", "Went home")
        assertEquals(ActionResult.Status.SUCCESS, result.status)
        assertTrue(result.verified)
        assertTrue(result.isVerifiedSuccess)
        assertTrue(result.evidence.isEmpty())
    }

    // ── Factory: unverified() ──────────────────────────────────────────

    @Test
    fun unverified_createsUnverifiedSuccess() {
        val result = ActionResult.unverified("send_prepared_whatsapp",
            "WhatsApp may have opened", mapOf("timestamp" to "1234567890"))
        assertEquals(ActionResult.Status.SUCCESS, result.status)
        assertFalse("Unverified result should not be verified", result.verified)
        assertFalse("Unverified success should not be verified success",
            result.isVerifiedSuccess)
        assertEquals("send_prepared_whatsapp", result.tool)
    }

    // ── Factory: failed() ──────────────────────────────────────────────

    @Test
    fun failed_createsVerifiedFailure() {
        val result = ActionResult.failed("resolve_contact",
            "Contact not found", "CONTACT_NOT_FOUND")
        assertEquals(ActionResult.Status.FAILED, result.status)
        assertTrue("Failed result should be verified (we verified it failed)", result.verified)
        assertFalse("Failed result should not be verified success", result.isVerifiedSuccess)
        assertEquals("CONTACT_NOT_FOUND", result.recoverableError)
    }

    @Test
    fun failed_withoutRecoverableError() {
        val result = ActionResult.failed("open_app", "App not installed")
        assertNull(result.recoverableError)
    }

    // ── Factory: partial() ─────────────────────────────────────────────

    @Test
    fun partial_createsPartialResult() {
        val result = ActionResult.partial("draft_whatsapp_message",
            "Draft created but not sent", mapOf("draftId" to "abc123"))
        assertEquals(ActionResult.Status.PARTIAL, result.status)
        assertFalse(result.verified)
        assertFalse(result.isVerifiedSuccess)
        assertEquals("abc123", result.evidence["draftId"])
    }

    // ── isVerifiedSuccess semantics ────────────────────────────────────

    @Test
    fun isVerifiedSuccess_true_onlyForVerifiedSuccess() {
        // SUCCESS + verified = true
        assertTrue(ActionResult.success("x", "ok").isVerifiedSuccess)

        // SUCCESS + unverified = false
        assertFalse(ActionResult.unverified("x", "maybe").isVerifiedSuccess)

        // FAILED + verified = false
        assertFalse(ActionResult.failed("x", "fail").isVerifiedSuccess)

        // PARTIAL + unverified = false
        assertFalse(ActionResult.partial("x", "half").isVerifiedSuccess)
    }

    // ── Evidence map ───────────────────────────────────────────────────

    @Test
    fun evidence_canHoldMultipleKeys() {
        val result = ActionResult.success("create_calendar_event",
            "Event created",
            mapOf(
                "eventId" to "evt_123",
                "foregroundPackage" to "com.google.android.calendar",
                "timestamp" to "1700000000"
            ))
        assertEquals(3, result.evidence.size)
        assertEquals("evt_123", result.evidence["eventId"])
    }

    @Test
    fun evidence_defaultsToEmpty() {
        val result = ActionResult.success("go_home", "Went home")
        assertTrue(result.evidence.isEmpty())
    }

    // ── Status enum completeness ────────────────────────────────────────

    @Test
    fun status_enum_hasAllValues() {
        val statuses = ActionResult.Status.values()
        assertEquals(3, statuses.size)
        assertTrue(statuses.contains(ActionResult.Status.SUCCESS))
        assertTrue(statuses.contains(ActionResult.Status.PARTIAL))
        assertTrue(statuses.contains(ActionResult.Status.FAILED))
    }

    // ── Realistic scenarios ─────────────────────────────────────────────

    @Test
    fun scenario_appLaunch_verified() {
        // Scenario: WhatsApp launch confirmed by foreground package
        val result = ActionResult.success(
            "open_app",
            "WhatsApp opened",
            mapOf(
                "foregroundPackage" to "com.whatsapp",
                "timestamp" to "1700000000"
            )
        )
        assertTrue(result.isVerifiedSuccess)
        assertEquals("com.whatsapp", result.evidence["foregroundPackage"])
    }

    @Test
    fun scenario_appLaunch_unverified() {
        // Scenario: WhatsApp launch attempted but foreground check failed
        val result = ActionResult.unverified(
            "open_app",
            "WhatsApp may have opened"
        )
        assertFalse(result.isVerifiedSuccess)
        assertFalse(result.verified)
        // Model should NOT announce "WhatsApp opened" — must say "may have opened"
    }

    @Test
    fun scenario_contactResolution_notFound() {
        // Scenario: Contact not found — recoverable (try different name)
        val result = ActionResult.failed(
            "resolve_contact",
            "Contact 'Rahul' not found",
            "CONTACT_NOT_FOUND"
        )
        assertFalse(result.isVerifiedSuccess)
        assertEquals("CONTACT_NOT_FOUND", result.recoverableError)
        // Model can retry with a different name
    }

    @Test
    fun scenario_calendarConflict() {
        // Scenario: Calendar conflict detected — recoverable (pick different time)
        val result = ActionResult.failed(
            "create_calendar_event",
            "Calendar conflict at 3pm",
            "CALENDAR_CONFLICT"
        )
        assertFalse(result.isVerifiedSuccess)
        assertEquals("CALENDAR_CONFLICT", result.recoverableError)
    }

    @Test
    fun scenario_accessibilityClick_verified() {
        // Scenario: Accessibility click confirmed by node state change
        val result = ActionResult.success(
            "click_accessibility_node",
            "Clicked send button",
            mapOf("nodeId" to "com.whatsapp:id/send", "actionPerformed" to "click")
        )
        assertTrue(result.isVerifiedSuccess)
    }
}