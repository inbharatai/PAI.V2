package com.unoone.agent.storage.entity

import androidx.room.Entity
import androidx.room.Index
import androidx.room.PrimaryKey

@Entity(
    tableName = "notes",
    indices = [
        Index("title"),
        Index("tags"),
        Index("createdAt")
    ]
)
data class NoteEntity(
    @PrimaryKey(autoGenerate = true)
    val id: Long = 0,
    val title: String,
    val content: String,
    val tags: String = "", // comma-separated
    val createdAt: Long = System.currentTimeMillis(),
    val updatedAt: Long = System.currentTimeMillis(),
    val reminderTime: Long? = null,
    /**
     * Record id in the shared drive vault once this note has been written
     * there, or null while it lives only in the local cache (created offline,
     * pending flush on the next unlock). The vault is authoritative; this
     * column is the cache→vault link, not a second identity.
     */
    val vaultRecordId: String? = null
)