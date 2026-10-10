package com.unoone.agent.storage.cache

import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class DatabaseOpenPolicyTest {
    @get:Rule val tmp = TemporaryFolder()
    private val db get() = File(tmp.root, "unoone_database")
    private val key get() = File(tmp.root, "cache_db_key.wrapped")

    private fun refusesWithoutMutation() {
        val before = tmp.root.listFiles()!!.associate { it.name to it.readBytes().toList() }
        try { DatabaseOpenPolicy.requireSafeOpen(db, key); fail("Must require recovery") }
        catch (_: DatabaseRecoveryRequired) { }
        assertEquals(before, tmp.root.listFiles()!!.associate { it.name to it.readBytes().toList() })
    }
    @Test fun freshInstallCanCreateEncryptedDatabase() {
        DatabaseOpenPolicy.requireSafeOpen(db, key)
        assertFalse(db.exists()); assertFalse(key.exists())
    }
    @Test fun plaintextStandaloneDatabaseAndUnsyncedWalArePreserved() {
        db.writeBytes("SQLite format 3\u0000".toByteArray() + ByteArray(100))
        File(db.path + "-wal").writeText("only copy of unsynced local rows")
        key.writeText("existing-key-must-not-be-replaced")
        refusesWithoutMutation()
    }
    @Test fun encryptedFileWithoutKeyIsNotReinitialized() {
        db.writeBytes(ByteArray(4096) { 71 })
        refusesWithoutMutation()
    }
    @Test fun orphanWalIsNotMistakenForFreshInstall() {
        File(db.path + "-wal").writeText("recoverable journal")
        refusesWithoutMutation()
    }
    @Test fun emptyExistingFileIsNotMistakenForFreshInstall() {
        db.createNewFile(); key.writeText("wrapped")
        refusesWithoutMutation()
    }
    @Test fun interruptedKeyPresencePermitsAuthenticationButDoesNotInventKey() {
        db.writeBytes(ByteArray(4096) { 71 })
        val pending = File(key.path + ".tmp").apply { writeText("needs AEAD verification") }
        DatabaseOpenPolicy.requireSafeOpen(db, key)
        assertFalse(key.exists()); assertEquals("needs AEAD verification", pending.readText())
    }
    @Test fun danglingDatabaseLinkIsNotFreshInstall() {
        val target = File(tmp.root, "absent")
        java.nio.file.Files.createSymbolicLink(db.toPath(), target.toPath())
        try { DatabaseOpenPolicy.requireSafeOpen(db, key); fail() } catch (_: DatabaseRecoveryRequired) { }
        assertFalse(target.exists()); assertFalse(key.exists())
        assertTrue(java.nio.file.Files.isSymbolicLink(db.toPath()))
    }
    @Test fun ciphertextWithKeyIsOnlyAdmittedForSubsequentAuthenticatedOpen() {
        db.writeBytes(ByteArray(4096) { 71 }); key.writeText("wrapped")
        DatabaseOpenPolicy.requireSafeOpen(db, key)
        assertEquals(4096, db.length().toInt())
        // This is admission, NOT SQLCipher authentication or a device migration pass.
    }
}
