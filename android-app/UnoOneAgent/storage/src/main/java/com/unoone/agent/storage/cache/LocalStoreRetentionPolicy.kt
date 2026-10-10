package com.unoone.agent.storage.cache

/** The independent encrypted store is user data, not a USB cache. A mirror id is not deletion
 * authority: it says nothing about pending edits or whether another device still has that record.
 */
object LocalStoreRetentionPolicy {
    enum class Event { STARTUP_TTL, LEGACY_DRIVE_DETACH }
    @Suppress("UNUSED_PARAMETER")
    fun mayAutomaticallyDelete(event: Event): Boolean = false
}
