package com.unoone.agent.vaultbridge

import com.unoone.agent.core.contracts.ContractJson
import com.unoone.agent.core.contracts.EnvObservation
import com.unoone.agent.core.util.Logger
import com.unoone.agent.storage.dao.*
import com.unoone.agent.storage.entity.PendingTombstoneEntity
import com.unoone.agent.storage.entity.PendingWriteEntity
import com.unoone.agent.vault.VaultCrypto
import com.unoone.agent.vault.VaultRecordFactory
import com.unoone.agent.vault.VaultRecordWriter
import com.unoone.agent.vault.VaultSyncPlanner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.time.Instant
import java.util.UUID

/**
 * Drains the authoritative Room outbox. Mutation + generation + identity + delete intent are
 * committed by SQLite inside the DAO mutation, NOT these best-effort wake-up callbacks.
 * No private row contents or backend exception messages are logged here.
 *
 * One process-wide IO lane prevents an older snapshot from overwriting a newer drain or a
 * tombstone. Room edits remain concurrent: both stamp and retire CAS the captured generation.
 * A killed write reuses the same record ID/revision; death after stamp leaves retryable work.
 */
class VaultMirror(
    private val noteDao: NoteDao,
    private val memoryDao: MemoryDao,
    private val tombstoneDao: PendingTombstoneDao,
    private val writerProvider: () -> VaultRecordWriter?,
    private val deviceId: String,
    private val pendingWriteDao: PendingWriteDao,
    private val skillDao: SkillDao? = null,
    private val turnDao: ConversationTurnDao? = null,
    private val idGen: () -> String = { UUID.randomUUID().toString() },
    private val isoNow: () -> String = { Instant.now().toString() },
    private val isoOf: (Long) -> String = { Instant.ofEpochMilli(it).toString() },
    /** Fault boundary for real-Room tests; never carries private payloads. */
    private val checkpoint: suspend (String) -> Unit = {},
) {
    companion object { private val ioLane = Mutex() }

    suspend fun onNoteCreated(localId: Long) = wake("NOTE", localId)
    suspend fun onMemoryUpserted(localId: Long) = wake("MEMORY", localId)
    suspend fun onSkillUpserted(localId: Long) = wake("SKILL", localId)
    suspend fun onTurnRecorded(localId: Long) = wake("TRANSCRIPT", localId)
    suspend fun onEnvFactRecorded(localId: Long) = wake("ENVOBS", localId)

    /** Compatibility wake-up only. A callback is neither deletion consent nor an outbox commit.
     * Call AFTER DAO deletion. Older pre-delete callers still cannot lose intent: the DAO queues it.
     */
    @Suppress("UNUSED_PARAMETER")
    suspend fun onRowDeleted(vaultRecordId: String?, kind: VaultSyncPlanner.Kind) = drainBacklog()

    private suspend fun bestEffort(block: suspend () -> Unit) {
        try { block() }
        catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { Logger.w("Vault mirror deferred; durable Room work retained") }
    }

    private suspend fun wake(kind: String, localId: Long) = ioLane.withLock {
        bestEffort {
            val writer = writerProvider() ?: return@bestEffort
            writePending(kind, localId, writer)
            drainTombstones(writer)
        }
    }

    private suspend fun snapshot(p: PendingWriteEntity): VaultRecordFactory.Mapped? = when (p.recordKind) {
        "NOTE" -> noteDao.getById(p.localId)?.let {
            VaultRecordFactory.forNote(p.recordId, idGen(), deviceId, it.title, it.content, it.tags,
                isoOf(it.createdAt), isoOf(it.updatedAt), p.revision)
        }
        "MEMORY" -> memoryDao.getByIdOnce(p.localId)?.takeUnless {
            it.type in setOf("outcome", "procedure_outcome", "envobs_hypo", "envobs", "harness_memory", "skill_usage")
        }?.let {
            VaultRecordFactory.forMemory(p.recordId, idGen(), deviceId, it.key, it.value, it.type,
                isoOf(it.createdAt), isoOf(it.updatedAt), p.revision)
        }
        "SKILL" -> skillDao?.getById(p.localId)?.let {
            VaultRecordFactory.forSkill(p.recordId, idGen(), deviceId, it.name, it.triggerPhrases,
                it.stepsJson, it.riskLevel, it.enabled, isoOf(it.createdAt), isoOf(it.updatedAt), p.revision)
        }
        "TRANSCRIPT" -> turnDao?.getById(p.localId)?.let {
            val mapped = VaultRecordFactory.forTurn(p.recordId, idGen(), deviceId, it.sessionId,
                it.role, it.content, it.inputType, isoOf(it.createdAt), isoOf(it.createdAt))
            VaultRecordFactory.Mapped(mapped.fields + ("revision" to p.revision), mapped.content)
        }
        "ENVOBS" -> memoryDao.getByIdOnce(p.localId)?.takeIf { it.type == "envobs" }?.let {
            // Unparseable facts remain pending; they are never silently retired or promoted.
            val fact = ContractJson.decodeFromString(EnvObservation.serializer(), it.value)
            VaultRecordFactory.forEnvFact(p.recordId, idGen(), deviceId, fact.subject,
                fact.observedCapability, fact.epistemicStatus.serialName, fact.verificationRef ?: "",
                it.value, isoOf(it.createdAt), isoOf(it.updatedAt), p.revision)
        }
        else -> null
    }

    private suspend fun writePending(kind: String, localId: Long, writer: VaultRecordWriter) {
        val p = pendingWriteDao.get(kind, localId) ?: return
        val mapped = snapshot(p) ?: return
        try {
            // The payload read and generation must describe the SAME committed mutation.
            if (pendingWriteDao.get(kind, localId)?.let { it.id == p.id && it.revision == p.revision } != true) return
            checkpoint("before-write")
            writer.writeRecord(mapped.fields, mapped.content)
            checkpoint("after-write")
            when (kind) {
                "NOTE" -> pendingWriteDao.stampNote(p.id, localId, p.recordId, p.revision)
                "MEMORY", "ENVOBS" -> pendingWriteDao.stampMemory(p.id, localId, p.recordId, p.revision)
                "SKILL" -> pendingWriteDao.stampSkill(p.id, localId, p.recordId, p.revision)
                "TRANSCRIPT" -> pendingWriteDao.stampTurn(p.id, localId, p.recordId, p.revision)
            }
            checkpoint("after-stamp")
            pendingWriteDao.retire(p.id, p.revision) // exact captured generation, never deleteByRow
            checkpoint("after-retire")
        } finally { mapped.content.fill(0) }
    }

    /** v7 deletion has its own revision and is an idempotent empty tombstone UPSERT. This also
     * covers an ID minted before the first write: read-then-tombstone would fail on a missing file.
     * Pre-v7 tombstones retain their native read/authenticate path and cannot bind a fresh vault.
     */
    private fun writeTombstone(t: PendingTombstoneEntity, writer: VaultRecordWriter) {
        if (t.revision == 0) { writer.tombstone(t.vaultRecordId, t.deletedAtIso); return }
        val empty = ByteArray(0)
        val fields = linkedMapOf<String, Any?>(
            "record_id" to t.vaultRecordId,
            "record_type" to when (t.recordKind) { "MEMORY" -> "MEMORY"; "TRANSCRIPT" -> "TRANSCRIPT"; else -> "DOCUMENT" },
            "schema_version" to 1, "encryption_version" to 1,
            "created_at" to isoOf(t.createdAt), "updated_at" to t.deletedAtIso,
            "revision" to t.revision, "origin_platform" to "ANDROID", "origin_device_id" to deviceId,
            "transaction_id" to idGen(), "content_hash" to VaultCrypto.sha256Hex(empty),
            "parent_record_id" to null, "source_record_ids" to emptyList<String>(),
            "privacy_level" to "PRIVATE", "tombstone" to true, "deleted_at" to t.deletedAtIso,
        )
        writer.writeRecord(fields, empty)
    }

    private suspend fun drainTombstones(writer: VaultRecordWriter) {
        tombstoneDao.getAll().forEach { captured -> bestEffort {
            checkpoint("before-tombstone")
            writeTombstone(captured, writer)
            checkpoint("after-tombstone")
            tombstoneDao.deleteById(captured.id) // never retire a later deletion for this record
        } }
    }

    suspend fun drainBacklog() = ioLane.withLock {
        bestEffort {
            val writer = writerProvider() ?: return@bestEffort
            pendingWriteDao.getAll().forEach { p -> bestEffort { writePending(p.recordKind, p.localId, writer) } }
            drainTombstones(writer) // includes deletes that raced a captured write
        }
    }
}
