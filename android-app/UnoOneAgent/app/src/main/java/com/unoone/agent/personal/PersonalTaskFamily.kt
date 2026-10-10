package com.unoone.agent.personal

import com.unoone.agent.core.task.*
import com.unoone.agent.core.personal.*
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/** Fixed catalog on the real coordinator. Model input cannot add registrations or scopes.
 * Coordinator shares one atomic model budget/deadline among siblings and revokes the tree.
 * No Android platform dependency: exercised with explicitly labelled test model callbacks. */
class PersonalTaskFamily(private val model: suspend (TaskContext, String) -> String) {
    private val parentKind = WorkerKind("personal-reviewed-family")
    private val childKind = WorkerKind("personal-reviewed-specialist")
    private class Family(val permit: PersonalDraftPermit, val prompts: List<String>, val check: () -> Unit) {
        val children = mutableListOf<TaskId>()
        val output = mutableMapOf<Int, String>()
        var bytes = 0
    }
    private data class Child(val family: Family, val index: Int, val handles: Set<String>, val duration: Long)
    private val pending = ConcurrentHashMap<String, Family>()
    private val childCalls = ConcurrentHashMap<String, Child>()
    private val admissions = ConcurrentHashMap<TaskId, String>()
    private val families = ConcurrentHashMap<TaskId, Family>()
    fun registrations() = listOf(
        WorkerRegistration(parentKind, WorkerLane.BACKGROUND, NativeTaskWorker { ctx ->
            val family = checkNotNull(pending.remove(ctx.instruction))
            family.check(); ctx.checkActive()
            families[ctx.taskId] = family
            val grant = family.permit.grant.snapshot()
            for (index in 0..1) {
                family.check(); ctx.checkActive()
                val requested = grant.scopes.copy(tools = listOf("model.respond"), data = if (index == 0) grant.scopes.data else emptyList())
                val host = grant.scopes.copy(tools = listOf("model.respond"))
                val spec = attenuateChild(AgentSpec(UUID.randomUUID().toString(), family.permit.taskId,
                    if(index == 0) "summarize_sources" else "draft", 1, ModelSelectionRule.LOCAL_QUALIFIED_ONLY,
                    requested, Budget(1,0,30_000,1800,0,1,0), 1, grant.expires_at_ms, grant.stop_generation, "response-only"),
                    family.permit.grant, host, grant.replica_id, System.currentTimeMillis(), grant.stop_generation)
                check("model.respond" in spec.scopes.tools)
                // §3.6 host-owned guardian gate on the child spawn: tools ⊆ parent, no network/browser, depth one.
                val childScope = TaskScope(setOf(TaskCapability.MODEL), objectHandles = spec.scopes.data.map { it.resource_id }.toSet())
                com.unoone.agent.core.guardian.PrivacyGuardian.enforce(com.unoone.agent.core.guardian.PrivacyGuardian.check(
                    com.unoone.agent.core.guardian.Intent.SpawnChild(spec.purpose, spec.scopes.tools, TaskCapability.BROWSER in childScope.capabilities, spec.depth.toInt(), spec.budget.max_depth.toInt()),
                    com.unoone.agent.core.guardian.Context(parentTools = grant.scopes.tools)), null, System.currentTimeMillis())
                val handles = childScope.objectHandles
                val key = UUID.randomUUID().toString()
                childCalls[key] = Child(family, index, handles, spec.budget.max_duration_ms)
                val admitted = ctx.delegate(ChildRequest(RequestId(key), childKind, key, TaskScope(setOf(TaskCapability.MODEL), objectHandles=handles)))
                if (admitted !is Admission.Accepted) { childCalls.remove(key); error("Child rejected; no fallback") }
                synchronized(family) { family.children.add(admitted.taskId) }
            }
            WorkerResult.WaitingForChildren
        }),
        WorkerRegistration(childKind, WorkerLane.BACKGROUND, NativeTaskWorker { ctx ->
            val call = checkNotNull(childCalls.remove(ctx.instruction))
            call.family.check(); ctx.checkActive()
            check(ctx.scope.capabilities == setOf(TaskCapability.MODEL) && ctx.scope.objectHandles == call.handles)
            ctx.beforeModelCall()
            val text = kotlinx.coroutines.withTimeout(call.duration) { model(ctx, call.family.prompts[call.index]) }
            call.family.check(); ctx.checkActive()
            val size = text.toByteArray().size
            require(size <= 1800)
            ctx.complete(TaskResult(TaskOutcome.RESPONDED)) {
                synchronized(call.family) {
                    check(call.family.bytes + size <= 3600)
                    call.family.bytes += size
                    call.family.output[call.index] = text
                }
            }
            WorkerResult.Finished(TaskResult(TaskOutcome.RESPONDED))
        })
    )
    fun submit(coordinator: TaskCoordinator, permit: PersonalDraftPermit, sourcePrompt: String, draftPrompt: String, check: () -> Unit): Admission {
        check()
        val g = permit.grant.snapshot()
        require(g.budget.max_children >= 2 && g.budget.max_depth >= 1 && g.budget.max_steps >= 3 && g.budget.max_bytes >= 7696)
        require(sourcePrompt.toByteArray().size <= 16_384 && draftPrompt.toByteArray().size <= 8192)
        val remaining = minOf(g.expires_at_ms-System.currentTimeMillis(),g.budget.max_duration_ms)
        require(remaining > 0)
        val key = UUID.randomUUID().toString()
        pending[key] = Family(permit, listOf(sourcePrompt, draftPrompt), check)
        val admission = coordinator.submit(TaskRequest(RequestId(key),parentKind,key,
            TaskScope(setOf(TaskCapability.MODEL),objectHandles=g.scopes.data.map { it.resource_id }.toSet()),
            coordinator.captureGeneration(),TaskSource.NATIVE,TaskPriority.NORMAL,TaskBudget(remaining,0,2)))
        if (admission is Admission.Rejected) pending.remove(key)
        if (admission is Admission.Accepted) admissions[admission.taskId] = key
        return admission
    }
    fun output(id: TaskId): String {
        val family = checkNotNull(families[id]); family.check()
        return synchronized(family) {
            check(family.output.size == 2)
            "Summary child RESPONDED (unverified): ${family.output[0]}\nIndependent draft child RESPONDED (unverified): ${family.output[1]}"
        }
    }
    fun discard(id: TaskId) {
        admissions.remove(id)?.let { pending.remove(it) }
        val family = families.remove(id) ?: return
        childCalls.entries.removeIf { it.value.family === family }
        synchronized(family) { family.output.clear() }
    }
}
