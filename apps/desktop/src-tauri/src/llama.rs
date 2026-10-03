// UnoOne Power — Desktop Model Manager
// Manages Gemma 4 12B Q4 GGUF model via llama.cpp

use serde::{Deserialize, Serialize};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Model configuration for Gemma 4 12B
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model_path: String,
    pub context_size: u32,
    pub batch_size: u32,
    pub threads: u32,
    pub gpu_layers: i32, // -1 = all, 0 = CPU only
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub repeat_penalty: f32,
    pub max_tokens: u32,
    /// Path to the multimodal projector (mmproj) model file for vision/OCR
    pub mmproj_path: Option<String>,
    /// KV-cache quantization for K (f16 default; q8_0/q4_0 shrink the cache
    /// so large contexts fit low-VRAM hosts). Host-adaptive: the shipped
    /// llama-server b10075 supports `-ctk`.
    #[serde(default)]
    pub cache_type_k: Option<String>,
    /// KV-cache quantization for V. Non-f16 V cache requires flash
    /// attention, which llama-server b10075 enables via `-fa auto` by
    /// default.
    #[serde(default)]
    pub cache_type_v: Option<String>,
    /// Force flash attention on/off. None = the server default (auto).
    #[serde(default)]
    pub flash_attention: Option<bool>,
}

impl ModelConfig {
    /// Only the KV-cache types the shipped llama-server build accepts.
    fn valid_cache_type(value: &str) -> bool {
        matches!(value, "f16" | "q8_0" | "q4_0" | "bf16")
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            model_path: String::new(),
            context_size: 4096,
            batch_size: 512,
            threads: 0,     // 0 = auto-detect
            gpu_layers: -1, // -1 = offload all layers
            temperature: 0.7,
            top_p: 0.9,
            top_k: 40,
            repeat_penalty: 1.1,
            max_tokens: 4096,
            mmproj_path: None,
            cache_type_k: None,
            cache_type_v: None,
            flash_attention: None,
        }
    }
}

/// Total physical RAM in GiB, best-effort. None = unknown host (keep the
/// safest baseline config).
fn detected_ram_gib() -> Option<u32> {
    #[cfg(target_os = "windows")]
    {
        #[repr(C)]
        struct MemoryStatusEx {
            dw_length: u32,
            dw_memory_load: u32,
            ull_total_phys: u64,
            ull_avail_phys: u64,
            ull_total_page_file: u64,
            ull_avail_page_file: u64,
            ull_total_virtual: u64,
            ull_avail_virtual: u64,
            ull_avail_extended_virtual: u64,
        }
        extern "system" {
            fn GlobalMemoryStatusEx(lp_buffer: *mut MemoryStatusEx) -> i32;
        }
        let mut status = MemoryStatusEx {
            dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
            dw_memory_load: 0,
            ull_total_phys: 0,
            ull_avail_phys: 0,
            ull_total_page_file: 0,
            ull_avail_page_file: 0,
            ull_total_virtual: 0,
            ull_avail_virtual: 0,
            ull_avail_extended_virtual: 0,
        };
        // SAFETY: the struct is a plain C POD initialized with the correct
        // byte length; the FFI fills exactly that struct.
        if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
            return u32::try_from(status.ull_total_phys / (1024 * 1024 * 1024)).ok();
        }
        None
    }
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = meminfo
            .lines()
            .find(|line| line.starts_with("MemTotal:"))
            .and_then(|line| line.split_whitespace().nth(1).and_then(|v| v.parse().ok()))?;
        u32::try_from(kb / (1024 * 1024)).ok()
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .arg("-n")
            .arg("hw.memsize")
            .output()
            .ok()?;
        let bytes: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
        u32::try_from(bytes / (1024 * 1024 * 1024)).ok()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    None
}

/// Model-identity strictness for the running package.
///
/// `Strict` requires the manifest to declare a SHA-256 for the model and
/// enforces it; a missing declared hash is itself a failure. This is the
/// correct posture for a package whose trust model is "verify every asset",
/// and the schema-v2 manifest declares a hash for every asset, so the normal
/// path is unaffected — only substitution and hashless packages are newly
/// rejected. If a specific prototype drive ships without model hashes, flip
/// this to `PrototypeAllowMissingHash` (documented in runtime-select).
const MODEL_IDENTITY_POLICY: unoone_runtime_select::IdentityPolicy =
    unoone_runtime_select::IdentityPolicy::Strict;

/// Map the runtime-select backend enum to the desktop wire enum.
///
/// The two enums are kept separate so runtime-select carries no desktop /
/// serde dependency; this is the single conversion point.
fn map_backend(b: unoone_runtime_select::Backend) -> AccelerationBackend {
    match b {
        unoone_runtime_select::Backend::Cuda => AccelerationBackend::Cuda,
        unoone_runtime_select::Backend::Metal => AccelerationBackend::Metal,
        unoone_runtime_select::Backend::Vulkan => AccelerationBackend::Vulkan,
        unoone_runtime_select::Backend::Cpu => AccelerationBackend::Cpu,
    }
}

/// Hardware-acceleration backend
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AccelerationBackend {
    Cuda,
    Metal,
    Vulkan,
    Cpu,
}

/// Model loading status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ModelStatus {
    NotLoaded,
    Loading,
    Loaded,
    Generating,
    Error,
}

/// Inference request — D1: used by the agentic loop to send completions to llama-server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceRequest {
    pub prompt: String,
    pub system_prompt: Option<String>,
    pub conversation_history: Vec<ConversationTurn>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub stop_sequences: Option<Vec<String>>,
    pub tools: Option<Vec<ToolDefinition>>,
    /// Gap 2b/4 (2026-09-16): when Some(true), the completion is sent with
    /// `chat_template_kwargs: {"enable_thinking": false}` + `reasoning_budget: 0`
    /// so Gemma 4 answers directly instead of emitting chain-of-thought first.
    /// Measured live on the staged drive (A/B, same describe prompt, same
    /// server): think-on 376 completion tokens / 1181-char reasoning_content /
    /// 44.6 s; think-off 55 tokens / zero reasoning / 5.7 s — an ~8x latency
    /// win with comparable answer length. Set by the latency-critical vision
    /// lanes (OCR, blind-aid describe); None keeps the model's default
    /// behavior (the agent loop reasons, as before).
    pub disable_reasoning: Option<bool>,
    /// Universal-adaptive context (2026-10-02): the granted context window
    /// for THIS session (from gguf_meta::derive_context_budget, stored when
    /// the server started). When set, send_completion trims the oldest
    /// conversation turns to keep prompt + response reserve inside the
    /// window — the alternative was a request that silently overflows and
    /// fails mid-session. None = no trimming (single-shot lanes).
    #[serde(default)]
    pub context_budget: Option<u32>,
}

/// Item 24 — token-accurate truncation priorities for the chat history.
///
/// Conservative token estimate: bytes/3 overestimates English (~4 B/token)
/// and is close for Devanagari (~3 B/code point), so trimming fires a little
/// early rather than after the server has already rejected the prompt.
pub(crate) fn estimated_tokens(text: &str) -> usize {
    text.len() / 3 + 1
}

/// Tokens a serialized turn is assumed to cost (role + tool-call JSON adds a
/// small fixed overhead on top of the content itself).
fn turn_tokens(turn: &ConversationTurn) -> usize {
    let content = match &turn.content {
        Content::Text(text) => estimated_tokens(text),
        // Multimodal turns carry a base64 image — the vision encoder's token
        // budget is dominated by image patches; charge the textual parts and
        // a flat image cost.
        Content::Multimodal(parts) => parts
            .iter()
            .map(|p| match p {
                ContentPart::text { text } => estimated_tokens(text),
                ContentPart::image_url { .. } => 1_024,
            })
            .sum(),
    };
    let tool_overhead = turn
        .tool_calls
        .as_ref()
        .map(|calls| {
            calls
                .iter()
                .map(|c| estimated_tokens(&c.name) + estimated_tokens(&c.arguments.to_string()))
                .sum()
        })
        .unwrap_or(0);
    content + tool_overhead + 8
}

/// Trim the oldest turns so prompt estimate + response reserve fit the
/// granted window. Priorities, newest-last: the system prompt and the LAST
/// user turn are never dropped (the current task must always reach the
/// model); older turns go first. A turn that alone exceeds the window is
/// kept anyway — the server's error is the honest outcome, silently
/// dropping the task the user just asked for is not.
///
/// Returns the trimmed history and how many turns were dropped.
pub(crate) fn trim_history_to_budget(
    system_prompt: Option<&str>,
    history: &[ConversationTurn],
    tools: Option<&Vec<ToolDefinition>>,
    context_budget: u32,
    max_tokens: u32,
) -> (Vec<ConversationTurn>, u32) {
    let reserve = max_tokens.max(256) + 512; // response + chat-template/tool overhead
    let mut budget = context_budget.saturating_sub(reserve) as usize;

    let mut fixed = 0usize;
    if let Some(sys) = system_prompt {
        fixed += estimated_tokens(sys);
    }
    if let Some(tools) = tools {
        let serialized = serde_json::to_string(tools).unwrap_or_default();
        fixed += estimated_tokens(&serialized);
    }
    budget = budget.saturating_sub(fixed);

    let cost: Vec<usize> = history.iter().map(turn_tokens).collect();
    let total: usize = cost.iter().sum();

    // Everything fits: no trimming, no notice.
    if total <= budget || history.is_empty() {
        return (history.to_vec(), 0);
    }

    // Index of the last user turn — protected.
    let last_user = history
        .iter()
        .rposition(|t| t.role == "user")
        .unwrap_or(history.len() - 1);

    let mut dropped: u32 = 0;
    let mut remaining = total;
    let mut keep = vec![true; history.len()];
    for i in 0..history.len() {
        if remaining <= budget {
            break;
        }
        if i == last_user {
            continue; // never drop the current task
        }
        keep[i] = false;
        remaining = remaining.saturating_sub(cost[i]);
        dropped += 1;
    }

    let trimmed: Vec<ConversationTurn> = history
        .iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, t)| t.clone())
        .collect();
    (trimmed, dropped)
}

/// Conversation turn — extended with multimodal content support for vision/OCR
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub role: String, // "user", "assistant", or "tool"
    pub content: Content,
    /// For assistant turns with tool calls, the OpenAI-format tool_calls array
    pub tool_calls: Option<Vec<ToolCallResult>>,
    /// For tool role turns, the ID of the tool call this responds to
    pub tool_call_id: Option<String>,
}

/// Content can be plain text or an array of multimodal parts (text + images)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    /// Plain text content (backward compatible with existing messages)
    Text(String),
    /// Multimodal content array (text + image_url parts)
    Multimodal(Vec<ContentPart>),
}

impl Default for Content {
    fn default() -> Self {
        Content::Text(String::new())
    }
}

impl Content {
    /// Create plain text content
    pub fn text(s: impl Into<String>) -> Self {
        Content::Text(s.into())
    }

    /// Create multimodal content with an image
    pub fn with_image(prompt: &str, image_base64: &str, mime_type: &str) -> Self {
        Content::Multimodal(vec![
            ContentPart::text {
                text: prompt.to_string(),
            },
            ContentPart::image_url {
                image_url: ImageUrl {
                    url: format!("data:{};base64,{}", mime_type, image_base64),
                },
            },
        ])
    }
}

/// A single part of multimodal content (OpenAI format)
/// Serializes as {"type": "text", "text": "..."} or {"type": "image_url", "image_url": {"url": "..."}}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[allow(non_camel_case_types)]
pub enum ContentPart {
    text { text: String },
    image_url { image_url: ImageUrl },
}

/// Image URL with data URI support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String, // data:image/png;base64,... or https://...
}

/// D1: Tool definition for OpenAI-compatible function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
}

/// D1: Parsed tool call from model output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Inference response — D1: extended with tool_calls and finish_reason
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceResponse {
    pub text: String,
    pub tokens_generated: u32,
    pub tokens_per_second: f32,
    pub model_id: String,
    pub tool_calls: Option<Vec<ToolCallResult>>,
    pub finish_reason: Option<String>, // "stop", "tool_calls", "length"
    /// Gap 4 (2026-09-16): the model's chain-of-thought when the template
    /// emits it into `reasoning_content` (Gemma 4 does unless think-off is
    /// requested). Callers use it to tell the honest "empty visible reply"
    /// case (defect #44: reasoning consumed the whole budget) apart from a
    /// model that said nothing at all. None when the server reported none.
    #[serde(default)]
    pub reasoning: Option<String>,
    /// Gap 3 (2026-09-16): how many prompt tokens the server served from its
    /// prefix cache (usage.prompt_tokens_details.cached_tokens). Measured
    /// live: llama-server DOES cache across requests (second same-prefix
    /// request served cached_tokens=91 of its prompt) — surfacing it lets
    /// callers verify the cache is actually being hit instead of assuming.
    #[serde(default)]
    pub cached_prompt_tokens: Option<u32>,
    /// Item 24 (2026-10-02): set when send_completion had to drop older
    /// conversation turns to fit the granted context window — "context note:
    /// trimmed N oldest turns to fit 32,768-token budget". None = the full
    /// history was sent. Rendered by the UI so trimming is visible, not
    /// silent memory loss.
    #[serde(default)]
    pub context_note: Option<String>,
}

/// Verified server identity returned after a successful start.
#[derive(Debug, Clone)]
pub struct ServerIdentity {
    pub pid: u32,
    pub model_id: String,
}

/// Model info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub model_type: String,
    pub quantization: String,
    pub file_size_gb: f64,
    pub context_length: u32,
    /// True only when `context_length` was read from the artifact's own GGUF
    /// metadata; false means it is the conservative declared default, not a
    /// verified trained-context claim.
    pub context_verified: bool,
    pub available: bool,
    pub path: String,
    pub mmproj_path: Option<String>,
}

