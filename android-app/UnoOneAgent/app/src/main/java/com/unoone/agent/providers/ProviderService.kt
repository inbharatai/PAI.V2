package com.unoone.agent.providers

import android.content.Context
import android.accounts.Account
import android.app.Activity
import android.content.Intent
import com.google.android.gms.auth.api.identity.AuthorizationRequest
import com.google.android.gms.auth.api.identity.AuthorizationResult
import com.google.android.gms.auth.api.identity.Identity
import com.google.android.gms.auth.api.identity.RevokeAccessRequest
import com.google.android.gms.common.api.Scope
import com.unoone.agent.vaultbridge.VaultConnection
import com.unoone.agent.personal.*
import kotlinx.serialization.json.JsonObject

/** Native UI only; never exposed to ActionExecutor, model tools, grants from JSON or sync. */
internal class ProviderService(context: Context) {
    private val context=context.applicationContext
    private var pendingVault: String? = null
    private var pendingEpoch: Long = -1
    private var pendingStarted: Long = 0
    private fun <T> local(block: (ProviderStore,ProviderLocalState)->T): T = synchronized(VaultConnection) {
        check(VaultConnection.isBridgeAllowed()){ "Unlock local vault; migration quarantine must be resolved" }
        val epoch=VaultConnection.sessionEpoch();val id=checkNotNull(VaultConnection.localVaultId());val store=ProviderStore(context,id);val result=block(store,store.load());check(epoch==VaultConnection.sessionEpoch()){ "Vault session changed; reload/reconcile" };result
    }
    fun view(): ProviderView = local {_,s->ProviderView(if(s.config==null)"UNCONFIGURED" else if(s.tokens==null)"DISCONNECTED" else "CONNECTED_NOT_QUALIFIED",s.tokens?.account,s.config!=null,s.entries.toList())}
    fun configure(clientId: String) = local {store,s->check(s.tokens==null){"Disconnect first"};val config=AndroidOAuthConfig(clientId);config.validate();s.config=config;store.save(s)}
    /** Google does not support desktop loopback/custom-scheme OAuth on Android.
     * Official SDK launches a Google-owned resolution PendingIntent. Its registered
     * package/signer determines Android client, not a web/server/desktop client ID. */
    fun authorize(activity: Activity, write: Boolean, onResolution: (android.app.PendingIntent)->Unit, onResult: (AuthorizationResult)->Unit, onError: (String)->Unit) {
        try {
            local {_,s->checkNotNull(s.config){"UNCONFIGURED"}.validate();pendingVault=s.vaultId;pendingEpoch=VaultConnection.sessionEpoch();pendingStarted=android.os.SystemClock.elapsedRealtime()}
            val scopes=READ_SCOPES + if(write)listOf("https://www.googleapis.com/auth/gmail.compose","https://www.googleapis.com/auth/gmail.modify","https://www.googleapis.com/auth/calendar.events") else emptyList()
            val request=AuthorizationRequest.builder().setRequestedScopes(scopes.map(::Scope)).build()
            Identity.getAuthorizationClient(activity).authorize(request).addOnSuccessListener { result ->
                if(result.hasResolution()) { val resolution=result.pendingIntent;if(resolution!=null)onResolution(resolution) else onError("Authorization resolution missing") } else onResult(result)
            }.addOnFailureListener { onError("Google authorization unavailable or declined; no connection. Check registered package/signer and Play services.") }
        } catch(_:Exception) {onError("UNCONFIGURED or locked: configure registered Android client first")}
    }
    fun resolution(intent: Intent?): AuthorizationResult = Identity.getAuthorizationClient(context).getAuthorizationResultFromIntent(intent)
    fun accept(result: AuthorizationResult) = local {store,s->
        check(pendingVault==s.vaultId && pendingEpoch==VaultConnection.sessionEpoch() && android.os.SystemClock.elapsedRealtime()-pendingStarted in 0..180_000){"Authorization session changed; reconnect"};pendingVault=null
        val access=checkNotNull(result.accessToken);require(access.isNotEmpty() && access.length<=8192)
        require(READ_SCOPES.all {it in result.grantedScopes}){"Required read scopes not granted"}
        val api=GoogleHttpAdapter(access,"",result.grantedScopes.toSet());val account=api.profile()
        require(s.tokens==null || s.tokens?.account==account){"Account changed; disconnect old account first"}
        // No refresh token requested/stored on Android. Reauthorize with SDK as needed.
        s.tokens=ProviderTokens(access,account,result.grantedScopes.toSet(),System.currentTimeMillis()+45*60*1000);store.save(s)
    }
    fun disconnect(onRemoteResult: (String)->Unit) {
        val old=local {store,s->val old=s.tokens;s.tokens=null;s.entries.filter{it.status==CommitStatus.PREPARED}.forEach{it.status=CommitStatus.REJECTED};store.save(s);old}
        pendingVault=null
        if(old==null){onRemoteResult("Local access revoked");return}
        val request=RevokeAccessRequest.builder().setAccount(Account(old.account,"com.google")).setScopes(old.scopes.map(::Scope)).build()
        Identity.getAuthorizationClient(context).revokeAccess(request).addOnSuccessListener {onRemoteResult("Local and Google access revoked")}.addOnFailureListener {onRemoteResult("Local access revoked; remote revocation unconfirmed. Remove app access in Google Account settings.")}
    }
    private fun api(s: ProviderLocalState): GoogleHttpAdapter {val token=checkNotNull(s.tokens){"Connect Google first"};check(System.currentTimeMillis()<token.expiresMs){"Token expired; reconnect through Google SDK"};return GoogleHttpAdapter(token.access,token.account,token.scopes)}
    fun read(operation: String,container: String="",query: String="",id: String="",start: String="",end: String="",page: String?=null): JsonObject = local {_,s->val api=api(s);when(operation){"SEARCH"->api.search(container,query,page);"MESSAGE"->api.message(container,id);"THREAD"->api.thread(container,id);"LABELS"->api.labels();"CALENDARS"->api.calendars(page);"EVENTS"->api.events(container,start,end,page);"FREE_BUSY"->api.freeBusy(container,start,end);else->error("Unsupported read")}}
    private fun note(review: Review,detail: String) {
        val service=PersonalAgentService(context);val v=service.view();val task=v.tasks.firstOrNull{it.spec.task_id==review.task_id && !it.deleted}?:error("Choose an existing task")
        require(v.conflicts.isEmpty() && review.owner_replica==v.replicaId && (task.ownerReplicaId==null || task.ownerReplicaId==v.replicaId) && task.status!="CANCELLED"){"Conflicted/cancelled/foreign task owner; held"}
        if(detail.startsWith("NEEDS_RECONCILIATION"))require(task.draft.contains(review.operation_id) && task.draft.contains(review.digest())){"Task changed after preparation; fresh review required"}
        service.mutate(PersonalRequest(PersonalLedger.id(),v.revision,PersonalAction.EDIT,review.task_id,task.spec.goal,"Provider ${review.operation_id} · ${review.account} · ${review.container} · ${review.digest()}\n$detail\nDeclarative note, not execution authority. Exact review remains device-local.",null,v.replicaId))
    }
    fun prepare(review: Review) = local {store,s->
        review.validate(System.currentTimeMillis());require(s.entries.size<128 && s.entries.none{it.review.operation_id==review.operation_id || (it.review.task_id==review.task_id && it.status==CommitStatus.NEEDS_RECONCILIATION)}){"Uncertain existing task attempt must reconcile before preparing again"};require(s.tokens==null || s.tokens?.account==review.account)
        note(review,"PREPARED locally; no external effect")
        s.entries.add(PreparedEntry(review,review.digest()));store.save(s)
    }
    /** §3.6 guardian decision shown BEFORE commit; no effect. */
    fun guardianPreview(operationId: String): com.unoone.agent.core.guardian.Decision? = local {_,s->val entry=s.entries.single{it.review.operation_id==operationId};ProviderGuardian.decision(entry.review,ProviderGuardian.context(s))}
    fun consentPreview(write: Boolean): String = ProviderGuardian.googleManifest(null,write,System.currentTimeMillis()).consentPreview()
    /** Visible report/correct-warning control: records a correction note on the task through the runtime's tiny API. */
    fun guardianCorrection(taskId: String,fingerprint: String,kind: com.unoone.agent.core.guardian.CorrectionKind,comment: String) {
        require(fingerprint.length<=2048 && comment.length<=2048)
        PersonalAgentService(context).withStore {store->store.save(store.load().recordGuardianNote(taskId,com.unoone.agent.core.guardian.Correction.create(kind,fingerprint,comment,System.currentTimeMillis()).ledgerNote(),System.currentTimeMillis()))}
    }
    fun commit(operationId: String,exactDigest: String,acknowledgedFingerprint: String?=null): ProviderReceipt = local {store,s->
        val entry=s.entries.single{it.review.operation_id==operationId};check(entry.status==CommitStatus.PREPARED){"Already attempted; reconcile, never replay"}
        val api=api(s);val grant=CapabilityGrant.fromNativeReview(entry.review,exactDigest,System.currentTimeMillis());grant.check(entry.review,api.account,System.currentTimeMillis())
        // §3.6 guardian BEFORE reserving the attempt: BLOCK/unacknowledged WARN records a refusal receipt and dispatches nothing.
        val ctx=ProviderGuardian.context(s)
        val guardianReceipt=try { ProviderGuardian.enforce(entry.review,ctx,acknowledgedFingerprint,System.currentTimeMillis()) }
            catch(e: com.unoone.agent.core.guardian.GuardianRefusal) { runCatching { PersonalAgentService(context).withStore {ps->ps.save(ps.load().recordGuardianNote(entry.review.task_id,e.receipt.ledgerNote(),System.currentTimeMillis()))} }; throw e }
        entry.status=CommitStatus.NEEDS_RECONCILIATION;store.save(s) // crash-safe reservation before network
        note(entry.review,"NEEDS_RECONCILIATION: attempt reserved; automatic retries forbidden")
        guardianReceipt?.let {r->PersonalAgentService(context).withStore {ps->ps.save(ps.load().recordGuardianNote(entry.review.task_id,r.ledgerNote(),System.currentTimeMillis()))}}
        val receipt=api.withGuardian(ctx,acknowledgedFingerprint).commit(entry.review,grant);entry.receipt=receipt;entry.status=receipt.status;store.save(s)
        note(entry.review,"Provider readback VERIFIED: ${receipt.provider_id}. Task remains review-only.");receipt
    }
    fun reconcile(operationId: String,providerId: String): ProviderReceipt = local {store,s->
        val entry=s.entries.single{it.review.operation_id==operationId};check(entry.status==CommitStatus.NEEDS_RECONCILIATION)
        val receipt=api(s).verify(entry.review,providerId);entry.receipt=receipt;entry.status=receipt.status;store.save(s)
        note(entry.review,"Provider reconciliation VERIFIED: ${receipt.provider_id}; no resend occurred");receipt
    }
}
