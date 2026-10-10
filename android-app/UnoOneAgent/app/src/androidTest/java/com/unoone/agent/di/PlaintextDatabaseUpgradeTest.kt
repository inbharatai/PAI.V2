package com.unoone.agent.di

import android.content.Context
import android.database.sqlite.SQLiteDatabase
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.unoone.agent.storage.cache.CacheKeyManager
import com.unoone.agent.storage.cache.DatabaseOpenPolicy
import com.unoone.agent.storage.cache.DatabaseRecoveryRequired
import com.unoone.agent.storage.cache.PassphraseCipher
import com.unoone.agent.storage.cache.PlaintextUpgradeFiles
import net.zetetic.database.DatabaseErrorHandler
import net.zetetic.database.sqlcipher.SQLiteDatabase as CipherDatabase
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

/** Actual Android SQLite + SQLCipher 4.17 + Room generated migrations, not mocks. These tests are
 * intentionally NOT labelled device-passing until connectedAndroidTest runs on a provisioned device.
 */
@RunWith(AndroidJUnit4::class)
class PlaintextDatabaseUpgradeTest {
    private lateinit var context: Context
    private lateinit var directory: File
    private lateinit var database: File
    private lateinit var wrapped: File
    private val cipher = object : PassphraseCipher {
        // Test-only wrapping; production uses Keystore AES-GCM. Native SQLCipher encryption is real.
        override fun encrypt(plaintext: ByteArray) = byteArrayOf(42) + plaintext
        override fun decrypt(blob: ByteArray): ByteArray { require(blob[0] == 42.toByte()); return blob.copyOfRange(1, blob.size) }
    }
    @Before fun setup() {
        context = ApplicationProvider.getApplicationContext()
        directory = File(context.noBackupFilesDir, "upgrade-test-${UUID.randomUUID()}").canonicalFile.apply { check(mkdir()) }
        database = File(directory, "unoone_database")
        wrapped = File(directory, "test.wrapped")
        System.loadLibrary("sqlcipher")
    }
    @After fun cleanup() { directory.deleteRecursively() }
    private fun upgrade(checkpoint: (String) -> Unit = {}) = PlaintextDatabaseUpgrade(context,
        database, File(directory, "recovery"), wrapped, CacheKeyManager(cipher, wrapped), checkpoint)
    private fun refusal(block: () -> Unit) {
        try { block(); fail("Recovery refusal required") } catch (_: DatabaseRecoveryRequired) { }
    }
    private fun encrypted(key: ByteArray = CacheKeyManager(cipher, wrapped).getOrCreate().passphrase): CipherDatabase =
        CipherDatabase.openDatabase(database.path, key, null, CipherDatabase.OPEN_READONLY or CipherDatabase.NO_LOCALIZED_COLLATORS,
            DatabaseErrorHandler { _, _ -> throw IllegalStateException("test corruption") }, null)

