package com.unoone.agent.storage.db

import android.content.Context
import androidx.room.Room
import androidx.room.withTransaction
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.storage.entity.*
import kotlinx.coroutines.runBlocking
import org.junit.*
import org.junit.Assert.*
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/** Generated Room DAOs + actual SQLite, never mock queue counts as transactional evidence. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], manifest = Config.NONE)
class VaultMutationDurabilityTest {
    private lateinit var db: UnoOneDatabase
    private lateinit var context: Context
    private val name = "vault-mutation-regression.db"
    private fun open() = Room.databaseBuilder(context, UnoOneDatabase::class.java, name)
        .allowMainThreadQueries().build()
    @Before fun setup() { context = ApplicationProvider.getApplicationContext(); context.deleteDatabase(name); db = open() }
    @After fun cleanup() { db.close(); context.deleteDatabase(name) }
    private fun reopen() { db.close(); db = open() }

    @Test fun noteMutationCommitsOutboxWithoutCallback() = runBlocking {
        val id = db.noteDao().insert(NoteEntity(title = "fixture", content = "v1"))
        reopen() // death after Room commit, before any mirror callback
        val pending = db.pendingWriteDao().get("NOTE", id)
        assertNotNull("Room mutation must atomically enqueue, not rely on callback", pending)
        assertEquals("v1", db.noteDao().getById(id)!!.content)
    }

    @Test fun linkedMemoryEditSurvivesDeathBeforeCallback() = runBlocking {
        val id = db.memoryDao().insert(MemoryEntity(key = "fixture", value = "v1", type = "preference", vaultRecordId = "old-record", vaultRevision = 3))
        db.memoryDao().update(db.memoryDao().getByIdOnce(id)!!.copy(value = "v2"))
        reopen()
        assertEquals("v2", db.memoryDao().getByIdOnce(id)!!.value)
        assertEquals("old-record", db.pendingWriteDao().get("MEMORY", id)?.recordId)
    }

    @Test fun skillDeleteRetainsBothLinkedAndWriteBeforeStampIdentity() = runBlocking {
        val id = db.skillDao().insert(SkillEntity(name = "fixture", triggerPhrases = "fixture", stepsJson = "[]", vaultRecordId = "linked"))
        db.pendingWriteDao().insert(PendingWriteEntity(recordKind = "SKILL", localId = id, recordId = "pending-before-stamp"))
        db.skillDao().delete(db.skillDao().getById(id)!!)
        reopen()
        assertNull(db.skillDao().getById(id))
        assertEquals(setOf("linked", "pending-before-stamp"), db.pendingTombstoneDao().getAll().map { it.vaultRecordId }.toSet())
        assertNull(db.pendingWriteDao().get("SKILL", id))
    }

    @Test fun mutationAndEnqueueRollbackTogether() = runBlocking {
        try {
            db.withTransaction {
                db.noteDao().insert(NoteEntity(title = "rolled back", content = "fixture"))
                error("injected process boundary before commit")
            }
        } catch (_: IllegalStateException) { }
        reopen()
        assertTrue(db.noteDao().allOnce().isEmpty())
        assertTrue(db.pendingWriteDao().getAll().isEmpty())
    }
}
