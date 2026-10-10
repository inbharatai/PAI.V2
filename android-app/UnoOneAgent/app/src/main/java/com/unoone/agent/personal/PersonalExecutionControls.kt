package com.unoone.agent.personal

import androidx.compose.foundation.layout.Column
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Same conversation, fixed local draft template. Reload/hydration never starts work. */
@Composable
fun PersonalExecutionControls(busy: Boolean, onReviewed: (ReviewedPersonalDraft?) -> Unit) {
    val context = LocalContext.current
    val service = remember { PersonalAgentService(context) }
    val scope = rememberCoroutineScope()
    var view by remember { mutableStateOf<PersonalView?>(null) }
    var error by remember { mutableStateOf("") }
    var selected by remember { mutableStateOf<String?>(null) }
    var reviewed by remember { mutableStateOf(false) }
    var children by remember { mutableStateOf(false) }
    var records by remember { mutableStateOf("") }
    var query by remember { mutableStateOf("") }
    var epoch by remember { mutableStateOf(0) }
    var vaultEpoch by remember { mutableStateOf(-1L) }
    val lifecycle = androidx.lifecycle.compose.LocalLifecycleOwner.current
    DisposableEffect(lifecycle) {
        val observer = androidx.lifecycle.LifecycleEventObserver { _, event ->
            if (event == androidx.lifecycle.Lifecycle.Event.ON_STOP) { epoch++; view = null; selected = null; reviewed = false; onReviewed(null) }
        }
        lifecycle.lifecycle.addObserver(observer)
        onDispose { lifecycle.lifecycle.removeObserver(observer) }
    }
    LaunchedEffect(Unit) {
        while (true) {
            if (!com.unoone.agent.vaultbridge.VaultConnection.isBridgeAllowed()) { epoch++; view = null; selected = null; reviewed = false; onReviewed(null) }
            kotlinx.coroutines.delay(200)
        }
    }
    Column {
        TextButton(enabled = !busy, onClick = {
            onReviewed(null); selected = null; reviewed = false
            val captured = epoch
            val session = com.unoone.agent.vaultbridge.VaultConnection.sessionEpoch()
            scope.launch { try { val loaded = withContext(Dispatchers.IO) { service.view() }; if (captured == epoch && session == com.unoone.agent.vaultbridge.VaultConnection.sessionEpoch()) { view = loaded; vaultEpoch = session; error = "" } } catch (_: Exception) { if (captured == epoch) { view = null; error = "Unlock and reload. Personal task data unavailable." } } }
        }) { Text("Reload accepted personal tasks") }
        if (error.isNotBlank()) Text(error)
        view?.let { v ->
            v.tasks.filter { it.status == "READY_FOR_REVIEW" && it.ownerReplicaId == v.replicaId && it.snoozeUntilMs == null }.forEach { task ->
                TextButton(enabled = !busy, onClick = { selected = task.spec.task_id; reviewed = false; onReviewed(null) }) { Text(task.spec.goal) }
            }
            v.tasks.find { it.spec.task_id == selected }?.let { task ->
                Text("Reviewed goal: ${task.spec.goal}. One local-model draft; 60 seconds, 4096 bytes, optional exact selected encrypted notes/documents, no file changes/network. Facts unverified. Next Go runs this goal, not additional text. No automatic retry.")
                Checkbox(checked=children, enabled=!busy, onCheckedChange={ children=it; reviewed=false; onReviewed(null) })
                Text("Two temporary specialists: selected-source summary + independent draft. Shared model-call budget and deadline; no tools/network, no inherited manual grants. Older one-call tasks need a new task budget.")
                OutlinedTextField(value = records, enabled = !busy, onValueChange = { records = it; reviewed = false; onReviewed(null) }, label = { Text("Optional exact record UUIDs (1–8, comma separated)") })
                OutlinedTextField(value = query, enabled = !busy, onValueChange = { query = it; reviewed = false; onReviewed(null) }, label = { Text("Search only those records") })
                Text("Source input: 8192 bytes maximum. Unselected records are never decrypted. Desktop filesystem/coding needs a paired capable host; Android has no isolated desktop coding backend.")
                Checkbox(checked = reviewed, enabled = !busy, onCheckedChange = { reviewed = it; onReviewed(if (it) ReviewedPersonalDraft(task.spec.task_id, v.revision, vaultEpoch, PersonalSource(records.split(",").map { id -> id.trim() }.filter { id -> id.isNotEmpty() }, query), children) else null) })
                Text("I reviewed this draft-only template; run it on my next Go")
            }
        }
    }
}
