package com.unoone.agent.core.personal

enum class FoldState { EMPTY, WAITING_FOR_OWNER, MISSING_PREDECESSOR, CONFLICT, TRANSITION }
data class TaskProjection(val state: FoldState, val transition: TaskTransition?, val eventIds: List<String>) {
    val executeOnHydration: Boolean get() = false
}

/** Immutable union/dedup and conservative causal projection, not sync or execution.
 * Owner is separately resolved authenticated assignment, not the newest event's claim.
 * Concurrent branches, incompatible operations and cycles are visible conflicts; no LWW.
 */
fun foldTask(taskId: String, events: List<TaskEvent>, owner: String?, verified: List<VerifiedReceipt> = emptyList()): TaskProjection {
    require(events.size <= 1024)
    val ids = sortedMapOf<String, TaskEvent>()
    val operations = mutableMapOf<String, TaskEvent>()
    var conflict = false
    events.forEach { e ->
        PersonalCodec.validate(e)
        require(e.task_id == taskId)
        if (ids[e.event_id]?.let { it != e } == true || operations[e.operation_id]?.let { it != e } == true) conflict = true
        ids[e.event_id] = e
        operations[e.operation_id] = e
    }
    fun result(state: FoldState, transition: TaskTransition? = null) = TaskProjection(state, transition, ids.keys.toList())
    if (conflict) return result(FoldState.CONFLICT)
    if (ids.isEmpty()) return result(FoldState.EMPTY)
    if (owner == null || ids.values.any { it.assigned_replica_id != owner || it.origin_replica_id != owner }) return result(FoldState.WAITING_FOR_OWNER)
    val roots = ids.values.filter { it.predecessor_event_id == null }
    if (ids.values.any { it.predecessor_event_id != null && it.predecessor_event_id !in ids }) return result(FoldState.MISSING_PREDECESSOR)
    if (roots.size != 1 || roots[0].transition != TaskTransition.PLANNED) return result(FoldState.CONFLICT)
    var current = roots.single()
    var visited = 1
    while (true) {
        val next = ids.values.filter { it.predecessor_event_id == current.event_id }
        if (next.isEmpty()) break
        if (next.size != 1) return result(FoldState.CONFLICT)
        val child = next.single()
        if (child.step != current.step + 1 || child.deadline_ms != current.deadline_ms || !allowed(current.transition, child.transition)) return result(FoldState.CONFLICT)
        current = child
        if (++visited > ids.size) return result(FoldState.CONFLICT)
    }
    if (visited != ids.size) return result(FoldState.CONFLICT)
    val trusted = verified.any { v -> v.snapshot().let { r ->
        r.task_id == taskId && r.operation_id == current.operation_id && r.replica_id == owner && current.evidence_ref in r.after_evidence_refs && current.external_object_id == r.external_object_id
    } }
    return result(FoldState.TRANSITION, if (current.transition == TaskTransition.VERIFIED && !trusted) TaskTransition.AWAITING_VERIFICATION else current.transition)
}
private fun allowed(from: TaskTransition, to: TaskTransition): Boolean {
    if (from in setOf(TaskTransition.VERIFIED, TaskTransition.CANCELLED)) return false
    if (to in setOf(TaskTransition.CANCELLED, TaskTransition.BLOCKED)) return true
    return to in when (from) {
        TaskTransition.PLANNED -> setOf(TaskTransition.WAITING_FOR_ACCESS, TaskTransition.DRAFTED, TaskTransition.IN_PROGRESS)
        TaskTransition.WAITING_FOR_ACCESS -> setOf(TaskTransition.DRAFTED, TaskTransition.READY_FOR_REVIEW, TaskTransition.IN_PROGRESS)
        TaskTransition.DRAFTED -> setOf(TaskTransition.READY_FOR_REVIEW)
        TaskTransition.READY_FOR_REVIEW -> setOf(TaskTransition.IN_PROGRESS)
        TaskTransition.IN_PROGRESS -> setOf(TaskTransition.AWAITING_VERIFICATION)
        TaskTransition.AWAITING_VERIFICATION -> setOf(TaskTransition.VERIFIED)
        TaskTransition.BLOCKED -> setOf(TaskTransition.WAITING_FOR_ACCESS, TaskTransition.READY_FOR_REVIEW)
        else -> emptySet()
    }
}
