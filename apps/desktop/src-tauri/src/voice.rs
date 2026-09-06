// UnoOne Power — Desktop Voice Module
// D4: Interface layer for STT (Whisper.cpp) and TTS (Piper).
// Provides trait-based abstraction so the agent loop can request
// speech-to-text and text-to-speech without knowing the backend.
//
// STATUS: Fully wired. Whisper.cpp and Piper binaries are invoked
// via std::process::Command when found on PATH or in RUNTIMES directory.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Voice capability status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VoiceCapabilityStatus {
    Available,
    NotAvailable,
    Initializing,
    Error,
}

/// STT (speech-to-text) result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttResult {
    pub text: String,
    pub language: String,
    /// Whisper.cpp CLI does not expose calibrated utterance confidence.
    pub confidence: Option<f32>,
    pub processing_time_ms: u64,
    pub status: VoiceCapabilityStatus,
    /// Failure detail. Previously error strings were returned as `text`,
    /// i.e. "Whisper transcription failed: …" could be shown (and stored)
    /// as if it were the user's own words.
    pub error: Option<String>,
}

/// TTS (text-to-speech) result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsResult {
    pub audio_path: Option<String>,
    pub duration_seconds: Option<f32>,
    pub sample_rate: u32,
    pub status: VoiceCapabilityStatus,
    pub error: Option<String>,
    pub processing_time_ms: u64,
}

/// Voice engine type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VoiceEngine {
    WhisperCpp,
    Piper,
    SystemDefault,
}

/// Voice module configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    pub stt_engine: VoiceEngine,
    pub tts_engine: VoiceEngine,
    pub language: String,
    /// Path to the Whisper.cpp binary (USB binary preferred)
    pub whisper_bin_path: Option<String>,
    /// Path to the Whisper.cpp model (e.g., ggml-base.en.bin)
    pub whisper_model_path: Option<String>,
    /// Path to the Piper binary (USB binary preferred)
    pub piper_bin_path: Option<String>,
    /// Path to the Piper voice model (.onnx)
    pub piper_model_path: Option<String>,
    /// Path to the Piper voice config (.onnx.json)
    pub piper_config_path: Option<String>,
    /// Output directory for TTS audio files
    pub output_dir: Option<String>,
    /// Pocket AI package root (the drive root that owns manifest.json).
    /// When set, legacy binaries and models are SHA-256 verified against the
    /// package manifest before every run; when absent (dev hosts without a
    /// package manifest), verification fails closed only for untracked files.
    pub package_root: Option<String>,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            stt_engine: VoiceEngine::WhisperCpp,
            tts_engine: VoiceEngine::Piper,
            language: "en".to_string(),
            whisper_bin_path: None,
            whisper_model_path: None,
            piper_bin_path: None,
            piper_model_path: None,
            piper_config_path: None,
            output_dir: None,
            package_root: None,
        }
    }
}

/// D4: Voice module — orchestrates STT and TTS backends.
/// Invokes Whisper.cpp and Piper binaries when available.
pub struct VoiceModule {
    config: VoiceConfig,
}

impl VoiceModule {
    pub fn new(config: VoiceConfig) -> Self {
        Self { config }
    }

    /// Check whether Whisper.cpp STT is available on this system
    pub fn check_stt_availability(&self) -> VoiceCapabilityStatus {
        // Prefer a discovered USB binary path
        if let Some(bin) = &self.config.whisper_bin_path {
            if PathBuf::from(bin).exists() {
                if let Some(model_path) = &self.config.whisper_model_path {
                    if PathBuf::from(model_path).exists() {
                        return VoiceCapabilityStatus::Available;
                    }
                }
            }
        }

        // Fall back to RUNTIMES directory and system PATH
        if self.find_whisper_binary().is_some() {
            if let Some(model_path) = &self.config.whisper_model_path {
                if PathBuf::from(model_path).exists() {
                    return VoiceCapabilityStatus::Available;
                }
            }
        }

        VoiceCapabilityStatus::NotAvailable
    }

    /// Check whether Piper TTS is available on this system
    pub fn check_tts_availability(&self) -> VoiceCapabilityStatus {
        // Prefer a discovered USB binary path
        if let Some(bin) = &self.config.piper_bin_path {
            if PathBuf::from(bin).exists() {
                if let Some(model_path) = &self.config.piper_model_path {
                    if PathBuf::from(model_path).exists() {
                        return VoiceCapabilityStatus::Available;
                    }
                }
            }
        }

        // Fall back to RUNTIMES directory and system PATH
        if self.find_piper_binary().is_some() {
            if let Some(model_path) = &self.config.piper_model_path {
                if PathBuf::from(model_path).exists() {
                    return VoiceCapabilityStatus::Available;
                }
            }
        }

        VoiceCapabilityStatus::NotAvailable
    }

