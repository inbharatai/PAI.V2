package com.unoone.agent

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.IntentFilter
import android.content.Intent
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.edit
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.navigation.compose.rememberNavController
import com.unoone.agent.accessibilitycontrol.UnoOneAccessibilityService
import com.unoone.agent.core.runtime.AgentRuntimeGate
import androidx.lifecycle.lifecycleScope
import com.unoone.agent.di.DatabaseProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import com.unoone.agent.ui.navigation.UnoOneNavHost
import com.unoone.agent.ui.theme.UnoOneTheme
import com.unoone.agent.ui.viewmodel.AgentViewModel
import com.unoone.agent.ui.viewmodel.AuditViewerViewModel
import com.unoone.agent.ui.viewmodel.LanguagePacksViewModel
import com.unoone.agent.ui.viewmodel.LogsViewModel
import com.unoone.agent.ui.viewmodel.ModelStatusViewModel
import com.unoone.agent.ui.viewmodel.NotesViewModel
import com.unoone.agent.ui.viewmodel.PrivacySettingsViewModel
import com.unoone.agent.ui.viewmodel.SecureBrowserViewModel
import com.unoone.agent.ui.viewmodel.SettingsViewModel
import com.unoone.agent.ui.viewmodel.SkillsViewModel
import com.unoone.agent.ui.viewmodel.VoiceTestViewModel
import com.unoone.agent.vault.PocketVaultAccess
import com.unoone.agent.vault.PocketVaultResult
import com.unoone.agent.vaultbridge.VaultConnection
import com.unoone.agent.vaultbridge.VaultMirror
import dagger.hilt.android.AndroidEntryPoint
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow

@AndroidEntryPoint
class MainActivity : ComponentActivity() {

    private lateinit var agentOrchestrator: AgentOrchestrator
    private val mutablePocketUsbStatus = MutableStateFlow<PocketUsbStatus>(PocketUsbStatus.Idle)
    private val pocketUsbStatus = mutablePocketUsbStatus.asStateFlow()

    private val mutableVaultUnlockUi = MutableStateFlow(VaultUnlockUi())
    private val vaultUnlockUi = mutableVaultUnlockUi.asStateFlow()

    /** Optional read-only legacy proposal. Never drains Room work or hydrates grants. */
    private fun requestVaultUnlock(password: String) {
        if (password.isEmpty() || mutableVaultUnlockUi.value.busy) return
        mutableVaultUnlockUi.value = VaultUnlockUi(busy = true)
        val bytes = password.toByteArray(Charsets.UTF_8)
        lifecycleScope.launch(Dispatchers.IO) {
            try {
                val (records, tombstones) = VaultConnection.inspectLegacy(bytes)
                mutableVaultUnlockUi.value = VaultUnlockUi(unlocked = true,
                    error = "Read-only review: $records authenticated readable records, $tombstones tombstones. No import performed. Backup and migration validation are still required.")
            } catch (_: Exception) {
                mutableVaultUnlockUi.value = VaultUnlockUi(error = "Legacy review failed or uses an unsupported record cipher. Original files were not modified; no import performed.")
            } finally { bytes.fill(0) }
        }
    }

    companion object {
        private const val PREFS_NAME = "unoone_permissions"
        private const val KEY_PROMPTED_VERSION = "permissions_prompted_version"
        private const val CURRENT_PERMISSIONS_VERSION = 2
        private const val ACTION_USB_PERMISSION = "com.unoone.agent.action.POCKET_USB_PERMISSION"
        private const val PROTOTYPE_VENDOR_ID = 0x0781
        private const val PROTOTYPE_PRODUCT_ID = 0x55A9
    }

