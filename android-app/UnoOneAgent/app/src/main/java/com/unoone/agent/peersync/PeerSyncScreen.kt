package com.unoone.agent.peersync

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString

/** No rememberSaveable/preferences; backgrounding clears prose and closes live sockets. */
@Composable fun PeerSyncScreen(onBack: () -> Unit) {
    val context = LocalContext.current; val service = remember { PeerSyncService(context) }; val scope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    var view by remember { mutableStateOf<PeerSyncService.View?>(null) }
    var offer by remember { mutableStateOf("") }; var address by remember { mutableStateOf("") }
    var compared by remember { mutableStateOf(false) }; var persona by remember { mutableStateOf(false) }
    var selected by remember { mutableStateOf(setOf<String>()) }; var separate by remember { mutableStateOf(false) }
    var unify by remember { mutableStateOf(false) }; var archive by remember { mutableStateOf(false) }; var sharedTasks by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }; var message by remember { mutableStateOf("") }
    var generation by remember { mutableLongStateOf(0) }; var confirmRevoke by remember { mutableStateOf(false) }
    fun run(block: () -> PeerSyncService.View) {
        if (busy) return
        busy = true; message = ""; val ticket = generation
        scope.launch { try { val result = withContext(Dispatchers.IO) { block() }; if (ticket == generation) { view = result; message = "Local operation completed. Reload the personal board after sync. V2 uses the merged store; review-only pairings stay separate. No task ran." } }
            catch (e: Exception) { if (ticket == generation) message = e.message?.take(400) ?: "Transfer failed; retained data unchanged. Check unlock, local IP and full fingerprints." }
            finally { busy = false } }
    }
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_STOP) { generation++; service.stop(); view = null; offer = ""; message = ""; selected = emptySet(); compared = false } }
        lifecycle.addObserver(observer); onDispose { generation++; service.stop(); lifecycle.removeObserver(observer) }
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        TextButton(onClick = { service.stop(); onBack() }) { Text("Back") }
        Text("Local peer sync · explicit shared identity v2", style = MaterialTheme.typography.headlineSmall)
        Text("Manual LAN IPv4 only, TLS 1.3 mutual authentication. No cloud, master-key sharing, background sending, grants or execution. Wi-Fi Direct and automatic discovery are not verified.")
        Text("Same person refuses mismatched IDs. Keep separate for review preserves both identities and histories without merging your task board. Both screens must choose the same option. Unify archives old local records without rebinding, and adopts person/agent from the smaller replica ID only after both screens approve and authenticate. Vaults, replica IDs and keys stay distinct.")
        Button(enabled = !busy, onClick = { run(service::view) }) { Text("Show my pairing identity") }
        if (message.isNotEmpty()) Text(message)
        view?.let { v ->
            val s = v.state
            Text("Compare entire SHA-256 fingerprint directly on both screens:")
            OutlinedTextField(value = s.local.fingerprint, onValueChange = {}, readOnly = true, label = { Text("My fingerprint") }, modifier = Modifier.fillMaxWidth())
            OutlinedTextField(value = PeerProtocol.json.encodeToString(s.local), onValueChange = {}, readOnly = true, label = { Text("My manual pairing offer (public IDs)") }, modifier = Modifier.fillMaxWidth())
            if (s.peer == null) {
                OutlinedTextField(value = offer, onValueChange = { if (it.length <= 2048) { offer = it; compared = false } }, label = { Text("Paste Power's public pairing offer") }, modifier = Modifier.fillMaxWidth())
                Row { Checkbox(checked = compared, onCheckedChange = { compared = it }); Text("I compared the entire fingerprint with Power’s screen, not a network message or short code.") }
                Row { Checkbox(checked = separate, onCheckedChange = { separate = it }); Text(if (separate) "Keep separate for review (no adoption/overwrite)" else "Same person (refuse mismatched IDs)") }
                Row { Checkbox(checked = unify, onCheckedChange = { unify = it; archive = false }); Text("Unify identity and archive old local board (v2)") }
                if (unify) {
                    Text("Starts a new shared board. Old persona/tasks remain encrypted in a local immutable archive, NOT rebound or sent. No archive import/reset/rollback UI. Review selections for sensitive prose.")
                    Row { Checkbox(checked = archive, onCheckedChange = { archive = it }); Text("I explicitly approve archiving my board and adopting the deterministic shared identity after both screens approve.") }
                    Row { Checkbox(checked = sharedTasks, onCheckedChange = { sharedTasks = it }); Text("Share all new shared tasks, drafts/notes, events, reminders and tombstones") }
                }
                Text("Select records this phone may send. Fixed selection; new tasks are not automatically shared. Check selected prose for secrets/raw mail before approval.")
                Row { Checkbox(checked = persona, onCheckedChange = { persona = it }); Text("Share persona/name and retained preference history") }
                if (!unify) v.local.tasks.forEach { t -> Row { Checkbox(checked = t.spec.task_id in selected, onCheckedChange = { checked -> selected = if (checked) selected + t.spec.task_id else selected - t.spec.task_id }); Text(t.spec.goal + " (draft/events/reminder/deletion history)") } }
                Button(enabled = !busy && compared && offer.isNotBlank() && (!unify || archive), onClick = { run { service.approve(offer, PeerSelection(persona, if (unify) { if (sharedTasks) listOf("00000000-0000-0000-0000-000000000000") else emptyList() } else selected.toList()), if (unify) IdentityChoice.UNIFY_ARCHIVE else if (separate) IdentityChoice.KEEP_SEPARATE_REVIEW else IdentityChoice.SAME_PERSON, compared) } }) { Text("Approve fingerprint and selected records") }
            } else {
                Text("Peer fingerprint: ${s.peer.offer.fingerprint}")
                Text(if (s.peer.revoked) "REVOKED — future connections blocked; old copies cannot be erased remotely." else "Approved here. Power must also approve this phone, then choose Listen for one page.")
                Text("Received operations: ${s.received.size} · Acknowledged outbound: ${s.sent_ack} · Pending sequence positions: ${(v.local.pendingMutations - s.sent_ack).coerceAtLeast(0)}")
                OutlinedTextField(value = address, onValueChange = { if (it.length <= 80) address = it }, label = { Text("Power LAN IPv4:port, e.g. 192.168.1.12:43123") }, modifier = Modifier.fillMaxWidth())
                Button(enabled = !busy && !s.peer.revoked && address.isNotBlank(), onClick = { run { service.sync(address) } }) { Text("Sync one page now") }
                TextButton(onClick = { generation++; service.stop(); message = "Session stopped. Reload to reconcile persisted pages." }) { Text("Stop session") }
                TextButton(enabled = !busy && !s.peer.revoked, onClick = { confirmRevoke = true }) { Text("Revoke peer") }
                Text(if (s.peer.choice == IdentityChoice.UNIFY_ARCHIVE) "Shared v2: reload Personal agent to view/edit merged causal projection. Conflicts retained for review. Receipt and handoff claims are inert; execution disabled." else "PEER_REVIEW_ONLY: histories stay separate. No reset/re-pair/compaction.")
                s.personaReview()?.let { Text("Persona conflict review: $it") }
                s.remoteTasks().forEach { t ->
                    Text(if (t.deleted) "Deleted task (tombstone retained)" else t.goal, style = MaterialTheme.typography.titleMedium)
                    Text("${t.status} · no execution authority")
                    if (!t.deleted) { Text(t.draft); t.events.forEach { e -> Text("${e.transition} · step ${e.step} · evidence ${e.evidence_ref ?: "none"}") }; t.snooze?.let { Text("Manual reminder: ${java.time.Instant.ofEpochMilli(it)}") } }
                }
            }
            Text("At most 8 changes/page, 256 KiB HTTP body, 2048 retained operations and 4 MiB encrypted review state. No eviction. Logical deletion, not secure erasure. No conversation, provider account, model or skill sync.")
        }
    }
    if (confirmRevoke) AlertDialog(onDismissRequest = { confirmRevoke = false }, title = { Text("Revoke this peer?") }, text = { Text("Future sync stops. Previous copies remain. V1 has no reset/re-pair flow.") }, confirmButton = { TextButton(onClick = { confirmRevoke = false; run(service::revoke) }) { Text("Revoke") } }, dismissButton = { TextButton(onClick = { confirmRevoke = false }) { Text("Keep peer") } })
}