    /// Transcribe audio using Whisper.cpp (STT)
    /// Invokes the Whisper binary found during availability check.
    pub fn transcribe(&self, audio_path: &str) -> SttResult {
        let start = std::time::Instant::now();

        let status = self.check_stt_availability();

        if status != VoiceCapabilityStatus::Available {
            return SttResult {
                text: String::new(),
                language: self.config.language.clone(),
                confidence: None,
                processing_time_ms: start.elapsed().as_millis() as u64,
                status: VoiceCapabilityStatus::NotAvailable,
                error: Some("Whisper STT is not available (binary or model missing)".to_string()),
            };
        }

        // Find the Whisper binary path (RUNTIMES directory or system PATH)
        let whisper_bin = self.find_whisper_binary();
        let whisper_bin = match whisper_bin {
            Some(bin) => bin,
            None => {
                return SttResult {
                    text: String::new(),
                    language: self.config.language.clone(),
                    confidence: None,
                    processing_time_ms: start.elapsed().as_millis() as u64,
                    status: VoiceCapabilityStatus::Error,
                    error: Some("Whisper binary not found".to_string()),
                };
            }
        };

        // Create a temp directory for Whisper output
        let temp_dir = std::env::temp_dir().join("unoone-whisper");
        if let Err(e) = std::fs::create_dir_all(&temp_dir) {
            return SttResult {
                text: String::new(),
                language: self.config.language.clone(),
                confidence: None,
                processing_time_ms: start.elapsed().as_millis() as u64,
                status: VoiceCapabilityStatus::Error,
                error: Some(format!("Failed to create temp directory: {}", e)),
            };
        }

        let model_path = match &self.config.whisper_model_path {
            Some(path) => path.clone(),
            None => {
                return SttResult {
                    text: String::new(),
                    language: self.config.language.clone(),
                    confidence: None,
                    processing_time_ms: start.elapsed().as_millis() as u64,
                    status: VoiceCapabilityStatus::Error,
                    error: Some("No Whisper model path configured".to_string()),
                };
            }
        };

        // Fail closed on untracked or tampered legacy assets: binaries and
        // models must match the Pocket AI package manifest before use.
        if let Some(root) = &self.config.package_root {
            let root = std::path::Path::new(root);
            for asset in [&whisper_bin, &model_path] {
                if let Err(e) = verify_legacy_asset(root, std::path::Path::new(asset)) {
                    return SttResult {
                        text: String::new(),
                        language: self.config.language.clone(),
                        confidence: None,
                        processing_time_ms: start.elapsed().as_millis() as u64,
                        status: VoiceCapabilityStatus::Error,
                        error: Some(e),
                    };
                }
            }
        }

        // Unique per-call output prefix: two concurrent transcriptions used
        // to share `transcription.txt`, each reading (and deleting) the
        // other's transcript — a cross-request leak and a lost-result race.
        let call_id = uuid::Uuid::new_v4().simple().to_string();
        let output_prefix = temp_dir
            .join(format!("transcription-{call_id}"))
            .to_string_lossy()
            .to_string();
        // The session language is a canonical BCP-47 tag; the Whisper CLI
        // wants the base code (en, hi, …). `auto` passes through.
        let cli_language = legacy_cli_language(&self.config.language);

        let result = run_with_deadline(
            {
                let mut cmd = std::process::Command::new(&whisper_bin);
                cmd.args([
                    "--model",
                    &model_path,
                    "--language",
                    &cli_language,
                    "-otxt",
                    "-of",
                    &output_prefix,
                    audio_path,
                ]);
                cmd
            },
            LEGACY_INFERENCE_TIMEOUT,
        );

        match result {
            Ok(output) => {
                if output.status.success() {
                    // Read the transcription output file (Whisper appends .txt).
                    let output_file = temp_dir.join(format!("transcription-{call_id}.txt"));
                    let read = std::fs::read_to_string(&output_file);
                    // Always clean up, success or not — the transcript is
                    // transient user speech and must not linger in temp.
                    let _ = std::fs::remove_file(&output_file);
                    match read {
                        Ok(raw) if !raw.trim().is_empty() => SttResult {
                            text: raw.trim().to_string(),
                            language: self.config.language.clone(),
                            confidence: None,
                            processing_time_ms: start.elapsed().as_millis() as u64,
                            status: VoiceCapabilityStatus::Available,
                            error: None,
                        },
                        // A successful exit with a missing or empty output
                        // file is a failure, never a silent empty success.
                        Ok(_) => SttResult {
                            text: String::new(),
                            language: self.config.language.clone(),
                            confidence: None,
                            processing_time_ms: start.elapsed().as_millis() as u64,
                            status: VoiceCapabilityStatus::Error,
                            error: Some(
                                "Whisper produced no transcript text".to_string(),
                            ),
                        },
                        Err(e) => SttResult {
                            text: String::new(),
                            language: self.config.language.clone(),
                            confidence: None,
                            processing_time_ms: start.elapsed().as_millis() as u64,
                            status: VoiceCapabilityStatus::Error,
                            error: Some(format!("Whisper output unreadable: {}", e)),
                        },
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    SttResult {
                        text: String::new(),
                        language: self.config.language.clone(),
                        confidence: None,
                        processing_time_ms: start.elapsed().as_millis() as u64,
                        status: VoiceCapabilityStatus::Error,
                        error: Some(format!("Whisper transcription failed: {}", stderr.trim())),
                    }
                }
            }
            Err(e) => SttResult {
                text: String::new(),
                language: self.config.language.clone(),
                confidence: None,
                processing_time_ms: start.elapsed().as_millis() as u64,
                status: VoiceCapabilityStatus::Error,
                error: Some(format!("Failed to run Whisper: {}", e)),
            },
        }
    }

    /// Synthesize speech using Piper (TTS)
    /// Invokes the Piper binary found during availability check.
    pub fn synthesize(&self, text: &str) -> TtsResult {
        let start = std::time::Instant::now();

        let status = self.check_tts_availability();

        if status != VoiceCapabilityStatus::Available {
            return TtsResult {
                audio_path: None,
                duration_seconds: None,
                sample_rate: 22050,
                status: VoiceCapabilityStatus::NotAvailable,
                error: Some("TTS is not available. Piper binary not found.".to_string()),
                processing_time_ms: start.elapsed().as_millis() as u64,
            };
        }

        // Find the Piper binary path
        let piper_bin = self.find_piper_binary();
        let piper_bin = match piper_bin {
            Some(bin) => bin,
            None => {
                return TtsResult {
                    audio_path: None,
                    duration_seconds: None,
                    sample_rate: 22050,
                    status: VoiceCapabilityStatus::Error,
                    error: Some("Piper binary not found".to_string()),
                    processing_time_ms: start.elapsed().as_millis() as u64,
                };
            }
        };

        // Create output directory
        let output_dir = match &self.config.output_dir {
            Some(dir) => {
                let p = std::path::PathBuf::from(dir)
                    .join("VAULT")
                    .join("recordings");
                let _ = std::fs::create_dir_all(&p);
                p
            }
            None => {
                let p = std::env::temp_dir().join("unoone-piper");
                let _ = std::fs::create_dir_all(&p);
                p
            }
        };

        // Unique per-call output name: the old millisecond timestamp collided
        // for concurrent requests.
        let output_file = output_dir.join(format!(
            "tts_{}.wav",
            uuid::Uuid::new_v4().simple()
        ));

        let model_path = match &self.config.piper_model_path {
            Some(path) => path.clone(),
            None => {
                return TtsResult {
                    audio_path: None,
                    duration_seconds: None,
                    sample_rate: 22050,
                    status: VoiceCapabilityStatus::Error,
                    error: Some("No Piper model path configured".to_string()),
                    processing_time_ms: start.elapsed().as_millis() as u64,
                };
            }
        };

        let config_path = self.config.piper_config_path.clone().unwrap_or_default();

        // Fail closed on untracked or tampered legacy assets (see transcribe).
        if let Some(root) = &self.config.package_root {
            let root = std::path::Path::new(root);
            for asset in [&piper_bin, &model_path] {
                if let Err(e) = verify_legacy_asset(root, std::path::Path::new(asset)) {
                    return TtsResult {
                        audio_path: None,
                        duration_seconds: None,
                        sample_rate: 22050,
                        status: VoiceCapabilityStatus::Error,
                        error: Some(e),
                        processing_time_ms: start.elapsed().as_millis() as u64,
                    };
                }
            }
        }

        // Run: echo "text" | piper --model <model> [--config <config>] --output_file <file>
        let mut cmd = std::process::Command::new(&piper_bin);
        cmd.args(["--model", &model_path])
            .arg("--output_file")
            .arg(&output_file)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if !config_path.is_empty() {
            cmd.arg("--config").arg(&config_path);
        }

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                return TtsResult {
                    audio_path: None,
                    duration_seconds: None,
                    sample_rate: 22050,
                    status: VoiceCapabilityStatus::Error,
                    error: Some(format!("Failed to start Piper: {}", e)),
                    processing_time_ms: start.elapsed().as_millis() as u64,
                };
            }
        };

