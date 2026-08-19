package com.unoone.pai.vault.android.usb

import android.content.Context
import android.hardware.usb.UsbManager
import android.net.Uri
import android.os.storage.StorageManager
import android.os.storage.StorageVolume
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import java.io.File
import java.io.InputStream
import java.io.OutputStream

/**
 * Detects and manages UnoOne Pocket USB drive connections on Android.
 *
 * Uses Android's Storage Access Framework (SAF) for USB drive access
 * and USB Host API for direct USB communication where available.
 *
 * Flow:
 * 1. Detect USB connection via StorageManager / broadcast receiver
 * 2. Verify it's a valid UnoOne vault (check for VAULT/identity/vault.id)
 * 3. Present password-only unlock screen
 * 4. Derive key and unlock vault
 * 5. Monitor USB disconnection → flush, lock, clear cache
 */
class UsbVaultConnector(private val context: Context) {

    companion object {
        /** Marker file that identifies a UnoOne Pocket USB */
        private const val VAULT_ID_FILE = "VAULT/identity/vault.id"

        /** Marker file for vault header */
        private const val VAULT_HEADER_FILE = "VAULT/identity/vault.json.enc"

        /** UnoOne directory name on the USB drive */
        private const val UNOONE_DIR = "UNOONE"
    }

    /**
     * Check if a USB drive is connected and contains a UnoOne vault.
     *
     * TRUTHFULNESS: every detection attempt reports WHY it could not succeed.
     * On Android 11+ (API 30+), raw file scanning of other apps' volumes is
     * impossible without MANAGE_EXTERNAL_STORAGE — which this app deliberately
     * does not request (see AndroidManifest.xml). The supported path on
     * Android 10+ is the Storage Access Framework grant that MainActivity
     * obtains when the system offers the USB drive (USB_DEVICE_ATTACHED →
     * ACTION_OPEN_DOCUMENT_TREE → takePersistableUriPermission).
     *
     * @return a result where `detected=false` always carries `reason`.
     */
    fun detectVault(): VaultDetectionResult {
        val reasons = mutableListOf<String>()

        // Method 1: Direct file access below Android 11. On API 30+ this path
        // is blocked by scoped storage, so we skip it and say so — running it
        // "anyway" and enumerating nothing was making permission denial look
        // like a hardware fault.
        if (android.os.Build.VERSION.SDK_INT <= 28) {
            val externalDirs = context.getExternalFilesDirs(null)
            for (dir in externalDirs) {
                if (dir != null) {
                    val unooneDir = File(
                        dir.parentFile?.parentFile?.parentFile?.parentFile,
                        UNOONE_DIR
                    )
                    if (unooneDir.exists()) {
                        val vaultIdFile = File(unooneDir, VAULT_ID_FILE)
                        if (vaultIdFile.exists()) {
                            return VaultDetectionResult(
                                detected = true,
                                vaultRoot = unooneDir,
                                vaultId = vaultIdFile.readText().trim(),
                                connectionType = ConnectionType.DIRECT_FILE
                            )
                        }
                    }
                }
            }
            reasons.add("No UNOONE directory found on any direct-accessible volume")
        } else {
            reasons.add(
                "Direct file scanning unavailable on Android " +
                    "${android.os.Build.VERSION.SDK_INT} (scoped storage; MANAGE_EXTERNAL_STORAGE is not requested by design)"
            )
        }

        // The modern path: a SAF tree URI must have been granted by the user.
        // This connector cannot enumerate USB drives by itself on API 30+;
        // detection is only truthful when it reports that dependency.
        reasons.add(
            "Storage Access Framework: detection requires the USB_DEVICE_ATTACHED " +
                "flow (system offers the drive → user grants access → persistable URI)"
        )
        return VaultDetectionResult(
            detected = false,
            reason = reasons.joinToString("; ")
        )
    }

    /**
     * Check if the detected vault is valid (has required structure).
     */
    fun isValidVault(vaultRoot: File): Boolean {
        val vaultId = File(vaultRoot, VAULT_ID_FILE)
        val vaultHeader = File(vaultRoot, VAULT_HEADER_FILE)
        return vaultId.exists() && vaultHeader.exists()
    }

    /**
     * Get the vault root directory on the USB drive.
     * Returns null if no vault is detected.
     */
    fun getVaultRoot(): File? {
        val result = detectVault()
        return if (result.detected) result.vaultRoot else null
    }
}

data class VaultDetectionResult(
    val detected: Boolean,
    val vaultRoot: File? = null,
    val vaultId: String? = null,
    val connectionType: ConnectionType = ConnectionType.NONE,
    /** Truthful explanation when [detected] is false — never empty there. */
    val reason: String = ""
) {
    init {
        require(detected || reason.isNotEmpty()) {
            "A failed detection must explain why — silent detection failures " +
                "were previously indistinguishable from hardware faults."
        }
    }
}

enum class ConnectionType {
    NONE,           // No USB detected
    DIRECT_FILE,   // Direct file access (pre-Android 10 or desktop)
    SAF,            // Storage Access Framework (Android 10+)
    USB_HOST       // USB Host API (direct USB communication)
}