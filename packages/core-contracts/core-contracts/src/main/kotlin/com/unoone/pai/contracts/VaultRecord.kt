package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * Base trait for all vault records.
 *
 * Every record in the shared vault must include these fields
 * to support cross-platform sync, conflict detection, and deletion propagation.
 */
@Serializable
data class VaultRecordMetadata(
    val id: String,                     // UUID v7 (time-sortable)
    val createdAt: String,               // ISO-8601 instant
    val updatedAt: String,              // ISO-8601 instant
    val sourcePlatform: Platform,       // Which platform created this record
    val sourceDeviceId: String,         // Unique device identifier
    val revision: Long,                 // Monotonic revision number
    val deleted: Boolean = false,        // Tombstone flag — deleted records propagate
    val schemaVersion: Int = 1          // Schema version for migration
)

@Serializable
enum class Platform {
    ANDROID,
    WINDOWS,
    MACOS,
    LINUX
}