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
import org.junit.Assert.assertNull
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
 * 1. `clearOnVaultDisconnect` and TTL eviction may only remove rows that have
 *    actually reached the vault (`vaultRecordId != null`); an unsynced row is
 *    the ONLY copy in existence and must survive both paths.
 * 2. Skills are device-local user assets (never vault-mirrored): wiping them
 *    on disconnect/TTL would destroy the only copy — they are exempt, like
 *    model_metadata.
 * 3. Action logs are device-local audit and never mirror: they are still
 *    fully cleared on disconnect (privacy wipe) and TTL-evicted.
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
    fun `disconnect wipes rows that already reached the vault`() = runBlocking {
        val noteId = db.noteDao().insert(NoteEntity(title = "Synced note", content = "copy in vault", vaultRecordId = "rec-1"))
        db.memoryDao().insert(MemoryEntity(key = "lang", value = "hi", vaultRecordId = "rec-2"))

        val cleared = VaultCacheLifecycle.clearOnVaultDisconnect(db)

        assertEquals(2, cleared)
        assertNull(db.noteDao().getById(noteId))
        assertNull(db.memoryDao().getByKey("lang"))
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
    fun `disconnect keeps unsynced conversation turns and wipes synced ones`() = runBlocking {
        val unsynced = db.conversationTurnDao().insert(
            ConversationTurnEntity(sessionId = "s1", role = "user", content = "offline question", inputType = "voice"),
        )
        db.conversationTurnDao().insert(
            ConversationTurnEntity(sessionId = "s2", role = "user", content = "synced question", inputType = "voice", vaultRecordId = "rec-t1"),
        )

        VaultCacheLifecycle.clearOnVaultDisconnect(db)

        assertNotNull("unsynced turn is the only copy in existence", db.conversationTurnDao().getById(unsynced))
        assertTrue(
            "synced turn (copy in vault) must be wiped from the plaintext cache",
            db.conversationTurnDao().allOnce().none { it.sessionId == "s2" },
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
    fun `TTL eviction removes expired rows that already reached the vault`() = runBlocking {
        val stale = System.currentTimeMillis() - VaultCacheLifecycle.DEFAULT_TTL_MILLIS - 1
        val noteId = db.noteDao().insert(
            NoteEntity(title = "Old synced note", content = "vault has it", createdAt = stale, vaultRecordId = "rec-1"),
        )
        db.memoryDao().insert(
            MemoryEntity(key = "old_synced", value = "v", createdAt = stale, updatedAt = stale, vaultRecordId = "rec-2"),
        )

        val evicted = VaultCacheLifecycle.evictExpired(db)

        assertTrue("at least the synced rows must be evicted", evicted >= 2)
        assertNull(db.noteDao().getById(noteId))
        assertNull(db.memoryDao().getByKey("old_synced"))
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
    fun `TTL eviction keeps expired unsynced turns and removes expired synced ones`() = runBlocking {
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
            "expired synced turn (copy in vault) must be evicted",
            db.conversationTurnDao().allOnce().none { it.sessionId == "s2" },
        )
    }
}