    private val pocketTreePicker = registerForActivityResult(
        ActivityResultContracts.OpenDocumentTree()
    ) { uri ->
        if (uri == null) {
            mutablePocketUsbStatus.value = PocketUsbStatus.AccessRequired(
                "Legacy review cancelled. Local phone storage is unchanged; no drive is required."
            )
            return@registerForActivityResult
        }
        try {
            // Legacy drive is only an explicit read-only source, never live authority.
            contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (_: SecurityException) {
            // Some providers only offer session access; no write grant is requested.
        }
        mutablePocketUsbStatus.value = PocketUsbStatus.Validating
        mutablePocketUsbStatus.value = when (val result = PocketVaultAccess.validateTree(this, uri)) {
            is PocketVaultResult.Valid -> {
                // Separate read-only legacy source. Does not replace the local session.
                VaultConnection.attach(this, uri)
                mutableVaultUnlockUi.value = VaultUnlockUi() // fresh tree, locked
                PocketUsbStatus.Connected(result.vaultId, result.paiVersion)
            }
            is PocketVaultResult.Invalid -> PocketUsbStatus.Invalid(result.reason)
        }
    }

    private val usbPermissionReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action != ACTION_USB_PERMISSION) return
            val device = intent.usbDevice()
            val granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
            if (device == null || !granted) {
                mutablePocketUsbStatus.value = PocketUsbStatus.AccessRequired(
                    "Android USB permission was denied. Reconnect Pocket AI and grant access."
                )
                return
            }
            openPocketTreePicker()
        }
    }

    private val requestPermissionLauncher = registerForActivityResult(
        ActivityResultContracts.RequestMultiplePermissions()
    ) { permissions ->
        val allGranted = permissions.all { it.value }
        if (allGranted) {
            agentOrchestrator.clearPendingAndReExecute()
            markPermissionsPrompted()
        } else {
            val permanentlyDenied = PermissionManager.getPermanentlyDeniedPermissions(this)
            if (permanentlyDenied.isNotEmpty()) {
                Toast.makeText(
                    this,
                    "Some permissions were permanently denied. Please enable them in Settings.",
                    Toast.LENGTH_LONG
                ).show()
                startActivity(PermissionManager.getAppSettingsIntent(this))
            } else {
                Toast.makeText(this, "Expert features require the requested permissions.", Toast.LENGTH_LONG).show()
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val recovery = (application as UnoOneApplication).databaseRecoveryMessage
        if (recovery != null) {
            setContent {
                UnoOneTheme {
                    Surface(modifier = Modifier.fillMaxSize()) {
                        Column(modifier = Modifier.padding(24.dp)) {
                            Text("Local data recovery required", style = MaterialTheme.typography.headlineSmall)
                            Text(recovery)
                            Text("Do not uninstall, clear storage, replace keys, or downgrade. Keep this installation and its database sidecars. Supported standalone databases can be encrypted through the approval dialog while retaining recovery copies. Ambiguous or damaged files need assisted recovery. After a successful upgrade, force stop and reopen the app.")
                            Button(onClick = { finish() }) { Text("Close safely") }
                        }
                    }
                }
            }
            return
        }

        enableEdgeToEdge()

        val app = application as UnoOneApplication
        agentOrchestrator = app.orchestrator
        val database = DatabaseProvider.getDatabase(this)
        val voiceModule = app.sharedVoiceModule

        agentOrchestrator.onPermissionRequired = { missing ->
            requestPermissionLauncher.launch(missing.toTypedArray())
        }

        // System permissions (Accessibility / MediaProjection / Overlay) cannot be granted via the
        // runtime-permission dialog — they each need their own settings/consent screen. Surface the
        // first still-missing one as a one-tap deep-link, stash the command in the orchestrator
        // (it sets pendingCommand itself before invoking this callback), and resume it on return.
        agentOrchestrator.onSystemPermissionRequired = { missing ->
            val intent = missing.firstNotNullOfOrNull { req ->
                PermissionManager.getRequirementIntent(this, req)
            }
            if (intent != null) {
                try {
                    intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    startActivity(intent)
                    Toast.makeText(
                        this,
                        "Grant the system access, then return — your command resumes.",
                        Toast.LENGTH_LONG
                    ).show()
                } catch (_: Exception) {
                    Toast.makeText(this, "Unable to open system settings.", Toast.LENGTH_LONG).show()
                }
            } else {
                Toast.makeText(this, "System access is required for that action.", Toast.LENGTH_LONG).show()
            }
        }

        // Keep the AgentViewModel across configuration changes. Constructing it manually here
        // recreated disable collectors and transient state on every Activity recreation.
        val agentViewModel = ViewModelProvider(
            this,
            object : ViewModelProvider.Factory {
                @Suppress("UNCHECKED_CAST")
                override fun <T : ViewModel> create(modelClass: Class<T>): T {
                    if (modelClass.isAssignableFrom(AgentViewModel::class.java)) {
                        return AgentViewModel(agentOrchestrator, voiceModule, app) as T
                    }
                    throw IllegalArgumentException("Unknown ViewModel class: ${modelClass.name}")
                }
            }
        )[AgentViewModel::class.java]
        // Single app-scoped mirror (constructed in UnoOneApplication before
        // the orchestrator, so executor/memory writes canonicalise too).
        val vaultMirror = app.vaultMirror
        val notesViewModel = NotesViewModel(database.noteDao(), vaultMirror)
        val logsViewModel = LogsViewModel(database.actionLogDao())
        val skillsViewModel = SkillsViewModel(agentOrchestrator.skillsModule)
        val settingsViewModel = SettingsViewModel(this)
        val privacySettingsViewModel = PrivacySettingsViewModel(this)
        val modelStatusViewModel = ModelStatusViewModel(this, database.modelMetadataDao(), agentOrchestrator)
        val languagePacksViewModel = LanguagePacksViewModel(this)
        val voiceTestViewModel = VoiceTestViewModel(voiceModule)
        val auditViewerViewModel = AuditViewerViewModel(database.actionLogDao())
        val secureBrowserViewModel = SecureBrowserViewModel(
            this,
            app.secureBrowserModelLease,
            database.actionLogDao(),
            voiceModule
        )

        setContent {
            val usbStatus by pocketUsbStatus.collectAsState()
            val unlockUi by vaultUnlockUi.collectAsState()
            UnoOneTheme {
                Surface(
                    modifier = Modifier.fillMaxSize(),
                    color = MaterialTheme.colorScheme.background
                ) {
                    Column {
                        com.unoone.agent.vaultbridge.LocalVaultPanel(onLegacySelect = { pocketTreePicker.launch(null) })
                        Box(Modifier.weight(1f)) {
                            UnoOneApp(
                                agentViewModel = agentViewModel,
                                notesViewModel = notesViewModel,
                                logsViewModel = logsViewModel,
                                skillsViewModel = skillsViewModel,
                                settingsViewModel = settingsViewModel,
                                privacySettingsViewModel = privacySettingsViewModel,
                                modelStatusViewModel = modelStatusViewModel,
                                languagePacksViewModel = languagePacksViewModel,
                                voiceTestViewModel = voiceTestViewModel,
                                auditViewerViewModel = auditViewerViewModel,
                                secureBrowserViewModel = secureBrowserViewModel,
                                pocketUsbStatus = usbStatus,
                                vaultUnlock = unlockUi,
                                onVaultUnlock = ::requestVaultUnlock
                            )
                        }
                    }
                }
            }
        }

        ContextCompat.registerReceiver(
            this,
            usbPermissionReceiver,
            IntentFilter(ACTION_USB_PERMISSION),
            ContextCompat.RECEIVER_NOT_EXPORTED
        )
        val launchedForPocketUsb = handlePocketUsbIntent(intent)
        if (!launchedForPocketUsb) checkInitialExpertPermissionsIfNeeded()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if ((application as UnoOneApplication).databaseRecoveryMessage != null) return
        setIntent(intent)
        handlePocketUsbIntent(intent)
    }

    override fun onStop() {
        com.unoone.agent.vaultbridge.LocalVaultSetup.backgrounded()
        super.onStop()
    }

    override fun onDestroy() {
        runCatching { unregisterReceiver(usbPermissionReceiver) }
        super.onDestroy()
    }

    private fun handlePocketUsbIntent(intent: Intent?): Boolean {
        return when (intent?.action) {
            UsbManager.ACTION_USB_DEVICE_ATTACHED -> {
                val device = intent.usbDevice()
                if (device == null ||
                    device.vendorId != PROTOTYPE_VENDOR_ID ||
                    device.productId != PROTOTYPE_PRODUCT_ID
                ) {
                    mutablePocketUsbStatus.value = PocketUsbStatus.Invalid(
                        "An unsupported USB device was attached. Nothing was opened."
                    )
                    true
                } else {
                    mutablePocketUsbStatus.value = PocketUsbStatus.AccessRequired(
                        "Optional legacy drive detected. Use Local vault → Review a legacy drive for a read-only proposal."
                    )
                    true
                }
            }
            UsbManager.ACTION_USB_DEVICE_DETACHED -> {
                mutablePocketUsbStatus.value = PocketUsbStatus.Disconnected
                // Discard only the read-only legacy source; local vault remains independent.
                VaultConnection.detach()
                mutableVaultUnlockUi.value = VaultUnlockUi()
                // Detach only the optional legacy drive session. The independent local store,
                // including pending writes, tombstones and audit history, remains authoritative here.
                true
            }
            else -> false
        }
    }

    private fun requestPocketUsbAccess(device: UsbDevice) {
        val usbManager = getSystemService(USB_SERVICE) as UsbManager
        if (usbManager.hasPermission(device)) {
            openPocketTreePicker()
            return
        }
        mutablePocketUsbStatus.value = PocketUsbStatus.AccessRequired(
            "Pocket AI is connected. Allow Android USB access to validate it."
        )
        val permissionIntent = PendingIntent.getBroadcast(
            this,
            0,
            Intent(ACTION_USB_PERMISSION).setPackage(packageName),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE
        )
        usbManager.requestPermission(device, permissionIntent)
    }

    private fun openPocketTreePicker() {
        mutablePocketUsbStatus.value = PocketUsbStatus.AccessRequired(
            "Select the UNOONE drive. UnoOne reads only the selected Pocket AI tree."
        )
        pocketTreePicker.launch(null)
    }

    private fun checkInitialExpertPermissionsIfNeeded() {
        val prefs = getSharedPreferences(PREFS_NAME, MODE_PRIVATE)
        val promptedVersion = prefs.getInt(KEY_PROMPTED_VERSION, 0)
        if (promptedVersion < CURRENT_PERMISSIONS_VERSION) checkInitialExpertPermissions()
    }

    private fun markPermissionsPrompted() {
        getSharedPreferences(PREFS_NAME, MODE_PRIVATE)
            .edit()
            .putInt(KEY_PROMPTED_VERSION, CURRENT_PERMISSIONS_VERSION)
            .apply()
    }

    private fun checkInitialExpertPermissions() {
        val missing = PermissionManager.getMissingPermissions(this)
        if (missing.isNotEmpty()) requestPermissionLauncher.launch(missing.toTypedArray())

        if (!Settings.canDrawOverlays(this)) {
            startActivity(
                Intent(
                    Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                    Uri.parse("package:$packageName")
                )
            )
            Toast.makeText(this, "Enable 'Display over other apps' for the floating AI.", Toast.LENGTH_LONG).show()
        } else {
            startService(Intent(this, FloatingAgentService::class.java))
        }

        if (!UnoOneAccessibilityService.isEnabled()) {
            Toast.makeText(
                this,
                "Enable UnoOne Accessibility Service for native-app and external-browser automation.",
                Toast.LENGTH_LONG
            ).show()
        }

        requestBatteryOptimizationExemption()
        markPermissionsPrompted()
    }

    private fun requestBatteryOptimizationExemption() {
        val powerManager = getSystemService(POWER_SERVICE) as PowerManager
        if (!powerManager.isIgnoringBatteryOptimizations(packageName)) {
            try {
                startActivity(
                    Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                        data = Uri.parse("package:$packageName")
                    }
                )
            } catch (_: Exception) {
                Toast.makeText(
                    this,
                    "Please disable battery optimization for UnoOne in Settings.",
                    Toast.LENGTH_LONG
                ).show()
            }
        }

        PermissionManager.getAutostartIntent(this)?.let { intent ->
            try {
                startActivity(intent)
                Toast.makeText(this, "Please enable autostart for UnoOne.", Toast.LENGTH_LONG).show()
            } catch (_: Exception) {
                // Manufacturer-specific activity is unavailable.
            }
        }
    }

    override fun onResume() {
        super.onResume()
        if ((application as UnoOneApplication).databaseRecoveryMessage != null) return
        if (AgentRuntimeGate.isEnabled()) {
            runCatching { com.unoone.agent.voice.VoiceService.start(this) }
                .onFailure { com.unoone.agent.core.util.Logger.e("MainActivity: failed to resume voice activation", it) }
        }
        if (AgentRuntimeGate.isEnabled() && Settings.canDrawOverlays(this)) {
            startService(Intent(this, FloatingAgentService::class.java))
        }
        if (AgentRuntimeGate.isEnabled()) {
            (application as? UnoOneApplication)?.reloadLlmIfUnloaded()
        }
        // Resume a command that was paused on a missing system permission once the user returns
        // from the settings/consent screen. No-op when nothing is pending; the orchestrator re-checks
        // access and re-surfaces only whatever is still missing.
        if (AgentRuntimeGate.isEnabled()) agentOrchestrator.clearPendingAndReExecute()
    }
}

