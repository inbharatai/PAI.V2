package com.unoone.agent.di

import android.app.Activity
import android.app.AlertDialog
import android.app.Application
import android.content.Context
import android.os.Bundle

/** Narrow startup-only UI bridge. Application is already in recovery mode, so voice/services and
 * Room startup remain blocked. No manifest or activity changes, no approval by a background service.
 */
internal object PlaintextUpgradeConsent {
    private var installed = false
    @Synchronized fun install(context: Context, upgrade: () -> Unit) {
        if (installed) return
        installed = true
        val application = context.applicationContext as Application
        application.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            private var shown = false
            override fun onActivityResumed(activity: Activity) {
                if (shown || activity.javaClass.name != "com.unoone.agent.MainActivity") return
                shown = true
                application.unregisterActivityLifecycleCallbacks(this)
                AlertDialog.Builder(activity)
                    .setTitle("Encrypt existing local data?")
                    .setMessage("Your standalone database is not encrypted. Upgrade notes, memories, skills and action history to encrypted storage? Settings outside the database are left unchanged.\n\nFor recovery, this creates a temporary plaintext working copy and retains the ORIGINAL plaintext database and its transaction files in this app's private, cloud-backup-excluded storage. These recovery files are NOT encrypted and remain readable to anyone with access to app-private files. They are not uploaded. Additional free space is required.\n\nKeep the backup until you have checked your data. Cleanup is disabled until migration store/key lineage is proven through assisted recovery; do not clear app data or uninstall. Declining does not alter your data.")
                    .setNegativeButton("Not now", null)
                    .setPositiveButton("Encrypt and retain backup") { _, _ ->
                        val progress = AlertDialog.Builder(activity).setTitle("Upgrading local data")
                            .setMessage("Keep this installation. Agent startup stays disabled until you restart after a successful upgrade.")
                            .setCancelable(false).create()
                        progress.show()
                        Thread({
                            val success = try { upgrade(); true } catch (_: Exception) { false }
                            activity.runOnUiThread {
                                if (!activity.isDestroyed && !activity.isFinishing) {
                                    progress.dismiss()
                                    AlertDialog.Builder(activity)
                                        .setTitle(if (success) "Encrypted upgrade complete" else "Upgrade paused safely")
                                        .setMessage(if (success)
                                            "Encrypted schema 6 passed structural, row-count and content verification. Original plaintext recovery files remain in no_backup/plaintext-upgrade-v1. Force stop this app in Android Settings, then reopen it to check your data. Recovery-copy deletion is disabled until assisted recovery proves the original-to-store/key lineage; never clear all app data."
                                        else "Original data and recovery files are retained. Do not clear data or uninstall. Free space if needed, then force stop and reopen to retry, or request assisted recovery. No agent operations have been enabled.")
                                        .setPositiveButton("OK", null).show()
                                }
                            }
                        }, "plaintext-upgrade").start()
                    }.show()
            }
            override fun onActivityCreated(activity: Activity, state: Bundle?) = Unit
            override fun onActivityStarted(activity: Activity) = Unit
            override fun onActivityPaused(activity: Activity) = Unit
            override fun onActivityStopped(activity: Activity) = Unit
            override fun onActivitySaveInstanceState(activity: Activity, state: Bundle) = Unit
            override fun onActivityDestroyed(activity: Activity) = Unit
        })
    }
}
