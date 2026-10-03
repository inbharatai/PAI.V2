use crate::{llama, recording, DesktopVaultState};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use unoone_usb_manifest::{ValidatedPackage, ValidationFailure};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StartupPhase {
    Starting,
    WaitingForPai,
    ValidatingPai,
    PaiInvalid,
    PaiConnected,
    /// The BootGate passed: identity AND every required runtime executable
    /// (the binaries the model server spawns) are verified. The model may
    /// start NOW from the digest-verified host cache; the full DesktopLaunch
    /// sweep of models/voice/speech continues in the background and flips
    /// the phase to PAI_CONNECTED when it finishes.
    BootAssetsVerified,
    CheckingAssets,
    WaitingForUnlock,
    Unlocking,
    ScanningHost,
    SelectingBackend,
    StartingModel,
    VerifyingModel,
    Ready,
    LimitedMode,
    Disconnected,
    Error,
    ShuttingDown,
}

#[derive(Debug, Clone, Serialize)]
pub struct StartupStatus {
    pub phase: StartupPhase,
    pub vault_root: Option<String>,
    pub vault_id: Option<String>,
    pub validation_failures: Vec<ValidationFailure>,
}

pub struct StartupCoordinator {
    phase: Mutex<StartupPhase>,
    supplied_root: Mutex<Option<PathBuf>>,
    connected_root: Mutex<Option<PathBuf>>,
    vault_id: Mutex<Option<String>>,
    validation_failures: Mutex<Vec<ValidationFailure>>,
    /// BootGate (identity + runtime executables) passed. Authority for
    /// `is_boot_gate_complete`; the StartupPhase remains UI display state.
    boot_gate_complete: Mutex<bool>,
    /// Full DesktopLaunch sweep completed. Authority for
    /// `is_asset_validation_complete` (the model-server gate).
    asset_sweep_complete: Mutex<bool>,
}

