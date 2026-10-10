package com.unoone.agent.personal

import com.unoone.agent.core.personal.*
import kotlinx.serialization.json.*

/** Explicit local UI template review only. Not serializable or reconstructible from peers. */
class PersonalDraftPermit private constructor(val taskId: String, val revision: Long, val grant: LocalGrant, val source: PersonalSource) {
    companion object {
        fun approve(view: PersonalView, taskId: String, expectedRevision: Long, now: Long, generation: Long, source: PersonalSource = PersonalSource(), children: Boolean = false): PersonalDraftPermit {
            source.validate()
            require(view.revision == expectedRevision && view.conflicts.isEmpty())
            val task = view.tasks.single { it.spec.task_id == taskId }
            localOwner(view, task)
            require(task.status == "READY_FOR_REVIEW" && task.snoozeUntilMs == null && task.spec.deadline_ms > now)
            // Native fixed template requested by the distinct review action. Wire scaffolding is NOT a grant.
            val host = Scopes(emptyList(), (if (source.recordIds.isEmpty()) emptyList() else listOf("personal.notes.search")) + if (children) listOf("model.respond") else emptyList(), source.recordIds.map { DataScope(DataKind.RECORD, it, listOf(Operation.READ)) }, emptyList(), listOf(view.replicaId), listOf(Operation.READ, Operation.DRAFT, Operation.SUGGEST), DelegationLevel.PREPARE_DRAFTS, NetworkPolicy.OFFLINE_ONLY)
            val budget = task.spec.budget.intersect(if(children) Budget(3, 0, 60_000, 8192, 0, 1, 2) else Budget(1, 0, 60_000, 4096, 0, 0, 0))
            require(!children || (budget.max_steps >= 3 && budget.max_children >= 2 && budget.max_depth >= 1 && budget.max_bytes >= 7696)) { "Task budget predates child template; create a new reviewed task" }
            val grant = CapabilityGrant(PersonalLedger.id(), view.agent.person_id, view.replicaId, host.intersect(host), budget, now, minOf(now + budget.max_duration_ms, task.spec.deadline_ms), generation, false)
            return PersonalDraftPermit(taskId, view.revision, LocalGrant.approveLocally(grant, view.replicaId, "local-reviewed-draft-template-v1", now, generation), source.copy(recordIds = source.recordIds.toList()))
        }
    }
}
private fun localOwner(view: PersonalView, task: PersonalTask) {
    require(!task.deleted && task.ownerEpoch == 1L && task.ownerReplicaId == view.replicaId && task.spec.origin_replica_id == view.replicaId && task.spec.target_replica_id == view.replicaId && task.remoteClaims.isEmpty()) { "Foreign/ambiguous/handoff task cannot execute" }
}
enum class PersonalDraftPhase { STARTED, RESPONDED, FAILED }
data class ReviewedPersonalDraft(val taskId: String, val expectedRevision: Long, val vaultEpoch: Long, val source: PersonalSource = PersonalSource(), val children: Boolean = false)

/** Exact same existing TaskEvent/outbox representation as the Rust native adapter. */
fun PersonalLedger.recordDraftAttempt(expectedRevision: Long, phase: PersonalDraftPhase, output: String, permit: PersonalDraftPermit, now: Long, generation: Long): PersonalLedger {
    require(expectedRevision == permit.revision + if (phase == PersonalDraftPhase.STARTED) 0 else 1)
    val view = view()
    require(view.revision == expectedRevision && view.conflicts.isEmpty())
    require(permit.grant.live(view.replicaId, now, generation) && permit.grant.snapshot().person_id == view.agent.person_id)
    val task = view.tasks.single { it.spec.task_id == permit.taskId }; localOwner(view, task)
    require(output.toByteArray().size <= 3800)
    val (transition, draft) = when (phase) {
        PersonalDraftPhase.STARTED -> { require(task.status == "READY_FOR_REVIEW"); TaskTransition.IN_PROGRESS to "Native plan: one local-model draft with exact reviewed local sources; no file changes, messages or events sent. Attempt started; interruption needs explicit review." }
        PersonalDraftPhase.RESPONDED -> { require(task.status == "IN_PROGRESS"); TaskTransition.AWAITING_VERIFICATION to "RESPONDED — generated draft, not verified facts or external effect. Review before use.\n$output" }
        PersonalDraftPhase.FAILED -> { require(task.status == "IN_PROGRESS"); TaskTransition.BLOCKED to "Native attempt failed/stopped. No automatic retry. Review and accept again." }
    }
    val operation = PersonalLedger.id()
    val request = PersonalRequest(operation, expectedRevision, PersonalAction.EDIT, task.spec.task_id, task.spec.goal, draft, null, view.replicaId)
    val previous = task.events.last()
    val event = TaskEvent(PersonalLedger.id(), operation, task.spec.task_id, previous.event_id, replica_id, replica_id, previous.step + 1, task.spec.deadline_ms, transition, null, null)
    val records = listOf(task.spec, event).map { Json.parseToJsonElement(PersonalCodec.encode(it)).jsonObject }
    return (if (shared != null) copy(shared = shared.append(replica_id, request, records))
        else copy(mutations = mutations + PersonalMutation(view.revision + 1, operation, mutations.last().operation_id, request, records))).also { it.bytes() }
}

/** Tiny guardian API mirror of Rust `Ledger::record_guardian_note`: a masked, bounded guardian receipt or
 * correction becomes an inert EDIT draft note on the task through the same encrypted ledger/outbox. */
val GUARDIAN_NOTE_PREFIXES = listOf("GUARDIAN RECEIPT v1", "GUARDIAN CORRECTION v1")
fun PersonalLedger.recordGuardianNote(taskId: String, note: String, now: Long): PersonalLedger {
    require(GUARDIAN_NOTE_PREFIXES.any { note.startsWith(it) } && note.toByteArray().size <= 3800) { "Guardian note must carry the guardian prefix and stay within 3800 bytes" }
    val view = view()
    val task = view.tasks.find { it.spec.task_id == taskId } ?: error("Task missing or deleted")
    localOwner(view, task)
    require(task.status != "CANCELLED") { "Task is terminal" }
    return apply(PersonalRequest(PersonalLedger.id(), view.revision, PersonalAction.EDIT, taskId, task.spec.goal, note, null, view.replicaId), now)
}
