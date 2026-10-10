package com.unoone.agent.storage.cache

import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.nio.file.Files

/** Real filesystem preservation tests, not a native-SQLCipher authentication claim. */
class EncryptedRecoveryCopyTest {
    @get:Rule val temporary = TemporaryFolder()
    private val suffixes = listOf("", "-wal", "-shm", "-journal")
    private fun database(): File = File(temporary.root, "db").also { db ->
        suffixes.forEachIndexed { index, suffix -> File(db.path + suffix).writeBytes(ByteArray(4096) { (index + 11).toByte() }) }
    }
    @Test fun writableProbeCannotMutateRetainedTransactionSet() {
        val db = database()
        val copy = EncryptedRecoveryCopy(db, File(temporary.root, "retained"), {})
        val snapshot = copy.create()
        for (suffix in suffixes) {
            assertArrayEquals(File(db.path + suffix).readBytes(), File(snapshot.retained.path + suffix).readBytes())
            File(snapshot.probe.path + suffix).writeText("simulated recovery modifies scratch")
        }
        copy.requireUnchanged(snapshot)
    }
    @Test fun sourceChangesOrTamperedRetainedCopyFailClosed() {
        val db = database()
        val copy = EncryptedRecoveryCopy(db, File(temporary.root, "retained"), {})
        val snapshot = copy.create()
        File(db.path + "-wal").appendText("new commit")
        try { copy.requireUnchanged(snapshot); fail() } catch (_: IllegalArgumentException) { }
        assertTrue(snapshot.retained.exists()); assertTrue(File(snapshot.retained.path + "-wal").exists())
    }
    @Test fun unsafeSidecarAndRecoveryRootNeverFollowed() {
        val db = database()
        val wal = File(db.path + "-wal")
        assertTrue(wal.delete())
        Files.createSymbolicLink(wal.toPath(), File(temporary.root, "absent").toPath())
        val root = File(temporary.root, "retained")
        try { EncryptedRecoveryCopy(db, root, {}).create(); fail() } catch (_: IllegalArgumentException) { }
        assertFalse(root.exists()); assertFalse(File(temporary.root, "absent").exists())
    }
    @Test fun fsyncFailureLeavesSourceByteIdentical() {
        val db = database()
        val before = suffixes.map { PlaintextUpgradeFiles.hash(File(db.path + it)) }
        try { EncryptedRecoveryCopy(db, File(temporary.root, "retained"), { throw java.io.IOException("disk full") }).create(); fail() }
        catch (_: java.io.IOException) { }
        assertEquals(before, suffixes.map { PlaintextUpgradeFiles.hash(File(db.path + it)) })
    }
    @Test fun independentStoreNeverExpiresOnStartupOrDetach() {
        LocalStoreRetentionPolicy.Event.values().forEach { assertFalse(LocalStoreRetentionPolicy.mayAutomaticallyDelete(it)) }
    }
}
