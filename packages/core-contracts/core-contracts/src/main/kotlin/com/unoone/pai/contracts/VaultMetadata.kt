package com.unoone.pai.contracts

import kotlinx.serialization.Serializable

/**
 * Vault metadata — the manifest that describes the vault state.
 *
 * Stored at VAULT/identity/vault.json (encrypted).
 * Contains vault identity, schema version, device sessions, and integrity checksums.
 */
@Serializable
data class VaultMetadata(
    val vaultId: String,                     // Unique vault identifier (UUID v7)
    val schemaVersion: Int = 1,              // Vault schema version
    val createdAt: String,                   // ISO-8601 instant
    val updatedAt: String,                   // ISO-8601 instant
    val profileName: String? = null,         // Optional display name (e.g., "My UnoOne")
    val kdfAlgorithm: String = "Argon2id",   // Key derivation algorithm
    val kdfParams: KdfParams = KdfParams(),
    val cipherAlgorithm: String = "XChaCha20-Poly1305", // Primary cipher
    val fallbackCipher: String = "AES-256-GCM",         // Fallback cipher
    val deviceSessions: List<DeviceSession> = emptyList(),
    val manifests: List<ManifestEntry> = emptyList(),
    val recoveryKeySet: Boolean = false
)

@Serializable
data class KdfParams(
    val memoryBytes: Long = 256 * 1024 * 1024, // 256 MB
    val iterations: Int = 3,
    val parallelism: Int = 4,
    val saltLength: Int = 32,
    val outputLength: Int = 32
)

@Serializable
data class DeviceSession(
    val deviceId: String,                   // Unique device identifier
    val platform: Platform,
    val deviceName: String,                  // e.g., "Pixel 8", "MacBook Pro"
    val firstConnectedAt: String,            // ISO-8601 instant
    val lastConnectedAt: String,             // ISO-8601 instant
    val isActive: Boolean = false            // Currently connected?
)

@Serializable
data class ManifestEntry(
    val path: String,                        // Relative path within VAULT/
    val sha256: String,                      // SHA-256 hash of the encrypted file
    val size: Long,                          // File size in bytes
    val schemaVersion: Int = 1,
    val domainKeyUsed: String? = null        // Which domain encryption key was used
)