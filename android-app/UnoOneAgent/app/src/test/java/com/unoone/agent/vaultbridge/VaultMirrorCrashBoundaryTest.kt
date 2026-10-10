package com.unoone.agent.vaultbridge

import android.content.Context
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.storage.db.UnoOneDatabase
import com.unoone.agent.storage.entity.MemoryEntity
import com.unoone.agent.storage.entity.NoteEntity
import com.unoone.agent.storage.entity.SkillEntity
import com.unoone.agent.vault.VaultRecordWriter
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * F2 regression: generated Room DAOs against a FILE-backed SQLite database that is closed and
 * reopened at every fault boundary (the only honest stand-in for process death on the JVM), plus
 * a vault writer that keeps the LAST durable state per record id. Assertions are on exact row
 * values, record revisions and tombstones — never on queue counts alone.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], manifest = Config.NONE)
class VaultMirrorCrashBoundaryTest {
    private class Death(val at: String) : RuntimeException("process death at $at")

    /** Vault stand-in: one durable envelope per record id (writes overwrite, like the real IO). */
    private class VaultStore : VaultRecordWriter {
        val records = linkedMapOf<String, Pair<Map<String, Any?>, ByteArray>>()
        var writes = 0
        var legacyTombstones = 0
        override fun writeRecord(fields: Map<String, Any?>, content: ByteArray): String {
            writes++
            val id = fields["record_id"] as String
            records[id] = fields.toMap() to content.copyOf()
            return id
        }
        override fun tombstone(vaultRecordId: String, deletedAtIso: String) { legacyTombstones++ }
        /** Plaintext value carried by the record envelope (JSON body); "" for an empty tombstone body. */
        fun body(id: String): String = String(records.getValue(id).second, Charsets.UTF_8)
            .let { json -> Regex("\"(?:content|value)\":\"((?:[^\"\\\\]|\\\\.)*)\"").find(json)?.groupValues?.get(1) ?: json }
        fun revision(id: String) = records.getValue(id).first["revision"] as Int
        fun isTombstone(id: String) = records.getValue(id).first["tombstone"] == true
    }

    private lateinit var context: Context
    private lateinit var db: UnoOneDatabase
    private val vault = VaultStore()
    private val name = "crash-boundary.db"
    private var dieAt: String? = null
    private var onCheckpoint: suspend (String) -> Unit = {}

    private fun open() = Room.databaseBuilder(context, UnoOneDatabase::class.java, name).allowMainThreadQueries().build()
    private fun crash() { db.close(); db = open() } // death + restart: only committed SQLite state survives

    private fun mirror() = VaultMirror(
        noteDao = db.noteDao(), memoryDao = db.memoryDao(), tombstoneDao = db.pendingTombstoneDao(),
        writerProvider = { vault }, deviceId = "device-under-test", pendingWriteDao = db.pendingWriteDao(),
        skillDao = db.skillDao(), turnDao = db.conversationTurnDao(),
        checkpoint = { at -> onCheckpoint(at); if (at == dieAt) { dieAt = null; throw Death(at) } },
    )

    @Before fun setUp() { context = ApplicationProvider.getApplicationContext(); context.deleteDatabase(name); db = open() }
    @After fun tearDown() { db.close(); context.deleteDatabase(name) }

    private suspend fun noteBody(id: Long) = db.noteDao().getById(id)!!

    private fun assertDrained(kind: String, localId: Long) {
        runBlocking { assertNull("generation must be retired", db.pendingWriteDao().get(kind, localId)) }
    }

    @Test fun deathBeforeVaultWriteKeepsGenerationAndDrainsExactlyOnce() = runBlocking {
        val id = db.noteDao().insert(NoteEntity(title = "t", content = "v1"))
        dieAt = "before-write"
        mirror().onNoteCreated(id) // bestEffort swallows the injected death
        crash()
        assertEquals(0, vault.writes)
        val pending = db.pendingWriteDao().get("NOTE", id)!!
        mirror().drainBacklog()
        assertEquals(1, vault.writes)
        assertEquals("v1", vault.body(pending.recordId))
        assertEquals(pending.recordId, noteBody(id).vaultRecordId)
        assertEquals(pending.revision, noteBody(id).vaultRevision)
        assertDrained("NOTE", id)
        mirror().drainBacklog()
        assertEquals("nothing left to drain", 1, vault.writes)
    }

