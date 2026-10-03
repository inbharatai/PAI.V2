// UnoOne Power — Private AI Desktop Workstation
// Tauri backend: USB vault detection, hardware profiling, model management, safety guard, agent loop

mod accessibility;
mod agent;
// The InBharat Audio adapter: the production ASR/TTS route (Qwen3-ASR /
// OmniVoice via the audio.cpp CLI subprocess), gated fail-closed behind a
// hash-before-spawn acceptance gate (CLIs and models are SHA-256-verified
// against the acceptance attestation BEFORE any process spawns). Reached
// through the SpeechRouter's InbharatAudioThenLegacy policy in `speech` /
// `recording` — the legacy Whisper/Piper plane is the explicit fallback, not
// the default.
mod bharat_audio;
// Boot waterfall tracing — see boot_trace.rs. Marks every long boot step so
// a slow launch is attributed to the exact blocking call, not guessed at.
mod boot_trace;
mod browser;
mod capability;
// Persistent conversation memory: every completed chat turn is written to
// the encrypted vault as a MESSAGE record, and a new session recalls the
// recent turns so the model has continuity across sessions. The vault is
// the single canonical store — the same records serve the agent's
// search plane and any vault-aware client on any host.
mod chat_memory;
mod document_migration;
mod documents;
// Pure-Rust document renderers behind doc.create (PDF via lopdf, DOCX via
// zip); every renderer round-trips through the real readers in `documents`.
mod doc_writer;
// P1-C: desktop producer of the shared ProcedureOutcome contract record —
// every completed harness agent run leaves honest, never-promotable
// telemetry in the canonical vault.
mod desktop_model_policy;
mod desktop_process;
mod env_learning;
mod gguf_meta;
mod granted_fs;
mod harness_bridge;
mod llama;
// Live website preview (web.preview): a bounded mirror of the agent's site
// under $TEMP (asset-protocol scoped), a frontend-created preview window,
// and a heartbeat poll that re-stages + reloads — no web server anywhere.
mod preview;
mod recording;
mod safety;
mod security;
// The product speech plane: SpeechBackend implementations (InBharat Audio +
// wrapped legacy Whisper/Piper) behind the explicit SpeechRouter policy
// (`product_router` = InbharatAudioThenLegacy). This IS the production
// speech path — recording's transient transcription and the voice commands
// route through it; no backend ever serves a language outside its declared
// provider coverage.
mod speech;
mod startup;
mod tool_contract_pin;
mod voice;

use base64::Engine;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use unoone_vault_core::{PrivacyLevel, Record, RecordType, Vault};

/// D7: Rich vault state that holds the live Vault object (with decrypted master key)
/// after unlock, instead of dropping it. The Vault's Drop impl zeros the master key
/// on drop, so locking sets Option to None (which drops the Vault, zeroing the key).
pub struct DesktopVaultState {
    /// The live Vault struct. None when locked, Some(Vault) when unlocked.
    /// Arc-wrapped so the unified Harness bridge can share the single canonical
    /// vault authority across a `spawn_blocking` boundary without copying it.
    /// There is intentionally no second memory store: Harness memory writes go
    /// through `PaiVaultMemoryProvider` against this same encrypted Vault.
    vault: Arc<Mutex<Option<Vault>>>,
    /// Fast metadata mirrors (for reads without locking the vault mutex).
    unlocked: Mutex<bool>,
    vault_id: Mutex<String>,
    vault_root: Mutex<String>,
}

impl DesktopVaultState {
    fn emergency_lock(&self) {
        if let Ok(mut vault) = self.vault.lock() {
            if let Some(open_vault) = vault.as_mut() {
                let _ = open_vault.lock();
            }
            *vault = None;
        }
        if let Ok(mut unlocked) = self.unlocked.lock() {
            *unlocked = false;
        }
        if let Ok(mut vault_id) = self.vault_id.lock() {
            vault_id.clear();
        }
        if let Ok(mut vault_root) = self.vault_root.lock() {
            vault_root.clear();
        }
    }
}

/// Defect #41 (live-caught 2026-09-15): Tauri requires webviews whose
/// `additionalBrowserArgs` differ to use different data directories. The
/// defect #21 fix set per-window `additionalBrowserArgs` in tauri.conf.json,
/// so the main window's WebView2 environment ran with
/// `--autoplay-policy=no-user-gesture-required` while every runtime-created
/// window (the browser-workspace the BrowserWorkspace UI and the agent's
/// browser.act lane open) resolved wry's default arguments — a conflicting
/// environment on the SAME user-data folder. Live-verified consequence: the
/// runtime-created window registered in Tauri but its WebView2 content never
/// started (no OS window, no CDP target, evals never answer) — every browser
/// session since 2026-09-14 bound to a dead shell.
///
/// The WebView2 loader appends `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` to
/// EVERY environment it creates in this process, so setting the autoplay
/// policy here (instead of per-window) gives all windows an identical
/// argument set: one shared browser process, and defect #21's autoplay
/// policy keeps working everywhere — including the browser workspace, which
/// the per-window override never covered. Must run before Tauri creates the
/// first webview. Any existing value (e.g. a live-test remote-debugging
/// flag) is preserved and extended, not replaced.
fn ensure_webview2_browser_args() {
    const AUTOPLAY: &str = "--autoplay-policy=no-user-gesture-required";
    let existing = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
    if existing.contains("autoplay-policy") {
        return;
    }
    let combined = if existing.trim().is_empty() {
        AUTOPLAY.to_owned()
    } else {
        format!("{existing} {AUTOPLAY}")
    };
    std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", combined);
}

