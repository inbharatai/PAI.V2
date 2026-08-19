package com.unoone.agent.vaultbridge

import android.content.Context
import androidx.core.content.edit
import java.util.UUID

/**
 * Stable per-install identifier used as `origin_device_id` on vault records.
 * A random UUID persisted on first use — provenance, not a hardware id, and
 * never derived from user data.
 */
object VaultDeviceId {

    private const val PREFS = "vault_device"
    private const val KEY = "vault_device_id"

    fun getOrCreate(context: Context): String {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs.getString(KEY, null)?.let { return it }
        val id = UUID.randomUUID().toString()
        prefs.edit { putString(KEY, id) }
        return id
    }
}
