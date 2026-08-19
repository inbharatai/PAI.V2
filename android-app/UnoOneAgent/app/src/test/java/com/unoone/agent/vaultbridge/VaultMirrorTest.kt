package com.unoone.agent.vaultbridge

import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.storage.db.UnoOneDatabase
import com.unoone.agent.storage.entity.MemoryEntity
import com.unoone.agent.storage.entity.NoteEntity
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
        val written = mutableListOf<Pair<String, ByteArray>>() // recordId, content
        val writtenFields = mutableListOf<Map<String, Any?>>()
        val tombstoned = mutableListOf<String>()
        override fun writeRecord(fields: Map<String, Any?>, content: ByteArray): String {
            val id = fields["record_id"] as String
            written.add(id to content)
            writtenFields.add(fields)
            return id
        }
        override fun tombstone(vaultRecordId: String, deletedAtIso: String) {
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
        // Never synced (no vaultRecordId) → deletion has nothing to tombstone.
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
}
