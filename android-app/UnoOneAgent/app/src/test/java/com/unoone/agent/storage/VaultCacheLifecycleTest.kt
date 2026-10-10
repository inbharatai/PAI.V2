package com.unoone.agent.storage

import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.storage.cache.VaultCacheLifecycle
import com.unoone.agent.storage.db.UnoOneDatabase
import com.unoone.agent.storage.entity.ConversationTurnEntity
import com.unoone.agent.storage.entity.MemoryEntity
import com.unoone.agent.storage.entity.NoteEntity
import com.unoone.agent.storage.entity.SkillEntity
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The vault cache lifecycle against a REAL in-memory Room database, so the
 * actual DAO queries run. Durability contract under test:
 *
 * The adopted independent-local-store contract retains all rows on legacy USB
 * disconnect and TTL entry points, regardless of mirror status. Explicit user
 * deletion is separate. These tests execute real DAO reads after both no-op paths.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class VaultCacheLifecycleTest {

    private lateinit var db: UnoOneDatabase

    @Before
    fun setUp() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        db = Room.inMemoryDatabaseBuilder(context, UnoOneDatabase::class.java)
            .allowMainThreadQueries()
            .build()
    }

    @After
    fun tearDown() = db.close()

    // ---- clearOnVaultDisconnect --------------------------------------------

    @Test
    fun `disconnect keeps unsynced rows that only exist in the cache`() = runBlocking {
        val noteId = db.noteDao().insert(NoteEntity(title = "Offline note", content = "only copy"))
        db.memoryDao().insert(MemoryEntity(key = "wake_word", value = "namaste", type = "preference"))

        VaultCacheLifecycle.clearOnVaultDisconnect(db)

        assertNotNull("unsynced note must survive disconnect (it is the only copy)", db.noteDao().getById(noteId))
        assertNotNull(
            "unsynced memory must survive disconnect (it is the only copy)",
            db.memoryDao().getByKey("wake_word"),
        )
    }

    @Test
    fun `disconnect retains rows that already reached the vault`() = runBlocking {
        val noteId = db.noteDao().insert(NoteEntity(title = "Synced note", content = "copy in vault", vaultRecordId = "rec-1"))
        db.memoryDao().insert(MemoryEntity(key = "lang", value = "hi", vaultRecordId = "rec-2"))

        val cleared = VaultCacheLifecycle.clearOnVaultDisconnect(db)

        assertEquals(0, cleared)
        assertNotNull(db.noteDao().getById(noteId))
        assertNotNull(db.memoryDao().getByKey("lang"))
    }

    @Test
    fun `disconnect keeps skills - device-local assets, never vault-mirrored`() = runBlocking {
        val skillId = db.skillDao().insert(
            SkillEntity(name = "morning briefing", triggerPhrases = "brief me", stepsJson = "[]"),
        )

        VaultCacheLifecycle.clearOnVaultDisconnect(db)

        assertNotNull("skill must survive vault disconnect (Room is its only store)", db.skillDao().getById(skillId))
    }

    @Test
    fun `disconnect keeps both unsynced and synced conversation turns`() = runBlocking {
        val unsynced = db.conversationTurnDao().insert(
            ConversationTurnEntity(sessionId = "s1", role = "user", content = "offline question", inputType = "voice"),
        )
        db.conversationTurnDao().insert(
            ConversationTurnEntity(sessionId = "s2", role = "user", content = "synced question", inputType = "voice", vaultRecordId = "rec-t1"),
        )

        VaultCacheLifecycle.clearOnVaultDisconnect(db)

        assertNotNull("unsynced turn is the only copy in existence", db.conversationTurnDao().getById(unsynced))
        assertTrue(
            "synced turn must remain in the independent local store",
            db.conversationTurnDao().allOnce().any { it.sessionId == "s2" },
        )
    }

    // ---- TTL eviction --------------------------------------------------------

    @Test
    fun `TTL eviction keeps expired unsynced rows - they have nowhere else to live`() = runBlocking {
        val stale = System.currentTimeMillis() - VaultCacheLifecycle.DEFAULT_TTL_MILLIS - 1
        val noteId = db.noteDao().insert(NoteEntity(title = "Old offline note", content = "still the only copy", createdAt = stale))
        db.memoryDao().insert(
            MemoryEntity(key = "old_pref", value = "v", type = "preference", createdAt = stale, updatedAt = stale),
        )

        VaultCacheLifecycle.evictExpired(db)

        assertNotNull("expired but unsynced note must survive TTL", db.noteDao().getById(noteId))
        assertNotNull("expired but unsynced memory must survive TTL", db.memoryDao().getByKey("old_pref"))
    }

    @Test
    fun `TTL entry point retains expired rows that already reached the vault`() = runBlocking {
        val stale = System.currentTimeMillis() - VaultCacheLifecycle.DEFAULT_TTL_MILLIS - 1
        val noteId = db.noteDao().insert(
            NoteEntity(title = "Old synced note", content = "vault has it", createdAt = stale, vaultRecordId = "rec-1"),
        )
        db.memoryDao().insert(
            MemoryEntity(key = "old_synced", value = "v", createdAt = stale, updatedAt = stale, vaultRecordId = "rec-2"),
        )

        val evicted = VaultCacheLifecycle.evictExpired(db)

        assertEquals("independent local store never auto-evicts", 0, evicted)
        assertNotNull(db.noteDao().getById(noteId))
        assertNotNull(db.memoryDao().getByKey("old_synced"))
    }

    @Test
    fun `TTL eviction keeps skills`() = runBlocking {
        val stale = System.currentTimeMillis() - VaultCacheLifecycle.DEFAULT_TTL_MILLIS - 1
        val skillId = db.skillDao().insert(
            SkillEntity(name = "old skill", triggerPhrases = "go", stepsJson = "[]", createdAt = stale),
        )

        VaultCacheLifecycle.evictExpired(db)

        assertNotNull("expired skill must survive TTL (Room is its only store)", db.skillDao().getById(skillId))
    }

    @Test
    fun `TTL entry point keeps expired unsynced and synced turns`() = runBlocking {
        val stale = System.currentTimeMillis() - VaultCacheLifecycle.DEFAULT_TTL_MILLIS - 1
        val unsynced = db.conversationTurnDao().insert(
            ConversationTurnEntity(sessionId = "s1", role = "user", content = "only copy", inputType = "voice", createdAt = stale),
        )
        db.conversationTurnDao().insert(
            ConversationTurnEntity(sessionId = "s2", role = "assistant", content = "vault has it", inputType = "voice", createdAt = stale, vaultRecordId = "rec-t2"),
        )

        VaultCacheLifecycle.evictExpired(db)

        assertNotNull("expired unsynced turn is the only copy in existence", db.conversationTurnDao().getById(unsynced))
        assertTrue(
            "expired synced turn must remain in the independent local store",
            db.conversationTurnDao().allOnce().any { it.sessionId == "s2" },
        )
    }
}