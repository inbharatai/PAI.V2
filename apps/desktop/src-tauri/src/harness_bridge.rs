//! Pocket AI desktop embedding for the reusable InBharat Harness.
//!
//! Migration posture: this is exposed beside the existing ReAct agent until
//! acceptance tests prove parity. The bridge uses the same verified llama
//! process, the same encrypted vault and the same document/security helpers;
//! it does not create a second model runtime or persistence store.

use crate::{
    browser::{self, BrowserAction, BrowserStateHolder, ScrollDirection},
    documents,
    llama::{Content, ConversationTurn, ModelManagerState},
    safety::{DesktopSafetyGuard, SafetyGuardState, ToolAction},
    security, DesktopVaultState,
};
use inbharat_harness_core::providers::{EnforcementQuality, SandboxGrant, SandboxRequest};
use inbharat_harness_core::{
    tools::{ListFilesTool, ReadFileTool, RunProcessTool, WriteFileTool},
    AttachmentMetadata, BudgetLimits, CancellationToken, Capability, CapabilitySet,
    ConfirmationMode, ConfirmationOutcome, Determinism, ErrorCode, ExecutionLevel, Failure,
    FailureClass, HarnessBuilder, HarnessResult, LocalExecutionBroker, MemoryOptions,
    PermissionDecision, PermissionProvider, RootedFs, RunOptions, SandboxProvider, SideEffect,
    StaticConfirmationProvider, Tool, ToolArguments, ToolContext, ToolManifest, ToolOutput, Value,
};
use pai_harness_adapter::{
    PaiLlamaLocalProvider, PaiVaultMemoryProvider, PaiVaultMemoryProviderConfig,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use unoone_vault_core::Vault;

#[derive(Debug, serde::Serialize)]
pub struct HarnessChatResult {
    pub session_id: String,
    pub route: String,
    pub route_reason: String,
    pub output: String,
    pub steps: u32,
    pub tool_calls: u32,
    pub event_count: usize,
    pub elapsed_ms: u64,
    pub model_id: String,
    pub memory_namespace: String,
}

#[derive(Clone, Copy, Debug)]
enum DesktopToolKind {
    SearchNotes,
    ListDocuments,
    ReadDocument,
    VerifyVault,
}

struct DesktopReadTool {
    manifest: ToolManifest,
    kind: DesktopToolKind,
    vault_root: String,
    vault: Arc<Mutex<Option<Vault>>>,
    safety: Arc<Mutex<DesktopSafetyGuard>>,
}

impl DesktopReadTool {
    fn new(
        kind: DesktopToolKind,
        vault_root: String,
        vault: Arc<Mutex<Option<Vault>>>,
        safety: Arc<Mutex<DesktopSafetyGuard>>,
    ) -> Self {
        let (id, description, input_schema) = match kind {
            DesktopToolKind::SearchNotes => (
                "pai.search_notes",
                "Search the user's local Pocket AI notes, memories and migrated document text.",
                r#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["query"],"additionalProperties":false}"#,
            ),
            DesktopToolKind::ListDocuments => (
                "pai.list_documents",
                "List documents available in the local Pocket AI vault.",
                r#"{"type":"object","properties":{},"additionalProperties":false}"#,
            ),
            DesktopToolKind::ReadDocument => (
                "pai.read_document",
                "Read one local Pocket AI document by its document id.",
                r#"{"type":"object","properties":{"document_id":{"type":"string"}},"required":["document_id"],"additionalProperties":false}"#,
            ),
            DesktopToolKind::VerifyVault => (
                "pai.verify_vault",
                "Verify the Pocket AI package/vault manifest integrity.",
                r#"{"type":"object","properties":{},"additionalProperties":false}"#,
            ),
        };
        Self {
            manifest: ToolManifest {
                id: id.to_owned(),
                version: "1.0.0".to_owned(),
                description: description.to_owned(),
                input_schema: input_schema.to_owned(),
                output_schema: r#"{"type":"string"}"#.to_owned(),
                required_capabilities: CapabilitySet::from_slice(&[Capability::FileRead]),
                supported_levels: vec![ExecutionLevel::L1, ExecutionLevel::L2, ExecutionLevel::L3],
                determinism: Determinism::Idempotent,
                side_effect: SideEffect::Read,
                confirmation: ConfirmationMode::Never,
                concurrency_safe: false,
                default_timeout: Duration::from_secs(30),
                max_output_bytes: 64 * 1024,
                verification: "bounded-local-read-v1".to_owned(),
                compensation: "none".to_owned(),
            },
            kind,
            vault_root,
            vault,
            safety,
        }
    }

    fn vault_guard(&self) -> HarnessResult<std::sync::MutexGuard<'_, Option<Vault>>> {
        self.vault.lock().map_err(|_| {
            inbharat_harness_core::Failure::new(
                inbharat_harness_core::ErrorCode::ProviderFailed,
                inbharat_harness_core::FailureClass::Persistence,
                "pai.tool.vault",
                "Pocket AI vault mutex was poisoned",
            )
        })
    }

    fn search_notes(&self, query: &str, limit: u32) -> String {
        // Harness never searches legacy plaintext memory. Only decrypted
        // canonical-vault records participate in agent memory retrieval.
        let search_query = documents::MemorySearchQuery {
            query: query.to_owned(),
            memory_types: vec![
                "note".to_owned(),
                "document".to_owned(),
                "memory".to_owned(),
            ],
            limit,
            min_relevance: 0.1,
        };
        let mut results = match self.vault.lock() {
            Ok(guard) => guard
                .as_ref()
                .map(|vault| {
                    documents::search_migrated_contents(
                        &search_query,
                        &self.vault_root,
                        Some(vault),
                    )
                })
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        results.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit as usize);
        if results.is_empty() {
            return format!("No encrypted-vault results for '{query}'.");
        }
        let mut output = format!("Found {} encrypted local result(s):\n", results.len());
        for result in results {
            output.push_str(&format!(
                "- {} [{}] {:.0}% relevant\n  {}\n",
                result.title,
                result.memory_type,
                result.relevance * 100.0,
                result.preview
            ));
        }
        unoone_text::truncate_bytes_with_notice(&output, self.manifest.max_output_bytes)
    }

    fn list_documents(&self) -> String {
        // Only encrypted migrated documents are visible to Harness. Legacy
        // plaintext documents remain outside the production agent path until
        // the existing migration flow moves them into the vault.
        let docs = match self.vault.lock() {
            Ok(guard) => guard
                .as_ref()
                .map(|vault| documents::list_migrated_documents(&self.vault_root, Some(vault)))
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        if docs.is_empty() {
            return "No encrypted documents are available in the Pocket AI vault.".to_owned();
        }
        let mut output = format!("{} encrypted document(s):\n", docs.len());
        for doc in docs {
            output.push_str(&format!("- {} [id={}]\n", doc.title, doc.id));
        }
        unoone_text::truncate_bytes_with_notice(&output, self.manifest.max_output_bytes)
    }

    fn read_document(&self, document_id: &str) -> String {
        // Prefer encrypted migrated originals where available. This avoids
        // reintroducing a plaintext memory/document dependency into Harness.
        if let Ok(guard) = self.vault.lock() {
            if let Some(vault) = guard.as_ref() {
                if let Some(bytes) =
                    documents::read_migrated_document_content(&self.vault_root, document_id, vault)
                {
                    return match String::from_utf8(bytes) {
                        Ok(text) => unoone_text::truncate_bytes_with_notice(
                            &text,
                            self.manifest.max_output_bytes,
                        ),
                        Err(error) => {
                            let size = error.into_bytes().len();
                            format!(
                                "Document '{document_id}' is binary ({size} bytes); text extraction is unavailable for this migrated payload."
                            )
                        }
                    };
                }
            }
        }
        format!(
            "Document '{document_id}' is not present in the encrypted canonical vault; run the document migration before exposing it to Harness."
        )
    }

    fn verify_vault(&self) -> String {
        match security::verify_manifest(self.vault_root.clone()) {
            Ok(result) if result.manifest_valid && result.hmac_valid => format!(
                "Pocket AI package integrity OK: {} entries verified; manifest and HMAC valid.",
                result.entries_verified
            ),
            Ok(result) => format!(
                "Pocket AI package integrity FAILED: {} of {} entries failed; manifest_valid={}; hmac_valid={}; {}",
                result.entries_failed,
                result.total_entries,
                result.manifest_valid,
                result.hmac_valid,
                result.errors.join("; ")
            ),
            Err(error) => format!("Pocket AI package verification could not run: {error}"),
        }
    }
}

impl Tool for DesktopReadTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        let allowed: &[&str] = match self.kind {
            DesktopToolKind::SearchNotes => &["query", "limit"],
            DesktopToolKind::ReadDocument => &["document_id"],
            DesktopToolKind::ListDocuments | DesktopToolKind::VerifyVault => &[],
        };
        if arguments.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(inbharat_harness_core::Failure::invalid(
                "pai.tool.arguments",
                "tool call contains an unsupported argument",
            ));
        }
        match self.kind {
            DesktopToolKind::SearchNotes => {
                required_string(arguments, "query")?;
                if let Some(value) = arguments.get("limit") {
                    match value {
                        Value::Integer(limit) if (1..=50).contains(limit) => {}
                        _ => {
                            return Err(inbharat_harness_core::Failure::invalid(
                                "pai.search_notes.limit",
                                "limit must be an integer from 1 to 50",
                            ))
                        }
                    }
                }
            }
            DesktopToolKind::ReadDocument => {
                required_string(arguments, "document_id")?;
            }
            DesktopToolKind::ListDocuments | DesktopToolKind::VerifyVault => {}
        }
        Ok(())
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        context.cancel.check("pai.desktop_tool")?;
        // A locked vault is a hard denial. Harness never falls back to legacy
        // plaintext memory/documents, so this cannot become a privacy downgrade.
        {
            let guard = self.vault_guard()?;
            if guard.as_ref().is_none_or(|vault| !vault.is_unlocked()) {
                return Err(inbharat_harness_core::Failure::new(
                    inbharat_harness_core::ErrorCode::PermissionDenied,
                    inbharat_harness_core::FailureClass::Persistence,
                    "pai.desktop_tool",
                    "Pocket AI vault is locked",
                ));
            }
        }

        // Reuse the exact UnoOne safety guard for every model-selected tool
        // action. The Harness core never executes raw model output directly.
        let parameter_json: serde_json::Value = serde_json::from_str(
            &Value::Object(arguments.clone()).to_canonical_json(),
        )
        .map_err(|error| {
            inbharat_harness_core::Failure::invalid(
                "pai.desktop_tool.safety",
                format!("could not canonicalize tool arguments: {error}"),
            )
        })?;
        let action = ToolAction {
            action_id: format!("harness-{}", uuid::Uuid::new_v4()),
            tool_name: self.manifest.id.clone(),
            parameters: parameter_json,
            confidence: None,
            raw_output: Value::Object(arguments.clone()).to_canonical_json(),
        };
        let verdict = self
            .safety
            .lock()
            .map_err(|_| {
                inbharat_harness_core::Failure::new(
                    inbharat_harness_core::ErrorCode::ProviderFailed,
                    inbharat_harness_core::FailureClass::Policy,
                    "pai.desktop_tool.safety",
                    "UnoOne safety state lock failed",
                )
            })?
            .review_action(&action);
        if !verdict.approved {
            return Err(inbharat_harness_core::Failure::new(
                inbharat_harness_core::ErrorCode::PermissionDenied,
                inbharat_harness_core::FailureClass::Policy,
                "pai.desktop_tool.safety",
                verdict.reason,
            ));
        }

        let text = match self.kind {
            DesktopToolKind::SearchNotes => {
                let query = required_string(arguments, "query")?;
                let limit = arguments
                    .get("limit")
                    .and_then(|value| match value {
                        Value::Integer(value) => u32::try_from(*value).ok(),
                        _ => None,
                    })
                    .unwrap_or(10);
                self.search_notes(query, limit)
            }
            DesktopToolKind::ListDocuments => self.list_documents(),
            DesktopToolKind::ReadDocument => {
                self.read_document(required_string(arguments, "document_id")?)
            }
            DesktopToolKind::VerifyVault => self.verify_vault(),
        };
        Ok(ToolOutput {
            value: Value::String(text.clone()),
            model_content: text,
            presentation: BTreeMap::new(),
        })
    }
}

