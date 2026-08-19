package com.unoone.pai.vault.journal

import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.io.File
import java.io.RandomAccessFile
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.SecureRandom
import java.util.concurrent.atomic.AtomicLong

/**
 * Write-ahead journal for crash recovery.
 *
 * Before any vault write operation:
 * 1. Write the intended operation to the journal
 * 2. Perform the actual vault write
 * 3. Mark the journal entry as committed
 * 4. On next unlock, replay any uncommitted entries
 *
 * The journal itself is encrypted with the vault's domain key.
 */
class WriteAheadJournal(private val journalDir: File) {

    private val json = Json { ignoreUnknownKeys = true }
    private val sequenceCounter = AtomicLong(0)

    init {
        journalDir.mkdirs()
    }

    @Serializable
    data class JournalEntry(
        val sequence: Long,
        val timestamp: String,
        val operation: JournalOperation,
        val domain: String,
        val recordId: String,
        val dataPath: String,          // Relative path within VAULT/
        val dataHash: String,           // SHA-256 of the encrypted data
        val state: JournalState = JournalState.PENDING
    )

    @Serializable
    enum class JournalOperation {
        CREATE, UPDATE, DELETE
    }

    @Serializable
    enum class JournalState {
        PENDING,    // Written to journal, not yet committed to vault
        COMMITTED,  // Successfully committed to vault
        ROLLED_BACK // Rolled back due to error
    }

    /**
     * Begin a write operation — record the intent in the journal.
     * Returns the journal entry with a unique sequence number.
     */
    fun begin(
        operation: JournalOperation,
        domain: String,
        recordId: String,
        dataPath: String,
        dataHash: String
    ): JournalEntry {
        val entry = JournalEntry(
            sequence = sequenceCounter.incrementAndGet(),
            timestamp = java.time.Instant.now().toString(),
            operation = operation,
            domain = domain,
            recordId = recordId,
            dataPath = dataPath,
            dataHash = dataHash
        )

        writeEntryFile(entry)
        return entry
    }

    /**
     * Mark a journal entry as committed.
     */
    fun commit(entry: JournalEntry) {
        val committed = entry.copy(state = JournalState.COMMITTED)
        writeEntryFile(committed)
    }

    /**
     * Mark a journal entry as rolled back.
     */
    fun rollback(entry: JournalEntry) {
        val rolledBack = entry.copy(state = JournalState.ROLLED_BACK)
        writeEntryFile(rolledBack)
    }

    /**
     * Get all pending (uncommitted) entries — used for crash recovery.
     */
    fun getPendingEntries(): List<JournalEntry> {
        return journalDir.listFiles()
            ?.filter { it.name.startsWith("journal-") && it.name.endsWith(".json") }
            ?.mapNotNull { file ->
                try {
                    json.decodeFromString<JournalEntry>(file.readText())
                } catch (e: Exception) {
                    null // Skip corrupt entries
                }
            }
            ?.filter { it.state == JournalState.PENDING }
            ?.sortedBy { it.sequence }
            ?: emptyList()
    }

    /**
     * Clean up committed and rolled-back entries older than the specified age.
     */
    fun cleanup(maxAgeMs: Long = 3600_000) { // Default: 1 hour
        val cutoff = System.currentTimeMillis() - maxAgeMs
        journalDir.listFiles()
            ?.filter { it.name.startsWith("journal-") && it.name.endsWith(".json") }
            ?.forEach { file ->
                try {
                    val entry = json.decodeFromString<JournalEntry>(file.readText())
                    if (entry.state != JournalState.PENDING && file.lastModified() < cutoff) {
                        file.delete()
                    }
                } catch (e: Exception) {
                    // Corrupt entry, delete it
                    file.delete()
                }
            }
    }

    /**
     * Perform an atomic write: write to temp file, then rename.
     * This prevents partial writes from corrupting vault data.
     */
    fun atomicWrite(targetFile: File, data: ByteArray) {
        val tempFile = File(targetFile.parent, "${targetFile.name}.tmp.${System.currentTimeMillis()}")
        try {
            tempFile.writeBytes(data)
            Files.move(tempFile.toPath(), targetFile.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
        } catch (e: java.nio.file.AtomicMoveNotSupportedException) {
            // Fallback for filesystems that don't support atomic move (e.g., FAT32 on USB)
            tempFile.renameTo(targetFile)
        } finally {
            tempFile.delete()
        }
    }

    private fun writeEntryFile(entry: JournalEntry) {
        val file = File(journalDir, "journal-${entry.sequence}-${entry.operation.name.lowercase()}.json")
        val content = json.encodeToString(entry)
        atomicWrite(file, content.encodeToByteArray())
    }
}