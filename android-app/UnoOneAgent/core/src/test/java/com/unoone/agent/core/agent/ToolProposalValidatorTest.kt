package com.unoone.agent.core.agent

import com.unoone.agent.core.model.CanonicalToolRegistry
import com.unoone.agent.core.model.ModelProfiles
import com.unoone.agent.core.model.ToolCall
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM unit tests for [ToolProposalValidator].
 *
 * Verifies tool proposal validation against the candidate set:
 * unknown tools rejected, known-but-uncandidate tools rejected,
 * candidate tools accepted, required args checked, speak_response always allowed.
 */
class ToolProposalValidatorTest {

    private fun call(tool: String, vararg args: Pair<String, String>): ToolCall =
        if (args.isEmpty()) ToolCall(tool, JsonObject(emptyMap()))
        else ToolCall(tool, JsonObject(args.associate { (k, v) -> k to JsonPrimitive(v) }))

    private val messagingCandidates = CandidateToolSelector.select(
        CandidateToolSelector.TaskIntent.MESSAGING,
        ModelProfiles.LITE
    )

    private val calendarCandidates = CandidateToolSelector.select(
        CandidateToolSelector.TaskIntent.CALENDAR,
        ModelProfiles.MEDIUM
    )

    // ── Unknown tool rejection ──────────────────────────────────────────

    @Test
    fun unknownTool_isRejected() {
        val result = ToolProposalValidator.validate(
            call("make_payment", "amount" to "100"),
            messagingCandidates
        )
        assertTrue("Unknown tool should be rejected", result is ToolProposalValidator.ValidationResult.Rejected)
        assertTrue("Reason should mention unknown",
            (result as ToolProposalValidator.ValidationResult.Rejected).reason.contains("Unknown tool"))
    }

    @Test
    fun nonsensicalTool_isRejected() {
        val result = ToolProposalValidator.validate(
            call("hack_phone"),
            messagingCandidates
        )
        assertTrue(result is ToolProposalValidator.ValidationResult.Rejected)
    }

    // ── Known but not in candidate set ────────────────────────────────────

    @Test
    fun toolNotInCandidateSet_isRejected() {
        // "create_note" is a known tool but not in the messaging candidate set
        val result = ToolProposalValidator.validate(
            call("create_note", "text" to "hello"),
            messagingCandidates
        )
        assertTrue("Tool outside candidate set should be rejected",
            result is ToolProposalValidator.ValidationResult.Rejected)
        val rejected = result as ToolProposalValidator.ValidationResult.Rejected
        assertTrue(rejected.reason.contains("not in candidate set"))
    }

    // ── Candidate tools accepted ────────────────────────────────────────

    @Test
    fun candidateTool_isAccepted() {
        // resolve_contact should be in the messaging candidate set
        val result = ToolProposalValidator.validate(
            call("resolve_contact", "query" to "Rahul"),
            messagingCandidates
        )
        assertTrue("resolve_contact should be accepted in messaging context",
            result is ToolProposalValidator.ValidationResult.Valid)
    }

    @Test
    fun speakResponse_alwaysAllowed_evenOutsideCandidateSet() {
        val result = ToolProposalValidator.validate(
            call("speak_response", "text" to "I can't do that"),
            messagingCandidates
        )
        assertTrue("speak_response should always be accepted",
            result is ToolProposalValidator.ValidationResult.Valid)
    }

    // ── Required arguments ──────────────────────────────────────────────

    @Test
    fun missingRequiredArgument_isRejected() {
        // "query" is required for resolve_contact
        val result = ToolProposalValidator.validate(
            call("resolve_contact"), // no query arg
            messagingCandidates
        )
        assertTrue("Missing required argument should be rejected",
            result is ToolProposalValidator.ValidationResult.Rejected)
        val rejected = result as ToolProposalValidator.ValidationResult.Rejected
        assertTrue(rejected.reason.lowercase().contains("missing required argument") ||
            rejected.reason.lowercase().contains("required argument"))
    }

    // ── Calendar candidates ─────────────────────────────────────────────

    @Test
    fun calendarTool_inCalendarCandidates_isAccepted() {
        val result = ToolProposalValidator.validate(
            call("check_calendar_conflict", "date" to "2024-01-15", "time" to "15:00"),
            calendarCandidates
        )
        assertTrue("check_calendar_conflict should be accepted in calendar context",
            result is ToolProposalValidator.ValidationResult.Valid)
    }

    // ── Batch validation ────────────────────────────────────────────────

    @Test
    fun batchValidation_separatesValidFromRejected() {
        val calls = listOf(
            call("resolve_contact", "query" to "Rahul"),
            call("make_payment", "amount" to "100"),
            call("speak_response", "text" to "done")
        )
        val (valid, rejected) = ToolProposalValidator.validateBatch(calls, messagingCandidates)
        assertTrue("Should have 2 valid calls", valid.size == 2)
        assertTrue("Should have 1 rejected call", rejected.size == 1)
        assertEquals("make_payment", rejected[0].first.tool)
    }

    // ── isCandidateOrFallback ───────────────────────────────────────────

    @Test
    fun isCandidateOrFallback_trueForCandidate() {
        assertTrue("resolve_contact should be in messaging candidates",
            ToolProposalValidator.isCandidateOrFallback("resolve_contact", messagingCandidates))
    }

    @Test
    fun isCandidateOrFallback_trueForSpeakResponse() {
        assertTrue("speak_response should always be allowed",
            ToolProposalValidator.isCandidateOrFallback("speak_response", emptyList()))
    }

    @Test
    fun isCandidateOrFallback_falseForNonCandidate() {
        assertFalse("create_note should not be in messaging candidates",
            ToolProposalValidator.isCandidateOrFallback("create_note", messagingCandidates))
    }

    // ── All canonical tools known ────────────────────────────────────────

    @Test
    fun allCandidateTools_areKnownToCanonicalRegistry() {
        for (intent in CandidateToolSelector.TaskIntent.entries) {
            if (intent == CandidateToolSelector.TaskIntent.CHAT) continue
            val tools = CandidateToolSelector.select(intent, ModelProfiles.MEDIUM)
            for (tool in tools) {
                assertTrue("Tool '${tool.name}' should be known to CanonicalToolRegistry",
                    CanonicalToolRegistry.isKnown(tool.name))
            }
        }
    }
}