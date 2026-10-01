package com.unoone.agent.storage.entity

import androidx.room.Entity
import androidx.room.Index
import androidx.room.PrimaryKey

/**
 * The vault record id minted for a cache row whose write has NOT yet
 * succeeded. Inserted BEFORE the vault write so a retry after a crash
 * between "write to the vault" and "stamp the cache row" reuses the SAME
 * record id instead of minting a second record (an interrupted write must
 * never duplicate vault records). Deleted once the row is linked.
 *
 * One row per (recordKind, localId): a retry finds and reuses it.
 */
@Entity(
    tableName = "pending_writes",
    indices = [Index(value = ["recordKind", "localId"], unique = true)]
)
data class PendingWriteEntity(
    @PrimaryKey(autoGenerate = true)
    val id: Long = 0,
    val recordKind: String, // VaultSyncPlanner.Kind name
    val localId: Long,
    val recordId: String,
    val createdAt: Long = System.currentTimeMillis()
)