    /** Independent source v2 DDL. Copy the closed-to-writes producer's main/WAL/SHM while it is open,
     * then close it: the migration input contains committed rows ONLY in WAL, with no live handles.
     */
    private fun plaintextFixture(version: Int = 2, uncommitted: Boolean = false) {
        val producer = File(directory, "producer")
        SQLiteDatabase.openOrCreateDatabase(producer, null).use { db ->
            db.execSQL("CREATE TABLE notes(id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,title TEXT NOT NULL,content TEXT NOT NULL,tags TEXT NOT NULL,createdAt INTEGER NOT NULL,updatedAt INTEGER NOT NULL,reminderTime INTEGER)")
            db.execSQL("CREATE TABLE memories(id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,`key` TEXT NOT NULL,value TEXT NOT NULL,type TEXT NOT NULL,createdAt INTEGER NOT NULL,updatedAt INTEGER NOT NULL)")
            db.execSQL("CREATE TABLE skills(id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,name TEXT NOT NULL,triggerPhrases TEXT NOT NULL,stepsJson TEXT NOT NULL,riskLevel INTEGER NOT NULL,enabled INTEGER NOT NULL,createdAt INTEGER NOT NULL,updatedAt INTEGER NOT NULL)")
            db.execSQL("CREATE TABLE action_logs(id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,inputText TEXT NOT NULL,inputType TEXT NOT NULL,selectedTool TEXT NOT NULL,toolArgsJson TEXT NOT NULL,riskLevel INTEGER NOT NULL,status TEXT NOT NULL,errorMessage TEXT,sttLatencyMs INTEGER,modelLatencyMs INTEGER,ttsLatencyMs INTEGER,createdAt INTEGER NOT NULL)")
            db.execSQL("CREATE TABLE model_metadata(id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,modelName TEXT NOT NULL,modelType TEXT NOT NULL,localPath TEXT NOT NULL,checksum TEXT NOT NULL,status TEXT NOT NULL,lastLoadedAt INTEGER)")
            db.execSQL("CREATE TABLE room_master_table(id INTEGER PRIMARY KEY,identity_hash TEXT)")
            db.execSQL("INSERT INTO room_master_table VALUES(42,'fixture-source-room2')")
            if (version == 2) listOf(
                "CREATE INDEX index_notes_title ON notes(title)", "CREATE INDEX index_notes_tags ON notes(tags)",
                "CREATE INDEX index_notes_createdAt ON notes(createdAt)", "CREATE INDEX index_action_logs_status ON action_logs(status)",
                "CREATE INDEX index_action_logs_createdAt ON action_logs(createdAt)", "CREATE UNIQUE INDEX index_memories_key ON memories(`key`)",
                "CREATE INDEX index_memories_type ON memories(type)", "CREATE UNIQUE INDEX index_skills_name ON skills(name)"
            ).forEach(db::execSQL)
            db.version = version
            assertTrue(db.enableWriteAheadLogging())
            db.rawQuery("PRAGMA wal_checkpoint(TRUNCATE)", null).use { assertTrue(it.moveToFirst()); assertEquals(0, it.getInt(0)) }
            db.rawQuery("PRAGMA wal_autocheckpoint=0", null).use { assertTrue(it.moveToFirst()) }
            db.beginTransaction()
            try {
                db.execSQL("INSERT INTO notes VALUES(7,?,?,?,?,?,NULL)", arrayOf<Any>("O'Brien; DROP TABLE notes;--", "Unicode हिन्दी\u0000tail", "", Long.MAX_VALUE - 2, 3L))
                db.execSQL("INSERT INTO notes VALUES(99,'deleted','deleted','',1,1,NULL)")
                db.execSQL("DELETE FROM notes WHERE id=99")
                db.execSQL("INSERT INTO memories VALUES(4,'pref','preserve','preference',1,2)")
                db.execSQL("INSERT INTO skills VALUES(9,'custom','trigger','[{\"action\":\"note\"}]',2,0,1,2)")
                db.execSQL("INSERT INTO action_logs VALUES(8,'input','text','tool','{}',1,'failed',NULL,0,NULL,99,2)")
                db.execSQL("INSERT INTO model_metadata VALUES(3,'model','llm','/private/path','abc','present',NULL)")
                db.setTransactionSuccessful()
            } finally { db.endTransaction() }
            assertTrue(File(producer.path + "-wal").length() > 0)
            if (uncommitted) {
                db.execSQL("PRAGMA cache_size=1")
                db.beginTransaction()
                repeat(20) { db.execSQL("INSERT INTO notes(title,content,tags,createdAt,updatedAt) VALUES('uncommitted',?,'',1,1)", arrayOf("x".repeat(16384))) }
            }
            for (suffix in listOf("", "-wal", "-shm")) {
                val input = File(producer.path + suffix)
                if (input.exists()) input.copyTo(File(database.path + suffix))
            }
            if (uncommitted) db.endTransaction()
        }
    }
    @Test fun plaintextTwoWalToEncryptedSixPreservesIdentityAndSettings() {
        plaintextFixture()
        val original = listOf("", "-wal", "-shm").associateWith { PlaintextUpgradeFiles.hash(File(database.path + it)) }
        val preferenceName = "migration-fixture-${UUID.randomUUID()}"
        val preferences = context.getSharedPreferences(preferenceName, Context.MODE_PRIVATE)
        assertTrue(preferences.edit().putString("language", "hi-IN").putBoolean("enabled", false).commit())
        val prefsFile = File(context.applicationInfo.dataDir, "shared_prefs/$preferenceName.xml")
        val prefsHash = PlaintextUpgradeFiles.hash(prefsFile.canonicalFile)
        try {
            val migration = upgrade()
            migration.run(true)
            assertEquals(PlaintextUpgradeFiles.Phase.DONE, migration.files.read().phase)
            original.forEach { (suffix, hash) -> assertEquals(hash, PlaintextUpgradeFiles.hash(File(migration.files.backup.path + suffix))) }
            assertFalse(PlaintextUpgradeFiles.isPlaintext(database))
            encrypted().use { db ->
                assertEquals(6, db.version)
                for (table in listOf("notes", "memories", "skills", "action_logs", "model_metadata")) {
                    db.rawQuery("SELECT count(*) FROM $table", null).use { assertTrue(it.moveToFirst()); assertEquals(1, it.getInt(0)) }
                }
                db.rawQuery("SELECT id,title,content,createdAt,reminderTime,vaultRecordId FROM notes", null).use {
                    assertTrue(it.moveToFirst()); assertEquals(7, it.getInt(0)); assertEquals("O'Brien; DROP TABLE notes;--", it.getString(1))
                    assertEquals("Unicode हिन्दी\u0000tail", it.getString(2)); assertEquals(Long.MAX_VALUE - 2, it.getLong(3)); assertTrue(it.isNull(4)); assertTrue(it.isNull(5))
                }
                db.rawQuery("SELECT enabled,vaultRevision FROM skills", null).use { assertTrue(it.moveToFirst()); assertEquals(0, it.getInt(0)); assertEquals(1, it.getInt(1)) }
                db.rawQuery("SELECT seq FROM sqlite_sequence WHERE name='notes'", null).use { assertTrue(it.moveToFirst()); assertEquals(99, it.getInt(0)) }
            }
            assertEquals(prefsHash, PlaintextUpgradeFiles.hash(prefsFile.canonicalFile))
            assertEquals("hi-IN", preferences.getString("language", null)); assertFalse(preferences.getBoolean("enabled", true))
            val completedHash = PlaintextUpgradeFiles.hash(database)
            migration.run(false) // completed journal never converts/rewrites again
            assertEquals(completedHash, PlaintextUpgradeFiles.hash(database))
        } finally { context.deleteSharedPreferences(preferenceName) }
    }
    @Test fun uncommittedWalFramesAreNotInventedAsCommittedRows() {
        plaintextFixture(uncommitted = true)
        val before = PlaintextUpgradeFiles.hash(File(database.path + "-wal"))
        val migration = upgrade(); migration.run(true)
        assertEquals(before, PlaintextUpgradeFiles.hash(File(migration.files.backup.path + "-wal")))
        encrypted().use { db -> db.rawQuery("SELECT count(*) FROM notes", null).use { assertTrue(it.moveToFirst()); assertEquals(1, it.getInt(0)) } }
    }
    @Test fun cleanupRefusedUntilStoreAndWrappedKeyLineageCanBeProven() {
        plaintextFixture(); val migration = upgrade(); migration.run(true)
        val liveHash = PlaintextUpgradeFiles.hash(database); val keyHash = PlaintextUpgradeFiles.hash(wrapped)
        refusal { migration.removeRecoveryCopies(false) }
        refusal { migration.removeRecoveryCopies(true) }
        assertEquals(liveHash, PlaintextUpgradeFiles.hash(database)); assertEquals(keyHash, PlaintextUpgradeFiles.hash(wrapped))
        assertTrue(migration.files.backup.exists()); assertTrue(migration.files.snapshot.exists())
        assertEquals(PlaintextUpgradeFiles.Phase.DONE, migration.files.read().phase)
        migration.run(true); assertEquals(liveHash, PlaintextUpgradeFiles.hash(database))
    }
    @Test fun plaintextOneUsesFullRoomMigrationChain() {
        plaintextFixture(1); upgrade().run(true)
        encrypted().use { assertEquals(6, it.version) }
    }
    @Test fun resumeEachCrashPhaseRetainsOriginalAndCommittedWal() {
        for (phase in listOf("directory-created", "journal-synced:PREPARED", "prepared", "snapshot", "exported", "room6", "validated", "verified", "backup0", "backup1", "backup2", "promoted", "done")) {
            directory.listFiles()!!.forEach { it.deleteRecursively() }
            plaintextFixture()
            val originalHash = PlaintextUpgradeFiles.hash(database)
            refusal { upgrade { if (it == phase) throw IllegalStateException("test crash") }.run(true) }
            val recovered = upgrade()
            recovered.run(true)
            assertEquals(originalHash, PlaintextUpgradeFiles.hash(recovered.files.backup))
            encrypted().use { db -> db.rawQuery("SELECT count(*) FROM notes", null).use { assertTrue(it.moveToFirst()); assertEquals(1, it.getInt(0)) } }
        }
    }
    @Test fun wrongKeyAfterVerifiedNeverPromotesOrDeletes() {
        plaintextFixture()
        refusal { upgrade { if (it == "verified") throw IllegalStateException("crash") }.run(true) }
        val before = PlaintextUpgradeFiles.hash(database)
        val encryptedBefore = PlaintextUpgradeFiles.hash(upgrade().files.candidate)
        wrapped.writeBytes(cipher.encrypt(ByteArray(32) { 99 }))
        refusal { upgrade().run(true) }
        assertEquals(before, PlaintextUpgradeFiles.hash(database))
        assertEquals(encryptedBefore, PlaintextUpgradeFiles.hash(upgrade().files.candidate))
        assertFalse(upgrade().files.backup.exists())
    }
    @Test fun unavailableWrappedKeyAndDiskFaultRetainOriginal() {
        plaintextFixture()
        refusal { upgrade { if (it == "snapshot") throw java.io.IOException("simulated disk full") }.run(true) }
        val before = PlaintextUpgradeFiles.hash(database)
        wrapped.writeBytes(byteArrayOf(0, 1))
        refusal { upgrade().run(true) }
        assertEquals(before, PlaintextUpgradeFiles.hash(database))
        assertArrayEquals(byteArrayOf(0, 1), wrapped.readBytes())
    }
    @Test fun unknownSchemaNeverCreatesAKey() {
        plaintextFixture()
        SQLiteDatabase.openDatabase(database.path, null, SQLiteDatabase.OPEN_READWRITE).use { it.version = 99 }
        val before = PlaintextUpgradeFiles.hash(database)
        refusal { upgrade().run(true) }
        assertFalse(wrapped.exists()); assertEquals(before, PlaintextUpgradeFiles.hash(database))
    }
    @Test fun encryptedSixExistingPathIsNotAPlaintextConversion() {
        plaintextFixture(); upgrade().run(true)
        val before = PlaintextUpgradeFiles.hash(database)
        DatabaseOpenPolicy.requireSafeOpen(database, wrapped)
        encrypted().use { assertEquals(6, it.version) }
        upgrade().run(false)
        assertEquals(before, PlaintextUpgradeFiles.hash(database))
    }
    @Test fun corruptionHandlerNeverDeletesEncryptedFile() {
        plaintextFixture(); upgrade().run(true)
        val bytes = database.readBytes(); bytes[500] = (bytes[500].toInt() xor 127).toByte(); database.writeBytes(bytes)
        val hash = PlaintextUpgradeFiles.hash(database)
        val key = CacheKeyManager(cipher, wrapped).getOrCreate().passphrase
        val config = androidx.sqlite.db.SupportSQLiteOpenHelper.Configuration.builder(context)
            .name(database.absolutePath).callback(object : androidx.sqlite.db.SupportSQLiteOpenHelper.Callback(6) {
                override fun onCreate(db: androidx.sqlite.db.SupportSQLiteDatabase) = error("must not create")
                override fun onUpgrade(db: androidx.sqlite.db.SupportSQLiteDatabase, oldVersion: Int, newVersion: Int) = error("must not migrate")
            }).build()
        val helper = RecoveryOpenHelperFactory(key).create(config)
        try { helper.writableDatabase.query("PRAGMA integrity_check").use { while (it.moveToNext()) { } } } catch (_: Exception) { }
        finally { helper.close() }
        assertEquals(hash, PlaintextUpgradeFiles.hash(database))
    }
    @Test fun interruptedWrappedKeyIsAdoptedAndAuthenticatedAgainstEncryptedSix() {
        plaintextFixture(); upgrade().run(true)
        val keyBytes = wrapped.readBytes()
        val keyHash = PlaintextUpgradeFiles.hash(wrapped)
        assertTrue(wrapped.renameTo(File(wrapped.path + ".tmp")))
        DatabaseOpenPolicy.requireSafeOpen(database, wrapped)
        val key = CacheKeyManager(cipher, wrapped).getOrCreate().passphrase
        try {
            RecoveryOpenHelperFactory.requireAuthenticatedExisting(database, key, File(directory, "admission"))
            encrypted(key).use { assertEquals(6, it.version) }
        } finally { key.fill(0) }
        assertArrayEquals(keyBytes, wrapped.readBytes()); assertEquals(keyHash, PlaintextUpgradeFiles.hash(wrapped))
    }
    @Test fun plaintextPreparedResumesWithAuthenticatedTemporaryKeyWithoutEncryptionCall() {
        plaintextFixture()
        val expectedKey = ByteArray(32) { 53 }
        val blob = cipher.encrypt(expectedKey)
        File(wrapped.path + ".tmp").writeBytes(blob)
        val noNewKeys = object : PassphraseCipher {
            override fun encrypt(plaintext: ByteArray): ByteArray = error("must adopt interrupted key")
            override fun decrypt(blob: ByteArray) = cipher.decrypt(blob)
        }
        val migration = PlaintextDatabaseUpgrade(context, database, File(directory, "recovery"), wrapped,
            CacheKeyManager(noNewKeys, wrapped, syncDirectory = DatabaseDirectorySync::sync))
        migration.run(true)
        assertArrayEquals(blob, wrapped.readBytes())
        encrypted(expectedKey).use { assertEquals(6, it.version) }
        assertTrue(migration.files.backup.exists())
    }
    @Test fun validDifferentStoreKeyCannotAuthorizeDeletingPriorMigrationOriginals() {
        plaintextFixture(); val migration = upgrade(); migration.run(true)
        val backup = PlaintextUpgradeFiles.hash(migration.files.backup)
        val differentKey = ByteArray(32) { 71 }
        val previousKey = CacheKeyManager(cipher, wrapped).getOrCreate().passphrase
        CipherDatabase.openDatabase(database.path, previousKey, null,
            CipherDatabase.OPEN_READWRITE or CipherDatabase.NO_LOCALIZED_COLLATORS,
            DatabaseErrorHandler { _, _ -> error("test corruption") }, null).use { db ->
            db.execSQL("DELETE FROM notes")
            db.changePassword(differentKey)
        }
        wrapped.writeBytes(cipher.encrypt(differentKey))
        encrypted(differentKey).use { assertEquals(6, it.version) }
        refusal { migration.removeRecoveryCopies(true) }
        assertEquals(backup, PlaintextUpgradeFiles.hash(migration.files.backup))
        assertTrue(migration.files.snapshot.exists())
    }

