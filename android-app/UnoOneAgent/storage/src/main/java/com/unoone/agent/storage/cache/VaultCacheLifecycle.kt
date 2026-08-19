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
 * `model_metadata` is deliberately excluded: it tracks models installed on
 * THIS device, which is device-local state, not a mirror of vault data.
 */
object VaultCacheLifecycle {

    /** Rows older than this are evicted at app start. */
    const val DEFAULT_TTL_MILLIS: Long = 24L * 60 * 60 * 1000 // 24 hours

    /**
     * Evict vault-mirror rows whose lifetime has expired.
     * @return total rows deleted across all vault-mirror tables.
     */
    suspend fun evictExpired(
        db: UnoOneDatabase,
        ttlMillis: Long = DEFAULT_TTL_MILLIS,
        nowMillis: Long = System.currentTimeMillis()
    ): Int {
        val cutoff = nowMillis - ttlMillis
        return db.noteDao().deleteOlderThan(cutoff) +
            db.skillDao().deleteOlderThan(cutoff) +
            db.memoryDao().deleteOlderThan(cutoff) +
            db.actionLogDao().deleteOlderThan(cutoff)
    }

    /** Count of rows that WILL be evicted for a given TTL — used by tests. */
    fun cutoffFor(nowMillis: Long, ttlMillis: Long = DEFAULT_TTL_MILLIS): Long =
        nowMillis - ttlMillis

    /**
     * Vault disconnect: the vault is gone, so the plaintext cache MUST NOT
     * keep a readable copy of vault-mirrored data behind.
     * @return total rows deleted.
     */
    suspend fun clearOnVaultDisconnect(db: UnoOneDatabase): Int {
        return db.noteDao().deleteAll() +
            db.skillDao().deleteAll() +
            db.memoryDao().deleteAll() +
            db.actionLogDao().clearAll()
    }
}