        // Write text to Piper's stdin
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
        }

        let output = match wait_child_with_deadline(child, LEGACY_INFERENCE_TIMEOUT) {
            Ok(output) => output,
            Err(e) => {
                return TtsResult {
                    audio_path: None,
                    duration_seconds: None,
                    sample_rate: 22050,
                    status: VoiceCapabilityStatus::Error,
                    error: Some(format!("Piper process error: {}", e)),
                    processing_time_ms: start.elapsed().as_millis() as u64,
                };
            }
        };

        if output.status.success() && output_file.exists() {
            // Parse the actual WAV header instead of assuming 22050 Hz mono
            // 16-bit: Piper voices have per-model sample rates (16000, 22050,
            // 44100…) and the file-size heuristic produced wrong durations
            // and wrong rates for every voice that is not 22050.
            match hound::WavReader::open(&output_file) {
                Ok(reader) => {
                    let spec = reader.spec();
                    if spec.sample_rate == 0 || reader.duration() == 0 {
                        let _ = std::fs::remove_file(&output_file);
                        return TtsResult {
                            audio_path: None,
                            duration_seconds: None,
                            sample_rate: 0,
                            status: VoiceCapabilityStatus::Error,
                            error: Some("Piper produced an empty WAV stream".to_string()),
                            processing_time_ms: start.elapsed().as_millis() as u64,
                        };
                    }
                    let duration = reader.duration() as f32 / spec.sample_rate as f32;
                    let sample_rate = spec.sample_rate;
                    drop(reader);
                    TtsResult {
                        audio_path: Some(output_file.to_string_lossy().to_string()),
                        duration_seconds: Some(duration),
                        sample_rate,
                        status: VoiceCapabilityStatus::Available,
                        error: None,
                        processing_time_ms: start.elapsed().as_millis() as u64,
                    }
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&output_file);
                    TtsResult {
                        audio_path: None,
                        duration_seconds: None,
                        sample_rate: 0,
                        status: VoiceCapabilityStatus::Error,
                        error: Some(format!("Piper produced an invalid WAV file: {}", e)),
                        processing_time_ms: start.elapsed().as_millis() as u64,
                    }
                }
            }
        } else {
            // A failed run may still have written a partial WAV — remove it.
            let _ = std::fs::remove_file(&output_file);
            let stderr = String::from_utf8_lossy(&output.stderr);
            TtsResult {
                audio_path: None,
                duration_seconds: None,
                sample_rate: 0,
                status: VoiceCapabilityStatus::Error,
                error: Some(format!("Piper synthesis failed: {}", stderr.trim())),
                processing_time_ms: start.elapsed().as_millis() as u64,
            }
        }
    }

    /// Find the Whisper binary on the system
    fn find_whisper_binary(&self) -> Option<String> {
        let whisper_names = if cfg!(target_os = "windows") {
            vec!["whisper.exe", "main.exe"]
        } else {
            vec!["whisper", "main"]
        };

        // Prefer discovered USB binary path
        if let Some(bin) = &self.config.whisper_bin_path {
            if PathBuf::from(bin).exists() {
                return Some(bin.clone());
            }
        }

        // Check RUNTIMES directory
        if let Some(vault_root) = &self.config.output_dir {
            for name in &whisper_names {
                let path = PathBuf::from(vault_root)
                    .join("RUNTIMES")
                    .join(if cfg!(target_os = "windows") {
                        "WINDOWS"
                    } else if cfg!(target_os = "macos") {
                        "MACOS"
                    } else {
                        "LINUX"
                    })
                    .join("VOICE")
                    .join(name);
                if path.exists() {
                    return Some(path.to_string_lossy().to_string());
                }
            }
        }

        None
    }

    /// Find the Piper binary on the system
    fn find_piper_binary(&self) -> Option<String> {
        let piper_names = if cfg!(target_os = "windows") {
            vec!["piper.exe"]
        } else {
            vec!["piper"]
        };

        // Prefer discovered USB binary path
        if let Some(bin) = &self.config.piper_bin_path {
            if PathBuf::from(bin).exists() {
                return Some(bin.clone());
            }
        }

        // Check RUNTIMES directory
        if let Some(vault_root) = &self.config.output_dir {
            for name in &piper_names {
                let path = PathBuf::from(vault_root)
                    .join("RUNTIMES")
                    .join(if cfg!(target_os = "windows") {
                        "WINDOWS"
                    } else if cfg!(target_os = "macos") {
                        "MACOS"
                    } else {
                        "LINUX"
                    })
                    .join("VOICE")
                    .join(name);
                if path.exists() {
                    return Some(path.to_string_lossy().to_string());
                }
            }
        }

        None
    }
}