fn main() {
    ensure_webview2_browser_args();
    let startup_state = startup::StartupCoordinator::from_process_args();
    let vault_state = DesktopVaultState {
        vault: Arc::new(Mutex::new(None)),
        unlocked: Mutex::new(false),
        vault_id: Mutex::new(String::new()),
        vault_root: Mutex::new(String::new()),
    };

    let recording_state = recording::RecordingStateHolder::new();
    let browser_state = std::sync::Arc::new(browser::BrowserStateHolder::new());

    // D5/D6: Safety guard held as Tauri managed state — persists across calls,
    // accumulates audit log, respects the current security level, and D6:
    // persists level changes to VAULT/config/security.json on disk.
    // On startup, if a vault root is detected, load the persisted security level.
    let initial_vault_root = String::new(); // Will be set after vault detection
    let safety_guard = if initial_vault_root.is_empty() {
        // No vault detected yet — use default Standard level.
        // Once a vault is detected and unlock_vault is called, the guard
        // is re-initialized with the vault root for persistence.
        safety::DesktopSafetyGuard::new(safety::SecurityLevel::Standard)
    } else {
        safety::DesktopSafetyGuard::new_with_vault_root(&initial_vault_root)
    };
    let safety_state = safety::SafetyGuardState {
        guard: Arc::new(Mutex::new(safety_guard)),
    };

    // D1: Model manager state for inference pipeline
    let model_state = llama::ModelManagerState::new();

    // D2: Agent loop state
    let agent_state = agent::AgentLoopState::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            let startup = app.state::<startup::StartupCoordinator>();
            startup.accept_process_args(&args);
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
            let _ = app.emit("pai-rescan-requested", ());
        }))
        .manage(startup_state)
        .manage(vault_state)
        .manage(recording_state)
        .manage(browser_state)
        .manage(safety_state)
        .manage(model_state)
        .manage(agent_state)
        // The live website preview's bounded mirror + reload session.
        .manage(preview::PreviewState::default())
        .manage(desktop_process::DesktopProcessState::default())
        // Stop control for in-flight harness runs, keyed by conversation.
        .manage(harness_bridge::HarnessRunRegistry::new())
        // In-chat folder-grant approval cards (see harness_bridge.rs).
        .manage(harness_bridge::PendingGrantRequests::default())
        .invoke_handler(tauri::generate_handler![
            // Vault commands
            detect_vault,
            startup::get_startup_status,
            startup::set_startup_limited,
            unlock_vault,
            setup_vault,
            #[cfg(feature = "dev-bypass")]
            dev_bypass_unlock,
            lock_vault,
            get_vault_status,
            // Hardware profile
            get_hardware_profile,
            // Model management
            llama::list_models,
            llama::select_desktop_model,
            llama::detect_acceleration,
            llama::get_model_config,
            llama::get_context_budget,
            llama::get_model_status,
            llama::start_model_server,
            llama::stop_model_server,
            llama::check_model_health,
            llama::model_cache_status,
            llama::stage_model_cache,
            // Filesystem helpers
            check_file_exists,
            // Safety guard
            safety::get_security_level,
            safety::set_security_level,
            // Recording
            recording::start_recording,
            recording::pause_recording,
            recording::resume_recording,
            recording::stop_recording,
            recording::add_bookmark,
            // Browser workspace
            browser::browser_start_session,
            browser::browser_stop_session,
            browser::browser_session_status,
            browser::browser_execute,
            browser::get_browser_bridge_script,
            browser::browser_eval,
            // Capability profile
            capability::get_desktop_capability_profile,
            // Documents and memory
            documents::list_documents,
            documents::parse_attached_document,
            documents::search_memories,
            migrate_plaintext_documents_to_vault,
            // Accessibility
            accessibility::get_accessibility_status,
            accessibility::perform_ocr,
            accessibility::describe_image,
            accessibility::save_vision_snapshot,
            accessibility::capture_screen_snapshot,
            // Security hardening
            security::generate_manifest,
            security::verify_manifest,
            security::recover_from_crash,
            security::emergency_lock,
            // D2: Agent loop
            agent::agent_chat,
            // D7: Vault state commands
            vault_is_unlocked,
            vault_read_record,
            vault_write_record,
            save_chat_turn,
            recall_chat_memory,
            // Settings and configuration
            get_version,
            set_settings,
            get_settings,
            set_accessibility_status,
            get_accessibility_settings,
            get_vault_domain_counts,
            // D4: Voice module (Whisper.cpp STT + Piper TTS)
            voice::get_voice_status,
            voice::transcribe_audio,
            voice::synthesize_speech,
            voice::read_spoken_audio,
            // Unified Harness text plane (production path) + InBharat Audio
            // adapter. The legacy agent loop remains registered as an explicit
            // rollback path until acceptance parity is proven on device.
            harness_bridge::harness_chat,
            harness_bridge::harness_stop_run,
            get_execution_status,
            set_execution_permission,
            harness_bridge::get_workspace_root,
            // P7 (2026-10-01): user-granted agent workspace root (Settings UI).
            harness_bridge::get_agent_workspace_info,
            harness_bridge::set_agent_workspace_root,
            harness_bridge::add_agent_folder,
            harness_bridge::remove_agent_folder,
            harness_bridge::agent_pending_folder_grants,
            harness_bridge::agent_respond_folder_grant,
            // Live website preview (web.preview)
            preview::preview_poll,
            preview::preview_stop,
            preview::preview_focus,
            bharat_audio::get_bharat_audio_status,
        ])
        .setup(|app| {
            startup::start_mount_monitor(app.handle().clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building UnoOne Power")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                stop_desktop_work(app);
                app.state::<recording::RecordingStateHolder>()
                    .emergency_discard();
                app.state::<llama::ModelManagerState>()
                    .emergency_stop_blocking();
                app.state::<DesktopVaultState>().emergency_lock();
            }
        });
}

