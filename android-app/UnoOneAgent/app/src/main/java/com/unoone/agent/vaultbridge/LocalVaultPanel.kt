package com.unoone.agent.vaultbridge

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp

/** Password/phrase UI. Text is intentionally NOT rememberSaveable or retained by a ViewModel. */
@Composable
fun LocalVaultPanel(onLegacySelect: () -> Unit) {
    val context = LocalContext.current
    val state by LocalVaultSetup.state.collectAsState()
    var showing by remember { mutableStateOf(false) }
    var firstPrompt by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { LocalVaultSetup.refresh(context) }
    LaunchedEffect(state.checked) {
        if (state.checked && !state.exists && !firstPrompt) { showing = true; firstPrompt = true }
    }
    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.SpaceBetween) {
        Text(if (state.unlocked) "Phone file vault open" else "Phone file vault locked", Modifier.padding(top = 12.dp))
        TextButton(onClick = { showing = true }) { Text("Local vault") }
    }
    if (!showing) return
    var password by remember { mutableStateOf("") }
    var confirmation by remember { mutableStateOf("") }
    var acknowledged by remember { mutableStateOf(false) }
    var retain by remember { mutableStateOf(LocalVaultSetup.retainWhileBackgrounded) }
    val lifecycle = androidx.lifecycle.compose.LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = androidx.lifecycle.LifecycleEventObserver { _, event ->
            if (event == androidx.lifecycle.Lifecycle.Event.ON_STOP) { password = ""; confirmation = "" }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    AlertDialog(
        onDismissRequest = { password = ""; confirmation = ""; showing = false },
        title = { Text(if (state.exists) "This phone's encrypted file vault" else "Create your phone vault") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                Text("No USB drive or Power computer is required. This phone creates its own vault ID and key. Your existing encrypted local database remains available independently.")
                Spacer(Modifier.height(8.dp))
                if (!state.exists) {
                    Text("Choose a strong password or a long phrase (at least 12 characters). This uses the existing vault encryption format. No recovery words or password reset are available yet. Keep your password safely; clearing app data loses this vault.")
                }
                if (!state.unlocked) {
                    OutlinedTextField(password, { if (it.length <= 1024) password = it }, label = { Text("Password or phrase") },
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.Password),
                        singleLine = true, enabled = !state.busy)
                    if (!state.exists) {
                        OutlinedTextField(confirmation, { if (it.length <= 1024) confirmation = it }, label = { Text("Repeat password or phrase") },
                            visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.Password),
                        singleLine = true, enabled = !state.busy)
                        Row {
                            Checkbox(acknowledged, { acknowledged = it }, enabled = !state.busy)
                            Text("I understand there is no password recovery yet.")
                        }
                    }
                }
                Row {
                    Checkbox(retain, { retain = it; LocalVaultSetup.retainInBackground(it) })
                    Text("Keep the file vault unlocked in the background for this app session (optional).")
                }
                Text("By default the file key is cleared when this screen's activity leaves the foreground. Locking this file vault does not erase or lock the separate encrypted Room database. Automatic deletion/retention expiry is disabled.")
                state.message?.let { Text(it, Modifier.padding(top = 8.dp)) }
                TextButton(onClick = { password = ""; confirmation = ""; showing = false; onLegacySelect() }, enabled = !state.busy) {
                    Text("Review a legacy drive (read-only proposal)")
                }
                Text("Legacy review does not import data, copy keys, activate skills or grant permissions. Backup and verified migration remain required before import.")
            }
        },
        confirmButton = {
            if (state.unlocked) {
                TextButton(onClick = { LocalVaultSetup.lock(); password = ""; confirmation = "" }) { Text("Lock file vault") }
            } else {
                val canOpen = state.checked && !state.busy && password.isNotEmpty() &&
                    (state.exists || (password.length >= 12 && password == confirmation && acknowledged))
                TextButton(enabled = canOpen, onClick = {
                    val bytes = password.toByteArray(Charsets.UTF_8)
                    password = ""; confirmation = ""
                    LocalVaultSetup.open(context, bytes, create = !state.exists)
                }) { Text(if (state.busy) "Working…" else if (state.exists) "Unlock" else "Create local vault") }
            }
        },
        dismissButton = { TextButton(onClick = { password = ""; confirmation = ""; showing = false }) { Text("Continue in app") } }
    )
}