/// Discover voice binaries and models from the USB vault layout and manifest.
/// Uses only the Pocket AI package; host-installed voice binaries are not trusted.
pub(crate) fn discover_voice_assets(vault_root: &str, language: &str) -> VoiceConfig {
    let root = PathBuf::from(vault_root);

    let whisper_bin_path = find_binary_in_dir(
        root.join("RUNTIMES")
            .join(if cfg!(target_os = "windows") {
                "WINDOWS"
            } else if cfg!(target_os = "macos") {
                "MACOS"
            } else {
                "LINUX"
            })
            .join("VOICE")
            .as_path(),
        &["whisper.exe", "main.exe"],
    );

    let piper_bin_path = find_binary_in_dir(
        root.join("RUNTIMES")
            .join(if cfg!(target_os = "windows") {
                "WINDOWS"
            } else if cfg!(target_os = "macos") {
                "MACOS"
            } else {
                "LINUX"
            })
            .join("VOICE")
            .as_path(),
        &["piper.exe"],
    );

    // Model discovery from the package manifest or default paths. The
    // manifest lookup now reads the schema-v2 structure first
    // (`platforms.<os>.voice` entries with kinds WHISPER_MODEL /
    // PIPER_MODEL) — the old dotted keys belong to schema-v1 manifests the
    // product no longer ships, so they never matched and discovery silently
    // ran on hardcoded defaults that merely happened to coincide with the
    // staged layout.
    let whisper_model = discover_model_path_v2(vault_root, &["WHISPER_MODEL"]).unwrap_or_else(|| {
        discover_model_path(
            vault_root,
            &[
                "models.desktop.whisper.path",
                "models.desktop.whisper_model.path",
            ],
            "MODELS/DESKTOP/whisper-base.en.bin",
        )
    });

    let piper_model = discover_model_path_v2(vault_root, &["PIPER_MODEL"]).unwrap_or_else(|| {
        discover_model_path(
            vault_root,
            &[
                "models.desktop.piper.path",
                "models.desktop.piper_model.path",
            ],
            "MODELS/DESKTOP/voice.onnx",
        )
    });

    let piper_config = if piper_model.ends_with(".onnx") {
        Some(piper_model.clone() + ".json")
    } else {
        None
    };

    let whisper_model_path = file_exists_under(root.as_path(), &whisper_model)
        .then(|| root.join(&whisper_model).to_string_lossy().to_string());

    let piper_model_path = file_exists_under(root.as_path(), &piper_model)
        .then(|| root.join(&piper_model).to_string_lossy().to_string());

    let piper_config_path = piper_config.and_then(|p| {
        file_exists_under(root.as_path(), &p).then(|| root.join(&p).to_string_lossy().to_string())
    });

    VoiceConfig {
        stt_engine: VoiceEngine::WhisperCpp,
        tts_engine: VoiceEngine::Piper,
        language: language.to_string(),
        whisper_bin_path,
        whisper_model_path,
        piper_bin_path,
        piper_model_path,
        piper_config_path,
        output_dir: Some(vault_root.to_string()),
        package_root: Some(vault_root.to_string()),
    }
}

