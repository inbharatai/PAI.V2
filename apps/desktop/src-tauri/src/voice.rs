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
                };
            }
        };

        // Create a temp directory for Whisper output
        let temp_dir = std::env::temp_dir().join("unoone-whisper");
        if let Err(e) = std::fs::create_dir_all(&temp_dir) {
            return SttResult {
                text: format!("Failed to create temp directory: {}", e),
                language: self.config.language.clone(),
                confidence: None,
                processing_time_ms: start.elapsed().as_millis() as u64,
                status: VoiceCapabilityStatus::Error,
            };
        }

        let model_path = match &self.config.whisper_model_path {
            Some(path) => path.clone(),
            None => {
                return SttResult {
                    text: "No Whisper model path configured".to_string(),
                    language: self.config.language.clone(),
                    confidence: None,
                    processing_time_ms: start.elapsed().as_millis() as u64,
                    status: VoiceCapabilityStatus::Error,
                };
            }
        };

        let output_prefix = temp_dir.join("transcription").to_string_lossy().to_string();

        let result = std::process::Command::new(&whisper_bin)
            .args([
                "--model",
                &model_path,
                "--language",
                &self.config.language,
                "-otxt",
                "-of",
                &output_prefix,
                audio_path,
            ])
            .output();

        match result {
            Ok(output) => {
                if output.status.success() {
                    // Read the transcription output file (Whisper appends .txt)
                    let output_file = temp_dir.join("transcription.txt");
                    let text = std::fs::read_to_string(&output_file)
                        .unwrap_or_default()
                        .trim()
                        .to_string();

                    // Clean up temp file
                    let _ = std::fs::remove_file(&output_file);

                    SttResult {
                        text,
                        language: self.config.language.clone(),
                        confidence: None,
                        processing_time_ms: start.elapsed().as_millis() as u64,
                        status: VoiceCapabilityStatus::Available,
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    SttResult {
                        text: format!("Whisper transcription failed: {}", stderr.trim()),
                        language: self.config.language.clone(),
                        confidence: None,
                        processing_time_ms: start.elapsed().as_millis() as u64,
                        status: VoiceCapabilityStatus::Error,
                    }
                }
            }
            Err(e) => SttResult {
                text: format!("Failed to run Whisper: {}", e),
                language: self.config.language.clone(),
                confidence: None,
                processing_time_ms: start.elapsed().as_millis() as u64,
                status: VoiceCapabilityStatus::Error,
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

        let output_file =
            output_dir.join(format!("tts_{}.wav", chrono::Utc::now().timestamp_millis()));

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

        let output = match child.wait_with_output() {
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
            // Estimate duration from file size (WAV at 22050 Hz, 16-bit mono ≈ 44100 bytes/sec)
            let file_size = std::fs::metadata(&output_file)
                .map(|m| m.len())
                .unwrap_or(0);
            let duration = file_size as f32 / 44100.0;

            TtsResult {
                audio_path: Some(output_file.to_string_lossy().to_string()),
                duration_seconds: Some(duration),
                sample_rate: 22050,
                status: VoiceCapabilityStatus::Available,
                error: None,
                processing_time_ms: start.elapsed().as_millis() as u64,
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            TtsResult {
                audio_path: None,
                duration_seconds: None,
                sample_rate: 22050,
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

    // Model discovery from manifest or default paths
    let whisper_model = discover_model_path(
        vault_root,
        &[
            "models.desktop.whisper.path",
            "models.desktop.whisper_model.path",
        ],
        "MODELS/DESKTOP/whisper-base.en.bin",
    );

    let piper_model = discover_model_path(
        vault_root,
        &[
            "models.desktop.piper.path",
            "models.desktop.piper_model.path",
        ],
        "MODELS/DESKTOP/voice.onnx",
    );

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
    let config = discover_voice_assets(&vault_root, &language);
    let module = VoiceModule::new(config);
    module.transcribe(&audio_path)
}

#[tauri::command]
pub async fn synthesize_speech(text: String, vault_root: String, language: String) -> TtsResult {
    let config = discover_voice_assets(&vault_root, &language);
    let module = VoiceModule::new(config);
    module.synthesize(&text)
}