    @Test fun localDataIncludingOldAuditAndMirroredRowsSurvivesExpiryAndDetach() {
        plaintextFixture(); upgrade().run(true)
        val key = CacheKeyManager(cipher, wrapped).getOrCreate().passphrase
        val room = androidx.room.Room.databaseBuilder(context, com.unoone.agent.storage.db.UnoOneDatabase::class.java, database.absolutePath)
            .openHelperFactory(RecoveryOpenHelperFactory(key))
            .setJournalMode(androidx.room.RoomDatabase.JournalMode.TRUNCATE).build()
        try {
            room.openHelper.writableDatabase.execSQL("UPDATE notes SET vaultRecordId='legacy-note'")
            room.openHelper.writableDatabase.execSQL("INSERT INTO notes(title,content,tags,createdAt,updatedAt,vaultRevision) VALUES('unsynced','local','',1,1,1)")
            kotlinx.coroutines.runBlocking {
                assertEquals(0, com.unoone.agent.storage.cache.VaultCacheLifecycle.evictExpired(room, nowMillis = Long.MAX_VALUE))
                assertEquals(0, com.unoone.agent.storage.cache.VaultCacheLifecycle.clearOnVaultDisconnect(room))
            }
            room.openHelper.readableDatabase.query("SELECT count(*) FROM notes").use { assertTrue(it.moveToFirst()); assertEquals(2, it.getInt(0)) }
            room.openHelper.readableDatabase.query("SELECT count(*) FROM action_logs").use { assertTrue(it.moveToFirst()); assertEquals(1, it.getInt(0)) }
        } finally { room.close(); key.fill(0) }
    }

