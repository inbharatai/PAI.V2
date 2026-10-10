package com.unoone.agent.personal

import com.unoone.agent.core.personal.*
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.*

/** V2 causal union. V1 mutations are an immutable local archive, not rebound requests. */
@Serializable data class SharedIdentity(val person_id: String, val agent_id: String, val founder_replica_id: String, val replicas: Map<String, String>)
@Serializable data class SharedAssignment(val owner_replica_id: String, val epoch: Long)
@Serializable data class SharedBody(val assignment: SharedAssignment?, val action: PersonalAction, val task_id: String?, val draft: String, val snooze_until_ms: Long?, val records: List<JsonObject>)
@Serializable data class SharedOperation(val replica_id: String, val sequence: Long, val operation_id: String, val predecessor_operation_id: String?, val context: Map<String, Long>, val body: SharedBody?)
@Serializable data class SharedLedger(val identity: SharedIdentity, val operations: Map<String, List<SharedOperation>> = emptyMap(), val local_requests: List<PersonalRequest> = emptyList()) {
    private fun uuid(s: String) { require(java.util.UUID.fromString(s).toString() == s) }
    fun clock() = operations.toSortedMap().mapValues { it.value.size.toLong() }
    fun validate() {
        val i = identity
        listOf(i.person_id, i.agent_id, i.founder_replica_id).forEach(::uuid)
        require(i.replicas.size == 2 && i.founder_replica_id in i.replicas)
        i.replicas.forEach { (r, fp) -> uuid(r); require(fp.matches(Regex("[0-9a-f]{64}"))) }
        require(operations.values.sumOf { it.size } <= 2048)
        val ids = mutableSetOf<String>()
        operations.forEach { (r, ops) ->
            require(r in i.replicas); var previous: String? = null; var oldClock = emptyMap<String, Long>()
            ops.forEachIndexed { n, op ->
                uuid(op.operation_id)
                require(op.replica_id == r && op.sequence == n + 1L && op.predecessor_operation_id == previous && ids.add(op.operation_id)) { "Whole history collision/gap; hold for review" }
                require(op.context.size <= 2 && op.context.keys.all { it in i.replicas } && (op.context[r] ?: 0L) == n.toLong() && op.context.values.all { it in 0..2048 })
                oldClock.forEach { (k, v) -> require((op.context[k] ?: 0) >= v) { "Causal clock rollback" } }
                op.context.forEach { (dependency, sequence) -> if (sequence > 0) operations[dependency]?.getOrNull(sequence.toInt() - 1)?.let { prior -> require(prior.context.all { (r, n) -> n <= (op.context[r] ?: 0) }) { "Causal cycle/transitive dependency" } } }
                oldClock = op.context; previous = op.operation_id
                op.body?.let { b ->
                    if (b.action == PersonalAction.CREATE) require(b.assignment?.owner_replica_id == r && b.assignment.epoch == 1L) else require(b.assignment == null) { "Handoff is inert; cannot overwrite assignment" }
                    require(b.records.size <= 8 && b.draft.toByteArray().size <= 4096)
                    require(b.snooze_until_ms == null || b.snooze_until_ms in 0..9007199254740991)
                    b.task_id?.let(::uuid)
                    require((b.action in setOf(PersonalAction.PERSONA, PersonalAction.CLEAR_PERSONA)) == (b.task_id == null))
                    b.records.forEach { v ->
                        val valid = when (val record = PersonalCodec.decode(v.toString()).record) {
                            is PersonalAgent -> b.task_id == null && record.person_id == i.person_id && record.agent_id == i.agent_id
                            is Persona -> b.task_id == null && record.person_id == i.person_id && record.provenance.replica_id == r
                            is TaskSpec -> b.task_id == record.task_id && record.person_id == i.person_id && record.agent_id == i.agent_id && record.origin_replica_id in i.replicas && record.target_replica_id in i.replicas && record.scopes.capabilities.isEmpty() && record.scopes.tools.isEmpty() && record.scopes.data.isEmpty() && record.scopes.recipients.isEmpty() && record.scopes.hosts.isEmpty() && record.scopes.operations.isEmpty() && record.scopes.delegation == DelegationLevel.READ_AND_SUGGEST && record.scopes.network == NetworkPolicy.OFFLINE_ONLY
                            is TaskEvent -> b.task_id == record.task_id && record.origin_replica_id == r
                            is TaskReceipt -> b.task_id == record.task_id && record.replica_id == r
                            is Handoff -> b.task_id == record.task_id && record.from_replica_id == r
                            else -> false
                        }; require(valid) { "Unsupported shared record/authority/identity" }
                    }
                }
            }
        }
    }
    fun append(replica: String, request: PersonalRequest, records: List<JsonObject>): SharedLedger {
        val ops = operations[replica].orEmpty()
        return copy(operations = operations + (replica to (ops + SharedOperation(replica, ops.size + 1L, request.operation_id, ops.lastOrNull()?.operation_id, clock(), SharedBody(if (request.action == PersonalAction.CREATE) SharedAssignment(replica, 1) else null, request.action, request.task_id, request.draft, request.snooze_until_ms, records)))), local_requests = local_requests + request).also { it.validate() }
    }
    fun receive(replica: String, incoming: List<SharedOperation>): SharedLedger {
        require(replica in identity.replicas)
        val ops = operations[replica].orEmpty().toMutableList()
        incoming.forEach { op -> require(op.replica_id == replica && op.sequence > 0)
            ops.getOrNull(op.sequence.toInt() - 1)?.let { require(it == op) { "Whole history collision; original retained" } } ?: run { require(op.sequence == ops.size + 1L); ops += op }
        }
        return copy(operations = operations + (replica to ops)).also { it.validate() }
    }
    private fun canonical(v: JsonElement): JsonElement = when (v) { is JsonObject -> JsonObject(v.toSortedMap().mapValues { canonical(it.value) }); is JsonArray -> JsonArray(v.map(::canonical)); else -> v }
    private fun conflictJson(ops: List<SharedOperation>) = Json.encodeToString(ops.map { op -> op.copy(context = op.context.toSortedMap(), body = op.body?.let { b -> b.copy(records = b.records.map { canonical(it).jsonObject }) }) })
    private fun heads(ops: List<SharedOperation>) = ops.filter { a -> ops.none { b -> a.operation_id != b.operation_id && (b.context[a.replica_id] ?: 0) >= a.sequence } }
    fun view(ledger: PersonalLedger): PersonalView {
        validate()
        val all = operations.toSortedMap().values.flatten(); val clock = clock(); val i = identity
        val missing = all.any { o -> o.context.any { (r, n) -> n > (clock[r] ?: 0) } }
        var agent = PersonalAgent(i.agent_id, i.person_id, 1, "UnoOne", 1, emptyList(), emptyList())
        var persona = Persona(i.person_id, 1, emptyList(), Sensitivity.PRIVATE, Provenance(ProvenanceSource.USER, i.person_id, i.founder_replica_id, null), false, null)
        val conflicts = mutableListOf<String>()
        if (missing) conflicts += "CAUSAL_GAP: sync remaining pages; no execution or review acceptance"
        val personal = all.filter { it.body?.task_id == null && it.body != null }
        val clears = personal.filter { it.body?.action == PersonalAction.CLEAR_PERSONA }
        val personaHeads = heads(clears.ifEmpty { personal })
        if (personaHeads.size == 1) personaHeads[0].body!!.records.forEach { v -> when (val record = PersonalCodec.decode(v.toString()).record) { is PersonalAgent -> agent = record; is Persona -> persona = record; else -> Unit } }
        else if (personaHeads.size > 1) { conflicts += "PERSONA_CONFLICT: " + conflictJson(personaHeads); agent = agent.copy(display_name = "Persona conflict — review retained versions") }
        if (clears.isNotEmpty()) persona = persona.copy(deleted = true, preferences = emptyList())
        val tids = all.mapNotNull { it.body?.task_id }.toSortedSet(); require(tids.size <= 128)
        val tasks = mutableListOf<PersonalTask>()
        tids.forEach { tid ->
            val ops = all.filter { it.body?.task_id == tid }
            if (ops.any { it.body!!.action == PersonalAction.DELETE }) return@forEach
            val creates = ops.filter { it.body!!.action == PersonalAction.CREATE }; require(creates.size <= 1) { "Task creation collision; whole history held" }
            if (creates.isEmpty()) { conflicts += "MISSING_TASK_HISTORY: $tid"; return@forEach }
            val owner = creates[0].replica_id
            val genesis = creates[0].body!!.records.mapNotNull { PersonalCodec.decode(it.toString()).record as? TaskSpec }.first()
            require(genesis.origin_replica_id == owner && genesis.target_replica_id == owner)
            ops.flatMap { it.body!!.records }.forEach { value -> (PersonalCodec.decode(value.toString()).record as? TaskSpec)?.let { spec ->
                require(spec.origin_replica_id == genesis.origin_replica_id && spec.target_replica_id == genesis.target_replica_id && spec.deadline_ms == genesis.deadline_ms && spec.created_at_ms == genesis.created_at_ms && spec.idempotency_key == genesis.idempotency_key) { "Task identity/deadline/assignment collision; hold" }
            } }
            val versions = heads(ops.filter { it.body!!.action in setOf(PersonalAction.CREATE, PersonalAction.EDIT) })
            val specs = versions.flatMap { it.body!!.records }.mapNotNull { PersonalCodec.decode(it.toString()).record as? TaskSpec }; require(specs.isNotEmpty())
            var spec = specs[0]; val conflict = versions.size > 1
            val draft = if (conflict) { conflicts += "TASK_CONFLICT $tid: " + conflictJson(versions); spec = spec.copy(goal = "Conflicting task edits — review retained versions"); "" } else versions[0].body!!.draft
            val reminderHeads = heads(ops.filter { it.body!!.action in setOf(PersonalAction.CREATE, PersonalAction.EDIT, PersonalAction.ACCEPT, PersonalAction.SNOOZE) })
            val snooze = if (reminderHeads.size == 1) reminderHeads[0].body!!.snooze_until_ms else { conflicts += "REMINDER_CONFLICT $tid: " + conflictJson(reminderHeads); null }
            val events = mutableListOf<TaskEvent>(); val claims = mutableListOf<JsonObject>()
            ops.forEach { o -> o.body!!.records.forEach { v -> when (val record = PersonalCodec.decode(v.toString()).record) { is TaskEvent -> events += record; is TaskReceipt, is Handoff -> claims += v; else -> Unit } } }
            events.sortWith(compareBy({ it.step }, { it.event_id }))
            val fold = foldTask(tid, events, owner)
            val names = mapOf(FoldState.WAITING_FOR_OWNER to "WaitingForOwner", FoldState.MISSING_PREDECESSOR to "MissingPredecessor", FoldState.CONFLICT to "Conflict", FoldState.EMPTY to "Empty")
            val status = if (missing || conflict || reminderHeads.size > 1) "CONFLICT_REVIEW" else fold.transition?.name ?: "REVIEW_${names[fold.state]}"
            tasks += PersonalTask(spec, events, draft, snooze, false, status, owner, 1, claims)
        }
        return PersonalView(ledger.mutations.size + all.size.toLong(), ledger.replica_id, agent, persona, tasks, operations[ledger.replica_id].orEmpty().size, conflicts, ledger.mutations.size)
    }
}
