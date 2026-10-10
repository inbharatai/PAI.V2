package com.unoone.agent.personal

import com.unoone.agent.core.personal.*
import com.unoone.agent.vault.*
import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files
import java.io.File

class PersonalLedgerTest {
    private val now = 1791629205000L
    private fun req(l: PersonalLedger, action: PersonalAction, tid: String? = null, text: String = "") = PersonalRequest(PersonalLedger.id(), l.view().revision, action, tid, text, "", null, l.replica_id)
    private fun store(repo: MobileVaultRepository, session: VaultSession, io: VaultIO) = PersonalStore(session.vaultId,
        object : VaultRecordReader {
            override fun listRecordMetadata() = repo.listRecordMetadata(session)
            override fun readRecord(recordId: String) = repo.readRecord(session, recordId)
        }, object : VaultRecordWriter {
            override fun writeRecord(fields: Map<String, Any?>, content: ByteArray) = repo.writeRecord(session, fields, content)
            override fun tombstone(vaultRecordId: String, deletedAtIso: String) = repo.tombstoneRecord(session, vaultRecordId, deletedAtIso)
        }, { io.exists("VAULT/records/${PersonalLedger.RECORD_ID}.enc.json") })
    @Test fun realVaultRestartPersonaTaskTombstoneAndAtomicFailure() {
        val dir = Files.createTempDirectory("personal-kotlin-").toFile()
        try {
            val io = PrivateFileVaultIO(File(dir, "vault")); val repo = MobileVaultRepository(io)
            var session = repo.create("personal-ledger-private-password".toByteArray())
            var s = store(repo, session, io); var l = s.load(); val identity = l.view()
            l = l.apply(req(l, PersonalAction.PERSONA, text = "Private name marker").copy(draft = "Private preference marker: नमस्ते"), now)
            val tid = PersonalLedger.id(); val create = req(l, PersonalAction.CREATE, tid, "Private goal marker").copy(draft = "Private draft marker")
            l = l.apply(create, now); assertEquals(l, l.apply(create, now)); l = l.apply(req(l, PersonalAction.ACCEPT, tid), now); s.save(l)
            val ciphertext = io.read("VAULT/records/${PersonalLedger.RECORD_ID}.enc.json").toString(Charsets.UTF_8)
            listOf("Private name marker", "Private goal marker", "Private draft marker", identity.replicaId, identity.agent.person_id).forEach { assertFalse(ciphertext.contains(it)) }
            session.close(); session = repo.unlock("personal-ledger-private-password".toByteArray()); s = store(repo, session, io); l = s.load()
            assertEquals(identity.agent.agent_id, l.view().agent.agent_id); assertEquals(identity.replicaId, l.replica_id)
            assertEquals("READY_FOR_REVIEW", l.view().tasks.single().status); assertEquals("Private draft marker", l.view().tasks.single().draft)
            val faultIO = PrivateFileVaultIO(File(dir, "vault"), beforeAtomicReplace = { error("Injected pre-rename failure") })
            val faultStore = store(MobileVaultRepository(faultIO), session, faultIO)
            val change = req(l, PersonalAction.EDIT, tid, "Not persisted")
            assertThrows(IllegalStateException::class.java) { faultStore.mutate(change, now) }
            assertEquals(l, s.load()) // history and pending outbox both unchanged
            l = l.apply(req(l, PersonalAction.CLEAR_PERSONA), now); l = l.apply(req(l, PersonalAction.DELETE, tid), now); s.save(l); session.close()
            session = repo.unlock("personal-ledger-private-password".toByteArray()); l = store(repo, session, io).load()
            assertTrue(l.view().tasks.isEmpty()); assertTrue(l.view().persona.deleted)
            assertThrows(IllegalArgumentException::class.java) { l.apply(req(l, PersonalAction.CREATE, tid, "resurrect"), now) }
            session.close()
        } finally { dir.deleteRecursively() }
    }
    @Test fun separateIdentitiesBindingRevisionAndOperationDedup() {
        var a = PersonalLedger.fresh(PersonalLedger.id()); val b = PersonalLedger.fresh(PersonalLedger.id())
        assertNotEquals(a.replica_id, b.replica_id); assertNotEquals(a.view().agent.person_id, b.view().agent.person_id)
        assertThrows(IllegalArgumentException::class.java) { PersonalLedger.decode(a.bytes(), b.local_vault_id) }
        val r = req(a, PersonalAction.CREATE, PersonalLedger.id(), "task"); val stale = req(a, PersonalAction.PERSONA, text = "stale")
        assertThrows(IllegalArgumentException::class.java) { a.apply(r.copy(expected_replica_id = b.replica_id), now) }
        a = a.apply(r, now); assertEquals(a, a.apply(r, now))
        assertThrows(IllegalArgumentException::class.java) { a.apply(stale, now) }
        assertThrows(IllegalArgumentException::class.java) { a.apply(r.copy(text = "collision"), now) }
    }
    @Test fun causalFoldDedupAndNoModelVerification() {
        var l = PersonalLedger.fresh(PersonalLedger.id()); val tid = PersonalLedger.id()
        l = l.apply(req(l, PersonalAction.CREATE, tid, "goal"), now)
        l = l.apply(req(l, PersonalAction.ACCEPT, tid), now)
        val events = l.view().tasks.single().events.toMutableList()
        val dedup = foldTask(tid, events + events.first(), l.replica_id)
        assertEquals(TaskTransition.READY_FOR_REVIEW, dedup.transition); assertFalse(dedup.executeOnHydration)
        listOf(TaskTransition.IN_PROGRESS, TaskTransition.AWAITING_VERIFICATION, TaskTransition.VERIFIED).forEach { t ->
            val previous = events.last()
            events += previous.copy(event_id = PersonalLedger.id(), operation_id = PersonalLedger.id(), predecessor_event_id = previous.event_id,
                step = previous.step + 1, transition = t, evidence_ref = if (t == TaskTransition.VERIFIED) PersonalLedger.id() else null)
        }
        assertEquals(TaskTransition.AWAITING_VERIFICATION, foldTask(tid, events, l.replica_id).transition)
        assertEquals(FoldState.MISSING_PREDECESSOR, foldTask(tid, events.filterIndexed { i, _ -> i != 1 }, l.replica_id).state)
        assertEquals(FoldState.CONFLICT, foldTask(tid, events + events[1].copy(event_id = PersonalLedger.id(), operation_id = PersonalLedger.id()), l.replica_id).state)
    }
    @Test fun editedDraftRevokesReviewSnoozeCancelAndNoResurrection() {
        var l = PersonalLedger.fresh(PersonalLedger.id()); val tid = PersonalLedger.id()
        l = l.apply(req(l, PersonalAction.CREATE, tid, "goal"), now); l = l.apply(req(l, PersonalAction.ACCEPT, tid), now)
        l = l.apply(req(l, PersonalAction.EDIT, tid, "edited").copy(draft = "edit"), now); assertEquals("BLOCKED", l.view().tasks.single().status)
        l = l.apply(req(l, PersonalAction.SNOOZE, tid).copy(snooze_until_ms = now + 60000), now)
        l = PersonalLedger.decode(l.bytes(), l.local_vault_id); assertEquals(now + 60000, l.view().tasks.single().snoozeUntilMs)
        l = l.apply(req(l, PersonalAction.CANCEL, tid), now)
        assertThrows(IllegalArgumentException::class.java) { l.apply(req(l, PersonalAction.ACCEPT, tid), now) }
    }
    @Test fun sharedMergedStorePersistsAcrossRealVaultRestartAndFailedAtomicReplace() {
        val dir = Files.createTempDirectory("shared-store-kotlin-").toFile()
        try {
            val io = PrivateFileVaultIO(File(dir, "vault")); val repo = MobileVaultRepository(io)
            var session = repo.create("shared-fixture-password".toByteArray()); var storage = store(repo, session, io)
            val legacy = storage.load(); var peer = PersonalLedger.fresh(PersonalLedger.id())
            val identity = SharedIdentity(legacy.view().agent.person_id, legacy.view().agent.agent_id, legacy.replica_id,
                mapOf(legacy.replica_id to "a".repeat(64), peer.replica_id to "b".repeat(64)))
            var local = legacy.adopt(identity, true); peer = peer.adopt(identity, true)
            val tid = PersonalLedger.id(); peer = peer.apply(req(peer, PersonalAction.CREATE, tid, "shared private fixture").copy(draft = "imported note"), now)
            local = local.importShared(peer.replica_id, peer.shared!!.operations[peer.replica_id]!!); storage.save(local)
            val ciphertext = io.read("VAULT/records/${PersonalLedger.RECORD_ID}.enc.json").toString(Charsets.UTF_8)
            assertFalse(ciphertext.contains("shared private fixture")); assertFalse(ciphertext.contains("imported note"))
            session.close(); session = repo.unlock("shared-fixture-password".toByteArray()); storage = store(repo, session, io)
            local = storage.load(); assertEquals("imported note", local.view().tasks.single().draft); assertEquals(legacy.mutations, local.mutations)
            assertEquals(peer.view().agent.person_id, local.view().agent.person_id); assertNotEquals(peer.replica_id, local.replica_id)
            val faultIO = PrivateFileVaultIO(File(dir, "vault"), beforeAtomicReplace = { error("shared pre-rename fault") })
            val fault = store(MobileVaultRepository(faultIO), session, faultIO)
            assertThrows(IllegalStateException::class.java) { fault.mutate(req(local, PersonalAction.EDIT, tid, "uncommitted edit"), now) }
            assertEquals(local, storage.load())
            local = local.apply(req(local, PersonalAction.DELETE, tid), now); storage.save(local); session.close()
            session = repo.unlock("shared-fixture-password".toByteArray()); assertTrue(store(repo, session, io).load().view().tasks.isEmpty()); session.close()
        } finally { dir.deleteRecursively() }
    }
    @Test fun corruptLedgerIsNotReplacedAndUnknownVersionRejected() {
        val l = PersonalLedger.fresh(PersonalLedger.id())
        assertThrows(IllegalArgumentException::class.java) { PersonalLedger.decode(l.copy(version = 2).let { kotlinx.serialization.json.Json.encodeToString(PersonalLedger.serializer(), it) }.toByteArray(), l.local_vault_id) }
        assertThrows(IllegalArgumentException::class.java) { l.copy(mutations = l.mutations + l.mutations.single()).view() }
    }
}