    @Test fun deathAfterWriteBeforeStampReusesSameRecordAndRevision() = runBlocking {
        val id = db.noteDao().insert(NoteEntity(title = "t", content = "v1"))
        dieAt = "after-write"
        mirror().onNoteCreated(id)
        crash()
        assertEquals(1, vault.records.size)
        assertNull("stamp did not happen before death", noteBody(id).vaultRecordId)
        val pending = db.pendingWriteDao().get("NOTE", id)!!
        assertTrue(vault.records.containsKey(pending.recordId))
        mirror().drainBacklog()
        assertEquals("retry rewrote the SAME record id, no duplicate", 1, vault.records.size)
        assertEquals(pending.revision, vault.revision(pending.recordId))
        assertEquals(pending.recordId, noteBody(id).vaultRecordId)
        assertDrained("NOTE", id)
    }

    @Test fun deathAfterStampBeforeRetireIsIdempotent() = runBlocking {
        val id = db.skillDao().insert(SkillEntity(name = "s", triggerPhrases = "a", stepsJson = "[\"x\"]"))
        dieAt = "after-stamp"
        mirror().onSkillUpserted(id)
        crash()
        val linked = db.skillDao().getById(id)!!
        assertNotNull("stamp committed before death", linked.vaultRecordId)
        val pending = db.pendingWriteDao().get("SKILL", id)!!
        assertEquals(linked.vaultRecordId, pending.recordId)
        mirror().drainBacklog()
        assertEquals(1, vault.records.size)
        assertEquals(pending.revision, vault.revision(pending.recordId))
        assertEquals(linked.vaultRecordId, db.skillDao().getById(id)!!.vaultRecordId)
        assertDrained("SKILL", id)
    }

    @Test fun deleteAfterWriteBeforeStampTombstonesTheWrittenRecord() = runBlocking {
        val id = db.noteDao().insert(NoteEntity(title = "t", content = "secret"))
        dieAt = "after-write"
        mirror().onNoteCreated(id)
        crash()
        val written = db.pendingWriteDao().get("NOTE", id)!!.recordId
        assertNull(noteBody(id).vaultRecordId) // UI copy would carry a null link
        db.noteDao().delete(noteBody(id))      // same path NotesViewModel.deleteNote uses
        crash()                                // death before any callback
        assertNull(db.noteDao().getById(id))
        assertNull(db.pendingWriteDao().get("NOTE", id))
        val tombstone = db.pendingTombstoneDao().getAll().single()
        assertEquals(written, tombstone.vaultRecordId)
        mirror().drainBacklog()
        assertTrue("record written before the crash carries a tombstone", vault.isTombstone(written))
        assertEquals("", vault.body(written))
        assertTrue(vault.revision(written) > 1)
        assertEquals(0, vault.legacyTombstones)
        assertTrue(db.pendingTombstoneDao().getAll().isEmpty())
    }

    @Test fun linkedUpdateCommitsOutboxWithMutationAndRewritesSameRecord() = runBlocking {
        val id = db.memoryDao().insert(MemoryEntity(key = "k", value = "v1", type = "preference"))
        mirror().onMemoryUpserted(id)
        val recordId = db.memoryDao().getByIdOnce(id)!!.vaultRecordId!!
        assertEquals(1, vault.revision(recordId))
        db.memoryDao().update(db.memoryDao().getByIdOnce(id)!!.copy(value = "v2"))
        crash() // death between Room commit and the module callback
        val pending = db.pendingWriteDao().get("MEMORY", id)!!
        assertEquals(recordId, pending.recordId)
        assertEquals(2, pending.revision)
        assertEquals(recordId, db.memoryDao().getByIdOnce(id)!!.vaultRecordId)
        assertEquals(listOf(id), db.memoryDao().notSynced().map { it.id })
        mirror().drainBacklog()
        assertEquals(1, vault.records.size)
        assertEquals("v2", vault.body(recordId))
        assertEquals(2, vault.revision(recordId))
        assertEquals(2, db.memoryDao().getByIdOnce(id)!!.vaultRevision)
        assertDrained("MEMORY", id)
    }

