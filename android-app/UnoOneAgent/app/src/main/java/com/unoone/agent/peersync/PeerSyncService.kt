package com.unoone.agent.peersync

import android.content.Context
import com.unoone.agent.personal.PersonalLedger
import com.unoone.agent.personal.PersonalStore
import com.unoone.agent.personal.PersonalView
import com.unoone.agent.vault.VaultRecordReader
import com.unoone.agent.vault.VaultRecordWriter
import com.unoone.agent.vaultbridge.VaultConnection
import java.io.File
import java.net.Socket
import java.nio.file.Files
import java.nio.file.LinkOption.NOFOLLOW_LINKS
import java.nio.file.NoSuchFileException
import java.nio.file.attribute.BasicFileAttributes
import java.time.Instant
import java.util.concurrent.atomic.AtomicLong
import kotlinx.serialization.encodeToString

/** Explicit foreground/unlocked transactions only. Authoritative outbound data stays in
 * PersonalStore; this encrypted store holds pins, selection, inbound history and cursors. */
class PeerSyncService(context: Context) {
    companion object { const val RECORD_ID = "98e180e1-d876-44c7-b069-e83e6b703070" }
    private val root = File(context.applicationContext.noBackupFilesDir.canonicalFile, "local-file-vault")
    private val generation = AtomicLong()
    @Volatile private var socket: Socket? = null
    data class View(val state: PeerState, val local: PersonalView)
    private data class Session(val vault: String, val epoch: Long, val reader: VaultRecordReader, val writer: VaultRecordWriter, val ledger: PersonalLedger, val state: PeerState)
    fun stop() { generation.incrementAndGet(); runCatching { socket?.close() }; socket = null }
    private fun exists(id: String): Boolean {
        val f = File(root, "VAULT/records/$id.enc.json"); var a: File? = f
        while (a != null) { require(!Files.isSymbolicLink(a.toPath())); a = a.parentFile }
        require(f.canonicalFile == f.absoluteFile)
        return try { val meta = Files.readAttributes(f.toPath(), BasicFileAttributes::class.java, NOFOLLOW_LINKS); require(meta.isRegularFile && meta.size() <= 10 * 1024 * 1024); true }
        catch (_: NoSuchFileException) { false }
    }
    private fun session(): Session {
        check(VaultConnection.isBridgeAllowed()) { "Unlock local vault and resolve historical migration hold first" }
        val vault = checkNotNull(VaultConnection.localVaultId()); val reader = checkNotNull(VaultConnection.reader()); val writer = checkNotNull(VaultConnection.writer())
        val ledger = PersonalStore(vault, reader, writer) { exists(PersonalLedger.RECORD_ID) }.load()
        val state = if (!exists(RECORD_ID)) {
            val alias = "unoone-peer-$vault-v1" // failed first save leaves a detectable orphan; never silently rotate
            val v = ledger.view(); val fp = NativePeerHttps.generate(alias)
            PeerState(1, vault, PeerOffer(1, ledger.replica_id, v.agent.person_id, v.agent.agent_id, fp), alias).also { save(writer, it) }
        } else {
            val (meta, bytes) = reader.readRecord(RECORD_ID)
            try { require(meta["tombstone"] != true); val text = Charsets.UTF_8.newDecoder().decode(java.nio.ByteBuffer.wrap(bytes)).toString(); PeerProtocol.preflight(text, PeerProtocol.MAX_STORE); PeerProtocol.json.decodeFromString<PeerState>(text) }
            finally { bytes.fill(0) }
        }
        require(state.version == 1 && state.vault_id == vault && state.local.replica_id == ledger.replica_id && (state.local.person_id == ledger.view().agent.person_id || ledger.shared != null))
        state.local.validate(); require(state.received.size <= 2048); state.remoteTasks()
        require(NativePeerHttps.fingerprint(state.key_alias) == state.local.fingerprint) { "Pairing key lost or changed; no silent trust reset" }
        return Session(vault, VaultConnection.sessionEpoch(), reader, writer, ledger, state)
    }
    private fun save(writer: VaultRecordWriter, state: PeerState) {
        val bytes = PeerProtocol.json.encodeToString(state).toByteArray(); require(bytes.size <= PeerProtocol.MAX_STORE) { "Review store full; nothing evicted" }
        val now = Instant.now().toString()
        val metadata = mapOf<String, Any?>("record_id" to RECORD_ID, "record_type" to "CONTEXT_SNAPSHOT", "schema_version" to 1, "encryption_version" to 1,
            "created_at" to now, "updated_at" to now, "revision" to 1, "origin_platform" to "peer-local", "origin_device_id" to "local", "transaction_id" to PersonalLedger.id(),
            "content_hash" to "", "parent_record_id" to null, "source_record_ids" to emptyList<String>(), "privacy_level" to "PRIVATE", "tombstone" to false, "deleted_at" to null)
        try { writer.writeRecord(metadata, bytes) } finally { bytes.fill(0) }
    }
    fun view(): View = synchronized(VaultConnection) { val s = session(); View(s.state, s.ledger.view()) }
    fun approve(offer: String, selection: PeerSelection, choice: IdentityChoice, compared: Boolean): View = synchronized(VaultConnection) {
        val s = session(); val next = s.state.approve(PeerProtocol.parseOffer(offer), selection, choice, compared, s.ledger)
        save(s.writer, next); View(next, s.ledger.view())
    }
    fun revoke(): View { stop(); return synchronized(VaultConnection) { val s = session(); val next = s.state.copy(peer = requireNotNull(s.state.peer).copy(revoked = true)); save(s.writer, next); View(next, s.ledger.view()) } }
    fun sync(address: String): View {
        stop(); val ticket = generation.get()
        val s = synchronized(VaultConnection) { session() }; val page = s.state.page(s.ledger, s.state.sent_ack)
        fun allowed() = generation.get() == ticket && VaultConnection.sessionEpoch() == s.epoch && VaultConnection.isBridgeAllowed()
        val reply = NativePeerHttps.exchange(address, s.state, PeerExchange(page, s.state.received.size.toLong()), ::allowed) { socket = it }
        return synchronized(VaultConnection) {
            require(allowed()) { "Session ended; no persisted ACK" }
            // Session-bound writer/reader still reject lock/unlock replacement, even for same vault.
            val current = session(); require(current.epoch == s.epoch && current.state.active() == s.state.active())
            require(reply.acknowledged in current.state.sent_ack..(page.changes.lastOrNull()?.sequence ?: page.after)) { "Invalid peer ACK" }
            val next = current.state.receive(reply.page).copy(sent_ack = reply.acknowledged)
            val merged = next.mergeInto(current.ledger)
            PersonalStore(s.vault, s.reader, s.writer) { exists(PersonalLedger.RECORD_ID) }.save(merged)
            save(s.writer, next) // receive cursor and data ONE encrypted write; next request ACKs this cursor
            View(next, merged.view())
        }
    }
}
