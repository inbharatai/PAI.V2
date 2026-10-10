package com.unoone.agent.personal

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
import java.time.Instant

/** All fields use remember (NOT rememberSaveable) so private drafts never enter saved-state. */
@Composable
fun PersonalAgentScreen(onBack: () -> Unit) {
    val context = LocalContext.current
    val service = remember { PersonalAgentService(context) }
    val scope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    var view by remember { mutableStateOf<PersonalView?>(null) }
    var error by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var preferences by remember { mutableStateOf("") }
    var goal by remember { mutableStateOf("") }
    var draft by remember { mutableStateOf("") }
    var editing by remember { mutableStateOf<String?>(null) }
    var selected by remember { mutableStateOf<String?>(null) }
    var deleting by remember { mutableStateOf<String?>(null) }
    var clearing by remember { mutableStateOf(false) }
    var generation by remember { mutableLongStateOf(0) }
    fun loaded(v: PersonalView) { view = v; name = v.agent.display_name; preferences = v.persona.preferences.joinToString("\n") { it.value } }
    fun reload() {
        if (busy) return
        busy = true; error = ""; val ticket = generation
        scope.launch {
            try { val v = withContext(Dispatchers.IO) { service.view() }; if (ticket == generation) loaded(v) }
            catch (_: Exception) { if (ticket == generation) { view = null; error = "Unlock your local file vault first. A historical migration hold or corrupt ledger is retained, never reset." } }
            finally { busy = false }
        }
    }
    fun mutate(action: PersonalAction, taskId: String? = null, text: String = "", body: String = "", until: Long? = null) {
        val v = view ?: return
        if (busy) return
        busy = true; error = ""; val ticket = generation
        val request = PersonalRequest(PersonalLedger.id(), v.revision, action, taskId, text, body, until, v.replicaId)
        scope.launch {
            try {
                val result = withContext(Dispatchers.IO) { service.mutate(request) }
                if (ticket == generation) {
                    loaded(result); deleting = null; clearing = false
                    if (action in setOf(PersonalAction.CREATE, PersonalAction.EDIT)) { goal = ""; draft = ""; editing = null }
                }
            } catch (_: Exception) { if (ticket == generation) error = "Change not confirmed. Reload after a concurrent edit, lock or write error before retrying. Nothing was executed." }
            finally { busy = false }
        }
    }
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP) { generation++; view = null; name = ""; preferences = ""; goal = ""; draft = ""; editing = null; selected = null; deleting = null; clearing = false }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    LaunchedEffect(Unit) { reload() }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        TextButton(onClick = onBack) { Text("Back") }
        Text(view?.agent?.display_name ?: "Your personal agent", style = MaterialTheme.typography.headlineMedium)
        Text("One identity; specialists stay internal. Local tasks are manually reviewed. Accepting does not run a task.")
        Text("Before explicit adoption, this installation keeps its own person/replica. After both screens approve v2 and authenticate, this active board reads the merged causal store. Replicas, vaults and keys remain distinct. Conversation sync and automatic reminders are not connected.")
        if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error)
        Button(onClick = { reload() }, enabled = !busy) { Text("Reload ledger") }
        view?.let { v ->
            Text("Agent: ${v.agent.agent_id}\nPerson: ${v.agent.person_id}\nReplica: ${v.replicaId}\nEncrypted pending mutations: ${v.pendingMutations}", style = MaterialTheme.typography.bodySmall)
            if (v.archivedMutations > 0) Text("Archived local mutations: ${v.archivedMutations}. Old identities/tasks retained unchanged, not automatically rebound or imported.")
            v.conflicts.forEach { Text("Conflict review — retained alternatives: $it") }
            Text("Persona", style = MaterialTheme.typography.titleLarge)
            OutlinedTextField(name, { if (it.toByteArray().size <= 4096) name = it }, label = { Text("Assistant name") }, enabled = !busy)
            OutlinedTextField(preferences, { if (it.toByteArray().size <= 4096) preferences = it }, label = { Text("Response preferences") }, enabled = !busy)
            Text("Private · revision ${v.persona.revision} · source ${v.persona.provenance.source}. Approved resolved preferences apply to Personal conversation in the Agent screen. Revoked, stale or conflicted preferences are skipped; they never authorize tools.")
            Button(onClick = { mutate(PersonalAction.PERSONA, text = name, body = preferences) }, enabled = !busy && name.isNotBlank()) { Text("Save persona") }
            TextButton(onClick = { clearing = true }, enabled = !busy) { Text("Clear preferences") }
            if (v.persona.deleted) Text("Preferences cleared (tombstoned).")
            Text(if (editing == null) "New personal task" else "Edit task", style = MaterialTheme.typography.titleLarge)
            OutlinedTextField(goal, { if (it.toByteArray().size <= 4096) goal = it }, label = { Text("Task goal") }, enabled = !busy)
            OutlinedTextField(draft, { if (it.toByteArray().size <= 4096) draft = it }, label = { Text("Private draft") }, enabled = !busy)
            Button(onClick = { mutate(if (editing == null) PersonalAction.CREATE else PersonalAction.EDIT, editing ?: PersonalLedger.id(), goal, draft) }, enabled = !busy && goal.isNotBlank()) { Text(if (editing == null) "Create task" else "Save task edit") }
            if (editing != null) TextButton(onClick = { editing = null; goal = ""; draft = "" }) { Text("Discard edit") }
            Text("No simulated AI. Drafts and metadata persist in your encrypted vault, not plaintext task logs. Limits: 128 task IDs including deleted, 2048 mutations or 4 MiB, whichever comes first. Pending data is never evicted; reviewed compaction is not yet available.")
            Text("Tasks", style = MaterialTheme.typography.titleLarge)
            if (v.tasks.isEmpty()) Text("No personal tasks yet.")
            v.tasks.forEach { task ->
                Card(Modifier.fillMaxWidth()) { Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(task.spec.goal, style = MaterialTheme.typography.titleMedium)
                    Text(task.status)
                    Text("Declarative owner: ${task.ownerReplicaId ?: "local"} · epoch ${task.ownerEpoch}. No execution authority.")
                    task.snoozeUntilMs?.let { Text("Snoozed until ${Instant.ofEpochMilli(it)} (manual reminder only)") }
                    TextButton(onClick = { selected = task.spec.task_id }) { Text("View timeline") }
                    if (task.status != "CANCELLED") {
                        TextButton(onClick = { editing = task.spec.task_id; goal = task.spec.goal; draft = task.draft }, enabled = !busy) { Text("Edit") }
                        Button(onClick = { mutate(PersonalAction.ACCEPT, task.spec.task_id) }, enabled = !busy && task.status != "READY_FOR_REVIEW") { Text("Accept for review") }
                        TextButton(onClick = { mutate(PersonalAction.SNOOZE, task.spec.task_id, until = System.currentTimeMillis() + 86400000) }, enabled = !busy) { Text("Snooze 1 day") }
                        TextButton(onClick = { mutate(PersonalAction.CANCEL, task.spec.task_id) }, enabled = !busy) { Text("Cancel task") }
                    }
                    TextButton(onClick = { deleting = task.spec.task_id }, enabled = !busy) { Text("Delete task") }
                    if (selected == task.spec.task_id) {
                        Text("Remote observed claims (not native completion): ${task.remoteClaims}")
                        Text(task.spec.user_visible_policy)
                        val receipt = parseGuardianReceipt(task.draft)
                        if (receipt != null) GuardianReceiptCard(receipt, enabled = !busy, onCorrect = { kind, comment ->
                            if (!busy) { busy = true; error = ""; val ticket = generation; scope.launch { var ok = false; try { withContext(Dispatchers.IO) { service.withStore { store -> store.save(store.load().recordGuardianNote(task.spec.task_id, com.unoone.agent.core.guardian.Correction.create(kind, receipt.fingerprint, comment, System.currentTimeMillis()).ledgerNote(), System.currentTimeMillis())) } }; ok = true } catch (e: Exception) { if (ticket == generation) error = e.message?.take(256) ?: "Correction was not recorded; unlock the vault and retry." } finally { if (ticket == generation) busy = false }; if (ok && ticket == generation) reload() } }
                        }) else if (task.draft.startsWith(com.unoone.agent.core.guardian.GUARDIAN_CORRECTION_PREFIX)) Text(task.draft.substringBefore('\n')) else Text("Draft: ${task.draft}")
                        task.events.forEach { Text("${it.transition} · step ${it.step} · evidence: ${it.evidence_ref ?: "none"}\n${it.event_id}", style = MaterialTheme.typography.bodySmall) }
                        TextButton(onClick = { selected = null }) { Text("Close timeline") }
                    }
                } }
            }
            com.unoone.agent.providers.ProviderPanel(v)
        }
    }
    if (clearing || deleting != null) AlertDialog(onDismissRequest = { clearing = false; deleting = null },
        title = { Text(if (clearing) "Clear preferences?" else "Delete task?") },
        text = { Text("The current record will be hidden by a tombstone. Encrypted history remains for future conflict review; this is not secure erasure.") },
        confirmButton = { TextButton(onClick = { if (clearing) mutate(PersonalAction.CLEAR_PERSONA) else mutate(PersonalAction.DELETE, deleting) }, enabled = !busy) { Text("Confirm") } },
        dismissButton = { TextButton(onClick = { clearing = false; deleting = null }) { Text("Keep") } })
}
