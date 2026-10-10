package com.unoone.agent.core.personal

import java.io.File
import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Test

class PersonalContractConformanceTest {
    // One checked-in fixture set, not a copied Android resource that can drift.
    private val fixtures: File = System.getProperty("personal.contract.fixtures")?.let(::File)
        ?: generateSequence(File(System.getProperty("user.dir"))) { it.parentFile }
            .map { File(it, "packages/personal-agent-contracts/fixtures") }.first { it.isDirectory }
    private fun raw(name: String) = File(fixtures, "$name.json").readText()
    private inline fun <reified T : PersonalRecord> fixture(name: String) = PersonalCodec.decode(raw(name)).record as T
    private fun event() = fixture<TaskEvent>("task_event")
    private fun receipt() = fixture<TaskReceipt>("task_receipt")
    private fun grant() = fixture<CapabilityGrant>("capability_grant")
    private fun child() = fixture<AgentSpec>("agent_spec")
    private fun reject(block: () -> Unit) { try { block() } catch (_: IllegalArgumentException) { return }; fail("input unexpectedly accepted") }
    private fun mutate(v: JsonElement, path: List<String>, replacement: JsonElement, remove: Boolean = false): JsonElement {
        if (path.isEmpty()) return replacement
        val key = path.first()
        return when (v) {
            is JsonObject -> JsonObject(v.toMutableMap().apply {
                if (path.size == 1 && remove) remove(key)
                else put(key, mutate(get(key) ?: JsonNull, path.drop(1), replacement, remove))
            })
            is JsonArray -> JsonArray(v.toMutableList().apply { val i = key.toInt(); set(i, mutate(get(i), path.drop(1), replacement, remove)) })
            else -> error("invalid mutation path")
        }
    }