    @Test fun newerEditRacingAnOldDrainIsNeverStampedOrRetired() = runBlocking {
        val id = db.noteDao().insert(NoteEntity(title = "t", content = "v1"))
        mirror().onNoteCreated(id)
        val recordId = noteBody(id).vaultRecordId!!
        db.noteDao().update(noteBody(id).copy(content = "v2"))
        var raced = false
        onCheckpoint = { at ->
            if (at == "after-write" && !raced) { raced = true; db.noteDao().update(noteBody(id).copy(content = "v3")) }
        }
        mirror().drainBacklog() // v2 reaches the vault, but v3 was committed before the stamp
        assertEquals("v2", vault.body(recordId))
        val pending = db.pendingWriteDao().get("NOTE", id)!!
        assertEquals("newer generation survives the old drain's CAS", 3, pending.revision)
        assertEquals("stale stamp must not claim the newer edit", 1, noteBody(id).vaultRevision)
        onCheckpoint = {}
        mirror().drainBacklog()
        assertEquals("v3", vault.body(recordId))
        assertEquals(3, vault.revision(recordId))
        assertEquals(3, noteBody(id).vaultRevision)
        assertDrained("NOTE", id)
    }

    @Test fun deathAfterTombstoneWriteBeforeRetireReplaysIdempotently() = runBlocking {
        val id = db.noteDao().insert(NoteEntity(title = "t", content = "v1"))
        mirror().onNoteCreated(id)
        val recordId = noteBody(id).vaultRecordId!!
        db.noteDao().delete(noteBody(id))
        dieAt = "after-tombstone"
        mirror().drainBacklog()
        crash()
        assertTrue(vault.isTombstone(recordId))
        assertEquals("tombstone intent retained until retired", recordId, db.pendingTombstoneDao().getAll().single().vaultRecordId)
        mirror().drainBacklog()
        assertTrue(vault.isTombstone(recordId))
        assertEquals(1, vault.records.size)
        assertTrue(db.pendingTombstoneDao().getAll().isEmpty())
    }

    @Test fun staleEntitySaveCannotEraseACommittedLink() = runBlocking {
        val id = db.noteDao().insert(NoteEntity(title = "t", content = "v1"))
        val stale = noteBody(id) // captured before the stamp: vaultRecordId == null
        mirror().onNoteCreated(id)
        val recordId = noteBody(id).vaultRecordId!!
        db.noteDao().update(stale) // identical content, null link, old revision
        crash()
        assertEquals(recordId, noteBody(id).vaultRecordId)
        assertEquals(1, noteBody(id).vaultRevision)
        assertNull(db.pendingWriteDao().get("NOTE", id))
        assertTrue(db.noteDao().notSynced().isEmpty())
    }

    @Test fun bulkDeleteCapturesEveryLinkedAndPendingIdentity() = runBlocking {
        val linked = db.noteDao().insert(NoteEntity(title = "a", content = "shared tag", tags = "x"))
        mirror().onNoteCreated(linked)
        val linkedId = noteBody(linked).vaultRecordId!!
        val unstamped = db.noteDao().insert(NoteEntity(title = "b", content = "shared tag", tags = "x"))
        dieAt = "after-write"
        mirror().onNoteCreated(unstamped)
        crash()
        val unstampedId = db.pendingWriteDao().get("NOTE", unstamped)!!.recordId
        assertEquals(2, db.noteDao().deleteByQuery("shared tag"))
        crash()
        assertEquals(setOf(linkedId, unstampedId), db.pendingTombstoneDao().getAll().map { it.vaultRecordId }.toSet())
        assertTrue(db.pendingWriteDao().getAll().isEmpty())
        mirror().drainBacklog()
        assertTrue(vault.isTombstone(linkedId))
        assertTrue(vault.isTombstone(unstampedId))
        assertTrue(db.pendingTombstoneDao().getAll().isEmpty())
    }
}