    private fun openWritable(file: File, key: ByteArray): CipherDatabase = CipherDatabase.openDatabase(
        file.path, key, null, CipherDatabase.OPEN_READWRITE or CipherDatabase.NO_LOCALIZED_COLLATORS,
        DatabaseErrorHandler { _, _ -> throw IllegalStateException("test corruption") }, null)

    private fun finalHelperCount(key: ByteArray): Int {
        val config = androidx.sqlite.db.SupportSQLiteOpenHelper.Configuration.builder(context)
            .name(database.absolutePath).callback(object : androidx.sqlite.db.SupportSQLiteOpenHelper.Callback(6) {
                override fun onCreate(db: androidx.sqlite.db.SupportSQLiteDatabase) = error("must not create")
                override fun onUpgrade(db: androidx.sqlite.db.SupportSQLiteDatabase, oldVersion: Int, newVersion: Int) = error("must not migrate")
            }).build()
        val helper = RecoveryOpenHelperFactory(key).create(config)
        try { return helper.writableDatabase.query("SELECT count(*) FROM notes").use { assertTrue(it.moveToFirst()); it.getInt(0) } }
        finally { helper.close() }
    }

    @Test fun sqlCipher417HotRollbackJournalNeedsWritableProbeAndPreservesOriginalSet() {
        plaintextFixture(); upgrade().run(true)
        val key = CacheKeyManager(cipher, wrapped).getOrCreate().passphrase
        val producer = File(directory, "encrypted-producer")
        database.copyTo(producer)
        openWritable(producer, key).use { db ->
            db.rawQuery("PRAGMA journal_mode=DELETE", null).use { assertTrue(it.moveToFirst()); assertEquals("delete", it.getString(0)) }
            db.execSQL("PRAGMA synchronous=FULL")
            db.execSQL("PRAGMA cache_size=1")
            db.execSQL("PRAGMA cache_spill=ON")
            db.beginTransaction()
            try {
                repeat(128) { db.execSQL("INSERT INTO notes(title,content,tags,createdAt,updatedAt,vaultRevision) VALUES('uncommitted',?,'',1,1,1)", arrayOf("x".repeat(16384))) }
                val journal = File(producer.path + "-journal")
                assertTrue(journal.length() > 512)
                val magic = journal.inputStream().use { input -> ByteArray(8).also { assertEquals(8, input.read(it)) } }
                assertArrayEquals(byteArrayOf(0xd9.toByte(),0xd5.toByte(),5,0xf9.toByte(),0x20,0xa1.toByte(),0x63,0xd7.toByte()), magic)
                producer.copyTo(database, overwrite = true)
                journal.copyTo(File(database.path + "-journal"), overwrite = true)
            } finally { db.endTransaction() } // rollback only PRODUCER; copied hot state remains
        }
        val original = listOf("", "-journal").associateWith { PlaintextUpgradeFiles.hash(File(database.path + it)) }
        var readonlyFailed = false
        try { encrypted(key).use { it.rawQuery("SELECT count(*) FROM notes", null).use { row -> row.moveToFirst() } } }
        catch (_: Exception) { readonlyFailed = true }
        assertTrue("Real 4.17 read-only open must not silently recover this hot journal", readonlyFailed)
        RecoveryOpenHelperFactory.requireAuthenticatedExisting(database, key, File(directory, "admission"))
        original.forEach { (suffix, hash) -> assertEquals(hash, PlaintextUpgradeFiles.hash(File(database.path + suffix))) }
        val retained = File(directory, "admission").listFiles()!!.single()
        original.forEach { (suffix, hash) -> assertEquals(hash, PlaintextUpgradeFiles.hash(File(retained, "original$suffix"))) }
        assertEquals(1, finalHelperCount(key)) // native rollback removes all uncommitted rows
        original.forEach { (suffix, hash) -> assertEquals(hash, PlaintextUpgradeFiles.hash(File(retained, "original$suffix"))) }
        key.fill(0)
    }

