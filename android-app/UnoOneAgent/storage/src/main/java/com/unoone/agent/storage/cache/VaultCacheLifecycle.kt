package com.unoone.agent.storage.cache

import com.unoone.agent.storage.db.UnoOneDatabase

/**
 * Cache semantics for the Room store.
 *
 * THE RULE: the USB vault is authoritative. The Room store is an at-rest
 * ENCRYPTED cache: SQLCipher via SupportOpenHelperFactory under a
 * Keystore-wrapped passphrase (see app/di/DatabaseProvider plus
 * CacheKeyManager / EncryptedDbPolicy in this package). Encryption protects
 * the bytes at rest; this class enforces the lifecycle half of the rule on
 * top — bounded lifetime + clear-on-disconnect — so vault-mirrored rows do
 * not outlive their welcome even as ciphertext.
 *
 * Two data-safety carve-outs are load-bearing:
 *
 * 1. UNSYNCED ROWS SURVIVE BOTH PATHS. A row whose `vaultRecordId` is null
 *    has never reached the vault — Room is the ONLY copy in existence.
 *    Evicting or clearing it would be silent data loss, so both
 *    [evictExpired] and [clearOnVaultDisconnect] delete *synced rows only*
 *    (`vaultRecordId IS NOT NULL`).
 *
 * 2. DEVICE-LOCAL TABLES ARE EXEMPT from both paths. `skills`,
 *    `model_metadata` and unsynced telemetry memories (`outcome`,
 *    `skill_usage` — which never mirror) are device-local state, not a cache
 *    of vault data; wiping them would destroy the only copy. `action_logs`
 *    stay in both paths on purpose: they never mirror, and the disconnect
 *    wipe is a privacy wipe of device-local audit data.
 */
object VaultCacheLifecycle {

    /** Rows older than this are evicted at app start. */
    const val DEFAULT_TTL_MILLIS: Long = 24L * 60 * 60 * 1000 // 24 hours

    /**
     * Evict vault-mirror rows whose lifetime has expired. Never touches
     * unsynced rows (only copy) or device-local skills.
     * @return total rows deleted across all vault-mirror tables.
     */
    suspend fun evictExpired(
        db: UnoOneDatabase,
        ttlMillis: Long = DEFAULT_TTL_MILLIS,
        nowMillis: Long = System.currentTimeMillis()
    ): Int {
        val cutoff = nowMillis - ttlMillis
        return db.noteDao().deleteOlderThanSynced(cutoff) +
            db.memoryDao().deleteOlderThanSynced(cutoff) +
            db.actionLogDao().deleteOlderThan(cutoff)
    }

    /** Count of rows that WILL be evicted for a given TTL — used by tests. */
    fun cutoffFor(nowMillis: Long, ttlMillis: Long = DEFAULT_TTL_MILLIS): Long =
        nowMillis - ttlMillis

    /**
     * Vault disconnect: the vault is gone, so the plaintext cache MUST NOT
     * keep a readable copy of vault-mirrored data behind. Only rows that
     * actually reached the vault are deleted — unsynced rows survive (they
     * exist nowhere else); skills survive (device-local, Room is their only
     * store); action logs are fully cleared (privacy wipe, they never mirror).
     * @return total rows deleted.
     */
    suspend fun clearOnVaultDisconnect(db: UnoOneDatabase): Int {
        return db.noteDao().deleteSynced() +
            db.memoryDao().deleteSynced() +
            db.actionLogDao().clearAll()
    }
}
