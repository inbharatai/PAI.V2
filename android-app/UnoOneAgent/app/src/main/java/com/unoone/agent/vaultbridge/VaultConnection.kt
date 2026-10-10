package com.unoone.agent.vaultbridge

import android.content.Context
import android.net.Uri
import android.system.Os
import android.system.OsConstants
import com.unoone.agent.vault.*
import java.io.File

/** Default writable destination is this installation's private file vault, never SAF/Power.
 * All session-bound calls serialize with lock/replace. Previously handed-out handles fail
 * after lock instead of encrypting with a zeroized key. Password byte arrays are consumed.
 * Room remains independently encrypted and usable while this extra file vault is locked.
 */
object VaultConnection {
    private val revocation = java.util.concurrent.atomic.AtomicLong()
    private var sessionGeneration = -1L
    fun revoke(): Long = revocation.incrementAndGet()
    /** Read-only epoch for user-invoked socket sessions; lock/unlock invalidates old work. */
    fun sessionEpoch(): Long = revocation.get()
    @Synchronized fun closeRevoked(ticket: Long) { if (revocation.get() == ticket) lock() }

    private var repository: MobileVaultRepository? = null
    private var session: VaultSession? = null
    private var privateIO: PrivateFileVaultIO? = null
    private var bridgeAllowed = false
    @Volatile private var legacyRepository: MobileVaultRepository? = null

    @Synchronized fun prepareLocal(context: Context): Boolean {
        if (privateIO == null) {
            val io = PrivateFileVaultIO(File(context.noBackupFilesDir.canonicalFile, "local-file-vault"), ::syncDirectory)
            privateIO = io
            repository = MobileVaultRepository(io)
        }
        return privateIO!!.exists("VAULT/header/header_a.json") || privateIO!!.exists("VAULT/header/header_b.json")
    }

    /** Only after checking historical Room links/pending operations, without modifying them. */
    @Synchronized fun createLocal(password: ByteArray, historicalLinksOrPending: Boolean) {
        var opened: VaultSession? = null
        val ticket = revocation.incrementAndGet()
        try {
            check(session == null)
            opened = checkNotNull(repository).create(password)
            // Durable local binding, NOT a capability grant and never accepted from peer JSON.
            val binding = "${opened.vaultId}\n${if (historicalLinksOrPending) "migration-required" else "local"}\n"
            privateIO!!.write("room-binding.txt", binding.toByteArray())
            check(ticket == revocation.get()) { "Vault opening cancelled by lock" }
            sessionGeneration = ticket
            session = opened
            bridgeAllowed = !historicalLinksOrPending
        } catch (e: Exception) {
            opened?.close()
            throw e
        } finally { password.fill(0) }
    }

    @Synchronized fun unlock(password: ByteArray): Boolean {
        lock()
        val ticket = revocation.get()
        return try {
            val opened = checkNotNull(repository).unlock(password)
            if (ticket != revocation.get()) { opened.close(); error("Vault opening cancelled by lock") }
            sessionGeneration = ticket
            session = opened
            bridgeAllowed = try {
                String(privateIO!!.read("room-binding.txt"), Charsets.UTF_8) == "${opened.vaultId}\nlocal\n"
            } catch (_: Exception) { false } // interruption/unknown binding never drains historical work
            true
        } finally { password.fill(0) }
    }

    @Synchronized fun isUnlocked(): Boolean = session != null && sessionGeneration == revocation.get()
    @Synchronized fun isBridgeAllowed(): Boolean = isUnlocked() && bridgeAllowed
    @Synchronized fun localVaultId(): String? = session?.vaultId

    @Synchronized fun writer(): VaultRecordWriter? {
        val active = session ?: return null
        if (!isBridgeAllowed()) return null
        val repo = repository ?: return null
        return object : VaultRecordWriter {
            override fun writeRecord(fields: Map<String, Any?>, content: ByteArray): String = synchronized(this@VaultConnection) {
                check(session === active && isBridgeAllowed()) { "Vault locked or changed" }
                repo.writeRecord(active, fields, content)
            }
            override fun tombstone(vaultRecordId: String, deletedAtIso: String) = synchronized(this@VaultConnection) {
                check(session === active && isBridgeAllowed()) { "Vault locked or changed" }
                repo.tombstoneRecord(active, vaultRecordId, deletedAtIso)
            }
        }
    }

    @Synchronized fun reader(): VaultRecordReader? {
        val active = session ?: return null
        if (!isBridgeAllowed()) return null
        val repo = repository ?: return null
        return object : VaultRecordReader {
            override fun listRecordMetadata(): List<Map<String, Any?>> = synchronized(this@VaultConnection) {
                check(session === active && isBridgeAllowed()) { "Vault locked or changed" }
                repo.listRecordMetadata(active)
            }
            override fun readRecord(recordId: String): Pair<Map<String, Any?>, ByteArray> = synchronized(this@VaultConnection) {
                check(session === active && isBridgeAllowed()) { "Vault locked or changed" }
                repo.readRecord(active, recordId)
            }
        }
    }

    @Synchronized fun lock() {
        revoke()
        session?.close()
        session = null
        bridgeAllowed = false
    }

    /** Optional legacy SOURCE only. Never replaces local writer or feeds hydration. */
    fun attach(context: Context, tree: Uri) {
        legacyRepository = MobileVaultRepository(SafVaultIO(context.applicationContext, tree, readOnly = true))
    }

    /** Authenticate records and return a proposal count only. Actual backup/transactional
     * migration remains a separate gate; no import, grant hydration or action occurs here. */
    fun inspectLegacy(password: ByteArray): Pair<Int, Int> {
        try {
            val repo = checkNotNull(legacyRepository) { "Select a legacy source first" }
            val opened = repo.unlock(password)
            try {
                var records = 0
                var tombstones = 0
                for (metadata in repo.listRecordMetadata(opened)) {
                    val id = metadata["record_id"] as? String ?: continue
                    val (verified, plaintext) = repo.readRecord(opened, id)
                    plaintext.fill(0)
                    records++
                    if (verified["tombstone"] == true) tombstones++
                }
                return records to tombstones
            } finally { opened.close() }
        } finally { password.fill(0) }
    }

    /** USB detach has no effect on independent local session/Room. */
    fun detach() { legacyRepository = null }

    private fun syncDirectory(directory: File) {
        val fd = Os.open(directory.absolutePath, OsConstants.O_RDONLY or OsConstants.O_NOFOLLOW, 0)
        try {
            require(OsConstants.S_ISDIR(Os.fstat(fd).st_mode)) { "Vault sync target is not a directory" }
            Os.fsync(fd)
        } finally { Os.close(fd) }
    }
}
