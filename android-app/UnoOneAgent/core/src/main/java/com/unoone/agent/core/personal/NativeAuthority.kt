package com.unoone.agent.core.personal

/** Native-only authority seam. NO serializer, no conversion from a hydrated approval.
 * Constructor callers must use local consent storage, never model/peer assertions.
 * Mutable collection inputs are snapshotted at the boundary.
 */
class LocalGrant private constructor(private val value: CapabilityGrant, val approvalRef: String) {
    fun snapshot(): CapabilityGrant = PersonalCodec.decode(PersonalCodec.encode(value)).record as CapabilityGrant
    fun live(replica: String, now: Long, generation: Long): Boolean =
        !value.revoked && value.replica_id == replica && now >= value.issued_at_ms && now < value.expires_at_ms && generation == value.stop_generation
    companion object {
        fun approveLocally(grant: CapabilityGrant, localReplica: String, approvalRef: String, now: Long, generation: Long): LocalGrant {
            val snapshot = PersonalCodec.decode(PersonalCodec.encode(grant)).record as CapabilityGrant
            require(approvalRef.isNotEmpty() && approvalRef.toByteArray(Charsets.UTF_8).size <= 128)
            val local = LocalGrant(snapshot, approvalRef)
            require(local.live(localReplica, now, generation))
            return local
        }
    }
}

/** Exact set intersection; no wildcard, path prefix, implicit recipient or domain expansion. */
fun Scopes.intersect(other: Scopes): Scopes {
    fun strings(a: List<String>, b: List<String>) = a.intersect(b.toSet()).sorted()
    fun operations(a: List<Operation>, b: List<Operation>) = a.intersect(b.toSet()).sortedBy { it.ordinal }
    val intersection = data.mapNotNull { a ->
        other.data.firstOrNull { it.kind == a.kind && it.resource_id == a.resource_id }?.let { b -> a.copy(operations = operations(a.operations, b.operations)) }
    }.sortedWith(compareBy<DataScope> { it.kind.ordinal }.thenBy { it.resource_id })
    return Scopes(strings(capabilities, other.capabilities), strings(tools, other.tools), intersection,
        strings(recipients, other.recipients), strings(hosts, other.hosts), operations(operations, other.operations),
        if (delegation.ordinal <= other.delegation.ordinal) delegation else other.delegation,
        if (network.ordinal <= other.network.ordinal) network else other.network)
}
fun Budget.intersect(other: Budget) = Budget(minOf(max_steps, other.max_steps), minOf(max_tool_calls, other.max_tool_calls),
    minOf(max_duration_ms, other.max_duration_ms), minOf(max_bytes, other.max_bytes), minOf(max_network_calls, other.max_network_calls),
    minOf(max_depth, other.max_depth), minOf(max_children, other.max_children))

/** Pure attenuation, NOT spawn or sibling-budget reservation. Native scheduler must account
 * consumption atomically and recheck revocation/generation before EACH side effect. */
fun attenuateChild(request: AgentSpec, parent: LocalGrant, host: Scopes, localReplica: String, now: Long, generation: Long): AgentSpec {
    PersonalCodec.validate(request); PersonalCodec.validateScopes(host)
    require(parent.live(localReplica, now, generation))
    require(request.stop_generation == generation && request.expires_at_ms > now)
    val grant = parent.snapshot()
    val scopes = request.scopes.intersect(grant.scopes).intersect(host)
    require(localReplica in scopes.hosts)
    val expiry = minOf(request.expires_at_ms, grant.expires_at_ms)
    val intersection = request.budget.intersect(grant.budget).let {
        if (scopes.network == NetworkPolicy.OFFLINE_ONLY) it.copy(max_network_calls = 0) else it
    }
    val child = request.copy(scopes = scopes, budget = intersection.copy(max_duration_ms = minOf(intersection.max_duration_ms, expiry - now)), expires_at_ms = expiry)
    require(child.depth <= child.budget.max_depth && grant.budget.max_children > 0)
    PersonalCodec.validate(child)
    return child
}

/** Must be populated by native postcondition observation, NEVER from JSON/model output. */
data class NativeObservation(val taskId: String, val operationId: String, val replicaId: String,
    val evidenceRef: String, val postconditionMatched: Boolean, val externalObjectId: String?)

/** Non-serializable runtime evidence wrapper. Wire TaskReceipt remains an untrusted claim. */
class VerifiedReceipt private constructor(private val value: TaskReceipt) {
    fun snapshot(): TaskReceipt = PersonalCodec.decode(PersonalCodec.encode(value)).record as TaskReceipt
    companion object {
        fun verifyNative(claim: TaskReceipt, observation: NativeObservation, localReplica: String): VerifiedReceipt {
            val r = PersonalCodec.decode(PersonalCodec.encode(claim)).record as TaskReceipt
            require(r.source == ProvenanceSource.NATIVE && r.replica_id == localReplica && observation.replicaId == localReplica)
            require(observation.taskId == r.task_id && observation.operationId == r.operation_id && observation.postconditionMatched)
            require(observation.evidenceRef.isNotEmpty() && observation.evidenceRef in r.after_evidence_refs)
            require(r.dispatch_intent !in setOf(DispatchIntent.OPEN_COMPOSER, DispatchIntent.OPEN_EVENT_FORM))
            if (r.dispatch_intent == DispatchIntent.PROVIDER_MUTATION) require(!observation.externalObjectId.isNullOrEmpty() && observation.externalObjectId == r.external_object_id)
            return VerifiedReceipt(r.copy(outcome = ReceiptOutcome.VERIFIED))
        }
    }
}
