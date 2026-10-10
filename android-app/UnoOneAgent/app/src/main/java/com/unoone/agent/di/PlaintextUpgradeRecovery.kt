package com.unoone.agent.di

import android.app.Activity
import android.app.AlertDialog
import android.app.Application
import android.content.Context
import android.os.Bundle

/** Retention notice only. v1 migration journals cannot prove encrypted-store/key lineage. */
internal object PlaintextUpgradeRecovery {
    private var installed = false
    @Synchronized fun install(context: Context) {
        if (installed) return
        installed = true
        val application = context.applicationContext as Application
        application.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            override fun onActivityResumed(activity: Activity) {
                if (activity.javaClass.name != "com.unoone.agent.MainActivity") return
                application.unregisterActivityLifecycleCallbacks(this)
                AlertDialog.Builder(activity).setTitle("Plaintext recovery backup retained")
                    .setMessage("Your encrypted database opened successfully. Unencrypted recovery copies remain in private, cloud-backup-excluded app storage. They are not uploaded. Check your notes, memories, skills and action history.\n\nAutomatic and in-app cleanup are disabled: this migration format cannot prove that the current database and wrapped key belong to these originals. Keep these copies and this installation until an assisted, verified recovery/export establishes that link. Do not clear app data or uninstall. This is not secure erasure of flash storage.")
                    .setPositiveButton("Keep recovery copies", null).show()
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