    @Test fun encryptedWalWithoutShmRecoversCommittedRowsThroughPreservedProbe() {
        plaintextFixture(); upgrade().run(true)
        val key = CacheKeyManager(cipher, wrapped).getOrCreate().passphrase
        val producer = File(directory, "encrypted-wal-producer")
        database.copyTo(producer)
        openWritable(producer, key).use { db ->
            assertTrue(db.enableWriteAheadLogging())
            db.rawQuery("PRAGMA wal_autocheckpoint=0", null).use { assertTrue(it.moveToFirst()) }
            db.execSQL("INSERT INTO notes(title,content,tags,createdAt,updatedAt,vaultRevision) VALUES('committed','wal','',1,1,1)")
            assertTrue(File(producer.path + "-wal").length() > 0)
            producer.copyTo(database, overwrite = true)
            File(producer.path + "-wal").copyTo(File(database.path + "-wal"), overwrite = true)
        }
        assertFalse(File(database.path + "-shm").exists())
        val original = listOf("", "-wal").associateWith { PlaintextUpgradeFiles.hash(File(database.path + it)) }
        RecoveryOpenHelperFactory.requireAuthenticatedExisting(database, key, File(directory, "admission"))
        assertFalse(File(database.path + "-shm").exists())
        original.forEach { (suffix, hash) -> assertEquals(hash, PlaintextUpgradeFiles.hash(File(database.path + suffix))) }
        assertEquals(2, finalHelperCount(key))
        key.fill(0)
    }

    @Test fun wrongKeyWithSidecarsLeavesLiveAndPreservedBytesUntouched() {
        plaintextFixture(); upgrade().run(true)
        File(database.path + "-journal").writeBytes(ByteArray(16))
        val before = PlaintextUpgradeFiles.hash(database)
        var failed = false
        try { RecoveryOpenHelperFactory.requireAuthenticatedExisting(database, ByteArray(32) { 88 }, File(directory, "admission")) }
        catch (_: Exception) { failed = true }
        assertTrue(failed); assertEquals(before, PlaintextUpgradeFiles.hash(database))
        assertEquals(before, PlaintextUpgradeFiles.hash(File(File(directory, "admission").listFiles()!!.single(), "original")))
    }

}
