package com.unoone.agent.personal

import com.unoone.agent.core.personal.*
import org.junit.Assert.*
import org.junit.Test

class PersonalExecutionTest {
    private val now = 1791629205000L
    private fun request(l: PersonalLedger, action: PersonalAction, id: String?, text: String = "", draft: String = "") = PersonalRequest(PersonalLedger.id(), l.view().revision, action, id, text, draft, null, l.replica_id)
    private fun accepted(): Pair<PersonalLedger, String> {
        var l = PersonalLedger.fresh(PersonalLedger.id())
        l = l.apply(request(l, PersonalAction.PERSONA, null, "UnoOne", "Reply in Hindi briefly"), now)
        val tid = PersonalLedger.id()
        l = l.apply(request(l, PersonalAction.CREATE, tid, "Prepare my draft"), now)
        l = l.apply(request(l, PersonalAction.ACCEPT, tid), now)
        return l to tid
    }
    @Test fun actualRequestBuilderIncludesOnlyCurrentApprovedUserPreferences() {
        val (l, _) = accepted(); val v = l.view()
        assertTrue(PersonalExecutionPolicy.request(v, "hello").contains("Hindi"))
        assertTrue(PersonalExecutionPolicy.request(v, "hello").startsWith(PersonalExecutionPolicy.POLICY))
        assertFalse(PersonalExecutionPolicy.request(v.copy(conflicts = listOf("fork")), "hello").contains("Hindi"))
        assertFalse(PersonalExecutionPolicy.request(v.copy(persona = v.persona.copy(deleted = true)), "hello").contains("Hindi"))
        assertFalse(PersonalExecutionPolicy.request(v.copy(persona = v.persona.copy(revision = 99)), "hello").contains("Hindi"))
        val preference = v.persona.preferences.single()
        val model = preference.copy(provenance = preference.provenance.copy(source = ProvenanceSource.MODEL))
        assertFalse(PersonalExecutionPolicy.context(v.copy(persona = v.persona.copy(preferences = listOf(model)))).contains("Hindi"))
        val long = preference.copy(value = "अ".repeat(2000))
        assertEquals("", PersonalExecutionPolicy.context(v.copy(persona = v.persona.copy(preferences = listOf(long)))))
    }
    @Test fun permitsAreLocalBoundedSingleAttemptAndOutputsAreNotVerified() {
        var (l, tid) = accepted(); val v = l.view()
        val permit = PersonalDraftPermit.approve(v, tid, v.revision, now, 7)
        assertEquals(0L, permit.grant.snapshot().budget.max_children)
        assertEquals(0L, permit.grant.snapshot().budget.max_tool_calls)
        assertTrue(permit.grant.snapshot().scopes.data.isEmpty())
        l = l.recordDraftAttempt(v.revision, PersonalDraftPhase.STARTED, "", permit, now, 7)
        assertEquals("IN_PROGRESS", l.view().tasks.single().status)
        assertThrows(IllegalArgumentException::class.java) { l.recordDraftAttempt(l.view().revision, PersonalDraftPhase.RESPONDED, "revoked", permit, now + 1, 8) }
        l = l.recordDraftAttempt(l.view().revision, PersonalDraftPhase.RESPONDED, "TEST MODEL output, not verified facts", permit, now + 1, 7)
        assertEquals("AWAITING_VERIFICATION", l.view().tasks.single().status)
        assertTrue(l.view().tasks.single().draft.contains("RESPONDED"))
        assertThrows(IllegalArgumentException::class.java) { l.recordDraftAttempt(l.view().revision, PersonalDraftPhase.STARTED, "", permit, now + 2, 7) }
        assertThrows(IllegalArgumentException::class.java) { PersonalDraftPermit.approve(v.copy(conflicts = listOf("fork")), tid, v.revision, now, 7) }
        assertThrows(IllegalArgumentException::class.java) { PersonalDraftPermit.approve(v, tid, v.revision - 1, now, 7) }
    }
}

class PersonalGuardianLedgerTest {
    private val now = 1791629205000L
    private fun request(l: PersonalLedger, action: PersonalAction, id: String?, text: String = "") = PersonalRequest(PersonalLedger.id(), l.view().revision, action, id, text, "", null, l.replica_id)
    /** Mirror of the Rust runtime test: guardian notes use the existing EDIT outbox; unprefixed/oversized notes fail. */
    @Test fun guardianNoteUsesExistingEditOutboxAndRejectsUnprefixedOrOversized() {
        var l = PersonalLedger.fresh(PersonalLedger.id()); val tid = PersonalLedger.id()
        l = l.apply(request(l, PersonalAction.CREATE, tid, "Prepare a private draft"), now)
        l = l.apply(request(l, PersonalAction.ACCEPT, tid), now)
        val before = l.view().revision
        assertThrows(IllegalArgumentException::class.java) { l.recordGuardianNote(tid, "free text pretending to be a receipt", now) }
        assertThrows(IllegalArgumentException::class.java) { l.recordGuardianNote(tid, "GUARDIAN RECEIPT v1 " + "x".repeat(4000), now) }
        l = l.recordGuardianNote(tid, "GUARDIAN RECEIPT v1 · SEND_MESSAGE · Warn · NOT PERFORMED\n{}", now)
        val task = l.view().tasks.single()
        assertTrue(task.draft.startsWith("GUARDIAN RECEIPT v1")); assertEquals("Prepare a private draft", task.spec.goal)
        assertEquals("BLOCKED", task.status); assertEquals(before + 1, l.view().revision)
        l = l.recordGuardianNote(tid, "GUARDIAN CORRECTION v1 · FalseAlarm · fp\n{}", now)
        assertTrue(l.view().tasks.single().draft.startsWith("GUARDIAN CORRECTION v1"))
        // The real guardian receipt round-trips through the ledger note and back into the UI card parser, masked.
        val d = com.unoone.agent.core.guardian.PrivacyGuardian.check(com.unoone.agent.core.guardian.Intent.SendMessage(listOf("x@example.test"), "code", "OTP 445566", false, emptyList()), com.unoone.agent.core.guardian.Context())
        val receipt = try { com.unoone.agent.core.guardian.PrivacyGuardian.enforce(d, null, now); error("block expected") } catch (e: com.unoone.agent.core.guardian.GuardianRefusal) { e.receipt }
        l = l.recordGuardianNote(tid, receipt.ledgerNote(), now)
        val parsed = checkNotNull(parseGuardianReceipt(l.view().tasks.single().draft))
        assertEquals(com.unoone.agent.core.guardian.Severity.BLOCK, parsed.severity); assertFalse(l.view().tasks.single().draft.contains("445566"))
    }
}
