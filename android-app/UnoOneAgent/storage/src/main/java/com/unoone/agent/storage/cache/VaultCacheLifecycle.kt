package com.unoone.agent.storage.cache

import com.unoone.agent.storage.db.UnoOneDatabase

/** Compatibility entry points for legacy callers. Independent local storage is the only admitted
 * mode: no startup TTL or drive detach may erase user data, even rows bearing an old mirror id.
 * There is deliberately no cache-mode toggle or implicit privacy wipe. Explicit user deletion
 * must use the normal mutation/tombstone path, not these old lifecycle APIs.
 */
object VaultCacheLifecycle {
    const val DEFAULT_TTL_MILLIS: Long = 24L * 60 * 60 * 1000

    @Suppress("UNUSED_PARAMETER")
    suspend fun evictExpired(
        db: UnoOneDatabase,
        ttlMillis: Long = DEFAULT_TTL_MILLIS,
        nowMillis: Long = System.currentTimeMillis()
    ): Int {
        check(!LocalStoreRetentionPolicy.mayAutomaticallyDelete(LocalStoreRetentionPolicy.Event.STARTUP_TTL))
        return 0
    }

    fun cutoffFor(nowMillis: Long, ttlMillis: Long = DEFAULT_TTL_MILLIS): Long = nowMillis - ttlMillis

    @Suppress("UNUSED_PARAMETER")
    suspend fun clearOnVaultDisconnect(db: UnoOneDatabase): Int {
        check(!LocalStoreRetentionPolicy.mayAutomaticallyDelete(LocalStoreRetentionPolicy.Event.LEGACY_DRIVE_DETACH))
        return 0
    }
}
