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
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use unoone_speech_contracts::{provider_serves, resolve_provider_key, LanguageTag, SpeechTask};

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
    /// Runtime facts straight from the universal library's status API, which
    /// derives readiness from an actual probe of usable local model assets
    /// (never compile-time truth). Surfaced even when the full production
    /// gate fails so the UI can show WHY: missing weights read differently
    /// from a disabled manifest or a stale attestation.
    pub inference_ready: bool,
    /// Honest streaming semantics for this route, from the shared contracts
    /// crate — "buffered-final" means the engine buffers the whole utterance
    /// and emits one final result; it must never be claimed as streaming.
    pub streaming_class: String,
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

/// Confine a caller-supplied audio input path. Accepted inputs are exactly:
/// a regular, non-symlink file of at most `MAX_INPUT_AUDIO_BYTES` that lives
/// under the verified Pocket AI root or under the OS capture subdirectory
/// (`<temp>/unoone-stt`) where `recording::transcribe_transiently` writes its
/// transient WAV captures. Anything else — including files merely anywhere in
/// OS temp, paths that escape via symlink canonicalization, or traversal — is
/// rejected before any subprocess sees it.
fn confine_audio_input(root: &Path, audio_path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(audio_path);
    let symlink_meta = std::fs::symlink_metadata(candidate)
        .map_err(|e| format!("cannot stat audio input: {e}"))?;
    if symlink_meta.file_type().is_symlink() {
        return Err("refusing symlinked audio input".to_string());
    }
    let input = candidate
        .canonicalize()
        .map_err(|e| format!("audio input is unavailable: {e}"))?;
    let meta = std::fs::metadata(&input).map_err(|e| format!("cannot stat audio input: {e}"))?;
    if !meta.is_file() || meta.len() > MAX_INPUT_AUDIO_BYTES {
        return Err("audio input is not a regular file or exceeds 512 MiB".to_string());
    }
    let capture_root = std::env::temp_dir().join("unoone-stt");
    std::fs::create_dir_all(&capture_root)
        .map_err(|e| format!("cannot prepare the OS capture temp area: {e}"))?;
    let capture_root = capture_root
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize the OS capture temp area: {e}"))?;
    if !input.starts_with(root) && !input.starts_with(&capture_root) {
        return Err(format!(
            "audio input must live under the Pocket AI root or the unoone-stt capture area: {}",
            input.display()
        ));
    }
    Ok(input)
}

/// How long a transient TTS WAV may outlive its request in the OS temp area
/// before a later synthesize call sweeps it. Playback needs the file to
/// survive the request that produced it; an hour comfortably covers "play it
/// back a few times" while guaranteeing plaintext derived from user text
/// never accumulates on disk.
const TTS_OUTPUT_TTL: Duration = Duration::from_secs(3600);

/// The transient TTS output area, OUTSIDE the encrypted vault tree (see the
/// note in `synthesize`). A sibling of the `unoone-stt` capture area the
/// recording module already owns.
fn tts_output_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("unoone-tts");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot prepare the TTS output temp area: {e}"))?;
    dir.canonicalize()
        .map_err(|e| format!("cannot canonicalize the TTS output temp area: {e}"))
}