    @Test fun everySharedGoldenRoundtripsBothDirections() {
        val files = fixtures.listFiles()!!.filter { it.extension == "json" && it.name != "negative_cases.json" }
        assertEquals(14, files.size)
        files.forEach { f ->
            val input = f.readText()
            val doc = PersonalCodec.decode(input)
            assertFalse(doc.mayExecuteOnHydration)
            val output = PersonalCodec.encode(doc.record)
            assertEquals(f.name, Json.parseToJsonElement(input), Json.parseToJsonElement(output))
            assertEquals(doc, PersonalCodec.decode(output))
        }
    }
    @Test fun sharedNegativeMutations() {
        val cases = Json.parseToJsonElement(raw("negative_cases")).jsonArray
        cases.forEach { case ->
            val c = case.jsonObject
            val value = Json.parseToJsonElement(raw(c.getValue("fixture").jsonPrimitive.content))
            val changed = mutate(value, c.getValue("pointer").jsonPrimitive.content.drop(1).split('/'), c["value"] ?: JsonNull, c["remove"]?.jsonPrimitive?.booleanOrNull == true)
            try { reject { PersonalCodec.decode(changed.toString()) } } catch (e: AssertionError) { throw AssertionError(c.getValue("name").toString(), e) }
        }
    }
    @Test fun hardLimitsAndDuplicateKeys() {
        reject { PersonalCodec.decode(" ".repeat(65537)) }
        reject { PersonalCodec.decode(byteArrayOf(0xff.toByte())) }
        reject { PersonalCodec.encode(fixture<PersonalAgent>("personal_agent").copy(conversation_refs = List(64) { "a".repeat(4096) })) }
        reject { PersonalCodec.decode("[".repeat(13) + "0" + "]".repeat(13)) }
        reject { PersonalCodec.encode(fixture<TaskSpec>("task_spec").copy(goal = "a".repeat(4097))) }
        reject { PersonalCodec.encode(fixture<PersonalAgent>("personal_agent").copy(conversation_refs = List(65) { "thread" })) }
        val text = raw("personal_agent")
        reject { PersonalCodec.decode(text.replaceFirst("\"agent_id\":", "\"agent_id\":\"evil\",\"agent_id\":")) }
        reject { PersonalCodec.decode(text.replaceFirst("\"agent_id\":", "\"agent_id\":\"evil\",\"agent_\\u0069d\":")) }
    }
    @Test fun readOnlyDefaultsAreNotMissingAuthorityDefaults() {
        var wire = Json.parseToJsonElement(raw("capability_grant"))
        wire = mutate(wire, listOf("payload", "scopes", "delegation"), JsonNull, true)
        wire = mutate(wire, listOf("payload", "scopes", "network"), JsonNull, true)
        val g = PersonalCodec.decode(wire.toString()).record as CapabilityGrant
        assertEquals(DelegationLevel.READ_AND_SUGGEST, g.scopes.delegation)
        assertEquals(NetworkPolicy.OFFLINE_ONLY, g.scopes.network)
        reject { PersonalCodec.encode(g.copy(scopes = g.scopes.copy(operations = listOf(Operation.SEND)))) }
    }
    @Test fun childIntersectionAndBudgetExpiryAttenuation() {
        val g = grant()
        val local = LocalGrant.approveLocally(g, "phone-1", "human-approval-1", 2000, 0)
        val requested = child().let { it.copy(budget = it.budget.copy(max_steps = 100, max_bytes = 100000), expires_at_ms = 100000,
            scopes = it.scopes.copy(tools = it.scopes.tools + "unapproved_tool", recipients = listOf("stranger@example.invalid"))) }
        val out = attenuateChild(requested, local, g.scopes, "phone-1", 2000, 0)
        assertEquals(g.scopes.tools, out.scopes.tools); assertTrue(out.scopes.recipients.isEmpty())
        assertEquals(8L, out.budget.max_steps); assertEquals(8192L, out.budget.max_bytes)
        assertEquals(59000L, out.budget.max_duration_ms); assertEquals(61000L, out.expires_at_ms)
        reject { attenuateChild(requested, local, g.scopes, "power-1", 2000, 0) }
        reject { attenuateChild(requested, local, g.scopes, "phone-1", 61000, 0) }
        reject { attenuateChild(requested, local, g.scopes, "phone-1", 2000, 1) }
        reject { LocalGrant.approveLocally(g.copy(revoked = true), "phone-1", "approval", 2000, 0) }
    }
    @Test fun exactDataScopeIntersection() {
        val a = grant().scopes.let { it.copy(delegation = DelegationLevel.ACT_WITHIN_SCOPE, operations = listOf(Operation.READ, Operation.SEND), data = it.data.map { d -> d.copy(operations = listOf(Operation.READ, Operation.SEND)) }) }
        val b = a.copy(operations = listOf(Operation.READ), data = a.data.map { it.copy(operations = listOf(Operation.READ)) })
        assertEquals(listOf(Operation.READ), a.intersect(b).data.single().operations)
        assertTrue(a.intersect(b.copy(data = b.data.map { it.copy(resource_id = "mail-1/subfolder") })).data.isEmpty())
    }
    private fun native(r: TaskReceipt) = NativeObservation(r.task_id, r.operation_id, r.replica_id, r.after_evidence_refs.first(), true, r.external_object_id)
    @Test fun verifiedClaimIsNotNativeVerification() {
        var r = receipt()
        reject { VerifiedReceipt.verifyNative(r, native(r), "phone-1") }
        r = r.copy(dispatch_intent = DispatchIntent.PROVIDER_MUTATION, external_object_id = "saved-object-1", source = ProvenanceSource.MODEL)
        reject { VerifiedReceipt.verifyNative(r, native(r), "phone-1") }
        r = r.copy(source = ProvenanceSource.NATIVE)
        assertEquals(ReceiptOutcome.VERIFIED, VerifiedReceipt.verifyNative(r, native(r), "phone-1").snapshot().outcome)
        reject { VerifiedReceipt.verifyNative(r, native(r).copy(externalObjectId = "different"), "phone-1") }
        r = r.copy(external_object_id = null)
        reject { VerifiedReceipt.verifyNative(r, native(r), "phone-1") }
    }
    private fun chain(): List<TaskEvent> {
        val root = event()
        val events = mutableListOf(root)
        listOf(TaskTransition.IN_PROGRESS, TaskTransition.AWAITING_VERIFICATION, TaskTransition.VERIFIED).forEachIndexed { n, t ->
            events.add(root.copy(event_id = "event-${n + 2}", operation_id = "op-${n + 2}", predecessor_event_id = events.last().event_id,
                step = (n + 1).toLong(), transition = t, evidence_ref = if (t == TaskTransition.VERIFIED) "evidence-1" else null))
        }
        return events
    }
    @Test fun causalFoldIsPermutationInvariantIdempotentAndInert() {
        val events = chain()
        val expected = foldTask("task-1", events, "phone-1")
        assertEquals(TaskTransition.AWAITING_VERIFICATION, expected.transition)
        assertEquals(expected, foldTask("task-1", events.reversed() + events.last(), "phone-1"))
        assertFalse(expected.executeOnHydration)
        assertEquals(FoldState.WAITING_FOR_OWNER, foldTask("task-1", events, null).state)
        assertEquals(FoldState.WAITING_FOR_OWNER, foldTask("task-1", events.dropLast(1) + events.last().copy(assigned_replica_id = "power-1"), "phone-1").state)
    }
    @Test fun foldConflictsMissingPredecessorsAndIllegalTransitions() {
        val e = event()
        assertEquals(FoldState.CONFLICT, foldTask("task-1", listOf(e, e.copy(transition = TaskTransition.BLOCKED)), "phone-1").state)
        val events = chain()
        assertEquals(FoldState.CONFLICT, foldTask("task-1", events + events[1].copy(event_id = "branch", operation_id = "branch-op"), "phone-1").state)
        assertEquals(FoldState.MISSING_PREDECESSOR, foldTask("task-1", events.drop(1), "phone-1").state)
        assertEquals(FoldState.CONFLICT, foldTask("task-1", events.mapIndexed { i, x -> if (i == 1) x.copy(transition = TaskTransition.CANCELLED) else x }, "phone-1").state)
        assertEquals(FoldState.CONFLICT, foldTask("task-1", listOf(e, e.copy(event_id = "different")), "phone-1").state)
    }
    @Test fun onlyNativeWrapperCanRaiseProjectionToVerified() {
        val r = receipt().copy(operation_id = "op-4", dispatch_intent = DispatchIntent.LOCAL_MUTATION)
        val verified = VerifiedReceipt.verifyNative(r, native(r), "phone-1")
        val projection = foldTask("task-1", chain(), "phone-1", listOf(verified))
        assertEquals(TaskTransition.VERIFIED, projection.transition); assertFalse(projection.executeOnHydration)
    }
    @Test fun localGrantSnapshotsMutableCollections() {
        val tools = grant().scopes.tools.toMutableList()
        val g = grant().let { it.copy(scopes = it.scopes.copy(tools = tools)) }
        val local = LocalGrant.approveLocally(g, "phone-1", "approval", 2000, 0)
        tools.add("unapproved_tool")
        assertFalse("unapproved_tool" in local.snapshot().scopes.tools)
    }
}
