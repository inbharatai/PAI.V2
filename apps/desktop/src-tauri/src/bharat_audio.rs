//! Pocket AI adapter for the universal InBharat Audio runtime.
//!
//! Product policy:
//! - The universal InBharat Audio library and upstream audio.cpp stay outside
//!   the UnoOne product layer.
//! - This adapter reads a vault-local, explicit speech manifest.
//! - Real audio.cpp inference is fail-closed: `ibaudio audio-cpp-status --json`
//!   must report `inference_ready=true` before this module will execute
//!   `audiocpp_cli`.
//! - If any gate fails, callers must retain the existing Whisper/Piper path.
//! - Commands are invoked directly (never through a shell) and model/output
//!   paths are confined to the verified Pocket AI package root.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CONFIG_RELATIVE_PATH: &str = "SPEECH/config/inbharat-audio.v1.json";
const ACCEPTANCE_RELATIVE_PATH: &str = "SPEECH/acceptance/audio-cpp.acceptance.v1.json";
const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_INPUT_AUDIO_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TTS_TEXT_BYTES: usize = 32 * 1024;
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const INFERENCE_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechTaskConfig {
    pub family: String,
    pub model_relative_path: String,
    #[serde(default)]
    pub default_language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InBharatAudioManifest {
    pub schema: String,
    pub enabled: bool,
    pub upstream_commit: String,
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default)]
    pub allowed_languages: Vec<String>,
    pub asr: Option<SpeechTaskConfig>,
    pub tts: Option<SpeechTaskConfig>,
}

