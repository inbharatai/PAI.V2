package com.unoone.agent.core.personal

import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.json.*

@Serializable
internal data class WireEnvelope(val schema: String, val version: Version, val kind: RecordKind, val payload: JsonObject)

/** Decoded records are inert claims, NEVER a local grant, verified receipt or executable task. */
data class HydratedDocument(val record: PersonalRecord) {
    val mayExecuteOnHydration: Boolean get() = false
}

object PersonalCodec {
    const val SCHEMA = "inbharat.pai.personal-agent"
    const val MAX_BYTES = 65536
    const val MAX_DEPTH = 12
    const val MAX_ITEMS = 64
    internal val json = Json { ignoreUnknownKeys = false; encodeDefaults = true; isLenient = false }

    fun decode(input: ByteArray): HydratedDocument {
        require(input.size <= MAX_BYTES)
        val text = try {
            Charsets.UTF_8.newDecoder().onMalformedInput(java.nio.charset.CodingErrorAction.REPORT)
                .onUnmappableCharacter(java.nio.charset.CodingErrorAction.REPORT).decode(java.nio.ByteBuffer.wrap(input)).toString()
        } catch (e: java.nio.charset.CharacterCodingException) { throw IllegalArgumentException("invalid UTF-8", e) }
        return decode(text)
    }

    fun decode(input: String): HydratedDocument {
        preflight(input)
        val element = json.parseToJsonElement(input)
        bounds(element, 0, "")
        val wire = json.decodeFromJsonElement<WireEnvelope>(element)
        require(wire.schema == SCHEMA && wire.version == Version(1, 0)) { "unsupported schema/version" }
        val record = decodeRecord(wire)
        validate(record)
        return HydratedDocument(record)
    }

    fun encode(record: PersonalRecord): String {
        validate(record)
        val (kind, payload) = encodeRecord(record)
        val wire = WireEnvelope(SCHEMA, Version(1, 0), kind, payload)
        val result = json.encodeToString(wire)
        preflight(result)
        return result
    }

    fun validate(record: PersonalRecord) {
        val (kind, payload) = encodeRecord(record)
        bounds(payload, 1, "payload")
        preflight(json.encodeToString(WireEnvelope(SCHEMA, Version(1, 0), kind, payload)))
        when (record) {
            is Persona -> {
                require(record.corrects_revision == null || record.corrects_revision < record.revision)
                require(!record.deleted || record.preferences.isEmpty())
                require(record.preferences.all { it.status == PreferenceStatus.OBSERVED || it.provenance.source == ProvenanceSource.USER })
            }
            is TaskSpec -> {
                scopeBudget(record.scopes, record.budget)
                window(record.created_at_ms, record.deadline_ms)
                require(record.budget.max_duration_ms <= record.deadline_ms - record.created_at_ms)
                require(record.goal.isNotBlank() && record.expected_postcondition.isNotBlank())
            }
            is TaskEvent -> {
                require(record.step <= 1024 && record.deadline_ms > 0 && record.predecessor_event_id != record.event_id)
                require(record.transition != TaskTransition.VERIFIED || !record.evidence_ref.isNullOrEmpty())
            }
            is AgentSpec -> {
                scopeBudget(record.scopes, record.budget)
                require(record.depth in 1..record.budget.max_depth && record.template_version > 0 && record.expires_at_ms > 0)
            }
            is TaskReceipt -> {
                require(record.outcome !in setOf(ReceiptOutcome.VERIFIED, ReceiptOutcome.ACTION_VERIFIED) || record.after_evidence_refs.isNotEmpty())
                require(record.outcome != ReceiptOutcome.VERIFIED || record.dispatch_intent !in setOf(DispatchIntent.OPEN_COMPOSER, DispatchIntent.OPEN_EVENT_FORM))
            }
            is Handoff -> require(record.from_replica_id != record.target_replica_id && record.encrypted_goal.isNotEmpty() && record.expires_at_ms > 0)
            is ReplicaChange -> {
                require(record.sequence > 0 && record.record_revision > 0 && record.provenance.replica_id == record.replica_id)
                require(record.content_hash.matches(Regex("[0-9a-f]{64}")))
                require(if (record.change == ChangeKind.TOMBSTONE) record.ciphertext == null else !record.ciphertext.isNullOrEmpty())
                require(record.record_kind != RecordKind.CAPABILITY_GRANT) { "grants cannot be replicated" }
            }
            is Draft -> require(record.reply_to == null || record.reply_to.account_id == record.account_id)
            is EventRef -> require(record.end_ms > record.start_ms && record.time_zone.isNotEmpty() && record.time_zone == record.calendar.time_zone)
            is CalendarRef -> require(record.time_zone.isNotEmpty())
            is CapabilityGrant -> {
                scopeBudget(record.scopes, record.budget)
                window(record.issued_at_ms, record.expires_at_ms)
                require(record.budget.max_duration_ms <= record.expires_at_ms - record.issued_at_ms)
            }
            else -> Unit
        }
    }

