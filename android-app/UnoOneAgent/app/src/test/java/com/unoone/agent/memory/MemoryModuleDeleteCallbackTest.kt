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
 * The deletion half of the vault-notification seam: `deleteMemory` must fire
 * [MemoryModule]'s `onUserMemoryDeleted` callback with the FULL entity (the
 * row's `vaultRecordId` included) so the app layer can tombstone the vault
 * record — BEFORE the local row is deleted, because after it the link is
 * unrecoverable. The orchestrator wires this callback to
 * `VaultMirror.onRowDeleted(kind = MEMORY)`; without it, deleting a memory on
 * the phone leaves an orphaned live record in the shared vault that the
 * desktop would resurrect.
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

    private fun module() = MemoryModule(
        db.memoryDao(),
        onUserMemoryDeleted = { deleted.add(it) },
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