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
}

impl StartupCoordinator {
    pub fn from_process_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        Self {
            phase: Mutex::new(StartupPhase::Starting),
            supplied_root: Mutex::new(parse_vault_root(&args)),
            connected_root: Mutex::new(None),
            vault_id: Mutex::new(None),
            validation_failures: Mutex::new(Vec::new()),
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
                    | StartupPhase::CheckingAssets
                    | StartupPhase::WaitingForUnlock
                    | StartupPhase::Unlocking
                    | StartupPhase::ScanningHost
            ) {
                *current = phase;
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
        self.set_phase(StartupPhase::PaiConnected);
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
    pub fn is_asset_validation_complete(&self) -> bool {
        matches!(
            self.phase
                .lock()
                .map(|phase| *phase)
                .unwrap_or(StartupPhase::Error),
            StartupPhase::PaiConnected
                | StartupPhase::ScanningHost
                | StartupPhase::SelectingBackend
                | StartupPhase::StartingModel
                | StartupPhase::VerifyingModel
                | StartupPhase::Ready
                | StartupPhase::LimitedMode
        )
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