fn default_backend() -> String {
    "best".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioCppReadiness {
    pub schema: String,
    pub adapter_compiled: bool,
    pub inference_ready: bool,
    pub reviewed_commit: String,
    pub upstream_source: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrAcceptance {
    pub family: String,
    pub model_relative_path: String,
    pub model_sha256: String,
    pub language: String,
    pub fixture_sha256: String,
    pub transcript_sha256: String,
    pub transcript_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsAcceptance {
    pub family: String,
    pub model_relative_path: String,
    pub model_sha256: String,
    pub language: String,
    pub output_sha256: String,
    pub sample_rate: u32,
    pub duration_seconds: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioCppAcceptance {
    pub schema: String,
    pub upstream_commit: String,
    pub platform: String,
    pub backend: String,
    pub audiocpp_cli_sha256: String,
    pub ibaudio_cli_sha256: String,
    pub asr: AsrAcceptance,
    pub tts: TtsAcceptance,
    pub tested_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BharatAudioStatus {
    pub configured: bool,
    pub enabled: bool,
    pub production_ready: bool,
    pub reason: String,
    pub upstream_commit: Option<String>,
    pub asr_family: Option<String>,
    pub tts_family: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BharatAsrResult {
    pub transcript: String,
    pub language: String,
    pub processing_time_ms: u64,
}

#[derive(Debug, Clone)]
pub struct BharatTtsResult {
    pub audio_path: String,
    pub sample_rate: u32,
    pub duration_seconds: Option<f32>,
    pub processing_time_ms: u64,
}

fn platform_runtime_dir() -> &'static str {
    if cfg!(target_os = "windows") {
        "WINDOWS"
    } else if cfg!(target_os = "macos") {
        "MACOS"
    } else {
        "LINUX"
    }
}

fn platform_binary(base: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{}.exe", base)
    } else {
        base.to_string()
    }
}

fn canonical_root(vault_root: &str) -> Result<PathBuf, String> {
    PathBuf::from(vault_root)
        .canonicalize()
        .map_err(|e| format!("Pocket AI root is unavailable: {e}"))
}

fn canonical_under(root: &Path, relative: &str, must_exist: bool) -> Result<PathBuf, String> {
    let rel = Path::new(relative);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("refusing non-confined relative path: {relative}"));
    }
    let candidate = root.join(rel);
    let canonical = if must_exist {
        candidate.canonicalize().map_err(|e| {
            format!(
                "required Pocket AI asset is missing ({}): {e}",
                candidate.display()
            )
        })?
    } else {
        let parent = candidate
            .parent()
            .ok_or_else(|| "output path has no parent".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create audio output directory: {e}"))?;
        let canonical_parent = parent
            .canonicalize()
            .map_err(|e| format!("cannot canonicalize audio output directory: {e}"))?;
        canonical_parent.join(
            candidate
                .file_name()
                .ok_or_else(|| "output path has no filename".to_string())?,
        )
    };
    if !canonical.starts_with(root) {
        return Err(format!(
            "path escapes Pocket AI root: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn read_manifest(vault_root: &str) -> Result<(PathBuf, InBharatAudioManifest), String> {
    let root = canonical_root(vault_root)?;
    let path = canonical_under(&root, CONFIG_RELATIVE_PATH, true)?;
    let size = std::fs::metadata(&path)
        .map_err(|e| format!("cannot stat InBharat Audio config: {e}"))?
        .len();
    if size > MAX_CONFIG_BYTES {
        return Err("InBharat Audio config exceeds 256 KiB".to_string());
    }
    let bytes =
        std::fs::read(&path).map_err(|e| format!("cannot read InBharat Audio config: {e}"))?;
    let manifest: InBharatAudioManifest = serde_json::from_slice(&bytes)
        .map_err(|e| format!("invalid InBharat Audio config: {e}"))?;
    if manifest.schema != "inbharat.pai.speech.v1" {
        return Err(format!(
            "unsupported InBharat Audio schema: {}",
            manifest.schema
        ));
    }
    if manifest.upstream_commit.len() != 40
        || !manifest
            .upstream_commit
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err("upstream_commit must be a full 40-character Git commit SHA".to_string());
    }
    if !matches!(
        manifest.backend.as_str(),
        "cpu" | "cuda" | "vulkan" | "metal" | "best"
    ) {
        return Err(format!(
            "unsupported audio.cpp backend: {}",
            manifest.backend
        ));
    }
    Ok((root, manifest))
}

fn ibaudio_cli(root: &Path) -> Result<PathBuf, String> {
    let rel = format!(
        "RUNTIMES/{}/AUDIO/{}",
        platform_runtime_dir(),
        platform_binary("ibaudio")
    );
    canonical_under(root, &rel, true)
}

fn audio_cpp_cli(root: &Path) -> Result<PathBuf, String> {
    let rel = format!(
        "RUNTIMES/{}/AUDIO/{}",
        platform_runtime_dir(),
        platform_binary("audiocpp_cli")
    );
    canonical_under(root, &rel, true)
}

fn read_stream(stream: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stream.take(8 * 1024 * 1024).read_to_end(&mut bytes);
        bytes
    })
}

fn run_command_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<(String, String), String> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to launch local audio runtime: {e}"))?;
    let stdout_reader = child.stdout.take().map(read_stream);
    let stderr_reader = child.stderr.take().map(read_stream);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "local audio runtime exceeded {} seconds",
                    timeout.as_secs()
                ));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("failed while waiting for local audio runtime: {e}"));
            }
        }
    };
    let stdout = stdout_reader
        .and_then(|j| j.join().ok())
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default();
    let stderr = stderr_reader
        .and_then(|j| j.join().ok())
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default();
    if !status.success() {
        return Err(format!(
            "local audio runtime exited with {status}: {}",
            stderr.trim()
        ));
    }
    Ok((stdout, stderr))
}

