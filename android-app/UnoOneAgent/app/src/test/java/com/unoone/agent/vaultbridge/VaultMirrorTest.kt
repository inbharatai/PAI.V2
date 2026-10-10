package com.unoone.agent.vaultbridge

import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.storage.db.UnoOneDatabase
import com.unoone.agent.storage.entity.ConversationTurnEntity
import com.unoone.agent.storage.entity.MemoryEntity
import com.unoone.agent.storage.entity.NoteEntity
import com.unoone.agent.storage.entity.SkillEntity
import com.unoone.agent.vault.VaultRecordWriter
import com.unoone.agent.vault.VaultSyncPlanner
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The vault write-through coordinator against a REAL in-memory Room database
 * (so the DAO queries — notSynced, setVaultRecordId, the tombstone queue — are
 * exercised for real) and a fake [VaultRecordWriter] that records what would
 * reach the drive. Proves: online write-through stamps the record id; offline
 * writes stay unsynced and flush in planner order on the next unlock;
 * deletions tombstone now or queue for later.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class VaultMirrorTest {

    private lateinit var db: UnoOneDatabase

    /** Records every vault call; toggle [online] to simulate lock/unlock. */
    private class FakeWriter : VaultRecordWriter {
        var online = true
        /** Fail the NEXT writeRecord call once (failure injection). */
        var failNextWrite = false
        var failTombstones = false
        val written = mutableListOf<Pair<String, ByteArray>>() // recordId, content
        val writtenFields = mutableListOf<Map<String, Any?>>()
        val tombstoned = mutableListOf<String>()
        override fun writeRecord(fields: Map<String, Any?>, content: ByteArray): String {
            val id = fields["record_id"] as String
            if (fields["tombstone"] == true) {
                // v7 deletions are idempotent empty tombstone writes (no read-then-rewrite).
                if (failTombstones) throw java.io.IOException("injected vault failure")
                assertEquals(0, content.size)
                tombstoned.add(id)
                return id
            }
            if (failNextWrite) {
                failNextWrite = false
                throw java.io.IOException("injected vault failure")
            }
            written.add(id to content.copyOf()) // the mirror zeroes its buffer after a durable write
            writtenFields.add(fields)
            return id
        }
        override fun tombstone(vaultRecordId: String, deletedAtIso: String) {
            if (failTombstones) throw java.io.IOException("injected vault failure")
            tombstoned.add(vaultRecordId)
        }
    }

    private val writer = FakeWriter()

    private fun mirror(): VaultMirror = VaultMirror(
        noteDao = db.noteDao(),
        memoryDao = db.memoryDao(),
        tombstoneDao = db.pendingTombstoneDao(),
        writerProvider = { if (writer.online) writer else null },
        deviceId = "test-device",
        pendingWriteDao = db.pendingWriteDao(),
        skillDao = db.skillDao(),
        turnDao = db.conversationTurnDao(),
    )

    @Before
    fun setUp() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        db = Room.inMemoryDatabaseBuilder(context, UnoOneDatabase::class.java)
            .allowMainThreadQueries()
            .build()
    }

    @After
    fun tearDown() = db.close()

    @Test
    fun `online note create writes through and stamps the vault record id`() = runBlocking {
        writer.online = true
        val id = db.noteDao().insert(NoteEntity(title = "Groceries", content = "turmeric"))
        mirror().onNoteCreated(id)

        assertEquals(1, writer.written.size)
        val stamped = db.noteDao().getById(id)!!.vaultRecordId
        assertEquals(writer.written.single().first, stamped)
        assertTrue("no rows should remain unsynced", db.noteDao().notSynced().isEmpty())
    }

    @Test
    fun `offline note create stays unsynced then flushes on unlock`() = runBlocking {
        writer.online = false
        val id = db.noteDao().insert(NoteEntity(title = "Offline", content = "written while locked"))
        mirror().onNoteCreated(id)

        assertTrue("nothing written while locked", writer.written.isEmpty())
        assertNull(db.noteDao().getById(id)!!.vaultRecordId)
        assertEquals(1, db.noteDao().notSynced().size)

        // Unlock and drain the backlog.
        writer.online = true
        mirror().drainBacklog()

        assertEquals(1, writer.written.size)
        assertTrue(db.noteDao().notSynced().isEmpty())
    }

    @Test
    fun `offline delete of a synced row queues a tombstone drained on unlock`() = runBlocking {
        // A row that already reached the vault.
        writer.online = true
        val id = db.noteDao().insert(NoteEntity(title = "Synced", content = "body"))
        mirror().onNoteCreated(id)
        val vid = db.noteDao().getById(id)!!.vaultRecordId!!

        // Delete it while offline.
        writer.online = false
        val row = db.noteDao().getById(id)!!
        db.noteDao().delete(row)
        mirror().onRowDeleted(row.vaultRecordId, VaultSyncPlanner.Kind.NOTE)
        assertTrue("tombstone deferred while locked", writer.tombstoned.isEmpty())
        assertEquals(1, db.pendingTombstoneDao().getAll().size)

        // Unlock: the queued tombstone drains and the queue empties.
        writer.online = true
        mirror().drainBacklog()
        assertEquals(listOf(vid), writer.tombstoned)
        assertTrue(db.pendingTombstoneDao().getAll().isEmpty())
    }

    @Test
    fun `purely local row delete does nothing in the vault`() = runBlocking {
        // No row and no outbox identity → the compatibility wake-up has nothing to tombstone.
        mirror().onRowDeleted(null, VaultSyncPlanner.Kind.NOTE)
        assertTrue(writer.tombstoned.isEmpty())
        assertTrue(db.pendingTombstoneDao().getAll().isEmpty())
    }

    @Test
    fun `memory value change rewrites the SAME vault record with revision plus one`() = runBlocking {
        writer.online = true
        val id = db.memoryDao().insert(MemoryEntity(key = "wake_word", value = "namaste", type = "preference"))
        mirror().onMemoryUpserted(id)
        val first = db.memoryDao().getByIdOnce(id)!!
        val recordId = first.vaultRecordId!!
        assertEquals(1, first.vaultRevision)

        // Upsert the same key with a new value, exactly as storePreference does.
        db.memoryDao().update(first.copy(value = "pranam"))
        mirror().onMemoryUpserted(id)

        assertEquals(2, writer.written.size)
        assertEquals("rewrite must target the same vault record", recordId, writer.written[1].first)
        val after = db.memoryDao().getByIdOnce(id)!!
        assertEquals(recordId, after.vaultRecordId)
        assertEquals(2, after.vaultRevision)
        assertEquals("metadata must carry the bumped revision", 2, writer.writtenFields[1]["revision"])
    }

    @Test
    fun `outcome telemetry never mirrors even when addressed directly`() = runBlocking {
        writer.online = true
        val id = db.memoryDao().insert(MemoryEntity(key = "outcome:sig:tool", value = "ok|", type = "outcome"))
        mirror().onMemoryUpserted(id)
        assertTrue("planner telemetry must stay device-local", writer.written.isEmpty())
        assertNull(db.memoryDao().getByIdOnce(id)!!.vaultRecordId)
    }

    @Test
    fun `memory backlog flushes but outcome telemetry is excluded`() = runBlocking {
        writer.online = false
        db.memoryDao().insert(MemoryEntity(key = "wake_word", value = "namaste", type = "preference"))
        db.memoryDao().insert(MemoryEntity(key = "outcome:sig:tool", value = "ok|", type = "outcome"))

        writer.online = true
        mirror().drainBacklog()

        // Only the user-meaningful preference reaches the vault; telemetry does not.
        assertEquals(1, writer.written.size)
        val syncedTypes = db.memoryDao().notSynced()
        assertTrue("preference should be synced away", syncedTypes.none { it.type == "preference" })
    }

    @Test
    fun `memory deletion of a synced row tombstones the vault record`() = runBlocking {
        // A preference that already reached the vault.
        writer.online = true
        val id = db.memoryDao().insert(MemoryEntity(key = "wake_word", value = "namaste", type = "preference"))
        mirror().onMemoryUpserted(id)
        val vid = db.memoryDao().getByIdOnce(id)!!.vaultRecordId!!

        // deleteMemory deletes the row (SQLite captures the link as a durable tombstone inside
        // that transaction) and THEN wakes the mirror — exactly as MemoryModule + the orchestrator do.
        val entity = db.memoryDao().getByIdOnce(id)!!
        db.memoryDao().delete(entity)
        mirror().onRowDeleted(entity.vaultRecordId, VaultSyncPlanner.Kind.MEMORY)

        assertEquals("the vault record must be tombstoned, not orphaned", listOf(vid), writer.tombstoned)
        assertNull("the local cache row is gone", db.memoryDao().getByKey("wake_word"))
    }

    @Test
    fun `memory deletion of an unsynced row tombstones only its minted identity and never a body`() = runBlocking {
        writer.online = false
        val id = db.memoryDao().insert(MemoryEntity(key = "local_pref", value = "v", type = "preference"))
        val entity = db.memoryDao().getByIdOnce(id)!!
        val minted = db.pendingWriteDao().get("MEMORY", id)!!.recordId

        db.memoryDao().delete(entity)
        writer.online = true
        mirror().onRowDeleted(entity.vaultRecordId, VaultSyncPlanner.Kind.MEMORY)

        // SQLite cannot know whether a write-before-stamp already reached the vault under the
        // minted id, so the deletion is an idempotent empty tombstone of that id — the body never
        // follows the row into the vault and the pending write is gone.
        assertTrue("the deleted value must never be written", writer.written.isEmpty())
        assertEquals(listOf(minted), writer.tombstoned)
        assertTrue(db.pendingWriteDao().getAll().isEmpty())
        assertTrue(db.pendingTombstoneDao().getAll().isEmpty())
    }

    @Test
    fun `online skill save writes through and stamps the vault record id`() = runBlocking {
        writer.online = true
        val id = db.skillDao().insert(
            SkillEntity(name = "morning briefing", triggerPhrases = "brief me", stepsJson = "[\"read_screen\"]"),
        )
        mirror().onSkillUpserted(id)

        assertEquals(1, writer.written.size)
        val fields = writer.writtenFields.single()
        assertEquals("skills canonicalize as DOCUMENT records", "DOCUMENT", fields["record_type"])
        assertEquals(1, fields["revision"])
        val stamped = db.skillDao().getById(id)!!
        assertEquals(writer.written.single().first, stamped.vaultRecordId)
        assertEquals(1, stamped.vaultRevision)
        assertTrue("no skill rows should remain unsynced", db.skillDao().notSynced().isEmpty())
    }

    @Test
    fun `skill re-save rewrites the SAME vault record with revision plus one`() = runBlocking {
        writer.online = true
        val id = db.skillDao().insert(
            SkillEntity(name = "morning briefing", triggerPhrases = "brief me", stepsJson = "[]"),
        )
        mirror().onSkillUpserted(id)
        val recordId = db.skillDao().getById(id)!!.vaultRecordId!!

        // Enable/disable or edit re-fires onSkillSaved with the same row.
        val edited = db.skillDao().getById(id)!!.copy(triggerPhrases = "brief me,good morning")
        db.skillDao().update(edited)
        mirror().onSkillUpserted(id)

        assertEquals(2, writer.written.size)
        assertEquals("rewrite must target the same vault record", recordId, writer.written[1].first)
        assertEquals("metadata must carry the bumped revision", 2, writer.writtenFields[1]["revision"])
        val after = db.skillDao().getById(id)!!
        assertEquals(recordId, after.vaultRecordId)
        assertEquals(2, after.vaultRevision)
        // The envelope is the honest {kind:"skill"} payload.
        val body = String(writer.written[1].second, Charsets.UTF_8)
        assertTrue(body.contains("\"kind\":\"skill\""))
        assertTrue(body.contains("good morning"))
    }

    @Test
    fun `offline skill save stays unsynced then flushes on unlock`() = runBlocking {
        writer.online = false
        val id = db.skillDao().insert(
            SkillEntity(name = "evening digest", triggerPhrases = "wrap up", stepsJson = "[]"),
        )
        mirror().onSkillUpserted(id)

        assertTrue("nothing written while locked", writer.written.isEmpty())
        assertNull(db.skillDao().getById(id)!!.vaultRecordId)
        assertEquals(1, db.skillDao().notSynced().size)

        // Unlock and drain the backlog.
        writer.online = true
        mirror().drainBacklog()

        assertEquals(1, writer.written.size)
        assertEquals("DOCUMENT", writer.writtenFields.single()["record_type"])
        assertTrue(db.skillDao().notSynced().isEmpty())
        assertEquals(writer.written.single().first, db.skillDao().getById(id)!!.vaultRecordId)
    }

    @Test
    fun `skill deletion of a synced row tombstones - unsynced delete is a no-op`() = runBlocking {
        // A skill that already reached the vault.
        writer.online = true
        val id = db.skillDao().insert(
            SkillEntity(name = "morning briefing", triggerPhrases = "brief me", stepsJson = "[]"),
        )
        mirror().onSkillUpserted(id)
        val vid = db.skillDao().getById(id)!!.vaultRecordId!!

        // deleteSkill deletes the row (the link is captured durably inside that transaction)
        // and THEN fires onSkillDeleted — exactly as SkillsModule + the orchestrator do.
        val entity = db.skillDao().getById(id)!!
        db.skillDao().delete(entity)
        mirror().onRowDeleted(entity.vaultRecordId, VaultSyncPlanner.Kind.SKILL)
        assertEquals("the vault record must be tombstoned, not orphaned", listOf(vid), writer.tombstoned)

        // A never-written skill deletion never writes its body; only its minted id is tombstoned.
        writer.tombstoned.clear()
        writer.online = false
        val localId = db.skillDao().insert(
            SkillEntity(name = "never synced", triggerPhrases = "x", stepsJson = "[]"),
        )
        val local = db.skillDao().getById(localId)!!
        val minted = db.pendingWriteDao().get("SKILL", localId)!!.recordId
        db.skillDao().delete(local)
        writer.online = true
        mirror().onRowDeleted(local.vaultRecordId, VaultSyncPlanner.Kind.SKILL)
        assertEquals(1, writer.written.size)
        assertEquals(listOf(minted), writer.tombstoned)
        assertTrue(db.pendingWriteDao().getAll().isEmpty())
        assertTrue(db.pendingTombstoneDao().getAll().isEmpty())
    }

    @Test
    fun `online turn record writes through and stamps the vault record id`() = runBlocking {
        writer.online = true
        val id = db.conversationTurnDao().insert(
            ConversationTurnEntity(sessionId = "sess-1", role = "user", content = "what's on my screen", inputType = "voice"),
        )
        mirror().onTurnRecorded(id)

        assertEquals(1, writer.written.size)
        val fields = writer.writtenFields.single()
        assertEquals("conversation turns canonicalize as TRANSCRIPT records", "TRANSCRIPT", fields["record_type"])
        assertEquals("turns are append-only", 1, fields["revision"])
        val body = String(writer.written.single().second, Charsets.UTF_8)
        assertTrue(body.contains("\"kind\":\"transcript\""))
        assertTrue(body.contains("what's on my screen"))
        assertEquals(writer.written.single().first, db.conversationTurnDao().getById(id)!!.vaultRecordId)
        assertTrue("no turns should remain unsynced", db.conversationTurnDao().notSynced().isEmpty())
    }

    @Test
    fun `offline turns accumulate then flush in backlog order`() = runBlocking {
        writer.online = false
        val u1 = db.conversationTurnDao().insert(
            ConversationTurnEntity(sessionId = "sess-1", role = "user", content = "call ravi", inputType = "voice"),
        )
        val a1 = db.conversationTurnDao().insert(
            ConversationTurnEntity(sessionId = "sess-1", role = "assistant", content = "Calling Ravi.", inputType = "voice"),
        )
        mirror().onTurnRecorded(u1)
        mirror().onTurnRecorded(a1)

        assertTrue("nothing written while locked", writer.written.isEmpty())
        assertEquals(2, db.conversationTurnDao().notSynced().size)

        // Unlock: drainBacklog flushes the turns in id order.
        writer.online = true
        mirror().drainBacklog()

        assertEquals(2, writer.written.size)
        assertTrue(db.conversationTurnDao().notSynced().isEmpty())
        assertEquals("turn order preserved", "call ravi", String(writer.written[0].second).substringAfter("\"content\":\"").substringBefore('"'))
    }

    @Test
    fun `already-mirrored turn is never mirrored twice`() = runBlocking {
        writer.online = true
        val id = db.conversationTurnDao().insert(
            ConversationTurnEntity(sessionId = "sess-1", role = "assistant", content = "Done.", inputType = "text"),
        )
        mirror().onTurnRecorded(id)
        // A duplicate notification (e.g. retried) must not create a second record.
        mirror().onTurnRecorded(id)

        assertEquals(1, writer.written.size)
    }

    // ---- interrupted-write + drain-failure isolation ----------------------

    @Test
    fun `interrupted vault write reuses the SAME record id on retry - no duplicate`() = runBlocking {
        // The vault write throws AFTER the record id was minted and persisted
        // (crash between vault write and cache stamp).
        writer.failNextWrite = true
        val id = db.memoryDao().insert(MemoryEntity(key = "wake_word", value = "namaste", type = "preference"))
        mirror().onMemoryUpserted(id)

        assertTrue("the write failed, so nothing reached the vault", writer.written.isEmpty())
        assertNull("the row stamp never happened", db.memoryDao().getByIdOnce(id)!!.vaultRecordId)
        assertEquals("the minted id must be persisted BEFORE the write", 1, db.pendingWriteDao().getAll().size)
        val minted = db.pendingWriteDao().getAll().single().recordId

        // Retry (next unlock): the SAME record id must be reused — the vault
        // write overwrites the same record file; a fresh id would duplicate it.
        mirror().drainBacklog()
        assertEquals(1, writer.written.size)
        assertEquals("retry must reuse the persisted id", minted, writer.written.single().first)
        assertEquals(minted, db.memoryDao().getByIdOnce(id)!!.vaultRecordId)
        assertTrue("the pending id retires once the row is stamped", db.pendingWriteDao().getAll().isEmpty())
    }

    @Test
    fun `a tombstone that throws is queued and retried on the next unlock`() = runBlocking {
        // A memory that already reached the vault.
        writer.online = true
        val id = db.memoryDao().insert(MemoryEntity(key = "wake_word", value = "namaste", type = "preference"))
        mirror().onMemoryUpserted(id)
        val vid = db.memoryDao().getByIdOnce(id)!!.vaultRecordId!!
        val entity = db.memoryDao().getByIdOnce(id)!!
        db.memoryDao().delete(entity)

        // The vault tombstone call throws — the deletion must not vanish with
        // the already-deleted local row; it queues for retry.
        writer.failTombstones = true
        mirror().onRowDeleted(entity.vaultRecordId, VaultSyncPlanner.Kind.MEMORY)
        assertTrue(writer.tombstoned.isEmpty())
        assertEquals(
            "failed tombstone must be queued, not dropped",
            listOf(vid),
            db.pendingTombstoneDao().getAll().map { it.vaultRecordId },
        )

        // Next unlock: the queued tombstone drains and the queue empties.
        writer.failTombstones = false
        mirror().drainBacklog()
        assertEquals(listOf(vid), writer.tombstoned)
        assertTrue(db.pendingTombstoneDao().getAll().isEmpty())
    }

    @Test
    fun `one failed drain op never aborts the rest of the backlog`() = runBlocking {
        writer.online = false
        db.noteDao().insert(NoteEntity(title = "Note A", content = "a"))
        db.memoryDao().insert(MemoryEntity(key = "k", value = "v", type = "preference"))

        // Unlock: the FIRST write call throws; the rest of the batch must
        // still flush (per-op isolation), and the failed op stays for the
        // next unlock.
        writer.online = true
        writer.failNextWrite = true
        mirror().drainBacklog()

        assertEquals("the failed op must not stop the batch", 1, writer.written.size)
        val stillUnsynced = db.noteDao().notSynced().size + db.memoryDao().notSynced().size
        assertEquals("only the failed op remains unsynced", 1, stillUnsynced)

        // And it flushes on the next drain.
        mirror().drainBacklog()
        assertEquals(2, writer.written.size)
        assertEquals(0, db.noteDao().notSynced().size + db.memoryDao().notSynced().size)
    }

    // ---- env-learning facts (type "envobs") -------------------------------

    /** A contract-valid fact value, exactly as EnvLearningRecorder stores it. */
    private fun envFactValue(
        subject: String,
        status: com.unoone.agent.core.contracts.EpistemicStatus,
    ): String {
        val observation = com.unoone.agent.core.contracts.EnvObservation(
            schema = com.unoone.agent.core.contracts.ContractSchemas.ENV_OBSERVATION,
            subject = subject,
            observedCapability = "execute skill '$subject'",
            evidence = "user explicitly enabled skill '$subject'",
            confidence = com.unoone.agent.core.contracts.Confidence.HIGH,
            scope = com.unoone.agent.core.contracts.EnvScope.USER,
            epistemicStatus = status,
            provenance = com.unoone.agent.core.contracts.Provenance(
                platform = "android",
                deviceId = "test-device",
                source = "user-approval",
            ),
            timestampMs = 1_760_000_000_000,
            verificationRef = if (status == com.unoone.agent.core.contracts.EpistemicStatus.VERIFIED_FACT) {
                "user_enabled_skill:$subject@1760000000000"
            } else {
                null
            },
        )
        return com.unoone.agent.core.contracts.ContractJson.encodeToString(
            com.unoone.agent.core.contracts.EnvObservation.serializer(),
            observation,
        )
    }

    private suspend fun insertEnvFact(
        subject: String,
        status: com.unoone.agent.core.contracts.EpistemicStatus = com.unoone.agent.core.contracts.EpistemicStatus.VERIFIED_FACT,
    ): Long = db.memoryDao().insert(
        MemoryEntity(key = "envfact:skill:$subject", value = envFactValue(subject, status), type = "envobs"),
    )

    @Test
    fun `online env fact writes through as an envobs DOCUMENT and stamps the link`() = runBlocking {
        writer.online = true
        val id = insertEnvFact("Suggested · Open Calendar")
        mirror().onEnvFactRecorded(id)

        assertEquals(1, writer.written.size)
        val fields = writer.writtenFields.single()
        assertEquals("env facts canonicalize as DOCUMENT records", "DOCUMENT", fields["record_type"])
        assertEquals(1, fields["revision"])
        val body = String(writer.written.single().second, Charsets.UTF_8)
        assertTrue("the honest {kind:\"envobs\"} envelope", body.contains("\"kind\":\"envobs\""))
        assertTrue("the contract body travels verbatim", body.contains("inbharat.pai.envobs.v1"))
        val stamped = db.memoryDao().getByIdOnce(id)!!
        assertEquals(writer.written.single().first, stamped.vaultRecordId)
        assertEquals(1, stamped.vaultRevision)
        assertTrue("no env fact should remain unsynced", db.memoryDao().notSynced().none { it.type == "envobs" })
    }

    @Test
    fun `an unparseable envobs value never mirrors`() = runBlocking {
        writer.online = true
        val id = db.memoryDao().insert(
            MemoryEntity(key = "envfact:skill:broken", value = "not json at all", type = "envobs"),
        )
        mirror().onEnvFactRecorded(id)
        assertTrue("honesty over reach — nothing must reach the vault", writer.written.isEmpty())
        assertNull(db.memoryDao().getByIdOnce(id)!!.vaultRecordId)
    }

    @Test
    fun `disapprove rewrites the SAME envobs record as a correction with revision plus one`() = runBlocking {
        writer.online = true
        val subject = "Suggested · Open Calendar"
        val id = insertEnvFact(subject)
        mirror().onEnvFactRecorded(id)
        val recordId = db.memoryDao().getByIdOnce(id)!!.vaultRecordId!!

        // The user disables the skill: EnvLearningRecorder.disapproveSkill
        // rewrites the SAME key as an honest CORRECTION, then re-fires the
        // mirror callback with the same row.
        val status = com.unoone.agent.core.contracts.EpistemicStatus.CORRECTION
        db.memoryDao().update(db.memoryDao().getByIdOnce(id)!!.copy(value = envFactValue(subject, status)))
        mirror().onEnvFactRecorded(id)

        assertEquals(2, writer.written.size)
        assertEquals("rewrite must target the same vault record", recordId, writer.written[1].first)
        assertEquals("metadata must carry the bumped revision", 2, writer.writtenFields[1]["revision"])
        val body = String(writer.written[1].second, Charsets.UTF_8)
        assertTrue(body.contains("\"epistemicStatus\":\"correction\""))
        assertEquals(2, db.memoryDao().getByIdOnce(id)!!.vaultRevision)
    }

    @Test
    fun `offline env facts flush on unlock but hypotheses never leave the device`() = runBlocking {
        writer.online = false
        insertEnvFact("Suggested · Open Calendar")
        // A hypothesis row the recorder wrote while locked too.
        db.memoryDao().insert(
            MemoryEntity(key = "envobs_hypo:open_calendar:Suggested · Open Calendar", value = "hypo", type = "envobs_hypo"),
        )

        assertTrue("nothing written while locked", writer.written.isEmpty())

        // Unlock: only the user-confirmed fact drains, via the envobs path.
        writer.online = true
        mirror().drainBacklog()

        assertEquals(1, writer.written.size)
        assertEquals("DOCUMENT", writer.writtenFields.single()["record_type"])
        val body = String(writer.written.single().second, Charsets.UTF_8)
        assertTrue(body.contains("\"kind\":\"envobs\""))
        assertTrue("hypotheses must never reach the vault", writer.written.none { it.second.toString(Charsets.UTF_8).contains("envobs_hypo") })
        assertTrue(db.memoryDao().notSynced().none { it.type == "envobs" })
        // The hypothesis row itself stays unsynced forever, by DAO contract.
        assertTrue(db.memoryDao().notSynced().none { it.type == "envobs_hypo" })
    }
}
