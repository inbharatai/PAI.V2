package com.unoone.agent.memory

import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.storage.db.UnoOneDatabase
import com.unoone.agent.storage.entity.MemoryEntity
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
 * The deletion half of the vault-notification seam: `deleteMemory` deletes the
 * row — SQLite captures its vault link as a durable `pending_tombstones` row
 * inside that same transaction — and THEN fires [MemoryModule]'s
 * `onUserMemoryDeleted` callback with the entity so the app layer can wake the
 * drain. The orchestrator wires this callback to
 * `VaultMirror.onRowDeleted(kind = MEMORY)`; the callback is a wake-up, the
 * tombstone row is the durability.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class MemoryModuleDeleteCallbackTest {

    private lateinit var db: UnoOneDatabase
    private val deleted = mutableListOf<MemoryEntity>()

    @Before
    fun setUp() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        db = Room.inMemoryDatabaseBuilder(context, UnoOneDatabase::class.java)
            .allowMainThreadQueries()
            .build()
    }

    @After
    fun tearDown() = db.close()

    private val tombstonesAtCallback = mutableListOf<String>()
    private fun module() = MemoryModule(
        db.memoryDao(),
        onUserMemoryDeleted = { entity ->
            deleted.add(entity)
            tombstonesAtCallback += db.pendingTombstoneDao().getAll().map { it.vaultRecordId }
        },
    )

    @Test
    fun `deleteMemory fires the callback with the full entity before the row is gone`() = runBlocking {
        val id = db.memoryDao().insert(
            MemoryEntity(key = "wake_word", value = "namaste", type = "preference", vaultRecordId = "rec-7"),
        )

        module().deleteMemory(db.memoryDao().getByIdOnce(id)!!)

        assertEquals(1, deleted.size)
        assertEquals("rec-7", deleted.single().vaultRecordId)
        assertNull("local row is deleted", db.memoryDao().getByKey("wake_word"))
        assertEquals("deletion intent was already durable when the wake-up ran", listOf("rec-7"), tombstonesAtCallback)
    }

    @Test
    fun `deleteMemory of a never-synced row still reports the entity - null link is the caller's no-op case`() = runBlocking {
        val id = db.memoryDao().insert(MemoryEntity(key = "local", value = "v", type = "preference"))

        module().deleteMemory(db.memoryDao().getByIdOnce(id)!!)

        assertEquals(1, deleted.size)
        assertNull(deleted.single().vaultRecordId)
        assertTrue(db.memoryDao().getByKey("local") == null)
    }
}