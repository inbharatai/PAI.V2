package com.unoone.agent.vaultbridge

import android.content.Context
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.skills.SkillsModule
import com.unoone.agent.storage.db.UnoOneDatabase
import com.unoone.agent.storage.entity.ConversationTurnEntity
import com.unoone.agent.storage.entity.NoteEntity
import com.unoone.agent.storage.entity.PendingTombstoneEntity
import com.unoone.agent.storage.entity.PendingWriteEntity
import com.unoone.agent.vault.MobileVaultRepository
import com.unoone.agent.vault.PrivateFileVaultIO
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.nio.file.Files

/**
 * F1 regression: a clean install that seeds the real built-in catalog and writes a note and a
 * transcript BEFORE the local file vault exists must bind as `local` and drain its backlog
 * once; data with genuine historical vault provenance must still quarantine. Real generated Room,
 * the real SkillsModule, the real VaultConnection/MobileVaultRepository crypto and files — only
 * the Android Context/Os creation path is bypassed by injecting the JVM file adapter (as in
 * LocalVaultConnectionTest). LocalVaultSetup.open's decision is exactly
 * `pendingWriteDao().hasHistoricalAuthority()` -> `VaultConnection.createLocal(password, it)`.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], manifest = Config.NONE)
class FreshInstallBindingTest {
    private lateinit var db: UnoOneDatabase
    private lateinit var io: PrivateFileVaultIO

    @Before fun setUp() {
        db = Room.inMemoryDatabaseBuilder(ApplicationProvider.getApplicationContext<Context>(), UnoOneDatabase::class.java)
            .allowMainThreadQueries().build()
        VaultConnection.lock()
        io = PrivateFileVaultIO(Files.createTempDirectory("fresh-install-bridge").toFile())
        for ((field, value) in mapOf("privateIO" to io, "repository" to MobileVaultRepository(io))) {
            VaultConnection::class.java.getDeclaredField(field).apply { isAccessible = true }.set(null, value)
        }
    }
    @After fun tearDown() { VaultConnection.lock(); db.close() }

    private fun password() = "synthetic fresh install phrase".toByteArray()
    private fun mirror() = VaultMirror(db.noteDao(), db.memoryDao(), db.pendingTombstoneDao(), { VaultConnection.writer() },
        "fresh-device", db.pendingWriteDao(), db.skillDao(), db.conversationTurnDao())
    private fun liveRecordIds() = VaultConnection.reader()!!.listRecordMetadata()
        .filter { it["tombstone"] != true }.map { it["record_id"] as String }.toSet()

    @Test fun cleanInstallSeedsBuiltInsAndPreSetupDataThenBindsLocalAndDrainsOnce() = runBlocking {
        val skills = SkillsModule(db.skillDao(), db.memoryDao(), onSkillSaved = { mirror().onSkillUpserted(it.id) })
        skills.ensureBuiltIns() // real seeding through the injected mirror callback while no vault exists
        val note = db.noteDao().insert(NoteEntity(title = "before setup", content = "pre-setup note"))
        mirror().onNoteCreated(note)
        val turn = db.conversationTurnDao().insert(ConversationTurnEntity(sessionId = "s", role = "user", content = "hi", inputType = "voice"))
        mirror().onTurnRecorded(turn)
        assertTrue(db.skillDao().allOnce().isNotEmpty())
        assertEquals(db.skillDao().allOnce().size + 2, db.pendingWriteDao().getAll().size)
        assertTrue(db.pendingWriteDao().getAll().all { it.origin == "FRESH" })
        assertNull("no file vault yet: nothing may be written", VaultConnection.writer())

        // First creation: exactly LocalVaultSetup.open(create = true)'s decision.
        val historical = db.pendingWriteDao().hasHistoricalAuthority()
        assertFalse("fresh unbound local data is not legacy authority", historical)
        VaultConnection.createLocal(password(), historical)
        assertEquals("${VaultConnection.localVaultId()}\nlocal\n", String(io.read("room-binding.txt"), Charsets.UTF_8))
        assertTrue(VaultConnection.isBridgeAllowed())

        mirror().drainBacklog()
        assertTrue(db.pendingWriteDao().getAll().isEmpty())
        assertTrue(db.noteDao().notSynced().isEmpty() && db.skillDao().notSynced().isEmpty() && db.conversationTurnDao().notSynced().isEmpty())
        val expected = (db.skillDao().allOnce().map { it.vaultRecordId!! } + db.noteDao().getById(note)!!.vaultRecordId!! +
            db.conversationTurnDao().getById(turn)!!.vaultRecordId!!).toSet()
        assertEquals(expected, liveRecordIds())
        val revisions = VaultConnection.reader()!!.listRecordMetadata().associate { it["record_id"] to it["revision"] }
        mirror().drainBacklog() // one-time backlog: a second drain changes nothing
        assertEquals(revisions, VaultConnection.reader()!!.listRecordMetadata().associate { it["record_id"] to it["revision"] })

        // Restart/unlock keeps the binding usable; a fresh note after setup writes through.
        VaultConnection.lock()
        assertTrue(VaultConnection.unlock(password()))
        assertTrue(VaultConnection.isBridgeAllowed())
        val later = db.noteDao().insert(NoteEntity(title = "after setup", content = "write-through"))
        mirror().onNoteCreated(later)
        assertNotNull(db.noteDao().getById(later)!!.vaultRecordId)
        assertEquals(expected.size + 1, liveRecordIds().size)
    }

    @Test fun genuineHistoricalPendingIdStillQuarantinesEvenNextToFreshSeeding() = runBlocking {
        SkillsModule(db.skillDao(), db.memoryDao()).ensureBuiltIns()
        val note = db.noteDao().insert(NoteEntity(title = "legacy", content = "written to an older vault before a crash"))
        // What MIGRATION_6_7 produces for a v6 pending id of unknown provenance.
        db.pendingWriteDao().retire(db.pendingWriteDao().get("NOTE", note)!!.id, db.pendingWriteDao().get("NOTE", note)!!.revision)
        db.pendingWriteDao().insert(PendingWriteEntity(recordKind = "NOTE", localId = note, recordId = "3c1f1b2a-9d7e-4c7a-a1d2-0f0e0d0c0b0a", origin = "HISTORICAL"))
        val historical = db.pendingWriteDao().hasHistoricalAuthority()
        assertTrue(historical)
        VaultConnection.createLocal(password(), historical)
        assertEquals("${VaultConnection.localVaultId()}\nmigration-required\n", String(io.read("room-binding.txt"), Charsets.UTF_8))
        assertFalse(VaultConnection.isBridgeAllowed())
        val before = db.pendingWriteDao().getAll()
        mirror().drainBacklog()
        assertEquals("nothing is drained into the new key root", before, db.pendingWriteDao().getAll())
        assertTrue(db.skillDao().allOnce().all { it.vaultRecordId == null })
        assertNull(VaultConnection.writer())
        assertTrue("no record reached the new key root", io.list("VAULT/records").isEmpty())
    }

    @Test fun historicalTombstoneOrLinkAloneIsAuthorityButFreshTombstoneIsNot() = runBlocking {
        val fresh = db.noteDao().insert(NoteEntity(title = "fresh", content = "deleted before setup"))
        db.noteDao().delete(db.noteDao().getById(fresh)!!)
        assertEquals("FRESH", db.pendingTombstoneDao().getAll().single().origin)
        assertFalse(db.pendingWriteDao().hasHistoricalAuthority())
        val legacy = db.pendingTombstoneDao().insert(PendingTombstoneEntity(vaultRecordId = "legacy-deleted", recordKind = "NOTE", deletedAtIso = "2026-01-01T00:00:00Z"))
        assertTrue(db.pendingWriteDao().hasHistoricalAuthority())
        db.pendingTombstoneDao().deleteById(legacy)
        assertFalse(db.pendingWriteDao().hasHistoricalAuthority())
        db.noteDao().insert(NoteEntity(title = "linked", content = "reached an earlier vault", vaultRecordId = "older-vault-record"))
        assertTrue(db.pendingWriteDao().hasHistoricalAuthority())
    }
}
