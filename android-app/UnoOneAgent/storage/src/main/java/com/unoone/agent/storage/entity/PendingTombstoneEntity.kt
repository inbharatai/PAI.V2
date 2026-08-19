package com.unoone.agent.storage.entity

import androidx.room.Entity
import androidx.room.PrimaryKey

/**
 * A deletion that happened while the drive vault was detached. On the next
 * unlock the vault record named by [vaultRecordId] is tombstoned and the row
 * is removed. Only rows that HAD reached the vault (non-null vaultRecordId)
 * ever become pending tombstones — a purely-local row is just deleted.
 */
@Entity(tableName = "pending_tombstones")
data class PendingTombstoneEntity(
    @PrimaryKey(autoGenerate = true)
    val id: Long = 0,
    val vaultRecordId: String,
    val recordKind: String, // "NOTE" | "MEMORY", for diagnostics
    val deletedAtIso: String,
    val createdAt: Long = System.currentTimeMillis()
)
