package com.unoone.agent.peersync

import com.unoone.agent.personal.*
import kotlinx.serialization.encodeToString
import org.junit.Assert.*
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.File

class PeerProtocolTest {
    private fun ledger() = PersonalLedger.fresh(PersonalLedger.id())
    private fun state(l: PersonalLedger, fp: String) = PeerState(1, l.local_vault_id, PeerOffer(1, l.replica_id, l.view().agent.person_id, l.view().agent.agent_id, fp.repeat(64)), "JVM_PROTOCOL_ONLY_NO_KEY")
    private fun mutate(l: PersonalLedger, action: PersonalAction, task: String): PersonalLedger = l.apply(PersonalRequest(PersonalLedger.id(), l.mutations.size.toLong(), action, task,
        if (action == PersonalAction.DELETE) "" else "समीक्षा local task", if (action == PersonalAction.DELETE) "" else "synthetic private draft", null, l.replica_id), 10000)
    private fun pair(a: PersonalLedger, b: PersonalLedger): Pair<PeerState, PeerState> {
        val sa = state(a, "a"); val sb = state(b, "b")
        return sa.approve(sb.local, PeerSelection(true, a.mutations.mapNotNull { it.request?.task_id }.distinct()), IdentityChoice.KEEP_SEPARATE_REVIEW, true, a) to
            sb.approve(sa.local, PeerSelection(true, b.mutations.mapNotNull { it.request?.task_id }.distinct()), IdentityChoice.KEEP_SEPARATE_REVIEW, true, b)
    }
    private fun rejects(block: () -> Unit) { try { block(); fail("Must reject") } catch (_: IllegalArgumentException) { } catch (_: IllegalStateException) { } }
    @Test fun explicitConsentIdentitySelectionAndRevocation() {
        val a = ledger(); val b = ledger(); val sa = state(a, "a"); val sb = state(b, "b")
        rejects { sa.approve(sb.local, PeerSelection(false, emptyList()), IdentityChoice.KEEP_SEPARATE_REVIEW, false, a) }
        rejects { sa.approve(sb.local, PeerSelection(false, emptyList()), IdentityChoice.SAME_PERSON, true, a) }
        val approved = sa.approve(sb.local, PeerSelection(false, emptyList()), IdentityChoice.KEEP_SEPARATE_REVIEW, true, a)
        assertTrue(approved.page(a, 0).changes.all { it.payload == null })
        rejects { approved.copy(peer = approved.peer!!.copy(revoked = true)).page(a, 0) }
    }
    @Test fun replayGapCollisionHashAndInertTombstone() {
        val tid = PersonalLedger.id(); var a = mutate(ledger(), PersonalAction.CREATE, tid); val b = ledger(); val (sa, sb) = pair(a,b)
        val page = sa.page(a, 0); val received = sb.receive(page)
        assertEquals(received, received.receive(page)); assertFalse(received.remoteTasks().first().executeOnHydration)
        rejects { sb.receive(page.copy(after = 1, changes = page.changes.drop(1))) }
        rejects { sb.receive(page.copy(version = 2)) }
        rejects { sb.receive(page.copy(choice = IdentityChoice.SAME_PERSON)) }
        rejects { sb.receive(page.copy(changes = listOf(page.changes.first().copy(content_hash = "0".repeat(64))))) }
        rejects { received.receive(page.copy(changes = listOf(page.changes.first().copy(operation_id = PersonalLedger.id())))) }
        a = mutate(a, PersonalAction.DELETE, tid); val deleted = received.receive(sa.page(a, 2))
        assertTrue(deleted.remoteTasks().first().deleted); assertEquals("", deleted.remoteTasks().first().draft)
        assertTrue(deleted.receive(page).remoteTasks().first().deleted); assertEquals(1, b.mutations.size)
    }
    @Test fun pagedCursorDoesNotDiscardUnsentHistory() {
        val tid = PersonalLedger.id(); var a = mutate(ledger(), PersonalAction.CREATE, tid); repeat(17) { a = mutate(a, PersonalAction.EDIT, tid) }
        val b = ledger(); val (sa, initial) = pair(a,b); var received = initial
        while (received.received.size < a.mutations.size) { val page = sa.page(a, received.received.size.toLong()); assertTrue(page.changes.size <= 8); received = received.receive(page) }
        assertEquals(19, received.received.size); assertEquals(a.view().tasks.first().draft, received.remoteTasks().first().draft)
    }
    @Test fun nativeHttpFramingAndLocalAddressBounds() {
        listOf("8.8.8.8:443", "example.com:443", "0.0.0.0:1", "127.0.0.1:0", "192.168.1.1:65536").forEach { rejects { NativePeerHttps.address(it) } }
        assertEquals(43123, NativePeerHttps.address("192.168.1.2:43123").port)
        listOf("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nx", "HTTP/1.1 200 OK\r\nContent-Length: 99999999\r\n\r\n", "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n").forEach { rejects { NativePeerHttps.readResponse(ByteArrayInputStream(it.toByteArray())) } }
    }
    @Test fun duplicateEscapedKeysAndNestingRejectedBeforeTree() {
        rejects { PeerProtocol.preflight("{\"a\":1,\"\\u0061\":2}", 100) }
        rejects { PeerProtocol.preflight("[".repeat(17) + "]".repeat(17), 100) }
        PeerProtocol.preflight("{\"a\":\"escaped \\\" quote\"}", 100)
    }
    @Test fun authenticatedPeerStillCannotTransferGrantsOrProviderRecords() {
        val a = ledger(); val b = ledger(); val (sa,sb) = pair(a,b)
        listOf("capability_grant", "mail_account", "draft").forEach { name ->
            val record = kotlinx.serialization.json.Json.parseToJsonElement(File("packages/personal-agent-contracts/fixtures/$name.json").readText()) as kotlinx.serialization.json.JsonObject
            val payload = PeerProtocol.json.encodeToString(PeerPayload(listOf(record), null)); val page = sa.page(a,0)
            val forged = page.changes.first().copy(payload = payload, content_hash = PeerProtocol.hash(payload.toByteArray()))
            rejects { sb.receive(page.copy(changes = listOf(forged))) }
        }
        assertTrue(sb.received.isEmpty())
    }
    @Test fun rustKotlinWireRoundTrip() {
        val dir = File(requireNotNull(System.getProperty("peer.interop")))
        val local = PersonalLedger.decode(File(dir,"rust-local-b.json").readBytes(), File(dir,"rust-vault-b.txt").readText())
        val offer = PeerProtocol.parseOffer(File(dir,"rust-offer-b.json").readText())
        val source = PeerProtocol.parseOffer(File(dir,"rust-offer-a.json").readText())
        val state = PeerState(1, local.local_vault_id, offer, "JVM_INTEROP_ONLY_NO_KEY").approve(source, PeerSelection(true, emptyList()), IdentityChoice.KEEP_SEPARATE_REVIEW, true, local)
        val raw = File(dir,"rust-page.json").readText(); PeerProtocol.preflight(raw, PeerProtocol.MAX_BODY)
        val received = state.receive(PeerProtocol.json.decodeFromString<PeerPage>(raw))
        assertEquals("fixture Rust goal नमस्ते", received.remoteTasks().single().goal)
        assertFalse(received.remoteTasks().single().executeOnHydration)
        val task = PersonalLedger.id(); val changed = mutate(local, PersonalAction.CREATE, task)
        val selected = state.copy(peer = state.peer!!.copy(selection = PeerSelection(true, listOf(task))))
        File(dir,"kotlin-page.json").writeText(PeerProtocol.json.encodeToString(selected.page(changed,0)))
        File(dir,"kotlin-received.json").writeText(PeerProtocol.json.encodeToString(received.received))
    }
}