fn required_string<'a>(arguments: &'a ToolArguments, key: &str) -> HarnessResult<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && value.len() <= 4096)
        .ok_or_else(|| {
            inbharat_harness_core::Failure::invalid(
                "pai.tool.arguments",
                format!("missing or invalid required string '{key}'"),
            )
        })
}

fn desktop_read_tools(
    vault_root: &str,
    vault: Arc<Mutex<Option<Vault>>>,
    safety: Arc<Mutex<DesktopSafetyGuard>>,
) -> Vec<Arc<dyn Tool>> {
    [
        DesktopToolKind::SearchNotes,
        DesktopToolKind::ListDocuments,
        DesktopToolKind::ReadDocument,
        DesktopToolKind::VerifyVault,
    ]
    .into_iter()
    .map(|kind| {
        Arc::new(DesktopReadTool::new(
            kind,
            vault_root.to_owned(),
            Arc::clone(&vault),
            Arc::clone(&safety),
        )) as Arc<dyn Tool>
    })
    .collect()
}

// ---------------------------------------------------------------------------
// Full-access local agent lane (coding, automation, browser)
// ---------------------------------------------------------------------------
//
// The user explicitly directed that Pocket AI, in full-access mode, may do
// everything Gemma can drive on this machine: read and write files, create
// directories, run programs, act in the browser workspace. What does NOT
// change is what makes the agent trustworthy: every call still passes the
// non-bypassable harness pipeline (validate → authorize per capability →
// confirm → budget → sandbox → execute → bound → verify) with the full JSONL
// audit trail, every subprocess is direct-argv (never a shell), cwd-confined
// to the workspace, environment-scrubbed and deadline-killed, and every file
// access stays behind the TOCTOU-hardened RootedFs fence.

/// The permission policy for full-access mode: every capability is
/// authorized. This replaces deny-by-default ONLY in the full-access lane
/// and only for the tool-backed local capabilities; the harness core's
/// pipeline, budgets and audit remain non-bypassable in front of every call.
struct FullAccessPermission;

impl PermissionProvider for FullAccessPermission {
    fn authorize(
        &self,
        _actor: &str,
        _capability: Capability,
        _resource: &str,
    ) -> HarnessResult<PermissionDecision> {
        Ok(PermissionDecision::Allow)
    }
}

/// The desktop sandbox policy. The builder default grants only
/// `[FileRead, Model]`, so without this provider every full-access tool
/// call — fs.write, workspace.patch, process.run, browser.act — failed the
/// sandbox stage ("sandbox capability is not granted") and the bridge fell
/// back to the read-only legacy agent. Full access mirrors the CLI's
/// --trusted-process posture: the whole lane capability surface is granted
/// and the allowlisted direct-argv broker is the enforcement boundary
/// (quality Partial, honestly reported, never silently upgraded). The
/// read-only chat lane keeps the default in-process fence surface.
struct DesktopSandbox {
    granted: CapabilitySet,
    trusted_process: bool,
}

impl SandboxProvider for DesktopSandbox {
    fn resolve(&self, request: &SandboxRequest) -> HarnessResult<SandboxGrant> {
        if !request.capabilities.is_subset_of(&self.granted) {
            return Err(Failure::new(
                ErrorCode::PermissionDenied,
                FailureClass::Policy,
                "sandbox.resolve",
                "sandbox capability is not granted",
            ));
        }
        if request.require_security_boundary && !self.trusted_process {
            return Err(Failure::new(
                ErrorCode::SandboxUnavailable,
                FailureClass::Policy,
                "sandbox.resolve",
                "process execution requires the full-access lane",
            ));
        }
        Ok(SandboxGrant {
            world_id: request.world_id.clone(),
            backend: if self.trusted_process {
                "unoone-allowlisted-direct-argv".to_owned()
            } else {
                "rooted-fs-fence".to_owned()
            },
            quality: if self.trusted_process {
                EnforcementQuality::Partial
            } else {
                EnforcementQuality::InProcessFence
            },
            granted: self.granted.clone(),
        })
    }
}

/// Programs the full-access agent may spawn. Direct argv only — the broker
/// never invokes a shell; each name is resolved from PATH at broker
/// construction and unresolvable names are silently skipped, so this is a
/// capability declaration, not a guarantee. Interpreters and shells are
/// included deliberately under the user's full-access directive: a coding
/// agent that cannot run `npm install`, a test runner or a build script is
/// not a coding agent. Every spawn is still audited, budgeted and
/// deadline-killed by the harness in front of the broker.
const FULL_ACCESS_PROGRAMS: &[&str] = &[
    // VCS + build + languages
    "git",
    "cargo",
    "rustc",
    "node",
    "npm",
    "npx",
    "python",
    "python3",
    "py",
    "pip",
    "dotnet",
    "go",
    "java",
    "mvn",
    "gradle",
    "cmake",
    "make",
    "gcc",
    "g++",
    "clang",
    "cl",
    // Shells: full access means the agent may script compound commands too.
    "powershell",
    "pwsh",
    "cmd",
    "bash",
    "sh",
];

