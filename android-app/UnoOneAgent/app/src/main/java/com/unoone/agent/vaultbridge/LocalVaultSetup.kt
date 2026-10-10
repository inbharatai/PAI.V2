package com.unoone.agent.vaultbridge

import android.content.Context
import androidx.room.withTransaction
import com.unoone.agent.UnoOneApplication
import com.unoone.agent.di.DatabaseProvider
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.concurrent.atomic.AtomicLong

/** Small lifecycle coordinator; passwords never enter preferences, saved state, logs or Room. */
object LocalVaultSetup {
    data class State(
        val checked: Boolean = false,
        val exists: Boolean = false,
        val unlocked: Boolean = false,
        val busy: Boolean = false,
        val message: String? = null,
    )
    private val mutable = MutableStateFlow(State())
    val state = mutable.asStateFlow()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val generation = AtomicLong()
    @Volatile var retainWhileBackgrounded: Boolean = false // explicit per-process opt-in only
        private set

    fun retainInBackground(enabled: Boolean) { retainWhileBackgrounded = enabled }

    fun refresh(context: Context) {
        val app = context.applicationContext
        scope.launch {
            try {
                val exists = VaultConnection.prepareLocal(app)
                mutable.value = mutable.value.copy(checked = true, exists = exists, unlocked = VaultConnection.isUnlocked())
            } catch (_: Exception) {
                mutable.value = State(checked = true, message = "Local vault needs assisted recovery. Keep app data; do not reset or uninstall.")
            }
        }
    }

    fun open(context: Context, password: ByteArray, create: Boolean) {
        if (mutable.value.busy) { password.fill(0); return }
        val ticket = generation.incrementAndGet()
        val app = context.applicationContext as UnoOneApplication
        mutable.value = mutable.value.copy(busy = true, message = null)
        scope.launch {
            try {
                VaultConnection.prepareLocal(app)
                if (create) {
                    val db = DatabaseProvider.getDatabase(app)
                    // A legacy identity has no durable per-vault outbox namespace. Fail closed:
                    // never repoint old links or retire old pending work against this new key.
                    db.withTransaction {
                        // Hold the mutation boundary through durable binding. Built-in/note/turn
                        // backlog minted by v7 is FRESH, not evidence of another key root. v6 IDs,
                        // real links and historical tombstones remain quarantined. No payload scan.
                        val historical = db.pendingWriteDao().hasHistoricalAuthority()
                        VaultConnection.createLocal(password, historical)
                    }
                } else { VaultConnection.unlock(password) }
                if (generation.get() != ticket) {
                    VaultConnection.lock()
                    mutable.value = State(checked = true, exists = true)
                    return@launch
                }
                val allowed = VaultConnection.isBridgeAllowed()
                if (allowed) app.vaultMirror.drainBacklog()
                if (generation.get() != ticket || !VaultConnection.isUnlocked()) {
                    VaultConnection.lock()
                    mutable.value = State(checked = true, exists = true)
                    return@launch
                }
                // No legacy/remote hydration here: imported skill JSON must never convey grants.
                mutable.value = State(checked = true, exists = true, unlocked = true,
                    message = if (allowed) "Local file vault open. Changes use this phone's own vault; no peer sync is enabled."
                    else "File vault open, but historical Room links/pending work require a reviewed migration. They are retained unchanged; the file bridge is paused.")
            } catch (e: Exception) {
                VaultConnection.lock()
                mutable.value = State(checked = true,
                    exists = runCatching { VaultConnection.prepareLocal(app) }.getOrDefault(true),
                    message = if (e.message?.contains("memory", ignoreCase = true) == true)
                        "This device cannot currently allocate the existing 256 MiB vault password KDF plus overhead. Close the model/apps and retry. Local encrypted Room storage remains available; no weaker cipher was substituted."
                    else "Could not open the local file vault. Check the password. Interrupted or damaged files are retained for recovery; do not reset app data.")
            } finally { password.fill(0) }
        }
    }

    fun lock() {
        generation.incrementAndGet()
        val revoked = VaultConnection.revoke() // immediately invalidates outstanding handles
        mutable.value = mutable.value.copy(unlocked = false, message = "File vault locked; encrypted Room data and pending writes are retained.")
        // KDF can hold the connection monitor; never wait for it on the UI thread.
        scope.launch { VaultConnection.closeRevoked(revoked) }
    }

    fun backgrounded() { if (!retainWhileBackgrounded) lock() }
}
