package com.unoone.pai.vault.android

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.hardware.usb.UsbManager
import android.os.Environment

/**
 * BroadcastReceiver for USB connection/disconnection events.
 *
 * Monitors:
 * - USB drive mount/unmount (via StorageManager / broadcast)
 * - USB device attach/detach (via UsbManager)
 *
 * When a USB drive is connected:
 * 1. Check if it's a UnoOne Pocket vault
 * 2. If yes, show the unlock screen
 * 3. If no, ignore (or offer to set up a new vault)
 *
 * When the USB drive is disconnected:
 * 1. Flush pending writes
 * 2. Close the vault database
 * 3. Clear encryption keys from memory
 * 4. Clear temporary decrypted cache
 * 5. Lock the application
 * 6. Show "UnoOne vault disconnected" message
 */
class UsbEventReceiver : BroadcastReceiver() {

    var onVaultConnected: ((vaultRoot: String) -> Unit)? = null
    var onVaultDisconnected: (() -> Unit)? = null

    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            Intent.ACTION_MEDIA_MOUNTED -> {
                val mountPath = intent.data?.path
                if (mountPath != null) {
                    // Check if this is a UnoOne vault
                    val vaultDir = java.io.File(mountPath, "UNOONE")
                    val vaultId = java.io.File(vaultDir, "VAULT/identity/vault.id")
                    if (vaultId.exists()) {
                        onVaultConnected?.invoke(vaultDir.absolutePath)
                    }
                }
            }
            Intent.ACTION_MEDIA_UNMOUNTED,
            Intent.ACTION_MEDIA_EJECT,
            Intent.ACTION_MEDIA_REMOVED -> {
                // USB drive disconnected — emergency lock
                onVaultDisconnected?.invoke()
            }
            UsbManager.ACTION_USB_DEVICE_DETACHED -> {
                // USB device detached — emergency lock
                onVaultDisconnected?.invoke()
            }
        }
    }

    companion object {
        /**
         * Create an IntentFilter for USB events.
         */
        fun createIntentFilter(): IntentFilter {
            return IntentFilter().apply {
                addAction(Intent.ACTION_MEDIA_MOUNTED)
                addAction(Intent.ACTION_MEDIA_UNMOUNTED)
                addAction(Intent.ACTION_MEDIA_EJECT)
                addAction(Intent.ACTION_MEDIA_REMOVED)
                addDataType("*/*")  // Required for MEDIA_MOUNTED
                addAction(UsbManager.ACTION_USB_DEVICE_ATTACHED)
                addAction(UsbManager.ACTION_USB_DEVICE_DETACHED)
            }
        }
    }
}