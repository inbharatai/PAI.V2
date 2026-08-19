package com.unoone.agent.storage.entity

import androidx.room.Entity
import androidx.room.Index
import androidx.room.PrimaryKey

@Entity(
    tableName = "memories",
    indices = [
        Index("key", unique = true),
        Index("type")
    ]
)
data class MemoryEntity(
    @PrimaryKey(autoGenerate = true)
    val id: Long = 0,
    val key: String,
    val value: String,
    val type: String = "general", // e.g. preference, correction, pattern
    val createdAt: Long = System.currentTimeMillis(),
    val updatedAt: Long = System.currentTimeMillis(),
    /**
     * Record id in the shared drive vault once this memory has been written
     * there, or null while it lives only in the local cache. The vault is
     * authoritative; this column is the cache→vault link, not a second
     * identity.
     */
    val vaultRecordId: String? = null,
    /**
     * Revision of the vault record this row last wrote. Memories UPSERT
     * (storePreference), so a value change rewrites the SAME vault record
     * with revision+1 — honest versioning the desktop can trust.
     */
    val vaultRevision: Int = 1
)