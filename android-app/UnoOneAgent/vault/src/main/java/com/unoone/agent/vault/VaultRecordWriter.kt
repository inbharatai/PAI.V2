package com.unoone.agent.vault

/**
 * The narrow vault-write surface the app coordinator needs, abstracted from
 * [MobileVaultRepository] so the coordinator is JVM-testable against a fake
 * (the repository itself needs a real unlocked session + SAF tree). The
 * session-backed implementation lives in the app module.
 */
interface VaultRecordWriter {
    /**
     * Write a record built by [VaultRecordFactory]. Returns the vault record id
     * (the `record_id` carried in [fields]).
     */
    fun writeRecord(fields: Map<String, Any?>, content: ByteArray): String

    /** Tombstone an existing vault record. */
    fun tombstone(vaultRecordId: String, deletedAtIso: String)
}