fn find_binary_in_dir(dir: &std::path::Path, names: &[&str]) -> Option<String> {
    for name in names {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

fn discover_model_path(vault_root: &str, manifest_keys: &[&str], default: &str) -> String {
    let manifest_path = PathBuf::from(vault_root).join("manifest.json");
    if let Ok(content) = std::fs::read_to_string(&manifest_path) {
        if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&content) {
            for key in manifest_keys {
                if let Some(path) = get_nested_string(&manifest, key) {
                    let absolute = PathBuf::from(vault_root).join(path);
                    if absolute.exists() {
                        return path.to_string();
                    }
                }
            }
        }
    }
    default.to_string()
}

/// Schema-v2 model discovery: the package manifest lists the legacy voice
/// models as `platforms.<os>.voice` entries with `kind` WHISPER_MODEL /
/// PIPER_MODEL and a package-relative `path`. Returns the first entry whose
/// file actually exists; a manifest naming a missing file is ignored rather
/// than trusted.
fn discover_model_path_v2(vault_root: &str, kinds: &[&str]) -> Option<String> {
    let section = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let manifest_path = PathBuf::from(vault_root).join("manifest.json");
    let content = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&content).ok()?;
    let entries = manifest
        .get("platforms")?
        .get(section)?
        .get("voice")?
        .as_array()?;
    for entry in entries {
        // Entries without a kind or path are skipped, not treated as a
        // failed lookup.
        let Some(kind) = entry.get("kind").and_then(|k| k.as_str()) else {
            continue;
        };
        if !kinds.contains(&kind) {
            continue;
        }
        let Some(path) = entry.get("path").and_then(|p| p.as_str()) else {
            continue;
        };
        if PathBuf::from(vault_root).join(path).exists() {
            return Some(path.to_string());
        }
    }
    None
}

fn get_nested_string<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a str> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    current.as_str()
}

fn file_exists_under(root: &std::path::Path, relative: &str) -> bool {
    root.join(relative).exists()
}

// Tauri commands for voice module

#[tauri::command]
pub fn get_voice_status(vault_root: String, language: String) -> serde_json::Value {
    let config = discover_voice_assets(&vault_root, &language);
    let module = VoiceModule::new(config.clone());

    serde_json::json!({
        "stt": module.check_stt_availability(),
        "tts": module.check_tts_availability(),
        "language": config.language,
        "whisper_model": config.whisper_model_path,
        "piper_model": config.piper_model_path,
    })
}

#[tauri::command]
pub async fn transcribe_audio(
    audio_path: String,
    vault_root: String,
    language: String,
) -> SttResult {
    // Production speech goes through the SpeechRouter: InBharat Audio
    // (audio.cpp / Qwen3-ASR) first, the legacy Whisper plane only as the
    // explicit, coverage-gated fallback. Calling VoiceModule directly here
    // was the router bypass the audit flagged.
    let router = crate::speech::product_router(&vault_root);
    match router.transcribe(std::path::Path::new(&audio_path), &language) {
        Ok(result) => SttResult {
            text: result.text,
            language: result.language.as_str().to_string(),
            confidence: None,
            processing_time_ms: result.processing_time_ms,
            status: VoiceCapabilityStatus::Available,
            error: None,
        },
        Err(error) => SttResult {
            text: String::new(),
            language,
            confidence: None,
            processing_time_ms: 0,
            status: VoiceCapabilityStatus::Error,
            error: Some(error.to_string()),
        },
    }
}

