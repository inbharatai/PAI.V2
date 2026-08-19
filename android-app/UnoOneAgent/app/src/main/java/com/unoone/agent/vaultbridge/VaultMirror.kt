package com.unoone.agent.vaultbridge

import com.unoone.agent.core.util.Logger
import com.unoone.agent.storage.dao.MemoryDao
import com.unoone.agent.storage.dao.NoteDao
import com.unoone.agent.storage.dao.PendingTombstoneDao
import com.unoone.agent.storage.entity.PendingTombstoneEntity
import com.unoone.agent.vault.VaultRecordFactory
import com.unoone.agent.vault.VaultRecordWriter
import com.unoone.agent.vault.VaultSyncPlanner
import java.time.Instant
import java.util.UUID

/**
 * Routes note/memory cache writes through to the shared drive vault, making
 * the vault the canonical store while Room stays the (encrypted) cache/index.
 *
 * Online (vault attached + unlocked): a create is written straight through and
 * the returned record id is stamped onto the cache row. Offline: the row keeps
 * a null vaultRecordId and is flushed by [drainBacklog] on the next unlock;
 * deletions of already-synced rows are queued as pending tombstones and drained
 * the same way. Flush order is decided by the pure, JVM-tested
 * [VaultSyncPlanner].
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
    private val idGen: () -> String = { UUID.randomUUID().toString() },
    private val isoNow: () -> String = { Instant.now().toString() },
    private val isoOf: (Long) -> String = { Instant.ofEpochMilli(it).toString() },
) {

    // ---- write-through --------------------------------------------------

    /** A note was created locally (row [localId]); mirror it if we can. */
    suspend fun onNoteCreated(localId: Long) {
        try {
            val writer = writerProvider() ?: return
            val note = noteDao.getById(localId) ?: return
            if (note.vaultRecordId != null) return
            val mapped = VaultRecordFactory.forNote(
                recordId = idGen(),
                transactionId = idGen(),
                deviceId = deviceId,
                title = note.title,
                content = note.content,
                tags = note.tags,
                createdAtIso = isoOf(note.createdAt),
                updatedAtIso = isoOf(note.updatedAt),
            )
            val vid = writer.writeRecord(mapped.fields, mapped.content)
            noteDao.setVaultRecordId(localId, vid)
        } catch (e: Exception) {
            Logger.w("VaultMirror.onNoteCreated non-fatal: ${e.message}")
        }
    }

    /**
     * A memory was created OR updated locally; mirror it if we can. Memories
     * upsert (storePreference), so a row that already reached the vault is
     * REWRITTEN under the same record id with revision+1 — the vault stays
     * canonical and the desktop sees an honest version bump. Planner
     * telemetry (type "outcome") is device-local and never mirrors.
     */
    suspend fun onMemoryUpserted(localId: Long) {
        try {
            val writer = writerProvider() ?: return
            val memory = memoryDao.getByIdOnce(localId) ?: return
            if (memory.type == "outcome") return
            val isRewrite = memory.vaultRecordId != null
            val recordId = memory.vaultRecordId ?: idGen()
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
        } catch (e: Exception) {
            Logger.w("VaultMirror.onMemoryUpserted non-fatal: ${e.message}")
        }
    }

    // ---- deletion -------------------------------------------------------

    /**
     * A vault-backed row was deleted locally. Tombstone in the vault now if
     * unlocked, else queue it. A null/blank [vaultRecordId] means the row never
     * reached the vault — nothing to do.
     */
    suspend fun onRowDeleted(vaultRecordId: String?, kind: VaultSyncPlanner.Kind) {
        if (vaultRecordId.isNullOrBlank()) return
        try {
            val deletedAt = isoNow()
            val writer = writerProvider()
            if (writer != null) {
                writer.tombstone(vaultRecordId, deletedAt)
            } else {
                tombstoneDao.insert(
                    PendingTombstoneEntity(
                        vaultRecordId = vaultRecordId,
                        recordKind = kind.name,
                        deletedAtIso = deletedAt,
                    ),
                )
            }
        } catch (e: Exception) {
            Logger.w("VaultMirror.onRowDeleted non-fatal: ${e.message}")
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
                writes.add(VaultSyncPlanner.PendingWrite(it.id, VaultSyncPlanner.Kind.MEMORY))
            }
            val tombstones = tombstoneDao.getAll()
                .map { VaultSyncPlanner.PendingTombstone(it.vaultRecordId, it.deletedAtIso) }

            for (op in VaultSyncPlanner.plan(writes, tombstones)) {
                when (op) {
                    is VaultSyncPlanner.Op.Write -> when (op.kind) {
                        VaultSyncPlanner.Kind.NOTE -> onNoteCreated(op.localId)
                        VaultSyncPlanner.Kind.MEMORY -> onMemoryUpserted(op.localId)
                    }
                    is VaultSyncPlanner.Op.Tombstone -> {
                        writer.tombstone(op.vaultRecordId, op.deletedAtIso)
                        tombstoneDao.deleteByVaultRecordId(op.vaultRecordId)
                    }
                }
            }
        } catch (e: Exception) {
            Logger.w("VaultMirror.drainBacklog non-fatal: ${e.message}")
        }
    }
}