/// The system-prompt briefing that tells the model what it actually is and
/// which tools it holds in this run. Without it the model only sees the
/// generic harness line ("Answer directly." / "Work toward the goal..."),
/// and L0 direct-answer requests carry no tools array at all — observed
/// live: the model told the user it "only operates within the encrypted
/// USB vault" while the full-access lane was enabled. The text must stay
/// truthful to what the bridge registers per mode: read-only vault tools
/// always; workspace file tools + allowlisted direct-argv commands +
/// browser control only in full-access mode.
fn desktop_system_prefix(full_access: bool) -> String {
    let workspace = workspace_root()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| "%USERPROFILE%\\UnoOneAgent".to_owned());
    if full_access {
        format!(
            "You are UnoOne, the user's private Pocket AI running locally on their Windows \
             computer (fully offline, no cloud). You are NOT limited to a vault: in this \
             session you have full agent tools, all audited and budgeted.\n\
             - Read/write/list/search/patch files in the workspace folder: {workspace}\n\
             (give tool paths relative to that folder, or as absolute paths \
             inside it — both are accepted and fenced to it)\n\
             - Run programs directly (git, cargo, rustc, node, npm, npx, python, pip, \
             dotnet, go, java, cmake, make, gcc, clang, powershell) inside that workspace\n\
             - Drive a real web browser (navigate, click, type, fill forms, screenshot) \
             via browser.act\n\
             - Read the user's encrypted Pocket AI vault records \
             (search_notes, list_documents, read_document, verify_vault)\n\
             When a task needs any of this, actually use the tools instead of claiming \
             you cannot. If a request falls outside what the tools above can reach, say \
             so honestly and specifically.\n\
             You are an autonomous agent: when the user asks you to build, create, \
             write, or fix something, do the whole task yourself with the tools — \
             create every file with fs.write, run and verify the result with \
             process.run, read back what you wrote with fs.read, and keep going until \
             the task is genuinely done. Never paste code or file contents into the \
             chat instead of creating the real files, never stop halfway to ask the \
             user to do steps you can do yourself, and never claim you cannot access \
             the filesystem or run programs — you can, and every step is verified \
             above.\n\
             Communicate like a normal assistant: for a complex task, reply with a \
             short plan first (what you will build and in what order), then execute \
             it; for questions, ideas, or brainstorming, answer naturally and \
             concretely in the user's language; use tools only when the task \
             actually needs them.\n\
             Images the user attaches to a message are delivered inline through \
             your vision encoder — you see them directly. When a message says \
             images are attached, describe what is actually shown; never claim \
             an image is missing."
        )
    } else {
        "You are UnoOne, the user's private Pocket AI running locally (fully offline, no \
         cloud). In this session you are in read-only mode: you can search, list and read \
         the user's encrypted Pocket AI vault records and verify the vault, but you cannot \
         read or modify other host files, run commands or drive the browser. Say so \
         honestly when a request needs those abilities, and suggest re-enabling full \
         access in the chat settings."
            .to_owned()
    }
}

/// Reduce the llama-server-reported model id to a harness-registry-safe value.
///
/// llama-server advertises `/v1/models` ids from the model's launch path — on
/// Windows that is the full `C:\...\D333….gguf` string. The vendored harness
/// registry's `valid_model_id` charset is `[A-Za-z0-9._-/:]` (no backslash),
/// so registering the raw id failed with `conflict:model.register` on EVERY
/// Windows chat — observed live: harness_chat always threw and the UI
/// silently fell back to the legacy vault-only agent, which is how the model
/// came to claim it "only operates within the encrypted USB vault". The
/// basename keeps the id stable, honest (it is the real model file — the
/// manifest-sha256 filename for cached launches, the model filename for
/// drive launches) and registry-valid on every platform. Every path-like
/// report reduces to its basename; plain registry-safe ids pass through.
fn registry_safe_model_id(reported: &str) -> String {
    let basename = reported
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(reported)
        .trim();
    if basename.is_empty() {
        // Nothing path-like in the report: keep the original string so an
        // id-less server still surfaces as an explicit registry rejection
        // rather than a silently re-labelled one.
        reported.trim().to_owned()
    } else {
        basename.to_owned()
    }
}

/// The coding/automation workspace root. Full-access file tools and
/// subprocesses are rooted here — never the encrypted pendrive vault — so
/// agent writes land on rewritable host disk, not the read-mostly package.
/// The L3 full-access session budget, set to the MAXIMUM the harness's hard
/// safety bounds permit (`validate_run_options`): 10,000 steps, 100,000 tool
/// calls, 1,000 rounds, 24 h of wall time, 64 MiB of accumulated tool output.
/// The user's standing directive is "no cap" — the full-access lane runs at
/// the validator ceiling in every dimension, so no real task is ever cut
/// short by a desktop-side budget. Only jobs/subagent-depth stay at their
/// conservative defaults (the desktop lane spawns no subagents or job
/// queues today; raising them is meaningless until those lanes exist).
/// Live-caught 2026-09-12 (defect #16): this shape must stay within the
/// harness's hard safety bounds — the 64 MiB cumulative output figure was
/// once rejected by the validator, every full-access chat call threw, and
/// the UI silently fell back to the read-only vault agent (the model then
/// truthfully told the user it could not write files).
/// `full_access_budget_is_accepted_by_the_harness` pins it.
fn full_access_budget() -> BudgetLimits {
    BudgetLimits {
        max_steps: 10_000,
        max_tool_calls: 100_000,
        max_rounds: 1_000,
        max_jobs: 0,
        max_subagent_depth: 0,
        max_output_bytes: 64 * 1024 * 1024,
        max_duration: Duration::from_secs(24 * 60 * 60),
    }
}

/// The coding/automation workspace root. Full-access file tools and
/// subprocesses are rooted here — never the encrypted pendrive vault — so
/// agent writes land on rewritable host disk, not the read-mostly package.
fn workspace_root() -> Result<PathBuf, String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            "cannot locate the user home directory for the agent workspace".to_string()
        })?;
    let root = home.join("UnoOneAgent");
    std::fs::create_dir_all(&root).map_err(|e| {
        format!(
            "cannot create the agent workspace at {}: {e}",
            root.display()
        )
    })?;
    Ok(root)
}

/// Vision lane: the image types llama.cpp accepts through an `image_url`
/// part (via the mmproj projector loaded at model-server startup).
const SUPPORTED_IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];
const MAX_IMAGES_PER_TURN: usize = 4;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// Parse data-URL image attachments into (a) harness attachment metadata —
/// ids, media types, lengths and SHA-256 digests for the audit trail — and
/// (b) the local base64 bytes the model provider renders into the OpenAI
/// request. Pixels never leave the machine; the audit records digests.
#[allow(clippy::type_complexity)]
fn parse_image_attachments(
    data_urls: &[String],
) -> Result<(Vec<AttachmentMetadata>, Vec<(String, String, String)>), String> {
    use base64::Engine as _;
    if data_urls.len() > MAX_IMAGES_PER_TURN {
        return Err(format!(
            "at most {MAX_IMAGES_PER_TURN} images can be attached per message"
        ));
    }
    let mut metadata = Vec::with_capacity(data_urls.len());
    let mut bytes_by_id = Vec::with_capacity(data_urls.len());
    for (index, data_url) in data_urls.iter().enumerate() {
        if data_url.len() > 16 * 1024 * 1024 {
            return Err("attached image is too large".to_owned());
        }
        let rest = data_url
            .strip_prefix("data:")
            .ok_or_else(|| "images must be data URLs (data:image/png;base64,...)".to_owned())?;
        let (header, payload) = rest
            .split_once(',')
            .ok_or_else(|| "image data URL is malformed".to_owned())?;
        let media_type = header
            .strip_suffix(";base64")
            .ok_or_else(|| "image data URL must be base64-encoded".to_owned())?
            .to_owned();
        if !SUPPORTED_IMAGE_TYPES.contains(&media_type.as_str()) {
            return Err(format!(
                "unsupported image type '{media_type}' (supported: png, jpeg, webp, gif)"
            ));
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(|error| format!("attached image is not valid base64: {error}"))?;
        if decoded.is_empty() {
            return Err("attached image is empty".to_owned());
        }
        if decoded.len() > MAX_IMAGE_BYTES {
            return Err(format!(
                "attached image exceeds {} MiB",
                MAX_IMAGE_BYTES / (1024 * 1024)
            ));
        }
        let digest = {
            use sha2::Digest;
            let mut hasher = sha2::Sha256::new();
            hasher.update(&decoded);
            hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let id = format!("attach-{}", index + 1);
        metadata.push(AttachmentMetadata {
            id: id.clone(),
            media_type: media_type.clone(),
            byte_len: decoded.len() as u64,
            digest,
            display_name: None,
        });
        bytes_by_id.push((id, media_type, payload.trim().to_owned()));
    }
    Ok((metadata, bytes_by_id))
}

/// Run the shared UnoOne safety review for one model-selected tool call.
/// Every desktop bridge tool uses the same guard as the vault read tools.
fn safety_review(
    safety: &Arc<Mutex<DesktopSafetyGuard>>,
    tool_id: &str,
    arguments: &ToolArguments,
) -> HarnessResult<()> {
    let canonical = Value::Object(arguments.clone()).to_canonical_json();
    let parameter_json: serde_json::Value = serde_json::from_str(&canonical).map_err(|error| {
        inbharat_harness_core::Failure::invalid(
            "pai.desktop_tool.safety",
            format!("could not canonicalize tool arguments: {error}"),
        )
    })?;
    let action = ToolAction {
        action_id: format!("harness-{}", uuid::Uuid::new_v4()),
        tool_name: tool_id.to_owned(),
        parameters: parameter_json,
        confidence: None,
        raw_output: canonical,
    };
    let verdict = safety
        .lock()
        .map_err(|_| {
            inbharat_harness_core::Failure::new(
                inbharat_harness_core::ErrorCode::ProviderFailed,
                inbharat_harness_core::FailureClass::Policy,
                "pai.desktop_tool.safety",
                "UnoOne safety state lock failed",
            )
        })?
        .review_action(&action);
    if !verdict.approved {
        return Err(inbharat_harness_core::Failure::new(
            inbharat_harness_core::ErrorCode::PermissionDenied,
            inbharat_harness_core::FailureClass::Policy,
            "pai.desktop_tool.safety",
            verdict.reason,
        ));
    }
    Ok(())
}

fn optional_string<'a>(arguments: &'a ToolArguments, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 64 * 1024)
}

fn value_as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        _ => None,
    }
}

