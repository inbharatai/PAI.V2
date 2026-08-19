package com.unoone.pai.vault.session

import com.unoone.pai.contracts.Platform
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.io.File
import java.net.NetworkInterface
import java.util.UUID

/**
 * Device session manager for vault access control.
 *
 * Tracks which devices have accessed the vault and when.
 * Implements vault-level file locking to prevent concurrent access
 * from multiple devices (though USB is normally connected to one device).
 */
class DeviceSessionManager(private val vaultRoot: File) {

    private val json = Json { prettyPrint = true; ignoreUnknownKeys = true }

    @Serializable
    data class SessionInfo(
        val sessionId: String,
        val deviceId: String,
        val platform: Platform,
        val deviceName: String,
        val startedAt: String,
        var lastActivityAt: String,
        var isActive: Boolean = true
    )

    @Serializable
    data class LockFile(
        val sessionId: String,
        val deviceId: String,
        val platform: Platform,
        val lockedAt: String,
        val pid: Long = ProcessHandle.current().pid(),
        val isActive: Boolean = true
    )

    private var currentSession: SessionInfo? = null

    /**
     * Generate a unique device ID based on hardware characteristics.
     * This ID is stable across reboots but unique per physical device.
     */
    fun generateDeviceId(): String {
        val builder = StringBuilder()
        try {
            // Use MAC addresses for device identification
            val interfaces = NetworkInterface.getNetworkInterfaces()
            while (interfaces.hasMoreElements()) {
                val ni = interfaces.nextElement()
                val mac = ni.hardwareAddress
                if (mac != null && !ni.isLoopback) {
                    builder.append(mac.joinToString("") { "%02x".format(it) })
                }
            }
        } catch (e: Exception) {
            // Fallback to random UUID if MAC addresses are not available
            return UUID.randomUUID().toString()
        }
        return if (builder.isEmpty()) {
            UUID.randomUUID().toString()
        } else {
            // Hash the MAC addresses for privacy
            val digest = java.security.MessageDigest.getInstance("SHA-256")
            val hash = digest.digest(builder.toString().encodeToByteArray())
            hash.take(16).joinToString("") { "%02x".format(it) }
        }
    }

    /**
     * Create a new device session.
     * Called when the vault is unlocked on a device.
     */
    fun createSession(platform: Platform, deviceName: String): SessionInfo {
        val sessionId = UUID.randomUUID().toString()
        val deviceId = generateDeviceId()

        val session = SessionInfo(
            sessionId = sessionId,
            deviceId = deviceId,
            platform = platform,
            deviceName = deviceName,
            startedAt = java.time.Instant.now().toString(),
            lastActivityAt = java.time.Instant.now().toString()
        )

        currentSession = session
        writeLockFile(session)
        return session
    }

    /**
     * Update the last activity timestamp.
     * Called periodically to indicate the vault is still in use.
     */
    fun updateActivity() {
        currentSession?.let {
            it.lastActivityAt = java.time.Instant.now().toString()
        }
    }

    /**
     * End the current session.
     * Called when the vault is locked or the USB is disconnected.
     */
    fun endSession() {
        currentSession?.let { session ->
            session.isActive = false
            // Remove lock file
            val lockFile = File(vaultRoot, "identity/vault.lock")
            lockFile.delete()
        }
        currentSession = null
    }

    /**
     * Check if another device has an active lock on the vault.
     * Returns the lock info if locked, null if unlocked.
     */
    fun checkLock(): LockFile? {
        val lockFile = File(vaultRoot, "identity/vault.lock")
        if (!lockFile.exists()) return null

        return try {
            val content = lockFile.readText()
            json.decodeFromString<LockFile>(content)
        } catch (e: Exception) {
            // Corrupt lock file — remove it
            lockFile.delete()
            null
        }
    }

    /**
     * Force unlock the vault (removes existing lock).
     * Should only be used after confirming with the user.
     */
    fun forceUnlock() {
        val lockFile = File(vaultRoot, "identity/vault.lock")
        lockFile.delete()
    }

    /**
     * Get the current session info.
     */
    fun getCurrentSession(): SessionInfo? = currentSession

    /**
     * Check if the vault is currently locked by another session.
     */
    fun isLockedByAnother(): Boolean {
        val lock = checkLock() ?: return false
        val current = currentSession ?: return lock.isActive
        return lock.sessionId != current.sessionId && lock.isActive
    }

    private fun writeLockFile(session: SessionInfo) {
        val lockDir = File(vaultRoot, "identity")
        lockDir.mkdirs()
        val lockFile = File(lockDir, "vault.lock")

        val lock = LockFile(
            sessionId = session.sessionId,
            deviceId = session.deviceId,
            platform = session.platform,
            lockedAt = java.time.Instant.now().toString()
        )

        lockFile.writeText(json.encodeToString(lock))
    }
}