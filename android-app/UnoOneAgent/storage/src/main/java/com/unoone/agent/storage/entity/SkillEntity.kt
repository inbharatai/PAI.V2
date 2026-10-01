package com.unoone.agent.storage.entity

import androidx.room.Entity
import androidx.room.Index
import androidx.room.PrimaryKey

@Entity(
    tableName = "skills",
    indices = [
        Index("name", unique = true)
    ]
)
data class SkillEntity(
    @PrimaryKey(autoGenerate = true)
    val id: Long = 0,
    val name: String,
    val triggerPhrases: String, // comma-separated
    val stepsJson: String,
    val riskLevel: Int = 0,
    val enabled: Boolean = true,
    val createdAt: Long = System.currentTimeMillis(),
    val updatedAt: Long = System.currentTimeMillis(),
    /**
     * Record id in the shared drive vault once this skill has been mirrored
     * there as a DOCUMENT {kind:"skill"} record, or null while it lives only
     * in the local cache (created offline, pending flush on the next unlock).
     * Mirrors the note/memory cache→vault link columns.
     */
    val vaultRecordId: String? = null,
    /** Revision of the vault record this row last wrote (upsert = revision+1). */
    val vaultRevision: Int = 1
)