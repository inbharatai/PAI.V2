// UnoOne Power — Desktop Capability Profile
// Provides a single, truthful status report for every major desktop feature.
// Status values are restricted to the P1 audit vocabulary:
// VERIFIED_WORKING, BUILDS_NOT_RUNTIME_TESTED, IMPLEMENTED_NOT_TESTED,
// PARTIALLY_IMPLEMENTED, NOT_IMPLEMENTED, BLOCKED_BY_ENVIRONMENT, FAILED.

use crate::llama::ModelManagerState;
use crate::voice::{discover_voice_assets, VoiceCapabilityStatus, VoiceModule};
use serde::{Deserialize, Serialize};

/// P1-audit-approved status vocabulary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FeatureStatus {
    VerifiedWorking,
    BuildsNotRuntimeTested,
    ImplementedNotTested,
    PartiallyImplemented,
    NotImplemented,
    BlockedByEnvironment,
    Failed,
}

impl std::fmt::Display for FeatureStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            FeatureStatus::VerifiedWorking => "VERIFIED_WORKING",
            FeatureStatus::BuildsNotRuntimeTested => "BUILDS_NOT_RUNTIME_TESTED",
            FeatureStatus::ImplementedNotTested => "IMPLEMENTED_NOT_TESTED",
            FeatureStatus::PartiallyImplemented => "PARTIALLY_IMPLEMENTED",
            FeatureStatus::NotImplemented => "NOT_IMPLEMENTED",
            FeatureStatus::BlockedByEnvironment => "BLOCKED_BY_ENVIRONMENT",
            FeatureStatus::Failed => "FAILED",
        };
        write!(f, "{}", s)
    }
}

/// Unified desktop capability profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopCapabilityProfile {
    pub vault: FeatureStatus,
    pub recording: FeatureStatus,
    pub browser: FeatureStatus,
    pub vision: FeatureStatus,
    pub voice: FeatureStatus,
    pub model: FeatureStatus,
    pub agent: FeatureStatus,
    pub documents: FeatureStatus,
    pub security: FeatureStatus,
    pub hardware: FeatureStatus,
    pub accessibility: FeatureStatus,
    pub usb: FeatureStatus,
    pub generated_at_utc: String,
    pub notes: Vec<String>,
}

/// Build a truthful capability profile for the current runtime.
#[tauri::command]
pub fn get_desktop_capability_profile(
    state: tauri::State<'_, crate::DesktopVaultState>,
    model_state: tauri::State<'_, ModelManagerState>,
) -> DesktopCapabilityProfile {
    let mut notes = Vec::new();

    // Vault: use the fast metadata mirror to know if we are unlocked.
    let vault_root_opt = state
        .vault_root
        .lock()
        .map(|s| if s.is_empty() { None } else { Some(s.clone()) })
        .unwrap_or(None);

    let vault = {
        let unlocked = *state
            .unlocked
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let connected = vault_root_opt.is_some();

        if unlocked {
            FeatureStatus::VerifiedWorking
        } else if connected {
            notes.push("Vault detected but not unlocked in this session.".to_string());
            FeatureStatus::ImplementedNotTested
        } else {
            notes.push("No UnoOne USB vault detected on removable drives.".to_string());
            FeatureStatus::BlockedByEnvironment
        }
    };

    // Recording: real cpal/hound pipeline is implemented and unit-tested, but
    // actual microphone capture has not been runtime-verified on this host.
    let recording = FeatureStatus::BuildsNotRuntimeTested;

    // Browser: WebView2 integration compiles and is correct; live browsing
    // depends on the WebView2 runtime and network.
    let browser = FeatureStatus::BuildsNotRuntimeTested;

    // Vision: backend OCR/describe commands exist and are wired, but the local
    // Gemma/mmproj model is not loaded in this build environment. Camera
    // preview uses standard Web APIs and has not been runtime-tested.
    let vision = FeatureStatus::PartiallyImplemented;

    // Voice: availability depends on Whisper.cpp/Piper binaries and models.
    // Use the same discovery logic as the Tauri voice commands so the profile
    // matches what the runtime will actually find on the USB vault.
    let voice = {
        let config = vault_root_opt
            .as_deref()
            .map(|root| discover_voice_assets(root, "en"))
            .unwrap_or_default();
        let module = VoiceModule::new(config);
        let stt = module.check_stt_availability();
        let tts = module.check_tts_availability();

        if stt == VoiceCapabilityStatus::Available && tts == VoiceCapabilityStatus::Available {
            notes.push(
                "Whisper.cpp and Piper binaries/models were discovered on the vault.".to_string(),
            );
            FeatureStatus::ImplementedNotTested
        } else {
            if stt != VoiceCapabilityStatus::Available {
                notes
                    .push("Whisper.cpp STT binary or model is missing from the vault.".to_string());
            }
            if tts != VoiceCapabilityStatus::Available {
                notes.push("Piper TTS binary or model is missing from the vault.".to_string());
            }
            FeatureStatus::BlockedByEnvironment
        }
    };

    // Model: commands are wired; actual inference depends on llama-server and
    // a downloaded model, neither of which is guaranteed here.
    let model = {
        let manager_set = model_state
            .manager
            .try_lock()
            .map(|m| m.is_some())
            .unwrap_or(false);
        if manager_set {
            FeatureStatus::ImplementedNotTested
        } else {
            notes.push("No llama-server/model manager is initialized.".to_string());
            FeatureStatus::ImplementedNotTested
        }
    };

    // Agent: chat command wired, not runtime tested.
    let agent = FeatureStatus::ImplementedNotTested;

    // Documents: list/process/search commands wired, not runtime tested.
    let documents = FeatureStatus::ImplementedNotTested;

    // Security: manifest/verify/recover commands wired, not runtime tested.
    let security = FeatureStatus::ImplementedNotTested;

    // Hardware: profile command compiles and returns real OS values, but
    // GPU/USB details depend on the host configuration.
    let hardware = FeatureStatus::BuildsNotRuntimeTested;

    // Accessibility: screen-reader detection works; vision toggles and voice
    // lab are wired, but model-backed inference has not been runtime-verified.
    let accessibility = FeatureStatus::PartiallyImplemented;

    // USB: vault detection scans removable drives correctly. If a vault root is
    // known, USB detection is at least build-correct; live transfer speed and
    // sustained I/O require a real device test.
    let usb = match vault_root_opt {
        Some(_) => FeatureStatus::BuildsNotRuntimeTested,
        None => FeatureStatus::BlockedByEnvironment,
    };

    DesktopCapabilityProfile {
        vault,
        recording,
        browser,
        vision,
        voice,
        model,
        agent,
        documents,
        security,
        hardware,
        accessibility,
        usb,
        generated_at_utc: chrono::Utc::now().to_rfc3339(),
        notes,
    }
}
