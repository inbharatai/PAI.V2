package com.unoone.agent.storage.cache

import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.nio.file.Files

class PlaintextUpgradeFilesTest {
    @get:Rule val temporary = TemporaryFolder()
    private val header = "SQLite format 3\u0000".toByteArray(Charsets.US_ASCII)
    private fun fixture(label: String = "case", fault: (String) -> Unit = {}): PlaintextUpgradeFiles {
        val parent = temporary.newFolder(label)
        val database = File(parent, "db").apply { writeBytes(header + ByteArray(4096)) }
        File(database.path + "-wal").writeBytes(byteArrayOf(1, 2, 3, 4))
        File(database.path + "-shm").writeBytes(byteArrayOf(5, 6))
        File(database.path + "-journal").writeBytes(byteArrayOf(7, 8))
        return PlaintextUpgradeFiles(database, File(parent, "recovery"), {}, fault)
    }
    private fun expectFailure(block: () -> Unit) {
        try { block(); fail("must refuse") } catch (_: IllegalArgumentException) { }
    }
    private fun build(files: PlaintextUpgradeFiles): PlaintextUpgradeFiles.Journal {
        val state = files.prepare(true)
        files.copySnapshot(state)
        files.candidate.writeBytes(ByteArray(8192) { 0x5a.toByte() })
        return files.verified(state)
    }
    @Test fun consentDeclinedWritesNothing() {
        val files = fixture()
        val before = files.database.readBytes()
        expectFailure { files.prepare(false) }
        assertFalse(files.root.exists())
        assertArrayEquals(before, files.database.readBytes())
    }
    @Test fun snapshotPreservesAllOriginalTransactionBytes() {
        val files = fixture()
        val state = files.prepare(true)
        files.copySnapshot(state)
        for (suffix in listOf("", "-wal", "-shm", "-journal")) {
            assertArrayEquals(File(files.database.path + suffix).readBytes(), File(files.snapshot.path + suffix).readBytes())
        }
        files.requireOriginals(state)
        assertEquals(PlaintextUpgradeFiles.Phase.PREPARED, files.read().phase)
    }
    @Test fun changedWalRefusesBeforeRemovingOriginals() {
        val files = fixture()
        val state = build(files)
        File(files.database.path + "-wal").appendBytes(byteArrayOf(10))
        expectFailure { files.promote(state) }
        assertTrue(PlaintextUpgradeFiles.isPlaintext(files.database))
        assertFalse(files.backup.exists())
    }
    @Test fun allPromotionCrashPointsResumeWithoutLosingOnlyCopy() {
        for (phase in listOf("backup0", "backup1", "backup2", "backup3", "promoted", "done")) {
            var armed = false
            val files = fixture(phase) { if (armed && it == phase) throw IllegalStateException("simulated process death") }
            val state = build(files)
            armed = true
            try { files.promote(state); fail("fault expected") } catch (_: IllegalStateException) { }
            val recovered = PlaintextUpgradeFiles(files.database, files.root, {})
            val journal = recovered.read()
            if (journal.phase != PlaintextUpgradeFiles.Phase.DONE) recovered.promote(journal)
            recovered.requireBackup(recovered.read())
            assertEquals(state.candidateHash, PlaintextUpgradeFiles.hash(recovered.database))
            assertEquals(PlaintextUpgradeFiles.Phase.DONE, recovered.read().phase)
            expectFailure { recovered.prepare(true) }
        }
    }
    @Test fun tamperedCandidateAndDestinationCollisionFailClosed() {
        val files = fixture()
        val state = build(files)
        files.candidate.appendBytes(byteArrayOf(0))
        expectFailure { files.promote(state) }
        files.requireOriginals(state)
        assertFalse(files.backup.exists())
    }
    @Test fun candidateWithWalNeverPromoted() {
        val files = fixture()
        val state = files.prepare(true)
        files.candidate.writeBytes(ByteArray(8192) { 1 })
        File(files.candidate.path + "-wal").writeBytes(byteArrayOf(1))
        expectFailure { files.verified(state) }
        files.requireOriginals(state)
    }
    @Test fun unknownJournalAndSymlinksNeverFollowed() {
        val files = fixture()
        files.prepare(true)
        val journal = File(files.root, "state")
        journal.writeText("PAI-UPGRADE-99\n")
        expectFailure { files.read() }
        val target = temporary.newFile("precious").apply { writeBytes(header + ByteArray(4096)) }
        files.database.delete()
        Files.createSymbolicLink(files.database.toPath(), target.toPath())
        expectFailure { PlaintextUpgradeFiles.hash(files.database) }
        assertEquals(4112, target.length().toInt())
    }
    @Test fun syncFailureNeverDeletesOriginalData() {
        val fixture = fixture()
        val before = fixture.database.readBytes()
        val files = PlaintextUpgradeFiles(fixture.database, fixture.root, { throw java.io.IOException("disk full") })
        try { files.prepare(true); fail() } catch (_: java.io.IOException) { }
        assertArrayEquals(before, fixture.database.readBytes())
        assertTrue(File(fixture.database.path + "-wal").exists())
    }
    @Test fun interruptedScratchCanBeRebuiltOnlyWithIntactOriginals() {
        val files = fixture()
        val state = files.prepare(true)
        files.snapshot.writeText("incomplete")
        files.candidate.writeText("incomplete encrypted work")
        files.copySnapshot(state)
        assertFalse(files.candidate.exists())
        files.requireOriginals(state)
    }
    @Test fun unknownOrTruncatedHeaderRefused() {
        val files = fixture()
        files.database.writeBytes(byteArrayOf(0, 1))
        expectFailure { files.prepare(true) }
        assertFalse(files.root.exists())
    }
    @Test fun cleanupRefusesEvenAuthenticatedSubstitutedStoreAndKeepsSoleOriginals() {
        val files = fixture()
        files.promote(build(files))
        val backup = PlaintextUpgradeFiles.hash(files.backup)
        files.database.writeBytes(ByteArray(8192) { 42 }) // another valid store could have different bytes
        expectFailure { files.cleanup(false) {} }
        try { files.cleanup(true) { fail("Authentication is not lineage") }; fail() }
        catch (_: IllegalStateException) { }
        assertEquals(backup, PlaintextUpgradeFiles.hash(files.backup))
        assertTrue(files.snapshot.exists())
        assertEquals(PlaintextUpgradeFiles.Phase.DONE, files.read().phase)
    }
    @Test fun oldCleaningJournalNeverContinuesDeleting() {
        val files = fixture()
        files.promote(build(files))
        val state = File(files.root, "state")
        state.writeText(state.readText().replace("\nDONE\n", "\nCLEANING\n"))
        val before = files.root.listFiles()!!.associate { it.name to it.readBytes().toList() }
        try { files.cleanup(true) {}; fail() } catch (_: IllegalStateException) { }
        assertEquals(before, files.root.listFiles()!!.associate { it.name to it.readBytes().toList() })
    }
    @Test fun initialDirectoryAndSyncedJournalCrashResumeOnlyAfterConsent() {
        for (phase in listOf("directory-created", "journal-synced:PREPARED")) {
            val files = fixture(phase.replace(':', '-')) { if (it == phase) throw IllegalStateException("crash") }
            try { files.prepare(true); fail() } catch (_: IllegalStateException) { }
            assertFalse(File(files.root, "state").exists())
            val resumed = PlaintextUpgradeFiles(files.database, files.root, {})
            val state = resumed.read()
            assertEquals(PlaintextUpgradeFiles.Phase.PREPARED, state.phase)
            assertFalse(File(files.root, "state").exists()) // inspection is non-mutating
            expectFailure { resumed.resumeInitialization(false) }
            resumed.resumeInitialization(true)
            assertTrue(File(files.root, "state").isFile)
            resumed.requireOriginals(state)
            resumed.copySnapshot(state)
        }
    }
    @Test fun lostStateWithScratchOrChangedSourceRefusesInitialization() {
        val files = fixture()
        files.prepare(true)
        val state = File(files.root, "state")
        assertTrue(state.renameTo(File(files.root, "state.next")))
        File(files.database.path + "-wal").appendText("changed")
        expectFailure { files.read() }
        files.snapshot.writeText("do not delete")
        expectFailure { files.resumeInitialization(true) }
        assertEquals("do not delete", files.snapshot.readText())
    }
    @Test fun emptyInitializationDirectoryDoesNotAuthorizeEncryptedSource() {
        val files = fixture()
        files.root.mkdir()
        files.database.writeBytes(ByteArray(8192) { 42 })
        expectFailure { files.resumeInitialization(true) }
        assertTrue(files.root.list()!!.isEmpty())
    }
    @Test fun danglingScratchSymlinkAndOversizedInputRefused() {
        val files = fixture()
        val state = files.prepare(true)
        Files.createSymbolicLink(files.snapshot.toPath(), File(temporary.root, "absent-victim").toPath())
        expectFailure { files.copySnapshot(state) }
        assertFalse(File(temporary.root, "absent-victim").exists())
        files.snapshot.delete()
        java.io.RandomAccessFile(files.database, "rw").use { it.setLength(PlaintextUpgradeFiles.MAX_SOURCE_BYTES + 1) }
        expectFailure { files.requireOriginals(state) }
    }
    @Test fun orphanJournalTempIsNotMistakenForAuthorization() {
        val files = fixture()
        files.root.mkdir()
        File(files.root, "state.next").writeText("partial")
        expectFailure { files.read() }
        assertTrue(PlaintextUpgradeFiles.isPlaintext(files.database))
    }
}