#[tauri::command]
pub async fn synthesize_speech(text: String, vault_root: String, language: String) -> TtsResult {
    let router = crate::speech::product_router(&vault_root);
    match router.synthesize(&text, &language) {
        Ok(result) => TtsResult {
            audio_path: Some(result.audio_path.to_string_lossy().to_string()),
            duration_seconds: result.duration_seconds,
            sample_rate: result.sample_rate,
            status: VoiceCapabilityStatus::Available,
            error: None,
            processing_time_ms: result.processing_time_ms,
        },
        Err(error) => TtsResult {
            audio_path: None,
            duration_seconds: None,
            sample_rate: 0,
            status: VoiceCapabilityStatus::Error,
            error: Some(error.to_string()),
            processing_time_ms: 0,
        },
    }
}

/// Map a canonical BCP-47 tag from `unoone-speech-contracts` to the legacy
/// Whisper/Piper CLI vocabulary (ISO-639-style base codes). The canonical
/// tag stays the session's internal truth; only the CLI boundary sees the
/// base code. `auto` (detect) passes through unchanged.
pub(crate) fn legacy_cli_language(canonical: &str) -> String {
    let tag = unoone_speech_contracts::canonicalize(canonical)
        .map(|tag| tag.as_str().to_string())
        .unwrap_or_else(|_| canonical.trim().to_string());
    tag.split('-').next().unwrap_or("en").to_string()
}

/// Legacy inference deadline. The Whisper/Piper subprocesses previously ran
/// without any deadline (`.output()` blocks forever on a hung binary).
const LEGACY_INFERENCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Run a legacy voice subprocess with a hard deadline, killing it on expiry.
/// Mirrors `bharat_audio::run_command_timeout` (direct invocation, never a
/// shell).
fn run_with_deadline(
    mut cmd: std::process::Command,
    deadline: std::time::Duration,
) -> Result<std::process::Output, String> {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to start legacy voice binary: {e}"))?;
    wait_child_with_deadline(child, deadline)
}

/// Wait for an already-spawned legacy voice subprocess with a hard deadline,
/// draining its pipes in threads so a full stdout pipe can never wedge it.
fn wait_child_with_deadline(
    mut child: std::process::Child,
    deadline: std::time::Duration,
) -> Result<std::process::Output, String> {
    use std::io::Read;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    let out_handle = std::thread::spawn(move || {
        if let Some(mut pipe) = stdout {
            let _ = pipe.read_to_end(&mut stdout_buf);
        }
        stdout_buf
    });
    let err_handle = std::thread::spawn(move || {
        if let Some(mut pipe) = stderr {
            let _ = pipe.read_to_end(&mut stderr_buf);
        }
        stderr_buf
    });
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = out_handle.join().unwrap_or_default();
                let stderr = err_handle.join().unwrap_or_default();
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if start.elapsed() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "legacy voice binary exceeded its {}s deadline and was stopped",
                        deadline.as_secs()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(format!("legacy voice binary failed: {e}")),
        }
    }
}