    internal fun window(start: Long, end: Long) { require(end > start && end - start <= 86400000) }
    internal fun scopeBudget(scopes: Scopes, budget: Budget) {
        validateScopes(scopes); validateBudget(budget)
        require(scopes.network != NetworkPolicy.OFFLINE_ONLY || budget.max_network_calls == 0L)
    }
    internal fun validateBudget(b: Budget) {
        require(b.max_steps in 0..1024 && b.max_tool_calls in 0..1024 && b.max_duration_ms in 1..86400000 && b.max_bytes in 0..67108864 && b.max_network_calls in 0..1024 && b.max_depth in 0..2 && b.max_children in 0..2)
    }
    internal fun validateScopes(s: Scopes) {
        listOf(s.capabilities, s.tools, s.recipients, s.hosts).forEach { list ->
            require(list.size <= MAX_ITEMS && list.distinct().size == list.size)
            require(list.all { it.isNotEmpty() && it.toByteArray(Charsets.UTF_8).size <= 256 && it.all { c -> c.code in 33..126 } && '*' !in it })
        }
        require(s.operations.distinct().size == s.operations.size && s.data.size <= MAX_ITEMS)
        require(s.data.map { it.kind to it.resource_id }.distinct().size == s.data.size)
        s.data.forEach { d ->
            require(d.resource_id.isNotEmpty() && '*' !in d.resource_id && d.resource_id.toByteArray(Charsets.UTF_8).size <= 128 && d.resource_id.all { it.code in 33..126 } && d.operations.distinct().size == d.operations.size)
            require(s.operations.containsAll(d.operations))
        }
        require(s.delegation != DelegationLevel.READ_AND_SUGGEST || s.operations.all { it == Operation.READ || it == Operation.SUGGEST })
        require(s.delegation != DelegationLevel.PREPARE_DRAFTS || s.operations.all { it in setOf(Operation.READ, Operation.SUGGEST, Operation.DRAFT) })
    }
    private fun bounds(v: JsonElement, depth: Int, key: String) {
        require(depth <= MAX_DEPTH)
        when (v) {
            is JsonObject -> { require(v.size <= MAX_ITEMS); v.forEach { (k, x) -> bounds(x, depth + 1, k) } }
            is JsonArray -> { require(v.size <= MAX_ITEMS); v.forEach { bounds(it, depth + 1, key) } }
            is JsonPrimitive -> if (v != JsonNull) {
                if (v.isString) {
                    val s = v.content
                    require(Charsets.UTF_8.newEncoder().canEncode(s)) { "invalid Unicode" }
                    require(s.toByteArray(Charsets.UTF_8).size <= 4096)
                    if (key.endsWith("_id") || key == "idempotency_key") require(s.isNotEmpty() && s.length <= 128 && s.all { it.code in 33..126 })
                } else if (v.booleanOrNull == null) {
                    require(v.content.matches(Regex("[0-9]+")) && v.longOrNull?.let { it in 0..9007199254740991L } == true)
                }
            }
        }
    }

    /** Bound nesting before allocating a JSON tree; reject duplicate (even escaped) keys. */
    private fun preflight(input: String) {
        require(input.toByteArray(Charsets.UTF_8).size <= MAX_BYTES)
        val stack = mutableListOf<MutableSet<String>?>()
        var i = 0
        while (i < input.length) {
            when (input[i]) {
                '{' -> { stack.add(mutableSetOf()); require(stack.size <= MAX_DEPTH) }
                '[' -> { stack.add(null); require(stack.size <= MAX_DEPTH) }
                '}', ']' -> { require(stack.isNotEmpty()); stack.removeAt(stack.lastIndex) }
                '"' -> {
                    val start = i++
                    var escaped = false
                    while (i < input.length) {
                        val c = input[i]
                        if (!escaped && c == '"') break
                        if (!escaped && c == '\\') escaped = true else escaped = false
                        i++
                    }
                    require(i < input.length)
                    var next = i + 1
                    while (next < input.length && input[next].isWhitespace()) next++
                    if (next < input.length && input[next] == ':') {
                        val key = json.decodeFromString<String>(input.substring(start, i + 1))
                        require(stack.lastOrNull()?.add(key) == true) { "duplicate/invalid key" }
                    }
                }
            }
            i++
        }
        require(stack.isEmpty())
    }
}