@Composable
fun UnoOneApp(
    agentViewModel: AgentViewModel,
    notesViewModel: NotesViewModel,
    logsViewModel: LogsViewModel,
    skillsViewModel: SkillsViewModel,
    settingsViewModel: SettingsViewModel,
    privacySettingsViewModel: PrivacySettingsViewModel,
    modelStatusViewModel: ModelStatusViewModel,
    languagePacksViewModel: LanguagePacksViewModel,
    voiceTestViewModel: VoiceTestViewModel,
    auditViewerViewModel: AuditViewerViewModel,
    secureBrowserViewModel: SecureBrowserViewModel,
    pocketUsbStatus: PocketUsbStatus,
    vaultUnlock: VaultUnlockUi = VaultUnlockUi(),
    onVaultUnlock: (String) -> Unit = {}
) {
    val navController = rememberNavController()

    // Eyes-free (WS4): bridge the `secure_browser_task` tool (fired by the orchestrator with an
    // already-approved origin) to the Secure Browser screen — navigate there and stash the pending
    // (origin, task) so the PageAgent run starts once the Gemma lease + runtime are ready. The live
    // executeTask + spoken page read are device-time gates; this wiring only requests the UI handoff.
    androidx.compose.runtime.LaunchedEffect(Unit) {
        agentViewModel.setSecureBrowserTaskHandler { origin, task ->
            secureBrowserViewModel.setPendingTask(origin, task)
            navController.navigate(com.unoone.agent.ui.navigation.Screen.SecureBrowser.route)
        }
    }
    androidx.compose.runtime.DisposableEffect(Unit) {
        onDispose { agentViewModel.setSecureBrowserTaskHandler(null) }
    }

    Box(modifier = Modifier.fillMaxSize()) {
        UnoOneNavHost(
            navController = navController,
            agentViewModel = agentViewModel,
            notesViewModel = notesViewModel,
            logsViewModel = logsViewModel,
            skillsViewModel = skillsViewModel,
            settingsViewModel = settingsViewModel,
            privacySettingsViewModel = privacySettingsViewModel,
            modelStatusViewModel = modelStatusViewModel,
            languagePacksViewModel = languagePacksViewModel,
            voiceTestViewModel = voiceTestViewModel,
            auditViewerViewModel = auditViewerViewModel,
            secureBrowserViewModel = secureBrowserViewModel
        )
        PocketUsbBanner(
            status = pocketUsbStatus,
            vaultUnlock = vaultUnlock,
            onVaultUnlock = onVaultUnlock,
            modifier = Modifier.align(Alignment.TopCenter)
        )
    }
}

