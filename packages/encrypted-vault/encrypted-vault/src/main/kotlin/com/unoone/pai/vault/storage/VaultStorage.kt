package com.unoone.pai.vault.storage

import com.unoone.pai.vault.crypto.Argon2idKdf
import com.unoone.pai.vault.crypto.VaultCipher
import com.unoone.pai.vault.crypto.VaultCipher.Algorithm
import com.unoone.pai.vault.journal.WriteAheadJournal
import kotlinx.serialization.json.Json
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap

/**
 * Encrypted file storage for vault data.
 *
 * This is the low-level storage layer that:
 * 1. Encrypts all data before writing to disk
 * 2. Uses write-ahead journaling for crash recovery
 * 3. Performs atomic writes to prevent corruption
 * 4. Verifies integrity on reads
 * 5. Manages domain-specific encryption keys
 */
class VaultStorage(
    private val vaultRoot: File,
    private val journalDir: File
) {
    private val json = Json { ignoreUnknownKeys = true }
    private val journal = WriteAheadJournal(journalDir)
    private val domainKeys = ConcurrentHashMap<String, ByteArray>()

    /** The cipher algorithm to use (set during vault setup) */
    var cipherAlgorithm: Algorithm = Algorithm.XCHACHA20_POLY1305
        private set

    /** The master key (in memory only while vault is unlocked) */
    private var masterKey: ByteArray? = null

    /**
     * Set the master key (derived from password via Argon2id).
     * Called during vault unlock.
     */
    fun setMasterKey(key: ByteArray) {
        masterKey = key
    }

    /**
     * Set the cipher algorithm (from vault header).
     */
    fun setCipherAlgorithm(algorithm: Algorithm) {
        cipherAlgorithm = algorithm
    }

    /**
     * Get the domain-specific encryption key.
     *
     * Derives sub-keys from the master key using HKDF-SHA256 so that the same
     * domain on Android and Desktop (Rust vault-core) produces the exact same
     * key. This is required for cross-platform vault sync.
     *
     *   salt = "unoone-vault-domain"
     *   info = domain
     */
    private fun getDomainKey(domain: String): ByteArray {
        return domainKeys.getOrPut(domain) {
            val key = masterKey ?: throw IllegalStateException("Vault is locked — master key not available")
            Argon2idKdf.deriveDomainKey(key, domain)
        }
    }

    /**
     * Clear all keys from memory. Called during vault lock.
     */
    fun clearKeys() {
        masterKey?.fill(0)
        masterKey = null
        domainKeys.values.forEach { it.fill(0) }
        domainKeys.clear()
    }

    /**
     * Write an encrypted record to the vault.
     *
     * @param domain Vault domain (e.g., "memory/personal", "chats", "recordings/audio")
     * @param recordId Unique record ID
     * @param data Plaintext data to encrypt and store
     * @param aad Optional additional authenticated data
     */
    fun write(domain: String, recordId: String, data: ByteArray, aad: ByteArray? = null) {
        require(masterKey != null) { "Vault is locked" }

        val domainKey = getDomainKey(domain)
        val encrypted = VaultCipher.encrypt(cipherAlgorithm, domainKey, data, aad)

        // Write to journal first
        val relativePath = "$domain/$recordId.enc"
        val dataHash = sha256Hex(encrypted)
        val journalEntry = journal.begin(
            WriteAheadJournal.JournalOperation.CREATE,
            domain, recordId, relativePath, dataHash
        )

        try {
            // Perform atomic write
            val targetFile = File(vaultRoot, relativePath)
            targetFile.parentFile?.mkdirs()
            journal.atomicWrite(targetFile, encrypted)

            // Commit journal
            journal.commit(journalEntry)
        } catch (e: Exception) {
            journal.rollback(journalEntry)
            throw e
        }
    }

    /**
     * Read and decrypt a record from the vault.
     *
     * @param domain Vault domain
     * @param recordId Record ID
     * @param aad Must match the AAD used during encryption
     * @return Decrypted plaintext data
     * @throws SecurityException if authentication fails (tampered data)
     * @throws NoSuchElementException if the record doesn't exist
     */
    fun read(domain: String, recordId: String, aad: ByteArray? = null): ByteArray {
        require(masterKey != null) { "Vault is locked" }

        val domainKey = getDomainKey(domain)
        val file = File(vaultRoot, "$domain/$recordId.enc")
        if (!file.exists()) {
            throw NoSuchElementException("Record not found: $domain/$recordId")
        }

        val encrypted = file.readBytes()
        return VaultCipher.decrypt(cipherAlgorithm, domainKey, encrypted, aad)
    }

    /**
     * Delete a record from the vault (creates a tombstone).
     *
     * @param domain Vault domain
     * @param recordId Record ID
     */
    fun delete(domain: String, recordId: String) {
        require(masterKey != null) { "Vault is locked" }

        val journalEntry = journal.begin(
            WriteAheadJournal.JournalOperation.DELETE,
            domain, recordId, "$domain/$recordId.enc", ""
        )

        try {
            // Create tombstone
            val tombstone = File(vaultRoot, "$domain/$recordId.tombstone")
            tombstone.parentFile?.mkdirs()
            tombstone.writeText(java.time.Instant.now().toString())

            // Delete encrypted data file
            val encFile = File(vaultRoot, "$domain/$recordId.enc")
            encFile.delete()

            journal.commit(journalEntry)
        } catch (e: Exception) {
            journal.rollback(journalEntry)
            throw e
        }
    }

    /**
     * List all records in a domain.
     *
     * @param domain Vault domain
     * @return List of record IDs (excluding tombstones)
     */
    fun list(domain: String): List<String> {
        val dir = File(vaultRoot, domain)
        if (!dir.exists()) return emptyList()

        val tombstones = dir.listFiles()
            ?.filter { it.name.endsWith(".tombstone") }
            ?.map { it.name.removeSuffix(".tombstone") }
            ?.toSet() ?: emptySet()

        return dir.listFiles()
            ?.filter { it.name.endsWith(".enc") }
            ?.map { it.name.removeSuffix(".enc") }
            ?.filter { it !in tombstones }
            ?: emptyList()
    }

    /**
     * Check if a record exists (and is not tombstoned).
     */
    fun exists(domain: String, recordId: String): Boolean {
        val encFile = File(vaultRoot, "$domain/$recordId.enc")
        val tombstone = File(vaultRoot, "$domain/$recordId.tombstone")
        return encFile.exists() && !tombstone.exists()
    }

    /**
     * Verify the integrity of a vault file by checking its SHA-256 hash.
     */
    fun verifyHash(domain: String, recordId: String, expectedHash: String): Boolean {
        val file = File(vaultRoot, "$domain/$recordId.enc")
        if (!file.exists()) return false
        return sha256Hex(file.readBytes()) == expectedHash
    }

    /**
     * Recover from a crash by replaying pending journal entries.
     * Called during vault unlock to ensure data consistency.
     */
    fun recoverFromCrash(): CrashRecoveryResult {
        val pendingEntries = journal.getPendingEntries()
        var recovered = 0
        var failed = 0

        for (entry in pendingEntries) {
            try {
                when (entry.operation) {
                    WriteAheadJournal.JournalOperation.CREATE -> {
                        // The write may or may not have completed.
                        // Verify the file exists and has the expected hash.
                        val file = File(vaultRoot, entry.dataPath)
                        if (file.exists() && verifyHash(entry.domain, entry.recordId, entry.dataHash)) {
                            journal.commit(entry)
                            recovered++
                        } else {
                            // Write didn't complete, roll back
                            journal.rollback(entry)
                            failed++
                        }
                    }
                    WriteAheadJournal.JournalOperation.DELETE -> {
                        // Deletion may or may not have completed.
                        val encFile = File(vaultRoot, "${entry.domain}/${entry.recordId}.enc")
                        val tombstone = File(vaultRoot, "${entry.domain}/${entry.recordId}.tombstone")
                        if (!encFile.exists() || tombstone.exists()) {
                            journal.commit(entry)
                            recovered++
                        } else {
                            // Deletion didn't complete, the file still exists
                            // This is actually OK — we can try again later
                            journal.rollback(entry)
                        }
                    }
                    WriteAheadJournal.JournalOperation.UPDATE -> {
                        // Same as CREATE — check if the update completed
                        val file = File(vaultRoot, entry.dataPath)
                        if (file.exists() && verifyHash(entry.domain, entry.recordId, entry.dataHash)) {
                            journal.commit(entry)
                            recovered++
                        } else {
                            journal.rollback(entry)
                            failed++
                        }
                    }
                }
            } catch (e: Exception) {
                journal.rollback(entry)
                failed++
            }
        }

        // Clean up old entries
        journal.cleanup()

        return CrashRecoveryResult(recovered = recovered, failed = failed)
    }

    private fun sha256(data: ByteArray): ByteArray {
        val digest = MessageDigest.getInstance("SHA-256")
        return digest.digest(data)
    }

    private fun sha256Hex(data: ByteArray): String {
        return sha256(data).joinToString("") { "%02x".format(it) }
    }

    data class CrashRecoveryResult(
        val recovered: Int,
        val failed: Int
    )
}