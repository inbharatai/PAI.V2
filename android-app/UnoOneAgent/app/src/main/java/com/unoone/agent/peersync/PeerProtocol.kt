package com.unoone.agent.peersync

import com.unoone.agent.core.personal.*
import com.unoone.agent.personal.PersonalAction
import com.unoone.agent.personal.PersonalLedger
import com.unoone.agent.personal.SharedLedger
import com.unoone.agent.personal.SharedIdentity
import com.unoone.agent.personal.SharedOperation
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.*
import java.security.MessageDigest
import java.util.UUID

/** Same bounded inert wire protocol as packages/local-peer-sync. No Request replay or grants. */
object PeerProtocol {
    const val MAX_BODY = 262144
    const val MAX_STORE = 4 * 1024 * 1024
    val json = Json { encodeDefaults = true; ignoreUnknownKeys = false }
    fun hash(b: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(b).joinToString("") { "%02x".format(it.toInt() and 255) }
    fun uuid(s: String) { require(UUID.fromString(s).toString() == s) { "Invalid identifier" } }
    fun parseOffer(text: String): PeerOffer { preflight(text, 2048); return json.decodeFromString<PeerOffer>(text).also { it.validate() } }
    // Same duplicate-key/nesting preflight pattern as the existing PersonalCodec.
    fun preflight(input: String, max: Int) {
        require(input.toByteArray().size <= max)
        val stack = mutableListOf<MutableSet<String>?>(); var i = 0
        while (i < input.length) {
            when (input[i]) {
                '{' -> { stack.add(mutableSetOf()); require(stack.size <= 16) }
                '[' -> { stack.add(null); require(stack.size <= 16) }
                '}', ']' -> { require(stack.isNotEmpty()); stack.removeAt(stack.lastIndex) }
                '"' -> {
                    val start = i++; var escaped = false
                    while (i < input.length) { val c = input[i]; if (!escaped && c == '"') break; escaped = !escaped && c == '\\'; i++ }
                    require(i < input.length); var next = i + 1
                    while (next < input.length && input[next].isWhitespace()) next++
                    if (next < input.length && input[next] == ':') { val key = json.decodeFromString<String>(input.substring(start, i + 1)); require(stack.lastOrNull()?.add(key) == true) { "Duplicate/invalid JSON key" } }
                }
            }; i++
        }; require(stack.isEmpty())
    }
}
@Serializable data class PeerOffer(val version: Int, val replica_id: String, val person_id: String, val agent_id: String, val fingerprint: String) {
    fun validate() { require(version == 1); listOf(replica_id, person_id, agent_id).forEach(PeerProtocol::uuid); require(fingerprint.matches(Regex("[0-9a-f]{64}"))) { "Compare complete SHA256 fingerprint" } }
}
@Serializable enum class IdentityChoice { SAME_PERSON, KEEP_SEPARATE_REVIEW, UNIFY_ARCHIVE }
@Serializable data class PeerSelection(val persona: Boolean, val task_ids: List<String>) {
    fun validate() { require(task_ids.size <= 128 && task_ids.distinct().size == task_ids.size); task_ids.forEach(PeerProtocol::uuid) }
}
@Serializable data class PeerApproval(val offer: PeerOffer, val choice: IdentityChoice, val selection: PeerSelection, val revoked: Boolean = false)
@Serializable data class PeerTaskState(val task_id: String, val action: PersonalAction, val draft: String, val snooze_until_ms: Long?)
@Serializable data class PeerPayload(val records: List<JsonObject>, val task_state: PeerTaskState?)
@Serializable data class PeerChange(val sequence: Long, val operation_id: String, val predecessor_operation_id: String?, val payload: String?, val content_hash: String) {
    fun validate(owner: PeerOffer) {
        require(sequence in 1..2048); PeerProtocol.uuid(operation_id); predecessor_operation_id?.let(PeerProtocol::uuid)
        require(content_hash == PeerProtocol.hash((payload ?: "").toByteArray())) { "Payload hash mismatch" }
        payload?.let { raw ->
            PeerProtocol.preflight(raw, 65536)
            if ("\"context\"" in raw) {
                val op = PeerProtocol.json.decodeFromString<SharedOperation>(raw)
                require(op.replica_id == owner.replica_id && op.sequence == sequence && op.operation_id == operation_id && op.predecessor_operation_id == predecessor_operation_id)
                return
            }
            val p = PeerProtocol.json.decodeFromString<PeerPayload>(raw); require(p.records.size <= 8)
            p.records.forEach { v ->
                val valid = when (val r = PersonalCodec.decode(v.toString()).record) {
                    is PersonalAgent -> r.person_id == owner.person_id && r.agent_id == owner.agent_id
                    is Persona -> r.person_id == owner.person_id && r.provenance.replica_id == owner.replica_id
                    is TaskSpec -> r.person_id == owner.person_id && r.agent_id == owner.agent_id && r.origin_replica_id == owner.replica_id && r.target_replica_id == owner.replica_id
                    is TaskEvent -> r.origin_replica_id == owner.replica_id && r.assigned_replica_id == owner.replica_id
                    else -> false
                }; require(valid) { "Unsupported record or owner mismatch; no grants/provider data" }
            }
            p.task_state?.let { t -> PeerProtocol.uuid(t.task_id); require(t.draft.toByteArray().size <= 4096 && t.action !in setOf(PersonalAction.PERSONA, PersonalAction.CLEAR_PERSONA)); require(t.snooze_until_ms == null || t.snooze_until_ms in 0..9007199254740991) }
        }
    }
}
@Serializable data class PeerPage(val version: Int, val sender: PeerOffer, val choice: IdentityChoice, val after: Long, val changes: List<PeerChange>)
@Serializable data class PeerExchange(val page: PeerPage, val want_after: Long)
@Serializable data class PeerReply(val page: PeerPage, val acknowledged: Long)
@OptIn(kotlinx.serialization.ExperimentalSerializationApi::class)
@Serializable data class PeerState(val version: Int, val vault_id: String, val local: PeerOffer, val key_alias: String,
    val peer: PeerApproval? = null, val received: List<PeerChange> = emptyList(), @kotlinx.serialization.EncodeDefault(kotlinx.serialization.EncodeDefault.Mode.NEVER) val peer_confirmed: Boolean = false, val sent_ack: Long = 0) {
    fun active(): PeerApproval = requireNotNull(peer) { "Approve full fingerprint on both screens first" }.also { require(!it.revoked) { "Peer revoked; previous copies cannot be remotely erased" } }
    fun approve(offer: PeerOffer, selection: PeerSelection, choice: IdentityChoice, compared: Boolean, ledger: PersonalLedger): PeerState {
        require(compared) { "Compare entire fingerprint on both screens first" }; require(peer == null) { "No silent replacement/history reset" }
        offer.validate(); selection.validate()
        require(offer.replica_id != local.replica_id && offer.fingerprint != local.fingerprint) { "Independent replicas/keys required" }
        require(choice != IdentityChoice.SAME_PERSON || (offer.person_id == local.person_id && offer.agent_id == local.agent_id)) { "IDENTITY_CONFLICT: independent identities; keep separate for review or cancel. Adoption requires reviewed migration; nothing overwritten" }
        val known = ledger.mutations.mapNotNull { it.request?.task_id }.toSet(); require(selection.task_ids.all { it in known || (choice == IdentityChoice.UNIFY_ARCHIVE && it == "00000000-0000-0000-0000-000000000000") })
        return copy(peer = PeerApproval(offer, choice, selection))
    }
    fun sharedIdentity(): SharedIdentity {
        val p = active(); require(p.choice == IdentityChoice.UNIFY_ARCHIVE)
        val founder = if (local.replica_id < p.offer.replica_id) local else p.offer
        return SharedIdentity(founder.person_id, founder.agent_id, founder.replica_id, sortedMapOf(local.replica_id to local.fingerprint, p.offer.replica_id to p.offer.fingerprint))
    }
    fun mergeInto(ledger: PersonalLedger): PersonalLedger {
        if (active().choice != IdentityChoice.UNIFY_ARCHIVE) return ledger
        require(peer_confirmed) { "Authenticated matching confirmation from both screens required" }
        val adopted = if (ledger.shared == null) ledger.adopt(sharedIdentity(), true) else ledger
        require(adopted.shared?.identity == sharedIdentity())
        return adopted.importShared(active().offer.replica_id, received.map { PeerProtocol.json.decodeFromString<SharedOperation>(requireNotNull(it.payload)) })
    }
    fun page(ledger: PersonalLedger, after: Long): PeerPage {
        if (active().choice == IdentityChoice.UNIFY_ARCHIVE) {
            require(ledger.local_vault_id == vault_id && ledger.replica_id == local.replica_id)
            val staged = if (ledger.shared == null) ledger.adopt(sharedIdentity(), true) else ledger
            require(staged.shared?.identity == sharedIdentity()); staged.view()
            val ops = staged.shared!!.operations[ledger.replica_id].orEmpty(); require(after in 0..ops.size.toLong())
            val selection = active().selection; val changes = mutableListOf<PeerChange>()
            for (op in ops.drop(after.toInt()).take(8)) {
                val selected = op.body?.let { b -> b.task_id?.let { it in selection.task_ids || "00000000-0000-0000-0000-000000000000" in selection.task_ids } ?: selection.persona } == true
                val raw = PeerProtocol.json.encodeToString(if (selected) op else op.copy(body = null))
                val ch = PeerChange(op.sequence, op.operation_id, op.predecessor_operation_id, raw, PeerProtocol.hash(raw.toByteArray())); ch.validate(local); changes += ch
                if (PeerProtocol.json.encodeToString(changes).toByteArray().size > PeerProtocol.MAX_BODY / 2) { changes.removeAt(changes.lastIndex); break }
            }
            require(after == ops.size.toLong() || changes.isNotEmpty())
            return PeerPage(2, local, IdentityChoice.UNIFY_ARCHIVE, after, changes)
        }
        val p = active(); val v = ledger.view()
        require(ledger.local_vault_id == vault_id && ledger.replica_id == local.replica_id && v.agent.person_id == local.person_id)
        require(after in 0..ledger.mutations.size.toLong()) { "Cursor ahead of authoritative ledger" }
        val changes = mutableListOf<PeerChange>()
        for (m in ledger.mutations.drop(after.toInt()).take(8)) {
            val selected = m.request?.task_id?.let { it in p.selection.task_ids } ?: p.selection.persona
            val payload = if (selected) PeerProtocol.json.encodeToString(PeerPayload(m.records, m.request?.let { r -> r.task_id?.let { PeerTaskState(it, r.action, r.draft, r.snooze_until_ms) } })) else null
            val ch = PeerChange(m.sequence, m.operation_id, m.predecessor_operation_id, payload, PeerProtocol.hash((payload ?: "").toByteArray())); ch.validate(local); changes += ch
            if (PeerProtocol.json.encodeToString(changes).toByteArray().size > PeerProtocol.MAX_BODY / 2) { changes.removeAt(changes.lastIndex); break }
        }
        require(after == ledger.mutations.size.toLong() || changes.isNotEmpty())
        return PeerPage(1, local, p.choice, after, changes)
    }
    fun receive(page: PeerPage): PeerState {
        val p = active()
        require(page.version == (if (p.choice == IdentityChoice.UNIFY_ARCHIVE) 2 else 1) && page.sender == p.offer && page.choice == p.choice) { "Pairing/identity choice/version mismatch; review on both screens" }
        require(page.changes.size <= 8 && page.after in 0..received.size.toLong()) { "Gap/page bound" }
        val next = received.toMutableList()
        page.changes.forEachIndexed { offset, ch ->
            ch.validate(p.offer); require(ch.sequence == page.after + offset + 1)
            val old = next.getOrNull(ch.sequence.toInt() - 1)
            if (old != null) require(old == ch) { "Immutable history conflict; original retained" }
            else { require(ch.predecessor_operation_id == next.lastOrNull()?.operation_id); require(next.none { it.operation_id == ch.operation_id }); next += ch }
        }
        if (p.choice == IdentityChoice.UNIFY_ARCHIVE) SharedLedger(sharedIdentity(), mapOf(p.offer.replica_id to next.map { PeerProtocol.json.decodeFromString<SharedOperation>(requireNotNull(it.payload)) })).validate()
        return copy(received = next, peer_confirmed = true).also { require(next.size <= 2048 && PeerProtocol.json.encodeToString(it).toByteArray().size <= PeerProtocol.MAX_STORE); it.remoteTasks() }
    }
    fun personaReview(): String? {
        if (peer?.choice == IdentityChoice.UNIFY_ARCHIVE) return null
        var name = ""; var persona: Persona? = null
        received.forEach { ch -> ch.payload?.let { raw -> PeerProtocol.json.decodeFromString<PeerPayload>(raw).records.forEach { v -> when (val r = PersonalCodec.decode(v.toString()).record) { is PersonalAgent -> name = r.display_name; is Persona -> persona = r; else -> Unit } } } }
        return persona?.let { p -> if (p.deleted) "Peer preferences cleared (tombstone). Local persona unchanged." else name + " — " + p.preferences.joinToString("\n") { it.value } + " (PEER_REVIEW_ONLY; local persona unchanged)" }
    }
    fun remoteTasks(): List<PeerTask> {
        if (peer?.choice == IdentityChoice.UNIFY_ARCHIVE) return emptyList()
        val tasks = sortedMapOf<String, PeerTask>()
        received.forEach { ch -> ch.payload?.let { raw ->
            val p = PeerProtocol.json.decodeFromString<PeerPayload>(raw)
            p.records.forEach { v -> val r = PersonalCodec.decode(v.toString()).record; if (r is TaskSpec) {
                val t = tasks.getOrPut(r.task_id) { PeerTask(r.task_id) }; if (!t.deleted) t.goal = r.goal
            } else if (r is TaskEvent) { val t = requireNotNull(tasks[r.task_id]) { "Task event predecessor missing" }; require(t.events.size < 1024); t.events += r } }
            p.task_state?.let { r -> val t = requireNotNull(tasks[r.task_id]) { "Task predecessor missing" }
                if (r.action == PersonalAction.DELETE) { t.deleted = true; t.goal = ""; t.draft = ""; t.snooze = null; t.status = "DELETED" }
                else if (!t.deleted) when (r.action) {
                    PersonalAction.CREATE, PersonalAction.EDIT -> { t.draft = r.draft; t.snooze = null }
                    PersonalAction.SNOOZE -> t.snooze = r.snooze_until_ms
                    PersonalAction.ACCEPT -> t.snooze = null
                    else -> Unit
                }
            }
        } }
        tasks.values.filterNot { it.deleted }.forEach { t -> val fold = foldTask(t.id, t.events, peer?.offer?.replica_id); t.status = "PEER_REVIEW_ONLY / " + (fold.transition?.name ?: fold.state.name) }
        return tasks.values.toList()
    }
}
data class PeerTask(val id: String, var goal: String = "", var draft: String = "", var snooze: Long? = null, var deleted: Boolean = false, var status: String = "PEER_REVIEW_ONLY", val executeOnHydration: Boolean = false, val events: MutableList<TaskEvent> = mutableListOf())
