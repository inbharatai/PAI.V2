package com.unoone.agent.task

import android.util.AtomicFile
import com.unoone.agent.core.task.*
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.nio.file.Files

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class TaskJournalIntegrationTest {
    @Test fun staleRunningAndCancellingCannotOverwriteTerminalActionEvidence() {
        val dir = Files.createTempDirectory("journal-action-retention").toFile()
        try {
            val file = java.io.File(dir, "journal")
            val first = TaskJournalStore(AtomicFile(file))
            val staleCollector = TaskJournalStore(AtomicFile(file))
            val id = TaskId("action")
            first.recordSummary(TaskSummary(id, null, TaskSource.NATIVE,
                TaskState.ACTION_VERIFIED, 1, TaskOutcome.ACTION_VERIFIED))
            val terminalBytes = file.readBytes()
            for (state in listOf(TaskState.RUNNING, TaskState.CANCELLING)) {
                staleCollector.recordSummary(TaskSummary(id, null, TaskSource.NATIVE, state, 0))
                assertArrayEquals(terminalBytes, file.readBytes())
            }
            val recovered = TaskJournalStore(AtomicFile(file)).recoveredTasks.single()
            assertEquals(TaskState.ACTION_VERIFIED, recovered.state)
            assertEquals(TaskOutcome.ACTION_VERIFIED, recovered.outcome)
        } finally { dir.deleteRecursively() }
    }

    @Test fun unknownPersistedEnumRefusesRecoveryWithoutDeletingOrReplaying() {
        val dir = Files.createTempDirectory("journal-enum-retention").toFile()
        try {
            val file = java.io.File(dir, "journal")
            TaskJournalStore(AtomicFile(file)).recordSummary(TaskSummary(TaskId("future"), null,
                TaskSource.NATIVE, TaskState.RUNNING, 1))
            val row = TaskJournalStore.decode(file.readBytes()).single()
            row.put("state", "UNKNOWN_OLD_OR_FUTURE_STATE")
            val canonical = row.keys().asSequence().filter { it != "checksum" }.sorted()
                .joinToString("\n") { key -> key + "=" + JSONObject.quote(row.get(key).toString()) }
            row.put("checksum", java.security.MessageDigest.getInstance("SHA-256")
                .digest(canonical.toByteArray()).joinToString("") { "%02x".format(it.toInt() and 255) })
            file.writeText(JSONArray(listOf(row)).toString())
            val original = file.readBytes()
            val recovered = TaskJournalStore(AtomicFile(file))
            assertEquals("RECOVERY_FAILED", recovered.health)
            assertTrue(recovered.recoveredTasks.isEmpty())
            try {
                recovered.record(TaskId("not-authorized"), ReceiptStage.DISPATCH_INTENT, 1)
                fail("Unknown enums must not authorize effects")
            } catch (_: IllegalStateException) { }
            assertArrayEquals(original, file.readBytes())
        } finally { dir.deleteRecursively() }
    }
}