#[derive(serde::Serialize, serde::Deserialize)]
struct VaultInfo {
    detected: bool,
    vault_root: String,
    vault_id: String,
    startup_state: startup::StartupPhase,
    validation_failures: Vec<unoone_usb_manifest::ValidationFailure>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct VaultUnlockResult {
    success: bool,
    vault_id: String,
    error: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct VaultSetupResult {
    success: bool,
    vault_id: String,
    recovery_key: String,
    error: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct HardwareProfile {
    total_ram_gb: f64,
    available_ram_gb: f64,
    cpu_count: usize,
    cpu_speed_ghz: f64,
    gpu_name: String,
    gpu_vram_gb: f64,
    os_name: String,
    os_version: String,
    has_cuda: bool,
    has_metal: bool,
    has_vulkan: bool,
    usb_speed: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct VaultStatus {
    is_connected: bool,
    is_unlocked: bool,
    vault_id: String,
    profile_name: String,
    used_space_gb: f64,
    total_space_gb: f64,
}

/// Scan removable drives for a valid UnoOne vault.
/// Validates via manifest.json + VERSION + vault.id — not hardcoded drive letters.
fn scan_removable_drives() -> Vec<String> {
    let mut drives = Vec::new();

    if cfg!(target_os = "windows") {
        // Enumerate all logical drives and filter to removable ones
        // Use WMI to find removable drives, then check each for UNOONE
        if let Ok(output) = std::process::Command::new("powershell")
            .args([
                "-NoProfile", "-Command",
                "Get-CimInstance Win32_LogicalDisk | Where-Object { $_.DriveType -eq 2 } | Select-Object -ExpandProperty DeviceID",
            ])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let drive = line.trim().to_string();
                    if !drive.is_empty() && drive.len() == 2 && drive.ends_with(':') {
                        drives.push(format!("{}\\", drive));
                    }
                }
            }
        }

        // Fallback: check common drive letters if WMI fails
        if drives.is_empty() {
            for letter in "DEFGHIJKLMNOP".chars() {
                let path = format!("{}:\\", letter);
                if std::path::Path::new(&path).exists() {
                    // Check if it looks like a removable drive
                    let unoone_path = std::path::Path::new(&path).join("UNOONE");
                    if unoone_path.exists() {
                        drives.push(path);
                    }
                }
            }
        }
    } else if cfg!(target_os = "macos") {
        // macOS: scan /Volumes/ for UNOONE directory
        if let Ok(entries) = std::fs::read_dir("/Volumes") {
            for entry in entries.flatten() {
                let path = entry.path();
                let unoone_path = path.join("UNOONE");
                if unoone_path.exists() {
                    drives.push(path.to_string_lossy().to_string());
                }
            }
        }
    } else {
        // Linux: scan common mount points
        for base in &["/mnt", "/media"] {
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let unoone_path = path.join("UNOONE");
                    if unoone_path.exists() {
                        drives.push(path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    drives
}

/// Guards against stacking background DesktopLaunch validations: the sweep is
/// expensive (it hashes every package asset), so only one may run at a time.
static ASSET_VALIDATION_RUNNING: AtomicBool = AtomicBool::new(false);

/// Run the BootGate first (identity + runtime executables — seconds, not the
/// multi-GB asset sweep), release model boot via
/// `StartupCoordinator::boot_gate_passed`, then continue with the full
/// DesktopLaunch sweep and flip the strong gate
/// (`full_sweep_completed`). The launch path only validates package identity
/// (`ValidationScope::PackageIdentity`) so the unlock screen appears fast;
/// this thread is what actually verifies every runtime, model, and voice
/// asset — and the model server refuses to start off the DRIVE until the
/// full sweep completes (a digest-verified host-cache copy may start at the
/// BootGate — see `llama::start_model_server`).
fn start_background_asset_validation(app_handle: tauri::AppHandle, root: &std::path::Path) {
    if ASSET_VALIDATION_RUNNING.swap(true, Ordering::SeqCst) {
        boot_trace::mark("background sweep: skipped (already running)");
        return;
    }
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        let startup_state = app_handle.state::<startup::StartupCoordinator>();

        // Phase A — BootGate: verify identity and the binaries inference will
        // spawn before releasing model boot. Fail-closed on any failure.
        let boot_started = std::time::Instant::now();
        boot_trace::mark("boot gate: begin (identity + runtimes)");
        let boot_report = unoone_usb_manifest::validate_package(
            &root,
            unoone_usb_manifest::ValidationScope::BootGate,
        );
        match boot_report.package {
            Some(package) => {
                // Identity connected early so the UI short-circuits detection.
                startup_state.connect(&package);
                startup_state.set_phase(startup::StartupPhase::CheckingAssets);
                startup_state.boot_gate_passed();
                boot_trace::mark_detail(
                    "boot gate: PASSED",
                    &format!("elapsed={:.1}s", boot_started.elapsed().as_secs_f32()),
                );
            }
            None => {
                startup_state.reject(boot_report.failures.clone());
                boot_trace::mark("boot gate: FAILED — package rejected");
                ASSET_VALIDATION_RUNNING.store(false, Ordering::SeqCst);
                return;
            }
        }

        // Phase B — the full DesktopLaunch sweep (models, voice, speech)
        // continues in the background while the model boots from the
        // digest-verified host cache.
        let sweep_started = std::time::Instant::now();
        boot_trace::mark("background sweep: begin (DesktopLaunch)");
        let report = unoone_usb_manifest::validate_package(
            &root,
            unoone_usb_manifest::ValidationScope::DesktopLaunch,
        );
        boot_trace::mark_detail(
            "background sweep: end",
            &format!(
                "elapsed={:.1}s valid={} failures={}",
                sweep_started.elapsed().as_secs_f32(),
                report.package.is_some(),
                report.failures.len()
            ),
        );
        if let Some(package) = report.package {
            startup_state.connect(&package);
            startup_state.full_sweep_completed();

            // Phase C — warm the speech model host cache. The measured
            // per-synthesis cost was 24 s of pure USB model read (0.04 s
            // CPU); staging OmniVoice + Qwen3-ASR onto the host disk after
            // the sweep (never during it — no drive contention with the
            // 556-asset hash pass) turns later syntheses into SSD reads.
            // Non-fatal and never blocking: speech serves from the drive
            // path until the verified copy lands.
            let speech_root = root.clone();
            std::thread::spawn(move || {
                if let Err(error) =
                    bharat_audio::stage_speech_models_to_host_cache(&speech_root.to_string_lossy())
                {
                    boot_trace::mark_detail("speech cache: skipped", &error);
                }
            });
        } else {
            startup_state.reject(report.failures.clone());
        }
        ASSET_VALIDATION_RUNNING.store(false, Ordering::SeqCst);
    });
}

#[tauri::command]
#[allow(unexpected_cfgs)]
fn detect_vault(
    app_handle: tauri::AppHandle,
    startup_state: tauri::State<'_, startup::StartupCoordinator>,
) -> Result<VaultInfo, String> {
    // An already-connected, identity-validated Pocket AI is reported directly
    // without re-scanning drives (and without blocking on the background
    // asset sweep, whose progress the UI observes via get_startup_status).
    if let Some(status) = startup_state.connected_status() {
        boot_trace::mark("detect_vault: short-circuit (already connected)");
        return Ok(VaultInfo {
            detected: true,
            vault_root: status.vault_root.unwrap_or_default(),
            vault_id: status.vault_id.unwrap_or_default(),
            startup_state: status.phase,
            validation_failures: status.validation_failures,
        });
    }

    if !startup_state.is_validating_assets() {
        startup_state.set_phase(startup::StartupPhase::ValidatingPai);
    }

    if let Some(supplied_root) = startup_state.take_supplied_root() {
        // Identity-only here: fast launch. The background sweep below does the
        // full asset verification.
        startup_state.set_phase(startup::StartupPhase::CheckingAssets);
        let report = unoone_usb_manifest::validate_package(
            &supplied_root,
            unoone_usb_manifest::ValidationScope::PackageIdentity,
        );
        if let Some(package) = report.package {
            startup_state.connect(&package);
            startup_state.set_phase(startup::StartupPhase::CheckingAssets);
            start_background_asset_validation(app_handle, &package.root);
            return Ok(VaultInfo {
                detected: true,
                vault_root: package.root.to_string_lossy().to_string(),
                vault_id: package.vault_id,
                startup_state: startup::StartupPhase::CheckingAssets,
                validation_failures: Vec::new(),
            });
        }
        startup_state.reject(report.failures.clone());
        return Ok(VaultInfo {
            detected: false,
            vault_root: supplied_root.to_string_lossy().to_string(),
            vault_id: String::new(),
            startup_state: startup::StartupPhase::PaiInvalid,
            validation_failures: report.failures,
        });
    }

    // Scan removable drives for a valid UnoOne vault
    // using the same strict schema and hashes as Dock and Start UnoOne.
    boot_trace::mark("detect_vault: full path (not yet connected)");
    let drives = scan_removable_drives();
    boot_trace::mark_detail("detect_vault: drives scanned", &format!("{drives:?}"));

    for drive_root in drives {
        let Some(candidate) = startup::normalize_candidate_root(std::path::Path::new(&drive_root))
        else {
            continue;
        };
        // Identity-only here as well: fast launch, background full sweep.
        let report = unoone_usb_manifest::validate_package(
            &candidate,
            unoone_usb_manifest::ValidationScope::PackageIdentity,
        );
        if let Some(package) = report.package {
            startup_state.connect(&package);
            startup_state.set_phase(startup::StartupPhase::CheckingAssets);
            start_background_asset_validation(app_handle, &package.root);
            return Ok(VaultInfo {
                detected: true,
                vault_root: package.root.to_string_lossy().to_string(),
                vault_id: package.vault_id,
                startup_state: startup::StartupPhase::CheckingAssets,
                validation_failures: Vec::new(),
            });
        }
        if !report.failures.is_empty() {
            startup_state.reject(report.failures.clone());
            return Ok(VaultInfo {
                detected: false,
                vault_root: candidate.to_string_lossy().to_string(),
                vault_id: String::new(),
                startup_state: startup::StartupPhase::PaiInvalid,
                validation_failures: report.failures,
            });
        }
    }

    // NOTE: Production builds MUST NOT fall back to local/development paths.
    // The C:\UNOONE and /tmp/UNOONE fallbacks are gated behind a compile-time
    // feature flag "dev-local-vault" to prevent accidental use in production.
    // Only removable, validated USB volumes are accepted in production builds.
    #[cfg(feature = "dev-local-vault")]
    {
        let fallback_paths = if cfg!(target_os = "windows") {
            vec!["C:\\UNOONE"]
        } else if cfg!(target_os = "macos") {
            vec!["/tmp/UNOONE"]
        } else {
            vec!["/tmp/UNOONE"]
        };

        for path in fallback_paths {
            if let Ok((vault_root, vault_id)) = validate_vault_root(path) {
                return Ok(VaultInfo {
                    detected: true,
                    vault_root,
                    vault_id,
                    startup_state: startup::StartupPhase::PaiConnected,
                    validation_failures: Vec::new(),
                });
            }
        }
    }

    startup_state.set_phase(startup::StartupPhase::WaitingForPai);
    Ok(VaultInfo {
        detected: false,
        vault_root: String::new(),
        vault_id: String::new(),
        startup_state: startup::StartupPhase::WaitingForPai,
        validation_failures: Vec::new(),
    })
}

/// Check whether a file exists on disk.
/// Used by the frontend for best-effort existence checks without encoding the file.
#[tauri::command]
fn check_file_exists(path: String) -> Result<bool, String> {
    if path.is_empty() {
        return Ok(false);
    }
    Ok(PathBuf::from(path).exists())
}

#[tauri::command]
fn unlock_vault(
    password: String,
    vault_root: String,
    state: tauri::State<'_, DesktopVaultState>,
    startup_state: tauri::State<'_, startup::StartupCoordinator>,
    app: tauri::AppHandle,
) -> Result<VaultUnlockResult, String> {
    startup_state.set_phase(startup::StartupPhase::Unlocking);
    if password.is_empty() {
        startup_state.set_phase(startup::StartupPhase::WaitingForUnlock);
        return Ok(VaultUnlockResult {
            success: false,
            vault_id: String::new(),
            error: "Password cannot be empty".to_string(),
        });
    }

    if vault_root.is_empty() {
        startup_state.set_phase(startup::StartupPhase::WaitingForPai);
        return Ok(VaultUnlockResult {
            success: false,
            vault_id: String::new(),
            error: "No vault root specified".to_string(),
        });
    }

    // Use vault-core to unlock the vault with Argon2id key derivation
    // and XChaCha20-Poly1305 authenticated encryption.
    // D7: The Vault object is stored in Tauri managed state so it persists
    // after unlock — the decrypted master key remains in memory for vault operations.
    let vault_path = PathBuf::from(&vault_root);
    let mut vault = unoone_vault_core::Vault::open(&vault_path)
        .map_err(|e| format!("Failed to open vault: {}", e))?;

    match vault.unlock(password.as_bytes()) {
        Ok(result) => {
            // Store the live Vault in managed state (not dropped!)
            *state
                .vault
                .lock()
                .map_err(|e| format!("State lock error: {}", e))? = Some(vault);
            *state
                .unlocked
                .lock()
                .map_err(|e| format!("State lock error: {}", e))? = true;
            *state
                .vault_id
                .lock()
                .map_err(|e| format!("State lock error: {}", e))? = result.vault_id.clone();
            *state
                .vault_root
                .lock()
                .map_err(|e| format!("State lock error: {}", e))? = vault_root.clone();
            app.state::<llama::ModelManagerState>().resume();
            app.state::<preview::PreviewState>().resume();
            startup_state.set_phase(startup::StartupPhase::ScanningHost);

            // Security-baseline bootstrap: verify_vault (and the Settings
            // security check) verify against the stored baseline, but the
            // drive shipped without one and no UI surface ever calls
            // generate_manifest — the tool's "run generate_manifest first"
            // was a dead end the user could not follow. The first successful
            // unlock is the known-good state, so create the baseline then.
            // Best-effort only: a baseline failure must never block unlocking.
            let baseline_path = PathBuf::from(&vault_root)
                .join("VAULT")
                .join("config")
                .join("manifest.json");
            if !baseline_path.exists() {
                let _ = security::generate_manifest(vault_root.clone());
            }

            Ok(VaultUnlockResult {
                success: true,
                vault_id: result.vault_id,
                error: String::new(),
            })
            // Note: D6 — Security level persistence is handled by the safety guard.
            // When the vault is first detected, set_security_level or a guard
            // re-init with vault_root enables disk persistence. The frontend
            // should call set_security_level after vault detection to trigger
            // the initial persist if needed.
        }
        Err(unoone_vault_core::VaultError::WrongPassword) => {
            startup_state.set_phase(startup::StartupPhase::WaitingForUnlock);
            Ok(VaultUnlockResult {
                success: false,
                vault_id: String::new(),
                error: "Wrong password".to_string(),
            })
        }
        Err(e) => {
            startup_state.set_phase(startup::StartupPhase::Error);
            Ok(VaultUnlockResult {
                success: false,
                vault_id: String::new(),
                error: format!("Unlock failed: {}", e),
            })
        }
    }
}

/// PROTOTYPE ONLY — auto-setup/unlock bypass for developer testing.
/// Compiled ONLY when `unoone-power/dev-bypass` feature is on (never in
/// release/CI artifacts), and enabled ONLY when `UNOONE_DEV_BYPASS=1` is
/// present in the environment. Even then, it performs the real encrypted
/// vault setup/open — it just skips the interactive password prompt.
#[cfg(feature = "dev-bypass")]
#[tauri::command]
fn dev_bypass_unlock(
    state: tauri::State<'_, DesktopVaultState>,
    startup_state: tauri::State<'_, startup::StartupCoordinator>,
) -> Result<VaultUnlockResult, String> {
    // Gate 1: env must be set.
    if std::env::var("UNOONE_DEV_BYPASS").as_deref() != Ok("1") {
        return Ok(VaultUnlockResult {
            success: false,
            vault_id: String::new(),
            error: "UNOONE_DEV_BYPASS is not enabled".to_string(),
        });
    }

    // Gate 2: find the vault root the same way detect_vault does.
    let drives = scan_removable_drives();
    let mut found_root: Option<String> = None;
    for drive_root in drives {
        let Some(candidate) = startup::normalize_candidate_root(std::path::Path::new(&drive_root))
        else {
            continue;
        };
        let report = unoone_usb_manifest::validate_package(
            &candidate,
            unoone_usb_manifest::ValidationScope::DesktopLaunch,
        );
        if let Some(package) = report.package {
            startup_state.connect(&package);
            found_root = Some(package.root.to_string_lossy().to_string());
            break;
        }
    }

    let vault_root = match found_root {
        Some(r) => r,
        None => {
            return Ok(VaultUnlockResult {
                success: false,
                vault_id: String::new(),
                error: "No valid UnoOne Pocket AI found on any removable drive".to_string(),
            });
        }
    };

    let dev_password =
        std::env::var("UNOONE_DEV_PASSWORD").unwrap_or_else(|_| "dev-unlock-2026".to_string());

    // Gate 3: if no vault header exists yet, create one first (first-use).
    let vault_path = PathBuf::from(&vault_root);
    #[cfg(feature = "dev-bypass")]
    fn vault_has_header(vault_root: &std::path::Path) -> bool {
        vault_root
            .join("VAULT")
            .join("header")
            .join("header_a.json")
            .exists()
            || vault_root
                .join("VAULT")
                .join("header")
                .join("header_b.json")
                .exists()
    }
    if !vault_has_header(&vault_path) {
        let (_, package_vault_id) = validate_vault_root(&vault_root)?;
        match unoone_vault_core::Vault::create_with_vault_id(
            &vault_path,
            dev_password.as_bytes(),
            &package_vault_id,
        ) {
            Ok(_) => {
                startup_state.set_phase(startup::StartupPhase::ScanningHost);
            }
            Err(e) => {
                return Ok(VaultUnlockResult {
                    success: false,
                    vault_id: String::new(),
                    error: format!("Dev-mode vault creation failed: {}", e),
                });
            }
        }
    }

    // Gate 4: open + unlock with dev password.
    let mut vault = unoone_vault_core::Vault::open(&vault_path)
        .map_err(|e| format!("Failed to open vault: {}", e))?;
    match vault.unlock(dev_password.as_bytes()) {
        Ok(result) => {
            *state
                .vault
                .lock()
                .map_err(|e| format!("State lock error: {}", e))? = Some(vault);
            *state
                .unlocked
                .lock()
                .map_err(|e| format!("State lock error: {}", e))? = true;
            *state
                .vault_id
                .lock()
                .map_err(|e| format!("State lock error: {}", e))? = result.vault_id.clone();
            *state
                .vault_root
                .lock()
                .map_err(|e| format!("State lock error: {}", e))? = vault_root.clone();
            startup_state.set_phase(startup::StartupPhase::ScanningHost);
            Ok(VaultUnlockResult {
                success: true,
                vault_id: result.vault_id,
                error: String::new(),
            })
        }
        Err(e) => Ok(VaultUnlockResult {
            success: false,
            vault_id: String::new(),
            error: format!("Dev-mode unlock failed: {}", e),
        }),
    }
}

#[tauri::command]
fn setup_vault(
    password: String,
    profile_name: Option<String>,
    vault_root: String,
) -> Result<VaultSetupResult, String> {
    // Profile name is accepted for forward compatibility; the vault header
    // does not store it yet.
    let _ = &profile_name;
    if password.len() < 8 {
        return Ok(VaultSetupResult {
            success: false,
            vault_id: String::new(),
            recovery_key: String::new(),
            error: "Password must be at least 8 characters".to_string(),
        });
    }

    if vault_root.is_empty() {
        return Ok(VaultSetupResult {
            success: false,
            vault_id: String::new(),
            recovery_key: String::new(),
            error: "No vault root specified".to_string(),
        });
    }

    // Use vault-core to create a new vault with Argon2id key derivation
    // and XChaCha20-Poly1305 authenticated encryption
    let vault_path = PathBuf::from(&vault_root);

    match unoone_vault_core::Vault::create(&vault_path, password.as_bytes()) {
        Ok(result) => Ok(VaultSetupResult {
            success: true,
            vault_id: result.vault_id,
            recovery_key: result.recovery_phrase.join(" "),
            error: String::new(),
        }),
        Err(unoone_vault_core::VaultError::InvalidPassword(msg)) => Ok(VaultSetupResult {
            success: false,
            vault_id: String::new(),
            recovery_key: String::new(),
            error: msg,
        }),
        Err(e) => Ok(VaultSetupResult {
            success: false,
            vault_id: String::new(),
            recovery_key: String::new(),
            error: format!("Vault creation failed: {}", e),
        }),
    }
}

/// Close admission before terminating owned work. The vault is dropped first so
/// a concurrent chat cannot register a new run after the cancellation sweep.
fn stop_desktop_work(app: &tauri::AppHandle) {
    app.state::<DesktopVaultState>().emergency_lock();
    app.state::<llama::ModelManagerState>().suspend();
    app.state::<harness_bridge::HarnessRunRegistry>().stop_all();
    app.state::<harness_bridge::PendingGrantRequests>()
        .deny_all();
    app.state::<desktop_process::DesktopProcessState>()
        .set_enabled(false);
    preview::emergency_stop(app);
    if let Some(window) = app.get_webview_window("browser-workspace") {
        let _ = window.close();
    }
}

#[tauri::command]
async fn lock_vault(app: tauri::AppHandle) -> Result<(), String> {
    stop_desktop_work(&app);
    app.state::<recording::RecordingStateHolder>()
        .emergency_discard();
    app.state::<llama::ModelManagerState>()
        .emergency_stop()
        .await;
    Ok(())
}

#[tauri::command]
fn get_execution_status(
    state: tauri::State<'_, desktop_process::DesktopProcessState>,
) -> desktop_process::ExecutionStatus {
    state.status()
}

#[tauri::command]
fn set_execution_permission(
    enabled: bool,
    state: tauri::State<'_, desktop_process::DesktopProcessState>,
    vault: tauri::State<'_, DesktopVaultState>,
) -> Result<desktop_process::ExecutionStatus, String> {
    // Keep the vault guard through permission admission, ordering it against lock.
    let guard = vault.vault.lock().map_err(|_| "Vault lock failed")?;
    if enabled && guard.as_ref().is_none_or(|v| !v.is_unlocked()) {
        return Err("Unlock Pocket AI before enabling host commands".to_owned());
    }
    state.set_enabled(enabled);
    drop(guard);
    harness_bridge::audit_workspace_grant(
        &vault.vault,
        if enabled {
            "host_commands_enabled"
        } else {
            "host_commands_revoked"
        },
        "session-only; host filesystem and network access",
    );
    Ok(state.status())
}

/// D7: Check if the vault is currently unlocked (fast metadata read, no vault lock needed).
#[tauri::command]
fn vault_is_unlocked(state: tauri::State<'_, DesktopVaultState>) -> Result<bool, String> {
    Ok(*state
        .unlocked
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?)
}

/// D7: Read a record from the unlocked vault. Returns the decrypted content as a string.
#[tauri::command]
fn vault_read_record(
    record_id: String,
    state: tauri::State<'_, DesktopVaultState>,
) -> Result<String, String> {
    let vault_opt = state
        .vault
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let vault = vault_opt.as_ref().ok_or("Vault is not unlocked")?;

    let (_record, plaintext) = vault
        .read_record(&record_id)
        .map_err(|e| format!("Read failed: {}", e))?;

    String::from_utf8(plaintext).map_err(|e| format!("Record content is not valid UTF-8: {}", e))
}

/// Write an encrypted record to the vault.
/// Content is base64-encoded bytes — supports both text and binary (audio, images).
/// The vault-core write_record encrypts with XChaCha20-Poly1305 using
/// domain-derived keys before writing to disk. No plaintext is ever stored.
#[tauri::command]
fn vault_write_record(
    record_type: String,
    content_base64: String,
    privacy_level: Option<String>,
    parent_record_id: Option<String>,
    state: tauri::State<'_, DesktopVaultState>,
) -> Result<String, String> {
    let mut vault_opt = state
        .vault
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let vault = vault_opt.as_mut().ok_or("Vault is not unlocked")?;

    // Decode base64 content
    let content_bytes = base64::engine::general_purpose::STANDARD
        .decode(&content_base64)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;

    // Parse record type
    let rtype = match record_type.to_uppercase().as_str() {
        "CONVERSATION" => RecordType::Conversation,
        "MESSAGE" => RecordType::Message,
        "MEMORY" => RecordType::Memory,
        "PREFERENCE" => RecordType::Preference,
        "TASK" => RecordType::Task,
        "TASK_STEP" => RecordType::TaskStep,
        "TOOL_RESULT" => RecordType::ToolResult,
        "DOCUMENT" => RecordType::Document,
        "RECORDING" => RecordType::Recording,
        "TRANSCRIPT" => RecordType::Transcript,
        "TRANSCRIPT_SEGMENT" => RecordType::TranscriptSegment,
        "SUMMARY" => RecordType::Summary,
        "ACTION_ITEM" => RecordType::ActionItem,
        "BROWSER_RESEARCH" => RecordType::BrowserResearch,
        "CONTACT_REFERENCE" => RecordType::ContactReference,
        "CONTEXT_SNAPSHOT" => RecordType::ContextSnapshot,
        "AUDIT_RECORD" => RecordType::AuditRecord,
        other => return Err(format!("Unknown record type: {}", other)),
    };

    // Parse privacy level
    let plevel = match privacy_level
        .as_deref()
        .unwrap_or("PRIVATE")
        .to_uppercase()
        .as_str()
    {
        "PRIVATE" => PrivacyLevel::Private,
        "SUMMARY_ONLY" => PrivacyLevel::SummaryOnly,
        "METADATA_ONLY" => PrivacyLevel::MetadataOnly,
        other => return Err(format!("Unknown privacy level: {}", other)),
    };

    // Build the record — save ID before write_record takes ownership
    let device_id = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "desktop-unknown".to_string());

    let mut record = Record::new(rtype, "DESKTOP", &device_id);
    record.privacy_level = plevel;
    if let Some(parent_id) = parent_record_id {
        record.parent_record_id = Some(parent_id);
    }

    let record_id = record.record_id.clone();

    // Encrypt and write to vault — XChaCha20-Poly1305 with domain key
    vault
        .write_record(record, &content_bytes)
        .map_err(|e| format!("Write failed: {}", e))?;

    Ok(record_id)
}

/// Save one completed chat turn (user message + assistant reply) to the
/// encrypted vault as a MESSAGE record. Persistent conversation memory:
/// the next session recalls it via `recall_chat_memory`. Requires the
/// vault to be unlocked — no silent plaintext fallback.
#[tauri::command]
fn save_chat_turn(
    session_id: String,
    user_message: String,
    assistant_message: String,
    state: tauri::State<'_, DesktopVaultState>,
) -> Result<String, String> {
    let mut vault_opt = state
        .vault
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let vault = vault_opt.as_mut().ok_or("Vault is not unlocked")?;
    let turn = chat_memory::ChatTurn::new(&session_id, &user_message, &assistant_message);
    chat_memory::save_chat_turn_to_vault(vault, &turn)
}

/// Recall the most recent persisted chat turns, oldest first, so a new
/// session starts with conversation continuity. Requires an unlocked
/// vault (content is encrypted); returns [] when nothing is stored.
#[tauri::command]
fn recall_chat_memory(
    limit: Option<u32>,
    state: tauri::State<'_, DesktopVaultState>,
) -> Result<serde_json::Value, String> {
    let vault_opt = state
        .vault
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let vault = vault_opt.as_ref().ok_or("Vault is not unlocked")?;
    let vault_root = std::path::PathBuf::from(
        state
            .vault_root
            .lock()
            .map_err(|e| format!("State lock error: {}", e))?
            .clone(),
    );
    let turns = chat_memory::recall_chat_turns(&vault_root, vault, limit.unwrap_or(30) as usize);
    crate::boot_trace::mark_detail(
        "chat memory: recall",
        &format!("{} turn(s) restored", turns.len()),
    );
    Ok(serde_json::json!({
        "turns": turns,
    }))
}

#[tauri::command]
fn get_hardware_profile() -> Result<HardwareProfile, String> {
    let _hw_step = boot_trace::step("get_hardware_profile");
    let total_ram_bytes = sys_info::mem_info().map(|m| m.total * 1024).unwrap_or(0);
    let total_ram_gb = total_ram_bytes as f64 / (1024.0 * 1024.0 * 1024.0);

    let avail_ram_bytes = sys_info::mem_info().map(|m| m.avail * 1024).unwrap_or(0);
    let available_ram_gb = avail_ram_bytes as f64 / (1024.0 * 1024.0 * 1024.0);

    let cpu_count = num_cpus::get();

    let os_name = sys_info::os_type().unwrap_or_else(|_| "Unknown".to_string());
    let os_version = sys_info::os_release().unwrap_or_else(|_| "Unknown".to_string());
    boot_trace::mark("get_hardware_profile: mem+os probed");

    // Detect GPU via nvidia-smi
    let (gpu_name, gpu_vram_gb, has_cuda) = detect_gpu();
    boot_trace::mark_detail(
        "get_hardware_profile: gpu probed",
        &format!("cuda={has_cuda} name={gpu_name}"),
    );

    // Detect Vulkan via DLL/so presence
    let has_vulkan = if cfg!(target_os = "windows") {
        std::path::Path::new("C:\\Windows\\System32\\vulkan-1.dll").exists()
    } else if cfg!(target_os = "linux") {
        std::path::Path::new("/usr/lib/x86_64-linux-gnu/libvulkan.so").exists()
            || std::path::Path::new("/usr/lib/libvulkan.so").exists()
    } else {
        false
    };

    // Detect USB speed by checking the vault drive
    let usb_speed = detect_usb_speed();
    boot_trace::mark_detail("get_hardware_profile: usb probed", &usb_speed);

    let cpu_speed = detect_cpu_speed();
    boot_trace::mark_detail(
        "get_hardware_profile: cpu-speed probed",
        &format!("{cpu_speed}"),
    );

    Ok(HardwareProfile {
        total_ram_gb: (total_ram_gb * 10.0).round() / 10.0,
        available_ram_gb: (available_ram_gb * 10.0).round() / 10.0,
        cpu_count,
        cpu_speed_ghz: cpu_speed,
        gpu_name,
        gpu_vram_gb,
        os_name,
        os_version,
        has_cuda,
        has_metal: cfg!(target_os = "macos"),
        has_vulkan,
        usb_speed,
    })
}

fn detect_gpu() -> (String, f64, bool) {
    // Try nvidia-smi for NVIDIA GPU detection
    if let Ok(output) = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let line = stdout.lines().next().unwrap_or("");
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 2 {
                let name = parts[0].trim().to_string();
                let vram: f64 = parts[1].trim().parse().unwrap_or(0.0);
                return (name, vram, true);
            }
        }
    }

