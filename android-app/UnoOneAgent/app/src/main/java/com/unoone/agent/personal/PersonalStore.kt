package com.unoone.agent.personal

import com.unoone.agent.vault.VaultRecordReader
import com.unoone.agent.vault.VaultRecordWriter
import java.time.Instant

/** Caller holds the session/native mutex across this entire transaction. This single
 * encrypted record is both authoritative mutation history AND pending local outbox.
 * No Room write, second outbox, plaintext journal or drop-on-error fallback exists. */
class PersonalStore(private val vaultId: String, private val reader: VaultRecordReader,
    private val writer: VaultRecordWriter, private val exists: () -> Boolean) {
    fun load(): PersonalLedger {
        if (!exists()) return PersonalLedger.fresh(vaultId).also(::save)
        val (metadata, bytes) = reader.readRecord(PersonalLedger.RECORD_ID)
        try {
            require(metadata["tombstone"] != true) { "Ledger tombstoned; cannot recreate" }
            return PersonalLedger.decode(bytes, vaultId)
        } finally { bytes.fill(0) }
    }
    fun save(ledger: PersonalLedger) {
        require(ledger.local_vault_id == vaultId)
        val now = Instant.now().toString()
        val metadata = mapOf<String, Any?>("record_id" to PersonalLedger.RECORD_ID, "record_type" to "CONTEXT_SNAPSHOT",
            "schema_version" to 1, "encryption_version" to 1, "created_at" to now, "updated_at" to now,
            "revision" to ledger.view().revision, "origin_platform" to "personal-local", "origin_device_id" to "local",
            "transaction_id" to PersonalLedger.id(), "content_hash" to "", "parent_record_id" to null,
            "source_record_ids" to emptyList<String>(), "privacy_level" to "PRIVATE", "tombstone" to false, "deleted_at" to null)
        val bytes = ledger.bytes()
        try { writer.writeRecord(metadata, bytes) } finally { bytes.fill(0) }
    }
    fun mutate(request: PersonalRequest, now: Long): PersonalView {
        val next = load().apply(request, now)
        save(next)
        return next.view()
    }
}
