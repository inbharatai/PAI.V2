package com.unoone.agent.personal

import com.unoone.agent.core.personal.*
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.*
import java.util.UUID

// Same encrypted aggregate as Rust personal-agent-runtime; no grant/scheduler/provider objects.
@Serializable enum class PersonalAction { PERSONA, CLEAR_PERSONA, CREATE, EDIT, ACCEPT, SNOOZE, CANCEL, DELETE }
@Serializable data class PersonalRequest(val operation_id: String, val expected_revision: Long, val action: PersonalAction,
    val task_id: String?, val text: String, val draft: String, val snooze_until_ms: Long?, val expected_replica_id: String)
@Serializable data class PersonalMutation(val sequence: Long, val operation_id: String, val predecessor_operation_id: String?,
    val request: PersonalRequest?, val records: List<JsonObject>)
@OptIn(kotlinx.serialization.ExperimentalSerializationApi::class)
@Serializable data class PersonalLedger(val schema: String, val version: Long, val local_vault_id: String,
    val replica_id: String, val mutations: List<PersonalMutation>, @kotlinx.serialization.EncodeDefault(kotlinx.serialization.EncodeDefault.Mode.NEVER) val shared: SharedLedger? = null) {
    companion object {
        const val RECORD_ID = "7683459b-5738-4d18-a824-b3485869673b"
        const val MAX_BYTES = 4 * 1024 * 1024
        private val json = Json { encodeDefaults = true; ignoreUnknownKeys = false }
        fun id(): String = UUID.randomUUID().toString()
        private fun uuid(value: String) { require(UUID.fromString(value).toString() == value) { "Invalid identifier" } }
        private fun text(value: String) { require(value.isNotBlank() && value.toByteArray().size <= 4096) { "Text must contain 1–4096 UTF-8 bytes" } }
        private fun doc(record: PersonalRecord) = Json.parseToJsonElement(PersonalCodec.encode(record)).jsonObject
        fun fresh(vaultId: String): PersonalLedger {
            uuid(vaultId)
            val replica = id(); val person = id()
            val agent = PersonalAgent(id(), person, 1, "UnoOne", 1, emptyList(), emptyList())
            val persona = Persona(person, 1, emptyList(), Sensitivity.PRIVATE, Provenance(ProvenanceSource.USER, person, replica, null), false, null)
            return PersonalLedger("inbharat.pai.personal-ledger", 1, vaultId, replica,
                listOf(PersonalMutation(1, id(), null, null, listOf(doc(agent), doc(persona)))))
        }
        fun decode(bytes: ByteArray, vaultId: String): PersonalLedger {
            require(bytes.size <= MAX_BYTES) { "Ledger full; nothing discarded" }
            val ledger = json.decodeFromString<PersonalLedger>(bytes.toString(Charsets.UTF_8))
            require(ledger.local_vault_id == vaultId) { "Vault binding mismatch; import/pairing required" }
            ledger.view(); return ledger
        }
    }
    fun bytes(): ByteArray {
        view()
        return json.encodeToString(this).toByteArray().also { require(it.size <= MAX_BYTES) { "Ledger full; no eviction. Reviewed sync/compaction required" } }
    }
    fun view(): PersonalView {
        if (version == 2L) { copy(version = 1, shared = null).view(); return requireNotNull(shared).view(this) }
        require(shared == null) { "V1 cannot contain migration state" }
        require(schema == "inbharat.pai.personal-ledger" && version == 1L) { "Unsupported ledger version" }
        uuid(local_vault_id); uuid(replica_id)
        require(mutations.size in 1..2048) { "Ledger mutation limit" }
        val seen = mutableSetOf<String>(); var previous: String? = null
        var agent: PersonalAgent? = null; var persona: Persona? = null
        val tasks = sortedMapOf<String, PersonalTask>()
        mutations.forEachIndexed { index, m ->
            uuid(m.operation_id)
            require(m.sequence == index + 1L && m.predecessor_operation_id == previous && seen.add(m.operation_id)) { "Mutation sequence/collision conflict" }
            require(m.records.size <= 8)
            m.request?.let { require(it.operation_id == m.operation_id && it.expected_revision == index.toLong() && it.expected_replica_id == replica_id) } ?: require(index == 0)
            previous = m.operation_id
            m.records.forEach { value ->
                when (val r = PersonalCodec.decode(value.toString()).record) {
                    is PersonalAgent -> { agent?.let { require(it.agent_id == r.agent_id && it.person_id == r.person_id) { "Identity replacement rejected" } }; agent = r }
                    is Persona -> { require(agent?.person_id == r.person_id); persona = r }
                    is TaskSpec -> {
                        require(r.person_id == agent?.person_id && r.agent_id == agent?.agent_id && r.origin_replica_id == replica_id && r.target_replica_id == replica_id) { "Task owner mismatch" }
                        val existing = tasks[r.task_id]
                        if (existing != null) { require(!existing.deleted) { "Tombstone prevents resurrection" }; existing.spec = r }
                        else { require(tasks.size < 128); tasks[r.task_id] = PersonalTask(r, ownerReplicaId = replica_id) }
                    }
                    is TaskEvent -> { val t = requireNotNull(tasks[r.task_id]); require(!t.deleted); t.events += r }
                    else -> error("Unsupported ledger record; no authority hydration")
                }
            }
            m.request?.let { r -> r.task_id?.let { tid ->
                val t = requireNotNull(tasks[tid]) { "Unknown task" }
                when (r.action) {
                    PersonalAction.CREATE, PersonalAction.EDIT -> { t.draft = r.draft; t.snoozeUntilMs = null }
                    PersonalAction.ACCEPT -> t.snoozeUntilMs = null
                    PersonalAction.SNOOZE -> t.snoozeUntilMs = r.snooze_until_ms
                    PersonalAction.DELETE -> { t.deleted = true; t.draft = "" }
                    else -> Unit
                }
            } }
        }
        tasks.values.forEach { t ->
            val projection = foldTask(t.spec.task_id, t.events, replica_id)
            require(projection.state == FoldState.TRANSITION) { "Causal task conflict; review required" }
            t.status = requireNotNull(projection.transition).name
            check(!projection.executeOnHydration)
        }
        return PersonalView(mutations.size.toLong(), replica_id, requireNotNull(agent), requireNotNull(persona), tasks.values.filterNot { it.deleted }, mutations.size)
    }
    fun apply(request: PersonalRequest, now: Long): PersonalLedger {
        uuid(request.operation_id)
        require(request.expected_replica_id == replica_id) { "Replica changed; reload before editing" }
        shared?.local_requests?.find { it.operation_id == request.operation_id }?.let { require(it == request); return this }
        mutations.find { it.operation_id == request.operation_id }?.let { require(it.request == request) { "Operation ID collision" }; return this }
        val v = view(); require(request.expected_revision == v.revision) { "Changed since opened; reload before editing" }
        require((shared?.operations?.values?.sumOf { it.size } ?: mutations.size) < 2048)
        require(request.text.toByteArray().size <= 4096 && request.draft.toByteArray().size <= 4096)
        val records = mutableListOf<JsonObject>()
        when (request.action) {
            PersonalAction.PERSONA, PersonalAction.CLEAR_PERSONA -> {
                require(request.task_id == null)
                val deleted = request.action == PersonalAction.CLEAR_PERSONA
                if (!deleted) text(request.text)
                require(shared == null || !v.persona.deleted || deleted) { "Shared persona tombstone; explicit new profile migration required" }
                val provenance = if (shared == null) v.persona.provenance else v.persona.provenance.copy(replica_id = replica_id)
                val p = v.persona.copy(provenance = provenance, revision = v.persona.revision + 1, corrects_revision = v.persona.revision,
                    deleted = deleted, preferences = if (deleted || request.draft.isEmpty()) emptyList() else listOf(Preference("response_preferences", request.draft, PreferenceStatus.CORRECTED, provenance)))
                val a = v.agent.copy(display_name = if (deleted) v.agent.display_name else request.text,
                    persona_revision = p.revision, profile_revision = v.agent.profile_revision + 1)
                records += doc(a); records += doc(p)
            }
            PersonalAction.CREATE -> {
                text(request.text); val tid = requireNotNull(request.task_id); uuid(tid)
                require(mutations.none { it.request?.task_id == tid }) { "Task ID already used" }
                var s = TaskSpec(tid, v.agent.agent_id, v.agent.person_id, request.operation_id, request.text, replica_id, replica_id, emptyList(),
                    Scopes(emptyList(), emptyList(), emptyList(), emptyList(), listOf(replica_id), listOf(Operation.READ, Operation.SUGGEST, Operation.DRAFT), DelegationLevel.PREPARE_DRAFTS, NetworkPolicy.OFFLINE_ONLY),
                    Budget(3, 0, 60000, 8192, 0, 1, 2), now, now + 86400000,
                    "Manual review only; no external action performed",
                    "Accept records your local review, not permission to execute. No automatic scheduling, model or provider adapter is connected.", Sensitivity.PRIVATE)
                if (shared != null) { require(shared.operations.values.flatten().none { it.body?.task_id == tid }) { "Task ID used/tombstoned" }; s = s.copy(scopes = s.scopes.copy(hosts = emptyList(), operations = emptyList(), delegation = DelegationLevel.READ_AND_SUGGEST)) }
                records += doc(s); records += doc(event(s, null, TaskTransition.PLANNED, request.operation_id))
            }
            else -> {
                val t = v.tasks.find { it.spec.task_id == request.task_id } ?: error("Task missing or deleted")
                require(v.conflicts.isEmpty() || request.action in setOf(PersonalAction.EDIT, PersonalAction.DELETE, PersonalAction.CANCEL)) { "Resolve conflicts before acceptance or snooze" }
                require(t.status !in setOf("CANCELLED", "VERIFIED") || request.action == PersonalAction.DELETE) { "Task is terminal" }
                when (request.action) {
                    PersonalAction.EDIT -> {
                        text(request.text); records += doc(t.spec.copy(goal = request.text))
                        if (t.status !in setOf("PLANNED", "BLOCKED")) records += doc(event(t.spec, t.events.last(), TaskTransition.BLOCKED, request.operation_id))
                    }
                    PersonalAction.ACCEPT -> {
                        var previous = t.events.last()
                        val steps = when (t.status) {
                            "PLANNED" -> listOf(TaskTransition.DRAFTED, TaskTransition.READY_FOR_REVIEW)
                            "DRAFTED", "WAITING_FOR_ACCESS", "BLOCKED" -> listOf(TaskTransition.READY_FOR_REVIEW)
                            else -> error("Already reviewed; no execution route connected")
                        }
                        steps.forEach { step -> val e = event(t.spec, previous, step, id()); records += doc(e); previous = e }
                    }
                    PersonalAction.SNOOZE -> {
                        val until = requireNotNull(request.snooze_until_ms)
                        require(until > now && until <= now + 31536000000L) { "Snooze must be within one year" }
                        if (t.status != "BLOCKED") records += doc(event(t.spec, t.events.last(), TaskTransition.BLOCKED, request.operation_id))
                    }
                    PersonalAction.CANCEL -> records += doc(event(t.spec, t.events.last(), TaskTransition.CANCELLED, request.operation_id))
                    PersonalAction.DELETE -> require(request.text.isEmpty() && request.draft.isEmpty())
                    else -> error("Invalid action")
                }
            }
        }
        if (shared != null) return copy(shared = shared.append(replica_id, request, records)).also { it.bytes() }
        return copy(mutations = mutations + PersonalMutation(v.revision + 1, request.operation_id, mutations.last().operation_id, request, records)).also { it.bytes() }
    }
    fun adopt(identity: SharedIdentity, confirmedArchive: Boolean): PersonalLedger {
        require(confirmedArchive && replica_id in identity.replicas) { "Explicit archive/adoption confirmation required" }
        shared?.let { require(it.identity == identity); return this }
        view(); return copy(version = 2, shared = SharedLedger(identity)).also { it.bytes() }
    }
    fun importShared(replica: String, ops: List<SharedOperation>): PersonalLedger {
        require(replica != replica_id)
        return copy(shared = requireNotNull(shared).receive(replica, ops)).also { it.bytes() }
    }
    fun outboundSize() = shared?.operations?.get(replica_id)?.size ?: if (shared != null) 0 else mutations.size
    private fun event(s: TaskSpec, previous: TaskEvent?, transition: TaskTransition, operationId: String) = TaskEvent(id(), operationId, s.task_id, previous?.event_id,
        replica_id, replica_id, previous?.let { it.step + 1 } ?: 0, s.deadline_ms, transition, null, null)
}
data class PersonalTask(var spec: TaskSpec, val events: MutableList<TaskEvent> = mutableListOf(), var draft: String = "", var snoozeUntilMs: Long? = null,
    var deleted: Boolean = false, var status: String = "", val ownerReplicaId: String? = null, val ownerEpoch: Long = 1, val remoteClaims: List<JsonObject> = emptyList())
data class PersonalView(val revision: Long, val replicaId: String, val agent: PersonalAgent, val persona: Persona, val tasks: List<PersonalTask>, val pendingMutations: Int, val conflicts: List<String> = emptyList(), val archivedMutations: Int = 0)