    (String::new(), 0.0, false)
}

/// Detect CPU speed in GHz using platform-specific methods
fn detect_cpu_speed() -> f64 {
    if cfg!(target_os = "windows") {
        // On Windows, use wmic to get max clock speed
        if let Ok(output) = std::process::Command::new("wmic")
            .args(["cpu", "get", "maxclockspeed", "/format:value"])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.starts_with("MaxClockSpeed=") {
                        if let Ok(mhz) = line
                            .trim_start_matches("MaxClockSpeed=")
                            .trim()
                            .parse::<f64>()
                        {
                            return (mhz / 1000.0 * 10.0).round() / 10.0; // Round to 1 decimal
                        }
                    }
                }
            }
        }
    } else if cfg!(target_os = "macos") {
        // On macOS, use sysctl
        if let Ok(output) = std::process::Command::new("sysctl")
            .args(["-n", "hw.cpufrequency"])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Ok(hz) = stdout.trim().parse::<f64>() {
                    return ((hz / 1_000_000_000.0) * 10.0).round() / 10.0;
                }
            }
        }
    } else {
        // On Linux, read from /proc/cpuinfo
        if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in content.lines() {
                if line.starts_with("cpu MHz") {
                    if let Some(mhz_str) = line.split(':').nth(1) {
                        if let Ok(mhz) = mhz_str.trim().parse::<f64>() {
                            return ((mhz / 1000.0) * 10.0).round() / 10.0;
                        }
                    }
                }
            }
        }
    }

    0.0 // Fallback — couldn't detect
}

