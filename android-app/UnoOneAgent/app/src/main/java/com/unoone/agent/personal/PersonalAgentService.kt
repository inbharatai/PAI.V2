package com.unoone.agent.personal

import android.content.Context
import com.unoone.agent.vaultbridge.VaultConnection
import java.io.File
import java.nio.file.Files
import java.nio.file.LinkOption.NOFOLLOW_LINKS
import java.nio.file.NoSuchFileException
import java.nio.file.attribute.BasicFileAttributes

/** No durable plaintext cache and no execution methods. Honours the existing historical
 * migration quarantine; never drains/adopts old Room rows or grants into a new identity. */
class PersonalAgentService(context: Context) {
    private val appContext = context.applicationContext
    internal fun <T> withStore(block: (PersonalStore) -> T): T = synchronized(VaultConnection) {
        check(VaultConnection.isBridgeAllowed()) { "Unlock local vault first; historical migration quarantine must be resolved before use" }
        val vaultId = checkNotNull(VaultConnection.localVaultId())
        val reader = checkNotNull(VaultConnection.reader())
        val writer = checkNotNull(VaultConnection.writer())
        block(PersonalStore(vaultId, reader, writer) {
            // Listing skips malformed envelopes, so it MUST NOT establish absence.
            val root = File(appContext.noBackupFilesDir.canonicalFile, "local-file-vault")
            val file = File(root, "VAULT/records/${PersonalLedger.RECORD_ID}.enc.json")
            var ancestor: File? = file
            while (ancestor != null) { require(!Files.isSymbolicLink(ancestor.toPath())); ancestor = ancestor.parentFile }
            require(file.canonicalFile == file.absoluteFile)
            try { require(Files.readAttributes(file.toPath(), BasicFileAttributes::class.java, NOFOLLOW_LINKS).isRegularFile); true }
            catch (_: NoSuchFileException) { false }
        })
    }
    fun view(): PersonalView = withStore { it.load().view() }
    fun mutate(request: PersonalRequest): PersonalView = withStore { it.mutate(request, System.currentTimeMillis()) }
}