/// Recursive literal-substring search across the agent workspace. The walk
/// goes through the same `RootedFs` fence as fs.read/fs.write (directories
/// are enumerated by `list`, files are read by `read_text`), so path escape,
/// symlink planting and oversized files are rejected by the fence rather
/// than by this tool's own logic.
struct DesktopSearchTool {
    manifest: ToolManifest,
    filesystem: RootedFs,
}

impl DesktopSearchTool {
    fn new(filesystem: RootedFs) -> Self {
        Self {
            manifest: ToolManifest {
                id: "workspace.search".to_owned(),
                version: "1.0.0".to_owned(),
                description: "Recursively search file contents in the agent workspace for a literal substring; returns path:line: text matches.".to_owned(),
                input_schema: r#"{"type":"object","properties":{"query":{"type":"string"},"path":{"type":"string"},"case_insensitive":{"type":"boolean"},"max_results":{"type":"integer","minimum":1,"maximum":200}},"required":["query"],"additionalProperties":false}"#.to_owned(),
                output_schema: r#"{"type":"string"}"#.to_owned(),
                required_capabilities: CapabilitySet::from_slice(&[Capability::FileRead]),
                supported_levels: vec![ExecutionLevel::L1, ExecutionLevel::L2, ExecutionLevel::L3],
                determinism: Determinism::Deterministic,
                side_effect: SideEffect::Read,
                confirmation: ConfirmationMode::Never,
                concurrency_safe: false,
                default_timeout: Duration::from_secs(60),
                max_output_bytes: 64 * 1024,
                verification: "fenced-local-read-v1".to_owned(),
                compensation: "none".to_owned(),
            },
            filesystem,
        }
    }
}

impl Tool for DesktopSearchTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        let allowed = ["query", "path", "case_insensitive", "max_results"];
        if arguments.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(inbharat_harness_core::Failure::invalid(
                "pai.tool.arguments",
                "workspace.search call contains an unsupported argument",
            ));
        }
        required_string(arguments, "query")?;
        if let Some(value) = arguments.get("max_results") {
            match value {
                Value::Integer(limit) if (1..=200).contains(limit) => {}
                _ => {
                    return Err(inbharat_harness_core::Failure::invalid(
                        "workspace.search.max_results",
                        "max_results must be an integer from 1 to 200",
                    ))
                }
            }
        }
        Ok(())
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        context.cancel.check("pai.workspace_search")?;
        let query = required_string(arguments, "query")?;
        let start = optional_string(arguments, "path").unwrap_or(".");
        let case_insensitive = arguments
            .get("case_insensitive")
            .and_then(value_as_bool)
            .unwrap_or(false);
        let max_results = arguments
            .get("max_results")
            .and_then(|value| match value {
                Value::Integer(limit) => u32::try_from(*limit).ok(),
                _ => None,
            })
            .unwrap_or(50) as usize;
        let needle = if case_insensitive {
            query.to_ascii_lowercase()
        } else {
            query.to_owned()
        };

        const MAX_FILES: usize = 2000;
        const MAX_DEPTH: usize = 12;

        let mut scanned = 0usize;
        let mut matches: Vec<String> = Vec::new();
        let mut queue: Vec<(String, usize)> = vec![(start.to_owned(), 0)];
        let mut truncated = false;
        while let Some((dir, depth)) = queue.pop() {
            if matches.len() >= max_results || scanned >= MAX_FILES {
                truncated = true;
                break;
            }
            if depth >= MAX_DEPTH {
                continue;
            }
            let entries = match self.filesystem.list(&dir) {
                Ok(entries) => entries,
                // Unreadable directories (permissions, fence) are skipped, not
                // fatal: a search reports what it could see.
                Err(_) => continue,
            };
            for name in entries {
                if matches.len() >= max_results || scanned >= MAX_FILES {
                    truncated = true;
                    break;
                }
                let relative = if dir == "." {
                    name
                } else {
                    format!("{dir}/{name}")
                };
                let resolved = match self.filesystem.resolve_existing(&relative) {
                    Ok(resolved) => resolved,
                    Err(_) => continue,
                };
                let metadata = match std::fs::symlink_metadata(&resolved) {
                    Ok(metadata) => metadata,
                    Err(_) => continue,
                };
                if metadata.is_symlink() {
                    continue;
                }
                if metadata.is_dir() {
                    queue.push((relative, depth + 1));
                    continue;
                }
                if !metadata.is_file() {
                    continue;
                }
                scanned += 1;
                let Ok(text) = self.filesystem.read_text(&relative) else {
                    continue;
                };
                for (index, line) in text.lines().enumerate() {
                    // Case-insensitive matching lowercases a copy of the
                    // line for comparison; the reported match text stays
                    // the original line, as the user wrote it.
                    let candidate: std::borrow::Cow<str> = if case_insensitive {
                        std::borrow::Cow::Owned(line.to_ascii_lowercase())
                    } else {
                        std::borrow::Cow::Borrowed(line)
                    };
                    if candidate.contains(&needle) {
                        matches.push(format!("{relative}:{}: {line}", index + 1));
                        if matches.len() >= max_results {
                            break;
                        }
                    }
                }
            }
        }
        let mut output = if matches.is_empty() {
            format!("No matches for '{query}' under {start} ({scanned} file(s) scanned).")
        } else {
            format!(
                "{} match(es) for '{query}' ({scanned} file(s) scanned):\n{}",
                matches.len(),
                matches.join("\n")
            )
        };
        if truncated {
            output.push_str("\n[result truncated: raise max_results or narrow the search path]");
        }
        let output =
            unoone_text::truncate_bytes_with_notice(&output, self.manifest.max_output_bytes);
        Ok(ToolOutput {
            value: Value::String(output.clone()),
            model_content: output,
            presentation: BTreeMap::new(),
        })
    }
}

/// Exact-string search/replace over one workspace file — the token-efficient
/// editing primitive for a coding agent (whole-file rewrites through
/// fs.write stay available, but a 12B model patches far more reliably than
/// it reproduces a whole file). Reads and writes go through the same
/// `RootedFs` fence as the built-in fs tools; the atomic write means a
/// failed or partial patch never leaves a torn file.
struct DesktopPatchTool {
    manifest: ToolManifest,
    filesystem: RootedFs,
}

impl DesktopPatchTool {
    fn new(filesystem: RootedFs) -> Self {
        Self {
            manifest: ToolManifest {
                id: "workspace.patch".to_owned(),
                version: "1.0.0".to_owned(),
                description: "Replace an exact literal substring inside one workspace file. By default the match must be unique; pass replace_all=true to replace every occurrence.".to_owned(),
                input_schema: r#"{"type":"object","properties":{"path":{"type":"string"},"find":{"type":"string"},"replace":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","find","replace"],"additionalProperties":false}"#.to_owned(),
                output_schema: r#"{"type":"string"}"#.to_owned(),
                required_capabilities: CapabilitySet::from_slice(&[Capability::FileWrite]),
                supported_levels: vec![ExecutionLevel::L1, ExecutionLevel::L2, ExecutionLevel::L3],
                determinism: Determinism::NonIdempotent,
                side_effect: SideEffect::Write,
                confirmation: ConfirmationMode::OnSideEffect,
                concurrency_safe: false,
                default_timeout: Duration::from_secs(30),
                max_output_bytes: 16 * 1024,
                verification: "fenced-atomic-write-v1".to_owned(),
                compensation: "re-write-v1".to_owned(),
            },
            filesystem,
        }
    }
}

impl Tool for DesktopPatchTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        let allowed = ["path", "find", "replace", "replace_all"];
        if arguments.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(inbharat_harness_core::Failure::invalid(
                "pai.tool.arguments",
                "workspace.patch call contains an unsupported argument",
            ));
        }
        required_string(arguments, "path")?;
        let find = arguments
            .get("find")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 64 * 1024)
            .ok_or_else(|| {
                inbharat_harness_core::Failure::invalid(
                    "workspace.patch.find",
                    "find must be a non-empty string of at most 64 KiB",
                )
            })?;
        if find.matches('\n').count() > 512 {
            return Err(inbharat_harness_core::Failure::invalid(
                "workspace.patch.find",
                "find spans too many lines; narrow the match",
            ));
        }
        let _replace = arguments
            .get("replace")
            .and_then(Value::as_str)
            .filter(|value| value.len() <= 256 * 1024)
            .ok_or_else(|| {
                inbharat_harness_core::Failure::invalid(
                    "workspace.patch.replace",
                    "replace must be a string of at most 256 KiB (an empty string deletes the matched text)",
                )
            })?;
        Ok(())
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        context.cancel.check("pai.workspace_patch")?;
        let path = required_string(arguments, "path")?;
        let find = required_string(arguments, "find")?;
        let replace = arguments
            .get("replace")
            .and_then(Value::as_str)
            .unwrap_or("");
        let replace_all = arguments
            .get("replace_all")
            .and_then(value_as_bool)
            .unwrap_or(false);

        let text = self.filesystem.read_text(path)?;
        let occurrences = text.matches(find).count();
        if occurrences == 0 {
            return Err(inbharat_harness_core::Failure::invalid(
                "workspace.patch.find",
                format!("the text to find is not present in {path}"),
            ));
        }
        if occurrences > 1 && !replace_all {
            return Err(inbharat_harness_core::Failure::invalid(
                "workspace.patch.find",
                format!(
                    "the text to find occurs {occurrences} times in {path}; include more surrounding context to make it unique, or pass replace_all=true"
                ),
            ));
        }
        let patched = if replace_all {
            text.replace(find, replace)
        } else {
            text.replacen(find, replace, 1)
        };
        self.filesystem.write_text_atomic(path, &patched)?;
        let summary = format!(
            "patched {path}: replaced {occurrences} occurrence(s); file is now {} bytes",
            patched.len()
        );
        Ok(ToolOutput {
            value: Value::String(summary.clone()),
            model_content: summary,
            presentation: BTreeMap::from([("kind".to_owned(), "file-patch".to_owned())]),
        })
    }
}