/// Verify a legacy voice binary/model against the Pocket AI package manifest
/// (schema v2). Legacy assets previously went entirely unverified — only
/// `Path::exists()` was checked. A binary or model that is not tracked by
/// the package manifest, or whose SHA-256 no longer matches, fails closed.
///
/// Verification results are memoized per (path, size, mtime) so repeated
/// transient transcriptions do not re-hash a 148 MB model every call; any
/// change to the file invalidates the cache.
fn verify_legacy_asset(root: &std::path::Path, path: &std::path::Path) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::collections::HashSet;
    use std::sync::Mutex;

    /// (path, size, mtime-secs) of one verified asset; any change to the file
    /// on disk invalidates the memoized entry.
    type VerifiedKey = (std::path::PathBuf, u64, u64);
    type VerifiedSet = Mutex<HashSet<VerifiedKey>>;

    static VERIFIED: std::sync::OnceLock<VerifiedSet> = std::sync::OnceLock::new();

    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("cannot stat legacy voice asset {}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "refusing symlinked legacy voice asset: {}",
            path.display()
        ));
    }
    let modified = meta
        .modified()
        .map_err(|e| format!("cannot read mtime of {}: {e}", path.display()))?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let key = (path.to_path_buf(), meta.len(), modified);

    let cache = VERIFIED.get_or_init(|| Mutex::new(HashSet::new()));
    if cache.lock().map_err(|_| "lock error")?.contains(&key) {
        return Ok(());
    }

    let manifest_path = root.join("manifest.json");
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|e| format!("cannot read Pocket AI package manifest: {e}"))?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| format!("invalid Pocket AI package manifest: {e}"))?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| {
            format!(
                "legacy voice asset escapes the package root: {}",
                path.display()
            )
        })?
        .to_string_lossy()
        .replace('\\', "/");

    // The schema-v2 manifest stores asset entries as arrays (runtimes,
    // models, voice, …) under each platform object. Find the entry whose
    // relative path matches this asset.
    let Some(platforms) = manifest.get("platforms").and_then(|p| p.as_object()) else {
        return Err("Pocket AI package manifest has no platforms section".to_string());
    };
    let mut expected: Option<String> = None;
    'search: for platform in platforms.values() {
        let Some(fields) = platform.as_object() else {
            continue;
        };
        for value in fields.values() {
            let Some(entries) = value.as_array() else {
                continue;
            };
            for entry in entries {
                let Some(entry_path) = entry.get("path").and_then(|p| p.as_str()) else {
                    continue;
                };
                if entry_path.eq_ignore_ascii_case(&relative) {
                    expected = entry
                        .get("sha256")
                        .and_then(|h| h.as_str())
                        .map(|h| h.to_ascii_lowercase());
                    break 'search;
                }
            }
        }
    }
    let Some(expected) = expected else {
        return Err(format!(
            "legacy voice asset '{}' is not tracked by the Pocket AI package manifest — refusing untracked binaries",
            relative
        ));
    };

    // Hash with a heap buffer (see bharat_audio::sha256_file — never a stack
    // array on Windows).
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("cannot open legacy voice asset {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 512 * 1024];
    loop {
        use std::io::Read;
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("cannot read legacy voice asset {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(format!(
            "legacy voice asset '{}' SHA-256 changed after packaging ({actual} != {expected})",
            relative
        ));
    }

    cache.lock().map_err(|_| "lock error")?.insert(key);
    Ok(())
}

#[cfg(test)]
mod legacy_voice_tests {
    use super::*;

    #[test]
    fn legacy_cli_language_maps_canonical_tags() {
        // The session's canonical tag maps to the Whisper/Piper CLI base
        // code; `auto` (detect) passes through unchanged.
        assert_eq!(legacy_cli_language("en-IN"), "en");
        assert_eq!(legacy_cli_language("hi-IN"), "hi");
        assert_eq!(legacy_cli_language("hinglish"), "hi");
        assert_eq!(legacy_cli_language("hi-en-codemix"), "hi");
        assert_eq!(legacy_cli_language("as-IN"), "as");
        assert_eq!(legacy_cli_language("en-US"), "en");
        assert_eq!(legacy_cli_language("auto"), "auto");
    }