@Composable
private fun PocketUsbBanner(
    status: PocketUsbStatus,
    vaultUnlock: VaultUnlockUi,
    onVaultUnlock: (String) -> Unit,
    modifier: Modifier = Modifier
) {
    val message = status.message ?: return
    val textColor = when (status) {
        is PocketUsbStatus.Invalid,
        PocketUsbStatus.Disconnected -> MaterialTheme.colorScheme.onErrorContainer
        else -> MaterialTheme.colorScheme.onSecondaryContainer
    }
    Surface(
        modifier = modifier
            .fillMaxWidth()
            .statusBarsPadding(),
        color = when (status) {
            is PocketUsbStatus.Connected -> MaterialTheme.colorScheme.primaryContainer
            is PocketUsbStatus.Invalid,
            PocketUsbStatus.Disconnected -> MaterialTheme.colorScheme.errorContainer
            else -> MaterialTheme.colorScheme.secondaryContainer
        },
        tonalElevation = 4.dp
    ) {
        Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 10.dp)) {
            Text(
                text = if (status is PocketUsbStatus.Connected && vaultUnlock.unlocked) {
                    "$message · read-only review finished"
                } else {
                    message
                },
                color = textColor,
                style = MaterialTheme.typography.bodyMedium
            )
            // Explicit legacy review only. Password never enables write-through/hydration.
            if (status is PocketUsbStatus.Connected && !vaultUnlock.unlocked) {
                var password by remember { mutableStateOf("") }
                val lifecycle = androidx.lifecycle.compose.LocalLifecycleOwner.current.lifecycle
                androidx.compose.runtime.DisposableEffect(lifecycle) {
                    val observer = androidx.lifecycle.LifecycleEventObserver { _, event ->
                        if (event == androidx.lifecycle.Lifecycle.Event.ON_STOP) password = ""
                    }
                    lifecycle.addObserver(observer)
                    onDispose { lifecycle.removeObserver(observer) }
                }
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.padding(top = 8.dp)
                ) {
                    OutlinedTextField(
                        value = password,
                        onValueChange = { password = it },
                        label = { Text("Vault password") },
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.Password),
                        singleLine = true,
                        enabled = !vaultUnlock.busy,
                        modifier = Modifier.weight(1f)
                    )
                    Button(
                        onClick = { onVaultUnlock(password); password = "" },
                        enabled = !vaultUnlock.busy && password.isNotEmpty(),
                        modifier = Modifier.padding(start = 8.dp)
                    ) {
                        Text(if (vaultUnlock.busy) "Reviewing…" else "Review only")
                    }
                }
                vaultUnlock.error?.let { error ->
                    Text(
                        text = error,
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(top = 4.dp)
                    )
                }
            }
            if (vaultUnlock.unlocked) vaultUnlock.error?.let { Text(it) }
        }
    }
}

/** UI state for the optional legacy review step. */
data class VaultUnlockUi(
    val unlocked: Boolean = false,
    val busy: Boolean = false,
    val error: String? = null
)

sealed interface PocketUsbStatus {
    val message: String?

    data object Idle : PocketUsbStatus {
        override val message: String? = null
    }

    data class AccessRequired(override val message: String) : PocketUsbStatus

    data object Validating : PocketUsbStatus {
        override val message = "Validating Pocket AI manifest and vault identity…"
    }

    data class Connected(val vaultId: String, val version: String) : PocketUsbStatus {
        override val message = "Legacy source selected (read-only) — $vaultId · $version"
    }

    data class Invalid(val reason: String) : PocketUsbStatus {
        override val message = "Pocket AI rejected: $reason"
    }

    data object Disconnected : PocketUsbStatus {
        override val message = "Pocket AI disconnected. USB access is no longer available."
    }
}

private fun Intent.usbDevice(): UsbDevice? =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice::class.java)
    } else {
        @Suppress("DEPRECATION")
        getParcelableExtra(UsbManager.EXTRA_DEVICE)
    }
