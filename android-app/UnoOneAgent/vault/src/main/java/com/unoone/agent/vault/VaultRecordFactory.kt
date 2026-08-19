package com.unoone.agent.vault

import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json

/**
 * Pure builder for the canonical vault record a note or memory becomes when it
 * is written to the shared drive vault. No Android, no I/O — every value is an
 * argument, so the whole mapping is JVM-unit-tested (the portable-logic rule).
 *
 * The output feeds [MobileVaultRepository.writeRecord]: a metadata map in the
 * exact field set + types [VaultCrypto.canonicalAad] accepts (which mirrors the
 * Rust `Record` declaration order), plus the plaintext content bytes.
 *
 * Record type is chosen to match the desktop read path
 * (apps/desktop/src-tauri/src/documents.rs): a note is a DOCUMENT with no
 * parent; a memory is a MEMORY with no parent. content_hash is the SHA-256 of
 * the plaintext content, exactly as vault-core `write_record` computes it.
 *
 * The content payload is a small self-describing JSON envelope so a note's
 * title/tags and a memory's key/type are not lost (the vault Record metadata
 * schema has no such fields). Desktop→Android hydration is out of scope for
 * this slice, so this payload schema is an Android-authored convention, pinned
 * by tests; when hydration is built both sides will agree on it.
 */
object VaultRecordFactory {

    private val json = Json { encodeDefaults = true }

    /** Metadata map (canonical field set) + plaintext content for one record. */
    class Mapped(val fields: Map<String, Any?>, val content: ByteArray)

    @Serializable
    data class NoteContent(
        val kind: String = "note",
        val title: String,
        val content: String,
        val tags: String,
    )

    @Serializable
    data class MemoryContent(
        val kind: String = "memory",
        val key: String,
        val value: String,
        val type: String,
    )

    fun forNote(
        recordId: String,
        transactionId: String,
        deviceId: String,
        title: String,
        content: String,
        tags: String,
        createdAtIso: String,
        updatedAtIso: String,
        revision: Int = 1,
    ): Mapped {
        val payload = json.encodeToString(NoteContent(title = title, content = content, tags = tags))
            .toByteArray(Charsets.UTF_8)
        return Mapped(
            baseFields(
                recordId = recordId,
                recordType = "DOCUMENT",
                transactionId = transactionId,
                deviceId = deviceId,
                createdAtIso = createdAtIso,
                updatedAtIso = updatedAtIso,
                content = payload,
                revision = revision,
            ),
            payload,
        )
    }

    fun forMemory(
        recordId: String,
        transactionId: String,
        deviceId: String,
        key: String,
        value: String,
        type: String,
        createdAtIso: String,
        updatedAtIso: String,
        revision: Int = 1,
    ): Mapped {
        val payload = json.encodeToString(MemoryContent(key = key, value = value, type = type))
            .toByteArray(Charsets.UTF_8)
        return Mapped(
            baseFields(
                recordId = recordId,
                recordType = "MEMORY",
                transactionId = transactionId,
                deviceId = deviceId,
                createdAtIso = createdAtIso,
                updatedAtIso = updatedAtIso,
                content = payload,
                revision = revision,
            ),
            payload,
        )
    }

    /**
     * The 16 canonical fields in the exact types [VaultCrypto.canonicalAad]
     * accepts (String / Int / Boolean / List<String> / null), matching the
     * Rust `Record` schema. `LinkedHashMap` preserves declaration order for
     * readability; canonicalAad re-orders by its own pinned list regardless.
     */
    private fun baseFields(
        recordId: String,
        recordType: String,
        transactionId: String,
        deviceId: String,
        createdAtIso: String,
        updatedAtIso: String,
        content: ByteArray,
        revision: Int,
    ): Map<String, Any?> = linkedMapOf(
        "record_id" to recordId,
        "record_type" to recordType,
        "schema_version" to 1,
        "encryption_version" to 1,
        "created_at" to createdAtIso,
        "updated_at" to updatedAtIso,
        "revision" to revision,
        "origin_platform" to "ANDROID",
        "origin_device_id" to deviceId,
        "transaction_id" to transactionId,
        "content_hash" to VaultCrypto.sha256Hex(content),
        "parent_record_id" to null,
        "source_record_ids" to emptyList<String>(),
        "privacy_level" to "PRIVATE",
        "tombstone" to false,
        "deleted_at" to null,
    )
}
