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
        }
    }
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
}

/// Verified server identity returned after a successful start.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ServerIdentity {
    pub port: u16,
    pub pid: u32,
    pub model_id: String,
    pub health: serde_json::Value,
}

/// Model info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub model_type: String,
    pub quantization: String,
    pub file_size_gb: f64,
    pub context_length: u32,
    pub available: bool,
    pub path: String,
    pub mmproj_path: Option<String>,
}

/// Model manager state
pub struct ModelManager {
    status: Mutex<ModelStatus>,
    backend: Mutex<AccelerationBackend>,
    llama_process: Mutex<Option<std::process::Child>>,
    /// Verified identity of the running llama-server process (PID, port, model id, health).
    server_identity: Mutex<Option<ServerIdentity>>,
}

impl ModelManager {
    pub fn new() -> Self {
        Self {
            status: Mutex::new(ModelStatus::NotLoaded),
            backend: Mutex::new(AccelerationBackend::Cpu),
            llama_process: Mutex::new(None),
            server_identity: Mutex::new(None),
        }
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
    fn sha256_file(path: &std::path::Path) -> Result<String, String> {
        use sha2::Digest;
        use std::io::Read;
        let mut file = std::fs::File::open(path)
            .map_err(|e| format!("Failed to open {} for hashing: {}", path.display(), e))?;
        let mut hasher = sha2::Sha256::new();
        let mut buffer = [0u8; 8192];
        loop {
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
    fn read_manifest_model_hash(vault_root: &str, model_path: &str) -> Option<String> {
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
        let health_body: serde_json::Value = health
            .json()
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
        let model_full_path = PathBuf::from(model_path);
        let disk_hash = if model_full_path.exists() {
            Some(Self::sha256_file(&model_full_path)?)
        } else {
            None
        };
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
            port,
            pid: 0, // Filled in by start_server after spawn.
            model_id,
            health: health_body,
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
                let mmproj_path = manifest
                    .platforms
                    .windows
                    .models
                    .iter()
                    .find(|asset| asset.kind == unoone_usb_manifest::AssetKind::Mmproj)
                    .map(|asset| {
                        PathBuf::from(vault_root)
                            .join(asset.path.replace('/', "\\"))
                            .to_string_lossy()
                            .to_string()
                    });
                for model in manifest
                    .platforms
                    .windows
                    .models
                    .iter()
                    .filter(|asset| asset.kind == unoone_usb_manifest::AssetKind::Model)
                {
                    let full_path = PathBuf::from(vault_root).join(model.path.replace('/', "\\"));
                    models.push(ModelInfo {
                        name: model.id.clone(),
                        model_type: "gemma-4-12b".to_string(),
                        quantization: "manifest-verified".to_string(),
                        file_size_gb: model.size_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                        context_length: 8192,
                        available: full_path.is_file(),
                        path: full_path.to_string_lossy().to_string(),
                        mmproj_path: mmproj_path.clone(),
                    });
                }
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
                                    context_length: 8192,
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
                                    context_length: 4096,
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
                                models.push(ModelInfo {
                                    name: "Gemma 4 12B Q4_K_M".to_string(),
                                    model_type: "gemma-4-12b".to_string(),
                                    quantization: "Q4_K_M".to_string(),
                                    file_size_gb: file_size,
                                    context_length: 8192,
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
                                models.push(ModelInfo {
                                    name: "Gemma 4 E2B Q4_K_M".to_string(),
                                    model_type: "gemma-4-e2b".to_string(),
                                    quantization: "Q4_K_M".to_string(),
                                    file_size_gb: file_size,
                                    context_length: 4096,
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub async fn start_server(
        &self,
        config: &ModelConfig,
        vault_root: &str,
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
        cmd.current_dir(&backend_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .args([
                "-m",
                &config.model_path,
                "--port",
                &port.to_string(),
                "-c",
                &config.context_size.to_string(),
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
        let mut child = cmd.spawn().map_err(|e| {
            *self.status.lock().unwrap() = ModelStatus::Error;
            format!("Failed to start llama-server: {}", e)
        })?;

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
                match Self::verify_server_identity(port, &config.model_path, vault_root).await {
                    Ok(mut identity) => {
                        identity.pid = pid;
                        *self.server_identity.lock().unwrap() = Some(identity);
                        *self.llama_process.lock().unwrap() = Some(child);
                        *self.status.lock().unwrap() = ModelStatus::Loaded;
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
    #[allow(dead_code)]
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
        *self.status.lock().unwrap() = ModelStatus::NotLoaded;
        Ok(())
    }

    /// Get current model status
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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

        // Build OpenAI-compatible request body
        let mut messages = Vec::new();
        if let Some(sys) = &request.system_prompt {
            messages.push(serde_json::json!({"role": "system", "content": sys}));
        }
        for turn in &request.conversation_history {
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
        });

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

        let client = reqwest::Client::new();
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
            tokens_per_second: 0.0,
            model_id: data
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("gemma-4-12b")
                .to_string(),
            tool_calls: final_tool_calls,
            finish_reason,
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
}

impl ModelManagerState {
    pub fn new() -> Self {
        Self {
            manager: tokio::sync::Mutex::new(None),
            server_port: std::sync::Mutex::new(8342),
        }
    }

    /// Emergency cleanup used when the Pocket AI is removed or the app exits.
    pub async fn emergency_stop(&self) {
        let manager = self.manager.lock().await;
        if let Some(manager) = manager.as_ref() {
            let _ = manager.stop_server();
        }
        if let Ok(mut port) = self.server_port.lock() {
            *port = 8342;
        }
    }

    pub fn emergency_stop_blocking(&self) {
        if let Ok(manager) = self.manager.try_lock() {
            if let Some(manager) = manager.as_ref() {
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

    #[test]
    fn find_free_port_returns_usable_local_port() {
        let port = ModelManager::find_free_port().expect("a free port should be available");
        assert!(port > 0);
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

        // Verify that the manifest hash mismatch is caught even though the server looks healthy.
        let result = ModelManager::verify_server_identity(
            port,
            model_path.to_str().unwrap(),
            vault_dir.to_str().unwrap(),
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
            .start_server(&config, vault_root.to_str().unwrap())
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
            .start_server(&config, vault_root.to_str().unwrap())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Model file not found"));
        assert_eq!(manager.get_status(), ModelStatus::Error);
    }
}

// Tauri command wrappers

#[tauri::command]
pub fn list_models(vault_root: String) -> Result<Vec<ModelInfo>, String> {
    let manager = ModelManager::new();
    Ok(manager.find_models(&vault_root))
}

#[tauri::command]
pub fn detect_acceleration(
    startup: tauri::State<'_, crate::startup::StartupCoordinator>,
) -> Vec<AccelerationBackend> {
    startup.set_phase(crate::startup::StartupPhase::SelectingBackend);
    let manager = ModelManager::new();
    manager.detect_backends()
}

#[tauri::command]
pub fn get_model_config() -> ModelConfig {
    ModelConfig::default()
}

#[tauri::command]
pub async fn get_model_status(
    state: tauri::State<'_, ModelManagerState>,
) -> Result<String, String> {
    let manager = state.manager.lock().await;
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
#[tauri::command]
pub async fn start_model_server(
    config: ModelConfig,
    vault_root: String,
    state: tauri::State<'_, ModelManagerState>,
    startup: tauri::State<'_, crate::startup::StartupCoordinator>,
) -> Result<u16, String> {
    startup.set_phase(crate::startup::StartupPhase::StartingModel);
    let manager = ModelManager::new();
    // Default to the best detected backend.
    let best_backend = manager
        .detect_backends()
        .into_iter()
        .next()
        .unwrap_or(AccelerationBackend::Cpu);
    manager.set_backend(best_backend);

    let port = match manager.start_server(&config, &vault_root).await {
        Ok(port) => port,
        Err(error) => {
            startup.set_phase(crate::startup::StartupPhase::LimitedMode);
            return Err(error);
        }
    };
    *state
        .server_port
        .lock()
        .map_err(|e| format!("State lock error: {}", e))? = port;
    *state.manager.lock().await = Some(manager);
    startup.set_phase(crate::startup::StartupPhase::VerifyingModel);
    Ok(port)
}

/// D1: Send a chat completion request to llama-server. Async because it uses reqwest.
#[tauri::command]
pub async fn send_chat_completion(
    request: InferenceRequest,
    state: tauri::State<'_, ModelManagerState>,
) -> Result<InferenceResponse, String> {
    let port = *state
        .server_port
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let manager = state.manager.lock().await;
    let manager = manager.as_ref().ok_or("Model manager not initialized")?;
    manager.send_completion(&request, port).await
}

/// D1: Proper health check using reqwest instead of raw TCP.
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
    let manager = state.manager.lock().await;
    if let Some(manager) = manager.as_ref() {
        manager.stop_server()?;
    }
    *state
        .server_port
        .lock()
        .map_err(|e| format!("State lock error: {}", e))? = 8342;
    Ok(())
}

/// Return only the directly managed, manifest-verified llama-server.
/// Host-installed Ollama and LM Studio are deliberately not trusted fallbacks.
#[tauri::command]
pub async fn detect_inference_backend(
    state: tauri::State<'_, ModelManagerState>,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();

    let uno_port = *state
        .server_port
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    let identity = {
        let manager_guard = state.manager.lock().await;
        let manager = manager_guard
            .as_ref()
            .ok_or_else(|| "The UnoOne model server has not been started".to_string())?;
        let verified_identity = manager
            .server_identity
            .lock()
            .map_err(|e| format!("Identity lock error: {e}"))?
            .clone()
            .ok_or_else(|| "The UnoOne model server identity is not verified".to_string())?;
        verified_identity
    };

    if let Ok(resp) = client
        .get(format!("http://127.0.0.1:{uno_port}/v1/models"))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
    {
        if resp.status().is_success() {
            return Ok(serde_json::json!({
                "backend": "llama-server",
                "port": uno_port,
                "url": format!("http://127.0.0.1:{uno_port}"),
                "compatible": "openai",
                "model_id": identity.model_id,
            }));
        }
    }

    Err("The managed UnoOne llama-server is not responding".to_string())
}