/// Native context of a model artifact, read from the GGUF header when the
/// file parses. Unreadable/absent metadata yields the conservative default
/// marked unverified — never a guess presented as the model's trained limit.
pub(crate) fn native_context_or_default(path: &std::path::Path) -> (u32, bool) {
    match crate::gguf_meta::read_metadata(path) {
        Ok(meta) => match meta.context_length {
            Some(ctx) => (ctx, true),
            None => (8192, false),
        },
        Err(_) => (8192, false),
    }
}

/// Model manager state
pub struct ModelManager {
    status: Mutex<ModelStatus>,
    backend: Mutex<AccelerationBackend>,
    llama_process: Mutex<Option<std::process::Child>>,
    /// Verified identity of the running llama-server process (PID, port, model id).
    server_identity: Mutex<Option<ServerIdentity>>,
    /// Item 24 (2026-10-02): the granted context window of the running
    /// server (clamped by artifact + host RAM in start_server). Agent/chat
    /// requests read this so their prompts are trimmed to the real window
    /// — with None (no server) no trimming is attempted.
    granted_context: Mutex<Option<u32>>,
}

/// Kill an uncommitted inference child even if its loading future is dropped
/// during application shutdown or an IPC/task failure.
struct StartingModelChild(Option<std::process::Child>);
impl std::ops::Deref for StartingModelChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("starting child")
    }
}
impl std::ops::DerefMut for StartingModelChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("starting child")
    }
}
impl Drop for StartingModelChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Drop for ModelManager {
    fn drop(&mut self) {
        if let Some(mut child) = self
            .llama_process
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl ModelManager {
    pub fn new() -> Self {
        Self {
            status: Mutex::new(ModelStatus::NotLoaded),
            backend: Mutex::new(AccelerationBackend::Cpu),
            llama_process: Mutex::new(None),
            server_identity: Mutex::new(None),
            granted_context: Mutex::new(None),
        }
    }

    /// Granted context window of the running server, for request trimming.
    pub fn granted_context(&self) -> Option<u32> {
        *self.granted_context.lock().unwrap()
    }

    /// Find a free localhost TCP port by binding a temporary socket to port 0.
    fn find_free_port() -> Result<u16, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("Failed to find a free localhost port: {}", e))?;
        listener
            .local_addr()
            .map(|a| a.port())
            .map_err(|e| format!("Failed to read temporary listener address: {}", e))
    }

    /// Compute the SHA-256 hex digest of a file.
    #[cfg(test)]
    pub(crate) fn sha256_file(path: &std::path::Path) -> Result<String, String> {
        Self::sha256_file_cancellable(path, None)
    }

    fn sha256_file_cancellable(
        path: &std::path::Path,
        cancel: Option<&inbharat_harness_core::CancellationToken>,
    ) -> Result<String, String> {
        use sha2::Digest;
        use std::io::Read;
        let mut file = std::fs::File::open(path)
            .map_err(|e| format!("Failed to open {} for hashing: {}", path.display(), e))?;
        let mut hasher = sha2::Sha256::new();
        // Heap buffer, 512 KiB: hashed files are multi-GB models on removable
        // media, where 8 KiB reads turn one hash into ~1M syscalls. Heap, not
        // stack, mirrors the bharat_audio.rs hardening so deep call stacks
        // cannot overflow.
        let mut buffer = vec![0u8; 512 * 1024];
        loop {
            if cancel.is_some_and(|token| token.is_cancelled()) {
                return Err("Model startup cancelled".to_owned());
            }
            let n = file
                .read(&mut buffer)
                .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        Ok(hex::encode(hasher.finalize()))
    }

    /// Strip Windows UNC prefix (`\\?\`) so canonical and non-canonical paths
    /// can be compared component-by-component.
    fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
        let s = path.to_string_lossy();
        let stripped = s.strip_prefix(r"\\?\").unwrap_or(&s);
        std::path::PathBuf::from(stripped)
    }

    /// Read the expected SHA-256 hash for a model path from the USB manifest.
    /// Accepts either the relative manifest path or an absolute on-disk path.
    pub(crate) fn read_manifest_model_hash(vault_root: &str, model_path: &str) -> Option<String> {
        // Host-disk model cache entries are named <manifest-sha256>.gguf and
        // are only published after the streamed copy is digest-verified, so
        // for a cache path the filename IS the manifest-expected hash. The
        // startup disk hash still has to match it (Strict policy), so a
        // tampered cache copy is refused exactly like a tampered drive copy.
        if let Ok(cache_dir) = model_cache_dir() {
            let path = PathBuf::from(model_path);
            if path.starts_with(&cache_dir) {
                let name = path.file_name()?.to_str()?;
                let hex = name.strip_suffix(".gguf")?;
                if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Some(hex.to_ascii_lowercase());
                }
                return None;
            }
        }
        let manifest_path = PathBuf::from(vault_root).join("manifest.json");
        let manifest_content = std::fs::read_to_string(&manifest_path).ok()?;
        if let Ok(manifest) =
            serde_json::from_str::<unoone_usb_manifest::PocketManifest>(&manifest_content)
        {
            let vault_root_path = PathBuf::from(vault_root);
            let requested_path = PathBuf::from(model_path);
            let requested_path = if requested_path.is_absolute() {
                requested_path
            } else {
                vault_root_path.join(requested_path)
            };
            let requested = Self::normalize_path(
                &std::fs::canonicalize(&requested_path).unwrap_or(requested_path),
            );
            for asset in manifest
                .platforms
                .windows
                .models
                .iter()
                .filter(|asset| asset.kind == unoone_usb_manifest::AssetKind::Model)
            {
                let full = vault_root_path.join(&asset.path);
                let full = Self::normalize_path(&std::fs::canonicalize(&full).unwrap_or(full));
                if full == requested {
                    return Some(asset.sha256.clone());
                }
            }
            // Speech models (SPEECH/models/…) get the same digest-bound
            // treatment: the speech plane serves ASR/TTS inference from the
            // host cache too, and staging needs the manifest sha256 here.
            if let Some(speech) = manifest.platforms.windows.speech.as_ref() {
                for asset in speech
                    .models
                    .iter()
                    .filter(|asset| asset.kind == unoone_usb_manifest::AssetKind::SpeechModel)
                {
                    let full = vault_root_path.join(&asset.path);
                    let full = Self::normalize_path(&std::fs::canonicalize(&full).unwrap_or(full));
                    if full == requested {
                        return Some(asset.sha256.clone());
                    }
                }
            }
        }
        let manifest: serde_json::Value = serde_json::from_str(&manifest_content).ok()?;
        let models = manifest.get("models")?;

        // Normalize the requested path so absolute paths can match manifest entries,
        // even when canonicalize returns a `\\?\` UNC prefix or the target does not exist.
        let vault_root_raw = PathBuf::from(vault_root);
        let model_path_raw = PathBuf::from(model_path);
        let requested_raw = if model_path_raw.is_absolute() {
            model_path_raw
        } else {
            vault_root_raw.join(model_path_raw)
        };
        let abs_model =
            std::fs::canonicalize(&requested_raw).unwrap_or_else(|_| requested_raw.clone());
        let vault_root_abs =
            std::fs::canonicalize(&vault_root_raw).unwrap_or_else(|_| vault_root_raw.clone());
        let abs_model_norm = Self::normalize_path(&abs_model);
        let requested_raw_norm = Self::normalize_path(&requested_raw);

        // Also resolve model_path against the vault root in case it is a relative
        // manifest entry and the file may not exist yet.
        let vault_relative = vault_root_raw.join(model_path);
        let vault_relative_norm = Self::normalize_path(&vault_relative);

        for section in ["desktop", "mobile"] {
            if let Some(obj) = models.get(section).and_then(|v| v.as_object()) {
                for (_key, model) in obj {
                    let entry_path = model.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    let entry_full_raw = vault_root_raw.join(entry_path);
                    let entry_full_raw_norm = Self::normalize_path(&entry_full_raw);
                    let entry_full = std::fs::canonicalize(&entry_full_raw)
                        .unwrap_or_else(|_| vault_root_abs.join(entry_path));
                    let entry_full_norm = Self::normalize_path(&entry_full);
                    if entry_path == model_path
                        || entry_full_norm == abs_model_norm
                        || entry_full_raw_norm == requested_raw_norm
                        || entry_full_norm == vault_relative_norm
                    {
                        return model
                            .get("sha256")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                    }
                }
            }
        }
        None
    }

    /// Verify that the server is healthy and loaded the expected model.
    /// Returns the verified identity on success, or an error string on failure.
    async fn verify_server_identity(
        port: u16,
        model_path: &str,
        vault_root: &str,
        disk_sha256: Option<String>,
    ) -> Result<ServerIdentity, String> {
        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{}", port);

        // 1. /health must succeed.
        let health = client
            .get(format!("{}/health", base))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| format!("Health request failed: {}", e))?;
        if !health.status().is_success() {
            return Err(format!("Health endpoint returned {}", health.status()));
        }
        // Parsing the body validates the server answers with well-formed
        // JSON; the payload itself carries no identity we consume.
        health
            .json::<serde_json::Value>()
            .await
            .map_err(|e| format!("Failed to parse health response: {}", e))?;