fn detect_usb_speed() -> String {
    // On Windows, check USB drive speed via WMI
    if cfg!(target_os = "windows") {
        let wmi_started = std::time::Instant::now();
        if let Ok(output) = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_DiskDrive | Where-Object { $_.InterfaceType -eq 'USB' } | Select-Object -ExpandProperty MediaType"
            ])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let speed = stdout.trim();
                if !speed.is_empty() {
                    boot_trace::mark_detail(
                        "detect_usb_speed: wmi hit",
                        &format!(
                            "elapsed={:.1}s speed={speed}",
                            wmi_started.elapsed().as_secs_f32()
                        ),
                    );
                    return speed.to_string();
                }
            }
        }
        boot_trace::mark_detail(
            "detect_usb_speed: wmi miss → fallback sweep path",
            &format!("elapsed={:.1}s", wmi_started.elapsed().as_secs_f32()),
        );

        // Fallback: check if a UNOONE vault exists on any drive.
        // Structural check only (manifest.json + VERSION + vault.id): this
        // feeds a hardware-profile LABEL, not a security gate — the old code
        // called validate_vault_root here, re-running the FULL DesktopLaunch
        // hash sweep (a second pass over 11 GB of assets) just to print
        // "USB 3.0+". Live-measured 2026-10-02: that redundant sweep was
        // 7.6s warm / ~49s cold per launch, and Defender-crippled on a
        // freshly staged drive.
        let drives = scan_removable_drives();
        boot_trace::mark_detail(
            "detect_usb_speed: scan_removable_drives done",
            &format!("drives={drives:?}"),
        );
        for drive in &drives {
            if let Some(root) = startup::normalize_candidate_root(std::path::Path::new(drive)) {
                let structural = ["manifest.json", "VERSION", "VAULT\\identity\\vault.id"];
                if structural.iter().all(|rel| root.join(rel).is_file()) {
                    return "USB 3.0+".to_string();
                }
            }
        }
    }

    "Unknown".to_string()
}

