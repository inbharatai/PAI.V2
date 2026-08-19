package com.unoone.agent.vaultbridge

import android.content.Context
import android.net.Uri
import com.unoone.agent.vault.MobileVaultRepository
import com.unoone.agent.vault.SafVaultIO
import com.unoone.agent.vault.VaultRecordWriter
import com.unoone.agent.vault.VaultSession

/**
 * App-wide holder for the live shared-vault session.
 *
 * Lifecycle: [attach] after the SAF tree is granted and validated
 * (MainActivity), [unlock] with the user password to open a [VaultSession]
 * (Argon2id is slow — call OFF the main thread), [detach] on USB disconnect to
 * zeroize the master key. The drive vault is authoritative; this object is the
 * single point that knows whether writes can currently reach it.
 *
 * [writer] returns null whenever the vault is not attached+unlocked, so the
 * [VaultMirror] coordinator naturally falls back to cache-only writes that are
 * flushed on the next unlock.
 */
object VaultConnection {

    private var repository: MobileVaultRepository? = null
    private var session: VaultSession? = null

    @Synchronized
    fun attach(context: Context, tree: Uri) {
        clearSession() // a new tree invalidates any prior session
        repository = MobileVaultRepository(SafVaultIO(context.applicationContext, tree))
    }

    /**
     * Open a session from the attached tree. Returns true on success, false on
     * a wrong password or corrupt header (no throw), so callers can surface a
     * retry without crashing. MUST run off the main thread (Argon2id).
     */
    @Synchronized
    fun unlock(password: ByteArray): Boolean {
        val repo = repository ?: return false
        return try {
            session = repo.unlock(password)
            true
        } catch (_: Exception) {
            // Wrong password, tampered/corrupt header, or any I/O failure —
            // all mean "not unlocked". Never crash the caller thread.
            session = null
            false
        }
    }

    @Synchronized
    fun isUnlocked(): Boolean = session != null

    /** A writer bound to the current session, or null when locked/detached. */
    @Synchronized
    fun writer(): VaultRecordWriter? {
        val repo = repository ?: return null
        val active = session ?: return null
        return SessionVaultRecordWriter(repo, active)
    }

    /** USB detach: drop the tree and zeroize the master key. */
    @Synchronized
    fun detach() {
        clearSession()
        repository = null
    }

    private fun clearSession() {
        session?.masterKey?.fill(0)
        session = null
    }
}

/** Binds the narrow [VaultRecordWriter] surface to an unlocked session. */
private class SessionVaultRecordWriter(
    private val repository: MobileVaultRepository,
    private val session: VaultSession,
) : VaultRecordWriter {

    override fun writeRecord(fields: Map<String, Any?>, content: ByteArray): String =
        repository.writeRecord(session, fields, content)

    override fun tombstone(vaultRecordId: String, deletedAtIso: String) {
        repository.tombstoneRecord(session, vaultRecordId, deletedAtIso)
    }
}
