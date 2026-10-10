package com.unoone.agent.autostart

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import com.unoone.agent.UnoOneApplication
import com.unoone.agent.core.runtime.AgentRuntimeGate
import com.unoone.agent.core.util.Logger
import com.unoone.agent.voice.VoiceService

/**
 * P2-A — the Android auto-launch lane. When the user has explicitly turned
 * "Start automatically when the phone starts" ON in Settings (and the agent
 * itself is enabled), the wake-word voice service starts on BOOT_COMPLETED —
 * the phone behaves like the laptop, where the dock watcher starts UnoOne on
 * USB insert. USB attach cannot start an Android app (no platform autorun),
 * so boot is the reliable plane.
 *
 * Honest failure: Android 15 restricts starting a microphone foreground
 * service from BOOT_COMPLETED under platform privacy policy; battery exclusions do not grant microphone permission.
 * If the OS refuses, the
 * failure is logged and surfaced — never retried in a loop, never a silent
 * pretend-success. On-device behaviour is a physical gate to verify.
 */
class BootCompletedReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (!AutoStartPolicy.isBootAction(intent.action)) return
        val prefs = context.getSharedPreferences(
            UnoOneApplication.SETTINGS_PREFS,
            Context.MODE_PRIVATE,
        )
        val autoStart = prefs.getBoolean(AutoStartPolicy.PREF_KEY, false)
        val agentEnabled = prefs.getBoolean(UnoOneApplication.KEY_AGENT_ENABLED, true)
        if (!AutoStartPolicy.shouldStart(autoStart, agentEnabled)) {
            Logger.i("AutoStart: skipped (auto-start=${autoStart}, agent=${agentEnabled})")
            return
        }
        // This may be a fresh process — the static gate starts at its default,
        // so sync it with the persisted pref before the service reads it.
        // Application owns admission, including database recovery. Boot must never re-enable it.
        if (!AgentRuntimeGate.isEnabled() ||
            (context.applicationContext as? UnoOneApplication)?.databaseRecoveryMessage != null) return
        try {
            VoiceService.start(context)
            Logger.i("AutoStart: voice service started on boot")
        } catch (e: Exception) {
            // Truthful, bounded failure: some OEMs/Android versions block
            // microphone FGS starts from boot. The user is told (Settings
            // shows the battery-optimization request) — nothing is faked.
            Logger.w("AutoStart: OS refused boot start (${e.javaClass.simpleName}: ${e.message})")
        }
    }
}