/// Model-driven browser lane: the same typed actions, session state and
/// verified result contract as the user-driven BrowserWorkspace buttons,
/// exposed to the model as one tool. `confirmed` is always true here — this
/// tool only exists in the user-directed full-access lane — but every call
/// still passes the shared safety review and the browser module's own
/// URL validation.
struct DesktopBrowserTool {
    manifest: ToolManifest,
    app: tauri::AppHandle,
    browser: Arc<BrowserStateHolder>,
    safety: Arc<Mutex<DesktopSafetyGuard>>,
}

impl DesktopBrowserTool {
    fn new(
        app: tauri::AppHandle,
        browser: Arc<BrowserStateHolder>,
        safety: Arc<Mutex<DesktopSafetyGuard>>,
    ) -> Self {
        Self {
            manifest: ToolManifest {
                id: "browser.act".to_owned(),
                version: "1.0.0".to_owned(),
                description: "Drive the desktop browser workspace: navigate, click, type, fill forms, scroll, extract page text, get page info or screenshot. Requires an active browser session (the user opens the BrowserWorkspace first).".to_owned(),
                input_schema: r#"{"type":"object","properties":{"action":{"type":"string","enum":["navigate","back","forward","reload","extract_page_text","extract_element_text","click","type","fill_form","scroll","wait","get_page_info","screenshot","close","clear_session"]},"url":{"type":"string"},"selector":{"type":"string"},"text":{"type":"string"},"direction":{"type":"string","enum":["up","down"]},"amount":{"type":"integer","minimum":1},"milliseconds":{"type":"integer","minimum":1,"maximum":30000},"fields":{"type":"array","items":{"type":"object","properties":{"selector":{"type":"string"},"value":{"type":"string"}},"required":["selector","value"],"additionalProperties":false}}},"required":["action"],"additionalProperties":false}"#.to_owned(),
                output_schema: r#"{"type":"string"}"#.to_owned(),
                required_capabilities: CapabilitySet::from_slice(&[Capability::Workspace]),
                supported_levels: vec![ExecutionLevel::L1, ExecutionLevel::L2, ExecutionLevel::L3],
                determinism: Determinism::NonIdempotent,
                side_effect: SideEffect::Process,
                confirmation: ConfirmationMode::OnSideEffect,
                concurrency_safe: false,
                default_timeout: Duration::from_secs(30),
                max_output_bytes: 64 * 1024,
                verification: "webview-verified-v1".to_owned(),
                compensation: "none".to_owned(),
            },
            app,
            browser,
            safety,
        }
    }
}

/// The browser.act argument contract, standalone so it is testable without
/// a Tauri AppHandle.
fn validate_browser_arguments(arguments: &ToolArguments) -> HarnessResult<()> {
    let allowed = [
        "action",
        "url",
        "selector",
        "text",
        "direction",
        "amount",
        "milliseconds",
        "fields",
    ];
    if arguments.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(inbharat_harness_core::Failure::invalid(
            "pai.tool.arguments",
            "browser.act call contains an unsupported argument",
        ));
    }
    let action = required_string(arguments, "action")?;
    let require = |fields: &[&str]| -> HarnessResult<()> {
        for field in fields {
            required_string(arguments, field)?;
        }
        Ok(())
    };
    match action {
        "navigate" => require(&["url"])?,
        "extract_element_text" | "click" => require(&["selector"])?,
        "type" => require(&["selector", "text"])?,
        "fill_form" => {
            let Some(Value::Array(fields)) = arguments.get("fields") else {
                return Err(inbharat_harness_core::Failure::invalid(
                    "browser.act.fields",
                    "fill_form requires a fields array",
                ));
            };
            if fields.is_empty() || fields.len() > 64 {
                return Err(inbharat_harness_core::Failure::invalid(
                    "browser.act.fields",
                    "fields must contain 1-64 entries",
                ));
            }
        }
        "scroll" => {
            require(&["direction"])?;
            let direction = required_string(arguments, "direction")?;
            if !matches!(direction, "up" | "down") {
                return Err(inbharat_harness_core::Failure::invalid(
                    "browser.act.direction",
                    "direction must be 'up' or 'down'",
                ));
            }
            require(&["amount"])?;
            match arguments.get("amount") {
                Some(Value::Integer(value)) if (1..=100_000).contains(value) => {}
                _ => {
                    return Err(inbharat_harness_core::Failure::invalid(
                        "browser.act.amount",
                        "amount must be an integer from 1 to 100000",
                    ))
                }
            }
        }
        "wait" => match arguments.get("milliseconds") {
            Some(Value::Integer(value)) if (1..=30_000).contains(value) => {}
            _ => {
                return Err(inbharat_harness_core::Failure::invalid(
                    "browser.act.milliseconds",
                    "milliseconds must be an integer from 1 to 30000",
                ))
            }
        },
        "back" | "forward" | "reload" | "extract_page_text" | "get_page_info" | "screenshot"
        | "close" | "clear_session" => {}
        other => {
            return Err(inbharat_harness_core::Failure::invalid(
                "browser.act.action",
                format!("unknown browser action '{other}'"),
            ))
        }
    }
    Ok(())
}

impl Tool for DesktopBrowserTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        validate_browser_arguments(arguments)
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        context.cancel.check("pai.browser_tool")?;
        safety_review(&self.safety, &self.manifest.id, arguments)?;

        let action = required_string(arguments, "action")?;
        let typed = match action {
            "navigate" => BrowserAction::Navigate {
                url: required_string(arguments, "url")?.to_owned(),
            },
            "back" => BrowserAction::Back,
            "forward" => BrowserAction::Forward,
            "reload" => BrowserAction::Reload,
            "extract_page_text" => BrowserAction::ExtractPageText,
            "extract_element_text" => BrowserAction::ExtractElementText {
                selector: required_string(arguments, "selector")?.to_owned(),
            },
            "click" => BrowserAction::Click {
                selector: required_string(arguments, "selector")?.to_owned(),
            },
            "type" => BrowserAction::Type {
                selector: required_string(arguments, "selector")?.to_owned(),
                text: required_string(arguments, "text")?.to_owned(),
            },
            "fill_form" => {
                let Some(Value::Array(fields)) = arguments.get("fields") else {
                    return Err(inbharat_harness_core::Failure::invalid(
                        "browser.act.fields",
                        "fill_form requires a fields array",
                    ));
                };
                let mut parsed = Vec::with_capacity(fields.len());
                for field in fields {
                    let Some(object) = field.as_object() else {
                        return Err(inbharat_harness_core::Failure::invalid(
                            "browser.act.fields",
                            "each field must be an object with selector and value",
                        ));
                    };
                    let selector = object
                        .get("selector")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let value = object
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if selector.is_empty() {
                        return Err(inbharat_harness_core::Failure::invalid(
                            "browser.act.fields",
                            "each field requires a non-empty selector",
                        ));
                    }
                    parsed.push(browser::FormFillField {
                        selector: selector.to_owned(),
                        value: value.to_owned(),
                    });
                }
                BrowserAction::FillForm { fields: parsed }
            }
            "scroll" => BrowserAction::Scroll {
                direction: if required_string(arguments, "direction")? == "up" {
                    ScrollDirection::Up
                } else {
                    ScrollDirection::Down
                },
                amount: match arguments.get("amount") {
                    Some(Value::Integer(value)) => u32::try_from(*value).unwrap_or(1),
                    _ => 1,
                },
            },
            "wait" => BrowserAction::Wait {
                milliseconds: match arguments.get("milliseconds") {
                    Some(Value::Integer(value)) => u64::try_from(*value).unwrap_or(100),
                    _ => 100,
                },
            },
            "get_page_info" => BrowserAction::GetPageInfo,
            "screenshot" => BrowserAction::Screenshot,
            "close" => BrowserAction::Close,
            "clear_session" => BrowserAction::ClearSession,
            other => {
                return Err(inbharat_harness_core::Failure::invalid(
                    "browser.act.action",
                    format!("unknown browser action '{other}'"),
                ))
            }
        };

        // Full access: risky element consent (submit/upload/download) is
        // auto-granted by design — the user directed this lane; the safety
        // review above still ran, and the browser module's URL validation
        // and session checks still apply.
        let result =
            browser::browser_execute_sync(typed, true, self.app.clone(), Arc::clone(&self.browser))
                .map_err(|error| {
                    inbharat_harness_core::Failure::new(
                        inbharat_harness_core::ErrorCode::ToolFailed,
                        inbharat_harness_core::FailureClass::Execution,
                        "pai.browser_tool",
                        error,
                    )
                })?;
        let summary = serde_json::json!({
            "success": result.success,
            "verified": result.verified,
            "current_url": result.current_url,
            "current_title": result.current_title,
            "message": result.user_message,
            "error": result.error,
            "screenshot_path": result.screenshot_path,
            "data": result.data,
        });
        let text = unoone_text::truncate_bytes_with_notice(
            &serde_json::to_string_pretty(&summary).unwrap_or_default(),
            self.manifest.max_output_bytes,
        );
        Ok(ToolOutput {
            value: Value::String(text.clone()),
            model_content: text,
            presentation: BTreeMap::from([("kind".to_owned(), "browser".to_owned())]),
        })
    }
}

