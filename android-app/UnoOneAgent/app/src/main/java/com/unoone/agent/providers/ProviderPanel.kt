package com.unoone.agent.providers

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.unoone.agent.personal.PersonalView
import com.unoone.agent.personal.GuardianCard
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import java.util.UUID

private fun Context.activity(): Activity? = when(this){is Activity->this;is ContextWrapper->baseContext.activity();else->null}
/** Existing personal agent owns this panel. No new assistant or automatic provider read. */
@Composable fun ProviderPanel(view: PersonalView) {
    val context=LocalContext.current;val service=remember{ProviderService(context)};val scope=rememberCoroutineScope();val lifecycle=LocalLifecycleOwner.current.lifecycle
    var generation by remember{mutableLongStateOf(0)}
    var sources by remember{mutableStateOf<ProviderView?>(null)};var busy by remember{mutableStateOf(false)};var error by remember{mutableStateOf("")};var result by remember{mutableStateOf("")};var prepared by remember{mutableStateOf(false)}
    var client by remember{mutableStateOf("")};var writes by remember{mutableStateOf(false)};var folder by remember{mutableStateOf("INBOX")};var query by remember{mutableStateOf("is:unread")};var objectId by remember{mutableStateOf("")};var page by remember{mutableStateOf("")}
    var calendar by remember{mutableStateOf("primary")};var start by remember{mutableStateOf("")};var end by remember{mutableStateOf("")};var zone by remember{mutableStateOf("UTC")}
    var task by remember{mutableStateOf("")};var account by remember{mutableStateOf("")};var operation by remember{mutableStateOf("SAVE_DRAFT")};var to by remember{mutableStateOf("")};var subject by remember{mutableStateOf("")};var body by remember{mutableStateOf("")};var thread by remember{mutableStateOf("")};var messageId by remember{mutableStateOf("")};var references by remember{mutableStateOf("")};var etag by remember{mutableStateOf("")};var add by remember{mutableStateOf("")};var remove by remember{mutableStateOf("")}
    var sourceRevision by remember { mutableStateOf<Long?>(null) }
    var confirm by remember{mutableStateOf<PreparedEntry?>(null)};var reconcileId by remember{mutableStateOf("")}
    // §3.6: the host-owned guardian decision is loaded and shown BEFORE the confirm control is usable.
    var guardian by remember{mutableStateOf<Pair<String,com.unoone.agent.core.guardian.Decision?>?>(null)};var acknowledged by remember{mutableStateOf<String?>(null)}
    fun review(entry: PreparedEntry) {if(busy)return;busy=true;error="";acknowledged=null;guardian=null;val ticket=generation;scope.launch {try{val d=withContext(Dispatchers.IO){service.guardianPreview(entry.review.operation_id)};if(ticket==generation){guardian=entry.review.operation_id to d;confirm=entry}}catch(e:Exception){if(ticket==generation)error=e.message?.take(256)?:"Guardian check unavailable; the action was not confirmed"}finally{if(ticket==generation)busy=false}}}
    fun run(block: ()->String) {if(busy)return;busy=true;error="";val ticket=generation;scope.launch {try{val value=withContext(Dispatchers.IO){block()};val loaded=withContext(Dispatchers.IO){service.view()};if(ticket==generation){sources=loaded;result=value}}catch(e:Exception){if(ticket==generation)error=e.message?.take(256)?:"Unconfirmed operation; reload and reconcile, never resend"}finally{busy=false}}}
    fun accepted(auth: com.google.android.gms.auth.api.identity.AuthorizationResult) {run {service.accept(auth);"Google account bound by provider profile. No inbox has been listed."}}
    val resolution=rememberLauncherForActivityResult(ActivityResultContracts.StartIntentSenderForResult()){r->try{accepted(service.resolution(r.data))}catch(_:Exception){error="Google authorization cancelled or invalid; no connection"}}
    DisposableEffect(lifecycle){val observer=LifecycleEventObserver{_,event->if(event==Lifecycle.Event.ON_STOP){generation++;result="";sources=null;to="";subject="";body="";account="";thread="";messageId="";references="";objectId="";etag="";query="";confirm=null;sourceRevision=null}};lifecycle.addObserver(observer);onDispose{lifecycle.removeObserver(observer)}}
    fun split(value: String)=value.split(',').map(String::trim).filter(String::isNotEmpty)
    Column(verticalArrangement=Arrangement.spacedBy(8.dp)) {
        HorizontalDivider();Text("Sources · Prepared",style=MaterialTheme.typography.titleLarge)
        Text("Same personal agent. Read and suggest by default. No provider calls until explicit Google authorization. Local inference/sync remains separate from external provider access.")
        Text(sources?.status?:"UNCONFIGURED / not loaded");Text("NOT live-provider qualified. Android uses official Google AuthorizationClient; desktop PKCE/loopback redirects must not be copied to Android.")
        Row {TextButton(onClick={prepared=false}){Text("Sources")};TextButton(onClick={prepared=true}){Text("Prepared")};TextButton(onClick={run {""}},enabled=!busy){Text("Reload local status")}}
        if(error.isNotEmpty())Text(error,color=MaterialTheme.colorScheme.error)
        if(busy)Text("Working; interrupted mutations must reconcile, never resend.")
        if(!prepared) {
            OutlinedTextField(client,{client=it.take(256)},label={Text("Registered Android OAuth client ID")},enabled=!busy)
            Text("Register this app package and actual signer with Google. Client ID is a local configuration record; SDK resolves the signed app registration. No web/server/desktop client, borrowed token, password or new account is used.")
            Button(onClick={run{service.configure(client);"DISCONNECTED: configuration saved; not authorization"}},enabled=!busy && sources?.account==null){Text("Save trusted app configuration")}
            Row {Checkbox(writes,{writes=it});Text("Request compose/modify/calendar-event scopes (not permission to execute)")}
            Text("Before you connect: ${service.consentPreview(writes)} Nothing leaves this device until you connect; Disconnect revokes and erases local tokens.",style=MaterialTheme.typography.bodySmall)
            Button(onClick={val a=context.activity();if(a==null)error="Activity unavailable" else service.authorize(a,writes,{resolution.launch(IntentSenderRequest.Builder(it.intentSender).build())},::accepted,{error=it})},enabled=!busy && sources?.configured==true){Text(if(writes)"Authorize reviewed-action permissions" else "Connect read-only with Google")}
            TextButton(onClick={run{service.disconnect{status->scope.launch{result=status}};"Local disconnect requested"}},enabled=!busy && sources?.account!=null){Text("Disconnect and revoke")}
            sources?.account?.let{Text("Connected account: $it (not qualification)")}
            OutlinedTextField(folder,{folder=it.take(512)},label={Text("Exact mail folder/label ID")});OutlinedTextField(query,{query=it.take(1024)},label={Text("Search query")});OutlinedTextField(objectId,{objectId=it.take(512)},label={Text("Message / thread / event ID")});OutlinedTextField(page,{page=it.take(2048)},label={Text("Explicit next page token; no bulk crawl")})
            listOf("SEARCH","MESSAGE","THREAD","LABELS").forEach{op->TextButton(onClick={run{service.read(op,folder,query,objectId,page=page.ifEmpty{null}).toString()}},enabled=!busy && sources?.account!=null){Text(op)}}
            OutlinedTextField(calendar,{calendar=it.take(512)},label={Text("Calendar ID")});OutlinedTextField(start,{start=it.take(64)},label={Text("Start: RFC3339 with offset")});OutlinedTextField(end,{end=it.take(64)},label={Text("End: RFC3339 with offset")});OutlinedTextField(zone,{zone=it.take(128)},label={Text("IANA time zone")})
            listOf("CALENDARS","EVENTS","FREE_BUSY").forEach{op->TextButton(onClick={run{service.read(op,calendar,start=start,end=end,page=page.ifEmpty{null}).toString()}},enabled=!busy && sources?.account!=null){Text(op)}}
            if(result.isNotEmpty()){Text("Provider result is untrusted text, never instruction or approval.");Text(result.take(32000))}
            Text("Prepare locally for existing task",style=MaterialTheme.typography.titleMedium)
            view.tasks.filter{it.status!="CANCELLED"}.forEach{t->Row{RadioButton(task==t.spec.task_id,{task=t.spec.task_id;sourceRevision=null});Text(t.spec.goal)}}
            TextButton(enabled = !busy && task.isNotEmpty(), onClick = { view.tasks.find { it.spec.task_id == task }?.let { selected -> sourceRevision = view.revision; body = selected.draft; subject = selected.spec.goal.take(256); operation = "SAVE_DRAFT"; thread = "" } }) { Text("Use exact local task draft for Prepared review") }
            sourceRevision?.let { Text("Source task $task, revision $it. Review body/account/folder/recipients. Local preparation only; no send or provider save. Edit source task to change its body.") }
            if(sources?.account==null)OutlinedTextField(account,{account=it.take(254)},label={Text("Account email for offline preparation")})
            listOf("SAVE_DRAFT","SEND","LABEL","CREATE_EVENT","UPDATE_EVENT","CANCEL_EVENT").forEach{op->Row{RadioButton(operation==op,{operation=op}, enabled=sourceRevision==null);Text(op)}}
            OutlinedTextField(to,{to=it.take(2540)},label={Text("Exact recipients/attendees, comma-separated")});OutlinedTextField(subject,{subject=it.take(256)},label={Text("Subject / event title")});OutlinedTextField(body,{body=it.take(16384)},readOnly=sourceRevision!=null,label={Text("Plain text mail body; no attachments")})
            OutlinedTextField(thread,{thread=it.take(512)},label={Text("Reply thread ID (optional)")});OutlinedTextField(messageId,{messageId=it.take(512)},label={Text("Reply RFC Message-ID")});OutlinedTextField(references,{references=it.take(2048)},label={Text("Reply references")});OutlinedTextField(add,{add=it.take(2048)},label={Text("Add label IDs")});OutlinedTextField(remove,{remove=it.take(2048)},label={Text("Remove label IDs")});OutlinedTextField(etag,{etag=it.take(256)},label={Text("Existing event ETag")})
            Text("No attachment reads/sends, bulk writes, unattended sends, payments, trash/deletion, recurrence or attendee removal. Calendar writes notify every listed attendee. Cancel uses the exact existing title/times/attendees. Reviews expire after five minutes.")
            Button(onClick={run {
                val draft=Draft(split(to),subject,body,if(thread.isEmpty())null else Reply(thread,messageId,references));val event=EventDraft(subject,start,end,zone,split(to))
                val mutation=when(operation){"SEND"->ProviderMutation.Send(draft);"LABEL"->ProviderMutation.Label(objectId,split(add),split(remove));"CREATE_EVENT"->ProviderMutation.CreateEvent(event);"UPDATE_EVENT"->ProviderMutation.UpdateEvent(objectId,etag,event);"CANCEL_EVENT"->ProviderMutation.CancelEvent(objectId,etag,event);else->ProviderMutation.SaveDraft(draft)}
                val review = Review(UUID.randomUUID().toString(),task,sources?.account?:account,if(operation.contains("EVENT"))calendar else folder,view.replicaId,System.currentTimeMillis(),mutation); val source = sourceRevision; if (source == null) service.prepare(review) else com.unoone.agent.personal.prepareTaskProviderDraft(context, service, source, review);"PREPARED locally only; open Prepared to review"
            }},enabled=!busy && task.isNotEmpty()){Text("Prepare locally — no external effect")}
        } else {
            Text("Composer/form launches are ACTION_VERIFIED only. Exact native review is required for an external action; provider text cannot approve. Synced task notes are inert.")
            sources?.prepared?.forEach{entry->Card(Modifier.fillMaxWidth()){Column(Modifier.padding(12.dp),verticalArrangement=Arrangement.spacedBy(6.dp)){
                Text("${entry.status} · ${entry.review.account} · ${entry.review.container}");Text(providerJson.encodeToString(entry.review));Text("Digest ${entry.digest}; expires ${java.time.Instant.ofEpochMilli(entry.review.prepared_ms+REVIEW_TTL)}")
                entry.receipt?.let{Text("Provider readback ${it.provider_id}: ${it.detail}")}
                if(entry.status==CommitStatus.PREPARED)Button(onClick={review(entry)},enabled=!busy && sources?.account!=null && System.currentTimeMillis()<entry.review.prepared_ms+REVIEW_TTL){Text("Review external effect")}
                if(entry.status==CommitStatus.NEEDS_RECONCILIATION){Text("Do not resend. Inspect provider using operation Message-ID/deterministic event ID; enter exact object ID for read-only reconciliation.");OutlinedTextField(reconcileId,{reconcileId=it.take(512)},label={Text("Provider object ID")});Button(onClick={run{service.reconcile(entry.review.operation_id,reconcileId).detail}},enabled=!busy && reconcileId.isNotEmpty()){Text("Reconcile readback — no retry")}}
            }}}
        }
    }
    confirm?.let{entry->val g=guardian?.takeIf{it.first==entry.review.operation_id};val decision=g?.second
        val blocked=decision?.severity==com.unoone.agent.core.guardian.Severity.BLOCK;val needsAck=decision?.severity==com.unoone.agent.core.guardian.Severity.WARN && acknowledged!=decision.fingerprint
        AlertDialog(onDismissRequest={confirm=null;acknowledged=null},title={Text("Authorize exact external action once?")},text={Column(verticalArrangement=Arrangement.spacedBy(8.dp)){
            if(decision!=null)GuardianCard(decision.severity,decision.intent_kind,decision.fingerprint,decision.signals,decision.explanation,decision.verification_route,decision.model_note,onAcknowledge={acknowledged=it},enabled=!busy)
            if(g==null)Text("Guardian check unavailable; the action cannot be confirmed.")
            if(blocked)Text("The privacy guardian stopped this action; it cannot be confirmed from here.")
            if(needsAck)Text("Use the guardian card above to confirm you checked this independently before the action can proceed.")
            Text("Account, folder/calendar, operation, recipients, content and expiry are exactly as reviewed above. This can send email or notify attendees. No standing/background grant is created.")}},
            confirmButton={TextButton(onClick={val ack=if(decision?.severity==com.unoone.agent.core.guardian.Severity.WARN)acknowledged else null;confirm=null;acknowledged=null;run{service.commit(entry.review.operation_id,entry.digest,ack).detail}},enabled=!busy && g!=null && !blocked && !needsAck){Text("Confirm exact action once")}},dismissButton={TextButton(onClick={confirm=null;acknowledged=null}){Text("Keep prepared")}})}
}