#[tauri::command]
fn get_vault_status(state: tauri::State<'_, DesktopVaultState>) -> Result<VaultStatus, String> {
    let vault_root = state
        .vault_root
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let unlocked = *state
        .unlocked
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let vault_id = state
        .vault_id
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;

    if vault_root.is_empty() {
        return Ok(VaultStatus {
            is_connected: false,
            is_unlocked: false,
            vault_id: String::new(),
            profile_name: String::new(),
            used_space_gb: 0.0,
            total_space_gb: 0.0,
        });
    }

    // Calculate real disk usage
    let vault_path = std::path::Path::new(&*vault_root);
    let used_space_bytes = dir_size(vault_path).unwrap_or(0);
    let used_space_gb = used_space_bytes as f64 / (1024.0 * 1024.0 * 1024.0);

    // Get total disk space for the drive
    let total_space_gb = std::fs::metadata(vault_path)
        .ok()
        .and_then(|_m| {
            // Try to get filesystem stats
            std::fs::metadata(vault_path).ok()
        })
        .and_then(|_| {
            // Use sys_info for disk stats
            sys_info::disk_info().ok()
        })
        .map(|d| d.total as f64 / (1024.0 * 1024.0 * 1024.0))
        .unwrap_or(0.0);

    // Profile name: read from vault identity if unlocked, otherwise empty
    let profile_name = if unlocked {
        // Try to read profile name from the vault's identity directory
        let profile_path = std::path::PathBuf::from(&*vault_root)
            .join("VAULT")
            .join("identity")
            .join("profile.txt");
        std::fs::read_to_string(&profile_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };

    Ok(VaultStatus {
        is_connected: true,
        is_unlocked: unlocked,
        vault_id: vault_id.clone(),
        profile_name,
        used_space_gb: (used_space_gb * 10.0).round() / 10.0,
        total_space_gb: (total_space_gb * 10.0).round() / 10.0,
    })
}

/// Recursively compute directory size in bytes
fn dir_size(path: &std::path::Path) -> Result<u64, String> {
    let mut total: u64 = 0;
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    total += dir_size(&entry_path)?;
                } else if let Ok(metadata) = entry.metadata() {
                    total += metadata.len();
                }
            }
        }
    }
    Ok(total)
}

