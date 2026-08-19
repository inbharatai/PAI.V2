package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * PocketMemoryVault interface — the contract for vault operations.
 *
 * Both the Android and desktop implementations must implement this interface.
 * The vault is the single source of truth. All persistent data lives on the
 * encrypted USB drive. Host storage (Android internal, desktop disk) is
 * temporary cache only.
 */
interface PocketMemoryVault {

    // === Vault lifecycle ===

    /** First-time setup: create password, generate keys, create recovery key. */
    suspend fun setupVault(password: String, profileName: String?): VaultSetupResult

    /** Unlock the vault with the password. Derives the master key. */
    suspend fun unlockVault(password: String): VaultUnlockResult

    /** Lock the vault. Clear all derived keys from memory. */
    suspend fun lockVault()

    /** Check if the vault is currently unlocked. */
    val isUnlocked: Boolean

    /** Emergency lock — immediately clear keys and lock. */
    suspend fun emergencyLock()

    // === CRUD operations ===

    /** Add a new record to the vault. */
    suspend fun <T : Any> addMemory(record: T, serializer: (T) -> String): String

    /** Update an existing record. Increments revision number. */
    suspend fun <T : Any> updateMemory(id: String, record: T, serializer: (T) -> String): Boolean

    /** Search memories by type, tags, or content. */
    suspend fun searchMemory(query: String, type: MemoryType? = null, limit: Int = 20): List<Memory>

    /** Delete a record (creates tombstone). Tombstones propagate across platforms. */
    suspend fun deleteMemory(id: String): Boolean

    /** Retrieve relevant memories for a given context (used by model retrieval). */
    suspend fun retrieveRelevantMemory(context: String, limit: Int = 10): List<Memory>

    // === Specialized storage ===

    /** Save a conversation to the vault. */
    suspend fun saveConversation(conversation: Conversation): String

    /** Save a task to the vault. */
    suspend fun saveTask(task: Task): String

    /** Save a recording (metadata + references to audio/transcript/summary files). */
    suspend fun saveRecording(recording: Recording): String

    /** Save a transcript. */
    suspend fun saveTranscript(transcript: Transcript): String

    /** Save a summary (can be mobile-generated or desktop-generated). */
    suspend fun saveSummary(summary: RecordingSummary): String

    /** Index a document for retrieval. */
    suspend fun indexDocument(document: Document): String

    /** Save a skill. */
    suspend fun saveSkill(skill: Skill): String

    // === Maintenance ===

    /** Rebuild the search index from vault contents. */
    suspend fun rebuildIndex(): Boolean

    /** Verify vault integrity (checksums, manifests, schema version). */
    suspend fun verifyIntegrity(): IntegrityCheckResult

    /** Get current vault metadata. */
    suspend fun getMetadata(): VaultMetadata
}

@Serializable
data class VaultSetupResult(
    val success: Boolean,
    val vaultId: String? = null,
    val recoveryKey: String? = null,       // Shown once, never stored
    val error: String? = null
)

@Serializable
data class VaultUnlockResult(
    val success: Boolean,
    val vaultId: String? = null,
    val failedAttempts: Int = 0,
    val lockoutUntil: String? = null,       // ISO-8601 if locked out
    val error: String? = null
)

@Serializable
data class IntegrityCheckResult(
    val valid: Boolean,
    val errors: List<String> = emptyList(),
    val warnings: List<String> = emptyList(),
    val recoveredFiles: Int = 0,
    val corruptedFiles: Int = 0
)