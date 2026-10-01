package com.unoone.agent.vaultbridge

import com.unoone.agent.core.contracts.ContractJson
import com.unoone.agent.core.util.Logger
import com.unoone.agent.storage.dao.MemoryDao
import com.unoone.agent.storage.dao.NoteDao
import com.unoone.agent.storage.dao.ConversationTurnDao
import com.unoone.agent.storage.dao.PendingTombstoneDao
import com.unoone.agent.storage.dao.PendingWriteDao
import com.unoone.agent.storage.dao.SkillDao
import com.unoone.agent.storage.entity.PendingTombstoneEntity
import com.unoone.agent.storage.entity.PendingWriteEntity
import com.unoone.agent.vault.VaultRecordFactory
import com.unoone.agent.vault.VaultRecordWriter
import com.unoone.agent.vault.VaultSyncPlanner
import java.time.Instant
import java.util.UUID

/**
 * Routes note/memory/skill/conversation-turn cache writes through to the shared
 * drive vault, making the vault the canonical store while Room stays the
 * (encrypted) cache/index.
 *
 * Online (vault attached + unlocked): a create is written straight through and
 * the returned record id is stamped onto the cache row. Offline: the row keeps
 * a null vaultRecordId and is flushed by [drainBacklog] on the next unlock;
 * deletions of already-synced rows are queued as pending tombstones and drained
 * the same way. Flush order is decided by the pure, JVM-tested
 * [VaultSyncPlanner].
 *
 * Interrupted writes can never duplicate a vault record: the record id for a
 * row's first write is minted once and persisted in the [PendingWriteDao]
 * queue BEFORE the vault write, so a crash between the write and the cache
 * stamp makes the retry reuse the SAME id (the vault write itself is an
 * idempotent overwrite of the same record file). A failed tombstone is the
 * same: queued, retried on the next drain — a deletion is never silently
 * dropped because one write threw.
 *
 * Every vault interaction is best-effort and non-fatal: a vault error must
 * never break a local note/memory write. On failure the row simply stays
 * unsynced for the next drain, and the cause is logged.
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
) {

    // ---- write identity ---------------------------------------------------

    /**
     * The record id for one cache row's first vault write. Minted once,
     * persisted BEFORE the vault write and reused by every retry, so a crash
     * between "record written" and "row stamped" can never mint a second
     * record. Rewrites of an already-linked row reuse the row's own link.
     */
    private suspend fun recordIdFor(kind: VaultSyncPlanner.Kind, localId: Long, linkedRecordId: String?): String {
        if (!linkedRecordId.isNullOrBlank()) return linkedRecordId
        val existing = pendingWriteDao.get(kind.name, localId)
        if (existing != null) return existing.recordId
        val fresh = idGen()
        pendingWriteDao.insert(
            PendingWriteEntity(recordKind = kind.name, localId = localId, recordId = fresh),
        )
        return fresh
    }

    /** The vault write succeeded and the row is stamped — the pending id retires. */
    private suspend fun writeCompleted(kind: VaultSyncPlanner.Kind, localId: Long) {
        pendingWriteDao.deleteByRow(kind.name, localId)
    }

    // ---- write-through --------------------------------------------------

    /** A note was created locally (row [localId]); mirror it if we can. */
    suspend fun onNoteCreated(localId: Long) {
        try {
            val writer = writerProvider() ?: return
            val note = noteDao.getById(localId) ?: return
            if (note.vaultRecordId != null) return
            val recordId = recordIdFor(VaultSyncPlanner.Kind.NOTE, localId, note.vaultRecordId)
            val mapped = VaultRecordFactory.forNote(
                recordId = recordId,
                transactionId = idGen(),
                deviceId = deviceId,
                title = note.title,
                content = note.content,
                tags = note.tags,
                createdAtIso = isoOf(note.createdAt),
                updatedAtIso = isoOf(note.updatedAt),
            )
            writer.writeRecord(mapped.fields, mapped.content)
            noteDao.setVaultRecordId(localId, recordId)
            writeCompleted(VaultSyncPlanner.Kind.NOTE, localId)
        } catch (e: Exception) {
            Logger.w("VaultMirror.onNoteCreated non-fatal: ${e.message}")
        }
    }

    /**
     * A memory was created OR updated locally; mirror it if we can. Memories
     * upsert (storePreference), so a row that already reached the vault is
     * REWRITTEN under the same record id with revision+1 — the vault stays
     * canonical and the desktop sees an honest version bump. Planner
     * telemetry (type "outcome") is device-local and never mirrors; env-learning
     * procedure records and hypotheses are device-local too; env FACTS
     * (type "envobs") mirror through their own [onEnvFactRecorded] path.
     */
    suspend fun onMemoryUpserted(localId: Long) {
        try {
            val writer = writerProvider() ?: return
            val memory = memoryDao.getByIdOnce(localId) ?: return
            if (memory.type == "outcome" || memory.type == "procedure_outcome" ||
                memory.type == "envobs_hypo" || memory.type == "envobs" ||
                // Hydrated desktop-harness memories are read-only here: a
                // phone rewrite would replace the harness's structured
                // envelope with a mobile one — a silent format clobber.
                memory.type == "harness_memory"
            ) return
            val isRewrite = memory.vaultRecordId != null
            val recordId = recordIdFor(VaultSyncPlanner.Kind.MEMORY, localId, memory.vaultRecordId)
            val revision = if (isRewrite) memory.vaultRevision + 1 else 1
            val mapped = VaultRecordFactory.forMemory(
                recordId = recordId,
                transactionId = idGen(),
                deviceId = deviceId,
                key = memory.key,
                value = memory.value,
                type = memory.type,
                createdAtIso = isoOf(memory.createdAt),
                updatedAtIso = isoOf(memory.updatedAt),
                revision = revision,
            )
            writer.writeRecord(mapped.fields, mapped.content)
            memoryDao.setVaultLink(localId, recordId, revision)
            writeCompleted(VaultSyncPlanner.Kind.MEMORY, localId)
        } catch (e: Exception) {
            Logger.w("VaultMirror.onMemoryUpserted non-fatal: ${e.message}")
        }
    }

    // ---- deletion -------------------------------------------------------

    private suspend fun queueTombstone(vaultRecordId: String, kind: VaultSyncPlanner.Kind, deletedAtIso: String) {
        tombstoneDao.insert(
            PendingTombstoneEntity(
                vaultRecordId = vaultRecordId,
                recordKind = kind.name,
                deletedAtIso = deletedAtIso,
            ),
        )
    }

    /**
     * A vault-backed row was deleted locally. Tombstone in the vault now if
     * unlocked, else queue it. A null/blank [vaultRecordId] means the row never
     * reached the vault — nothing to do. A tombstone that THROWS while the
     * vault is attached is queued too: a deletion must survive a transient
     * vault failure, never vanish with the local row already gone.
     */
    suspend fun onRowDeleted(vaultRecordId: String?, kind: VaultSyncPlanner.Kind) {
        if (vaultRecordId.isNullOrBlank()) return
        val deletedAt = isoNow()
        try {
            val writer = writerProvider()
            if (writer != null) {
                writer.tombstone(vaultRecordId, deletedAt)
            } else {
                queueTombstone(vaultRecordId, kind, deletedAt)
            }
        } catch (e: Exception) {
            Logger.w("VaultMirror.onRowDeleted tombstone failed, queued for retry: ${e.message}")
            try {
                queueTombstone(vaultRecordId, kind, deletedAt)
            } catch (queue: Exception) {
                Logger.w("VaultMirror.onRowDeleted queue also failed — deletion lost: ${queue.message}")
            }
        }
    }

    // ---- skill write-through ---------------------------------------------

    /**
     * A skill was saved or updated locally (row [localId]); mirror it if we
     * can. Skills upsert by name, so a save rewrites the SAME vault record
     * with revision+1 — the same honest versioning memories use. No-op when
     * the mirror was built without a skillDao (older call sites).
     */
    suspend fun onSkillUpserted(localId: Long) {
        val dao = skillDao ?: return
        try {
            val writer = writerProvider() ?: return
            val skill = dao.getById(localId) ?: return
            val isRewrite = skill.vaultRecordId != null
            val recordId = recordIdFor(VaultSyncPlanner.Kind.SKILL, localId, skill.vaultRecordId)
            val revision = if (isRewrite) skill.vaultRevision + 1 else 1
            val mapped = VaultRecordFactory.forSkill(
                recordId = recordId,
                transactionId = idGen(),
                deviceId = deviceId,
                name = skill.name,
                triggerPhrases = skill.triggerPhrases,
                stepsJson = skill.stepsJson,
                riskLevel = skill.riskLevel,
                enabled = skill.enabled,
                createdAtIso = isoOf(skill.createdAt),
                updatedAtIso = isoOf(skill.updatedAt),
                revision = revision,
            )
            writer.writeRecord(mapped.fields, mapped.content)
            dao.setVaultLink(localId, recordId, revision)
            writeCompleted(VaultSyncPlanner.Kind.SKILL, localId)
        } catch (e: Exception) {
            Logger.w("VaultMirror.onSkillUpserted non-fatal: ${e.message}")
        }
    }

    // ---- transcript write-through ---------------------------------------

    /**
     * A conversation turn was recorded locally (row [localId]); mirror it if
     * we can, so the vault holds the whole usage history from every host as
     * ONE source. Turns are append-only: there is no rewrite path, revision
     * stays 1. No-op when the mirror was built without a turn dao.
     */
    suspend fun onTurnRecorded(localId: Long) {
        val dao = turnDao ?: return
        try {
            val writer = writerProvider() ?: return
            val turn = dao.getById(localId) ?: return
            if (turn.vaultRecordId != null) return // already mirrored
            val recordId = recordIdFor(VaultSyncPlanner.Kind.TRANSCRIPT, localId, turn.vaultRecordId)
            val mapped = VaultRecordFactory.forTurn(
                recordId = recordId,
                transactionId = idGen(),
                deviceId = deviceId,
                sessionId = turn.sessionId,
                role = turn.role,
                content = turn.content,
                inputType = turn.inputType,
                createdAtIso = isoOf(turn.createdAt),
                updatedAtIso = isoOf(turn.createdAt),
            )
            val vid = writer.writeRecord(mapped.fields, mapped.content)
            dao.setVaultLink(localId, vid, 1)
            writeCompleted(VaultSyncPlanner.Kind.TRANSCRIPT, localId)
        } catch (e: Exception) {
            Logger.w("VaultMirror.onTurnRecorded non-fatal: ${e.message}")
        }
    }

    // ---- env-learning fact write-through ----------------------------------

    /**
     * A user-confirmed environment fact or correction was written locally
     * (row [localId], type "envobs"); mirror it if we can, so every host
     * learns what the user has approved on this one. Facts upsert per
     * subject, so an approve→disapprove cycle rewrites the SAME vault
     * record with revision+1 — the correction honestly replaces the fact.
     * Hypotheses never reach this path: they stay on the device that
     * recorded them ([MemoryDao.notSynced] excludes type "envobs_hypo").
     */
    suspend fun onEnvFactRecorded(localId: Long) {
        try {
            val writer = writerProvider() ?: return
            val fact = memoryDao.getByIdOnce(localId) ?: return
            if (fact.type != "envobs") return
            val record = try {
                ContractJson.decodeFromString(
                    com.unoone.agent.core.contracts.EnvObservation.serializer(),
                    fact.value,
                )
            } catch (_: Exception) {
                null
            } ?: return // never mirror an unparseable fact — honesty over reach
            val isRewrite = fact.vaultRecordId != null
            val recordId = recordIdFor(VaultSyncPlanner.Kind.ENVOBS, localId, fact.vaultRecordId)
            val revision = if (isRewrite) fact.vaultRevision + 1 else 1
            val mapped = VaultRecordFactory.forEnvFact(
                recordId = recordId,
                transactionId = idGen(),
                deviceId = deviceId,
                subject = record.subject,
                observedCapability = record.observedCapability,
                epistemicStatus = record.epistemicStatus.serialName,
                verificationRef = record.verificationRef ?: "",
                observationJson = fact.value,
                createdAtIso = isoOf(fact.createdAt),
                updatedAtIso = isoOf(fact.updatedAt),
                revision = revision,
            )
            writer.writeRecord(mapped.fields, mapped.content)
            memoryDao.setVaultLink(localId, recordId, revision)
            writeCompleted(VaultSyncPlanner.Kind.ENVOBS, localId)
        } catch (e: Exception) {
            Logger.w("VaultMirror.onEnvFactRecorded non-fatal: ${e.message}")
        }
    }

    // ---- backlog flush --------------------------------------------------

    /** Flush everything that accumulated while locked/detached. Call on unlock. */
    suspend fun drainBacklog() {
        val writer = writerProvider() ?: return
        try {
            val writes = ArrayList<VaultSyncPlanner.PendingWrite>()
            noteDao.notSynced().forEach {
                writes.add(VaultSyncPlanner.PendingWrite(it.id, VaultSyncPlanner.Kind.NOTE))
            }
            memoryDao.notSynced().forEach {
                // Env facts mirror through their own DOCUMENT {kind:"envobs"}
                // path; everything else is a plain memory record.
                val kind = if (it.type == "envobs") VaultSyncPlanner.Kind.ENVOBS
                else VaultSyncPlanner.Kind.MEMORY
                writes.add(VaultSyncPlanner.PendingWrite(it.id, kind))
            }
            skillDao?.notSynced()?.forEach {
                writes.add(VaultSyncPlanner.PendingWrite(it.id, VaultSyncPlanner.Kind.SKILL))
            }
            turnDao?.notSynced()?.forEach {
                writes.add(VaultSyncPlanner.PendingWrite(it.id, VaultSyncPlanner.Kind.TRANSCRIPT))
            }
            val tombstones = tombstoneDao.getAll()
                .map { VaultSyncPlanner.PendingTombstone(it.vaultRecordId, it.deletedAtIso) }

            for (op in VaultSyncPlanner.plan(writes, tombstones)) {
                // One failed op never aborts the rest: a write that throws
                // stays unsynced for the next drain, a tombstone that throws
                // stays queued (deleteByVaultRecordId runs only on success).
                try {
                    when (op) {
                        is VaultSyncPlanner.Op.Write -> when (op.kind) {
                            VaultSyncPlanner.Kind.NOTE -> onNoteCreated(op.localId)
                            VaultSyncPlanner.Kind.MEMORY -> onMemoryUpserted(op.localId)
                            VaultSyncPlanner.Kind.SKILL -> onSkillUpserted(op.localId)
                            VaultSyncPlanner.Kind.TRANSCRIPT -> onTurnRecorded(op.localId)
                            VaultSyncPlanner.Kind.ENVOBS -> onEnvFactRecorded(op.localId)
                        }
                        is VaultSyncPlanner.Op.Tombstone -> {
                            writer.tombstone(op.vaultRecordId, op.deletedAtIso)
                            tombstoneDao.deleteByVaultRecordId(op.vaultRecordId)
                        }
                    }
                } catch (e: Exception) {
                    Logger.w("VaultMirror.drain op non-fatal, retried next unlock: ${e.message}")
                }
            }
        } catch (e: Exception) {
            Logger.w("VaultMirror.drainBacklog non-fatal: ${e.message}")
        }
    }
}