/// Get the app version from Cargo.toml
#[tauri::command]
fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Settings stored in VAULT/config/settings.json
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct AppSettings {
    #[serde(default = "default_security_level")]
    security_level: String,
    #[serde(default = "default_auto_lock_minutes")]
    auto_lock_minutes: u32,
    #[serde(default = "default_model_name")]
    model_name: String,
    #[serde(default = "default_temperature")]
    temperature: f32,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
    #[serde(default = "default_context_size")]
    context_size: u32,
    #[serde(default = "default_gpu_layers")]
    gpu_layers: u32,
}

fn default_security_level() -> String {
    "STANDARD".to_string()
}
fn default_auto_lock_minutes() -> u32 {
    5
}
fn default_model_name() -> String {
    "gemma-4-12b".to_string()
}
fn default_temperature() -> f32 {
    0.7
}
fn default_max_tokens() -> u32 {
    4096
}
fn default_context_size() -> u32 {
    8192
}
fn default_gpu_layers() -> u32 {
    0
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            security_level: default_security_level(),
            auto_lock_minutes: default_auto_lock_minutes(),
            model_name: default_model_name(),
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            context_size: default_context_size(),
            gpu_layers: default_gpu_layers(),
        }
    }
}

/// Persist app settings to VAULT/config/settings.json
#[tauri::command]
fn set_settings(settings: AppSettings, vault_root: String) -> Result<String, String> {
    let config_dir = std::path::PathBuf::from(&vault_root)
        .join("VAULT")
        .join("config");
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Failed to create config dir: {}", e))?;

    let config_path = config_dir.join("settings.json");
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(&config_path, json).map_err(|e| format!("Failed to write settings: {}", e))?;

    Ok("Settings saved".to_string())
}