impl StartupCoordinator {
    pub fn from_process_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        Self::with_supplied_root(parse_vault_root(&args))
    }

    pub fn with_supplied_root(supplied_root: Option<PathBuf>) -> Self {
        Self {
            phase: Mutex::new(StartupPhase::Starting),
            supplied_root: Mutex::new(supplied_root),
            connected_root: Mutex::new(None),
            vault_id: Mutex::new(None),
            validation_failures: Mutex::new(Vec::new()),
            boot_gate_complete: Mutex::new(false),
            asset_sweep_complete: Mutex::new(false),
        }
    }

    pub fn accept_process_args(&self, args: &[String]) {
        if let Some(root) = parse_vault_root(args) {
            if let Ok(mut supplied) = self.supplied_root.lock() {
                *supplied = Some(root);
            }
            self.set_phase(StartupPhase::ValidatingPai);
        }
    }

    pub fn take_supplied_root(&self) -> Option<PathBuf> {
        self.supplied_root.lock().ok()?.take()
    }

    pub fn set_phase(&self, phase: StartupPhase) {
        crate::boot_trace::mark(&format!("phase -> {phase:?}"));
        if let Ok(mut current) = self.phase.lock() {
            *current = phase;
        }
    }

    /// Variant for idempotent host probes (backend/hardware detection): the
    /// probe may announce its phase only while the boot sequence is still
    /// running. A later re-probe — e.g. the Model panel listing acceleration
    /// backends while the model is already serving — must never regress
    /// READY/LIMITED back to SELECTING BACKEND, which sticks the startup
    /// pill on a false "selecting backend" forever (no code path restores
    /// READY after boot: only check_model_health does, and it runs once).
    pub fn set_phase_if_booting(&self, phase: StartupPhase) {
        if let Ok(mut current) = self.phase.lock() {
            if matches!(
                *current,
                StartupPhase::Starting
                    | StartupPhase::WaitingForPai
                    | StartupPhase::ValidatingPai
                    | StartupPhase::PaiConnected
                    | StartupPhase::BootAssetsVerified
                    | StartupPhase::CheckingAssets
                    | StartupPhase::WaitingForUnlock
                    | StartupPhase::Unlocking
                    | StartupPhase::ScanningHost
            ) {
                *current = phase;
                crate::boot_trace::mark(&format!("phase(if_booting) -> {phase:?}"));
            }
        }
    }

    pub fn connect(&self, package: &ValidatedPackage) {
        if let Ok(mut root) = self.connected_root.lock() {
            *root = Some(package.root.clone());
        }
        if let Ok(mut vault_id) = self.vault_id.lock() {
            *vault_id = Some(package.vault_id.clone());
        }
        if let Ok(mut failures) = self.validation_failures.lock() {
            failures.clear();
        }
        // Non-regressing: when the background sweep finishes after the model
        // is already serving, the phase must stay at READY/STARTING_MODEL —
        // only the sweep-complete flag (set by full_sweep_completed) advances.
        self.set_phase_if_booting(StartupPhase::PaiConnected);
    }

    pub fn reject(&self, problems: Vec<ValidationFailure>) {
        if let Ok(mut failures) = self.validation_failures.lock() {
            *failures = problems;
        }
        self.set_phase(StartupPhase::PaiInvalid);
    }

    /// The current status of an already-connected (identity-validated) Pocket
    /// AI, if any. Returns None while disconnected/invalid/waiting so the
    /// caller falls through to the full detection path.
    pub fn connected_status(&self) -> Option<StartupStatus> {
        let root = self.connected_root.lock().ok()?.clone()?;
        if root.as_os_str().is_empty() {
            return None;
        }
        let phase = self
            .phase
            .lock()
            .map(|phase| *phase)
            .unwrap_or(StartupPhase::Error);
        if matches!(
            phase,
            StartupPhase::Disconnected | StartupPhase::PaiInvalid | StartupPhase::WaitingForPai
        ) {
            return None;
        }
        Some(StartupStatus {
            phase,
            vault_root: Some(root.display().to_string()),
            vault_id: self.vault_id.lock().ok().and_then(|value| value.clone()),
            validation_failures: self
                .validation_failures
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default(),
        })
    }

    /// True while the background DesktopLaunch asset sweep is still running.
    pub fn is_validating_assets(&self) -> bool {
        matches!(
            self.phase
                .lock()
                .map(|phase| *phase)
                .unwrap_or(StartupPhase::Error),
            StartupPhase::CheckingAssets
        )
    }

    /// True once the background DesktopLaunch asset sweep has finished
    /// (successfully or in limited mode). The model server refuses to start
    /// before this, so inference is never served on unverified assets.
    /// Flag-based, not phase-based: host probes legally advance the phase
    /// (SELECTING_BACKEND, STARTING_MODEL…) while the sweep is still running,
    /// so the phase alone must never be read as "the full sweep completed".
    pub fn is_asset_validation_complete(&self) -> bool {
        self.asset_sweep_complete
            .lock()
            .map(|flag| *flag)
            .unwrap_or(false)
    }

    /// The BootGate (identity + runtime executables) has passed. This is a
    /// strictly weaker guarantee than `is_asset_validation_complete` (the
    /// full DesktopLaunch sweep): the model server may start under it ONLY
    /// when the model is served from the digest-verified host cache — the
    /// model's own bytes are hash-verified by `start_server` at load time
    /// either way, so nothing is served on unverified bytes.
    pub fn is_boot_gate_complete(&self) -> bool {
        self.boot_gate_complete
            .lock()
            .map(|flag| *flag || self.is_asset_validation_complete())
            .unwrap_or(false)
    }

    /// Announce the BootGate result from the background validation thread.
    /// Fail-closed: a failed BootGate rejects the package outright — the
    /// model must never start against binaries that failed verification.
    pub fn boot_gate_passed(&self) {
        if let Ok(mut flag) = self.boot_gate_complete.lock() {
            *flag = true;
        }
        // Announce for the UI, without regressing a phase the boot chain may
        // already have advanced past (the gate flag above is the authority).
        self.set_phase_if_booting(StartupPhase::BootAssetsVerified);
    }

    /// Announce the full DesktopLaunch sweep result from the background
    /// validation thread. This is the strong gate: from here the model may
    /// also be served straight off the drive.
    pub fn full_sweep_completed(&self) {
        if let Ok(mut flag) = self.asset_sweep_complete.lock() {
            *flag = true;
        }
        self.set_phase_if_booting(StartupPhase::PaiConnected);
    }

    pub fn limited(&self) {
        self.set_phase(StartupPhase::LimitedMode);
    }

    fn connected_root(&self) -> Option<PathBuf> {
        self.connected_root.lock().ok()?.clone()
    }

    fn disconnect(&self) {
        if let Ok(mut root) = self.connected_root.lock() {
            *root = None;
        }
        if let Ok(mut vault_id) = self.vault_id.lock() {
            *vault_id = None;
        }
        // Gate flags reset with the connection: a replug re-runs the BootGate
        // and the full sweep before inference is allowed again.
        if let Ok(mut boot_gate) = self.boot_gate_complete.lock() {
            *boot_gate = false;
        }
        if let Ok(mut sweep) = self.asset_sweep_complete.lock() {
            *sweep = false;
        }
        self.set_phase(StartupPhase::Disconnected);
    }

    fn status(&self) -> StartupStatus {
        StartupStatus {
            phase: self
                .phase
                .lock()
                .map(|phase| *phase)
                .unwrap_or(StartupPhase::Error),
            vault_root: self
                .connected_root
                .lock()
                .ok()
                .and_then(|value| value.as_ref().map(|path| path.display().to_string())),
            vault_id: self.vault_id.lock().ok().and_then(|value| value.clone()),
            validation_failures: self
                .validation_failures
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default(),
        }
    }
}