fn query_readiness(root: &Path) -> Result<AudioCppReadiness, String> {
    let cli = ibaudio_cli(root)?;
    let mut cmd = Command::new(cli);
    cmd.arg("audio-cpp-status").arg("--json");
    let (stdout, _) = run_command_timeout(cmd, STATUS_TIMEOUT)?;
    serde_json::from_str(stdout.trim())
        .map_err(|e| format!("invalid InBharat Audio readiness response: {e}"))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("cannot open {} for SHA-256: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("cannot read {} for SHA-256: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_asset(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("cannot stat {} for SHA-256: {e}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing symlinked audio asset: {}",
            path.display()
        ));
    }
    if metadata.is_file() {
        return sha256_file(path);
    }
    if !metadata.is_dir() {
        return Err(format!(
            "audio asset is neither a regular file nor directory: {}",
            path.display()
        ));
    }

    fn collect(root: &Path, current: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        if out.len() > 100_000 {
            return Err("audio model package exceeds 100,000 files".to_string());
        }
        for entry in std::fs::read_dir(current)
            .map_err(|e| format!("cannot enumerate {}: {e}", current.display()))?
        {
            let entry = entry.map_err(|e| format!("cannot enumerate audio model entry: {e}"))?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "refusing symlink inside audio model package: {}",
                    path.display()
                ));
            }
            if metadata.is_dir() {
                collect(root, &path, out)?;
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| "audio model package path escaped its root".to_string())?
                    .to_path_buf();
                out.push(relative);
            } else {
                return Err(format!(
                    "unsupported special file in audio model package: {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    collect(path, path, &mut files)?;
    files.sort_by(|left, right| {
        left.to_string_lossy()
            .replace('\\', "/")
            .cmp(&right.to_string_lossy().replace('\\', "/"))
    });
    if files.is_empty() {
        return Err(format!("audio model package is empty: {}", path.display()));
    }
    let mut hasher = Sha256::new();
    hasher.update(b"IBAUDIO_TREE_SHA256_V1\n");
    for relative in files {
        let relative_text = relative.to_string_lossy().replace('\\', "/");
        let file_hash = sha256_file(&path.join(&relative))?;
        hasher.update(relative_text.as_bytes());
        hasher.update([0u8]);
        hasher.update(file_hash.as_bytes());
        hasher.update(b"\n");
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn verify_acceptance(
    root: &Path,
    manifest: &InBharatAudioManifest,
    ibaudio_path: &Path,
    audiocpp_path: &Path,
) -> Result<AudioCppAcceptance, String> {
    let path = canonical_under(root, ACCEPTANCE_RELATIVE_PATH, true)?;
    let size = std::fs::metadata(&path)
        .map_err(|e| format!("cannot stat audio.cpp acceptance attestation: {e}"))?
        .len();
    if size == 0 || size > MAX_CONFIG_BYTES {
        return Err("audio.cpp acceptance attestation is empty or too large".to_string());
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("cannot read audio.cpp acceptance attestation: {e}"))?;
    let acceptance: AudioCppAcceptance = serde_json::from_slice(&bytes)
        .map_err(|e| format!("invalid audio.cpp acceptance attestation: {e}"))?;
    if acceptance.schema != "inbharat.pai.audio_cpp_acceptance.v1" {
        return Err(format!(
            "unsupported audio.cpp acceptance schema: {}",
            acceptance.schema
        ));
    }
    if !acceptance
        .upstream_commit
        .eq_ignore_ascii_case(&manifest.upstream_commit)
    {
        return Err("audio.cpp acceptance commit does not match speech manifest".to_string());
    }
    if !acceptance.backend.eq_ignore_ascii_case(&manifest.backend) {
        return Err("audio.cpp acceptance backend does not match speech manifest".to_string());
    }
    let expected_platform = platform_runtime_dir();
    if !acceptance.platform.eq_ignore_ascii_case(expected_platform) {
        return Err(format!(
            "audio.cpp acceptance platform '{}' does not match runtime '{}'",
            acceptance.platform, expected_platform
        ));
    }
    for digest in [
        acceptance.audiocpp_cli_sha256.as_str(),
        acceptance.ibaudio_cli_sha256.as_str(),
        acceptance.asr.model_sha256.as_str(),
        acceptance.asr.fixture_sha256.as_str(),
        acceptance.asr.transcript_sha256.as_str(),
        acceptance.tts.model_sha256.as_str(),
        acceptance.tts.output_sha256.as_str(),
    ] {
        if !is_sha256(digest) {
            return Err("audio.cpp acceptance contains a malformed SHA-256 digest".to_string());
        }
    }
    if acceptance.asr.transcript_bytes == 0 {
        return Err("audio.cpp ASR acceptance transcript was empty".to_string());
    }
    if acceptance.tts.sample_rate == 0
        || !acceptance.tts.duration_seconds.is_finite()
        || acceptance.tts.duration_seconds <= 0.0
    {
        return Err("audio.cpp TTS acceptance does not contain a valid WAV result".to_string());
    }
    let asr = manifest
        .asr
        .as_ref()
        .ok_or_else(|| "ASR is missing from enabled speech manifest".to_string())?;
    let tts = manifest
        .tts
        .as_ref()
        .ok_or_else(|| "TTS is missing from enabled speech manifest".to_string())?;
    if acceptance.asr.family != asr.family
        || acceptance.asr.model_relative_path != asr.model_relative_path
        || acceptance.tts.family != tts.family
        || acceptance.tts.model_relative_path != tts.model_relative_path
    {
        return Err("audio.cpp acceptance model/family does not match speech manifest".to_string());
    }
    validate_language(manifest, &acceptance.asr.language)?;
    validate_language(manifest, &acceptance.tts.language)?;

    let asr_model = canonical_under(root, &asr.model_relative_path, true)?;
    let tts_model = canonical_under(root, &tts.model_relative_path, true)?;
    let actual = [
        (
            audiocpp_path,
            acceptance.audiocpp_cli_sha256.as_str(),
            "audiocpp_cli",
        ),
        (
            ibaudio_path,
            acceptance.ibaudio_cli_sha256.as_str(),
            "ibaudio",
        ),
        (
            &asr_model,
            acceptance.asr.model_sha256.as_str(),
            "ASR model",
        ),
        (
            &tts_model,
            acceptance.tts.model_sha256.as_str(),
            "TTS model",
        ),
    ];
    for (path, expected, label) in actual {
        let got = sha256_asset(path)?;
        if !got.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "{label} SHA-256 changed after real acceptance testing"
            ));
        }
    }
    Ok(acceptance)
}

fn verify_pocket_ai_package(root: &Path) -> Result<(), String> {
    let result = crate::security::verify_manifest(root.to_string_lossy().to_string())?;
    if !result.manifest_valid || !result.hmac_valid || result.entries_failed != 0 {
        return Err(format!(
            "Pocket AI package integrity gate failed: manifest_valid={} hmac_valid={} entries_failed={}; {}",
            result.manifest_valid,
            result.hmac_valid,
            result.entries_failed,
            result.errors.join("; ")
        ));
    }
    Ok(())
}

fn ensure_production_ready(
    root: &Path,
    manifest: &InBharatAudioManifest,
) -> Result<AudioCppReadiness, String> {
    if !manifest.enabled {
        return Err("InBharat Audio is installed but not enabled for production".to_string());
    }
    if manifest.allowed_languages.is_empty() {
        return Err(
            "production speech manifest must explicitly declare allowed_languages".to_string(),
        );
    }
    if manifest.asr.as_ref().is_some_and(|task| {
        task.family.trim().is_empty() || task.model_relative_path.trim().is_empty()
    }) || manifest.tts.as_ref().is_some_and(|task| {
        task.family.trim().is_empty() || task.model_relative_path.trim().is_empty()
    }) {
        return Err("speech task family/model path must be non-empty".to_string());
    }
    verify_pocket_ai_package(root)?;
    let ibaudio_path = ibaudio_cli(root)?;
    let audiocpp_path = audio_cpp_cli(root)?;
    let status = query_readiness(root)?;
    if !status.adapter_compiled {
        return Err(format!(
            "InBharat Audio was not built against its reviewed audio.cpp checkout: {}",
            status.reason
        ));
    }
    if !status
        .reviewed_commit
        .eq_ignore_ascii_case(&manifest.upstream_commit)
    {
        return Err(format!(
            "audio.cpp commit mismatch: runtime={} config={}",
            status.reviewed_commit, manifest.upstream_commit
        ));
    }
    // The universal library deliberately does not claim its internal model-family
    // adapter is production-ready yet. Pocket AI uses the real upstream CLI path
    // and requires a hash-bound end-to-end ASR+TTS acceptance attestation instead.
    let _acceptance = verify_acceptance(root, manifest, &ibaudio_path, &audiocpp_path)?;
    Ok(status)
}

fn validate_language(manifest: &InBharatAudioManifest, requested: &str) -> Result<(), String> {
    if requested.trim().is_empty() || manifest.allowed_languages.is_empty() {
        return Ok(());
    }
    if manifest
        .allowed_languages
        .iter()
        .any(|lang| lang.eq_ignore_ascii_case(requested))
    {
        Ok(())
    } else {
        Err(format!(
            "language '{}' is not enabled in the Pocket AI speech pack",
            requested
        ))
    }
}

#[tauri::command]
pub fn get_bharat_audio_status(vault_root: String) -> BharatAudioStatus {
    status(&vault_root)
}

pub fn status(vault_root: &str) -> BharatAudioStatus {
    let Ok((root, manifest)) = read_manifest(vault_root) else {
        return BharatAudioStatus {
            configured: false,
            enabled: false,
            production_ready: false,
            reason: "SPEECH/config/inbharat-audio.v1.json not configured".to_string(),
            upstream_commit: None,
            asr_family: None,
            tts_family: None,
        };
    };
    let readiness = ensure_production_ready(&root, &manifest);
    BharatAudioStatus {
        configured: true,
        enabled: manifest.enabled,
        production_ready: readiness.is_ok(),
        reason: readiness
            .map(|_| "audio.cpp production gate passed".to_string())
            .unwrap_or_else(|e| e),
        upstream_commit: Some(manifest.upstream_commit.clone()),
        asr_family: manifest.asr.as_ref().map(|a| a.family.clone()),
        tts_family: manifest.tts.as_ref().map(|a| a.family.clone()),
    }
}

pub fn transcribe(
    vault_root: &str,
    audio_path: &str,
    language: &str,
) -> Result<BharatAsrResult, String> {
    let start = Instant::now();
    let (root, manifest) = read_manifest(vault_root)?;
    ensure_production_ready(&root, &manifest)?;
    let task = manifest
        .asr
        .as_ref()
        .ok_or_else(|| "ASR is not configured in the Pocket AI speech pack".to_string())?;
    let model = canonical_under(&root, &task.model_relative_path, true)?;
    let cli = audio_cpp_cli(&root)?;

    let input = PathBuf::from(audio_path)
        .canonicalize()
        .map_err(|e| format!("audio input is unavailable: {e}"))?;
    let input_meta =
        std::fs::metadata(&input).map_err(|e| format!("cannot stat audio input: {e}"))?;
    if !input_meta.is_file() || input_meta.len() > MAX_INPUT_AUDIO_BYTES {
        return Err("audio input is not a regular file or exceeds 512 MiB".to_string());
    }

    let transcript_rel = format!(
        "VAULT/recordings/transcripts/inbharat_asr_{}.txt",
        chrono::Utc::now().timestamp_millis()
    );
    let transcript_path = canonical_under(&root, &transcript_rel, false)?;
    let mut cmd = Command::new(cli);
    cmd.arg("--task")
        .arg("asr")
        .arg("--family")
        .arg(&task.family)
        .arg("--model")
        .arg(model)
        .arg("--backend")
        .arg(&manifest.backend)
        .arg("--audio")
        .arg(&input)
        .arg("--text-out")
        .arg(&transcript_path);
    let selected_language = if language.trim().is_empty() {
        task.default_language.as_deref().unwrap_or("")
    } else {
        language
    };
    if selected_language.is_empty() {
        return Err("ASR language must be explicit or provided by the speech manifest".to_string());
    }
    validate_language(&manifest, selected_language)?;
    if !selected_language.is_empty() {
        cmd.arg("--language").arg(selected_language);
    }
    let _ = run_command_timeout(cmd, INFERENCE_TIMEOUT)?;
    let transcript = std::fs::read_to_string(&transcript_path)
        .map_err(|e| format!("audio.cpp ASR did not produce its declared transcript file: {e}"))?
        .trim()
        .to_string();
    let _ = std::fs::remove_file(&transcript_path);
    if transcript.is_empty() {
        return Err("audio.cpp ASR returned an empty transcript".to_string());
    }
    Ok(BharatAsrResult {
        transcript,
        language: selected_language.to_string(),
        processing_time_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

pub fn synthesize(vault_root: &str, text: &str, language: &str) -> Result<BharatTtsResult, String> {
    let start = Instant::now();
    if text.trim().is_empty() || text.len() > MAX_TTS_TEXT_BYTES {
        return Err("TTS text must be non-empty and at most 32 KiB".to_string());
    }
    let (root, manifest) = read_manifest(vault_root)?;
    ensure_production_ready(&root, &manifest)?;
    let task = manifest
        .tts
        .as_ref()
        .ok_or_else(|| "TTS is not configured in the Pocket AI speech pack".to_string())?;
    let model = canonical_under(&root, &task.model_relative_path, true)?;
    let cli = audio_cpp_cli(&root)?;
    let relative_output = format!(
        "VAULT/recordings/tts/inbharat_tts_{}.wav",
        chrono::Utc::now().timestamp_millis()
    );
    let output = canonical_under(&root, &relative_output, false)?;

    let mut cmd = Command::new(cli);
    cmd.arg("--task")
        .arg("tts")
        .arg("--family")
        .arg(&task.family)
        .arg("--model")
        .arg(model)
        .arg("--backend")
        .arg(&manifest.backend)
        .arg("--text")
        .arg(text)
        .arg("--out")
        .arg(&output);
    let selected_language = if language.trim().is_empty() {
        task.default_language.as_deref().unwrap_or("")
    } else {
        language
    };
    if selected_language.is_empty() {
        return Err("TTS language must be explicit or provided by the speech manifest".to_string());
    }
    validate_language(&manifest, selected_language)?;
    cmd.arg("--language").arg(selected_language);
    let _ = run_command_timeout(cmd, INFERENCE_TIMEOUT)?;
    if !output.is_file() {
        return Err(
            "audio.cpp TTS completed without producing its declared output file".to_string(),
        );
    }
    let reader = hound::WavReader::open(&output).map_err(|error| {
        let _ = std::fs::remove_file(&output);
        format!("audio.cpp TTS produced an invalid WAV file: {error}")
    })?;
    let spec = reader.spec();
    if spec.sample_rate == 0 || spec.channels == 0 || reader.duration() == 0 {
        drop(reader);
        let _ = std::fs::remove_file(&output);
        return Err("audio.cpp TTS produced an empty or invalid WAV stream".to_string());
    }
    let duration_seconds = Some(reader.duration() as f32 / spec.sample_rate as f32);
    let sample_rate = spec.sample_rate;
    drop(reader);
    Ok(BharatTtsResult {
        audio_path: output.to_string_lossy().to_string(),
        sample_rate,
        duration_seconds,
        processing_time_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(canonical_under(&root, "../outside", false).is_err());
    }

    #[test]
    fn disabled_manifest_never_passes_readiness() {
        let manifest = InBharatAudioManifest {
            schema: "inbharat.pai.speech.v1".to_string(),
            enabled: false,
            upstream_commit: "a".repeat(40),
            backend: "cpu".to_string(),
            allowed_languages: vec!["en".to_string()],
            asr: None,
            tts: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(ensure_production_ready(&root, &manifest).is_err());
    }
}
