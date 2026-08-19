package com.unoone.agent.vault

/**
 * Pure offline-backlog planner: given the cache rows that have not yet reached
 * the vault and the deletions queued while the vault was detached, decide what
 * to flush — and in what order — on the next unlock. No Android, no I/O, fully
 * JVM-tested.
 *
 * This slice is one-directional (Android → vault): it flushes local writes and
 * drains local deletions. Vault→Android hydration and multi-device conflict
 * resolution are deliberately out of scope and are NOT decided here.
 */
object VaultSyncPlanner {

    enum class Kind { NOTE, MEMORY }

    /** A cache row with no vaultRecordId yet — needs a create in the vault. */
    data class PendingWrite(val localId: Long, val kind: Kind)

    /** A vault record whose local row was deleted while detached. */
    data class PendingTombstone(val vaultRecordId: String, val deletedAtIso: String)

    sealed interface Op {
        data class Write(val localId: Long, val kind: Kind) : Op
        data class Tombstone(val vaultRecordId: String, val deletedAtIso: String) : Op
    }

    /**
     * Order: writes first (so a record exists before anything can tombstone
     * it), then tombstones. Writes are ordered by localId for deterministic,
     * insertion-ordered flushing. Tombstones are de-duplicated by
     * vaultRecordId (last deletedAt wins) and blanks are dropped — a blank id
     * means the row never reached the vault, so there is nothing to tombstone.
     */
    fun plan(
        writes: List<PendingWrite>,
        tombstones: List<PendingTombstone>,
    ): List<Op> {
        val ops = ArrayList<Op>(writes.size + tombstones.size)

        writes.sortedBy { it.localId }
            .forEach { ops.add(Op.Write(it.localId, it.kind)) }

        tombstones
            .filter { it.vaultRecordId.isNotBlank() }
            .associate { it.vaultRecordId to it.deletedAtIso } // last write wins, dedup
            .forEach { (id, iso) -> ops.add(Op.Tombstone(id, iso)) }

        return ops
    }
}
