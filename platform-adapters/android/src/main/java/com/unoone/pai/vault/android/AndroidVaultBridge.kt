package com.unoone.pai.vault.android

import com.unoone.pai.contracts.*
import com.unoone.pai.vault.PocketMemoryVaultImpl
import com.unoone.pai.vault.android.usb.UsbVaultConnector
import com.unoone.pai.vault.crypto.VaultCipher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

/**
 * Bridge between the Android app and the PocketMemoryVault.
 *
 * This adapter:
 * 1. Detects USB vault connection
 * 2. Bridges between Room (temporary cache) and PocketMemoryVault (canonical)
 * 3. Handles USB connect/disconnect lifecycle
 * 4. Manages vault state (locked/unlocked)
 *
 * The Room database becomes a temporary cache. All writes go to the
 * USB vault first, then to Room for fast access. On USB disconnect,
 * Room data remains available in read-only mode.
 */
class AndroidVaultBridge(private val context: Context) {

    private var vault: PocketMemoryVaultImpl? = null
    private var vaultRoot: File? = null
    private var isVaultConnected = false
    private var isVaultUnlocked = false

    /**
     * Called when a USB drive is connected.
     * Detects if it's a valid UnoOne vault and prepares for unlock.
     *
     * @return VaultDetectionResult with vault info, or null if no vault found
     */
    suspend fun onUsbConnected(): VaultDetectionResult = withContext(Dispatchers.IO) {
        val connector = UsbVaultConnector(context)
        val result = connector.detectVault()

        if (result.detected && result.vaultRoot != null) {
            vaultRoot = result.vaultRoot
            isVaultConnected = true
        }

        result
    }

    /**
     * Called when the USB drive is disconnected.
     * Flushes writes, closes the vault, clears keys from memory.
     */
    suspend fun onUsbDisconnected() = withContext(Dispatchers.IO) {
        try {
            vault?.lockVault()
        } catch (_: Exception) {
            // Best effort — vault may already be inaccessible
        }

        vault = null
        vaultRoot = null
        isVaultConnected = false
        isVaultUnlocked = false

        // Clear any decrypted cache data from internal storage
        clearDecryptedCache()
    }

    /**
     * Unlock the vault with a password.
     * Called after USB detection when the user enters their password.
     *
     * @return VaultUnlockResult indicating success/failure
     */
    suspend fun unlockVault(password: String): VaultUnlockResult = withContext(Dispatchers.IO) {
        val root = vaultRoot
        if (root == null) {
            return@withContext VaultUnlockResult(
                success = false,
                error = "No vault detected — please connect your UnoOne Pocket USB"
            )
        }

        val vaultImpl = PocketMemoryVaultImpl(root)
        val result = vaultImpl.unlockVault(password)

        if (result.success) {
            vault = vaultImpl
            isVaultUnlocked = true
        }

        result
    }

    /**
     * Set up a new vault on the connected USB drive.
     * Called on first-time setup.
     *
     * @return VaultSetupResult with vault ID and recovery key
     */
    suspend fun setupVault(password: String, profileName: String?): VaultSetupResult = withContext(Dispatchers.IO) {
        val root = vaultRoot
        if (root == null) {
            return@withContext VaultSetupResult(
                success = false,
                error = "No USB drive detected — please connect your UnoOne Pocket USB"
            )
        }

        val vaultImpl = PocketMemoryVaultImpl(root)
        val result = vaultImpl.setupVault(password, profileName)

        if (result.success) {
            vault = vaultImpl
            isVaultUnlocked = true
        }

        result
    }

    /**
     * Emergency lock — called when USB is removed unexpectedly.
     * Immediately clears all keys from memory without graceful shutdown.
     */
    fun emergencyLock() {
        vault?.emergencyLock()
        vault = null
        isVaultUnlocked = false
    }

    /**
     * Check if the vault is currently connected and unlocked.
     */
    fun isReady(): Boolean = isVaultConnected && isVaultUnlocked && vault != null

    /**
     * Get the current vault instance (null if not unlocked).
     */
    fun getVault(): PocketMemoryVaultImpl? = vault

    /**
     * Clear decrypted cache data from internal storage.
     * Called on vault disconnect to ensure no plaintext data remains on the device.
     */
    private fun clearDecryptedCache() {
        // In a full implementation, this would clear the Room database cache
        // and any temporary decrypted files from internal storage.
        // For now, we mark the vault as disconnected and the app will
        // show a "Connect UnoOne Pocket" screen.
    }
}

/**
 * Placeholder for Android Context (to be replaced with android.content.Context
 * when building with the Android SDK).
 */
typealias Context = android.content.Context