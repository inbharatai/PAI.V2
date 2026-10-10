package com.unoone.agent.peersync

import com.unoone.agent.personal.*
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Test
import java.io.File

class SharedLedgerTest {
    private fun projection(v: PersonalView) = buildJsonObject {
        put("agent", PeerProtocol.json.encodeToJsonElement(v.agent)); put("persona", PeerProtocol.json.encodeToJsonElement(v.persona))
        put("tasks", JsonArray(v.tasks.map { t -> buildJsonObject {
            put("task_id", t.spec.task_id); put("goal", t.spec.goal); put("draft", t.draft); put("deadline", t.spec.deadline_ms)
            put("snooze", t.snoozeUntilMs?.let(::JsonPrimitive) ?: JsonNull); put("status", t.status)
            put("events", PeerProtocol.json.encodeToJsonElement(t.events)); put("owner", t.ownerReplicaId); put("epoch", t.ownerEpoch); put("claims", JsonArray(t.remoteClaims))
        } }))
        put("conflicts", JsonArray(v.conflicts.map(::JsonPrimitive)))
        put("conflict_kinds", JsonArray(v.conflicts.map { JsonPrimitive(it.substringBefore(':')) }))
    }
    private fun read(dir: File, name: String): PersonalLedger {
        val f = File(dir, name); val vault = Json.parseToJsonElement(f.readText()).jsonObject.getValue("local_vault_id").jsonPrimitive.content
        return PersonalLedger.decode(f.readBytes(), vault)
    }
    private fun edit(l: PersonalLedger, action: PersonalAction, task: String?, text: String, until: Long? = null) = l.apply(PersonalRequest(PersonalLedger.id(), l.view().revision, action, task, text, if (action == PersonalAction.DELETE) "" else "draft $text", until, l.replica_id), 10000)
    @Test fun sharedRustKotlinProjectionAndNativeLocalEditsRoundTrip() {
        val dir = File(requireNotNull(System.getProperty("peer.interop")))
        val a = read(dir, "shared-a.json"); var b = read(dir, "shared-b.json")
        val expected = Json.parseToJsonElement(File(dir, "shared-projection.json").readText())
        assertEquals(expected, projection(a.view())); assertEquals(expected, projection(b.view()))
        assertNotEquals(a.replica_id, b.replica_id); assertNotEquals(a.local_vault_id, b.local_vault_id)
        assertEquals(a.view().agent.person_id, b.view().agent.person_id)
        assertTrue(b.view().conflicts.any { "Rust A branch" in it && "Rust B branch" in it })
        val task = b.view().tasks.single().spec.task_id; val deadline = b.view().tasks.single().spec.deadline_ms
        b = edit(b, PersonalAction.PERSONA, null, "Kotlin reviewed persona")
        b = edit(b, PersonalAction.EDIT, task, "Kotlin reviewed task")
        assertTrue(b.view().conflicts.isEmpty())
        b = edit(b, PersonalAction.SNOOZE, task, "", 90000)
        assertEquals(deadline, b.view().tasks.single().spec.deadline_ms); assertEquals(90000L, b.view().tasks.single().snoozeUntilMs)
        File(dir, "shared-kotlin.json").writeBytes(b.bytes())
        File(dir, "shared-kotlin-projection.json").writeText(projection(b.view()).toString())
        val local = PeerProtocol.parseOffer(File(dir, "shared-offer-b.json").readText()); val peer = PeerProtocol.parseOffer(File(dir, "shared-offer-a.json").readText())
        val state = PeerState(1, b.local_vault_id, local, "JVM_NO_KEY").approve(peer, PeerSelection(true, listOf("00000000-0000-0000-0000-000000000000")), IdentityChoice.UNIFY_ARCHIVE, true, b)
        val page = state.page(b, 0); assertEquals(2, page.version); assertTrue(page.changes.all { it.payload != null })
        val revoked = state.copy(peer = state.peer!!.copy(revoked = true)); assertThrows(IllegalArgumentException::class.java) { revoked.page(b, 0) }
        // Replay is identical; changed payload at an existing dot holds the complete history.
        val ops = b.shared!!.operations[b.replica_id]!!
        val union = a.importShared(b.replica_id, ops)
        assertEquals(projection(b.view()), projection(union.view()))
        assertEquals(projection(union.view()), projection(union.importShared(b.replica_id, ops).view()))
        val collision = ops[0].copy(body = ops[0].body!!.copy(draft = "collision"))
        assertThrows(IllegalArgumentException::class.java) { union.importShared(b.replica_id, listOf(collision)) }
    }
    @Test fun v1RemainsV1UntilAuthenticatedMutualChoiceAndTombstoneDominates() {
        val a = PersonalLedger.fresh(PersonalLedger.id()); val b = PersonalLedger.fresh(PersonalLedger.id())
        val oa = PeerOffer(1, a.replica_id, a.view().agent.person_id, a.view().agent.agent_id, "a".repeat(64)); val ob = PeerOffer(1, b.replica_id, b.view().agent.person_id, b.view().agent.agent_id, "b".repeat(64))
        val select = PeerSelection(true, listOf("00000000-0000-0000-0000-000000000000"))
        val sa = PeerState(1, a.local_vault_id, oa, "JVM_NO_KEY").approve(ob, select, IdentityChoice.UNIFY_ARCHIVE, true, a)
        val sb = PeerState(1, b.local_vault_id, ob, "JVM_NO_KEY").approve(oa, select, IdentityChoice.UNIFY_ARCHIVE, true, b)
        assertEquals(1L, a.version); assertEquals(1L, b.version)
        var bb = sb.receive(sa.page(a, 0)).mergeInto(b); var aa = sa.receive(sb.page(bb, 0)).mergeInto(a)
        val tid = PersonalLedger.id(); aa = edit(aa, PersonalAction.CREATE, tid, "new task")
        bb = bb.importShared(aa.replica_id, aa.shared!!.operations[aa.replica_id]!!)
        val stale = edit(bb, PersonalAction.EDIT, tid, "stale resurrection")
        aa = edit(aa, PersonalAction.DELETE, tid, "")
        aa = aa.importShared(stale.replica_id, stale.shared!!.operations[stale.replica_id]!!)
        bb = stale.importShared(aa.replica_id, aa.shared!!.operations[aa.replica_id]!!)
        assertTrue(aa.view().tasks.isEmpty()); assertEquals(projection(aa.view()), projection(bb.view()))
        assertEquals(a.mutations, aa.mutations); assertEquals(b.mutations, bb.mutations)
    }
}