/// The full-access tool set: the harness built-ins (fenced fs.read/fs.list/
/// fs.write + allowlisted direct-argv process.run), plus the desktop search,
/// patch and browser adapters. Every tool stays behind the harness pipeline.
fn desktop_workspace_tools(
    filesystem: RootedFs,
    app: tauri::AppHandle,
    browser: Arc<BrowserStateHolder>,
    safety: Arc<Mutex<DesktopSafetyGuard>>,
) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadFileTool::default()) as Arc<dyn Tool>,
        Arc::new(ListFilesTool::default()),
        Arc::new(WriteFileTool::default()),
        Arc::new(RunProcessTool::default()),
        Arc::new(DesktopSearchTool::new(filesystem.clone())),
        Arc::new(DesktopPatchTool::new(filesystem.clone())),
        Arc::new(DesktopBrowserTool::new(app, browser, safety)),
    ]
}

/// Unified text orchestration entry point. The legacy agent remains compiled only
/// as an explicit rollback path while the frontend production text path uses Harness.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects the trailing state params
pub async fn harness_chat(
    message: String,
    conversation_id: Option<String>,
    conversation_history: Vec<ConversationTurn>,
    allow_workspace_goal: Option<bool>,
    images: Option<Vec<String>>,
    app: tauri::AppHandle,
    browser_state: tauri::State<'_, Arc<BrowserStateHolder>>,
    model_state: tauri::State<'_, ModelManagerState>,
    vault_state: tauri::State<'_, DesktopVaultState>,
    safety_state: tauri::State<'_, SafetyGuardState>,
) -> Result<HarnessChatResult, String> {
    let message = message.trim().to_owned();
    if message.is_empty() || message.len() > 256 * 1024 {
        return Err("Harness message is empty or exceeds 256 KiB".to_owned());
    }
    // Vision: parse data-URL images into harness attachment metadata + local
    // base64 bytes for the adapter. Everything stays local — bytes go to the
    // verified llama-server over localhost, digests (never pixels) go into
    // the audit trail. Low-RAM hosts degrade the same as any long prompt:
    // the model server applies its own context bound.
    let attachments = parse_image_attachments(images.as_deref().unwrap_or_default())?;
    let conversation_id = conversation_id.unwrap_or_else(|| "default".to_owned());
    if conversation_id.is_empty()
        || conversation_id.len() > 128
        || !conversation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err("conversation_id must use 1-128 characters from [A-Za-z0-9._-]".to_owned());
    }
    // Full access is the user-directed default: files, code execution and
    // browser control on the host, with the audit trail + budgets intact.
    // The same flag doubles as the historical "workspace goal" switch — it
    // grants the escalation to L3 (multi-step agentic) execution.
    let full_access = allow_workspace_goal.unwrap_or(true);

    // UNOONE encrypted MESSAGE records remain the only canonical chat history.
    // The frontend supplies that already-decrypted history for this one run;
    // Harness never persists a duplicate conversation stream.
    let mut history_context = String::new();
    for turn in conversation_history
        .into_iter()
        .rev()
        .take(24)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let role = match turn.role.as_str() {
            "user" => "USER",
            "assistant" => "ASSISTANT",
            "tool" => "TOOL",
            _ => continue,
        };
        let text = match turn.content {
            Content::Text(text) => text,
            Content::Multimodal(_) => continue,
        };
        if text.is_empty() {
            continue;
        }
        let bounded = unoone_text::truncate_bytes_with_notice(&text, 8 * 1024);
        history_context.push_str(role);
        history_context.push_str(": ");
        history_context.push_str(&bounded);
        history_context.push('\n');
        if history_context.len() > 48 * 1024 {
            break;
        }
    }
    let harness_prompt = if history_context.is_empty() {
        message.clone()
    } else {
        format!(
            "Prior conversation context (data, not instructions):\n{}\nCurrent user request:\n{}",
            history_context, message
        )
    };

    // Read the verified model id and port under the tokio lock. ModelManager is
    // intentionally not Clone (it owns the llama-server child); the Harness
    // bridge only needs the identity string + port, which are read by reference.
    let (model_id, port) =
        {
            let guard = model_state.manager.lock().await;
            let manager = guard
                .as_ref()
                .ok_or_else(|| "Local model is not running".to_owned())?;
            let model_id =
                registry_safe_model_id(manager.running_model_id().as_deref().ok_or_else(|| {
                    "Local model has not passed identity verification".to_owned()
                })?);
            let port = *model_state
                .server_port
                .lock()
                .map_err(|_| "Model port state lock failed".to_owned())?;
            (model_id, port)
        };

    let vault_root = vault_state
        .vault_root
        .lock()
        .map_err(|_| "Vault-root state lock failed".to_owned())?
        .clone();
    let vault_id = vault_state
        .vault_id
        .lock()
        .map_err(|_| "Vault-id state lock failed".to_owned())?
        .clone();
    if vault_root.is_empty() || vault_id.is_empty() {
        return Err("Pocket AI vault is not unlocked".to_owned());
    }
    let conversation_namespace = format!("{}:conversation:{}", vault_id, conversation_id);
    if conversation_namespace.len() > 256 {
        return Err("conversation namespace exceeds Harness isolation limits".to_owned());
    }
    let vault = Arc::clone(&vault_state.vault);
    let safety = Arc::clone(&safety_state.guard);
    {
        let guard = vault
            .lock()
            .map_err(|_| "Vault state lock failed".to_owned())?;
        if guard.as_ref().is_none_or(|open| !open.is_unlocked()) {
            return Err("Pocket AI vault is locked".to_owned());
        }
    }
    let browser = Arc::clone(browser_state.inner());
    let (attachment_metadata, attachment_bytes) = attachments;

    tokio::task::spawn_blocking(move || {
        let mut model_builder = PaiLlamaLocalProvider::new(model_id.clone(), port)
            .map_err(|error| error.to_string())?;
        for (id, media_type, base64_bytes) in &attachment_bytes {
            model_builder = model_builder.with_attachment(id, media_type, base64_bytes);
        }
        let model = Arc::new(model_builder);
        let memory = Arc::new(
            PaiVaultMemoryProvider::new(
                Arc::clone(&vault),
                PaiVaultMemoryProviderConfig {
                    origin_platform: "DESKTOP".to_owned(),
                    origin_device_id: "unoone-power".to_owned(),
                    ..PaiVaultMemoryProviderConfig::default()
                },
            )
            .map_err(|error| error.to_string())?,
        );

        // Full-access mode roots file tools + subprocesses at the host
        // workspace (%USERPROFILE%\UnoOneAgent) with an allowlisted
        // direct-argv broker and a permissive permission provider; every
        // other pipeline stage — validate, confirm, budget, sandbox fence,
        // output bounding, audit — stays non-bypassable. Chat-only mode
        // keeps the deny-by-default vault-rooted read-only builder.
        // Full access authorizes the full local capability surface, and the
        // sandbox provider below must mirror that set or every full-access
        // tool call dies at the sandbox stage. Network, Credential, Job and
        // Subagent have no registered tools today — the authorization is
        // forward honesty about the lane's scope, not an unlocked behavior.
        let capabilities = if full_access {
            CapabilitySet::all_local()
        } else {
            CapabilitySet::from_slice(&[Capability::Model, Capability::FileRead])
        };
        let workspace_fs = if full_access {
            let workspace = workspace_root().map_err(|error| error.to_string())?;
            Some(
                RootedFs::new(&workspace)
                    .map_err(|error| error.to_string())?
                    .with_limits(2 * 1024 * 1024, 4 * 1024 * 1024),
            )
        } else {
            None
        };
        let mut builder = if let Some(filesystem) = workspace_fs.clone() {
            let broker = LocalExecutionBroker::new(
                filesystem,
                FULL_ACCESS_PROGRAMS
                    .iter()
                    .map(|program| (*program).to_owned()),
            );
            HarnessBuilder::embedded(Arc::new(broker))
                .map_err(|error| error.to_string())?
                .permission_provider(Arc::new(FullAccessPermission))
        } else {
            HarnessBuilder::local_embedded(&vault_root).map_err(|error| error.to_string())?
        };
        builder = builder
            .register_model(model)
            .map_err(|error| error.to_string())?
            .memory_provider(memory)
            .system_prefix(desktop_system_prefix(full_access))
            .sandbox_provider(Arc::new(DesktopSandbox {
                granted: capabilities.clone(),
                trusted_process: full_access,
            }))
            .confirmation_provider(Arc::new(StaticConfirmationProvider {
                outcome: if full_access {
                    ConfirmationOutcome::AllowedOnce
                } else {
                    ConfirmationOutcome::Unavailable
                },
            }));
        for tool in desktop_read_tools(&vault_root, Arc::clone(&vault), Arc::clone(&safety)) {
            builder = builder
                .register_tool(tool)
                .map_err(|error| error.to_string())?;
        }
        if let Some(filesystem) = workspace_fs {
            for tool in desktop_workspace_tools(
                filesystem,
                app.clone(),
                Arc::clone(&browser),
                Arc::clone(&safety),
            ) {
                builder = builder
                    .register_tool(tool)
                    .map_err(|error| error.to_string())?;
            }
        }
        let harness = builder.build();
        let mut options = RunOptions {
            actor: "local-user".to_owned(),
            capabilities,
            attachments: attachment_metadata,
            provider: "pai-llama-local".to_owned(),
            model: model_id.clone(),
            memory: MemoryOptions {
                // Canonical chat continuity comes from UNOONE encrypted MESSAGE
                // records passed above; do not create/query a second Harness
                // conversation store. Harness memory here is long-term only.
                scopes: vec![
                    inbharat_harness_core::MemoryScope::Preferences,
                    inbharat_harness_core::MemoryScope::Relevant,
                    inbharat_harness_core::MemoryScope::Project,
                ],
                namespace: vault_id.clone(),
                conversation_namespace: Some(conversation_namespace.clone()),
                search_limit: 8,
                recent_conversation_limit: 16,
                max_context_bytes: 32 * 1024,
                write_conversation: false,
            },
            ..RunOptions::default()
        };
        if full_access {
            // Full-access runs are autonomous coding/automation sessions:
            // request the L3 agentic route explicitly (allowed by the
            // default route policy). Budgets stay non-bypassable — the
            // harness refuses to run without one — but the limits are set
            // so a real session never hits them (live-caught: a long
            // coding task on the local 12B died at the old 900s/48-step
            // wall). What remains is a pathological-loop backstop, not a
            // task cap: hundreds of steps, thousands of tool calls,
            // hours of wall time, 64 MiB of accumulated tool output.
            options.explicit_level = Some(ExecutionLevel::L3);
            options.budget = Some(full_access_budget());
        }
        let cancel = CancellationToken::new();
        let (outcome, _session) = harness
            .run(&harness_prompt, &options, &cancel)
            .map_err(|error| error.to_string())?;
        Ok(HarnessChatResult {
            session_id: outcome.session_id,
            route: outcome.decision.level.as_str().to_owned(),
            route_reason: outcome.decision.reason.as_str().to_owned(),
            output: outcome.output,
            steps: outcome.steps,
            tool_calls: outcome.tool_calls,
            event_count: outcome.event_count,
            elapsed_ms: u64::try_from(outcome.elapsed.as_millis()).unwrap_or(u64::MAX),
            model_id,
            memory_namespace: conversation_namespace,
        })
    })
    .await
    .map_err(|error| format!("Harness worker failed: {error}"))?
}