#[tauri::command]
pub fn get_startup_status(state: tauri::State<'_, StartupCoordinator>) -> StartupStatus {
    state.status()
}

#[tauri::command]
pub fn set_startup_limited(state: tauri::State<'_, StartupCoordinator>) {
    state.limited();
}

pub fn normalize_candidate_root(path: &Path) -> Option<PathBuf> {
    if path.join("manifest.json").is_file() {
        return Some(path.to_path_buf());
    }
    let nested = path.join("UNOONE");
    nested.join("manifest.json").is_file().then_some(nested)
}

pub fn start_mount_monitor(app: AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(2));
        let state = app.state::<StartupCoordinator>();
        if let Some(root) = state.connected_root() {
            if !root.join("manifest.json").is_file() {
                state.disconnect();
                let _ = app.emit("pai-disconnected", root.display().to_string());
                let cleanup_app = app.clone();
                tauri::async_runtime::spawn(async move {
                    cleanup_after_removal(cleanup_app).await;
                });
            }
        }
    });
}

async fn cleanup_after_removal(app: AppHandle) {
    crate::stop_desktop_work(&app);
    app.state::<recording::RecordingStateHolder>()
        .emergency_discard();
    app.state::<llama::ModelManagerState>()
        .emergency_stop()
        .await;
    app.state::<DesktopVaultState>().emergency_lock();
}

fn parse_vault_root(args: &[String]) -> Option<PathBuf> {
    for (index, argument) in args.iter().enumerate() {
        if argument == "--vault-root" {
            if let Some(value) = args.get(index + 1) {
                return Some(PathBuf::from(value));
            }
        }
        if let Some(value) = argument.strip_prefix("--vault-root=") {
            return Some(PathBuf::from(value));
        }
    }
    None
}

#[cfg(test)]
mod set_phase_if_booting_tests {
    use super::*;

    fn coordinator_at(phase: StartupPhase) -> StartupCoordinator {
        StartupCoordinator {
            phase: Mutex::new(phase),
            supplied_root: Mutex::new(None),
            connected_root: Mutex::new(None),
            vault_id: Mutex::new(None),
            validation_failures: Mutex::new(Vec::new()),
            boot_gate_complete: Mutex::new(false),
            asset_sweep_complete: Mutex::new(false),
        }
    }