/// Load app settings from VAULT/config/settings.json
#[tauri::command]
fn get_settings(vault_root: String) -> AppSettings {
    let config_path = std::path::PathBuf::from(&vault_root)
        .join("VAULT")
        .join("config")
        .join("settings.json");

    if config_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(settings) = serde_json::from_str::<AppSettings>(&content) {
                return settings;
            }
        }
    }

    AppSettings::default()
}

/// Accessibility settings stored in VAULT/config/accessibility.json
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct AccessibilitySettings {
    #[serde(default)]
    high_contrast: bool,
    #[serde(default)]
    reduced_motion: bool,
    #[serde(default = "default_font_scale")]
    font_scale: f32,
    #[serde(default = "default_stt_language")]
    stt_language: String,
    #[serde(default = "default_tts_language")]
    tts_language: String,
}

fn default_font_scale() -> f32 {
    1.0
}
fn default_stt_language() -> String {
    "en".to_string()
}
fn default_tts_language() -> String {
    "en".to_string()
}

impl Default for AccessibilitySettings {
    fn default() -> Self {
        AccessibilitySettings {
            high_contrast: false,
            reduced_motion: false,
            font_scale: 1.0,
            stt_language: "en".to_string(),
            tts_language: "en".to_string(),
        }
    }
}

/// Persist accessibility settings to VAULT/config/accessibility.json
#[tauri::command]
fn set_accessibility_status(
    settings: AccessibilitySettings,
    vault_root: String,
) -> Result<String, String> {
    let config_dir = std::path::PathBuf::from(&vault_root)
        .join("VAULT")
        .join("config");
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Failed to create config dir: {}", e))?;

    let config_path = config_dir.join("accessibility.json");
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("Failed to serialize accessibility settings: {}", e))?;
    std::fs::write(&config_path, json)
        .map_err(|e| format!("Failed to write accessibility settings: {}", e))?;

    Ok("Accessibility settings saved".to_string())
}

/// Load accessibility settings from VAULT/config/accessibility.json
#[tauri::command]
fn get_accessibility_settings(vault_root: String) -> AccessibilitySettings {
    let config_path = std::path::PathBuf::from(&vault_root)
        .join("VAULT")
        .join("config")
        .join("accessibility.json");

    if config_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(settings) = serde_json::from_str::<AccessibilitySettings>(&content) {
                return settings;
            }
        }
    }

    AccessibilitySettings::default()
}

/// Vault domain counts for the vault overview
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct VaultDomainCounts {
    memories: u32,
    chats: u32,
    recordings: u32,
    documents: u32,
    settings: u32,
    audit: u32,
}

/// Get per-domain item counts for the vault
#[tauri::command]
fn get_vault_domain_counts(vault_root: String) -> VaultDomainCounts {
    let vault_path = std::path::PathBuf::from(&vault_root).join("VAULT");

    fn count_files(dir: &std::path::Path) -> u32 {
        if !dir.exists() {
            return 0;
        }
        std::fs::read_dir(dir)
            .map(|entries| entries.count() as u32)
            .unwrap_or(0)
    }

    VaultDomainCounts {
        memories: count_files(&vault_path.join("memory")),
        chats: count_files(&vault_path.join("chats")),
        recordings: count_files(&vault_path.join("recordings")),
        documents: count_files(&vault_path.join("documents")),
        settings: count_files(&vault_path.join("config")),
        audit: count_files(&vault_path.join("audit")),
    }
}

/// Wave 3: migrate plaintext VAULT/documents + VAULT/memory into encrypted
/// records (verified read-back before any deletion; idempotent marker).
#[tauri::command]
fn migrate_plaintext_documents_to_vault(
    vault_state: tauri::State<'_, DesktopVaultState>,
) -> Result<document_migration::MigrationReport, String> {
    let mut vault_opt = vault_state
        .vault
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let vault = vault_opt
        .as_mut()
        .ok_or("Vault is locked — unlock before migrating plaintext.")?;
    let vault_root = vault.vault_root().to_path_buf();
    document_migration::migrate(vault, &vault_root)
}
