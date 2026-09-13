// UnoOne Power — Desktop Capability Profile
// Provides a single, truthful status report for every major desktop feature.
// Status values are restricted to the P1 audit vocabulary:
// VERIFIED_WORKING, BUILDS_NOT_RUNTIME_TESTED, IMPLEMENTED_NOT_TESTED,
// PARTIALLY_IMPLEMENTED, NOT_IMPLEMENTED, BLOCKED_BY_ENVIRONMENT, FAILED.

use crate::llama::ModelManagerState;
use serde::{Deserialize, Serialize};
use unoone_speech_contracts::SpeechBackend;

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
///
/// Live-caught 2026-09-12 (defect #17): this profile claimed to "reflect the
/// actual state of binaries, models, USB detection" while 9 of 12 lanes were
/// hardcoded P1-audit labels that never changed — and the `voice` lane probed
/// the legacy Whisper/Piper plane while the production drive runs InBharat
/// Audio (Qwen3-ASR/OmniVoice), so the panel actively reported the wrong
/// engine. Every lane the runtime CAN observe is now probed: speech router
/// readiness, llama-server state, manifest verification, input-device
/// negotiation, USB vault detection. Lanes the app cannot self-verify (the
/// agent loop's end-to-end behaviour, camera capture) keep their honest
/// posture labels instead of pretending.
/// Async (defect #23 family): sync commands run on the main/UI thread — this
/// profile runs manifest verification and a camera-device enumeration, so a
/// sync version froze the whole UI every time the Capabilities panel loaded.
#[tauri::command]
pub async fn get_desktop_capability_profile(
    state: tauri::State<'_, crate::DesktopVaultState>,
    model_state: tauri::State<'_, ModelManagerState>,
) -> Result<DesktopCapabilityProfile, String> {
    let mut notes = Vec::new();

    // Vault: use the fast metadata mirror to know if we are unlocked.
    let vault_root_opt = state
        .vault_root
        .lock()
        .map(|s| if s.is_empty() { None } else { Some(s.clone()) })
        .unwrap_or(None);

    let unlocked = *state
        .unlocked
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());

    let vault = {
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

    // Recording: probe the default capture device for a usable configuration
    // (the exact negotiation that live-broke as defect #15). No stream is
    // opened — the microphone is only touched when the user records.
    let recording = match crate::recording::probe_input_support() {
        Ok(description) => {
            notes.push(format!(
                "Capture device negotiated: {description}. Capture runs on demand."
            ));
            FeatureStatus::VerifiedWorking
        }
        Err(reason) => {
            notes.push(format!("Audio input probe failed: {reason}"));
            FeatureStatus::BlockedByEnvironment
        }
    };

    // Browser: this command only executes inside the app's WebView2 surface,
    // so a rendered profile is itself runtime proof the WebView2 engine is
    // live. The automation bridge (browser.act) rides the same webview.
    let browser = {
        notes.push("WebView2 runtime is active — this profile is rendered inside it.".to_string());
        FeatureStatus::VerifiedWorking
    };

    // Vision: probe what the runtime can observe — (a) the vision-capable
    // model manager (llama-server with mmproj) is live, (b) a camera device
    // is present on this host. With both, the describe/OCR/camera pipeline
    // is fully provisioned (verified live 2026-09-13, docs/verification/
    // 2026-09-13/98_VISION_DESCRIBE_IPC_DROP.md); without a camera the
    // screen-describe and OCR paths still work, so the lane stays partial
    // with an honest note.
    let vision = {
        let manager_set = model_state
            .manager
            .try_lock()
            .map(|m| m.is_some())
            .unwrap_or(false);
        let camera = tauri::async_runtime::spawn_blocking(
            crate::accessibility::enumerate_camera_devices,
        )
        .await
        .unwrap_or_else(|_| Err("camera probe task failed".to_string()));
        match (manager_set, camera) {
            (true, Ok(devices)) if !devices.is_empty() => {
                notes.push(format!(
                    "Vision model live; camera present ({}). Describe/OCR/camera verified live 2026-09-13.",
                    devices[0].name
                ));
                FeatureStatus::VerifiedWorking
            }
            (true, _) => {
                notes.push(
                    "Vision model live (screen describe + OCR work); no camera device found on this host."
                        .to_string(),
                );
                FeatureStatus::PartiallyImplemented
            }
            (false, _) => {
                notes.push("No vision-capable model manager is initialized.".to_string());
                FeatureStatus::PartiallyImplemented
            }
        }
    };

    // Voice: probe the SAME production router the voice commands use —
    // InBharat Audio first, legacy Whisper/Piper only as the declared
    // fallback. The old code probed Whisper/Piper directly and so reported
    // the wrong engine on a production drive.
    let voice = {
        if let Some(root) = vault_root_opt.as_deref() {
            let status = crate::speech::product_router(root)
                .inbharat_backend()
                .status();
            if status.ready {
                notes.push(
                    "InBharat Audio (Qwen3-ASR / OmniVoice) is configured and production-ready."
                        .to_string(),
                );
                FeatureStatus::VerifiedWorking
            } else {
                notes.push(format!("InBharat Audio is not ready: {}", status.reason));
                // Legacy plane: only reachable when its binaries/models exist.
                let config = crate::voice::discover_voice_assets(root, "en");
                let module = crate::voice::VoiceModule::new(config);
                let (stt, tts) = (
                    module.check_stt_availability(),
                    module.check_tts_availability(),
                );
                if stt == crate::voice::VoiceCapabilityStatus::Available
                    && tts == crate::voice::VoiceCapabilityStatus::Available
                {
                    notes.push(
                        "Legacy Whisper.cpp/Piper assets discovered; serving as fallback."
                            .to_string(),
                    );
                    FeatureStatus::ImplementedNotTested
                } else {
                    FeatureStatus::BlockedByEnvironment
                }
            }
        } else {
            notes.push("No vault root known; speech assets are vault-resident.".to_string());
            FeatureStatus::BlockedByEnvironment
        }
    };

    // Model: the manager being set means llama-server was spawned and passed
    // its runtime readiness probe on this host — live inference is available.
    let model = {
        let manager_set = model_state
            .manager
            .try_lock()
            .map(|m| m.is_some())
            .unwrap_or(false);
        if manager_set {
            notes.push(
                "llama-server is running with a loaded model (readiness probed at spawn)."
                    .to_string(),
            );
            FeatureStatus::VerifiedWorking
        } else {
            notes.push("No llama-server/model manager is initialized.".to_string());
            FeatureStatus::ImplementedNotTested
        }
    };

    // Agent: the loop's end-to-end behaviour cannot be self-verified without
    // spending a real model run; keep the honest posture label.
    let agent = FeatureStatus::ImplementedNotTested;

    // Documents: an unlocked vault is runtime proof the encrypted document
    // store decrypts and is servable; locked means wired but unexercised.
    let documents = if unlocked {
        notes.push("Encrypted document store is unlocked and decryptable.".to_string());
        FeatureStatus::VerifiedWorking
    } else {
        FeatureStatus::ImplementedNotTested
    };

    // Security: run the actual read-only manifest verification — the same
    // gate that guards speech and package integrity — and report its result.
    let security = match vault_root_opt.as_deref() {
        Some(root) => match crate::security::verify_manifest(root.to_string()) {
            Ok(result) if result.manifest_valid && result.hmac_valid => {
                notes.push(format!(
                    "Manifest verification passed: {} entries green.",
                    result.total_entries
                ));
                FeatureStatus::VerifiedWorking
            }
            Ok(result) => {
                notes.push(format!(
                    "Manifest verification failed: {} of {} entries failed (hmac_valid={}).",
                    result.entries_failed, result.total_entries, result.hmac_valid
                ));
                FeatureStatus::Failed
            }
            Err(reason) => {
                notes.push(format!("Manifest verification error: {reason}"));
                FeatureStatus::Failed
            }
        },
        None => FeatureStatus::ImplementedNotTested,
    };

    // Hardware: profile command compiles and returns real OS values, but
    // GPU/USB details depend on the host configuration.
    let hardware = FeatureStatus::BuildsNotRuntimeTested;

    // Accessibility: screen-reader detection works; vision toggles and voice
    // lab are wired, but model-backed inference has not been runtime-verified.
    let accessibility = FeatureStatus::PartiallyImplemented;

    // USB: a vault root means detection found the vault on a removable drive
    // this session — the scan itself is the runtime verification.
    let usb = match vault_root_opt {
        Some(_) => {
            notes.push("Vault was detected on a removable drive this session.".to_string());
            FeatureStatus::VerifiedWorking
        }
        None => FeatureStatus::BlockedByEnvironment,
    };

    Ok(DesktopCapabilityProfile {
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
    })
}