    fn phase_of(c: &StartupCoordinator) -> StartupPhase {
        c.phase.lock().map(|p| *p).unwrap_or(StartupPhase::Error)
    }

    // Live-caught regression (2026-10-01): opening the Model panel re-ran
    // detect_acceleration, whose set_phase knocked a READY boot all the way
    // back to SELECTING_BACKEND with no path to recover.
    #[test]
    fn probe_never_regresses_ready_or_limited() {
        for finished in [StartupPhase::Ready, StartupPhase::LimitedMode] {
            let c = coordinator_at(finished);
            c.set_phase_if_booting(StartupPhase::SelectingBackend);
            assert_eq!(phase_of(&c), finished);
        }
    }

    #[test]
    fn probe_announces_phase_only_while_booting() {
        // Early boot phases: the probe still drives the pill forward.
        for booting in [
            StartupPhase::Starting,
            StartupPhase::ScanningHost,
            StartupPhase::Unlocking,
        ] {
            let c = coordinator_at(booting);
            c.set_phase_if_booting(StartupPhase::SelectingBackend);
            assert_eq!(phase_of(&c), StartupPhase::SelectingBackend);
        }

        // Already past backend selection (server starting/verifying) or
        // disconnected: the probe stays silent so it cannot overwrite a
        // more advanced boot state.
        for advanced in [
            StartupPhase::StartingModel,
            StartupPhase::VerifyingModel,
            StartupPhase::Disconnected,
        ] {
            let c = coordinator_at(advanced);
            c.set_phase_if_booting(StartupPhase::SelectingBackend);
            assert_eq!(phase_of(&c), advanced);
        }
    }
}

#[cfg(test)]
mod boot_gate_tests {
    use super::*;

    #[test]
    fn gates_start_closed() {
        let c = StartupCoordinator::with_supplied_root(None);
        assert!(!c.is_boot_gate_complete());
        assert!(!c.is_asset_validation_complete());
    }

    #[test]
    fn boot_gate_opens_before_the_full_sweep() {
        let c = StartupCoordinator::with_supplied_root(None);
        c.boot_gate_passed();
        assert!(c.is_boot_gate_complete());
        // The weaker gate must never imply the full DesktopLaunch sweep.
        assert!(!c.is_asset_validation_complete());
        // The phase is announced for the UI without regressing probes.
        assert_eq!(phase_of(&c), StartupPhase::BootAssetsVerified);
    }

    #[test]
    fn full_sweep_satisfies_both_gates() {
        let c = StartupCoordinator::with_supplied_root(None);
        c.full_sweep_completed();
        assert!(c.is_asset_validation_complete());
        assert!(c.is_boot_gate_complete());
    }

    #[test]
    fn late_sweep_end_never_regresses_a_serving_phase() {
        // The BootGate flow: model boots (phase advances past PAI_CONNECTED)
        // while the background sweep is still hashing. When it finishes, the
        // phase must not clobber the serving state.
        let c = StartupCoordinator::with_supplied_root(None);
        c.boot_gate_passed();
        c.set_phase(StartupPhase::Ready);
        c.full_sweep_completed();
        assert_eq!(phase_of(&c), StartupPhase::Ready);
        assert!(c.is_asset_validation_complete());
    }

    #[test]
    fn host_probes_do_not_forge_the_full_sweep_gate() {
        // A probe legally advances the phase mid-sweep (e.g. SELECTING_BACKEND
        // from the Model panel); the phase must NOT read back as "full sweep
        // complete" — that inference was the old phase-based gate's hole.
        let c = StartupCoordinator::with_supplied_root(None);
        c.set_phase(StartupPhase::CheckingAssets);
        c.set_phase_if_booting(StartupPhase::SelectingBackend);
        assert!(!c.is_asset_validation_complete());
    }

    fn phase_of(c: &StartupCoordinator) -> StartupPhase {
        c.phase.lock().map(|p| *p).unwrap_or(StartupPhase::Error)
    }
}
