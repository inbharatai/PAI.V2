package com.unoone.agent.personal

import android.content.Context
import com.unoone.agent.providers.*
import com.unoone.agent.vaultbridge.VaultConnection

/** Native UI call only. PREPARED is inert; the existing provider exact-effect review remains separate. */
internal fun prepareTaskProviderDraft(context: Context, provider: ProviderService, revision: Long, review: Review) = synchronized(VaultConnection) {
    check(VaultConnection.isBridgeAllowed())
    val epoch = VaultConnection.sessionEpoch()
    val view = PersonalAgentService(context).view()
    val draft = (review.mutation as? ProviderMutation.SaveDraft)?.draft ?: error("Task handoff cannot send")
    require(view.revision == revision && view.conflicts.isEmpty()) { "Source changed; review again" }
    val task = view.tasks.single { it.spec.task_id == review.task_id }
    require(!task.deleted && task.status !in setOf("CANCELLED", "IN_PROGRESS") && task.remoteClaims.isEmpty())
    require(task.ownerEpoch == 1L && task.ownerReplicaId == view.replicaId && task.spec.origin_replica_id == view.replicaId && task.spec.target_replica_id == view.replicaId && review.owner_replica == view.replicaId)
    require(draft.body.isNotBlank() && draft.body == task.draft) { "Exact source body changed" }
    check(epoch == VaultConnection.sessionEpoch())
    provider.prepare(review) // no credentials, network, commit or verified receipt
}