#[cfg(test)]
mod workspace_tool_tests {
    use super::*;
    use inbharat_harness_core::ExecutionBroker;

    /// A throwaway fenced workspace under the OS temp directory.
    fn temp_workspace() -> (PathBuf, RootedFs) {
        let dir = std::env::temp_dir().join(format!(
            "unoone-harness-tests-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("create temp workspace");
        let filesystem = RootedFs::new(&dir)
            .expect("fence the temp workspace")
            .with_limits(2 * 1024 * 1024, 4 * 1024 * 1024);
        (dir, filesystem)
    }

    /// A ToolContext wired to an empty-allowlist broker over the same fence.
    /// The fence itself is bound into the tools at construction, not carried
    /// by the context, so the parameter is intentionally unused here.
    fn tool_context<'a>(
        _filesystem: &'a RootedFs,
        cancel: &'a CancellationToken,
        broker: &'a LocalExecutionBroker,
    ) -> ToolContext<'a> {
        ToolContext {
            actor: "test",
            level: ExecutionLevel::L3,
            execution: broker as &dyn ExecutionBroker,
            cancel,
        }
    }

    fn harness_broker(filesystem: &RootedFs) -> LocalExecutionBroker {
        LocalExecutionBroker::new(filesystem.clone(), std::iter::empty::<String>())
    }