        // 2. /v1/models must list the expected model.
        let models_resp = client
            .get(format!("{}/v1/models", base))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| format!("Models request failed: {}", e))?;
        if !models_resp.status().is_success() {
            return Err(format!("Models endpoint returned {}", models_resp.status()));
        }
        let models_body: serde_json::Value = models_resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse models response: {}", e))?;

        let model_id = models_body
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|arr| arr.first())
            .and_then(|m| m.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // 3. Verify model identity via unoone_runtime_select.
        //
        // The manifest SHA-256 is compared case-insensitively. Under MODEL_IDENTITY_POLICY
        // a declared hash is enforced; the previous code SKIPPED the check when
        // the manifest carried no hash, so a substituted model passed silently.
        // The disk hash is computed once by start_server BEFORE the server
        // spawns, so a tampered model never gets an inference process. Do not
        // re-hash here: the poll loop would re-read the multi-GB model off
        // removable media on every attempt while it is already loaded.
        let disk_hash = disk_sha256;
        let expected_hash = Self::read_manifest_model_hash(vault_root, model_path);
        unoone_runtime_select::verify_model_identity(
            &unoone_runtime_select::ModelIdentityFacts {
                reported_model_id: &model_id,
                disk_sha256: disk_hash.as_deref(),
                manifest_sha256: expected_hash.as_deref(),
            },
            MODEL_IDENTITY_POLICY,
        )
        .map_err(|e| e.to_string())?;

        Ok(ServerIdentity {
            pid: 0, // Filled in by start_server after spawn.
            model_id,
        })
    }

    /// Reset internal state to Error after a failed start and kill any child.
    fn reset_to_error(&self, child: &mut std::process::Child, reason: String) -> String {
        let _ = child.kill();
        let _ = child.wait();
        *self.llama_process.lock().unwrap() = None;
        *self.status.lock().unwrap() = ModelStatus::Error;
        reason
    }

    /// Detect available acceleration backends, in descending preference order.
    ///
    /// Ordering is decided by `unoone_runtime_select`, not by probe order. The
    /// previous implementation used `insert(0)` per probe, so a machine with
    /// both CUDA and Vulkan ranked Vulkan first merely because its probe ran
    /// last — and the caller takes the first entry, so it selected the slower
    /// backend on NVIDIA hardware.
    pub fn detect_backends(&self) -> Vec<AccelerationBackend> {
        let available = unoone_runtime_select::AvailableBackends {
            cuda: self.check_cuda(),
            metal: cfg!(target_os = "macos") && self.check_metal(),
            vulkan: self.check_vulkan(),
        };
        unoone_runtime_select::ranked_backends(available)
            .into_iter()
            .map(map_backend)
            .collect()
    }

    fn check_cuda(&self) -> bool {
        // Check for CUDA by trying to find nvcuda.dll (Windows) or libcuda.so (Linux)
        if cfg!(target_os = "windows") {
            std::path::Path::new("C:\\Windows\\System32\\nvcuda.dll").exists()
                || std::path::Path::new("C:\\Windows\\System32\\nvcuda64.dll").exists()
        } else if cfg!(target_os = "linux") {
            std::path::Path::new("/usr/lib/x86_64-linux-gnu/libcuda.so").exists()
                || std::path::Path::new("/usr/lib/libcuda.so").exists()
        } else {
            false
        }
    }

    fn check_metal(&self) -> bool {
        // Metal is always available on macOS
        cfg!(target_os = "macos")
    }

    fn check_vulkan(&self) -> bool {
        // Check for Vulkan runtime
        if cfg!(target_os = "windows") {
            std::path::Path::new("C:\\Windows\\System32\\vulkan-1.dll").exists()
        } else if cfg!(target_os = "linux") {
            std::path::Path::new("/usr/lib/x86_64-linux-gnu/libvulkan.so").exists()
                || std::path::Path::new("/usr/lib/libvulkan.so").exists()
        } else {
            false
        }
    }

    /// Find GGUF model files in the MODELS directory
    /// Uses manifest-based discovery: reads manifest.json for model metadata,
    /// then scans MODELS/DESKTOP/ and MODELS/MOBILE/ directories
    pub fn find_models(&self, vault_root: &str) -> Vec<ModelInfo> {
        let mut models = Vec::new();

        // Try manifest-based discovery first
        let manifest_path = PathBuf::from(vault_root).join("manifest.json");
        if let Ok(manifest_content) = std::fs::read_to_string(&manifest_path) {
            if let Ok(manifest) =
                serde_json::from_str::<unoone_usb_manifest::PocketManifest>(&manifest_content)
            {
                for model in manifest
                    .platforms
                    .windows
                    .models
                    .iter()
                    .filter(|asset| asset.kind == unoone_usb_manifest::AssetKind::Model)
                {
                    let full_path = PathBuf::from(vault_root).join(&model.path);
                    let tier = crate::desktop_model_policy::tier(&model.id);
                    // A projector must identify the same tier. Never attach a 12B
                    // projector to a smaller text model just because it is first.
                    let mmproj_path = manifest
                        .platforms
                        .windows
                        .models
                        .iter()
                        .find(|asset| {
                            asset.kind == unoone_usb_manifest::AssetKind::Mmproj
                                && tier.is_some()
                                && crate::desktop_model_policy::tier(&format!(
                                    "{} {}",
                                    asset.id, asset.path
                                )) == tier
                        })
                        .map(|asset| {
                            PathBuf::from(vault_root)
                                .join(&asset.path)
                                .to_string_lossy()
                                .into_owned()
                        });
                    let (native_context, context_verified) = native_context_or_default(&full_path);
                    models.push(ModelInfo {
                        name: model.id.clone(),
                        model_type: model.id.clone(),
                        quantization: "manifest-verified".to_string(),
                        file_size_gb: model.size_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                        context_length: native_context,
                        context_verified,
                        available: full_path.is_file(),
                        path: full_path.to_string_lossy().to_string(),
                        mmproj_path: mmproj_path.clone(),
                    });
                }
                return models;
            }
            if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&manifest_content) {
                // Read desktop models from manifest
                if let Some(desktop) = manifest.get("models").and_then(|m| m.get("desktop")) {
                    if let Some(obj) = desktop.as_object() {
                        for (_key, model) in obj {
                            let model_path =
                                model.get("path").and_then(|v| v.as_str()).unwrap_or("");
                            let full_path = PathBuf::from(vault_root).join(model_path);

                            if full_path.exists() {
                                let name = model
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown Model")
                                    .to_string();
                                let file_size = std::fs::metadata(&full_path)
                                    .map(|m| m.len() as f64 / (1024.0 * 1024.0 * 1024.0))
                                    .unwrap_or(0.0);
                                let (native_context, context_verified) =
                                    native_context_or_default(&full_path);

                                models.push(ModelInfo {
                                    name,
                                    model_type: model
                                        .get("architecture")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown")
                                        .to_string(),
                                    quantization: model
                                        .get("quantisation")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown")
                                        .to_string(),
                                    file_size_gb: file_size,
                                    context_length: native_context,
                                    context_verified,
                                    available: true,
                                    path: full_path.to_string_lossy().to_string(),
                                    mmproj_path: model
                                        .get("mmproj_path")
                                        .and_then(|value| value.as_str())
                                        .map(|path| {
                                            PathBuf::from(vault_root)
                                                .join(path)
                                                .to_string_lossy()
                                                .to_string()
                                        }),
                                });
                            }
                        }
                    }
                }

                // Read mobile models from manifest
                if let Some(mobile) = manifest.get("models").and_then(|m| m.get("mobile")) {
                    if let Some(obj) = mobile.as_object() {
                        for (_key, model) in obj {
                            let model_path =
                                model.get("path").and_then(|v| v.as_str()).unwrap_or("");
                            let full_path = PathBuf::from(vault_root).join(model_path);

                            if full_path.exists() {
                                let file_size = std::fs::metadata(&full_path)
                                    .map(|m| m.len() as f64 / (1024.0 * 1024.0 * 1024.0))
                                    .unwrap_or(0.0);

                                let (native_context, context_verified) =
                                    native_context_or_default(&full_path);
                                models.push(ModelInfo {
                                    name: model
                                        .get("name")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("Unknown Mobile Model")
                                        .to_string(),
                                    model_type: model
                                        .get("architecture")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown")
                                        .to_string(),
                                    quantization: model
                                        .get("quantisation")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown")
                                        .to_string(),
                                    file_size_gb: file_size,
                                    context_length: native_context,
                                    context_verified,
                                    available: true,
                                    path: full_path.to_string_lossy().to_string(),
                                    mmproj_path: None,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Fallback: scan directories directly if manifest parsing fails
        if models.is_empty() {
            // Desktop models (Gemma 12B)
            let desktop_dir = PathBuf::from(vault_root)
                .join("MODELS")
                .join("DESKTOP")
                .join("Gemma-12B");
            if desktop_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&desktop_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if name.ends_with(".gguf") && !name.contains("mmproj") {
                                let file_size = std::fs::metadata(&path)
                                    .map(|m| m.len() as f64 / (1024.0 * 1024.0 * 1024.0))
                                    .unwrap_or(0.0);
                                let (native_context, context_verified) =
                                    native_context_or_default(&path);
                                models.push(ModelInfo {
                                    name: "Gemma 4 12B Q4_K_M".to_string(),
                                    model_type: "gemma-4-12b".to_string(),
                                    quantization: "Q4_K_M".to_string(),
                                    file_size_gb: file_size,
                                    context_length: native_context,
                                    context_verified,
                                    available: true,
                                    path: path.to_string_lossy().to_string(),
                                    mmproj_path: std::fs::read_dir(&desktop_dir)
                                        .ok()
                                        .and_then(|entries| {
                                            entries.flatten().map(|entry| entry.path()).find(
                                                |candidate| {
                                                    candidate
                                                        .file_name()
                                                        .and_then(|name| name.to_str())
                                                        .is_some_and(|name| {
                                                            name.ends_with(".gguf")
                                                                && name.contains("mmproj")
                                                        })
                                                },
                                            )
                                        })
                                        .map(|path| path.to_string_lossy().to_string()),
                                });
                            }
                        }
                    }
                }
            }

            // Mobile models (E2B)
            let mobile_dir = PathBuf::from(vault_root).join("MODELS").join("MOBILE");
            if mobile_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&mobile_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if name.ends_with(".gguf") {
                                let file_size = std::fs::metadata(&path)
                                    .map(|m| m.len() as f64 / (1024.0 * 1024.0 * 1024.0))
                                    .unwrap_or(0.0);
                                let (native_context, context_verified) =
                                    native_context_or_default(&path);
                                models.push(ModelInfo {
                                    name: "Gemma 4 E2B Q4_K_M".to_string(),
                                    model_type: "gemma-4-e2b".to_string(),
                                    quantization: "Q4_K_M".to_string(),
                                    file_size_gb: file_size,
                                    context_length: native_context,
                                    context_verified,
                                    available: true,
                                    path: path.to_string_lossy().to_string(),
                                    mmproj_path: None,
                                });
                            }
                        }
                    }
                }
            }
        }

        // If no models found, mark as not available
        if models.is_empty() {
            models.push(ModelInfo {
                name: "Gemma 4 12B Q4_K_M".to_string(),
                model_type: "gemma-4-12b".to_string(),
                quantization: "Q4_K_M".to_string(),
                file_size_gb: 7.14,
                context_length: 8192,
                context_verified: false,
                available: false,
                path: String::new(),
                mmproj_path: None,
            });
        }

        models
    }

    /// Get the llama.cpp binary path for the current platform
    /// Uses manifest-informed directory structure (uppercase CUDA/CPU/VULKAN)
    /// Prefers CUDA > Vulkan > CPU based on detected hardware
    fn get_llama_binary_path(&self, vault_root: &str) -> PathBuf {
        let base_dir = if cfg!(target_os = "windows") {
            PathBuf::from(vault_root).join("RUNTIMES").join("WINDOWS")
        } else if cfg!(target_os = "macos") {
            PathBuf::from(vault_root).join("RUNTIMES").join("MACOS")
        } else {
            PathBuf::from(vault_root).join("RUNTIMES").join("LINUX")
        };

        let binary_name = if cfg!(target_os = "windows") {
            "llama-server.exe"
        } else {
            "llama-server"
        };

        // Order: CUDA > Vulkan > CPU (matches hardware acceleration priority)
        let backends = if cfg!(target_os = "macos") {
            vec!["METAL"]
        } else {
            vec!["CUDA", "VULKAN", "CPU"]
        };

        for backend in &backends {
            let path = base_dir.join(backend).join(binary_name);
            if path.exists() {
                // Verify the implementation DLL exists (9KB launcher is useless without it)
                let impl_path = base_dir.join(backend).join("llama-server-impl.dll");
                let impl_path_mac = base_dir.join(backend).join("llama-server-impl.dylib");
                if impl_path.exists() || impl_path_mac.exists() || !cfg!(target_os = "windows") {
                    return path;
                }
                // On Windows, if the impl DLL doesn't exist, skip this backend
            }
        }

        // Fallback: check lowercase paths for backwards compatibility
        let backends_compat = if cfg!(target_os = "macos") {
            vec!["metal"]
        } else {
            vec!["cuda", "vulkan", "cpu"]
        };

        for backend in &backends_compat {
            let path = base_dir.join(backend).join(binary_name);
            if path.exists() {
                return path;
            }
        }

        // Last resort: direct in runtime dir
        base_dir.join(binary_name)
    }

    /// Start llama-server for inference on a dynamically chosen free port.
    /// Verifies server identity (health + /v1/models + model hash) before
    /// marking the model as LOADED. Any failure resets status to ERROR and
    /// kills the child process.
    pub async fn start_server(
        &self,
        config: &ModelConfig,
        vault_root: &str,
        cancel: &inbharat_harness_core::CancellationToken,
    ) -> Result<u16, String> {
        let llama_path = self.get_llama_binary_path(vault_root);

        if !llama_path.exists() {
            *self.status.lock().unwrap() = ModelStatus::Error;
            return Err(format!(
                "llama-server not found at {:?}. Please install llama.cpp runtime.",
                llama_path
            ));
        }

        if config.model_path.is_empty() {
            *self.status.lock().unwrap() = ModelStatus::Error;
            return Err("No model path configured".to_string());
        }

        let model_path = PathBuf::from(&config.model_path);
        if !model_path.exists() {
            *self.status.lock().unwrap() = ModelStatus::Error;
            return Err(format!("Model file not found: {:?}", config.model_path));
        }

        // Universal-adaptive context (2026-10-02): the requested context is
        // clamped against the *artifact's* trained context (read from the
        // GGUF header) and the host RAM tier before the server ever sees it.
        // The derivation and its reasons are recorded in the session log so
        // the panel and the log agree on why the session runs at this size.
        let gguf_meta = crate::gguf_meta::read_metadata(&model_path).ok();
        crate::boot_trace::mark_detail(
            "start_server: gguf meta read",
            &format!(
                "ctx_len={:?}",
                gguf_meta.as_ref().and_then(|m| m.context_length)
            ),
        );
        let context_budget = crate::gguf_meta::derive_context_budget(
            gguf_meta.as_ref(),
            config.context_size,
            detected_ram_gib().map(|gib| gib as u64),
            config.cache_type_k.as_deref(),
        );
        *self.granted_context.lock().unwrap() = Some(context_budget.granted_context);

        // Strict identity policy requires a disk hash compared against the
        // manifest hash. Hash the model ONCE here, BEFORE spawning
        // llama-server:
        //   (a) a tampered model is refused before any inference process is
        //       even started on it;
        //   (b) the verification poll loop below must never re-read a
        //       multi-GB file off removable media — re-hashing after the
        //       model is already loaded saturates the USB drive for minutes
        //       while the UI is stuck showing "no model loaded" even though
        //       the server is up and healthy.
        *self.status.lock().unwrap() = ModelStatus::Loading;
        let hash_started = std::time::Instant::now();
        crate::boot_trace::mark_detail("start_server: model hash begin", &config.model_path);
        let disk_sha256 = Some(
            Self::sha256_file_cancellable(&model_path, Some(cancel)).inspect_err(|_e| {
                *self.status.lock().unwrap() = ModelStatus::Error;
            })?,
        );
        crate::boot_trace::mark_detail(
            "start_server: model hash end",
            &format!("elapsed={:.1}s", hash_started.elapsed().as_secs_f32()),
        );

        // Find an available port dynamically so multiple runs cannot collide.
        let port = Self::find_free_port()?;

        // The shipped llama-server.exe is a 9 KB stub that dynamically loads
        // llama-server-impl.dll from the same directory. If the working
        // directory is not the backend folder, Windows DLL search can fail and
        // the stub exits before printing anything. Run from the binary's parent.
        let backend_dir = llama_path
            .parent()
            .ok_or_else(|| {
                *self.status.lock().unwrap() = ModelStatus::Error;
                "llama-server path has no parent directory".to_string()
            })?
            .to_path_buf();

        let mut cmd = Command::new(&llama_path);
        cmd.current_dir(&backend_dir);
        // Defect #23 follow-up (live-caught 2026-09-13): the server's output
        // was discarded entirely (Stdio::null), making live failures — like
        // a request the server rejected instantly — undiagnosable from the
        // drive. Log to a rotating pair of files in the host temp area so a
        // debug session can see exactly what the server received and did.
        let log_dir = std::env::temp_dir().join("unoone-logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("llama-server.log");
        // Rotate the previous run's log so each session starts fresh.
        let _ = std::fs::rename(&log_path, log_dir.join("llama-server.prev.log"));
        let log_file = std::fs::File::create(&log_path).ok();
        {
            // Budget header: every clamp is visible in the same log the
            // server diagnostics land in.
            use std::io::Write as _;
            let native = context_budget
                .native_context
                .map(|n| n.to_string())
                .unwrap_or_else(|| "unverified (artifact not readable)".to_string());
            let kv = context_budget
                .kv_estimate_bytes
                .map(|b| format!("{:.2} GiB", b as f64 / (1024.0 * 1024.0 * 1024.0)))
                .unwrap_or_else(|| "unverified".to_string());
            let mut header = format!(
                "# context budget: requested {} granted {} | native {} | kv-estimate {} | {}\n",
                config.context_size,
                context_budget.granted_context,
                native,
                kv,
                context_budget.limiting_reason()
            );
            for reason in &context_budget.reasons {
                header.push_str(&format!("# clamp: {reason}\n"));
            }
            let _ = std::fs::OpenOptions::new()
                .append(true)
                .open(&log_path)
                .and_then(|mut f| f.write_all(header.as_bytes()));
        }
        match log_file.and_then(|f| f.try_clone().ok().map(|dup| (f, dup))) {
            Some((file, dup)) => {
                cmd.stdout(std::process::Stdio::from(file));
                cmd.stderr(std::process::Stdio::from(dup));
            }
            None => {
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
            }
        }
        cmd.args([
            "-m",
            &config.model_path,
            "--port",
            &port.to_string(),
            "-c",
            &context_budget.granted_context.to_string(),
            "-b",
            &config.batch_size.to_string(),
            "--temp",
            &config.temperature.to_string(),
            "--top-p",
            &config.top_p.to_string(),
            "--top-k",
            &config.top_k.to_string(),
            "--repeat-penalty",
            &config.repeat_penalty.to_string(),
            "-n",
            &config.max_tokens.to_string(),
        ]);

        // GPU layers
        if config.gpu_layers != 0 {
            cmd.args(["-ngl", &config.gpu_layers.to_string()]);
        }

        // Multimodal projector (mmproj) for vision/OCR
        if let Some(mmproj) = &config.mmproj_path {
            let mmproj_path = PathBuf::from(mmproj);
            if mmproj_path.exists() {
                cmd.args(["--mmproj", mmproj]);
            }
        }

        // KV-cache quantization: q8_0/q4_0 let a 16K-32K context fit hosts
        // with little VRAM (the 4 GB RTX 5050 laptop class). A quantized V
        // cache needs flash attention; the shipped b10075 server defaults
        // to `-fa auto`, which enables it where the backend supports it.
        // Invalid values are skipped, never fatal — a config typo must not
        // take down model loading on any host.
        if let Some(cache_type) = config.cache_type_k.as_deref() {
            if ModelConfig::valid_cache_type(cache_type) {
                cmd.args(["-ctk", cache_type]);
            }
        }
        if let Some(cache_type) = config.cache_type_v.as_deref() {
            if ModelConfig::valid_cache_type(cache_type) {
                cmd.args(["-ctv", cache_type]);
            }
        }
        match config.flash_attention {
            Some(true) => {
                cmd.args(["-fa", "on"]);
            }
            Some(false) => {
                cmd.args(["-fa", "off"]);
            }
            None => {}
        }

        // Pin the GGUF's native chat template explicitly. b10075 defaults
        // --jinja on, which renders the OpenAI `tools` array we send on
        // agentic runs; passing it explicitly protects tool calling if a
        // future bundled runtime changes that default. (Self-knowledge of
        // the agent's abilities does not rely on this: L0 direct-answer
        // requests carry no tools array, which is why the embedding also
        // installs a truthful system-prefix briefing in the harness.)
        cmd.arg("--jinja");

        // Threads
        if config.threads > 0 {
            cmd.args(["-t", &config.threads.to_string()]);
        }

        // Backend-specific flags. Scope the mutex guard so it is dropped before
        // any `.await` point, keeping the async future `Send`.
        // NOTE: llama.cpp has no `--gpu` flag. The bundled Windows builds pick
        // their backend from the ggml-*.dlls next to the executable, so the
        // CUDA/Vulkan variants need no extra argument — passing `--gpu` makes
        // llama-server exit with code 1 ("invalid argument: --gpu").
        {
            let backend = self.backend.lock().unwrap();
            match *backend {
                AccelerationBackend::Cuda
                | AccelerationBackend::Vulkan
                | AccelerationBackend::Metal => {
                    // Backend comes from the backend-specific DLL directory; GPU
                    // offload is already expressed via -ngl above.
                }
                AccelerationBackend::Cpu => {
                    cmd.args(["-ngl", "0"]);
                }
            }
        }

        // Start the process — DO NOT mark as Loaded until identity verification passes.
        crate::boot_trace::mark_detail(
            "start_server: spawning llama-server",
            &format!("port={port}"),
        );
        cancel
            .check("desktop.model.spawn")
            .map_err(|e| e.to_string())?;
        let child = cmd.spawn().map_err(|e| {
            *self.status.lock().unwrap() = ModelStatus::Error;
            format!("Failed to start llama-server: {}", e)
        })?;
        let mut child = StartingModelChild(Some(child));

        let pid = child.id();

        // Wait for the server to open its HTTP port or fail fast.
        // Loading a multi-GB model from removable media can take several
        // minutes; llama-server listens immediately but answers 503
        // ("Loading model") on /health until the load completes, so the
        // identity check must tolerate that pending state and keep polling.
        let deadline = Instant::now() + Duration::from_secs(240);
        let addr = format!("127.0.0.1:{}", port);
        let mut last_err = String::from("server did not open port in time");
        loop {
            if cancel.is_cancelled() {
                return Err(self.reset_to_error(&mut child, "Model startup cancelled".to_owned()));
            }
            // If the stub crashed before binding, surface it immediately.
            match child.try_wait() {
                Ok(Some(status)) => {
                    last_err = format!("llama-server exited early with {}", status);
                    break;
                }
                Ok(None) => {}
                Err(e) => {
                    last_err = format!("Failed to poll llama-server: {}", e);
                    break;
                }
            }

            if std::net::TcpStream::connect_timeout(
                &addr.parse().unwrap(),
                Duration::from_millis(200),
            )
            .is_ok()
            {
                // Port is open. Verify the server identity before claiming LOADED.
                match Self::verify_server_identity(
                    port,
                    &config.model_path,
                    vault_root,
                    disk_sha256.clone(),
                )
                .await
                {
                    Ok(mut identity) => {
                        identity.pid = pid;
                        *self.server_identity.lock().unwrap() = Some(identity);
                        *self.llama_process.lock().unwrap() = child.0.take();
                        *self.status.lock().unwrap() = ModelStatus::Loaded;
                        crate::boot_trace::mark_detail(
                            "start_server: VERIFIED LOADED",
                            &format!("port={port} pid={pid}"),
                        );
                        return Ok(port);
                    }
                    Err(e) => {
                        // 503 = server is up but the model is still loading.
                        // Keep polling until the deadline instead of killing a
                        // perfectly healthy startup.
                        let still_loading = e.contains("503")
                            || e.contains("Service Unavailable")
                            || e.to_lowercase().contains("loading");
                        if !still_loading {
                            return Err(self.reset_to_error(
                                &mut child,
                                format!(
                                    "llama-server identity verification failed on port {}: {}",
                                    port, e
                                ),
                            ));
                        }
                        last_err = format!("identity still pending: {}", e);
                    }
                }
            }

            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        Err(self.reset_to_error(
            &mut child,
            format!("llama-server failed to start on {}. {}", addr, last_err),
        ))
    }

    /// Stop the llama-server process
    pub fn stop_server(&self) -> Result<(), String> {
        let mut process = self.llama_process.lock().unwrap();
        if let Some(ref mut child) = *process {
            child
                .kill()
                .map_err(|e| format!("Failed to kill llama-server: {}", e))?;
            let _ = child.wait();
            *process = None;
        }
        *self.server_identity.lock().unwrap() = None;
        *self.granted_context.lock().unwrap() = None;
        *self.status.lock().unwrap() = ModelStatus::NotLoaded;
        Ok(())
    }

    /// Get current model status
    pub fn get_status(&self) -> ModelStatus {
        self.status.lock().unwrap().clone()
    }

    /// The verified model id of the currently running local server, or `None`
    /// when no server has passed identity verification. Used by the Harness
    /// bridge to bind a request to the verified 127.0.0.1 llama-server only.
    pub fn running_model_id(&self) -> Option<String> {
        self.server_identity
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|identity| identity.model_id.clone()))
    }

    /// Set backend
    pub fn set_backend(&self, backend: AccelerationBackend) {
        *self.backend.lock().unwrap() = backend;
    }

    /// D1: Send a chat completion request to llama-server via HTTP.
    /// Uses reqwest to POST to the OpenAI-compatible /v1/chat/completions endpoint.
    /// Supports both plain text responses and function/tool calling.
    pub async fn send_completion(
        &self,
        request: &InferenceRequest,
        port: u16,
    ) -> Result<InferenceResponse, String> {
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", port);

        // Item 24 (2026-10-02): token-accurate truncation. When the caller
        // supplies the granted context budget, the oldest turns (never the
        // system prompt or the last user turn) are dropped so the prompt +
        // response reserve fit the window. The note surfaces in the response
        // so the UI can show that history was trimmed.
        let (history, dropped_turns) = match request.context_budget {
            Some(budget) => trim_history_to_budget(
                request.system_prompt.as_deref(),
                &request.conversation_history,
                request.tools.as_ref(),
                budget,
                request.max_tokens.unwrap_or(4096),
            ),
            None => (request.conversation_history.clone(), 0),
        };
        let context_note = (dropped_turns > 0).then(|| {
            format!(
                "trimmed {} oldest turn{} to fit the {}-token context budget",
                dropped_turns,
                if dropped_turns == 1 { "" } else { "s" },
                request.context_budget.unwrap_or(0)
            )
        });

        // Build OpenAI-compatible request body
        let mut messages = Vec::new();
        if let Some(sys) = &request.system_prompt {
            messages.push(serde_json::json!({"role": "system", "content": sys}));
        }
        for turn in &history {
            // Serialize Content enum: Text becomes a plain string,
            // Multimodal becomes an array of content parts
            let content_value = match &turn.content {
                Content::Text(text) => serde_json::json!(text),
                Content::Multimodal(parts) => {
                    serde_json::json!(parts
                        .iter()
                        .map(|part| {
                            match part {
                                ContentPart::text { text } => serde_json::json!({
                                    "type": "text",
                                    "text": text,
                                }),
                                ContentPart::image_url { image_url } => serde_json::json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": image_url.url,
                                    },
                                }),
                            }
                        })
                        .collect::<Vec<_>>())
                }
            };
            let mut msg = serde_json::json!({"role": turn.role, "content": content_value});
            if let Some(tool_calls) = &turn.tool_calls {
                msg["tool_calls"] = serde_json::json!(tool_calls
                    .iter()
                    .map(|tc| {
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments.to_string(),
                            }
                        })
                    })
                    .collect::<Vec<_>>());
            }
            if let Some(tool_call_id) = &turn.tool_call_id {
                msg["tool_call_id"] = serde_json::json!(tool_call_id);
            }
            messages.push(msg);
        }
        // Add the current user prompt if not already in history
        if !request.prompt.is_empty() {
            messages.push(serde_json::json!({"role": "user", "content": &request.prompt}));
        }

        let mut body = serde_json::json!({
            "model": "gemma-4-12b",
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
            "temperature": request.temperature.unwrap_or(0.7),
            "stream": false,
            // Gap 3 (2026-09-16): explicit rather than relying on the server
            // default — llama-server prefix-caches across requests (measured
            // live: a grown-history request served cached_tokens=91), and this
            // makes the intent auditable in any request log.
            "cache_prompt": true,
        });

        // Gap 2b (2026-09-16): think-off for latency-critical lanes. Same
        // kwargs the harness provider already sends (llama_local.rs) and the
        // live A/B probe proved honored by the staged Gemma 4 template:
        // 376 -> 55 completion tokens, 44.6 s -> 5.7 s, reasoning_content
        // 1181 chars -> 0, with comparable answer length.
        if request.disable_reasoning.unwrap_or(false) {
            body["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": false });
            body["reasoning_budget"] = serde_json::json!(0);
        }

        if let Some(tools) = &request.tools {
            body["tools"] = serde_json::json!(tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect::<Vec<_>>());
        }

        // Bounded client: a completion can legitimately take minutes (large
        // prompts, vision payloads) but must never hang FOREVER — an
        // unbounded default client wedged agent loops and the blind-view
        // describe flow when the server stopped responding (defect #23).
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;
        let response = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Failed to connect to llama-server: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("llama-server error {}: {}", status, text));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        // Parse OpenAI-compatible response
        let choice = data.get("choices").and_then(|c| c.get(0));
        let message = choice.and_then(|c| c.get("message"));

        let text = message
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        // Gap 4 (2026-09-16): extract the model's chain-of-thought when the
        // template emits it into reasoning_content, so callers can tell the
        // honest empty-reply case (defect #44) apart and never mistake
        // "reasoned for the whole budget" for "said nothing".
        let reasoning = message
            .and_then(|m| m.get("reasoning_content"))
            .and_then(|r| r.as_str())
            .filter(|r| !r.is_empty())
            .map(|r| r.to_string());

        // Parse tool calls if present
        let tool_calls = message
            .and_then(|m| m.get("tool_calls"))
            .and_then(|tc| tc.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|tc| {
                        let id = tc
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arguments = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|a| a.as_str())
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                        ToolCallResult {
                            id,
                            name,
                            arguments,
                        }
                    })
                    .collect::<Vec<_>>()
            });

        let finish_reason = choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(|f| f.as_str())
            .map(|s| s.to_string());

        // If no structured tool_calls but text contains tool-call JSON, parse as fallback
        let final_tool_calls = if tool_calls.as_ref().is_none_or(|tc| tc.is_empty()) {
            Self::parse_text_tool_calls(&text)
        } else {
            tool_calls
        };

        Ok(InferenceResponse {
            text,
            tokens_generated: data
                .get("usage")
                .and_then(|u| u.get("completion_tokens"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as u32,
            // Gap 3 (2026-09-16): real generation speed from the server's
            // own timings object (present on non-streaming OAI completions)
            // instead of the old hardcoded 0.0 — measured live, e.g. the A/B
            // probe's think-off run: 55 tokens in 5.68 s ≈ 9.7 tok/s.
            tokens_per_second: data
                .get("timings")
                .and_then(|t| t.get("predicted_per_second"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32,
            cached_prompt_tokens: data
                .get("usage")
                .and_then(|u| u.get("prompt_tokens_details"))
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|t| t.as_u64())
                .map(|t| t as u32),
            model_id: data
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("gemma-4-12b")
                .to_string(),
            tool_calls: final_tool_calls,
            finish_reason,
            reasoning,
            context_note,
        })
    }

    /// Fallback parser for models that emit tool calls as text instead of structured JSON.
    /// Looks for ```json\n{"tool": "...", "args": {...}}\n``` blocks in the response.
    fn parse_text_tool_calls(text: &str) -> Option<Vec<ToolCallResult>> {
        let mut results = Vec::new();
        let mut id_counter = 0u32;

        // Pattern 1: ```json blocks containing tool calls
        for block in text.split("```json").skip(1) {
            if let Some(end) = block.find("```") {
                let json_str = block[..end].trim();
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                    let tool_name = parsed
                        .get("tool")
                        .or_else(|| parsed.get("name"))
                        .or_else(|| parsed.get("action"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !tool_name.is_empty() {
                        let args = parsed
                            .get("args")
                            .or_else(|| parsed.get("arguments"))
                            .or_else(|| parsed.get("parameters"))
                            .cloned()
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                        results.push(ToolCallResult {
                            id: format!("call_{}", id_counter),
                            name: tool_name,
                            arguments: args,
                        });
                        id_counter += 1;
                    }
                }
            }
        }

        if results.is_empty() {
            None
        } else {
            Some(results)
        }
    }
}

/// D1: State wrapper for ModelManager so it can be held as Tauri managed state.
/// Uses tokio::sync::Mutex so the guard can be held across .await points (Send).
pub struct ModelManagerState {
    pub manager: tokio::sync::Mutex<Option<ModelManager>>,
    pub server_port: std::sync::Mutex<u16>,
    generation: std::sync::atomic::AtomicU64,
    suspended: std::sync::atomic::AtomicBool,
    startup_cancel: std::sync::Mutex<inbharat_harness_core::CancellationToken>,
}

impl ModelManagerState {
    pub fn new() -> Self {
        Self {
            manager: tokio::sync::Mutex::new(None),
            server_port: std::sync::Mutex::new(8342),
            generation: std::sync::atomic::AtomicU64::new(0),
            suspended: std::sync::atomic::AtomicBool::new(false),
            startup_cancel: std::sync::Mutex::new(inbharat_harness_core::CancellationToken::new()),
        }
    }

    pub(crate) fn suspend(&self) {
        let cancel = self
            .startup_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cancel.cancel(inbharat_harness_core::CancelCause::Parent);
        self.suspended
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn resume(&self) {
        let mut cancel = self
            .startup_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // The first unlock must retain a pre-unlock boot's cancellation token.
        // Replace it only when a genuinely suspended session is reopened.
        if self
            .suspended
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            *cancel = inbharat_harness_core::CancellationToken::new();
            self.generation
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn cancel_pending_start(&self) {
        let mut cancel = self
            .startup_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cancel.cancel(inbharat_harness_core::CancelCause::User);
        *cancel = inbharat_harness_core::CancellationToken::new();
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Emergency cleanup used when the Pocket AI is removed or the app exits.
    pub async fn emergency_stop(&self) {
        let generation = self.generation.load(std::sync::atomic::Ordering::SeqCst);
        let mut manager = self.manager.lock().await;
        if generation != self.generation.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if let Some(manager) = manager.take() {
            let _ = manager.stop_server();
        }
        if let Ok(mut port) = self.server_port.lock() {
            *port = 8342;
        }
    }

    pub fn emergency_stop_blocking(&self) {
        self.suspend();
        if let Ok(mut manager) = self.manager.try_lock() {
            if let Some(manager) = manager.take() {
                let _ = manager.stop_server();
            }
        }
        if let Ok(mut port) = self.server_port.lock() {
            *port = 8342;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // -- item 24: token-accurate history trimming --

    fn turn(role: &str, text: &str) -> ConversationTurn {
        ConversationTurn {
            role: role.to_string(),
            content: Content::text(text),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn short_history_is_sent_untouched() {
        let history = vec![turn("user", "hello"), turn("assistant", "hi")];
        let (trimmed, dropped) = trim_history_to_budget(None, &history, None, 32_768, 4096);
        assert_eq!(dropped, 0);
        assert_eq!(trimmed.len(), 2);
    }

    #[test]
    fn oversized_history_drops_oldest_but_never_the_last_user_turn() {
        // 60 turns of ~600 estimated tokens each = ~36.5k; budget after
        // reserve (4096+512) is 28_160 — the oldest turns must go.
        let mut history = Vec::new();
        for i in 0..60 {
            let filler = "x".repeat(1_800); // ~600 tokens by bytes/3
            if i == 59 {
                history.push(turn("user", &format!("answer this: {}", filler)));
            } else {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                history.push(turn(role, &filler));
            }
        }
        let (trimmed, dropped) = trim_history_to_budget(None, &history, None, 32_768, 4096);
        assert!(dropped > 0, "a 36k-token history in a 28k budget must trim");
        assert!(
            dropped < 60,
            "the budget must keep most turns, not empty the history"
        );
        // The LAST user turn (index 59) must survive.
        let last = trimmed.last().expect("history must never be empty");
        assert_eq!(last.role, "user");
        let last_text = match &last.content {
            Content::Text(t) => t.clone(),
            Content::Multimodal(_) => String::new(),
        };
        assert!(last_text.contains("answer this:"));
        // Oldest turns went first.
        assert_eq!(
            trimmed[0].role,
            if dropped % 2 == 0 {
                "user"
            } else {
                "assistant"
            }
        );
    }

    #[test]
    fn system_prompt_and_tools_are_charged_to_the_budget() {
        // Same history, but now a 20k-token system prompt + tools must force
        // more trimming than the bare call.
        let mut history = Vec::new();
        for i in 0..30 {
            let filler = "x".repeat(1_800);
            history.push(turn("user", &format!("{} {}", i, filler)));
        }
        let sys = "s".repeat(30_000); // ~10k tokens
        let (_, dropped_bare) = trim_history_to_budget(None, &history, None, 32_768, 4096);
        let (_, dropped_with_sys) =
            trim_history_to_budget(Some(&sys), &history, None, 32_768, 4096);
        assert!(dropped_with_sys > dropped_bare);
    }

    #[test]
    fn a_single_turn_larger_than_the_window_is_kept_not_silently_dropped() {
        let huge = "x".repeat(300_000); // ~100k tokens > whole 32k window
        let history = vec![turn("user", &huge)];
        let (trimmed, dropped) = trim_history_to_budget(None, &history, None, 32_768, 4096);
        assert_eq!(dropped, 0);
        assert_eq!(
            trimmed.len(),
            1,
            "the current task must still reach the model"
        );
    }

    #[test]
    fn estimated_tokens_is_conservative() {
        // 12 bytes of ASCII ≈ 3 tokens at 4 B/token real; bytes/3 gives 5.
        assert!(estimated_tokens("hello world") >= 3);
        assert_eq!(estimated_tokens(""), 1);
    }

    #[test]
    fn find_free_port_returns_usable_local_port() {
        let port = ModelManager::find_free_port().expect("a free port should be available");
        assert!(port > 0);
    }

    /// The adaptive boot config must match the step for whatever RAM this
    /// host reports, so every host class (CI runners included) verifies the
    /// mapping, not just large-memory dev machines.
    #[test]
    fn boot_config_matches_the_detected_memory_step() {
        let config = get_model_config();
        match detected_ram_gib() {
            Some(ram) if ram >= 24 => {
                assert_eq!(config.context_size, 32768);
                assert_eq!(config.cache_type_k.as_deref(), Some("q8_0"));
                assert_eq!(config.cache_type_v.as_deref(), Some("q8_0"));
            }
            Some(ram) if ram >= 12 => {
                assert_eq!(config.context_size, 16384);
                assert_eq!(config.cache_type_k.as_deref(), Some("q8_0"));
                assert_eq!(config.cache_type_v.as_deref(), Some("q8_0"));
            }
            _ => {
                assert_eq!(config.context_size, 4096);
                assert_eq!(config.cache_type_k, None);
                assert_eq!(config.cache_type_v, None);
            }
        }
    }

    /// The memory probe must return a plausible positive value on a real
    /// machine (this runs on Windows/Linux/macOS CI hosts).
    #[test]
    fn detected_ram_is_plausible() {
        if let Some(ram) = detected_ram_gib() {
            assert!(ram >= 1, "nonsense RAM reading: {ram} GiB");
        }
    }

    #[test]
    fn sha256_file_matches_known_digest() {
        let tmp_path = std::env::temp_dir().join("unoone-sha256-test.txt");
        let mut file = std::fs::File::create(&tmp_path).expect("create temp file");
        file.write_all(b"hello").expect("write temp file");
        drop(file);

        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let actual = ModelManager::sha256_file(&tmp_path).expect("hash should compute");
        assert_eq!(actual, expected);
        let _ = std::fs::remove_file(&tmp_path);
    }

    #[test]
    fn read_manifest_model_hash_matches_relative_and_absolute_paths() {
        let vault_dir = std::env::temp_dir().join("unoone-manifest-test");
        let _ = std::fs::remove_dir_all(&vault_dir);
        std::fs::create_dir_all(&vault_dir).expect("create vault dir");

        let manifest = serde_json::json!({
            "models": {
                "desktop": {
                    "gemma": {
                        "path": "models/gemma.gguf",
                        "sha256": "deadbeef"
                    }
                }
            }
        });
        let manifest_path = vault_dir.join("manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::to_string(&manifest).expect("serialize manifest"),
        )
        .expect("write manifest");

        // Relative path from manifest.
        let hash = ModelManager::read_manifest_model_hash(
            vault_dir.to_str().unwrap(),
            "models/gemma.gguf",
        );
        assert_eq!(hash, Some("deadbeef".to_string()));

        // Absolute path to the same file.
        let abs_model = vault_dir.join("models/gemma.gguf");
        let hash = ModelManager::read_manifest_model_hash(
            vault_dir.to_str().unwrap(),
            abs_model.to_str().unwrap(),
        );
        assert_eq!(hash, Some("deadbeef".to_string()));

        let _ = std::fs::remove_dir_all(&vault_dir);
    }

    #[test]
    fn read_schema_v2_manifest_model_hash_matches_relative_and_absolute_paths() {
        let vault_dir = std::env::temp_dir().join("unoone-manifest-v2-test");
        let _ = std::fs::remove_dir_all(&vault_dir);
        std::fs::create_dir_all(vault_dir.join("models")).expect("create model directory");
        std::fs::write(vault_dir.join("models/gemma.gguf"), b"model").expect("write model");

        let manifest = serde_json::json!({
            "product_id": unoone_usb_manifest::PRODUCT_ID,
            "schema_version": unoone_usb_manifest::MANIFEST_SCHEMA_VERSION,
            "pai_version": "test",
            "vault": { "id_path": "VAULT/identity/vault.id" },
            "platforms": {
                "windows": {
                    "architectures": [std::env::consts::ARCH],
                    "desktop": {
                        "id": "power",
                        "kind": "DESKTOP_EXECUTABLE",
                        "path": "apps/power.exe",
                        "size_bytes": 1,
                        "sha256": "00",
                        "required": true
                    },
                    "runtimes": [],
                    "models": [{
                        "id": "gemma",
                        "kind": "MODEL",
                        "path": "models/gemma.gguf",
                        "size_bytes": 5,
                        "sha256": "deadbeef",
                        "required": true
                    }],
                    "voice": []
                }
            }
        });
        std::fs::write(
            vault_dir.join("manifest.json"),
            serde_json::to_string(&manifest).expect("serialize manifest"),
        )
        .expect("write manifest");

        let relative = ModelManager::read_manifest_model_hash(
            vault_dir.to_str().unwrap(),
            "models/gemma.gguf",
        );
        assert_eq!(relative, Some("deadbeef".to_string()));

        let absolute = ModelManager::read_manifest_model_hash(
            vault_dir.to_str().unwrap(),
            vault_dir.join("models/gemma.gguf").to_str().unwrap(),
        );
        assert_eq!(absolute, Some("deadbeef".to_string()));
        let _ = std::fs::remove_dir_all(&vault_dir);
    }

    #[tokio::test]
    async fn verify_server_identity_rejects_mismatched_hash() {
        let vault_dir = std::env::temp_dir().join("unoone-verify-hash-test");
        let _ = std::fs::remove_dir_all(&vault_dir);
        std::fs::create_dir_all(&vault_dir).expect("create vault dir");

        // Create a model file and a manifest with a deliberately wrong hash.
        let model_path = vault_dir.join("model.gguf");
        std::fs::write(&model_path, b"model-data").unwrap();

        let manifest = serde_json::json!({
            "models": {
                "desktop": {
                    "gemma": { "path": "model.gguf", "sha256": "0000000000000000000000000000000000000000000000000000000000000000" }
                }
            }
        });
        std::fs::write(
            vault_dir.join("manifest.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();

        // Start a minimal mock server that responds correctly to /health and /v1/models.
        let mut server = mockito::Server::new_async().await;
        let health_body = serde_json::json!({ "status": "ok", "model_loaded": true });
        let models_body = serde_json::json!({ "data": [{ "id": "gemma-4-12b" }] });
        server
            .mock("GET", "/health")
            .with_body(health_body.to_string())
            .create_async()
            .await;
        server
            .mock("GET", "/v1/models")
            .with_body(models_body.to_string())
            .create_async()
            .await;

        let port = server
            .host_with_port()
            .split(':')
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap();

        // Verify that the manifest hash mismatch is caught even though the
        // server looks healthy. start_server hashes the model before spawn;
        // mirror that by passing the true disk hash.
        let result = ModelManager::verify_server_identity(
            port,
            model_path.to_str().unwrap(),
            vault_dir.to_str().unwrap(),
            Some(ModelManager::sha256_file(&model_path).unwrap()),
        )
        .await;
        assert!(result.is_err(), "Expected hash mismatch error");
        let err = result.unwrap_err();
        assert!(
            err.contains("SHA-256 mismatch"),
            "Error should mention hash mismatch: {}",
            err
        );
    }

    #[tokio::test]
    async fn verify_server_identity_rejects_no_model_id() {
        let vault_dir = std::env::temp_dir().join("unoone-verify-model-test");
        let _ = std::fs::remove_dir_all(&vault_dir);
        std::fs::create_dir_all(&vault_dir).expect("create vault dir");

        // No manifest, no model file needed for this path.
        let mut server = mockito::Server::new_async().await;
        let health_body = serde_json::json!({ "status": "ok" });
        let models_body = serde_json::json!({ "data": [{ "id": "" }] });
        server
            .mock("GET", "/health")
            .with_body(health_body.to_string())
            .create_async()
            .await;
        server
            .mock("GET", "/v1/models")
            .with_body(models_body.to_string())
            .create_async()
            .await;

        let port = server
            .host_with_port()
            .split(':')
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap();

        let result = ModelManager::verify_server_identity(
            port,
            "/nonexistent/model.gguf",
            vault_dir.to_str().unwrap(),
            None,
        )
        .await;
        assert!(result.is_err(), "Expected missing model id error");
        assert!(result.unwrap_err().contains("did not report a model id"));
    }

    #[tokio::test]
    async fn verify_server_identity_accepts_matching_hash() {
        let vault_dir = std::env::temp_dir().join("unoone-verify-match-test");
        let _ = std::fs::remove_dir_all(&vault_dir);
        std::fs::create_dir_all(&vault_dir).expect("create vault dir");

        let model_path = vault_dir.join("model.gguf");
        std::fs::write(&model_path, b"model-data").unwrap();

        let expected_hash = ModelManager::sha256_file(&model_path).unwrap();
        let manifest = serde_json::json!({
            "models": {
                "desktop": {
                    "gemma": { "path": "model.gguf", "sha256": expected_hash }
                }
            }
        });
        std::fs::write(
            vault_dir.join("manifest.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();

        let mut server = mockito::Server::new_async().await;
        let health_body = serde_json::json!({ "status": "ok" });
        let models_body = serde_json::json!({ "data": [{ "id": "gemma-4-12b" }] });
        server
            .mock("GET", "/health")
            .with_body(health_body.to_string())
            .create_async()
            .await;
        server
            .mock("GET", "/v1/models")
            .with_body(models_body.to_string())
            .create_async()
            .await;

        let port = server
            .host_with_port()
            .split(':')
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap();

        let identity = ModelManager::verify_server_identity(
            port,
            model_path.to_str().unwrap(),
            vault_dir.to_str().unwrap(),
            Some(expected_hash.clone()),
        )
        .await;
        assert!(
            identity.is_ok(),
            "Expected successful identity verification"
        );
        assert_eq!(identity.unwrap().model_id, "gemma-4-12b");
    }

    #[tokio::test]
    async fn verify_server_identity_rejects_when_manifest_has_no_hash() {
        // Under the Strict identity policy (MODEL_IDENTITY_POLICY), a manifest
        // that declares no SHA-256 for the model is itself a verification
        // failure. Previously this "fell back" to accepting any non-empty model
        // id, which let a substituted model pass unnoticed — the fail-open bug.
        let vault_dir = std::env::temp_dir().join("unoone-verify-nohash-test");
        let _ = std::fs::remove_dir_all(&vault_dir);
        std::fs::create_dir_all(&vault_dir).expect("create vault dir");

        // Model file exists but manifest has no sha256 entry.
        let model_path = vault_dir.join("model.gguf");
        std::fs::write(&model_path, b"model-data").unwrap();
        std::fs::write(vault_dir.join("manifest.json"), "{\"models\":{}}").unwrap();

        let mut server = mockito::Server::new_async().await;
        let health_body = serde_json::json!({ "status": "ok" });
        let models_body = serde_json::json!({ "data": [{ "id": "gemma-4-12b" }] });
        server
            .mock("GET", "/health")
            .with_body(health_body.to_string())
            .create_async()
            .await;
        server
            .mock("GET", "/v1/models")
            .with_body(models_body.to_string())
            .create_async()
            .await;

        let port = server
            .host_with_port()
            .split(':')
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap();

        let identity = ModelManager::verify_server_identity(
            port,
            model_path.to_str().unwrap(),
            vault_dir.to_str().unwrap(),
            Some(ModelManager::sha256_file(&model_path).unwrap()),
        )
        .await;
        assert!(
            identity.is_err(),
            "Strict policy must reject a manifest that declares no model hash"
        );
    }

    #[test]
    fn reset_to_error_kills_child_and_sets_error_status() {
        // Spawn a long-running child that we can then force-kill via reset_to_error.
        let mut child = if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/C", "timeout /t 30 > nul"])
                .spawn()
                .expect("spawn cmd")
        } else {
            std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("spawn sleep")
        };

        let manager = ModelManager::new();
        let reason = manager.reset_to_error(&mut child, "test failure".to_string());

        assert!(reason.contains("test failure"));
        assert_eq!(manager.get_status(), ModelStatus::Error);
        assert!(manager.llama_process.lock().unwrap().is_none());

        // The child should be dead.
        assert!(
            child.try_wait().unwrap().is_some(),
            "Child process should be killed"
        );
    }

    #[tokio::test]
    async fn start_server_fails_when_binary_missing() {
        let manager = ModelManager::new();
        manager.set_backend(AccelerationBackend::Cpu);
        let config = ModelConfig {
            model_path: String::from("/nonexistent/model.gguf"),
            ..Default::default()
        };

        // Use a vault root that does not contain any llama-server binary.
        let vault_root = std::env::temp_dir().join("unoone-no-binary-test");
        let _ = std::fs::remove_dir_all(&vault_root);
        std::fs::create_dir_all(&vault_root).unwrap();
        std::fs::create_dir_all(vault_root.join("RUNTIMES").join("WINDOWS").join("CPU")).unwrap();

        let result = manager
            .start_server(
                &config,
                vault_root.to_str().unwrap(),
                &inbharat_harness_core::CancellationToken::new(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("llama-server not found"));
        assert_eq!(manager.get_status(), ModelStatus::Error);
    }

    #[tokio::test]
    async fn start_server_fails_when_model_missing() {
        let manager = ModelManager::new();
        manager.set_backend(AccelerationBackend::Cpu);

        // Create a minimal fake llama-server binary path so the binary check passes,
        // but the model file does not exist. Use platform-specific runtime layout.
        let vault_root = std::env::temp_dir().join("unoone-no-model-test");
        let _ = std::fs::remove_dir_all(&vault_root);
        let (runtime_dir, binary_name) = if cfg!(target_os = "macos") {
            (
                vault_root.join("RUNTIMES").join("MACOS").join("METAL"),
                "llama-server",
            )
        } else if cfg!(target_os = "windows") {
            (
                vault_root.join("RUNTIMES").join("WINDOWS").join("CPU"),
                "llama-server.exe",
            )
        } else {
            (
                vault_root.join("RUNTIMES").join("LINUX").join("CPU"),
                "llama-server",
            )
        };
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let binary_path = runtime_dir.join(binary_name);
        std::fs::write(&binary_path, b"fake binary").unwrap();

        let config = ModelConfig {
            model_path: String::from("/nonexistent/model.gguf"),
            ..Default::default()
        };

        let result = manager
            .start_server(
                &config,
                vault_root.to_str().unwrap(),
                &inbharat_harness_core::CancellationToken::new(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Model file not found"));
        assert_eq!(manager.get_status(), ModelStatus::Error);
    }

    #[test]
    fn manual_model_stop_cancels_loading_and_preserves_restart_admission() {
        let state = ModelManagerState::new();
        let pending = state.startup_cancel.lock().unwrap().clone();
        state.cancel_pending_start();
        assert!(pending.is_cancelled());
        assert!(!state.startup_cancel.lock().unwrap().is_cancelled());
        assert!(!state.suspended.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn first_unlock_retains_early_boot_cancellation_and_reunlock_replaces_it() {
        let state = ModelManagerState::new();
        let early = state.startup_cancel.lock().unwrap().clone();
        state.resume();
        state.suspend();
        assert!(early.is_cancelled());
        let generation = state.generation.load(std::sync::atomic::Ordering::SeqCst);
        state.resume();
        assert!(!state.startup_cancel.lock().unwrap().is_cancelled());
        assert_ne!(
            generation,
            state.generation.load(std::sync::atomic::Ordering::SeqCst)
        );
    }

    #[test]
    fn cancelled_hash_never_reads_model_bytes() {
        let cancel = inbharat_harness_core::CancellationToken::new();
        cancel.cancel(inbharat_harness_core::CancelCause::User);
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(
            ModelManager::sha256_file_cancellable(file.path(), Some(&cancel))
                .unwrap_err()
                .contains("cancelled")
        );
    }

    #[test]
    fn desktop_discovery_and_selection_keep_tiers_and_projectors_separate() {
        let root = tempfile::tempdir().unwrap();
        let mut assets = Vec::new();
        for (id, size) in [
            ("gemma-4-12b", 7 * 1073741824_u64),
            ("gemma-4-e4b", 3 * 1073741824),
            ("gemma-4-e2b", 1610612736),
        ] {
            let path = format!("models/{id}.gguf");
            std::fs::create_dir_all(root.path().join("models")).unwrap();
            std::fs::File::create(root.path().join(&path))
                .unwrap()
                .set_len(size)
                .unwrap();
            assets.push(serde_json::json!({"id": id, "kind": "MODEL", "path": path, "size_bytes": size, "sha256": "a".repeat(64)}));
        }
        let projector = "models/mmproj-gemma-4-12b.gguf";
        std::fs::write(root.path().join(projector), b"projection").unwrap();
        assets.push(serde_json::json!({"id": "vision-projector", "kind": "MMPROJ", "path": projector, "size_bytes": 10, "sha256": "b".repeat(64)}));
        let manifest = serde_json::json!({
            "product_id": "test", "schema_version": 1, "pai_version": "0.1.0",
            "vault": {"id_path": "VAULT/id"},
            "platforms": {"windows": {"architectures": ["x86_64"],
            "desktop": {"id": "desktop", "kind": "DESKTOP_EXECUTABLE", "path": "desktop.exe", "size_bytes": 1, "sha256": "c".repeat(64)}, "models": assets}}
        });
        std::fs::write(
            root.path().join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let root = root.path().to_str().unwrap();
        let models = ModelManager::new().find_models(root);
        assert_eq!(models.len(), 3);
        assert!(models.iter().all(|model| model.available));
        assert!(models[0].mmproj_path.is_some());
        assert!(models[1].mmproj_path.is_none() && models[2].mmproj_path.is_none());
        assert_eq!(models[1].model_type, "gemma-4-e4b");
        assert_eq!(
            select_model_for_memory(root, 16.0, 12.0)
                .unwrap()
                .model
                .name,
            "gemma-4-12b"
        );
        assert_eq!(
            select_model_for_memory(root, 8.0, 6.0).unwrap().model.name,
            "gemma-4-e4b"
        );
        assert_eq!(
            select_model_for_memory(root, 4.0, 3.9).unwrap().model.name,
            "gemma-4-e2b"
        );
        assert!(select_model_for_memory(root, 32.0, 3.0).is_err());
    }

    // --- Host-disk model cache -------------------------------------------------

    fn write_cache_test_manifest(vault_dir: &std::path::Path, sha256: Option<&str>) {
        let mut model = serde_json::json!({ "path": "models/gemma.gguf" });
        if let Some(sha256) = sha256 {
            model["sha256"] = serde_json::json!(sha256);
        }
        let manifest = serde_json::json!({
            "models": { "desktop": { "gemma": model } }
        });
        std::fs::write(
            vault_dir.join("manifest.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
    }

    fn cache_test_fixture(
        name: &str,
        sha256: Option<&str>,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let vault_dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&vault_dir);
        std::fs::create_dir_all(vault_dir.join("models")).unwrap();
        let model_path = vault_dir.join("models").join("gemma.gguf");
        std::fs::write(&model_path, b"model bytes for cache staging").unwrap();
        write_cache_test_manifest(&vault_dir, sha256);
        (vault_dir, model_path)
    }

    #[test]
    fn stage_model_to_host_cache_verifies_digest_and_publishes() {
        let (vault_dir, model_path) = cache_test_fixture("unoone-cache-stage-test", None);
        let sha = ModelManager::sha256_file(&model_path).unwrap();
        write_cache_test_manifest(&vault_dir, Some(&sha));

        let (cached, size) =
            stage_model_to_host_cache(model_path.to_str().unwrap(), vault_dir.to_str().unwrap())
                .expect("staging should succeed");
        assert_eq!(size, std::fs::metadata(&model_path).unwrap().len());
        assert!(cached.is_file(), "the cached copy must exist");
        assert!(
            cached.file_name().unwrap().to_str().unwrap() == format!("{}.gguf", sha),
            "the cache entry must be keyed by the manifest sha256"
        );
        // The verification marker must exist alongside.
        assert!(cached
            .parent()
            .unwrap()
            .join(format!("{}.verified", sha))
            .is_file());

        // The cached path must resolve back to the manifest-expected hash via
        // the cache-path branch of read_manifest_model_hash.
        let resolved = ModelManager::read_manifest_model_hash(
            vault_dir.to_str().unwrap(),
            cached.to_str().unwrap(),
        )
        .expect("cache path should resolve to its filename hash");
        assert_eq!(resolved, sha);

        // A second staging call must hit the cheap verified path: same file.
        let (again, again_size) =
            stage_model_to_host_cache(model_path.to_str().unwrap(), vault_dir.to_str().unwrap())
                .expect("re-staging an unchanged copy should be cheap and succeed");
        assert_eq!(again, cached);
        assert_eq!(again_size, size);

        let _ = std::fs::remove_dir_all(&vault_dir);
        let _ = std::fs::remove_file(&cached);
        let _ = std::fs::remove_file(cached.parent().unwrap().join(format!("{}.verified", sha)));
    }

    #[test]
    fn stage_model_refuses_model_without_manifest_hash() {
        let (vault_dir, model_path) = cache_test_fixture("unoone-cache-nohash-test", None);
        let error =
            stage_model_to_host_cache(model_path.to_str().unwrap(), vault_dir.to_str().unwrap())
                .expect_err("staging must fail closed without a manifest hash");
        assert!(
            error.contains("Refusing to cache"),
            "expected a fail-closed refusal, got: {error}"
        );
        let _ = std::fs::remove_dir_all(&vault_dir);
    }

    #[test]
    fn verified_host_cache_release_is_path_and_marker_bound() {
        // The BootGate may release model boot ONLY for a path inside the
        // digest-verified host cache whose marker is intact. A drive path or
        // an unmarked cache file must not pass — those still require the
        // full DesktopLaunch sweep.
        let (vault_dir, model_path) = cache_test_fixture("unoone-cache-release-test", None);
        // Unique bytes so this test cannot collide with the other cache
        // tests' entries (the cache is keyed by content sha256).
        std::fs::write(&model_path, b"boot-gate release test bytes").unwrap();
        let sha = ModelManager::sha256_file(&model_path).unwrap();
        write_cache_test_manifest(&vault_dir, Some(&sha));

        let (cached, _size) =
            stage_model_to_host_cache(model_path.to_str().unwrap(), vault_dir.to_str().unwrap())
                .expect("staging should succeed");
        assert!(model_served_from_verified_host_cache(
            cached.to_str().unwrap()
        ));
        // The drive copy is not a host-cache path.
        assert!(!model_served_from_verified_host_cache(
            model_path.to_str().unwrap()
        ));
        // A random path outside the cache never passes.
        assert!(!model_served_from_verified_host_cache(
            "C:\\Windows\\System32\\drivers\\etc\\hosts"
        ));
        // Losing the marker (e.g. manual tampering) revokes release.
        let marker = cached.parent().unwrap().join(format!("{sha}.verified"));
        let backup = cached.parent().unwrap().join(format!("{sha}.verified.bak"));
        std::fs::rename(&marker, &backup).unwrap();
        assert!(!model_served_from_verified_host_cache(
            cached.to_str().unwrap()
        ));
        std::fs::rename(&backup, &marker).unwrap();

        let _ = std::fs::remove_dir_all(&vault_dir);
        let _ = std::fs::remove_file(&cached);
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn stage_model_rejects_digest_mismatch_and_leaves_nothing_behind() {
        let (vault_dir, model_path) =
            cache_test_fixture("unoone-cache-mismatch-test", Some(&"0".repeat(64)));
        let error =
            stage_model_to_host_cache(model_path.to_str().unwrap(), vault_dir.to_str().unwrap())
                .expect_err("a digest mismatch must fail staging");
        assert!(
            error.contains("does not match the manifest sha256"),
            "expected the digest-mismatch error, got: {error}"
        );
        // Neither the published copy nor a leftover .part may survive.
        let cache_dir = model_cache_dir().unwrap();
        assert!(!cache_dir.join(format!("{}.gguf", "0".repeat(64))).is_file());
        assert!(!cache_dir.join(format!("{}.part", "0".repeat(64))).is_file());
        let _ = std::fs::remove_dir_all(&vault_dir);
    }

    /// A speech model (SPEECH/models/…, AssetKind::SpeechModel — a separate
    /// manifest section from desktop models) must resolve its manifest
    /// sha256 and stage into the host cache exactly like a desktop model:
    /// the speech plane serves inference from the same digest-verified
    /// cache.
    #[test]
    fn read_manifest_model_hash_and_stage_cover_speech_models() {
        let dir = tempfile::tempdir().unwrap();
        let vault_dir = dir.path().join("unoone-speech-cache-test");
        let _ = std::fs::remove_dir_all(&vault_dir);
        let model_rel = "SPEECH/models/tts/omnivoice.gguf";
        std::fs::create_dir_all(vault_dir.join("SPEECH/models/tts")).unwrap();
        let model_path = vault_dir.join(model_rel);
        // Unique bytes so the cache key can never collide with other tests.
        std::fs::write(&model_path, b"speech model bytes unique to the cache test").unwrap();
        let sha = ModelManager::sha256_file(&model_path).unwrap();
        let size = std::fs::metadata(&model_path).unwrap().len();

        let manifest = serde_json::json!({
            "product_id": "unoone-pai",
            "schema_version": 2,
            "pai_version": "cache-test",
            "vault": { "id_path": "VAULT/identity/vault.id" },
            "platforms": { "windows": {
                "architectures": ["x64"],
                "desktop": {
                    "id": "desktop-exe",
                    "kind": "DESKTOP_EXECUTABLE",
                    "path": "APPS/WINDOWS/UnoOnePower.exe",
                    "size_bytes": 1,
                    "sha256": "00"
                },
                "speech": { "models": [ {
                    "id": "speech-model",
                    "kind": "SPEECH_MODEL",
                    "path": model_rel,
                    "size_bytes": size,
                    "sha256": sha
                } ] }
            } }
        });
        std::fs::write(
            vault_dir.join("manifest.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();

        // The absolute on-disk path must resolve to the speech section's
        // sha256 (typed PocketManifest branch, not the JSON fallback).
        let resolved = ModelManager::read_manifest_model_hash(
            vault_dir.to_str().unwrap(),
            model_path.to_str().unwrap(),
        )
        .expect("speech model hash must resolve from the speech manifest section");
        assert_eq!(resolved, sha);

        // And the model must stage into the host cache keyed by that sha.
        let (cached, staged_size) =
            stage_model_to_host_cache(model_path.to_str().unwrap(), vault_dir.to_str().unwrap())
                .expect("speech model staging must succeed");
        assert_eq!(staged_size, size);
        assert!(cached.is_file());
        assert_eq!(
            cached.file_name().unwrap().to_str().unwrap(),
            format!("{sha}.gguf"),
            "the speech cache entry must be keyed by the manifest sha256"
        );
        assert!(cached
            .parent()
            .unwrap()
            .join(format!("{sha}.verified"))
            .is_file());
        // Re-staging must take the cheap verified path and leave the file.
        let (again, _) =
            stage_model_to_host_cache(model_path.to_str().unwrap(), vault_dir.to_str().unwrap())
                .expect("re-staging a verified speech copy must succeed");
        assert_eq!(again, cached);
    }
}

// Tauri command wrappers

#[tauri::command]
pub fn list_models(vault_root: String) -> Result<Vec<ModelInfo>, String> {
    let _probe = crate::boot_trace::step("list_models");
    let manager = ModelManager::new();
    Ok(manager.find_models(&vault_root))
}

#[derive(Serialize)]
pub struct DesktopModelSelection {
    pub model: ModelInfo,
    pub config: ModelConfig,
    pub reason: String,
}

fn available_memory() -> Result<(f64, f64), String> {
    let memory =
        sys_info::mem_info().map_err(|e| format!("Cannot measure available memory: {e}"))?;
    let total = memory.total as f64 / (1024.0 * 1024.0);
    // sys-info reports avail=0 on some platforms; free is a conservative fallback.
    let available = if memory.avail > 0 {
        memory.avail
    } else {
        memory.free
    };
    let available = available as f64 / (1024.0 * 1024.0);
    if total <= 0.0 || available <= 0.0 {
        return Err("Available memory could not be measured".to_owned());
    }
    Ok((total, available))
}

fn admit_model(model: &ModelInfo, config: &ModelConfig, total: f64, available: f64) -> bool {
    let Some(tier) = crate::desktop_model_policy::tier(&model.name) else {
        return false;
    };
    let weights = std::fs::metadata(&model.path)
        .ok()
        .map(|m| m.len() as f64 / 1073741824.0)
        .unwrap_or(0.0);
    let projector = match config.mmproj_path.as_ref() {
        Some(path) => match std::fs::metadata(path) {
            Ok(m) => m.len() as f64 / 1073741824.0,
            Err(_) => return false,
        },
        None => 0.0,
    };
    let context = get_context_budget(
        config.model_path.clone(),
        config.context_size,
        config.cache_type_k.clone(),
    );
    let kv = context
        .kv_estimate_bytes
        .map(|b| b as f64 / 1073741824.0)
        .unwrap_or(1.0);
    crate::desktop_model_policy::fits(tier, total, available, weights, projector, kv)
}

#[tauri::command]
pub fn select_desktop_model(vault_root: String) -> Result<DesktopModelSelection, String> {
    // A self-declared filename or an arbitrary scanned GGUF is not a qualified
    // desktop tier. Only the validated package's declared model lane is eligible.
    let manifest = std::fs::read_to_string(PathBuf::from(&vault_root).join("manifest.json"))
        .map_err(|e| e.to_string())?;
    serde_json::from_str::<unoone_usb_manifest::PocketManifest>(&manifest)
        .map_err(|_| "Desktop boot requires the current Pocket AI manifest".to_owned())?;
    let (total, available) = available_memory()?;
    select_model_for_memory(&vault_root, total, available)
}

fn select_model_for_memory(
    vault_root: &str,
    total: f64,
    available: f64,
) -> Result<DesktopModelSelection, String> {
    let mut models = ModelManager::new().find_models(vault_root);
    models.retain(|model| {
        model.available && crate::desktop_model_policy::tier(&model.name).is_some()
    });
    models.sort_by_key(|model| std::cmp::Reverse(crate::desktop_model_policy::tier(&model.name)));
    for model in models {
        let defaults = get_model_config();
        let mut contexts = vec![defaults.context_size, 16384, 8192, 4096];
        contexts.retain(|context| *context <= defaults.context_size);
        contexts.sort_unstable_by(|a, b| b.cmp(a));
        contexts.dedup();
        for context_size in contexts {
            let mut config = defaults.clone();
            config.context_size = context_size;
            config.model_path = model.path.clone();
            config.mmproj_path = model.mmproj_path.clone();
            if admit_model(&model, &config, total, available) {
                return Ok(DesktopModelSelection {
                    reason: format!(
                        "{} selected with {:.1} GiB available RAM and a {}-token requested context",
                        model.name, available, context_size
                    ),
                    model,
                    config,
                });
            }
        }
    }
    Err(format!("No declared desktop model fits the current {:.1} GiB available RAM budget. Close other apps or stage a qualified smaller model in the package.", available))
}

#[tauri::command]
pub fn detect_acceleration(
    startup: tauri::State<'_, crate::startup::StartupCoordinator>,
) -> Vec<AccelerationBackend> {
    let _probe = crate::boot_trace::step("detect_acceleration");
    startup.set_phase_if_booting(crate::startup::StartupPhase::SelectingBackend);
    let manager = ModelManager::new();
    manager.detect_backends()
}

#[tauri::command]
pub fn get_model_config() -> ModelConfig {
    // Host-adaptive boot defaults. Long agent sessions (multi-file coding
    // with accumulated tool results) need a large context; the shipped
    // 32K + q8_0 KV lane was verified live on the target laptop class
    // (RTX 5050). Stepped by detected RAM so weak hosts still get a
    // working server; the Model view lets the user override at any time.
    // `start_server` additionally clamps this value against the loaded
    // artifact's own trained context — see gguf_meta::derive_context_budget.
    // 32K + q8_0 KV lane was verified live on the target laptop class
    // (RTX 5050). Stepped by detected RAM so weak hosts still get a
    // working server; the Model view lets the user override at any time.
    let mut config = ModelConfig::default();
    match detected_ram_gib() {
        Some(ram) if ram >= 24 => {
            config.context_size = 32768;
            config.cache_type_k = Some("q8_0".to_owned());
            config.cache_type_v = Some("q8_0".to_owned());
        }
        Some(ram) if ram >= 12 => {
            config.context_size = 16384;
            config.cache_type_k = Some("q8_0".to_owned());
            config.cache_type_v = Some("q8_0".to_owned());
        }
        // Unknown host or < 12 GiB: keep the safe 4096 f16 baseline.
        _ => {}
    }
    config
}

/// Universal-adaptive context: the exact derivation the server launcher will
/// apply, surfaced to the UI so the Model panel can state the granted
/// context and the reason for every clamp BEFORE a session starts.
#[tauri::command]
pub fn get_context_budget(
    model_path: String,
    requested_context: u32,
    cache_type_k: Option<String>,
) -> crate::gguf_meta::ContextBudget {
    let meta = crate::gguf_meta::read_metadata(&model_path).ok();
    crate::gguf_meta::derive_context_budget(
        meta.as_ref(),
        requested_context,
        detected_ram_gib().map(|gib| gib as u64),
        cache_type_k.as_deref(),
    )
}

#[tauri::command]
pub async fn get_model_status(
    state: tauri::State<'_, ModelManagerState>,
) -> Result<String, String> {
    let Ok(manager) = state.manager.try_lock() else {
        return Ok("LOADING".to_owned());
    };
    let Some(manager) = manager.as_ref() else {
        return Ok("NOT_LOADED".to_string());
    };

    // Report the verified status held by the ModelManager.
    let status = manager.get_status();
    Ok(match status {
        ModelStatus::NotLoaded => "NOT_LOADED",
        ModelStatus::Loading => "LOADING",
        ModelStatus::Loaded => {
            // Double-check the managed identity is still present.
            let identity = manager.server_identity.lock().unwrap();
            if identity.is_some() {
                "LOADED"
            } else {
                "PENDING_VERIFICATION"
            }
        }
        ModelStatus::Generating => "GENERATING",
        ModelStatus::Error => "ERROR",
    }
    .to_string())
}

/// D1: Start llama-server on a free port, verify its identity, and store it in state.
fn manager_model_for_config(vault_root: &str, config: &ModelConfig) -> Result<ModelInfo, String> {
    let requested = ModelManager::read_manifest_model_hash(vault_root, &config.model_path)
        .ok_or("Model is not declared in the package")?;
    let model = ModelManager::new()
        .find_models(vault_root)
        .into_iter()
        .find(|model| {
            model.available
                && ModelManager::read_manifest_model_hash(vault_root, &model.path).as_ref()
                    == Some(&requested)
        })
        .ok_or("Model or cache entry does not match a declared desktop model")?;
    if config.mmproj_path != model.mmproj_path {
        return Err("Projection artifact must match the selected model's declared tier".to_owned());
    }
    Ok(model)
}

#[tauri::command]
pub async fn start_model_server(
    config: ModelConfig,
    vault_root: String,
    state: tauri::State<'_, ModelManagerState>,
    startup: tauri::State<'_, crate::startup::StartupCoordinator>,
) -> Result<u16, String> {
    use std::sync::atomic::Ordering;
    if state.suspended.load(Ordering::SeqCst) {
        return Err("Unlock Pocket AI before restarting inference".to_owned());
    }
    let generation = state.generation.load(Ordering::SeqCst);
    let cancel = state
        .startup_cancel
        .lock()
        .map_err(|_| "Startup cancellation lock failed")?
        .clone();
    crate::boot_trace::mark_detail("start_model_server: entry", &config.model_path);
    // The model server is the inference gate, in two tiers:
    //   1. Full DesktopLaunch sweep complete → any model (drive or cache).
    //   2. BootGate complete (identity + runtime executables verified) → only
    //      a digest-verified host-cache copy. The cached copy's own bytes are
    //      hash-verified by `start_server` (disk sha256 compared against the
    //      manifest) before the server is trusted, so inference still never
    //      runs on unverified bytes — the gate only moves WHEN the multi-GB
    //      asset sweep must finish relative to model boot.
    let declared = manager_model_for_config(&vault_root, &config)?;
    let (total, available) = available_memory()?;
    if !admit_model(&declared, &config, total, available) {
        return Err(
            "Selected model and context exceed the current desktop memory budget".to_owned(),
        );
    }
    let serving_verified_host_cache = model_served_from_verified_host_cache(&config.model_path);
    // Two-tier gate (see the comment above); the release condition reads
    // cleanly as: full sweep done, or (boot gate done AND cached model).
    let gate_deadline = Instant::now() + Duration::from_secs(20 * 60);
    loop {
        cancel
            .check("desktop.model.validation")
            .map_err(|e| e.to_string())?;
        if state.suspended.load(Ordering::SeqCst)
            || generation != state.generation.load(Ordering::SeqCst)
        {
            return Err("Model startup was cancelled".to_owned());
        }
        let boot_released = startup.is_boot_gate_complete() && serving_verified_host_cache;
        if startup.is_asset_validation_complete() || boot_released {
            break;
        }
        if startup.validation_failed()
            || !PathBuf::from(&vault_root).join("manifest.json").is_file()
        {
            return Err("Pocket AI assets failed validation or the drive was removed".to_owned());
        }
        if Instant::now() >= gate_deadline {
            return Err("Pocket AI asset validation timed out".to_owned());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    // Serialize start/stop only after validation. Invalid requests leave the
    // current server intact, and cached boot does not await the full sweep.
    let mut active_manager = state.manager.lock().await;
    if state.suspended.load(Ordering::SeqCst)
        || generation != state.generation.load(Ordering::SeqCst)
    {
        return Err("Model startup was cancelled".to_owned());
    }
    // RAM pressure may have changed during a long removable-drive sweep.
    let (total, available) = available_memory()?;
    if !admit_model(&declared, &config, total, available) {
        return Err(
            "Memory pressure changed while validating the drive; select a smaller model or context"
                .to_owned(),
        );
    }
    crate::boot_trace::mark_detail(
        "start_model_server: gate passed",
        &format!(
            "full_sweep={} boot_gate={} host_cache={}",
            startup.is_asset_validation_complete(),
            startup.is_boot_gate_complete(),
            serving_verified_host_cache
        ),
    );
    if let Some(previous) = active_manager.take() {
        previous.stop_server()?;
    }
    startup.set_phase(crate::startup::StartupPhase::StartingModel);
    let manager = ModelManager::new();
    // Default to the best detected backend.
    let best_backend = manager
        .detect_backends()
        .into_iter()
        .next()
        .unwrap_or(AccelerationBackend::Cpu);
    manager.set_backend(best_backend);

    let port = match manager.start_server(&config, &vault_root, &cancel).await {
        Ok(port) => port,
        Err(error) => {
            if generation == state.generation.load(Ordering::SeqCst) {
                startup.set_phase(crate::startup::StartupPhase::LimitedMode);
            }
            return Err(error);
        }
    };
    *state
        .server_port
        .lock()
        .map_err(|e| format!("State lock error: {}", e))? = port;
    if state.suspended.load(Ordering::SeqCst)
        || generation != state.generation.load(Ordering::SeqCst)
    {
        let _ = manager.stop_server();
        return Err("Model startup was cancelled by lock or disconnect".to_owned());
    }
    *active_manager = Some(manager);
    startup.set_phase(crate::startup::StartupPhase::VerifyingModel);
    Ok(port)
}

/// Proper health check using reqwest instead of raw TCP.
/// Tries the configured UnoOne port first, then falls back to Ollama on 11434.
#[tauri::command]
pub async fn check_model_health(
    state: tauri::State<'_, ModelManagerState>,
    startup: tauri::State<'_, crate::startup::StartupCoordinator>,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();

    let uno_port = *state
        .server_port
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let verified_model_id = {
        let manager = state.manager.lock().await;
        let manager = manager
            .as_ref()
            .ok_or_else(|| "UnoOne model manager is not initialized".to_string())?;
        let identity = manager
            .server_identity
            .lock()
            .map_err(|e| format!("Identity lock error: {e}"))?;
        identity
            .as_ref()
            .map(|identity| identity.model_id.clone())
            .filter(|model_id| !model_id.is_empty())
            .ok_or_else(|| "UnoOne model identity has not been verified".to_string())?
    };

    // Try the active UnoOne port first.
    let response = client
        .get(format!("http://127.0.0.1:{}/health", uno_port))
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse health response: {}", e))?;
            startup.set_phase(crate::startup::StartupPhase::Ready);
            Ok(serde_json::json!({
                "backend": "llama-server",
                "port": uno_port,
                "model_id": verified_model_id,
                "health": body,
            }))
        }
        _ => {
            startup.set_phase(crate::startup::StartupPhase::LimitedMode);
            Err(format!(
                "The managed UnoOne llama-server on port {uno_port} is not healthy"
            ))
        }
    }
}

/// D1: Stop the currently managed llama-server process and clear state.
#[tauri::command]
pub async fn stop_model_server(state: tauri::State<'_, ModelManagerState>) -> Result<(), String> {
    state.cancel_pending_start();
    let mut manager = state.manager.lock().await;
    if let Some(manager) = manager.take() {
        manager.stop_server()?;
    }
    *state
        .server_port
        .lock()
        .map_err(|e| format!("State lock error: {}", e))? = 8342;
    Ok(())
}

// ---------------------------------------------------------------------------
// Host-disk model cache
//
// Models live on the removable USB drive, where sequential read throughput is
// the launch bottleneck. This cache streams a model from the drive to the
// host disk ONCE (per-platform user cache dir — see model_cache_dir), verifying the digest
// against the manifest in the same single pass; later launches load from the
// host SSD/NVMe instead. Cache entries are keyed by the manifest sha256, so
// a cached copy is only ever used after its bytes have been proven to match
// the manifest — no unverified model bytes ever reach the host, and the
// drive copy remains the canonical source.
// ---------------------------------------------------------------------------

/// Resolve the host-disk model cache directory, per-platform:
/// Windows `%LOCALAPPDATA%\UnoOne\model-cache`, macOS
/// `~/Library/Caches/UnoOne/model-cache`, Linux
/// `$XDG_CACHE_HOME/UnoOne/model-cache` (default `~/.cache/...`).
pub(crate) fn model_cache_dir() -> Result<PathBuf, String> {
    if cfg!(target_os = "windows") {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| "LOCALAPPDATA is not set; cannot locate the model cache".to_string())?;
        Ok(base.join("UnoOne").join("model-cache"))
    } else if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set; cannot locate the model cache".to_string())?;
        Ok(home
            .join("Library")
            .join("Caches")
            .join("UnoOne")
            .join("model-cache"))
    } else {
        // Linux: XDG cache, with the standard ~/.cache fallback.
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".cache"))
            })
            .ok_or_else(|| {
                "Neither XDG_CACHE_HOME nor HOME is set; cannot locate the model cache".to_string()
            })?;
        Ok(base.join("UnoOne").join("model-cache"))
    }
}

/// Marker value (size:mtime) for a cached file, so an unchanged verified copy
/// never needs to be re-hashed.
fn model_cache_marker_value(path: &std::path::Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let size = metadata.len();
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(format!("{}:{}", size, mtime))
}

/// True when a cached copy exists, has a verification marker, and has not
/// been touched since the marker was written.
pub(crate) fn model_cache_is_verified(cached: &std::path::Path, marker: &std::path::Path) -> bool {
    if !cached.is_file() || !marker.is_file() {
        return false;
    }
    let Some(current) = model_cache_marker_value(cached) else {
        return false;
    };
    match std::fs::read_to_string(marker) {
        Ok(recorded) => recorded.trim() == current,
        Err(_) => false,
    }
}

/// True when `model_path` points at a digest-verified copy in the host model
/// cache: the file lives in the cache directory, is keyed by its manifest
/// sha256 (the filename), and carries a `.verified` marker matching its
/// current size+mtime. `start_server` still re-hashes the actual bytes
/// against the manifest before the server is trusted, so this is a
/// release-timing fact, not the integrity check itself — it decides whether
/// the BootGate may start the model before the full asset sweep finishes.
fn model_served_from_verified_host_cache(model_path: &str) -> bool {
    let Ok(cache_dir) = model_cache_dir() else {
        return false;
    };
    let path = PathBuf::from(model_path);
    if !path.starts_with(&cache_dir) {
        return false;
    }
    let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    let marker = cache_dir.join(format!("{name}.verified"));
    model_cache_is_verified(&path, &marker)
}

/// Stream a manifest-vouched model from the drive to the host cache, hashing
/// in the same single pass. Returns the cached path and byte count.
pub(crate) fn stage_model_to_host_cache(
    model_path: &str,
    vault_root: &str,
) -> Result<(PathBuf, u64), String> {
    use sha2::Digest;
    use std::io::{Read, Write};

    let source = PathBuf::from(model_path);
    if !source.is_file() {
        return Err(format!("Model file not found: {}", source.display()));
    }
    // Fail closed: without a manifest hash there is nothing to verify the
    // cached copy against, so we refuse to put it on the host disk at all.
    let expected_sha = ModelManager::read_manifest_model_hash(vault_root, model_path)
        .filter(|hash| !hash.is_empty())
        .ok_or_else(|| {
            "Refusing to cache this model: the manifest records no sha256 for it".to_string()
        })?;

    let cache_dir = model_cache_dir()?;
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("Failed to create {}: {}", cache_dir.display(), e))?;
    let cached = cache_dir.join(format!("{}.gguf", expected_sha));
    let marker = cache_dir.join(format!("{}.verified", expected_sha));

    // Cheap path: a previously verified copy that has not changed since.
    if model_cache_is_verified(&cached, &marker) {
        let size = std::fs::metadata(&cached).map(|m| m.len()).unwrap_or(0);
        return Ok((cached, size));
    }

    // Stream-copy from the removable drive, hashing in the same single pass.
    // A separate verify pass would double the multi-GB read from the drive.
    let part = cache_dir.join(format!("{}.part", expected_sha));
    let mut input = std::fs::File::open(&source)
        .map_err(|e| format!("Failed to open {}: {}", source.display(), e))?;
    let mut output = std::fs::File::create(&part)
        .map_err(|e| format!("Failed to create {}: {}", part.display(), e))?;
    let mut hasher = sha2::Sha256::new();
    // Heap buffer, 512 KiB — mirrors sha256_file: multi-GB streams must not
    // turn into millions of syscalls on removable media.
    let mut buffer = vec![0u8; 512 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = input
            .read(&mut buffer)
            .map_err(|e| format!("Failed to read {}: {}", source.display(), e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        output
            .write_all(&buffer[..n])
            .map_err(|e| format!("Failed to write {}: {}", part.display(), e))?;
        total += n as u64;
    }
    output
        .flush()
        .map_err(|e| format!("Failed to flush {}: {}", part.display(), e))?;
    drop(output);

    let digest = hex::encode(hasher.finalize());
    if digest != expected_sha {
        let _ = std::fs::remove_file(&part);
        return Err("The cached copy does not match the manifest sha256 — the model bytes changed or the drive read failed. Nothing was staged.".to_string());
    }
    let source_size = std::fs::metadata(&source)
        .map(|m| m.len())
        .map_err(|e| format!("Failed to stat {}: {}", source.display(), e))?;
    if total != source_size {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "Truncated copy: staged {} bytes but the model is {} bytes. Nothing was staged.",
            total, source_size
        ));
    }
    // Atomic publish: rename the .part into place, then write the marker.
    std::fs::rename(&part, &cached)
        .map_err(|e| format!("Failed to publish {}: {}", cached.display(), e))?;
    let marker_value = model_cache_marker_value(&cached).unwrap_or_default();
    std::fs::write(&marker, &marker_value)
        .map_err(|e| format!("Failed to write {}: {}", marker.display(), e))?;
    Ok((cached, total))
}

/// Cheap probe: is this model already staged (and still verified) on the
/// host disk? Reads the manifest and stats two files — never hashes the
/// multi-GB model.
#[tauri::command]
pub async fn model_cache_status(
    model_path: String,
    vault_root: String,
) -> Result<serde_json::Value, String> {
    let expected_sha = ModelManager::read_manifest_model_hash(&vault_root, &model_path)
        .filter(|hash| !hash.is_empty())
        .ok_or_else(|| "The manifest records no sha256 for this model".to_string())?;
    let cache_dir = model_cache_dir()?;
    let cached = cache_dir.join(format!("{}.gguf", expected_sha));
    let marker = cache_dir.join(format!("{}.verified", expected_sha));
    let staged = model_cache_is_verified(&cached, &marker);
    crate::boot_trace::mark_detail("model_cache_status", &format!("staged={staged}"));
    Ok(serde_json::json!({
        "staged": staged,
        "cached_path": if staged {
            Some(cached.to_string_lossy().to_string())
        } else {
            None
        },
        "size_bytes": if staged {
            std::fs::metadata(&cached).ok().map(|m| m.len())
        } else {
            None
        },
        "sha256": expected_sha,
    }))
}

/// Heavy path: stream the model from the drive to the host cache (single
/// pass, digest-verified against the manifest). Blocking work runs off the
/// async runtime — the audio audit flagged blocking inside async commands.
#[tauri::command]
pub async fn stage_model_cache(
    model_path: String,
    vault_root: String,
) -> Result<serde_json::Value, String> {
    let (cached_path, size_bytes) =
        tokio::task::spawn_blocking(move || stage_model_to_host_cache(&model_path, &vault_root))
            .await
            .map_err(|e| format!("Model cache staging task failed: {}", e))??;
    Ok(serde_json::json!({
        "staged": true,
        "cached_path": cached_path.to_string_lossy().to_string(),
        "size_bytes": size_bytes,
    }))
}