/// Best-effort removal of TTS outputs older than `ttl`. Never fails a speech
/// request: a sweep error just leaves a file for the next sweep (or the OS
/// temp cleaner). The TTL is a parameter so tests can exercise the age
/// boundary without forging file times.
fn sweep_tts_outputs_older_than(dir: &Path, ttl: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("inbharat_tts_")
        {
            continue;
        }
        let stale = metadata
            .modified()
            .map(|mtime| mtime.elapsed().map(|age| age > ttl).unwrap_or(true))
            .unwrap_or(true);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn sweep_stale_tts_outputs(dir: &Path) {
    sweep_tts_outputs_older_than(dir, TTS_OUTPUT_TTL)
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
    // Heap-allocate the read buffer: a 1 MiB stack array overflows the 1 MiB
    // default Windows thread stack (STATUS_STACK_OVERFLOW 0xC00000FD) — the
    // same defect the C++ twin already fixed in
    // vendor/Inbharat-audiocpp/src/sha256.cpp (heap std::vector). Proven by
    // `sha256_file_runs_on_small_stack_thread`.
    let mut buffer = vec![0u8; 512 * 1024];
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

/// Memoized SHA-256 for acceptance-gate assets.
///
/// The preflight gate hashes the ASR/TTS model trees (≈2.5 GB) against the
/// acceptance attestation before every subprocess spawn, and one speech
/// request runs the gate twice (`status()` + the request itself) — every
/// transcribe/synthesize call re-read gigabytes of pendrive bytes for files
/// that did not change. Memoizing by (path, size, mtime) — the same posture
/// as the legacy plane's `verify_legacy_asset` — keeps the hash-before-spawn
/// property real while making unchanged assets a metadata lookup: any size
/// or mtime change is a cache miss and a full re-hash, and entries expire
/// after `HASH_MEMO_TTL` so a long-lived process periodically re-verifies
/// the bytes for real. A same-size, same-mtime byte swap inside the TTL is
/// the residual window, accepted deliberately (matches the legacy plane) —
/// the package manifest sweep still independently verifies the vault.
type HashMemoKey = (PathBuf, u64, u64);
type HashMemo = HashMap<HashMemoKey, (Instant, String)>;
const HASH_MEMO_TTL: Duration = Duration::from_secs(600);

fn sha256_file_memoized(path: &Path) -> Result<String, String> {
    fn memo() -> &'static Mutex<HashMemo> {
        static MEMO: OnceLock<Mutex<HashMemo>> = OnceLock::new();
        MEMO.get_or_init(|| Mutex::new(HashMap::new()))
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("cannot stat {} for SHA-256: {e}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "refusing symlinked audio asset: {}",
            path.display()
        ));
    }
    let modified = metadata
        .modified()
        .map_err(|e| format!("cannot read mtime of {}: {e}", path.display()))?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let key = (path.to_path_buf(), metadata.len(), modified);
    if let Ok(guard) = memo().lock() {
        if let Some((hashed_at, digest)) = guard.get(&key) {
            if hashed_at.elapsed() < HASH_MEMO_TTL {
                return Ok(digest.clone());
            }
        }
    }
    let digest = sha256_file(path)?;
    if let Ok(mut guard) = memo().lock() {
        // Bound the cache; the TTL retain keeps the working set tiny in
        // practice but the cap guarantees it even under adversarial churn.
        if guard.len() >= 4096 {
            guard.retain(|_, (hashed_at, _)| hashed_at.elapsed() < HASH_MEMO_TTL);
        }
        guard.insert(key, (Instant::now(), digest.clone()));
    }
    Ok(digest)
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
        return sha256_file_memoized(path);
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
        // The tree hash enumerates and re-keys every file on every call; only
        // the per-file digests are memoized, so added/removed/renamed files
        // still change the tree hash immediately.
        let file_hash = sha256_file_memoized(&path.join(&relative))?;
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

/// The three runtime-fact gates every production request must pass. Split out
/// so tests can exercise the semantics without a real CLI package: the
/// runtime status must say the adapter was compiled against the reviewed
/// checkout, the reviewed commit must match the speech pack, and — the fix
/// for the compile-time-truth bug — the runtime must report
/// `inference_ready=true`, which the universal library now derives from an
/// actual probe of usable local model assets, not from the fact that the
/// adapter was compiled in.
fn check_runtime_status(
    manifest: &InBharatAudioManifest,
    status: &AudioCppReadiness,
) -> Result<(), String> {
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
    if !status.inference_ready {
        return Err(format!(
            "audio.cpp inference is not ready on this machine: {}",
            status.reason
        ));
    }
    Ok(())
}

/// Manifest sanity + package integrity gate + CLI resolution + acceptance
/// hash verification + the runtime readiness query. Order is fail-closed, and
/// the order is the security property: `verify_acceptance` hashes the two
/// runtime executables (and the ASR/TTS models) against the acceptance
/// attestation BEFORE `query_readiness` spawns `ibaudio.exe`. Nothing gets
/// process execution until its bytes have been verified — the audit found the
/// CLIs were executed first and hash-checked only afterwards.
fn preflight_speech_gate(
    root: &Path,
    manifest: &InBharatAudioManifest,
) -> Result<(PathBuf, PathBuf, AudioCppAcceptance, AudioCppReadiness), String> {
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
    // Hash-verify the executables and models BEFORE the first spawn.
    let acceptance = verify_acceptance(root, manifest, &ibaudio_path, &audiocpp_path)?;
    let status = query_readiness(root)?;
    Ok((ibaudio_path, audiocpp_path, acceptance, status))
}

fn ensure_production_ready(
    root: &Path,
    manifest: &InBharatAudioManifest,
) -> Result<AudioCppReadiness, String> {
    let (_ibaudio_path, _audiocpp_path, _acceptance, status) =
        preflight_speech_gate(root, manifest)?;
    check_runtime_status(manifest, &status)?;
    // The universal library deliberately does not claim its internal model-family
    // adapter is production-ready yet. Pocket AI uses the real upstream CLI path
    // and requires a hash-bound end-to-end ASR+TTS acceptance attestation instead,
    // verified inside the preflight gate before any process spawn.
    Ok(status)
}

/// Validate a user- or manifest-supplied language against the speech pack's
/// allowlist, using BCP-47 canonicalization from `unoone-speech-contracts`
/// (`as`/`as-IN` → `as-IN`, `hi` → `hi-IN`, `hinglish` → `hi-en-codemix`;
/// global tags like `fr` pass through and are never re-rooted).
///
/// Fail-closed by construction: an empty request is an error and an empty
/// allowlist is an error — the previous silent-bypass (both returned Ok) is
/// gone. Returns the canonical tag the caller should report in results; the
/// CLI boundary keeps the original alias string because that is the exact
/// vocabulary the acceptance attestation pinned.
fn validate_language(
    manifest: &InBharatAudioManifest,
    requested: &str,
) -> Result<LanguageTag, String> {
    let tag = unoone_speech_contracts::canonicalize(requested)
        .map_err(|e| format!("cannot use speech language '{requested}': {e}"))?;
    if manifest.allowed_languages.is_empty() {
        return Err(
            "production speech manifest must explicitly declare allowed_languages".to_string(),
        );
    }
    for entry in &manifest.allowed_languages {
        let entry_tag = unoone_speech_contracts::canonicalize(entry)
            .map_err(|e| format!("speech manifest language '{entry}' is invalid: {e}"))?;
        if entry_tag == tag {
            return Ok(tag);
        }
    }
    Err(format!(
        "language '{}' is not enabled in the Pocket AI speech pack",
        tag
    ))
}

/// Enforce the SPEECH_ARCHITECTURE invariant on the InBharat route: the
/// configured model family must actually serve the requested language
/// according to the shared provider table (`languages.v1.json`), independent
/// of what the pack manifest allowlists. The manifest allowlist decides what
/// the PACK permits; the provider table decides what the MODEL can truthfully
/// do — allowlisting a language in the manifest can never add coverage
/// (without this check, a manifest line allowlisting `as` would route
/// Assamese straight to Qwen3-ASR, which cannot serve it).
fn ensure_provider_coverage(
    task: &SpeechTaskConfig,
    task_kind: SpeechTask,
    tag: &LanguageTag,
) -> Result<(), String> {
    if tag.is_auto() {
        // `auto` is the ASR detect directive: the engine decides the language
        // at inference time, so there is no coverage to assert up front.
        return Ok(());
    }
    let Some(key) = resolve_provider_key(&task.family, task_kind) else {
        return Err(format!(
            "speech model family '{}' is not a known provider in the language table",
            task.family
        ));
    };
    if provider_serves(&key, task_kind, tag) {
        Ok(())
    } else {
        Err(format!(
            "the {} model '{}' does not serve language '{}' (provider language table)",
            match task_kind {
                SpeechTask::Asr => "ASR",
                SpeechTask::Tts => "TTS",
            },
            task.family,
            tag
        ))
    }
}

#[tauri::command]
pub fn get_bharat_audio_status(vault_root: String) -> BharatAudioStatus {
    status(&vault_root)
}

pub fn status(vault_root: &str) -> BharatAudioStatus {
    let streaming_class = unoone_speech_contracts::StreamingClass::BufferedFinal
        .as_str()
        .to_string();
    let Ok((root, manifest)) = read_manifest(vault_root) else {
        return BharatAudioStatus {
            configured: false,
            enabled: false,
            production_ready: false,
            reason: "SPEECH/config/inbharat-audio.v1.json not configured".to_string(),
            upstream_commit: None,
            asr_family: None,
            tts_family: None,
            inference_ready: false,
            streaming_class,
        };
    };
    // One preflight feeds both the surfaced runtime facts and the production
    // gate, so a status poll never spawns the readiness CLI twice — and the
    // CLI is only ever spawned after the package integrity gate AND the
    // acceptance hash verification of the executables passed.
    let (inference_ready, readiness) = match preflight_speech_gate(&root, &manifest) {
        Ok((_ibaudio_path, _audiocpp_path, _acceptance, runtime)) => {
            let ready = runtime.inference_ready;
            let gate = check_runtime_status(&manifest, &runtime).map(|_| runtime);
            (ready, gate)
        }
        Err(error) => (false, Err(error)),
    };
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
        inference_ready,
        streaming_class,
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

    let input = confine_audio_input(&root, audio_path)?;

    // Unique per-call transcript name: a millisecond timestamp collided for
    // concurrent requests, and the transcript is user speech — a collision
    // let one request read (and keep or leak) another's words.
    let transcript_rel = format!(
        "VAULT/recordings/transcripts/inbharat_asr_{}.txt",
        uuid::Uuid::new_v4().simple()
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
    let trimmed = language.trim();
    let (cli_language, language_tag) = if trimmed.is_empty() {
        // Pack default: the speech pack's own declared value. Canonicalized
        // for the result, but exempt from the user-facing allowlist check —
        // it is the pack speaking, not a user claim (this is what allows the
        // truthful "auto" detect directive as a default).
        let default = task
            .default_language
            .as_deref()
            .map(str::trim)
            .unwrap_or("");
        if default.is_empty() {
            return Err(
                "ASR language must be explicit or provided by the speech manifest".to_string(),
            );
        }
        let tag = unoone_speech_contracts::canonicalize(default)
            .map_err(|e| format!("speech manifest default language is invalid: {e}"))?;
        (default.to_string(), tag)
    } else {
        let tag = validate_language(&manifest, trimmed)?;
        (trimmed.to_string(), tag)
    };
    // Provider-table coverage is enforced on the InBharat route itself, not
    // only on the legacy fallback: the manifest allowlist alone must never be
    // enough to route a language to a model family that cannot serve it.
    ensure_provider_coverage(task, SpeechTask::Asr, &language_tag)?;
    cmd.arg("--language").arg(&cli_language);
    let transcript = {
        // The transcript file must be deleted on EVERY path — CLI failure,
        // unreadable output, or success. The old code returned early on the
        // CLI/read error paths and left the (possibly partial) user speech
        // file behind in the vault scratch area.
        let outcome = (|| -> Result<String, String> {
            run_command_timeout(cmd, INFERENCE_TIMEOUT)?;
            let transcript = std::fs::read_to_string(&transcript_path)
                .map_err(|e| {
                    format!("audio.cpp ASR did not produce its declared transcript file: {e}")
                })?
                .trim()
                .to_string();
            if transcript.is_empty() {
                return Err("audio.cpp ASR returned an empty transcript".to_string());
            }
            Ok(transcript)
        })();
        let _ = std::fs::remove_file(&transcript_path);
        outcome?
    };
    Ok(BharatAsrResult {
        transcript,
        language: language_tag.as_str().to_string(),
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
    // Synthesized speech is plaintext derived from user text. It previously
    // persisted unencrypted at VAULT/recordings/tts/ — plaintext inside the
    // encrypted-vault tree, never cleaned up. It now goes to a transient OS
    // temp area OUTSIDE the vault (the audio can still be played from the
    // returned path), and every call sweeps outputs older than the TTL so
    // nothing accumulates indefinitely.
    let output_dir = tts_output_dir()?;
    sweep_stale_tts_outputs(&output_dir);
    let output = output_dir.join(format!(
        // Unique per-call output name (see the ASR transcript note above).
        "inbharat_tts_{}.wav",
        uuid::Uuid::new_v4().simple()
    ));
    // Defensive: never write through a pre-existing path (a planted symlink
    // at our generated name would make the CLI write elsewhere).
    let _ = std::fs::remove_file(&output);

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
    let trimmed = language.trim();
    let (cli_language, language_tag) = if trimmed.is_empty() {
        // Pack default (see transcribe): canonicalized but exempt from the
        // user-facing allowlist check.
        let default = task
            .default_language
            .as_deref()
            .map(str::trim)
            .unwrap_or("");
        if default.is_empty() {
            return Err(
                "TTS language must be explicit or provided by the speech manifest".to_string(),
            );
        }
        let tag = unoone_speech_contracts::canonicalize(default)
            .map_err(|e| format!("speech manifest default language is invalid: {e}"))?;
        (default.to_string(), tag)
    } else {
        let tag = validate_language(&manifest, trimmed)?;
        (trimmed.to_string(), tag)
    };
    // Same InBharat-route coverage rule as transcription.
    ensure_provider_coverage(task, SpeechTask::Tts, &language_tag)?;
    cmd.arg("--language").arg(&cli_language);
    if let Err(error) = run_command_timeout(cmd, INFERENCE_TIMEOUT) {
        // A failed run may still have written a partial WAV — remove it so
        // the vault scratch area never accumulates broken output.
        let _ = std::fs::remove_file(&output);
        return Err(error);
    }
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

    fn runtime_readiness(adapter_compiled: bool, inference_ready: bool) -> AudioCppReadiness {
        AudioCppReadiness {
            schema: "inbharat.ibaudio.audio_cpp_status.v1".to_string(),
            adapter_compiled,
            inference_ready,
            reviewed_commit: "a".repeat(40),
            upstream_source: "test://audio.cpp".to_string(),
            reason: "test reason".to_string(),
        }
    }

    /// Req 12/15 regression: a compiled adapter whose local model assets are
    /// missing must NOT be production-ready. The runtime status API derives
    /// inference_ready from a real probe; the desktop gate must honor a false
    /// verdict regardless of the adapter having compiled in.
    #[test]
    fn compiled_adapter_with_missing_models_is_not_production_ready() {
        let manifest = language_manifest(&["en"]);
        let status = runtime_readiness(true, false);
        let error = check_runtime_status(&manifest, &status)
            .expect_err("compiled adapter without usable assets must fail the gate");
        assert!(error.contains("inference is not ready"), "got: {error}");
    }

    /// The inverse regression: inference_ready=1 with adapter_compiled=0 is
    /// compile-time truth again — impossible from the real runtime, but the
    /// gate must reject the combination defensively, and a non-compiled
    /// adapter must never pass.
    #[test]
    fn not_compiled_adapter_never_passes_even_if_marked_ready() {
        let manifest = language_manifest(&["en"]);
        let status = runtime_readiness(false, true);
        let error = check_runtime_status(&manifest, &status)
            .expect_err("adapter_compiled=0 must fail the gate");
        assert!(error.contains("was not built against"), "got: {error}");
    }

    /// A pin mismatch is rejected before any readiness verdict matters.
    #[test]
    fn commit_mismatch_is_rejected() {
        let manifest = language_manifest(&["en"]);
        let mut status = runtime_readiness(true, true);
        status.reviewed_commit = "b".repeat(40);
        let error = check_runtime_status(&manifest, &status)
            .expect_err("commit mismatch must fail the gate");
        assert!(error.contains("commit mismatch"), "got: {error}");
    }

    /// The 1 MiB read buffer in `sha256_file` MUST be heap allocated: a stack
    /// array overflows the 1 MiB default Windows thread stack. Run the hash
    /// on a thread with a deliberately small (128 KiB) stack over a 4 MiB
    /// file — with a stack buffer this aborts the process; with the heap
    /// buffer it returns a digest.
    #[test]
    fn sha256_file_runs_on_small_stack_thread() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("4mib.bin");
        let payload = vec![0xA5u8; 4 * 1024 * 1024];
        std::fs::write(&file, &payload).unwrap();
        let path = file.clone();
        let worker = std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || sha256_file(&path))
            .expect("spawn small-stack thread");
        let digest = worker
            .join()
            .expect("small-stack thread must not crash")
            .expect("hash must succeed");
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    fn language_manifest(languages: &[&str]) -> InBharatAudioManifest {
        InBharatAudioManifest {
            schema: "inbharat.pai.speech.v1".to_string(),
            enabled: true,
            upstream_commit: "a".repeat(40),
            backend: "cpu".to_string(),
            allowed_languages: languages.iter().map(|s| s.to_string()).collect(),
            asr: None,
            tts: None,
        }
    }

    /// The old `validate_language` silently passed on an empty request or an
    /// empty allowlist. Both must now fail closed.
    #[test]
    fn empty_language_and_empty_allowlist_fail_closed() {
        let manifest = language_manifest(&["en", "hi", "hinglish"]);
        assert!(validate_language(&manifest, "   ").is_err());
        assert!(validate_language(&manifest, "").is_err());

        let empty = language_manifest(&[]);
        assert!(validate_language(&empty, "hi").is_err());
    }

    /// BCP-47 canonicalization reaches the speech pack gate: `as`/`as-IN`
    /// address Assamese, `hi`/`hi-IN` address Hindi, and aliases match an
    /// allowlist written in either form.
    #[test]
    fn validate_language_canonicalizes_aliases() {
        let manifest = language_manifest(&["en", "hi", "hinglish"]);
        assert_eq!(
            validate_language(&manifest, "hi-IN").unwrap().as_str(),
            "hi-IN"
        );
        assert_eq!(
            validate_language(&manifest, "hi").unwrap().as_str(),
            "hi-IN"
        );
        assert_eq!(
            validate_language(&manifest, "HINGLISH").unwrap().as_str(),
            "hi-en-codemix"
        );
        // Global languages pass through and never join the pack.
        assert!(validate_language(&manifest, "fr").is_err());
        assert!(validate_language(&manifest, "en-US").is_err());
        // Assamese is never enabled by the Qwen3-era pack allowlist.
        assert!(validate_language(&manifest, "as").is_err());
        assert!(validate_language(&manifest, "as-IN").is_err());
        // Malformed tags are rejected, not guessed.
        assert!(validate_language(&manifest, "french--").is_err());
    }

    #[test]
    fn audio_input_is_confined_to_root_or_capture_temp() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();

        // Inside the root: allowed.
        let inside = root.join("capture.wav");
        std::fs::write(&inside, b"RIFF").unwrap();
        assert!(confine_audio_input(&root, inside.to_str().unwrap()).is_ok());

        // Inside the unoone-stt capture area: allowed (recording writes its
        // transient WAV captures there).
        let capture_dir = std::env::temp_dir().join("unoone-stt");
        std::fs::create_dir_all(&capture_dir).unwrap();
        let capture = capture_dir.join("unoone-stt-test-confine.wav");
        std::fs::write(&capture, b"RIFF").unwrap();
        assert!(confine_audio_input(&root, capture.to_str().unwrap()).is_ok());

        // Merely anywhere else in OS temp is NOT enough — the whole temp root
        // is untrusted scratch space.
        let loose_temp = tempfile::tempdir().unwrap();
        let loose = loose_temp.path().join("loose.wav");
        std::fs::write(&loose, b"RIFF").unwrap();
        assert!(
            confine_audio_input(&root, loose.to_str().unwrap()).is_err(),
            "a WAV outside the root and outside unoone-stt must be rejected"
        );

        // Missing file: rejected.
        assert!(confine_audio_input(&root, "Z:\\does\\not\\exist.wav").is_err());
    }

    /// The acceptance-gate hash memo: an unchanged file (same size + mtime)
    /// serves the memoized digest, and any change to the file is a cache miss
    /// that re-hashes for real. This is what stops every speech request from
    /// re-reading 2.5 GB of model trees.
    #[test]
    fn hash_memo_serves_unchanged_files_and_invalidates_on_change() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("memo-model.bin");
        std::fs::write(&file, b"first payload").unwrap();
        let first = sha256_file_memoized(&file).expect("first hash");
        let second = sha256_file_memoized(&file).expect("memoized hash");
        assert_eq!(
            first, second,
            "unchanged file must serve the memoized digest"
        );

        // A size change (any real modification) must be a cache miss.
        std::fs::write(&file, b"second payload, different length").unwrap();
        let third = sha256_file_memoized(&file).expect("re-hash after change");
        assert_ne!(first, third, "changed file must be re-hashed");
        assert_eq!(
            third,
            sha256_file(&file).expect("direct hash"),
            "memoized and direct digests must agree"
        );
    }

    fn speech_task_config(family: &str) -> SpeechTaskConfig {
        SpeechTaskConfig {
            family: family.to_owned(),
            model_relative_path: "SPEECH/models/x.gguf".to_owned(),
            default_language: None,
        }
    }

    /// The InBharat route enforces the provider table, not just the manifest
    /// allowlist: a manifest line allowlisting Assamese can never route it to
    /// Qwen3-ASR, and the production `omnivoice` TTS family resolves to its
    /// table key and serves exactly what the table says.
    #[test]
    fn provider_coverage_is_enforced_on_the_inbharat_route() {
        let hindi = unoone_speech_contracts::canonicalize("hi").unwrap();
        let assamese = unoone_speech_contracts::canonicalize("as").unwrap();

        let qwen3 = speech_task_config("qwen3_asr");
        assert!(ensure_provider_coverage(&qwen3, SpeechTask::Asr, &hindi).is_ok());
        assert!(ensure_provider_coverage(&qwen3, SpeechTask::Asr, &assamese).is_err());

        // The manifest's bare "omnivoice" family must resolve to omnivoice_tts.
        let omnivoice = speech_task_config("omnivoice");
        assert!(ensure_provider_coverage(&omnivoice, SpeechTask::Tts, &hindi).is_ok());
        let english = unoone_speech_contracts::canonicalize("en").unwrap();
        assert!(ensure_provider_coverage(&omnivoice, SpeechTask::Tts, &english).is_ok());
        assert!(ensure_provider_coverage(&omnivoice, SpeechTask::Tts, &assamese).is_err());
        // OmniVoice does not do ASR at all.
        assert!(ensure_provider_coverage(&omnivoice, SpeechTask::Asr, &hindi).is_err());

        // An unknown family fails closed with zero coverage.
        let unknown = speech_task_config("does-not-exist");
        assert!(ensure_provider_coverage(&unknown, SpeechTask::Tts, &hindi).is_err());

        // The `auto` ASR detect directive is exempt (the engine decides).
        let auto = unoone_speech_contracts::canonicalize("auto").unwrap();
        assert!(ensure_provider_coverage(&qwen3, SpeechTask::Asr, &auto).is_ok());
    }

    /// TTS outputs are transient: they live outside the vault tree, and the
    /// sweep removes only `inbharat_tts_*` files whose age exceeds the TTL —
    /// fresh outputs and unrelated files are never touched.
    #[test]
    fn tts_sweep_removes_only_stale_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("inbharat_tts_current.wav");
        std::fs::write(&output, b"RIFF").unwrap();
        let unrelated = dir.path().join("someone-elses.wav");
        std::fs::write(&unrelated, b"RIFF").unwrap();

        // A just-written output is younger than the real TTL and survives.
        sweep_tts_outputs_older_than(dir.path(), TTS_OUTPUT_TTL);
        assert!(output.exists(), "fresh output must survive the real TTL");
        assert!(unrelated.exists(), "unrelated files must never be touched");

        // A zero TTL treats everything as stale — but still only our prefix.
        sweep_tts_outputs_older_than(dir.path(), Duration::ZERO);
        assert!(!output.exists(), "stale output must be swept");
        assert!(unrelated.exists(), "unrelated files must never be touched");
    }
}