    /// Schema-v2 manifest discovery: the shipped package manifest lists the
    /// legacy voice models under `platforms.windows.voice` with kinds
    /// WHISPER_MODEL / PIPER_MODEL. Discovery must read that structure —
    /// the old schema-v1 dotted keys never matched, so the manifest lookup
    /// was dead code and the hardcoded defaults carried the product.
    #[test]
    fn discover_model_path_reads_schema_v2_voice_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        // Stage two model files at NON-default paths so the test can tell
        // manifest-driven discovery apart from the hardcoded fallback.
        let whisper_rel = "MODELS/DESKTOP/custom/whisper-custom.bin";
        let piper_rel = "MODELS/DESKTOP/custom/piper-custom.onnx";
        for rel in [whisper_rel, piper_rel] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"model").unwrap();
        }
        let voice_entries = serde_json::json!([
            {"id": "model-whisper-base.en", "kind": "WHISPER_MODEL",
             "path": whisper_rel, "sha256": "x", "required": true},
            {"id": "model-voice", "kind": "PIPER_MODEL",
             "path": piper_rel, "sha256": "y", "required": true},
            {"id": "some-other-entry", "kind": "VOICE_RUNTIME",
             "path": "RUNTIMES/WINDOWS/VOICE/piper.exe", "sha256": "z", "required": true},
            // Entries without a kind or path must be skipped, not
            // treated as a failed lookup.
            {"id": "kindless", "path": "whatever"}
        ]);
        // The lookup is sectioned by host OS — provide the same entries under
        // every platform so the test is host-agnostic.
        let manifest = serde_json::json!({
            "schema_version": "2",
            "platforms": {
                "windows": {"voice": voice_entries},
                "macos": {"voice": voice_entries},
                "linux": {"voice": voice_entries}
            }
        });
        std::fs::write(root.join("manifest.json"), manifest.to_string()).unwrap();
        let root_str = root.to_string_lossy().to_string();
        assert_eq!(
            discover_model_path_v2(&root_str, &["WHISPER_MODEL"]).unwrap(),
            whisper_rel
        );
        assert_eq!(
            discover_model_path_v2(&root_str, &["PIPER_MODEL"]).unwrap(),
            piper_rel
        );
        // A manifest naming a missing file is ignored.
        std::fs::remove_file(root.join(piper_rel)).unwrap();
        assert_eq!(discover_model_path_v2(&root_str, &["PIPER_MODEL"]), None);

        // End to end: discover_voice_assets uses the manifest paths.
        let config = discover_voice_assets(&root_str, "en-IN");
        assert_eq!(
            config.whisper_model_path.as_deref(),
            Some(root.join(whisper_rel).to_str().unwrap())
        );
        // The piper model fell back once its manifest entry stopped
        // resolving — it must not stay pinned to a deleted manifest path.
        assert_ne!(
            config.piper_model_path.as_deref(),
            Some(root.join(piper_rel).to_str().unwrap())
        );
    }

    #[test]
    fn run_with_deadline_kills_hung_process() {
        // A subprocess that would run for ~30s must be killed at the
        // (shortened) deadline, not block the caller forever.
        let cmd = if cfg!(target_os = "windows") {
            let mut cmd = std::process::Command::new("cmd");
            cmd.args(["/C", "ping -n 30 127.0.0.1"]);
            cmd
        } else {
            let mut cmd = std::process::Command::new("sleep");
            cmd.arg("30");
            cmd
        };
        let start = std::time::Instant::now();
        let result = run_with_deadline(cmd, std::time::Duration::from_millis(500));
        let message = result
            .expect_err("hung process must hit the deadline")
            .to_string();
        assert!(message.contains("deadline"), "got: {message}");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "deadline kill must be prompt"
        );
    }

    /// Build a minimal schema-v2 package manifest with one tracked asset.
    fn package_with_asset(dir: &std::path::Path, relative: &str, content: &[u8]) -> PathBuf {
        use sha2::Digest;
        let asset = dir.join(relative);
        if let Some(parent) = asset.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&asset, content).unwrap();
        let mut hasher = sha2::Sha256::new();
        hasher.update(content);
        let sha256 = format!("{:x}", hasher.finalize());
        let manifest = serde_json::json!({
            "product_id": "unoone-pocket-ai-test",
            "schema_version": 2,
            "platforms": {
                "windows": {
                    "runtimes": [
                        {"path": relative, "size_bytes": content.len(), "sha256": sha256}
                    ]
                }
            }
        });
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        asset
    }

    #[test]
    fn verify_legacy_asset_accepts_matching_entry() {
        let dir = tempfile::tempdir().unwrap();
        let asset = package_with_asset(
            dir.path(),
            "RUNTIMES/WINDOWS/VOICE/whisper.exe",
            b"fake-binary",
        );
        assert!(
            verify_legacy_asset(dir.path(), &asset).is_ok(),
            "a manifest-tracked, hash-matching asset must verify"
        );
        // Memoized second call also succeeds.
        assert!(verify_legacy_asset(dir.path(), &asset).is_ok());
    }

    #[test]
    fn verify_legacy_asset_rejects_untracked_file() {
        let dir = tempfile::tempdir().unwrap();
        package_with_asset(
            dir.path(),
            "RUNTIMES/WINDOWS/VOICE/whisper.exe",
            b"fake-binary",
        );
        let outsider = dir.path().join("RUNTIMES/WINDOWS/VOICE/impostor.exe");
        std::fs::write(&outsider, b"fake-binary").unwrap();
        let err = verify_legacy_asset(dir.path(), &outsider)
            .expect_err("untracked asset must fail closed");
        assert!(err.contains("not tracked"), "got: {err}");
    }

    #[test]
    fn verify_legacy_asset_rejects_tampered_file() {
        let dir = tempfile::tempdir().unwrap();
        let asset = package_with_asset(
            dir.path(),
            "RUNTIMES/WINDOWS/VOICE/whisper.exe",
            b"original",
        );
        std::fs::write(&asset, b"tampered").unwrap();
        let err =
            verify_legacy_asset(dir.path(), &asset).expect_err("tampered asset must fail closed");
        assert!(err.contains("SHA-256 changed"), "got: {err}");
    }

    #[test]
    fn transcribe_reports_errors_as_error_not_text() {
        // Regression for the error-as-transcript bug: a failed whisper run
        // must leave `text` EMPTY and carry the failure in `error`.
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("whisper.exe");
        let model = dir.path().join("model.bin");
        std::fs::write(&bin, b"not a real executable").unwrap();
        std::fs::write(&model, b"not a real model").unwrap();
        let module = VoiceModule::new(VoiceConfig {
            language: "en-IN".to_string(),
            whisper_bin_path: Some(bin.to_string_lossy().to_string()),
            whisper_model_path: Some(model.to_string_lossy().to_string()),
            package_root: None,
            ..VoiceConfig::default()
        });
        let audio = dir.path().join("capture.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let result = module.transcribe(&audio.to_string_lossy());
        assert!(
            result.text.is_empty(),
            "failures must never masquerade as transcript text (got {:?})",
            result.text
        );
        assert!(result.error.is_some(), "failure detail must be in `error`");
        assert!(result.status != VoiceCapabilityStatus::Available);
    }
}
