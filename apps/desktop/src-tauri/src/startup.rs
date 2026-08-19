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
