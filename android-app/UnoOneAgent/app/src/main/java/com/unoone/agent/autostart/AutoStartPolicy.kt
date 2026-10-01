package com.unoone.agent.autostart

/**
 * The user-controlled auto-launch policy (P2-A): the laptop auto-launches
 * through the dock's USB-insert watcher; the honest Android equivalent is
 * starting the voice service on BOOT_COMPLETED — but ONLY when the user has
 * explicitly opted in. Default OFF; never hidden autostart.
 *
 * Pure JVM logic so the decision itself is unit-tested; the receiver is thin
 * glue. Android cannot auto-start an app on USB attach (the platform has no
 * autorun support — the same reason the dock mechanism exists on Windows),
 * so boot is the reliable plane.
 */
object AutoStartPolicy {

    /** SharedPreferences key in the shared unoone_settings store. */
    const val PREF_KEY = "auto_start_enabled"

    /** OEM-specific boot broadcast some devices send instead of BOOT_COMPLETED. */
    const val ACTION_QUICKBOOT_POWERON = "android.intent.action.QUICKBOOT_POWERON"

    /**
     * The boot receiver starts the agent only when BOTH hold:
     * 1. the user turned auto-launch ON in Settings (explicit opt-in);
     * 2. the agent itself is enabled (the emergency-stop toggle is respected —
     *    a disabled agent must never wake itself up).
     */
    fun shouldStart(autoStartEnabled: Boolean, agentEnabled: Boolean): Boolean =
        autoStartEnabled && agentEnabled

    /** True when [action] is a boot completion this receiver handles. */
    fun isBootAction(action: String?): Boolean =
        action == android.content.Intent.ACTION_BOOT_COMPLETED || action == ACTION_QUICKBOOT_POWERON
}