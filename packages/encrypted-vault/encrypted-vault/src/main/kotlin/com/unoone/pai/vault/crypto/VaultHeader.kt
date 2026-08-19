package com.unoone.pai.vault.crypto

import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json

/**
 * Vault header — stored at VAULT/identity/vault.json (encrypted with the master key).
 *
 * This header is the first thing read when unlocking the vault.
 * It contains everything needed to derive keys and verify integrity,
 * but the password itself is NEVER stored here.
 */
@Serializable
data class VaultHeader(
    val vaultId: String,
    val schemaVersion: Int = 1,
    val createdAt: String,
    val updatedAt: String,
    val profileName: String? = null,
    val kdfAlgorithm: String = "Argon2id",
    val kdfParams: KdfParamsHeader,
    val cipherAlgorithm: String,
    val salt: String,                  // Base64-encoded salt
    val verificationTag: String,       // Encrypted verification string to check password correctness
    val recoveryKeySet: Boolean = false,
    val recoveryKeySalt: String? = null, // Base64-encoded recovery key salt (if set)
    val deviceSessions: List<DeviceSessionHeader> = emptyList(),
    val manifestHash: String? = null    // SHA-256 of the vault manifest
)

@Serializable
data class KdfParamsHeader(
    val memoryBytes: Long = 256 * 1024 * 1024,
    val iterations: Int = 3,
    val parallelism: Int = 4,
    val saltLength: Int = 32,
    val outputLength: Int = 32
)

@Serializable
data class DeviceSessionHeader(
    val deviceId: String,
    val platform: String,
    val deviceName: String,
    val firstConnectedAt: String,
    val lastConnectedAt: String,
    val isActive: Boolean = false
)

/**
 * Utility for serializing/deserializing vault headers.
 */
object VaultHeaderCodec {
    private val json = Json { prettyPrint = true; ignoreUnknownKeys = true }

    fun encode(header: VaultHeader): String = json.encodeToString(header)

    fun decode(jsonStr: String): VaultHeader = json.decodeFromString(jsonStr)
}