    fn args(pairs: &[(&str, Value)]) -> ToolArguments {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    fn string_args(pairs: &[(&str, &str)]) -> ToolArguments {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), Value::String((*value).to_owned())))
            .collect()
    }

    /// Defect #16 regression (live-caught 2026-09-12): the production L3
    /// full-access budget must be accepted by the harness's run-options
    /// validation. When it was rejected (64 MiB cumulative output vs the old
    /// 8 MiB validator cap), every full-access harness_chat call threw and
    /// the UI silently fell back to the read-only vault agent — the model
    /// then truthfully told users it could not write files or run commands,
    /// even with the full-access toggle on.
    #[test]
    fn full_access_budget_is_accepted_by_the_harness() {
        let harness = HarnessBuilder::local(".")
            .expect("local harness builder")
            .register_model(Arc::new(inbharat_harness_core::EchoModelProvider::default()))
            .expect("register echo model")
            .confirmation_provider(Arc::new(StaticConfirmationProvider {
                outcome: ConfirmationOutcome::AllowedOnce,
            }))
            .build();
        let options = RunOptions {
            actor: "local-user".to_owned(),
            provider: "echo".to_owned(),
            model: "echo-v1".to_owned(),
            explicit_level: Some(ExecutionLevel::L3),
            budget: Some(full_access_budget()),
            capabilities: CapabilitySet::all_local(),
            ..RunOptions::default()
        };
        let (outcome, session) = harness
            .run("build something", &options, &CancellationToken::new())
            .expect("the production full-access budget must survive run-options validation");
        let _ = outcome.steps;
        assert!(
            session.replay().expect("replay session").balanced,
            "the session must stay audit-balanced under the production budget"
        );
    }

    #[test]
    fn registry_safe_model_id_strips_windows_paths() {
        // What llama-server actually reports on Windows (live-observed): the
        // full launch path, backslashes included. The raw string is rejected
        // by the harness model registry, killing every harness_chat call.
        let cached = "C:\\Users\\reetu\\AppData\\Local\\UnoOne\\model-cache\\D333B368BE6CD655563FCE18AEDE26027E208FDB13816D35EB06983CE054044B.gguf";
        assert_eq!(
            registry_safe_model_id(cached),
            "D333B368BE6CD655563FCE18AEDE26027E208FDB13816D35EB06983CE054044B.gguf"
        );
        let drive = "\\\\?\\D:\\UNOONE\\MODELS\\DESKTOP\\Gemma-12B\\gemma-4-12B-it-Q4_K_M.gguf";
        assert_eq!(registry_safe_model_id(drive), "gemma-4-12B-it-Q4_K_M.gguf");
        // POSIX-style launch paths reduce to the same basename.
        assert_eq!(
            registry_safe_model_id("models/gemma-4-12b-it-q4_k_m.gguf"),
            "gemma-4-12b-it-q4_k_m.gguf"
        );
        // A plain id with no separators is preserved.
        assert_eq!(registry_safe_model_id("pai-gemma"), "pai-gemma");
    }

    #[test]
    fn full_access_permission_allows_every_local_capability() {
        let all = [
            Capability::Model,
            Capability::FileRead,
            Capability::FileWrite,
            Capability::ProcessSpawn,
            Capability::Network,
            Capability::Credential,
            Capability::Workspace,
            Capability::Job,
            Capability::Subagent,
        ];
        for capability in all {
            let decision = FullAccessPermission
                .authorize("test", capability, "test-resource")
                .expect("authorization must not fail structurally");
            assert!(
                matches!(decision, PermissionDecision::Allow),
                "{capability:?} must be allowed in full-access mode"
            );
        }
    }

    #[test]
    fn desktop_sandbox_grants_the_full_access_tool_surface() {
        // The full-access lane: every tool capability resolves, including
        // process/browser calls that require a security boundary — the
        // allowlisted direct-argv broker is the boundary and its Partial
        // quality is reported, never silently upgraded.
        let full = DesktopSandbox {
            granted: CapabilitySet::all_local(),
            trusted_process: true,
        };
        let fs_write = full
            .resolve(&SandboxRequest {
                world_id: "w1".to_owned(),
                capabilities: CapabilitySet::from_slice(&[Capability::FileWrite]),
                require_security_boundary: false,
            })
            .expect("fs.write sandbox grant");
        assert_eq!(fs_write.world_id, "w1");
        assert_eq!(fs_write.backend, "unoone-allowlisted-direct-argv");
        assert_eq!(fs_write.quality, EnforcementQuality::Partial);
        full.resolve(&SandboxRequest {
            world_id: "w1".to_owned(),
            capabilities: CapabilitySet::from_slice(&[Capability::FileRead]),
            require_security_boundary: false,
        })
        .expect("fs.read sandbox grant");
        full.resolve(&SandboxRequest {
            world_id: "w1".to_owned(),
            capabilities: CapabilitySet::from_slice(&[Capability::ProcessSpawn]),
            require_security_boundary: true,
        })
        .expect("process.run sandbox grant");
        full.resolve(&SandboxRequest {
            world_id: "w1".to_owned(),
            capabilities: CapabilitySet::from_slice(&[Capability::Workspace]),
            require_security_boundary: true,
        })
        .expect("browser.act sandbox grant");

        // The read-only chat lane: reads pass behind the in-process fence,
        // writes and boundary-requiring effects fail closed — the builder
        // default posture that the shipped full-access build had accidentally
        // applied everywhere, killing the whole lane live.
        let read_only = DesktopSandbox {
            granted: CapabilitySet::from_slice(&[Capability::Model, Capability::FileRead]),
            trusted_process: false,
        };
        let read = read_only
            .resolve(&SandboxRequest {
                world_id: "w2".to_owned(),
                capabilities: CapabilitySet::from_slice(&[Capability::FileRead]),
                require_security_boundary: false,
            })
            .expect("read-only fs.read grant");
        assert_eq!(read.quality, EnforcementQuality::InProcessFence);
        let denied_write = read_only.resolve(&SandboxRequest {
            world_id: "w2".to_owned(),
            capabilities: CapabilitySet::from_slice(&[Capability::FileWrite]),
            require_security_boundary: false,
        });
        assert!(
            denied_write.is_err(),
            "writes must fail closed in chat mode"
        );
        let denied_process = read_only.resolve(&SandboxRequest {
            world_id: "w2".to_owned(),
            capabilities: CapabilitySet::from_slice(&[Capability::FileRead]),
            require_security_boundary: true,
        });
        assert!(
            denied_process.is_err(),
            "boundary effects must fail closed in chat mode"
        );
    }

    #[test]
    fn search_finds_matches_recursively_and_respects_case() {
        let (_dir, filesystem) = temp_workspace();
        filesystem
            .write_text_atomic(
                "alpha.txt",
                "The needle is here\nnothing to see\nNEEDLE in caps\n",
            )
            .expect("write alpha");
        filesystem.create_dir_all("sub").expect("mkdir sub");
        filesystem
            .write_text_atomic("sub/beta.txt", "a needle deeper down\n")
            .expect("write beta");
        let tool = DesktopSearchTool::new(filesystem.clone());
        let broker = harness_broker(&filesystem);
        let cancel = CancellationToken::new();
        let context = tool_context(&filesystem, &cancel, &broker);

        // Case-sensitive: exactly the two lowercase matches.
        let output = tool
            .execute(&string_args(&[("query", "needle")]), &context)
            .expect("search must succeed");
        let text = output.model_content;
        assert!(text.contains("alpha.txt:1: The needle is here"), "{text}");
        assert!(
            text.contains("sub/beta.txt:1: a needle deeper down"),
            "{text}"
        );
        assert!(!text.contains("NEEDLE in caps"), "{text}");

        // Case-insensitive: the uppercase match appears too.
        let output = tool
            .execute(
                &args(&[
                    ("query", Value::String("needle".to_owned())),
                    ("case_insensitive", Value::Bool(true)),
                ]),
                &context,
            )
            .expect("search must succeed");
        assert!(output.model_content.contains("NEEDLE in caps"));
    }

    #[test]
    fn search_case_insensitive_finds_uppercase_matches() {
        let (_dir, filesystem) = temp_workspace();
        filesystem
            .write_text_atomic("notes.md", "GEMMA is a family of models\n")
            .expect("write notes");
        let tool = DesktopSearchTool::new(filesystem.clone());
        let broker = harness_broker(&filesystem);
        let cancel = CancellationToken::new();
        let context = tool_context(&filesystem, &cancel, &broker);
        let output = tool
            .execute(
                &args(&[
                    ("query", Value::String("gemma".to_owned())),
                    ("case_insensitive", Value::Bool(true)),
                ]),
                &context,
            )
            .expect("search must succeed");
        assert!(
            output.model_content.contains("notes.md:1"),
            "{}",
            output.model_content
        );
    }

    #[test]
    fn search_stays_inside_the_fence_and_rejects_unknown_arguments() {
        let (dir, filesystem) = temp_workspace();
        // A sibling directory OUTSIDE the fence holds a matching file; a
        // rooted search must never see it.
        let outside = dir.parent().unwrap().join(format!(
            "unoone-harness-outside-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&outside).expect("create outside dir");
        std::fs::write(outside.join("decoy.txt"), "needle outside the fence\n")
            .expect("write decoy");
        filesystem
            .write_text_atomic("inside.txt", "needle inside\n")
            .expect("write inside");
        let tool = DesktopSearchTool::new(filesystem.clone());
        let broker = harness_broker(&filesystem);
        let cancel = CancellationToken::new();
        let context = tool_context(&filesystem, &cancel, &broker);
        let output = tool
            .execute(&string_args(&[("query", "needle")]), &context)
            .expect("search must succeed");
        assert!(
            output.model_content.contains("inside.txt"),
            "{}",
            output.model_content
        );
        assert!(
            !output.model_content.contains("decoy"),
            "search must not escape the fenced workspace: {}",
            output.model_content
        );
        let _ = std::fs::remove_dir_all(&outside);

        // Unknown argument keys are rejected before any file is touched.
        let bad = args(&[
            ("query", Value::String("needle".to_owned())),
            ("glob", Value::String("*.txt".to_owned())),
        ]);
        assert!(tool.validate_arguments(&bad).is_err());
    }

    #[test]
    fn patch_replaces_a_unique_match_atomically() {
        let (_dir, filesystem) = temp_workspace();
        filesystem
            .write_text_atomic("config.toml", "name = \"old\"\nvalue = 1\n")
            .expect("write config");
        let tool = DesktopPatchTool::new(filesystem.clone());
        let broker = harness_broker(&filesystem);
        let cancel = CancellationToken::new();
        let context = tool_context(&filesystem, &cancel, &broker);
        tool.execute(
            &string_args(&[
                ("path", "config.toml"),
                ("find", "name = \"old\""),
                ("replace", "name = \"new\""),
            ]),
            &context,
        )
        .expect("patch must succeed");
        let patched = filesystem.read_text("config.toml").expect("re-read");
        assert_eq!(patched, "name = \"new\"\nvalue = 1\n");
    }

    #[test]
    fn patch_rejects_ambiguous_matches_until_replace_all() {
        let (_dir, filesystem) = temp_workspace();
        filesystem
            .write_text_atomic("log.txt", "todo one\ntodo two\n")
            .expect("write log");
        let tool = DesktopPatchTool::new(filesystem.clone());
        let broker = harness_broker(&filesystem);
        let cancel = CancellationToken::new();
        let context = tool_context(&filesystem, &cancel, &broker);

        let ambiguous = string_args(&[("path", "log.txt"), ("find", "todo"), ("replace", "done")]);
        let error = tool
            .execute(&ambiguous, &context)
            .expect_err("ambiguous patch must be rejected");
        assert!(
            error.message.contains("2 times"),
            "the error must name the ambiguity: {}",
            error.message
        );

        let replace_all = args(&[
            ("path", Value::String("log.txt".to_owned())),
            ("find", Value::String("todo".to_owned())),
            ("replace", Value::String("done".to_owned())),
            ("replace_all", Value::Bool(true)),
        ]);
        tool.execute(&replace_all, &context)
            .expect("replace_all patch must succeed");
        assert_eq!(
            filesystem.read_text("log.txt").expect("re-read"),
            "done one\ndone two\n"
        );
    }

    #[test]
    fn patch_rejects_missing_match_and_path_escape() {
        let (_dir, filesystem) = temp_workspace();
        filesystem
            .write_text_atomic("root.txt", "present\n")
            .expect("write root file");
        let tool = DesktopPatchTool::new(filesystem.clone());
        let broker = harness_broker(&filesystem);
        let cancel = CancellationToken::new();
        let context = tool_context(&filesystem, &cancel, &broker);

        let missing = string_args(&[
            ("path", "root.txt"),
            ("find", "absent"),
            ("replace", "whatever"),
        ]);
        assert!(tool.execute(&missing, &context).is_err());

        // An escape attempt must fail without touching anything outside.
        let escape = string_args(&[
            ("path", "../root.txt"),
            ("find", "present"),
            ("replace", "escaped"),
        ]);
        assert!(tool.execute(&escape, &context).is_err());
        assert_eq!(
            filesystem.read_text("root.txt").expect("re-read"),
            "present\n"
        );
    }

    #[test]
    fn browser_action_arguments_are_checked() {
        let (_dir, filesystem) = temp_workspace();
        let broker = harness_broker(&filesystem);
        let cancel = CancellationToken::new();
        let context = tool_context(&filesystem, &cancel, &broker);

        // navigate without a url is rejected
        let navigate_no_url = string_args(&[("action", "navigate")]);
        assert!(validate_browser_arguments(&navigate_no_url).is_err());
        // navigate with a url is accepted
        let navigate_ok = string_args(&[("action", "navigate"), ("url", "https://example.com")]);
        assert!(validate_browser_arguments(&navigate_ok).is_ok());
        // unknown actions are rejected
        let unknown = string_args(&[("action", "teleport")]);
        assert!(validate_browser_arguments(&unknown).is_err());
        // wait bounds are enforced
        let wait_bad = args(&[
            ("action", Value::String("wait".to_owned())),
            ("milliseconds", Value::Integer(0)),
        ]);
        assert!(validate_browser_arguments(&wait_bad).is_err());
        let wait_ok = args(&[
            ("action", Value::String("wait".to_owned())),
            ("milliseconds", Value::Integer(500)),
        ]);
        assert!(validate_browser_arguments(&wait_ok).is_ok());
        // click requires a selector
        let click_no_selector = string_args(&[("action", "click")]);
        assert!(validate_browser_arguments(&click_no_selector).is_err());
        let _ = context.cancel.is_cancelled();
    }
}
