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

    @Serializable
    data class SkillContent(
        val kind: String = "skill",
        val name: String,
        val triggerPhrases: String,
        val stepsJson: String,
        val riskLevel: Int,
        val enabled: Boolean,
    )

    /**
     * One conversation turn. The vault is the ONE universal usage history:
     * every user command and every spoken agent response lands here as a
     * TRANSCRIPT record, whatever host the conversation happened on.
     * sessionId groups the turns of one command invocation; role is
     * "user" or "assistant".
     */
    @Serializable
    data class TurnContent(
        val kind: String = "transcript",
        val sessionId: String,
        val role: String,
        val content: String,
        val inputType: String,
    )

    /**
     * A user-confirmed environment fact (or correction) — the vault-mirrored
     * half of bounded env learning. The full capability-contract observation
     * JSON travels in [observationJson] (schema inbharat.pai.envobs.v1); the
     * envelope fields let every host index it without parsing the contract
     * body. Only VERIFIED_FACT and CORRECTION records are ever mirrored —
     * hypotheses stay device-local.
     */
    @Serializable
    data class EnvFactContent(
        val kind: String = "envobs",
        val subject: String,
        val observedCapability: String,
        val epistemicStatus: String,
        val verificationRef: String,
        val observationJson: String,
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
     * A skill mirrors as a DOCUMENT record carrying the {kind:"skill"} envelope —
     * the same convention notes use, so every host can tell record types apart
     * without decrypt-dependent side tables. Skills upsert by name, so a save
     * rewrites the SAME record with revision+1 (see [VaultMirror.onSkillUpserted]).
     */
    fun forSkill(
        recordId: String,
        transactionId: String,
        deviceId: String,
        name: String,
        triggerPhrases: String,
        stepsJson: String,
        riskLevel: Int,
        enabled: Boolean,
        createdAtIso: String,
        updatedAtIso: String,
        revision: Int = 1,
    ): Mapped {
        val payload = json.encodeToString(
            SkillContent(
                name = name,
                triggerPhrases = triggerPhrases,
                stepsJson = stepsJson,
                riskLevel = riskLevel,
                enabled = enabled,
            ),
        ).toByteArray(Charsets.UTF_8)
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

    /**
     * A conversation turn mirrors as a TRANSCRIPT record — the record type the
     * desktop already uses for its voice-recording transcripts, so the whole
     * usage history from every host lives in one record space. Turns are
     * append-only: no rewrite path exists, revision stays 1.
     */
    fun forTurn(
        recordId: String,
        transactionId: String,
        deviceId: String,
        sessionId: String,
        role: String,
        content: String,
        inputType: String,
        createdAtIso: String,
        updatedAtIso: String,
        revision: Int = 1,
    ): Mapped {
        val payload = json.encodeToString(
            TurnContent(sessionId = sessionId, role = role, content = content, inputType = inputType),
        ).toByteArray(Charsets.UTF_8)
        return Mapped(
            baseFields(
                recordId = recordId,
                recordType = "TRANSCRIPT",
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
     * A user-confirmed env-learning fact mirrors as a DOCUMENT record with the
     * {kind:"envobs"} envelope. Facts upsert per subject (approve → disapprove
     * rewrites the same record as a correction), so a re-approval rewrites the
     * SAME vault record with revision+1 (see [VaultMirror.onEnvFactRecorded]).
     */
    fun forEnvFact(
        recordId: String,
        transactionId: String,
        deviceId: String,
        subject: String,
        observedCapability: String,
        epistemicStatus: String,
        verificationRef: String,
        observationJson: String,
        createdAtIso: String,
        updatedAtIso: String,
        revision: Int = 1,
    ): Mapped {
        val payload = json.encodeToString(
            EnvFactContent(
                subject = subject,
                observedCapability = observedCapability,
                epistemicStatus = epistemicStatus,
                verificationRef = verificationRef,
                observationJson = observationJson,
            ),
        ).toByteArray(Charsets.UTF_8)
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

    /**
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
