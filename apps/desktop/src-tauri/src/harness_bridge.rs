//! Pocket AI desktop embedding for the reusable InBharat Harness.
//!
//! Migration posture: this is exposed beside the existing ReAct agent until
//! acceptance tests prove parity. The bridge uses the same verified llama
//! process, the same encrypted vault and the same document/security helpers;
//! it does not create a second model runtime or persistence store.

#[path = "chat_context.rs"]
mod chat_context;

use crate::{
    browser::{self, BrowserAction, BrowserStateHolder, ScrollDirection},
    documents,
    granted_fs::{DeniedPathResolution, GrantedFolderBroker, GrantedFolders},
    llama::{Content, ConversationTurn, ModelManagerState},
    safety::{DesktopSafetyGuard, SafetyGuardState, ToolAction},
    security, DesktopVaultState,
};
use inbharat_harness_core::jobs::{run_scoped_subagent, SubagentProvider};
use inbharat_harness_core::providers::{EnforcementQuality, SandboxGrant, SandboxRequest};
use inbharat_harness_core::{
    tools::{
        CopyFileTool, ListFilesTool, MakeDirTool, ReadFileTool, RunProcessTool, WriteFileTool,
    },
    AttachmentMetadata, BudgetLimits, CancelCause, CancellationToken, Capability, CapabilitySet,
    ConfirmationMode, ConfirmationOutcome, Determinism, ErrorCode, ExecutionLevel, Failure,
    FailureClass, HarnessBuilder, HarnessResult, MemoryOptions, PermissionDecision,
    PermissionProvider, RootedFs, RunOptions, SandboxProvider, SideEffect,
    StaticConfirmationProvider, SubagentRequest, SubagentResult, Tool, ToolArguments, ToolContext,
    ToolManifest, ToolOutput, Value,
};
use pai_harness_adapter::{
    PaiLlamaLocalProvider, PaiVaultMemoryProvider, PaiVaultMemoryProviderConfig,
};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, Manager};
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
    /// Context omissions, per-turn shortening and estimated byte-budget
    /// limitations — rendered by ChatView so context changes are never silent.
    /// This is not a tokenizer-exact guarantee for the complete model input.
    #[serde(default)]
    pub context_note: Option<String>,
    pub personal_binding: Option<pai_harness_adapter::personal_execution::Binding>,
}

/// Gap 1 (2026-09-16): one generated token of a plain (tool-free) chat
/// answer, streamed to the chat panel as it is produced. The adapter's
/// token tap forwards each SSE delta; `conversation_id` lets the UI ignore
/// tokens from a conversation it is no longer showing.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ChatTokenEvent {
    pub conversation_id: String,
    pub delta: String,
}

/// One live-activity line streamed to the chat panel while the agent runs.
/// Live-caught 2026-09-14 (user directive: "the chat panel should show what
/// it is doing, what codes it's writing — like Codex/GLM"): a multi-file
/// build previously showed a bare spinner for minutes while the model wrote
/// whole files, so a real run looked frozen. `ProgressTool` emits these as
/// Tauri `agent-progress` events around every tool execution.
#[derive(Clone, Debug, serde::Serialize)]
pub struct AgentProgressEvent {
    /// "call" before the tool runs, "result" after it returns.
    pub phase: &'static str,
    pub tool: String,
    /// Human summary, e.g. "Writing task-board/index.html (1,874 bytes)".
    pub detail: String,
    /// Short head of the code being written (fs.write contents) so the user
    /// can literally watch the file appear, Codex-style. Bounded tightly.
    pub code_preview: Option<String>,
    /// Local wall-clock "HH:MM:SS" of the event — Codex/GLM timestamp every
    /// action and the user asked for the same (defect #37, live-caught
    /// 2026-09-14: "why can't we see when it was built like we can in other
    /// AI"). Bounded by the event, not the run: each line carries its own
    /// time so a long run reads like a timeline.
    pub at: String,
    /// The result's presentation kind (e.g. "web-preview"), so the chat
    /// feed can offer the affordances that belong to it — call events and
    /// plain results carry `None`.
    pub kind: Option<String>,
}

/// Local wall-clock "HH:MM:SS" for a progress event.
fn progress_timestamp() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Short, human phrasing of what a tool call is about to do.
fn progress_detail(tool: &str, arguments: &ToolArguments) -> String {
    let arg = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    match tool {
        "fs.write" => {
            let path = arg("path");
            let bytes = arguments
                .get("contents")
                .and_then(Value::as_str)
                .map(str::len);
            match (path.is_empty(), bytes) {
                (false, Some(len)) => format!("Writing {path} ({len} bytes)"),
                (false, None) => format!("Writing {path}"),
                _ => "Writing file".to_owned(),
            }
        }
        "fs.read" => {
            let path = arg("path");
            if path.is_empty() {
                "Reading file".to_owned()
            } else {
                format!("Reading {path}")
            }
        }
        "fs.list" => {
            let path = arg("path");
            if path.is_empty() {
                "Listing directory".to_owned()
            } else {
                format!("Listing {path}")
            }
        }
        "fs.mkdir" => {
            let path = arg("path");
            if path.is_empty() {
                "Creating directory".to_owned()
            } else {
                format!("Creating directory {path}")
            }
        }
        "fs.copy" => {
            let from = arg("from");
            let to = arg("to");
            match (from.is_empty(), to.is_empty()) {
                (false, false) => format!("Copying {from} → {to}"),
                (false, true) => format!("Copying {from}"),
                _ => "Copying file".to_owned(),
            }
        }
        "process.run" => {
            let program = arg("program");
            let background = matches!(arguments.get("background"), Some(Value::Bool(true)));
            match (program.is_empty(), background) {
                (true, false) => "Running command".to_owned(),
                (false, false) => format!("Running {program}"),
                (true, true) => "Starting background process".to_owned(),
                (false, true) => format!("Starting {program} in background"),
            }
        }
        "browser.act" => {
            let action = arg("action");
            let url = arg("url");
            match (action.is_empty(), url.is_empty()) {
                (false, false) => format!("Browser: {action} {url}"),
                (false, true) => format!("Browser: {action}"),
                _ => "Driving the browser".to_owned(),
            }
        }
        "workspace.search" => "Searching the workspace".to_owned(),
        "workspace.patch" => "Patching a workspace file".to_owned(),
        "doc.create" => {
            let filename = arg("filename");
            if filename.is_empty() {
                "Creating a document".to_owned()
            } else {
                format!("Creating document {filename}")
            }
        }
        "web.preview" => {
            let path = arg("path");
            if path.is_empty() {
                "Opening a live preview".to_owned()
            } else {
                format!("Opening a live preview of {path}")
            }
        }
        "agent.spawn" => {
            let task = arg("task");
            if task.is_empty() {
                "Spawning a sub-agent".to_owned()
            } else {
                let head: String = task.trim().chars().take(90).collect();
                format!("Spawning a sub-agent: {head}…")
            }
        }
        _ => {
            let canonical = Value::Object(
                arguments
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), value.clone()))
                    .collect(),
            )
            .to_canonical_json();
            if canonical.len() > 160 {
                format!("{tool} {canonical:.160}…")
            } else {
                format!("{tool} {canonical}")
            }
        }
    }
}

/// Head of the contents an fs.write is about to create, for the
/// watch-it-write activity feed. Bounded so a huge write cannot flood the
/// IPC channel; newlines are kept for the frontend's pre-formatted render.
fn progress_code_preview(tool: &str, arguments: &ToolArguments) -> Option<String> {
    if tool != "fs.write" {
        return None;
    }
    let contents = arguments.get("contents")?.as_str()?;
    if contents.is_empty() {
        return None;
    }
    Some(contents.chars().take(240).collect())
}

/// P7 run trail (2026-10-01, user directive: "the memory of the drive should
/// have the context and steps and what was done"). One bounded entry per live
/// tool call/result, captured by the same `ProgressTool` wrapper that
/// streams the UI feed, so a completed agent run leaves an honest step-by-step
/// trail in the vault memory instead of only counts. Bounds: the trail stops
/// recording past `MAX_TRAIL_STEPS` with one explicit overflow marker — a
/// pathological 10,000-step run must never produce an unbounded record.
const MAX_TRAIL_STEPS: usize = 250;
const MAX_TRAIL_DETAIL_CHARS: usize = 160;

type SharedAgentTrail = Arc<Mutex<Vec<AgentTrailStep>>>;

#[derive(Clone)]
pub(crate) struct AgentTrailStep {
    /// "call" before the tool runs, "result" after it returns.
    pub(crate) phase: &'static str,
    pub(crate) tool: String,
    pub(crate) detail: String,
}

/// Append one bounded step to the shared trail. Best-effort by design: a
/// poisoned lock or a missing trail (unit-test registrations) is a skipped
/// entry, never a broken tool run.
fn push_trail_step(
    trail: &Option<SharedAgentTrail>,
    phase: &'static str,
    tool: &str,
    detail: &str,
) {
    let Some(trail) = trail else { return };
    let Ok(mut steps) = trail.lock() else { return };
    if steps.len() > MAX_TRAIL_STEPS {
        return;
    }
    let bounded: String = detail.chars().take(MAX_TRAIL_DETAIL_CHARS).collect();
    steps.push(AgentTrailStep {
        phase,
        tool: tool.to_owned(),
        detail: bounded,
    });
    if steps.len() == MAX_TRAIL_STEPS {
        steps.push(AgentTrailStep {
            phase: "result",
            tool: "(trail truncated)".to_owned(),
            detail: format!(
                "recording stopped at {MAX_TRAIL_STEPS} steps; the run continued but is not recorded step-by-step"
            ),
        });
    }
}

/// Wraps every registered tool so the chat panel can show live agent
/// activity (2026-09-14). Transparent to the harness — manifest and
/// validation delegate unchanged; execution emits a `call` event, runs the
/// inner tool, then emits a `result` event. Emission is best-effort: a UI
/// without listeners or an emit failure must never break a real tool run.
struct ProgressTool {
    inner: Arc<dyn Tool>,
    app: tauri::AppHandle,
    /// Prepended to every emitted detail line. Sub-agent tool runs set this
    /// to "[subagent-xxxx]" so the user can tell child activity apart from
    /// the parent agent's in the same live feed.
    detail_prefix: Option<String>,
    /// P7 run trail collector. `None` in unit-test registrations (no run to
    /// record). Sub-agent tools share the PARENT's collector so one trail
    /// covers everything the run actually did, child steps tagged by their
    /// existing "[subagent-xxxx]" prefix.
    trail: Option<SharedAgentTrail>,
}

impl Tool for ProgressTool {
    fn manifest(&self) -> &ToolManifest {
        self.inner.manifest()
    }
    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        self.inner.validate_arguments(arguments)
    }
    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        let tool = self.inner.manifest().id.clone();
        let detail = match &self.detail_prefix {
            Some(prefix) => format!("{prefix} {}", progress_detail(&tool, arguments)),
            None => progress_detail(&tool, arguments),
        };
        let preview = progress_code_preview(&tool, arguments);
        push_trail_step(&self.trail, "call", &tool, &detail);
        let _ = self.app.emit(
            "agent-progress",
            AgentProgressEvent {
                phase: "call",
                tool: tool.clone(),
                detail: detail.clone(),
                code_preview: preview,
                at: progress_timestamp(),
                kind: None,
            },
        );
        match self.inner.execute(arguments, context) {
            Ok(output) => {
                let mut summary = output.model_content.clone();
                if summary.len() > 160 {
                    summary.truncate(160);
                    summary.push('…');
                }
                push_trail_step(&self.trail, "result", &tool, &format!("Done: {summary}"));
                let _ = self.app.emit(
                    "agent-progress",
                    AgentProgressEvent {
                        phase: "result",
                        tool: tool.clone(),
                        detail: format!("Done: {summary}"),
                        code_preview: None,
                        at: progress_timestamp(),
                        kind: output.presentation.get("kind").map(|kind| kind.to_string()),
                    },
                );
                Ok(output)
            }
            Err(failure) => {
                push_trail_step(&self.trail, "result", &tool, &format!("Failed: {failure}"));
                let _ = self.app.emit(
                    "agent-progress",
                    AgentProgressEvent {
                        phase: "result",
                        tool: tool.clone(),
                        detail: format!("Failed: {failure}"),
                        code_preview: None,
                        at: progress_timestamp(),
                        kind: None,
                    },
                );
                Err(failure)
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum DesktopToolKind {
    SearchNotes,
    ListDocuments,
    ReadDocument,
    VerifyVault,
}

pub(crate) struct DesktopReadTool {
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

pub(crate) fn desktop_read_tools(
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
    // P7 folder grants: every additional folder the user granted this
    // agent is enumerated truthfully — the briefing must never claim more
    // or less reach than the fence actually enforces.
    let granted_lines = granted_folder_roots()
        .iter()
        .map(|folder| format!("\n- plus the user-granted folder: {}", folder.display()))
        .collect::<Vec<_>>()
        .join("");
    if full_access {
        format!(
            "You are UnoOne, the user's private Pocket AI running locally on their Windows \
             computer (fully offline, no cloud). You are NOT limited to a vault: in this \
             session you have full agent tools, all audited and budgeted.\n\
             - Read/write/list/search/patch files in the workspace folder: {workspace}\n\
             (give tool paths relative to that folder, or as absolute paths inside \
             it — both are accepted and fenced to it){granted_lines}\n\
             - A path outside every granted folder opens an approval card in \
             the app where the user can Grant that folder: tell the user to \
             click Grant there, wait for the tool call to finish, and it \
             completes on the widened access. A decline (or no answer) means \
             the folder stays off-limits — never claim a file outside the \
             grants was read or written.\n\
             - Host commands require separate session permission in Settings. Folder grants \
             do not sandbox programs: commands can access host files and the network. \
             A denial means ask the user to enable permission and restart the task. \
             Run programs directly (git, cargo, rustc, node, npm, npx, python, pip, \
             dotnet, go, java, cmake, make, gcc, clang, powershell) inside that workspace\n\
             - Deploy long-running processes (servers, watchers): pass \
             background:true to process.run — it returns immediately with the \
             pid and the process keeps running until lock, unplug, command revocation or app exit. Its output is NOT captured, so \
             verify the effect itself (browser.act to the served \
             http://localhost:PORT and check the page) and report the pid in \
             your answer so the user can stop it later.\n\
             - Drive a real web browser (navigate, click, type, fill forms, screenshot) \
             via browser.act — the browser session is PERSISTENT: every \
             browser window shares the machine's WebView2 profile, so a site \
             the user logged into once (Gmail, Notion, Instagram, webmail) \
             stays logged in across app restarts. Reach the user's accounts \
             through that browser session and work on their email, notes and \
             pages directly — NO API keys or OAuth apps are involved or ever \
             needed. Never ask the user for their password and never type \
             credentials for them: if a site needs a login that is not \
             already active, tell the user to log in themselves in the \
             Browser window, then continue once they have. Sign-in buttons \
             that open a new tab/popup work normally.\n\
             - Create real downloadable documents (PDF, DOCX, MD, TXT) from plain \
             text via doc.create — give `filename` plus the full `content`; the \
             user can open the result immediately\n\
             - Open a live website preview via web.preview — write the site \
             files first, then give the entry .html path; the preview window \
             reloads itself as you keep editing, the user watches it build, \
             and no web server is started\n\
             - Build DESIGNED, visually rich websites — modern landing pages, \
             portfolios, dashboards — not plain text pages: use CSS gradients, \
             flexbox/grid layouts, cards with shadows and rounded corners, \
             web fonts, animations, a coherent color palette, and author any \
             graphic (logo, icon, illustration) yourself as inline SVG inside \
             the HTML so it scales crisply. When the user's own image files \
             (PNG, JPG, GIF, WebP) should appear in the site or a folder, copy \
             them with fs.copy — it is binary-safe and byte-exact, while \
             fs.read + fs.write would CORRUPT images (they are UTF-8 \
             text-only). Reference copied images with a relative <img> tag \
             and open the site with web.preview. Never claim images or \
             graphics are impossible — SVG authoring and fs.copy cover both\n\
             - Spawn sub-agents via agent.spawn to complete complex work: give \
             each one a COMPLETE, self-contained task (every path and detail, \
             because it sees nothing else — not even this conversation) and \
             its finished report comes back as the tool's output. Spawn a \
             sub-agent when a task splits into independent pieces (e.g. one \
             writes the tests while you build the app, or one explores a \
             problem while you build the main path), or to get a fresh \
             independent pass that double-checks risky work. Sub-agents have \
             your same tools, up to 2 levels deep. Treat their reports as \
             claims: verify anything important before relying on it, and \
             fold the result into your own answer — the user never talks to \
             the sub-agent directly.\n\
             - Read the user's encrypted Pocket AI vault records \
             (search_notes, list_documents, read_document, verify_vault)\n\
             When a task needs any of this, actually use the tools instead of claiming \
             you cannot. If a request falls outside what the tools above can reach, say \
             so honestly and specifically.\n\
             Before any substantial build (a website, an app, a design, a long \
             document), ask the user the few questions whose answers materially \
             change the result — style, tech stack, language, scope — IF they \
             have not already said. Keep it to ONE short message with 2-4 \
             questions asked together, then WAIT for the answer; never start \
             building and ask later, and never drag it out one question at a \
             time. Small or fully-specified tasks need no questions: when the \
             request is already clear, build it.\n\
             You are an autonomous agent: when the user asks you to build, create, \
             write, or fix something, do the whole task yourself with the tools — \
             create every file with fs.write (missing parent folders are created \
             automatically, so you can write a nested file in one call), run and \
             verify the result with process.run, read back what you wrote with \
             fs.read, and keep going until \
             the task is genuinely done. A failing test or command is the next \
             step of the task, not the end: read the exact error output, inspect \
             the relevant files to find the concrete cause, fix it, and re-run — \
             iterate like this until the test passes or you have positively \
             established why it cannot. Never report the task complete or claim \
             code is 'functional' while its own output shows a failure, and never \
             explain a failure away with environment speculation when the output \
             points at a defect in the files you wrote. When a task creates or \
             changes files, end your answer by listing the exact ABSOLUTE path \
             of every file you created or changed (the workspace root is \
             {workspace}), so the user can find and open them — never say only \
             'in your workspace' or a bare filename. Never paste code or file \
             contents into the \
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
/// calls, 24 h of wall time, 64 MiB of accumulated tool output.
/// The user's standing directive is "no cap" — the full-access lane runs at
/// the validator ceiling in every dimension, so no real task is ever cut
/// short by a desktop-side budget. Sub-agent depth is 2 (2026-09-15
/// multi-agent lane: `agent.spawn` delegates to nested harness runs; the
/// ceiling matches `SUBAGENT_DEPTH_CEILING` and `run_scoped_subagent`
/// enforces it). Only jobs stay at 0 — the desktop lane runs no job
/// queues today.
/// Live-caught 2026-09-12 (defect #16): this shape must stay within the
/// harness's hard safety bounds — the 64 MiB cumulative output figure was
/// once rejected by the validator, every full-access chat call threw, and
/// the UI silently fell back to the read-only vault agent (the model then
/// truthfully told the user it could not write files).
/// Live-caught 2026-09-13 (defect #26): `max_rounds` MUST be 1 here. The
/// harness's goal loop divides the step budget across rounds
/// (`steps_per_round = max_steps / max_rounds`), and with 1,000 rounds every
/// round got only 10 model steps. A real long-coding task needs 15–25; the
/// loop died after 10 steps with a fatal "budget_exceeded:agent.loop: agent
/// step budget exhausted before completion", the UI fell back to the
/// read-only agent, and the user got a confident "I cannot write files"
/// denial with the full-access toggle ON. Goal rounds only exist to retry
/// verification failures — and this lane's verifier
/// (`CanonicalVerificationProvider`) passes any output under 8 MiB, so
/// round 1 always satisfies it and rounds 2+ can never trigger. Revisit ONLY
/// if a real goal verifier lands; until then 1 round = all 10,000 steps
/// available to the model loop, with the shared `budget.reserve_step` still
/// enforcing the global 10,000-step cap.
/// `full_access_budget_is_accepted_by_the_harness` pins it.
fn full_access_budget() -> BudgetLimits {
    BudgetLimits {
        max_steps: 10_000,
        max_tool_calls: 100_000,
        max_rounds: 1,
        max_jobs: 0,
        max_subagent_depth: SUBAGENT_DEPTH_CEILING,
        max_output_bytes: 64 * 1024 * 1024,
        max_duration: Duration::from_secs(24 * 60 * 60),
    }
}

/// The coding/automation workspace root. Full-access file tools and
/// subprocesses are rooted here — never the encrypted pendrive vault — so
/// agent writes land on rewritable host disk, not the read-mostly package.
/// P7 (2026-10-01, user directive: the agent should be able to build and
/// store folders on the Desktop): the user can grant a different root in
/// Settings. The grant is stored HOST-LOCAL — `%LOCALAPPDATA%\UnoOne\agent-
/// workspace.json`, the same base as the model cache — and deliberately
/// NEVER in the vault: an absolute host folder picked on this computer must
/// not follow the drive onto another machine, where the same path could
/// point anywhere. A stale grant (the folder no longer exists) is ignored,
/// not honored — the agent falls back to the default root rather than
/// writing into a re-created folder it was never granted.
fn workspace_root() -> Result<PathBuf, String> {
    if let Some(granted) = granted_workspace_root() {
        return Ok(granted);
    }
    default_workspace_root()
}

/// The persisted user grant, honored only while the folder still exists.
fn granted_workspace_root() -> Option<PathBuf> {
    let grant = read_grants_file().workspace?;
    let path = PathBuf::from(grant.root.trim());
    path.is_dir().then_some(path)
}

/// Where the host-local grant file lives.
fn workspace_config_path() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            "cannot locate the host config directory for the agent workspace grant".to_owned()
        })?;
    Ok(base.join("UnoOne").join("agent-workspace.json"))
}

/// The persisted shape of one user grant.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct WorkspaceGrant {
    root: String,
    granted_at_ms: u64,
}

/// The on-disk store (v2): the workspace grant plus every additional
/// user-granted folder (P7). A v1 file — a bare `WorkspaceGrant` — parses
/// as workspace-only, so pre-folder-grant installs migrate on first write.
#[derive(serde::Serialize, serde::Deserialize)]
struct WorkspaceGrantsFile {
    version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace: Option<WorkspaceGrant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    folders: Vec<WorkspaceGrant>,
}

/// Reads the host-local grant store. An absent or unreadable file is simply
/// no grant at all — the agent falls back to the default workspace root.
fn read_grants_file() -> WorkspaceGrantsFile {
    let empty = WorkspaceGrantsFile {
        version: 2,
        workspace: None,
        folders: Vec::new(),
    };
    let Ok(config) = workspace_config_path() else {
        return empty;
    };
    let Ok(text) = std::fs::read_to_string(config) else {
        return empty;
    };
    // v1 first: the bare grant shape puts `root` at top level and has no
    // `version`, so the two shapes never parse as each other.
    if let Ok(v1) = serde_json::from_str::<WorkspaceGrant>(&text) {
        return WorkspaceGrantsFile {
            version: 2,
            workspace: Some(v1),
            folders: Vec::new(),
        };
    }
    serde_json::from_str::<WorkspaceGrantsFile>(&text).unwrap_or(empty)
}

/// Persists the grant store; the canonical paths are stored verbatim and
/// are never re-canonicalized on read.
fn write_grants_file(file: &WorkspaceGrantsFile) -> Result<(), String> {
    let config = workspace_config_path()?;
    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!("cannot create the config directory for the agent grants: {error}")
        })?;
    }
    let json = serde_json::to_string_pretty(file)
        .map_err(|error| format!("cannot encode the agent grants: {error}"))?;
    std::fs::write(&config, json)
        .map_err(|error| format!("cannot persist the agent grants: {error}"))
}

/// The additional user-granted folders (beyond the workspace root),
/// honored only while each still exists — the same stale-grant rule as the
/// workspace — and never twice: deduped against each other and against the
/// effective workspace root, case-insensitively.
fn granted_folder_roots() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    if let Ok(workspace) = workspace_root() {
        seen.push(workspace.to_string_lossy().to_lowercase());
    }
    for grant in read_grants_file().folders {
        let path = PathBuf::from(grant.root.trim());
        let key = path.to_string_lossy().to_lowercase();
        if !path.is_dir() || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        folders.push(path);
    }
    folders
}

/// The multi-root fence for one agent run: the workspace root plus every
/// additional user-granted folder, one `RootedFs` per folder with the
/// full-access byte limits (2 MiB read / 4 MiB write) applied uniformly —
/// the main lane and every sub-agent child build from this same set.
fn granted_folders() -> Result<GrantedFolders, String> {
    const MAX_READ_BYTES: usize = 2 * 1024 * 1024;
    const MAX_WRITE_BYTES: usize = 4 * 1024 * 1024;
    let workspace = workspace_root()?;
    let mut roots = vec![RootedFs::new(&workspace)
        .map_err(|error| error.to_string())?
        .with_limits(MAX_READ_BYTES, MAX_WRITE_BYTES)];
    for folder in granted_folder_roots() {
        match RootedFs::new(&folder) {
            Ok(fenced) => roots.push(fenced.with_limits(MAX_READ_BYTES, MAX_WRITE_BYTES)),
            Err(error) => {
                // A folder that vanished between the stale-check and the
                // canonicalize is skipped honestly, never fatal to the run.
                eprintln!("granted folder skipped: {error}");
            }
        }
    }
    GrantedFolders::new(roots).map_err(|error| error.to_string())
}

/// The un-granted default: `%USERPROFILE%\UnoOneAgent`, created on demand.
fn default_workspace_root() -> Result<PathBuf, String> {
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

/// What the Settings UI shows: the effective root, the persisted grant (if
/// any), the default the grant replaced, and every additional granted
/// folder.
#[derive(Debug, serde::Serialize)]
pub struct AgentWorkspaceInfo {
    pub effective_root: String,
    pub user_granted: Option<String>,
    pub default_root: String,
    pub folders: Vec<GrantedFolderInfo>,
}

/// One additional user-granted folder as the Settings UI shows it.
#[derive(Debug, serde::Serialize)]
pub struct GrantedFolderInfo {
    pub root: String,
    pub granted_at_ms: u64,
    /// False when the folder no longer exists: the grant is displayed but
    /// not honored (the same stale rule as the workspace grant).
    pub exists: bool,
}

fn agent_workspace_info() -> Result<AgentWorkspaceInfo, String> {
    let effective = workspace_root()?;
    let default = default_workspace_root()?;
    let folders = read_grants_file()
        .folders
        .into_iter()
        .map(|grant| {
            let exists = PathBuf::from(grant.root.trim()).is_dir();
            GrantedFolderInfo {
                root: grant.root,
                granted_at_ms: grant.granted_at_ms,
                exists,
            }
        })
        .collect();
    Ok(AgentWorkspaceInfo {
        effective_root: effective.to_string_lossy().into_owned(),
        user_granted: granted_workspace_root().map(|path| path.to_string_lossy().into_owned()),
        default_root: default.to_string_lossy().into_owned(),
        folders,
    })
}

/// P7 audit: every grant and revocation lands in the encrypted vault as an
/// `AuditRecord` — the user-visible scope change is as auditable as any
/// tool call. Best-effort: an audit write failure must never block the
/// grant itself, only be reported loudly.
pub(crate) fn audit_workspace_grant(vault: &Arc<Mutex<Option<Vault>>>, action: &str, root: &str) {
    let Ok(mut guard) = vault.lock() else {
        eprintln!("workspace-grant audit skipped: vault lock poisoned");
        return;
    };
    let Some(open) = guard.as_mut() else {
        eprintln!("workspace-grant audit skipped: vault locked");
        return;
    };
    use unoone_vault_core::{Record, RecordType};
    let at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let payload = serde_json::json!({
        "kind": "workspace_grant",
        "action": action,
        "root": root,
        "at_ms": at_ms,
    });
    let Ok(bytes) = serde_json::to_vec(&payload) else {
        return;
    };
    let record = Record::new(RecordType::AuditRecord, "DESKTOP", "unoone-power");
    if let Err(error) = open.write_record(record, &bytes) {
        eprintln!("workspace-grant audit write failed (non-fatal): {error}");
    }
}

/// The grant validations every user-directed folder grant shares (P7): a
/// grant must name an EXISTING folder, must not be a filesystem root, and
/// must not sit inside the encrypted Pocket AI package — agent writes must
/// never land on the read-mostly drive. Returns the canonicalized path.
fn validate_grant_path(candidate: &Path, vault_root: &str) -> Result<PathBuf, String> {
    if candidate.as_os_str().is_empty() {
        return Err("the folder path is empty".to_owned());
    }
    if !candidate.is_absolute() {
        return Err(format!(
            "the folder path must be absolute: {}",
            candidate.display()
        ));
    }
    let canonical = std::fs::canonicalize(candidate).map_err(|error| {
        format!(
            "cannot use {} as a granted folder: {error}",
            candidate.display()
        )
    })?;
    if !canonical.is_dir() {
        return Err(format!(
            "the granted folder must exist: {}",
            canonical.display()
        ));
    }
    if canonical.parent().is_none() {
        return Err(
            "a whole drive cannot be granted — pick a folder (e.g. the Desktop)".to_owned(),
        );
    }
    if !vault_root.is_empty() {
        let vault_path = PathBuf::from(vault_root);
        let inside_vault = canonical.starts_with(&vault_path)
            || std::fs::canonicalize(&vault_path)
                .map(|resolved| canonical.starts_with(resolved))
                .unwrap_or(false);
        if inside_vault {
            return Err(
                "that folder sits inside the encrypted Pocket AI package — grant a folder on the host disk (e.g. the Desktop)"
                    .to_owned(),
            );
        }
    }
    Ok(canonical)
}

/// Read the effective agent workspace for the Settings UI.
#[tauri::command]
pub async fn get_agent_workspace_info() -> Result<AgentWorkspaceInfo, String> {
    agent_workspace_info()
}

/// Grant or revoke the agent workspace root (P7, user-directed). `None`
/// revokes the workspace grant and returns to the default root — any
/// additional granted folders survive. A grant must name an EXISTING
/// directory (the picker never creates folders), must not be a filesystem
/// root, and must not sit inside the encrypted Pocket AI package. The
/// canonical path is persisted host-locally and audited in the vault.
#[tauri::command]
pub async fn set_agent_workspace_root(
    root: Option<String>,
    vault_state: tauri::State<'_, DesktopVaultState>,
) -> Result<AgentWorkspaceInfo, String> {
    let vault_root = vault_state
        .vault_root
        .lock()
        .map_err(|_| "vault-root state lock failed".to_owned())?
        .clone();
    let mut file = read_grants_file();
    let Some(raw) = root else {
        let was_granted = file.workspace.take().is_some();
        if was_granted {
            if file.folders.is_empty() {
                // Nothing left to remember — the store goes away entirely,
                // matching the pre-folder-grant observable behavior.
                let config = workspace_config_path()?;
                std::fs::remove_file(&config).ok();
            } else {
                write_grants_file(&file)?;
            }
            audit_workspace_grant(&vault_state.vault, "revoke", "");
        }
        return agent_workspace_info();
    };
    let canonical = validate_grant_path(Path::new(raw.trim()), &vault_root)?;
    let granted_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    file.workspace = Some(WorkspaceGrant {
        root: canonical.to_string_lossy().into_owned(),
        granted_at_ms,
    });
    write_grants_file(&file)?;
    audit_workspace_grant(&vault_state.vault, "grant", &canonical.to_string_lossy());
    agent_workspace_info()
}

/// Grant one additional folder to the agent (P7, user-directed): every
/// fs tool call routed inside that folder runs with the full `RootedFs`
/// fence — grants widen the file surface, never the program allowlist.
/// Same validations as the workspace grant; the canonical path is persisted
/// host-locally and audited in the vault. Duplicate grants (including the
/// workspace root itself) are refused.
#[tauri::command]
pub async fn add_agent_folder(
    path: String,
    vault_state: tauri::State<'_, DesktopVaultState>,
) -> Result<AgentWorkspaceInfo, String> {
    let vault_root = vault_state
        .vault_root
        .lock()
        .map_err(|_| "vault-root state lock failed".to_owned())?
        .clone();
    let canonical = validate_grant_path(Path::new(path.trim()), &vault_root)?;
    execute_folder_grant(canonical, &vault_state.vault)?;
    agent_workspace_info()
}

/// Revoke one additional folder grant. Revoking the workspace root itself
/// stays on `set_agent_workspace_root(None)`; this removes only from the
/// folders list. A path not in the list is an error, never a silent no-op.
#[tauri::command]
pub async fn remove_agent_folder(
    path: String,
    vault_state: tauri::State<'_, DesktopVaultState>,
) -> Result<AgentWorkspaceInfo, String> {
    let key = PathBuf::from(path.trim()).to_string_lossy().to_lowercase();
    let mut file = read_grants_file();
    let before = file.folders.len();
    file.folders
        .retain(|grant| grant.root.trim().to_lowercase() != key);
    if file.folders.len() == before {
        return Err("that folder is not in the granted list".to_owned());
    }
    write_grants_file(&file)?;
    audit_workspace_grant(&vault_state.vault, "folder_revoke", path.trim());
    agent_workspace_info()
}

// ---------------------------------------------------------------------------
// In-chat folder-grant approval (2026-10-03). Live-caught in the drive app:
// the user asked the agent to review `C:\Users\reetu\Desktop\Stanford`, then
// said "i give you permission" IN THE CHAT — and the agent still could not
// act, because grants only existed as a Settings flow and the per-run fence
// was frozen at run start. This block closes both gaps:
//
// - a routed path outside every grant asks the human with an approval CARD
//   in the app UI (`unoone:folder-grant-request`), bounded and deny-by-
//   default — Grant runs the SAME validations, store write and vault audit
//   as the Settings lane, then the tool call completes on the widened
//   fence;
// - the re-check inside the request consults the CURRENT persisted grants
//   on every denial, so grants made mid-run (Settings, an earlier tool
//   call, a sibling agent) resolve without re-asking — the fence is no
//   longer effectively frozen for the run.
// ---------------------------------------------------------------------------

/// How long an approval card waits for the human. No answer by then is a
/// denial — deny by default, never block a run forever.
const GRANT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The answer channel of one approval card: `None` while waiting,
/// `Some(approved)` once the human answers; the condvar wakes the hook.
type GrantDecisionChannel = Arc<(Mutex<Option<bool>>, std::sync::Condvar)>;

/// One live approval card: the answer channel the hook waits on.
struct PendingGrantRequest {
    /// The path the agent tried to reach.
    path: String,
    /// The folder the human is asked to grant (see `propose_grant_folder`).
    proposed_folder: String,
    /// `None` while waiting; `Some(approved)` once the human answers.
    decision: GrantDecisionChannel,
    created_at_ms: u64,
}

/// The managed state for in-chat folder grants: the live requests the
/// frontend renders as approval cards, plus the folders the human already
/// DECLINED this app session — a declined folder never re-asks (the model
/// is told why; the Settings lane still grants it any time).
#[derive(Default)]
pub struct PendingGrantRequests {
    next_id: std::sync::atomic::AtomicU64,
    pending: Mutex<Vec<(u64, PendingGrantRequest)>>,
    declined: Mutex<Vec<String>>,
}

impl PendingGrantRequests {
    pub(crate) fn deny_all(&self) {
        for (_, request) in self.lock_pending().drain(..) {
            *request.decision.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(false);
            request.decision.1.notify_all();
        }
    }

    /// Registers a new card and returns its id + the channel to wait on.
    fn insert(&self, path: String, proposed_folder: String) -> (u64, GrantDecisionChannel) {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let decision = Arc::new((Mutex::new(None), std::sync::Condvar::new()));
        let created_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut pending = self.lock_pending();
        pending.push((
            id,
            PendingGrantRequest {
                path,
                proposed_folder,
                decision: Arc::clone(&decision),
                created_at_ms,
            },
        ));
        (id, decision)
    }

    /// Records the human's answer and wakes the waiting hook. False when no
    /// live request has that id (already answered or timed out).
    fn answer(&self, id: u64, approved: bool) -> bool {
        let mut pending = self.lock_pending();
        let Some(position) = pending.iter().position(|(known, _)| *known == id) else {
            return false;
        };
        let (_, request) = pending.remove(position);
        if let Ok(mut slot) = request.decision.0.lock() {
            *slot = Some(approved);
        }
        request.decision.1.notify_all();
        true
    }

    /// Drops a request whose wait ended (timeout) so stale cards cannot be
    /// answered later.
    fn expire(&self, id: u64) {
        self.lock_pending().retain(|(known, _)| *known != id);
    }

    /// The live cards, for the frontend's query command.
    fn snapshot(&self) -> Vec<PendingGrantInfo> {
        self.lock_pending()
            .iter()
            .map(|(id, request)| PendingGrantInfo {
                request_id: *id,
                path: request.path.clone(),
                proposed_folder: request.proposed_folder.clone(),
                created_at_ms: request.created_at_ms,
            })
            .collect()
    }

    fn lock_pending(&self) -> std::sync::MutexGuard<'_, Vec<(u64, PendingGrantRequest)>> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn is_declined(&self, folder_key: &str) -> bool {
        self.declined
            .lock()
            .map(|list| list.iter().any(|known| known == folder_key))
            .unwrap_or(false)
    }

    fn mark_declined(&self, folder_key: String) {
        let Ok(mut list) = self.declined.lock() else {
            return;
        };
        if !list.contains(&folder_key) {
            list.push(folder_key);
        }
    }

    fn clear_declined(&self, folder_key: &str) {
        if let Ok(mut list) = self.declined.lock() {
            list.retain(|known| known != folder_key);
        }
    }
}

/// One approval card as the frontend renders it.
#[derive(Debug, serde::Serialize, Clone)]
pub struct PendingGrantInfo {
    pub request_id: u64,
    pub path: String,
    pub proposed_folder: String,
    pub created_at_ms: u64,
}

/// The live approval cards (the frontend re-syncs on mount).
#[tauri::command]
pub fn agent_pending_folder_grants(
    state: tauri::State<'_, PendingGrantRequests>,
) -> Vec<PendingGrantInfo> {
    state.snapshot()
}

/// The human's answer to an approval card. This records ONLY the decision;
/// the grant itself (validations, store write, vault audit, fence widen)
/// runs inside `request_folder_grant` on the tool thread, exactly like the
/// Settings lane — the UI can never write a grant directly.
#[tauri::command]
pub fn agent_respond_folder_grant(
    request_id: u64,
    approved: bool,
    state: tauri::State<'_, PendingGrantRequests>,
) -> Result<(), String> {
    if state.answer(request_id, approved) {
        Ok(())
    } else {
        Err(format!(
            "no pending folder-grant request with id {request_id} (already answered or timed out)"
        ))
    }
}

/// The folder an approval card proposes: the requested path itself when it
/// names an existing directory, else the nearest EXISTING ancestor
/// directory — the human grants FOLDERS, not files. Never a drive root
/// (`validate_grant_path` refuses those anyway). None when no grantable
/// folder exists on the path's ancestor chain.
fn propose_grant_folder(path: &Path) -> Option<PathBuf> {
    let mut candidate = path.to_path_buf();
    loop {
        if candidate.is_dir() && candidate.parent().is_some() {
            return Some(candidate);
        }
        candidate = candidate.parent()?.to_path_buf();
    }
}

/// The grant core the Settings lane and the in-chat approval share: refuse
/// duplicates against the store, persist, audit. Returns the canonical path.
fn execute_folder_grant(
    canonical: PathBuf,
    vault: &Arc<Mutex<Option<Vault>>>,
) -> Result<PathBuf, String> {
    let key = canonical.to_string_lossy().to_lowercase();
    let mut file = read_grants_file();
    if file
        .workspace
        .as_ref()
        .is_some_and(|grant| grant.root.trim().to_lowercase() == key)
    {
        return Err("that folder is already granted as the workspace root".to_owned());
    }
    if file
        .folders
        .iter()
        .any(|grant| grant.root.trim().to_lowercase() == key)
    {
        return Err("that folder is already granted".to_owned());
    }
    let granted_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    file.folders.push(WorkspaceGrant {
        root: canonical.to_string_lossy().into_owned(),
        granted_at_ms,
    });
    write_grants_file(&file)?;
    audit_workspace_grant(vault, "folder_grant", &canonical.to_string_lossy());
    Ok(canonical)
}

/// The in-chat approval hook (see `granted_folders_with_approval`): runs on
/// harness tool threads, blocks up to [`GRANT_REQUEST_TIMEOUT`], and never
/// grants anything without the human clicking Grant on the card.
fn request_folder_grant(app: &tauri::AppHandle, path: &Path) -> DeniedPathResolution {
    // 1. Live truth first: a grant may have landed while this run's frozen
    //    fence was being built (Settings, an earlier tool call, a sibling
    //    agent) — consult the persisted store, never re-ask for a grant
    //    that already exists.
    if let Ok(fresh) = granted_folders() {
        if let Some((fenced, remainder)) = fresh.try_route_absolute(path) {
            return DeniedPathResolution::Granted(fenced, remainder);
        }
    }
    // 2. Propose the folder to grant. Pre-validate BEFORE showing the card:
    //    the card must never offer a grant the validations would refuse
    //    (inside the encrypted package, a drive root, a missing folder).
    let Some(proposed) = propose_grant_folder(path) else {
        return DeniedPathResolution::Denied(
            "no existing folder could be proposed for a grant — the user can grant one via Settings"
                .to_owned(),
        );
    };
    let vault_state = app.state::<crate::DesktopVaultState>();
    let vault_root = match vault_state
        .vault_root
        .lock()
        .map_err(|_| "vault-root state lock failed".to_owned())
        .map(|guard| guard.clone())
    {
        Ok(root) => root,
        Err(reason) => return DeniedPathResolution::Denied(reason),
    };
    if let Err(reason) = validate_grant_path(&proposed, &vault_root) {
        return DeniedPathResolution::Denied(format!("that folder cannot be granted: {reason}"));
    }
    let proposed_key = proposed.to_string_lossy().to_lowercase();
    let requests = app.state::<PendingGrantRequests>();
    if requests.is_declined(&proposed_key) {
        return DeniedPathResolution::Denied(format!(
            "the user already declined to grant '{}' in this session — the Settings lane can still grant it",
            proposed.display()
        ));
    }
    // Serialize card admission with vault locking: the lock sweep must see
    // every pending card, including one racing the end of a tool call.
    let (request_id, decision) = {
        let guard = vault_state.vault.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().is_none_or(|v| !v.is_unlocked()) {
            return DeniedPathResolution::Denied("Pocket AI was locked".to_owned());
        }
        requests.insert(path.display().to_string(), proposed.display().to_string())
    };
    let shown = app.emit(
        "unoone:folder-grant-request",
        serde_json::json!({
            "request_id": request_id,
            "path": path.display().to_string(),
            "proposed_folder": proposed.display().to_string(),
        }),
    );
    if shown.is_err() {
        requests.expire(request_id);
        return DeniedPathResolution::Denied(
            "the approval card could not be shown to the user".to_owned(),
        );
    }
    let (lock, cvar) = &*decision;
    let mut slot = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let deadline = std::time::Instant::now() + GRANT_REQUEST_TIMEOUT;
    while slot.is_none() {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        let waited = cvar
            .wait_timeout(slot, deadline - now)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slot = waited.0;
    }
    let answer = slot.take();
    requests.expire(request_id);
    let _ = app.emit(
        "unoone:folder-grant-resolved",
        serde_json::json!({
            "request_id": request_id,
            "outcome": match answer {
                Some(true) => "granted",
                Some(false) => "declined",
                None => "timeout",
            },
        }),
    );
    match answer {
        Some(true) => {
            // 4. Grant — the same validations, store write and vault audit
            //    as the Settings lane — then route on the widened fence.
            match execute_folder_grant(proposed.clone(), &vault_state.vault) {
                Ok(_) => {
                    requests.clear_declined(&proposed_key);
                    match granted_folders().and_then(|fresh| {
                        fresh.try_route_absolute(path).ok_or_else(|| {
                            "the grant was recorded but the path still did not route".to_owned()
                        })
                    }) {
                        Ok((fenced, remainder)) => DeniedPathResolution::Granted(fenced, remainder),
                        Err(reason) => DeniedPathResolution::Denied(reason),
                    }
                }
                Err(reason) => DeniedPathResolution::Denied(format!(
                    "the user granted the folder but the grant failed: {reason}"
                )),
            }
        }
        Some(false) => {
            requests.mark_declined(proposed_key);
            DeniedPathResolution::Denied(format!(
                "the user declined to grant '{}'",
                proposed.display()
            ))
        }
        None => DeniedPathResolution::Denied(
            "the grant request timed out without an answer — deny by default".to_owned(),
        ),
    }
}

/// The per-run fence with the in-chat approval hook installed: a path
/// outside every grant asks the human instead of failing outright. The
/// hookless form (`granted_folders`) stays the test/sandbox baseline.
pub(crate) fn granted_folders_with_approval(
    app: Option<&tauri::AppHandle>,
) -> Result<GrantedFolders, String> {
    let mut folders = granted_folders()?;
    if let Some(app) = app {
        let hook_app = app.clone();
        folders = folders.with_denied_request(std::sync::Arc::new(move |path| {
            request_folder_grant(&hook_app, path)
        }));
    }
    Ok(folders)
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
pub(crate) struct DesktopSearchTool {
    manifest: ToolManifest,
    folders: GrantedFolders,
}

impl DesktopSearchTool {
    pub(crate) fn new(folders: GrantedFolders) -> Self {
        Self {
            manifest: ToolManifest {
                id: "workspace.search".to_owned(),
                version: "1.0.0".to_owned(),
                description: "Recursively search file contents in the agent workspace (and any additional user-granted folders, addressed by absolute path) for a literal substring; returns path:line: text matches.".to_owned(),
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
            folders,
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
            let entries = match self.folders.list(&dir) {
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
                let resolved = match self.folders.resolve_existing(&relative) {
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
                let Ok(text) = self.folders.read_text(&relative) else {
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
pub(crate) struct DesktopPatchTool {
    manifest: ToolManifest,
    folders: GrantedFolders,
}

impl DesktopPatchTool {
    pub(crate) fn new(folders: GrantedFolders) -> Self {
        Self {
            manifest: ToolManifest {
                id: "workspace.patch".to_owned(),
                version: "1.0.0".to_owned(),
                description: "Replace an exact literal substring inside one workspace file (or a file in any granted folder, by absolute path). By default the match must be unique; pass replace_all=true to replace every occurrence.".to_owned(),
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
            folders,
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

        let text = self.folders.read_text(path)?;
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
        self.folders.write_text_atomic(path, &patched)?;
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

/// The document-creation tool (doc.create): the user's "agent creates
/// downloadable PDF/DOCX" capability, pure Rust through the same fence.
/// PDF and DOCX render through `doc_writer` (lopdf + zip, zero new
/// dependencies) and write as BINARY through the fence; MD and TXT are
/// plain text. Every output is round-trip tested against the real document
/// readers in `documents.rs` — a writer bug cannot pass the reader gate.
pub(crate) struct DesktopDocCreateTool {
    manifest: ToolManifest,
    folders: GrantedFolders,
}

impl DesktopDocCreateTool {
    pub(crate) fn new(folders: GrantedFolders) -> Self {
        Self {
            manifest: ToolManifest {
                id: "doc.create".to_owned(),
                version: "1.0.0".to_owned(),
                description: "Create a document file (PDF, DOCX, MD or TXT) from plain text. Renders real PDF/DOCX binaries — round-trip readable by this same product's document reader — or writes plain text for MD/TXT. Give `filename` relative to the workspace folder or as an absolute path inside any granted folder; the format comes from the extension unless `format` says otherwise. `content` is the full document text, one line per paragraph; `title` (PDF only) heads the first page. PDF rendering is text-only with Latin fonts; non-Latin text extracts correctly but will not display as glyphs.".to_owned(),
                input_schema: r#"{"type":"object","properties":{"filename":{"type":"string"},"format":{"type":"string","enum":["pdf","docx","md","txt"]},"title":{"type":"string"},"content":{"type":"string"}},"required":["filename","content"],"additionalProperties":false}"#.to_owned(),
                output_schema: r#"{"type":"string"}"#.to_owned(),
                required_capabilities: CapabilitySet::from_slice(&[Capability::FileWrite]),
                supported_levels: vec![ExecutionLevel::L1, ExecutionLevel::L2, ExecutionLevel::L3],
                determinism: Determinism::NonIdempotent,
                side_effect: SideEffect::Write,
                confirmation: ConfirmationMode::OnSideEffect,
                concurrency_safe: false,
                default_timeout: Duration::from_secs(30),
                max_output_bytes: 16 * 1024,
                verification: "fenced-binary-write-v1".to_owned(),
                compensation: "delete-v1".to_owned(),
            },
            folders,
        }
    }

    /// The format for one call: explicit argument wins, else the filename
    /// extension, else plain text. Unknown extensions are refused rather
    /// than silently guessed.
    fn resolve_format(arguments: &ToolArguments, filename: &str) -> Result<DocFormat, String> {
        if let Some(explicit) = arguments.get("format").and_then(Value::as_str) {
            return match explicit {
                "pdf" => Ok(DocFormat::Pdf),
                "docx" => Ok(DocFormat::Docx),
                "md" => Ok(DocFormat::Md),
                "txt" => Ok(DocFormat::Txt),
                other => Err(format!(
                    "unsupported format {other:?} — use pdf, docx, md or txt"
                )),
            };
        }
        match filename.rsplit('.').next() {
            Some("pdf") => Ok(DocFormat::Pdf),
            Some("docx") => Ok(DocFormat::Docx),
            Some("md") => Ok(DocFormat::Md),
            Some("txt") | Some("") | None => Ok(DocFormat::Txt),
            Some(other) => Err(format!(
                "unknown extension .{other} — pass format explicitly (pdf, docx, md or txt)"
            )),
        }
    }
}

/// The concrete document kinds doc.create writes — an enum (not bare
/// strings) so the execute match is exhaustively checked at compile time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DocFormat {
    Pdf,
    Docx,
    Md,
    Txt,
}

impl Tool for DesktopDocCreateTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        let allowed = ["filename", "format", "title", "content"];
        if arguments.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(inbharat_harness_core::Failure::invalid(
                "pai.tool.arguments",
                "doc.create call contains an unsupported argument",
            ));
        }
        let filename = required_string(arguments, "filename")?;
        if Self::resolve_format(arguments, filename).is_err() {
            return Err(inbharat_harness_core::Failure::invalid(
                "doc.create.format",
                "format must be pdf, docx, md or txt (the filename extension is used when omitted)",
            ));
        }
        let content = arguments
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                inbharat_harness_core::Failure::invalid(
                    "doc.create.content",
                    "content must be the full document text",
                )
            })?;
        if content.len() > 256 * 1024 {
            return Err(inbharat_harness_core::Failure::invalid(
                "doc.create.content",
                "content is limited to 256 KiB per document",
            ));
        }
        Ok(())
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        context.cancel.check("pai.doc_create")?;
        let filename = required_string(arguments, "filename")?;
        let content = required_string(arguments, "content")?;
        let format = Self::resolve_format(arguments, filename).map_err(|message| {
            inbharat_harness_core::Failure::invalid("doc.create.format", message)
        })?;
        let title = arguments
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("UnoOne Document");

        let lines: Vec<String> = content
            .lines()
            .map(|line| line.to_owned())
            .collect::<Vec<_>>();
        let summary = match format {
            DocFormat::Pdf => {
                let bytes =
                    crate::doc_writer::render_pdf_bytes(title, &lines).map_err(|message| {
                        inbharat_harness_core::Failure::invalid("doc.create.render", message)
                    })?;
                self.folders.write_bytes_atomic(filename, &bytes)?;
                format!(
                    "created {filename} — a real PDF ({} bytes, {} line(s), title {title:?})",
                    bytes.len(),
                    lines.len()
                )
            }
            DocFormat::Docx => {
                let bytes = crate::doc_writer::render_docx_bytes(&lines).map_err(|message| {
                    inbharat_harness_core::Failure::invalid("doc.create.render", message)
                })?;
                self.folders.write_bytes_atomic(filename, &bytes)?;
                format!(
                    "created {filename} — a real DOCX ({} bytes, {} paragraph(s))",
                    bytes.len(),
                    lines.len()
                )
            }
            DocFormat::Md | DocFormat::Txt => {
                self.folders.write_text_atomic(filename, content)?;
                format!("created {filename} — plain text ({} bytes)", content.len())
            }
        };
        Ok(ToolOutput {
            value: Value::String(summary.clone()),
            model_content: summary,
            presentation: BTreeMap::from([("kind".to_owned(), "doc-create".to_owned())]),
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
                description: "Drive the desktop browser workspace: navigate, click, type, fill forms, scroll, extract page text, get page info or screenshot. The browser window opens itself on the first action if none is open — you can show the user any page (e.g. the app you just deployed) without asking them to open anything first.".to_owned(),
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

/// The live website preview tool (web.preview): the agent writes a site with
/// fs.write and the user watches it render in its own window — NO web
/// server anywhere. `preview::start_preview` mirrors the entry's folder
/// (bounded) into the asset-protocol-scoped `$TEMP` dir, the frontend
/// creates the window (Rust cannot — defects #40/#41), and the frontend's
/// `preview_poll` heartbeat re-stages + reloads as the agent keeps editing.
/// The preview window carries NO Tauri capabilities, so scripts inside the
/// previewed page can never touch the IPC surface.
struct DesktopPreviewTool {
    manifest: ToolManifest,
    folders: GrantedFolders,
    app: tauri::AppHandle,
}

impl DesktopPreviewTool {
    fn new(folders: GrantedFolders, app: tauri::AppHandle) -> Self {
        Self {
            manifest: ToolManifest {
                id: "web.preview".to_owned(),
                version: "1.0.0".to_owned(),
                description: "Open a live preview window of an HTML page you wrote (a website, a report, an app UI). Give `path` of the entry .html file, relative to the workspace or an absolute path inside a granted folder; the page and every file next to it (css, js, images) shows in its own window and RELOADS ITSELF as you keep editing the site — the user watches it build. No web server is started. Call it again with another .html to point the preview at a different page.".to_owned(),
                input_schema: r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}"#.to_owned(),
                output_schema: r#"{"type":"string"}"#.to_owned(),
                required_capabilities: CapabilitySet::from_slice(&[Capability::Workspace]),
                supported_levels: vec![ExecutionLevel::L1, ExecutionLevel::L2, ExecutionLevel::L3],
                determinism: Determinism::NonIdempotent,
                side_effect: SideEffect::Process,
                confirmation: ConfirmationMode::OnSideEffect,
                concurrency_safe: false,
                default_timeout: Duration::from_secs(30),
                max_output_bytes: 4 * 1024,
                verification: "preview-mirror-v1".to_owned(),
                compensation: "close-window-v1".to_owned(),
            },
            folders,
            app,
        }
    }
}

impl Tool for DesktopPreviewTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        let allowed = ["path"];
        if arguments.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(inbharat_harness_core::Failure::invalid(
                "pai.tool.arguments",
                "web.preview call contains an unsupported argument",
            ));
        }
        required_string(arguments, "path")?;
        Ok(())
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        context.cancel.check("pai.web_preview")?;
        let path = required_string(arguments, "path")?;
        // The fence resolves the entry to its canonical absolute path; a
        // path outside every granted folder is refused here.
        let entry = self.folders.resolve_existing(path)?;
        let state = self.app.state::<crate::preview::PreviewState>();
        let info = crate::preview::start_preview(&state, &entry).map_err(|message| {
            inbharat_harness_core::Failure::invalid("web.preview.stage", message)
        })?;
        self.app
            .emit(
                "unoone:ensure-preview-window",
                crate::preview::EnsurePreviewPayload {
                    path: info.mirror_entry.to_string_lossy().to_string(),
                },
            )
            .map_err(|error| {
                inbharat_harness_core::Failure::new(
                    ErrorCode::Internal,
                    FailureClass::Internal,
                    "web.preview.window",
                    format!("could not request the preview window: {error}"),
                )
            })?;
        let summary = format!(
            "opened a live preview of {path} — {} file(s), {} bytes mirrored; the \
             preview window reloads itself as you edit the site's files, and the user \
             can watch it build",
            info.file_count, info.total_bytes
        );
        Ok(ToolOutput {
            value: Value::String(summary.clone()),
            model_content: summary,
            presentation: BTreeMap::from([("kind".to_owned(), "web-preview".to_owned())]),
        })
    }
}

/// The full-access tool set: the harness built-ins (fenced fs.read/fs.list/
/// fs.write/fs.mkdir/fs.copy + allowlisted direct-argv process.run), plus the
/// desktop search, patch, document, preview and browser adapters. Every tool
/// stays behind the harness pipeline. The fs tools are fenced to the
/// granted-folder set — the workspace root plus every additional
/// user-granted folder. fs.copy is the binary-safe lane: images and other
/// binary files arrive byte-exact instead of being corrupted by the UTF-8
/// read+write round-trip.
fn desktop_workspace_tools(
    folders: GrantedFolders,
    app: tauri::AppHandle,
    browser: Arc<BrowserStateHolder>,
    safety: Arc<Mutex<DesktopSafetyGuard>>,
) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadFileTool::default()) as Arc<dyn Tool>,
        Arc::new(ListFilesTool::default()),
        Arc::new(WriteFileTool::default()),
        Arc::new(MakeDirTool::default()),
        Arc::new(CopyFileTool::default()),
        Arc::new(RunProcessTool::default()),
        Arc::new(DesktopSearchTool::new(folders.clone())),
        Arc::new(DesktopPatchTool::new(folders.clone())),
        Arc::new(DesktopDocCreateTool::new(folders.clone())),
        Arc::new(DesktopPreviewTool::new(folders.clone(), app.clone())),
        Arc::new(DesktopBrowserTool::new(app, browser, safety)),
    ]
}

// ---------------------------------------------------------------------------
// Multi-agent lane (2026-09-15). The harness core ships a designed,
// unit-tested sub-agent seam (`SubagentProvider` + `run_scoped_subagent` +
// `BudgetLimits.max_subagent_depth` + `Capability::Subagent`) with no
// consumer — the desktop lane ran at depth 0 with no provider. This block
// wires that seam: `agent.spawn` lets the parent agent delegate a
// self-contained sub-task to a fresh nested harness run with the same
// verified model, the same fenced workspace and the same audited tools.
// The user's standing ask: multi-agent task completion "just like Codex
// or GLM" — isolated context per sub-task, up to 2 levels deep, every
// child step still audited and budgeted.
// ---------------------------------------------------------------------------

/// Sub-agent spawn ceiling shared by every level: a top-level agent may
/// spawn children at depth 1, those may spawn at depth 2, and depth 2
/// cannot spawn further. Matches `full_access_budget().max_subagent_depth`
/// so the request the tool builds always passes `run_scoped_subagent`'s
/// validation at the top level and is rejected structurally at the fringe.
const SUBAGENT_DEPTH_CEILING: u8 = 2;

/// The current depth of an agent run, parsed from its actor id. Top-level
/// runs use "local-user" (depth 0); every child runs as
/// "subagent-<short>-d<depth>". The spawn tool uses this to compute the
/// child's depth — the depth chain is enforced by the actor string the
/// provider stamps on every child run, so a child cannot lie its way
/// deeper than the ceiling without faking an actor it was never given.
fn actor_depth(actor: &str) -> u8 {
    actor
        .rsplit("-d")
        .next()
        .and_then(|tail| tail.parse::<u8>().ok())
        .filter(|depth| *depth > 0 && actor.starts_with("subagent-"))
        .unwrap_or(0)
}

/// The sub-agent's own run budget: generous enough for a real build task,
/// bounded so a runaway child cannot burn unbounded host resources.
/// `max_subagent_depth` is the remaining headroom below the ceiling, so
/// a depth-2 child gets 0 and the harness itself refuses deeper spawns.
fn subagent_budget(remaining_depth: u8) -> BudgetLimits {
    BudgetLimits {
        max_steps: 2_000,
        max_tool_calls: 10_000,
        max_rounds: 1,
        max_jobs: 0,
        max_subagent_depth: remaining_depth,
        max_output_bytes: 8 * 1024 * 1024,
        max_duration: Duration::from_secs(60 * 60),
    }
}

/// The sub-agent's system briefing. A sub-agent never sees the user: its
/// whole job is to complete one task handed to it by the parent agent and
/// return a report the parent can verify and fold into its own answer.
fn subagent_system_prefix() -> String {
    let workspace = workspace_root()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| "%USERPROFILE%\\UnoOneAgent".to_owned());
    let granted_lines = granted_folder_roots()
        .iter()
        .map(|folder| format!("\n- plus the user-granted folder: {}", folder.display()))
        .collect::<Vec<_>>()
        .join("");
    format!(
        "You are a sub-agent spawned by another AI agent to complete ONE \
         task inside the user's private Pocket AI workspace. You never talk \
         to the user directly — the parent agent receives your final report \
         and verifies it.\n\
         You have the same tools as the parent, all audited and budgeted:\n\
         - Read/write/list/search/patch files in the workspace folder: {workspace}{granted_lines}\n\
         - Host commands inherit the parent session permission. Folder grants do not \
         sandbox commands; programs can access host files and the network. Never retry \
         through another tool when permission is denied. \
         Run programs directly (git, cargo, rustc, node, npm, npx, python, pip, \
         dotnet, go, java, cmake, make, gcc, clang, powershell)\n\
         - Deploy long-running processes with background:true on process.run\n\
         - Drive the real web browser via browser.act — the session is \
         PERSISTENT (the machine's WebView2 profile), so sites the user is \
         logged into stay logged in; never ask for or type credentials, and \
         report back if a needed login is missing so the PARENT tells the user\n\
         - Create real documents (PDF, DOCX, MD, TXT) via doc.create\n\
         - Open a live website preview via web.preview (the entry .html path)\n\
         - Copy the user's image files with fs.copy — it is binary-safe, while \
         fs.read + fs.write would corrupt images; author graphics yourself as \
         inline SVG\n\
         Do the whole task yourself: create the real files, run the real \
         commands, read the exact error output when something fails, fix it \
         and re-run until it genuinely works. Never claim a task is complete \
         while its own output shows a failure.\n\
         Your final message IS the report. Make it self-contained: state \
         exactly what you did, the exact ABSOLUTE path of every file you \
         created or changed, the exact output of anything you ran, and \
         anything the parent must double-check. The parent treats your \
         report as data from a worker, not as established truth — include \
         the evidence that lets it verify your claims."
    )
}

/// Runs a sub-agent to completion against the same verified llama-server,
/// the same encrypted vault and the same fenced workspace as the parent.
/// Stateless per run: every `run` builds a fresh nested harness with its
/// own budget, its own isolated conversation namespace and (at depth
/// below the ceiling) its own `agent.spawn` tool, so the child can itself
/// delegate. Every child tool is wrapped in `ProgressTool` so the user
/// watches sub-agent activity live in the same chat feed, tagged with the
/// child id.
struct PaiSubagentProvider {
    model_id: String,
    port: u16,
    vault_root: String,
    vault: Arc<Mutex<Option<Vault>>>,
    vault_id: String,
    safety: Arc<Mutex<DesktopSafetyGuard>>,
    browser: Arc<BrowserStateHolder>,
    /// `None` in unit tests: child tools then register without the live
    /// progress wrapper (there is no UI to stream to).
    app: Option<tauri::AppHandle>,
    /// The capability set every spawned child inherits (the parent's own
    /// full-access set). `run_scoped_subagent` already refuses a request
    /// whose capabilities are not a subset of the parent's.
    capabilities: CapabilitySet,
    /// The PARENT's run-trail collector: child tool activity lands in the
    /// same trail (tagged by the existing "[subagent-xxxx]" prefix) so the
    /// vault memory records what the whole run did, children included.
    trail: SharedAgentTrail,
    process_lease: crate::desktop_process::ProcessLease,
}

impl PaiSubagentProvider {
    #[allow(clippy::too_many_arguments)] // one construction site + tests
    fn new(
        model_id: String,
        port: u16,
        vault_root: String,
        vault: Arc<Mutex<Option<Vault>>>,
        vault_id: String,
        safety: Arc<Mutex<DesktopSafetyGuard>>,
        browser: Arc<BrowserStateHolder>,
        app: Option<tauri::AppHandle>,
        capabilities: CapabilitySet,
        trail: SharedAgentTrail,
    ) -> Self {
        let process_lease = app
            .as_ref()
            .map(|app| {
                app.state::<crate::desktop_process::DesktopProcessState>()
                    .lease()
            })
            .unwrap_or_else(|| crate::desktop_process::DesktopProcessState::default().lease());
        Self {
            process_lease,
            model_id,
            port,
            vault_root,
            vault,
            vault_id,
            safety,
            browser,
            app,
            capabilities,
            trail,
        }
    }

    fn with_process_lease(mut self, lease: crate::desktop_process::ProcessLease) -> Self {
        self.process_lease = lease;
        self
    }

    /// Build and run one child harness. Returns the child's final report.
    /// Child-level failures are data, not transport errors: the caller
    /// wraps them into `SubagentResult::failure` so the parent model can
    /// read and react to them.
    fn run_child(
        &self,
        request: &SubagentRequest,
        child_id: &str,
        cancel: &CancellationToken,
    ) -> HarnessResult<String> {
        let child_model_builder = PaiLlamaLocalProvider::new(self.model_id.clone(), self.port)
            .map_err(|error| {
                Failure::new(
                    ErrorCode::ProviderFailed,
                    FailureClass::Internal,
                    "subagent.model",
                    error.to_string(),
                )
            })?
            // Child agents do not render tokens in the parent chat, but they
            // still need SSE progress-aware deadlines. A no-op tap selects
            // the same resilient streaming transport as the main agent while
            // preserving the child's isolated transcript and final report.
            .with_token_emitter(Arc::new(|_| {}));
        let model = Arc::new(child_model_builder);
        let memory = Arc::new(
            PaiVaultMemoryProvider::new(
                Arc::clone(&self.vault),
                PaiVaultMemoryProviderConfig {
                    origin_platform: "DESKTOP".to_owned(),
                    origin_device_id: "unoone-power".to_owned(),
                    ..PaiVaultMemoryProviderConfig::default()
                },
            )
            .map_err(|error| {
                Failure::new(
                    ErrorCode::ProviderFailed,
                    FailureClass::Internal,
                    "subagent.memory",
                    error.to_string(),
                )
            })?
            // Gap 5 rerank on the child's own model server (same fail-open
            // contract as the main lane).
            .with_lexical_rerank(self.model_id.clone(), self.port),
        );
        let folders = granted_folders_with_approval(self.app.as_ref()).map_err(|error| {
            Failure::new(
                ErrorCode::FilesystemDenied,
                FailureClass::Policy,
                "subagent.workspace",
                error,
            )
        })?;
        let broker = GrantedFolderBroker::with_process_lease(
            folders.clone(),
            FULL_ACCESS_PROGRAMS.iter().map(|p| (*p).to_owned()),
            self.process_lease.clone(),
        );
        let child_prefix = format!("[{child_id}]");
        let mut builder = HarnessBuilder::embedded(Arc::new(broker))
            .map_err(|error| {
                Failure::new(
                    ErrorCode::Conflict,
                    FailureClass::Internal,
                    "subagent.build",
                    error.to_string(),
                )
            })?
            .permission_provider(Arc::new(FullAccessPermission));
        builder = builder.register_model(model).map_err(|error| {
            Failure::new(
                ErrorCode::Conflict,
                FailureClass::Internal,
                "subagent.build",
                error.to_string(),
            )
        })?;
        builder = builder
            .memory_provider(memory)
            .system_prefix(subagent_system_prefix())
            .sandbox_provider(Arc::new(DesktopSandbox {
                granted: request.capabilities.clone(),
                trusted_process: true,
            }))
            .confirmation_provider(Arc::new(StaticConfirmationProvider {
                outcome: ConfirmationOutcome::AllowedOnce,
            }));
        for tool in desktop_read_tools(
            &self.vault_root,
            Arc::clone(&self.vault),
            Arc::clone(&self.safety),
        ) {
            builder = builder.register_tool(self.child_tool(tool, &child_prefix))?;
        }
        for tool in desktop_workspace_tools(
            folders,
            self.app.clone().ok_or_else(|| {
                Failure::new(
                    ErrorCode::Internal,
                    FailureClass::Internal,
                    "subagent.build",
                    "sub-agent runs require a UI app handle",
                )
            })?,
            Arc::clone(&self.browser),
            Arc::clone(&self.safety),
        ) {
            builder = builder.register_tool(self.child_tool(tool, &child_prefix))?;
        }
        // The child may itself delegate while it is above the depth
        // fringe. `run_scoped_subagent` + the actor-stamped depth reject
        // anything past the ceiling, so this registration is safe at
        // every level that can reach it.
        if request.depth < SUBAGENT_DEPTH_CEILING {
            let child_provider = Arc::new(PaiSubagentProvider::new(
                self.model_id.clone(),
                self.port,
                self.vault_root.clone(),
                Arc::clone(&self.vault),
                self.vault_id.clone(),
                Arc::clone(&self.safety),
                Arc::clone(&self.browser),
                self.app.clone(),
                self.capabilities.clone(),
                Arc::clone(&self.trail),
            ));
            builder = builder.register_tool(
                self.child_tool(Arc::new(AgentSpawnTool::new(child_provider)), &child_prefix),
            )?;
        }
        let harness = builder.build();
        let actor = format!("{child_id}-d{}", request.depth);
        let conversation_namespace = format!("{}:subagent:{}", self.vault_id, child_id);
        let remaining_depth = SUBAGENT_DEPTH_CEILING.saturating_sub(request.depth);
        let options = RunOptions {
            actor,
            capabilities: request.capabilities.clone(),
            provider: "pai-llama-local".to_owned(),
            model: self.model_id.clone(),
            memory: MemoryOptions {
                // Long-term memory only — a sub-agent never reads or
                // writes the user's conversation history (isolated
                // namespace, no conversation write-back).
                scopes: vec![
                    inbharat_harness_core::MemoryScope::Preferences,
                    inbharat_harness_core::MemoryScope::Relevant,
                    inbharat_harness_core::MemoryScope::Project,
                ],
                namespace: self.vault_id.clone(),
                conversation_namespace: Some(conversation_namespace),
                search_limit: 8,
                recent_conversation_limit: 16,
                max_context_bytes: 32 * 1024,
                write_conversation: false,
            },
            explicit_level: Some(ExecutionLevel::L3),
            budget: Some(subagent_budget(remaining_depth)),
            // Sub-agents are pure act-and-report workers (they never talk
            // to the user), so the prose-dump corrective retry applies
            // fully here too.
            corrective_retries: 1,
            ..RunOptions::default()
        };
        let (outcome, _session) = harness.run(&request.prompt, &options, cancel)?;
        Ok(outcome.output)
    }

    /// Wrap one child tool with the live-progress emitter when a UI is
    /// attached; register it raw in tests. The child writes into the
    /// PARENT's run-trail collector so one trail covers the whole run.
    fn child_tool(&self, tool: Arc<dyn Tool>, child_prefix: &str) -> Arc<dyn Tool> {
        match &self.app {
            Some(app) => Arc::new(ProgressTool {
                inner: tool,
                app: app.clone(),
                detail_prefix: Some(child_prefix.to_owned()),
                trail: Some(Arc::clone(&self.trail)),
            }),
            None => tool,
        }
    }
}

impl SubagentProvider for PaiSubagentProvider {
    fn run(
        &self,
        request: &SubagentRequest,
        cancel: &CancellationToken,
    ) -> HarnessResult<SubagentResult> {
        let child_id = format!(
            "subagent-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        // Child-level failures are data, not transport errors: the
        // parent model reads the failure and decides what to do.
        let outcome = self.run_child(request, &child_id, cancel);
        match outcome {
            Ok(output) => Ok(SubagentResult {
                child_id,
                output,
                failure: None,
            }),
            Err(failure) => Ok(SubagentResult {
                child_id,
                output: String::new(),
                failure: Some(failure),
            }),
        }
    }
}

/// The parent agent's delegation tool. One argument — the complete,
/// self-contained task — because a 12B model calls a simple schema far
/// more reliably than a complex one (the defect-#38 lesson: the schema
/// IS the model-facing contract). The child inherits the parent's
/// capability set; the harness's `run_scoped_subagent` validates depth
/// and capability narrowing before any child process exists.
struct AgentSpawnTool {
    manifest: ToolManifest,
    provider: Arc<PaiSubagentProvider>,
}

impl AgentSpawnTool {
    fn new(provider: Arc<PaiSubagentProvider>) -> Self {
        Self {
            manifest: ToolManifest {
                id: "agent.spawn".to_owned(),
                version: "1.0.0".to_owned(),
                description: "Spawn a sub-agent to complete one self-contained task in your \
                    workspace and return its report. The sub-agent has your same tools \
                    (files, programs, browser) and works independently — it does not see \
                    your conversation, only the task you give it, and its final report \
                    comes back as this tool's output. Give it a COMPLETE, self-contained \
                    task description: every file path and detail it needs, because it \
                    knows nothing else. Use it to parallelize independent pieces of a \
                    large task or to get a fresh independent pass on risky work. \
                    Verify its claims before relying on them."
                    .to_owned(),
                input_schema: r#"{"type":"object","properties":{"task":{"type":"string","description":"The complete, self-contained task for the sub-agent: exactly what to build or check, with every path and detail it needs. Its final report returns as this tool's output."}},"required":["task"],"additionalProperties":false}"#
                    .to_owned(),
                output_schema: r#"{"type":"object","properties":{"child_id":{"type":"string"},"status":{"type":"string"},"output":{"type":"string"}},"required":["child_id","status","output"],"additionalProperties":false}"#
                    .to_owned(),
                required_capabilities: CapabilitySet::from_slice(&[Capability::Subagent]),
                supported_levels: vec![ExecutionLevel::L3],
                determinism: Determinism::NonIdempotent,
                side_effect: SideEffect::None,
                confirmation: ConfirmationMode::Never,
                concurrency_safe: true,
                default_timeout: Duration::from_secs(60 * 60),
                max_output_bytes: 512 * 1024,
                verification: "child-report-v1".to_owned(),
                compensation: "none".to_owned(),
            },
            provider,
        }
    }
}

impl Tool for AgentSpawnTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        if arguments.keys().any(|key| key != "task") {
            return Err(Failure::invalid(
                "agent.spawn.arguments",
                "agent.spawn accepts only the task argument",
            ));
        }
        let task = arguments
            .get("task")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Failure::invalid(
                    "agent.spawn.task",
                    "task must be a non-empty string of at most 32 KiB",
                )
            })?;
        if task.trim().is_empty() {
            return Err(Failure::invalid(
                "agent.spawn.task",
                "task must be a non-empty string",
            ));
        }
        if task.len() > 32 * 1024 {
            return Err(Failure::invalid(
                "agent.spawn.task",
                "task must be at most 32 KiB",
            ));
        }
        Ok(())
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        context.cancel.check("agent.spawn")?;
        let task = arguments
            .get("task")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty() && value.len() <= 32 * 1024)
            .ok_or_else(|| {
                Failure::invalid(
                    "agent.spawn.task",
                    "task must be a non-empty string of at most 32 KiB",
                )
            })?
            .to_owned();
        // The depth chain: this run's depth comes from its actor id, the
        // child is one level deeper, and run_scoped_subagent rejects any
        // request past the ceiling before a child harness ever exists.
        let depth = actor_depth(context.actor) + 1;
        let request = SubagentRequest {
            prompt: task,
            parent_id: context.actor.to_owned(),
            depth,
            max_depth: SUBAGENT_DEPTH_CEILING,
            capabilities: self.provider.capabilities.clone(),
            max_output_bytes: 256 * 1024,
        };
        let result = run_scoped_subagent(
            self.provider.as_ref(),
            &request,
            &self.provider.capabilities,
            context.cancel,
        )?;
        let (status, report) = match &result.failure {
            None => ("completed".to_owned(), result.output),
            Some(failure) => (
                "failed".to_owned(),
                format!("The sub-agent failed: {failure}"),
            ),
        };
        let value_json = serde_json::json!({
            "child_id": result.child_id,
            "status": status,
            "output": report,
        });
        let value = Value::parse_json(&value_json.to_string()).map_err(|message| {
            Failure::invalid(
                "agent.spawn.output",
                format!("invalid result JSON: {message}"),
            )
        })?;
        let model_content = format!(
            "Sub-agent {} (depth {depth}) {status}.\n\nReport:\n{report}",
            result.child_id
        );
        Ok(ToolOutput {
            value,
            model_content,
            presentation: BTreeMap::new(),
        })
    }
}

/// Unified text orchestration entry point. The legacy agent remains compiled only
/// as an explicit rollback path while the frontend production text path uses Harness.
/// The agent workspace's real absolute path for UI display (defect #36,
/// live-caught 2026-09-14: the chat's full-access label showed a literal
/// unexpanded "%USERPROFILE%\UnoOneAgent" and the agent's answers said only
/// "in your workspace" — the user could not find the files the tool built).
/// Live harness chat runs, keyed by conversation id, so the UI's Stop control
/// can interrupt an in-flight agent loop. One live run per conversation: a
/// newer request supersedes (and cancels) the previous one — the newest
/// request owns the conversation.
///
/// The harness loop checks its token between steps (`cancel.check` at every
/// tool stage), so a stop takes effect at the next step boundary; the loop
/// then returns with `Failure::Cancelled`, which the UI shows as an honest
/// "stopped" state rather than a fabricated result.
#[derive(Default)]
pub struct HarnessRunRegistry {
    runs: Mutex<HashMap<String, (u64, CancellationToken)>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl HarnessRunRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `token` as the live run for `conversation_id`. A previous
    /// still-registered run is cancelled with `CancelCause::Parent`
    /// (superseded); first-cause-wins means its own loop stops at the next
    /// step boundary and reports the supersede honestly.
    pub fn register(&self, conversation_id: &str, token: CancellationToken) -> u64 {
        let Ok(mut runs) = self.runs.lock() else {
            token.cancel(CancelCause::Parent);
            return 0; // fail closed if registration is unavailable
        };
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if let Some((_, previous)) = runs.insert(conversation_id.to_owned(), (id, token)) {
            previous.cancel(CancelCause::Parent);
        }
        id
    }

    /// User stop: cancels the live run for `conversation_id`. True only when
    /// a live run existed and this call was the first to cancel it.
    pub fn stop(&self, conversation_id: &str) -> bool {
        let Ok(runs) = self.runs.lock() else {
            return false;
        };
        match runs.get(conversation_id) {
            Some((_, token)) => token.cancel(CancelCause::User),
            None => false,
        }
    }

    /// Removes the finished (or crashed) run WITHOUT cancelling: a late Stop
    /// after the loop already ended must not report a phantom cancellation.
    pub fn finish(&self, conversation_id: &str, id: u64) {
        if let Ok(mut runs) = self.runs.lock() {
            if runs
                .get(conversation_id)
                .is_some_and(|(current, _)| *current == id)
            {
                runs.remove(conversation_id);
            }
        }
    }

    pub fn stop_all(&self) {
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        for (_, (_, token)) in runs.drain() {
            token.cancel(CancelCause::Parent);
        }
    }
}

#[tauri::command]
pub async fn get_workspace_root() -> Result<String, String> {
    workspace_root().map(|path| path.to_string_lossy().into_owned())
}

/// User stop for an in-flight harness chat run. Cancels the live loop for
/// `conversation_id`; the run itself returns with a cancellation failure the
/// UI renders as an honest "stopped by you" state. Idempotent: a second
/// click reports `false` (nothing left to stop).
#[tauri::command]
pub async fn harness_stop_run(
    conversation_id: String,
    run_registry: tauri::State<'_, HarnessRunRegistry>,
) -> Result<bool, String> {
    let conversation_id = conversation_id.trim();
    if conversation_id.is_empty() || conversation_id.len() > 128 {
        return Err("conversation_id must be 1-128 characters".to_owned());
    }
    Ok(run_registry.stop(conversation_id))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects the trailing state params
pub async fn harness_chat(
    message: String,
    conversation_id: Option<String>,
    conversation_history: Vec<ConversationTurn>,
    allow_workspace_goal: Option<bool>,
    images: Option<Vec<String>>,
    personal_mode: Option<bool>,
    personal_user_message: Option<String>,
    personal_task: Option<crate::personal_execution::DraftRequest>,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    browser_state: tauri::State<'_, Arc<BrowserStateHolder>>,
    model_state: tauri::State<'_, ModelManagerState>,
    vault_state: tauri::State<'_, DesktopVaultState>,
    safety_state: tauri::State<'_, SafetyGuardState>,
    run_registry: tauri::State<'_, HarnessRunRegistry>,
) -> Result<HarnessChatResult, String> {
    // Validate whitespace-only input without rewriting the current request.
    if message.trim().is_empty() || message.len() > 256 * 1024 {
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
    let personal_mode = personal_mode.unwrap_or(false);
    if personal_mode
        && personal_user_message
            .as_ref()
            .is_none_or(|s| s.trim().is_empty() || s.len() > 4096)
    {
        return Err("Personal conversation requires 1–4096 bytes of typed text; attachments are per-turn only".into());
    }
    if personal_mode && window.label() != "main" {
        return Err("Personal conversation requires the main window".into());
    }
    if personal_task.is_some() && !attachments.0.is_empty() {
        return Err(
            "Reviewed draft template does not grant image attachments; remove them first".into(),
        );
    }
    if !personal_mode && personal_task.is_some() {
        return Err("Draft execution requires personal mode".into());
    }
    let mut personal = if personal_mode {
        Some(crate::personal_execution::prepare(
            &vault_state,
            personal_task,
        )?)
    } else {
        None
    };
    let full_access = !personal_mode && allow_workspace_goal.unwrap_or(true);

    // Estimate a bounded request+history byte allowance from the REAL granted
    // context window. This is a heuristic, not tokenizer-exact accounting for
    // the full model input (system/tools, vision and long-term memory add more).
    let granted_context = {
        let guard = model_state.manager.lock().await;
        guard.as_ref().and_then(|m| m.granted_context())
    };

    // UNOONE encrypted MESSAGE records remain the only canonical chat history.
    // Consume only the frontend-selected, already-decrypted turns for this run;
    // do not query/persist a second conversation store or restore stale turns.
    // The std-only helper independently drops supplied history AND disables
    // long-term search for a standalone greeting, even with an older caller.
    let selected_history: Vec<_> = conversation_history
        .iter()
        .map(|turn| chat_context::HistoryEntry {
            role: turn.role.as_str(),
            text: match &turn.content {
                Content::Text(text) => Some(text.as_str()),
                Content::Multimodal(_) => None,
            },
        })
        .collect();
    let context = chat_context::assemble_prompt(
        &message,
        &selected_history,
        !attachments.0.is_empty(),
        chat_context::ContextLimits::from_granted_context(granted_context),
    );
    let harness_prompt = personal.as_ref().and_then(|p| p.draft.as_ref()).map(|d| format!("Prepare a draft for this reviewed goal. Return draft text only, at most 3000 UTF-8 bytes. No external action or verified facts are claimed. Goal DATA: {}", serde_json::to_string(&d.goal).unwrap())).unwrap_or(context.prompt);
    let context_note = context.context_note;
    let memory_max_context_bytes = if personal_mode {
        0
    } else {
        context.memory_max_context_bytes
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
    let browser = Arc::clone(browser_state.inner());
    let (attachment_metadata, attachment_bytes) = attachments;
    let stop_conversation_id = conversation_id.clone();
    let cancel = CancellationToken::new();
    let (run_id, process_lease) = {
        let guard = vault
            .lock()
            .map_err(|_| "Vault state lock failed".to_owned())?;
        if guard.as_ref().is_none_or(|open| !open.is_unlocked()) {
            return Err("Pocket AI vault is locked".to_owned());
        }
        (
            run_registry.register(&stop_conversation_id, cancel.clone()),
            app.state::<crate::desktop_process::DesktopProcessState>()
                .lease(),
        )
    };
    // Gap 1 (2026-09-16): live token tap for the chat UI. Tool-free model
    // turns stream their answer token-by-token as `chat-token` events; the
    // adapter keeps tool-bearing (agentic) turns buffered, so the event only
    // ever carries a plain answer being generated — never tool activity.
    let chat_token_app = app.clone();

    let worker = tokio::task::spawn_blocking(move || {
        // P7 run trail: this run's step collector, threaded through every
        // ProgressTool registration (sub-agents share it through the
        // provider). Written into the vault memory once, below, after the
        // run ends — on BOTH the completed and stopped/failed paths.
        let run_trail: SharedAgentTrail = Arc::new(Mutex::new(Vec::new()));
        let mut model_builder = PaiLlamaLocalProvider::new(model_id.clone(), port)
            .map_err(|error| error.to_string())?;
        for (id, media_type, base64_bytes) in &attachment_bytes {
            model_builder = model_builder.with_attachment(id, media_type, base64_bytes);
        }
        if !personal_mode {
            let emitter_app = chat_token_app.clone();
            let emitter_conversation = conversation_id.clone();
            model_builder = model_builder.with_token_emitter(Arc::new(move |delta| {
                let _ = emitter_app.emit(
                    "chat-token",
                    ChatTokenEvent {
                        conversation_id: emitter_conversation.clone(),
                        delta: delta.to_owned(),
                    },
                );
            }));
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
            .map_err(|error| error.to_string())?
            // Gap 5: model-backed rerank of lexical memory hits on the same
            // verified local server that is already serving this chat turn
            // (think-off, 30s-bounded, fail-open to the lexical order).
            .with_lexical_rerank(model_id.clone(), port),
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
        let capabilities = if personal_mode {
            CapabilitySet::from_slice(&[Capability::Model])
        } else if full_access {
            CapabilitySet::all_local()
        } else {
            CapabilitySet::from_slice(&[Capability::Model, Capability::FileRead])
        };
        let workspace_fs = if full_access {
            // The in-chat approval hook rides inside the set: a denied
            // folder asks the human with a chat card (bounded, deny by
            // default) and a granted folder is usable by every tool of this
            // run, not only the one that triggered the ask.
            Some(granted_folders_with_approval(Some(&app)).map_err(|error| error.to_string())?)
        } else {
            None
        };
        let mut builder = if let Some(folders) = workspace_fs.clone() {
            let broker = GrantedFolderBroker::with_process_lease(
                folders,
                FULL_ACCESS_PROGRAMS
                    .iter()
                    .map(|program| (*program).to_owned()),
                process_lease.clone(),
            );
            HarnessBuilder::embedded(Arc::new(broker))
                .map_err(|error| error.to_string())?
                .permission_provider(Arc::new(FullAccessPermission))
        } else {
            HarnessBuilder::local_embedded(&vault_root).map_err(|error| error.to_string())?
        };
        if personal_mode {
            builder = builder.permission_provider(Arc::new(
                pai_harness_adapter::personal_execution::PersonalChatPermission,
            ));
        }
        // The trait-object Arc the builder takes; a second typed Arc of the
        // same provider stays local for the post-run P7 trail write.
        let memory_provider: Arc<dyn inbharat_harness_core::providers::MemoryProvider> =
            memory.clone();
        builder = builder
            .register_model(model.clone())
            .map_err(|error| error.to_string())?
            .memory_provider(memory_provider)
            .system_prefix(
                personal
                    .as_ref()
                    .map(|p| p.system.clone())
                    .unwrap_or_else(|| desktop_system_prefix(full_access)),
            )
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
        for tool in if personal_mode {
            Vec::new()
        } else {
            desktop_read_tools(&vault_root, Arc::clone(&vault), Arc::clone(&safety))
        } {
            // Live activity: every tool is wrapped so the chat panel shows
            // what the agent is doing while it works (2026-09-14).
            builder = builder
                .register_tool(Arc::new(ProgressTool {
                    inner: tool,
                    app: app.clone(),
                    detail_prefix: None,
                    trail: Some(Arc::clone(&run_trail)),
                }))
                .map_err(|error| error.to_string())?;
        }
        let subagent_provider = workspace_fs.as_ref().map(|_folders| {
            // The multi-agent lane (2026-09-15): agent.spawn delegates a
            // self-contained sub-task to a fresh nested harness run — a
            // sub-agent with the same verified model, the same fenced
            // workspace and the same audited tools, up to 2 levels deep.
            Arc::new(
                PaiSubagentProvider::new(
                    model_id.clone(),
                    port,
                    vault_root.clone(),
                    Arc::clone(&vault),
                    vault_id.clone(),
                    Arc::clone(&safety),
                    Arc::clone(&browser),
                    Some(app.clone()),
                    capabilities.clone(),
                    Arc::clone(&run_trail),
                )
                .with_process_lease(process_lease.clone()),
            )
        });
        if let Some(folders) = workspace_fs.clone() {
            for tool in desktop_workspace_tools(
                folders,
                app.clone(),
                Arc::clone(&browser),
                Arc::clone(&safety),
            ) {
                builder = builder
                    .register_tool(Arc::new(ProgressTool {
                        inner: tool,
                        app: app.clone(),
                        detail_prefix: None,
                        trail: Some(Arc::clone(&run_trail)),
                    }))
                    .map_err(|error| error.to_string())?;
            }
            if let Some(provider) = subagent_provider.clone() {
                builder = builder
                    .register_tool(Arc::new(ProgressTool {
                        inner: Arc::new(AgentSpawnTool::new(provider)),
                        app: app.clone(),
                        detail_prefix: None,
                        trail: Some(Arc::clone(&run_trail)),
                    }))
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
                // records passed above; Harness memory here is long-term only.
                // Bare greetings must not run substring searches (e.g. `hi`
                // matching `this`): empty scopes + zero bytes short-circuit
                // retrieval in Harness before the memory provider is queried.
                scopes: if memory_max_context_bytes == 0 {
                    Vec::new()
                } else {
                    vec![
                        inbharat_harness_core::MemoryScope::Preferences,
                        inbharat_harness_core::MemoryScope::Relevant,
                        inbharat_harness_core::MemoryScope::Project,
                    ]
                },
                namespace: vault_id.clone(),
                conversation_namespace: Some(conversation_namespace.clone()),
                search_limit: 8,
                recent_conversation_limit: 16,
                max_context_bytes: memory_max_context_bytes,
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
            // Prose-dump corrective retry (2026-10-03): complex multi-step
            // tasks on the local 12B could come back as pasted code
            // instead of tool calls. ONE corrective nudge per run, then
            // the model's text stands — chat-only runs keep the default
            // (off) so a plain "show me code" answer is never second-
            // guessed.
            options.corrective_retries = 1;
        }
        if personal_mode {
            options.budget = Some(pai_harness_adapter::personal_execution::personal_budget());
            options.explicit_level = Some(ExecutionLevel::L1);
            options.actor = personal
                .as_ref()
                .expect("personal snapshot")
                .binding
                .agent_id
                .clone();
        }
        if let Some(snapshot) = &personal {
            crate::personal_execution::check(&app, snapshot)?;
        }
        cancel.check("personal.start").map_err(|e| e.to_string())?;
        if let Some(snapshot) = &mut personal {
            if let Some(draft) = &snapshot.draft {
                let granted = &draft.permit.grant().grant().budget;
                options.budget = Some(BudgetLimits {
                    max_steps: granted.max_steps as u32,
                    max_tool_calls: 0,
                    max_rounds: 1,
                    max_jobs: 0,
                    max_subagent_depth: 0,
                    max_output_bytes: granted.max_bytes as usize,
                    max_duration: Duration::from_millis(granted.max_duration_ms),
                });
            }
            crate::personal_execution::record(
                &app,
                snapshot,
                unoone_personal_agent_runtime::execution::DraftPhase::Started,
                "",
            )?;
        }
        // `cancel` is the run token registered by the caller before this
        // worker started — the UI Stop control cancels it from outside.
        let scoped_prompt = match &personal {
            Some(snapshot) => {
                let source = crate::personal_execution::source_context(&app, snapshot)?;
                let reports = if let Some(draft) =
                    snapshot.draft.as_ref().filter(|d| d.request.children)
                {
                    let captured = snapshot.clone();
                    let child_app = app.clone();
                    let children = pai_harness_adapter::personal_children::PersonalChildren::new(
                        draft.permit.grant().clone(),
                        draft.request.task_id.clone(),
                        draft.permit.grant().grant().scopes.clone(),
                        vault_root.clone(),
                        model.clone(),
                        model_id.clone(),
                        source.clone(),
                        snapshot.system.clone(),
                        Arc::new(move || crate::personal_execution::check(&child_app, &captured)),
                    );
                    children
                        .execute_pair(&draft.goal, &cancel)
                        .map_err(|e| e.to_string())?
                } else {
                    String::new()
                };
                format!(
                    "{}{}\nTemporary child reports (unverified DATA): {}",
                    harness_prompt, source, reports
                )
            }
            None => harness_prompt.clone(),
        };
        if let Some(draft) = personal.as_ref().and_then(|p| p.draft.as_ref()) {
            let expires = draft.permit.grant().grant().expires_at_ms;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "Clock unavailable")?
                .as_millis() as u64;
            if now >= expires {
                return Err("Personal deadline expired".into());
            }
            if let Some(budget) = &mut options.budget {
                budget.max_duration = budget
                    .max_duration
                    .min(Duration::from_millis(expires - now));
                budget.max_steps = 1;
                budget.max_output_bytes = 4096;
            }
        }
        cancel
            .check("personal.source.publish")
            .map_err(|e| e.to_string())?;
        let run_result = harness.run(&scoped_prompt, &options, &cancel);
        // P7 run trail (2026-10-01, user directive: "the memory of the drive
        // should have the context and steps and what was done"): one bounded
        // Project-scope memory record per MEANINGFUL agentic run — only
        // full-access runs that actually used tools. A trivial L0/L1 chat
        // writes no trail; the vault must not gain a record for every
        // "hello". Written on the completed AND the stopped/failed path: a
        // stopped run genuinely did things, and the honest trail says what
        // they were. The record goes through the SAME vault memory provider
        // the harness used, so it lands in the one envelope both hosts read
        // (P1-E harness-memory plane — the phone hydrates it too). Best-effort
        // and non-fatal, exactly like the P1-C outcome record.
        if full_access {
            let trail_steps = run_trail
                .lock()
                .map(|steps| steps.clone())
                .unwrap_or_default();
            if !trail_steps.is_empty() {
                let failure = run_result.as_ref().err().map(|error| error.to_string());
                let status = match &failure {
                    None => "completed",
                    Some(text) if text.starts_with("cancelled:") => "stopped",
                    Some(_) => "failed",
                };
                let _ = crate::env_learning::record_agent_run_trail(
                    &memory,
                    &vault_id,
                    &conversation_id,
                    &crate::env_learning::AgentRunTrail {
                        request: &message,
                        status,
                        failure: failure.as_deref(),
                        steps: run_result
                            .as_ref()
                            .ok()
                            .map(|(outcome, _)| outcome.steps)
                            // No outcome on the stopped/failed path: the
                            // honest count of tools ATTEMPTED is the number
                            // of recorded call entries in the trail itself.
                            .unwrap_or_else(|| {
                                trail_steps
                                    .iter()
                                    .filter(|step| step.phase == "call")
                                    .count() as u32
                            }),
                        tool_calls: run_result
                            .as_ref()
                            .ok()
                            .map(|(outcome, _)| outcome.tool_calls)
                            .unwrap_or(0),
                        elapsed_ms: run_result
                            .as_ref()
                            .ok()
                            .map(|(outcome, _)| {
                                u64::try_from(outcome.elapsed.as_millis()).unwrap_or(u64::MAX)
                            })
                            .unwrap_or(0),
                        model_id: &model_id,
                        trail: &trail_steps,
                        output: run_result
                            .as_ref()
                            .ok()
                            .map(|(outcome, _)| outcome.output.as_str()),
                        session_id: run_result
                            .as_ref()
                            .ok()
                            .map(|(outcome, _)| outcome.session_id.as_str()),
                    },
                );
            }
        }
        if run_result.is_err() {
            if let Some(snapshot) = &mut personal {
                let _ = crate::personal_execution::record(
                    &app,
                    snapshot,
                    unoone_personal_agent_runtime::execution::DraftPhase::Failed,
                    "",
                );
            }
        }
        let (outcome, _session) = run_result.map_err(|error| error.to_string())?;
        cancel
            .check("personal.publish")
            .map_err(|e| e.to_string())?;
        if let Some(snapshot) = &personal {
            crate::personal_execution::check(&app, snapshot)?;
        }
        if let Some(snapshot) = &mut personal {
            if let Err(error) = crate::personal_execution::record(
                &app,
                snapshot,
                unoone_personal_agent_runtime::execution::DraftPhase::Responded,
                &outcome.output,
            ) {
                let _ = crate::personal_execution::record(
                    &app,
                    snapshot,
                    unoone_personal_agent_runtime::execution::DraftPhase::Failed,
                    "",
                );
                return Err(error);
            }
        }
        // P1-C: the completed run leaves one honest ProcedureOutcome record in
        // the canonical vault — never promotable from this path (no streak,
        // no verified postconditions, no explicit approval), pure evidence for
        // the bounded environment-learning layer. Non-fatal by contract.
        let _ = crate::env_learning::record_harness_run_outcome(
            &vault,
            &crate::env_learning::HarnessRunEvidence {
                route_level: outcome.decision.level.as_str().to_owned(),
                full_access,
                steps: outcome.steps,
                tool_calls: outcome.tool_calls,
                elapsed_ms: u64::try_from(outcome.elapsed.as_millis()).unwrap_or(u64::MAX),
                model_id: model_id.clone(),
            },
        );
        if let Some(snapshot) = &personal {
            cancel
                .check("personal.persist")
                .map_err(|e| e.to_string())?;
            let user = snapshot
                .draft
                .as_ref()
                .map(|d| d.goal.as_str())
                .unwrap_or_else(|| {
                    personal_user_message
                        .as_deref()
                        .expect("validated typed text")
                });
            crate::personal_execution::save_turn(
                &app,
                snapshot,
                &conversation_id,
                user,
                &outcome.output,
            )?;
            cancel
                .check("personal.publish")
                .map_err(|e| e.to_string())?;
            crate::personal_execution::check(&app, snapshot)?;
        }
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
            context_note,
            personal_binding: personal.map(|p| p.binding),
        })
    });
    let worker_result = worker
        .await
        .map_err(|error| format!("Harness worker failed: {error}"));
    // Every exit path — worker panic, run error, cancellation, or success —
    // ends the run: clear the registry slot before returning so a late Stop
    // reports false instead of cancelling a phantom run.
    run_registry.finish(&stop_conversation_id, run_id);
    worker_result?
}

#[cfg(test)]
mod grant_approval_tests {
    use super::*;

    #[test]
    fn proposal_is_the_path_when_it_names_an_existing_folder() {
        let base = std::env::temp_dir().join(format!(
            "unoone-grant-propose-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&base).expect("test dir");
        let proposed = propose_grant_folder(&base).expect("the folder itself");
        assert_eq!(proposed, base);
    }

    #[test]
    fn proposal_for_a_missing_path_is_the_nearest_existing_ancestor() {
        let base = std::env::temp_dir().join(format!(
            "unoone-grant-ancestor-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&base).expect("test dir");
        let missing = base.join("deep").join("never").join("file.txt");
        let proposed =
            propose_grant_folder(&missing).expect("an ancestor must exist (the temp dir)");
        assert_eq!(proposed, base);
    }

    #[test]
    fn proposal_never_offers_a_drive_root() {
        // No ancestor of a missing path except the drive root exists: the
        // proposal must be None, never `C:\` (validate_grant_path refuses
        // drive roots — the card must never offer what a grant would refuse).
        let missing = std::path::PathBuf::from(r"C:\__unoone_never_exists__\file.txt");
        assert_eq!(propose_grant_folder(&missing), None);
    }

    #[test]
    fn lock_denies_and_removes_every_pending_card() {
        let requests = PendingGrantRequests::default();
        let (_, a) = requests.insert("p".into(), "f".into());
        let (_, b) = requests.insert("q".into(), "g".into());
        requests.deny_all();
        assert!(requests.snapshot().is_empty());
        assert_eq!(*a.0.lock().unwrap(), Some(false));
        assert_eq!(*b.0.lock().unwrap(), Some(false));
    }

    #[test]
    fn an_answered_card_wakes_exactly_once() {
        let requests = PendingGrantRequests::default();
        let (id, decision) = requests.insert("p".to_owned(), "f".to_owned());
        assert_eq!(requests.snapshot().len(), 1);
        let card = &requests.snapshot()[0];
        assert_eq!(card.request_id, id);
        assert_eq!(card.path, "p");
        assert_eq!(card.proposed_folder, "f");
        assert!(requests.answer(id, true));
        // A card answers exactly once — a second answer is refused.
        assert!(!requests.answer(id, true));
        assert!(requests.snapshot().is_empty());
        let slot = decision
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(*slot, Some(true));
    }

    #[test]
    fn an_expired_card_cannot_be_answered() {
        let requests = PendingGrantRequests::default();
        let (id, _) = requests.insert("p".to_owned(), "f".to_owned());
        requests.expire(id);
        assert!(!requests.answer(id, false));
        assert!(requests.snapshot().is_empty());
    }

    #[test]
    fn declined_folders_are_remembered_and_clearable() {
        let requests = PendingGrantRequests::default();
        assert!(!requests.is_declined(r"c:\folder"));
        requests.mark_declined(r"c:\folder".to_owned());
        assert!(requests.is_declined(r"c:\folder"));
        requests.clear_declined(r"c:\folder");
        assert!(!requests.is_declined(r"c:\folder"));
    }
}

#[cfg(test)]
mod run_registry_tests {
    use super::*;

    #[test]
    fn stop_cancels_the_registered_run_once() {
        let registry = HarnessRunRegistry::new();
        let token = CancellationToken::new();
        registry.register("conv-1", token.clone());
        // First stop wins and reports true; the token is genuinely cancelled.
        assert!(registry.stop("conv-1"));
        assert!(token.is_cancelled());
        assert_eq!(token.cause(), Some(CancelCause::User));
        // Second stop: nothing new to cancel (first-cause-wins) — honest false.
        assert!(!registry.stop("conv-1"));
    }

    #[test]
    fn stop_for_unknown_conversation_is_false() {
        let registry = HarnessRunRegistry::new();
        assert!(!registry.stop("never-started"));
    }

    #[test]
    fn finish_removes_the_run_without_cancelling() {
        let registry = HarnessRunRegistry::new();
        let token = CancellationToken::new();
        let id = registry.register("conv-2", token.clone());
        // The run completed normally: finish() must clear the slot but never
        // touch the token (a completed run cannot be "stopped" retroactively).
        registry.finish("conv-2", id);
        assert!(!token.is_cancelled());
        // A late Stop after the run ended reports false — no phantom stop.
        assert!(!registry.stop("conv-2"));
    }

    #[test]
    fn registering_a_new_run_supersedes_the_previous_one() {
        let registry = HarnessRunRegistry::new();
        let first = CancellationToken::new();
        let old_id = registry.register("conv-3", first.clone());
        let second = CancellationToken::new();
        registry.register("conv-3", second.clone());
        registry.finish("conv-3", old_id);
        // The superseded run is cancelled by the newer registration…
        assert!(first.is_cancelled());
        assert_eq!(first.cause(), Some(CancelCause::Parent));
        // …the new run is live and still cancellable by the user.
        assert!(!second.is_cancelled());
        assert!(registry.stop("conv-3"));
        assert_eq!(second.cause(), Some(CancelCause::User));
    }

    #[test]
    fn lock_cancels_and_removes_every_conversation() {
        let registry = HarnessRunRegistry::new();
        let a = CancellationToken::new();
        let b = CancellationToken::new();
        registry.register("a", a.clone());
        registry.register("b", b.clone());
        registry.stop_all();
        assert!(a.is_cancelled() && b.is_cancelled());
        assert!(!registry.stop("a") && !registry.stop("b"));
    }

    #[test]
    fn conversations_are_independent() {
        let registry = HarnessRunRegistry::new();
        let a = CancellationToken::new();
        let b = CancellationToken::new();
        registry.register("conv-a", a.clone());
        registry.register("conv-b", b.clone());
        assert!(registry.stop("conv-a"));
        assert!(a.is_cancelled());
        assert!(!b.is_cancelled());
    }
}

#[cfg(test)]
mod trail_tests {
    use super::*;

    /// The trail stops recording at the step cap with ONE explicit overflow
    /// marker — a pathological run must never grow an unbounded record.
    #[test]
    fn trail_caps_at_max_steps_with_one_overflow_marker() {
        let trail: SharedAgentTrail = Arc::new(Mutex::new(Vec::new()));
        for index in 0..MAX_TRAIL_STEPS + 50 {
            push_trail_step(
                &Some(Arc::clone(&trail)),
                "call",
                "fs.write",
                &format!("step {index}"),
            );
        }
        let steps = trail.lock().unwrap();
        assert_eq!(steps.len(), MAX_TRAIL_STEPS + 1, "cap + exactly one marker");
        assert_eq!(steps.last().unwrap().tool, "(trail truncated)");
        assert!(steps.last().unwrap().detail.contains("run continued"));
    }

    /// Each recorded detail is bounded to the per-step char cap.
    #[test]
    fn trail_details_are_bounded_per_step() {
        let trail: SharedAgentTrail = Arc::new(Mutex::new(Vec::new()));
        push_trail_step(
            &Some(Arc::clone(&trail)),
            "result",
            "fs.read",
            &"x".repeat(MAX_TRAIL_DETAIL_CHARS + 500),
        );
        let steps = trail.lock().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].detail.chars().count(), MAX_TRAIL_DETAIL_CHARS);
    }

    /// No collector (unit-test registrations) and a poisoned lock are
    /// skipped entries, never panics — the trail must never break a run.
    #[test]
    fn trail_without_collector_or_with_poisoned_lock_is_skipped() {
        push_trail_step(&None, "call", "fs.write", "harmless");
        let trail: SharedAgentTrail = Arc::new(Mutex::new(Vec::new()));
        {
            // Poison the lock: the helper thread takes it and panics while
            // HOLDING it, then finishes. Never hold the lock on this thread
            // while joining the helper — that ordering deadlocks (the exact
            // class of hang this test exists to pin, caught live in CI).
            let poison = trail.clone();
            std::thread::spawn(move || {
                let _guard = poison.lock().unwrap();
                panic!("poison the lock");
            })
            .join()
            .ok(); // the panic is intentional; ignore the join result
        }
        push_trail_step(&Some(trail.clone()), "call", "fs.write", "harmless");
        assert!(
            trail.lock().is_err() || trail.lock().map(|s| s.is_empty()).unwrap_or(true),
            "no entry was recorded through the poisoned lock"
        );
    }
}

#[cfg(test)]
mod workspace_tool_tests {
    use super::*;
    use inbharat_harness_core::ExecutionBroker;
    use inbharat_harness_core::LocalExecutionBroker;

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

    /// A granted-folder set wrapping the single fenced workspace — the shape
    /// the search/patch tools take in production (index 0 = primary).
    fn granted(filesystem: &RootedFs) -> GrantedFolders {
        GrantedFolders::new(vec![filesystem.clone()]).expect("granted-folder set")
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
        // Defect #26 (live-caught 2026-09-13): the goal loop divides
        // max_steps across max_rounds, so max_rounds > 1 here silently
        // capped every chat run at 10 model steps and a real long-coding
        // task died mid-run into a read-only fallback. One round = the
        // model loop gets all 10,000 steps (the shared reserve_step still
        // bounds the global total).
        let budget = full_access_budget();
        assert_eq!(budget.max_rounds, 1, "max_rounds must stay 1 (defect #26)");
        assert_eq!(
            budget.max_steps, 10_000,
            "max_steps must stay at the validator ceiling"
        );
    }

    /// A scripted provider for the prose-dump corrective retry (2026-10-03).
    /// Call 1 answers a real task with pasted code and NO tool call — the
    /// live-caught 12B failure ("long instructions come back as prose
    /// instead of actions"); call 2 (after the corrective nudge) makes the
    /// REAL tool call; call 3 reports what the tool actually returned.
    /// `first_action_is_tool_call` flips the script: call 1 acts, call 2
    /// pastes code — a model that already acted is never corrected.
    struct ProseDumpProvider {
        calls: Mutex<u32>,
        first_action_is_tool_call: bool,
    }

    impl inbharat_harness_core::ModelProvider for ProseDumpProvider {
        fn id(&self) -> &str {
            "prose"
        }
        fn models(&self) -> Vec<String> {
            vec!["prose-v1".to_owned()]
        }
        fn stream(
            &self,
            _request: &inbharat_harness_core::ModelRequest,
            _cancel: &CancellationToken,
            sink: &mut dyn FnMut(inbharat_harness_core::ModelChunk) -> HarnessResult<()>,
        ) -> HarnessResult<inbharat_harness_core::ModelResponse> {
            use inbharat_harness_core::providers::{FinishReason, ModelChunk, ModelResponse};
            let call = {
                let mut calls = self
                    .calls
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *calls += 1;
                *calls
            };
            let respond_text = |sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
                                text: &str|
             -> HarnessResult<ModelResponse> {
                sink(ModelChunk::TextDelta {
                    block: 0,
                    text: text.to_owned(),
                })?;
                Ok(ModelResponse {
                    text: text.to_owned(),
                    finish: FinishReason::Stop,
                    input_units: 1,
                    output_units: 1,
                    provider_request_id: None,
                })
            };
            let act_with_fs_list = |sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>| {
                sink(ModelChunk::ToolCall {
                    block: 0,
                    call_id: "call-1".to_owned(),
                    tool_id: "fs.list".to_owned(),
                    arguments: "{\"path\":\".\"}".to_owned(),
                })?;
                Ok(ModelResponse {
                    text: String::new(),
                    finish: FinishReason::ToolCalls,
                    input_units: 1,
                    output_units: 1,
                    provider_request_id: None,
                })
            };
            match call {
                1 if self.first_action_is_tool_call => act_with_fs_list(sink),
                1 => respond_text(
                    sink,
                    "Here is the program:\n```js\nconsole.log(\"hi\");\n```\n",
                ),
                2 if self.first_action_is_tool_call => respond_text(
                    sink,
                    "Here is the follow-up:\n```js\nconsole.log(\"bye\");\n```\n",
                ),
                2 => act_with_fs_list(sink),
                _ => respond_text(sink, "The directory is listed. Done."),
            }
        }
    }

    /// A harness with the fs.list tool registered and the scripted provider
    /// — the exact shape the corrective-retry tests run against.
    fn prose_dump_harness(provider: ProseDumpProvider) -> inbharat_harness_core::Harness {
        let (dir, filesystem) = temp_workspace();
        drop(dir);
        let broker = GrantedFolderBroker::new(granted(&filesystem), std::iter::empty::<String>());
        HarnessBuilder::embedded(Arc::new(broker))
            .expect("harness builder")
            .register_model(Arc::new(provider))
            .expect("register scripted model")
            .register_tool(Arc::new(ListFilesTool::default()))
            .expect("register fs.list")
            .confirmation_provider(Arc::new(StaticConfirmationProvider {
                outcome: ConfirmationOutcome::AllowedOnce,
            }))
            .build()
    }

    fn prose_dump_options(corrective_retries: u32) -> RunOptions {
        RunOptions {
            actor: "local-user".to_owned(),
            provider: "prose".to_owned(),
            model: "prose-v1".to_owned(),
            explicit_level: Some(ExecutionLevel::L3),
            budget: Some(full_access_budget()),
            capabilities: CapabilitySet::all_local(),
            corrective_retries,
            ..RunOptions::default()
        }
    }

    #[test]
    fn a_prose_dump_gets_one_corrective_retry_and_then_acts() {
        let harness = prose_dump_harness(ProseDumpProvider {
            calls: Mutex::new(0),
            first_action_is_tool_call: false,
        });
        let (outcome, session) = harness
            .run(
                "list the directory",
                &prose_dump_options(1),
                &CancellationToken::new(),
            )
            .expect("the corrected run completes");
        // The final output is the model's real report — not the prose dump.
        assert_eq!(outcome.output, "The directory is listed. Done.");
        // The run actually ACTED: without the corrective retry this would
        // be 0 and the output would be the pasted code block.
        assert_eq!(outcome.tool_calls, 1, "the retry must act with the tool");
        assert!(
            session.replay().expect("replay session").balanced,
            "the session must stay audit-balanced with the corrective retry"
        );
    }

    #[test]
    fn a_prose_dump_without_the_opt_in_stands() {
        let harness = prose_dump_harness(ProseDumpProvider {
            calls: Mutex::new(0),
            first_action_is_tool_call: false,
        });
        let (outcome, _session) = harness
            .run(
                "list the directory",
                &prose_dump_options(0),
                &CancellationToken::new(),
            )
            .expect("the uncorrected run completes");
        // corrective_retries = 0 disables the fix — the model's text stands
        // verbatim (other RunOptions consumers are unchanged).
        assert!(outcome.output.contains("```js"));
        assert_eq!(outcome.tool_calls, 0);
    }

    #[test]
    fn a_model_that_already_acted_is_never_corrected() {
        let harness = prose_dump_harness(ProseDumpProvider {
            calls: Mutex::new(0),
            first_action_is_tool_call: true,
        });
        let (outcome, _session) = harness
            .run(
                "list the directory then show me code",
                &prose_dump_options(1),
                &CancellationToken::new(),
            )
            .expect("the run completes");
        // Call 2 pastes a code block, but the model already used its tools:
        // the fenced code IS the answer (e.g. the user asked to see code),
        // so it must not be second-guessed.
        assert!(outcome.output.contains("```js"));
        assert_eq!(outcome.tool_calls, 1, "the real tool call still ran");
    }

    #[test]
    fn a_corrective_retry_is_bounded_to_one_nudge() {
        // After the nudge, if the model pastes code AGAIN, the text stands —
        // the retry budget is 1, not a loop.
        struct RepeatProseProvider;
        impl inbharat_harness_core::ModelProvider for RepeatProseProvider {
            fn id(&self) -> &str {
                "prose"
            }
            fn models(&self) -> Vec<String> {
                vec!["prose-v1".to_owned()]
            }
            fn stream(
                &self,
                _request: &inbharat_harness_core::ModelRequest,
                _cancel: &CancellationToken,
                sink: &mut dyn FnMut(inbharat_harness_core::ModelChunk) -> HarnessResult<()>,
            ) -> HarnessResult<inbharat_harness_core::ModelResponse> {
                let text = "Again as text:\n```js\nconsole.log(\"x\");\n```\n".to_owned();
                sink(inbharat_harness_core::ModelChunk::TextDelta {
                    block: 0,
                    text: text.clone(),
                })?;
                Ok(inbharat_harness_core::providers::ModelResponse {
                    text,
                    finish: inbharat_harness_core::providers::FinishReason::Stop,
                    input_units: 1,
                    output_units: 1,
                    provider_request_id: None,
                })
            }
        }
        let (dir, filesystem) = temp_workspace();
        drop(dir);
        let broker = GrantedFolderBroker::new(granted(&filesystem), std::iter::empty::<String>());
        let harness = HarnessBuilder::embedded(Arc::new(broker))
            .expect("harness builder")
            .register_model(Arc::new(RepeatProseProvider))
            .expect("register repeat-prose model")
            .register_tool(Arc::new(ListFilesTool::default()))
            .expect("register fs.list")
            .confirmation_provider(Arc::new(StaticConfirmationProvider {
                outcome: ConfirmationOutcome::AllowedOnce,
            }))
            .build();
        let (outcome, _session) = harness
            .run(
                "list the directory",
                &prose_dump_options(1),
                &CancellationToken::new(),
            )
            .expect("the run completes after one nudge");
        assert!(outcome.output.contains("```js"));
        assert_eq!(outcome.tool_calls, 0, "no infinite nudge loop");
        // The budget proves the second dump was accepted after exactly one
        // corrective retry: the run consumed at most 3 logical steps
        // (initial + retry + final).
        assert!(outcome.steps <= 3, "steps: {}", outcome.steps);
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

    /// 2026-09-14 long-coding acceptance run 3 (post defect-#33): the agent
    /// wrote a 4-file app whose test then failed on its own one-line bug
    /// (`__dirname`-relative server path), and instead of iterating it
    /// reported the task complete with a FALSE claim ("all files are
    /// correctly written and functional"). The full-access briefing must
    /// command the Codex-grade behavior explicitly: a failing test is the
    /// next step, never a completion signal.
    #[test]
    fn full_access_briefing_requires_iteration_on_failure() {
        let prompt = desktop_system_prefix(true);
        assert!(
            prompt.contains("A failing test or command is the next step of the task, not the end"),
            "the briefing must define what to do when a run fails"
        );
        assert!(
            prompt.contains("iterate like this until the test passes"),
            "the briefing must command the iterate-to-green loop"
        );
        assert!(
            prompt
                .contains("Never report the task complete or claim code is 'functional' while its own output shows a failure"),
            "the briefing must forbid false completion claims"
        );
        assert!(
            prompt.contains("never explain a failure away with environment speculation"),
            "the briefing must forbid rationalizing real defects"
        );
        // The read-only lane stays honest: no agent-tools briefing there.
        assert!(
            !desktop_system_prefix(false).contains("A failing test"),
            "read-only mode must not claim agent tools it does not hold"
        );
    }

    /// Defect #36 (live-caught 2026-09-14): the agent's answers said only
    /// "in your workspace" and the user could not find the files the tool
    /// built. The briefing must force absolute paths, and the command that
    /// feeds the chat label must return the real expanded root. Defect #37:
    /// every progress event must carry a local HH:MM:SS timestamp.
    #[test]
    fn briefing_requires_absolute_paths_and_timestamped_progress() {
        let prompt = desktop_system_prefix(true);
        assert!(
            prompt.contains("listing the exact ABSOLUTE path of every file you created or changed"),
            "the briefing must force absolute file paths in answers"
        );
        assert!(
            prompt.contains("never say only 'in your workspace' or a bare filename"),
            "the briefing must forbid vague file locations"
        );
        assert!(
            prompt.contains("the workspace root is"),
            "the briefing must tell the agent the workspace root"
        );

        // Defect #38 (live-caught 2026-09-15): the agent must know it can
        // deploy servers in the background — its foreground node deploy was
        // killed at the 180s deadline because a server never exits.
        assert!(
            prompt.contains("background:true to process.run"),
            "the briefing must teach the background deploy lane"
        );
        assert!(
            prompt.contains("browser.act to the served"),
            "the briefing must teach verifying a deploy through the browser"
        );

        // Multi-agent lane (2026-09-15): the briefing must teach the
        // delegation tool — an untaught tool is an unused tool (the
        // defect-#38 lesson, one defect earlier).
        assert!(
            prompt.contains("agent.spawn"),
            "the briefing must teach the agent.spawn sub-agent lane"
        );
        assert!(
            prompt.contains("COMPLETE, self-contained task"),
            "the briefing must teach that a sub-agent knows nothing but its task"
        );

        // Defect #36 label: the command's backing resolver must return the
        // real expanded root, never the literal %USERPROFILE% pattern.
        let root =
            workspace_root().unwrap_or_else(|failure| panic!("workspace_root failed: {failure}"));
        assert!(
            root.is_absolute(),
            "the workspace root must be absolute, got {}",
            root.display()
        );

        // Defect #37: HH:MM:SS local wall-clock shape on every event.
        let stamp = progress_timestamp();
        assert_eq!(
            stamp.len(),
            8,
            "progress timestamps must be HH:MM:SS, got {stamp}"
        );
        assert_eq!(stamp.as_bytes()[2], b':', "expected HH:MM:SS, got {stamp}");
        assert_eq!(stamp.as_bytes()[5], b':', "expected HH:MM:SS, got {stamp}");
        let parts: Vec<u32> = stamp
            .split(':')
            .map(|piece| piece.parse().unwrap_or(u32::MAX))
            .collect();
        assert_eq!(parts.len(), 3, "expected HH:MM:SS, got {stamp}");
        assert!(
            parts[0] < 24 && parts[1] < 60 && parts[2] < 60,
            "invalid time {stamp}"
        );
    }

    /// 2026-10-03 user directive: account work (email, Notion, Instagram)
    /// rides the PERSISTENT browser session with no API keys, and the model
    /// must never handle the user's credentials itself. The briefing must
    /// say exactly that — an untaught boundary is an unusable lane (the
    /// defect-#38 lesson), and a wrong claim here is a privacy defect.
    #[test]
    fn briefing_teaches_the_persistent_browser_session_and_credential_boundary() {
        let prompt = desktop_system_prefix(true);
        assert!(
            prompt.contains("the browser session is PERSISTENT"),
            "the briefing must teach that logins persist across restarts"
        );
        assert!(
            prompt.contains("stays logged in across app restarts"),
            "the briefing must state the persistence boundary concretely"
        );
        assert!(
            prompt.contains("NO API keys or OAuth apps"),
            "the briefing must tell the model accounts need no API"
        );
        assert!(
            prompt.contains("Never ask the user for their password"),
            "the briefing must forbid requesting credentials"
        );
        assert!(
            prompt.contains("never type credentials for them"),
            "the briefing must route logins through the human, not the model"
        );
        assert!(
            !desktop_system_prefix(false).contains("PERSISTENT"),
            "read-only mode must not claim the browser lane"
        );

        // The sub-agent inherits the same session honesty, with the added
        // rule that it REPORTS a missing login instead of asking the user
        // (it never talks to the user).
        let subagent = subagent_system_prefix();
        assert!(
            subagent.contains("PERSISTENT"),
            "the sub-agent briefing must state the persistent session"
        );
        assert!(
            subagent.contains("never ask for or type credentials"),
            "the sub-agent briefing must forbid credential handling"
        );
        assert!(
            subagent.contains("so the PARENT tells the user"),
            "a missing login must surface through the parent, not the child"
        );
    }

    /// The design-website lane (2026-10-03): the user's standing ask is that
    /// the agent "build design websites with images" like any mainstream
    /// assistant. The briefing must teach the three real capabilities that
    /// make that possible — designed CSS/SVG authoring, the binary-safe
    /// copy for the user's image files, and the live preview — AND the one
    /// trap (fs.read+fs.write corrupts binary images) so the model never
    /// fakes a copy through the text lane. An untaught lane is an unused
    /// lane (the defect-#38 lesson).
    #[test]
    fn briefing_teaches_the_design_website_lane() {
        let prompt = desktop_system_prefix(true);
        assert!(
            prompt.contains("Build DESIGNED, visually rich websites"),
            "the briefing must open the design lane explicitly"
        );
        assert!(
            prompt.contains("author any graphic (logo, icon, illustration) yourself as inline SVG"),
            "the briefing must teach SVG as the self-serve graphics path"
        );
        assert!(
            prompt.contains("copy them with fs.copy — it is binary-safe and byte-exact"),
            "the briefing must route image copies through fs.copy"
        );
        assert!(
            prompt.contains("fs.read + fs.write would CORRUPT images (they are UTF-8 text-only)"),
            "the briefing must name the text-lane trap"
        );
        assert!(
            prompt.contains("Never claim images or graphics are impossible"),
            "the briefing must forbid claiming the design lane is impossible"
        );
        assert!(
            !desktop_system_prefix(false).contains("fs.copy"),
            "read-only mode must not claim the copy lane"
        );

        // The sub-agent inherits the same binary-safety rule.
        let subagent = subagent_system_prefix();
        assert!(
            subagent.contains("fs.read + fs.write would corrupt images"),
            "the sub-agent briefing must carry the text-lane trap"
        );
    }

    /// 2026-09-14 live agent progress: the human phrasing of a tool call must
    /// carry the concrete file/command (the user watches "Writing
    /// app/index.html (1024 bytes)", not a generic "calling tool"), while a
    /// missing argument degrades to a generic line instead of an empty one.
    #[test]
    fn progress_detail_phrases_each_tool_concretely() {
        assert_eq!(
            progress_detail(
                "fs.write",
                &string_args(&[
                    ("path", "app/index.html"),
                    ("contents", "x".repeat(1024).as_str())
                ])
            ),
            "Writing app/index.html (1024 bytes)"
        );
        assert_eq!(
            progress_detail("fs.write", &string_args(&[("path", "a.txt")])),
            "Writing a.txt"
        );
        assert_eq!(progress_detail("fs.write", &args(&[])), "Writing file");
        assert_eq!(
            progress_detail(
                "fs.read",
                &string_args(&[("path", "C:\\Users\\me\\notes.md")])
            ),
            "Reading C:\\Users\\me\\notes.md"
        );
        assert_eq!(
            progress_detail("fs.list", &string_args(&[("path", "app")])),
            "Listing app"
        );
        assert_eq!(
            progress_detail("fs.mkdir", &string_args(&[("path", "dist")])),
            "Creating directory dist"
        );
        assert_eq!(
            progress_detail(
                "fs.copy",
                &string_args(&[("from", "assets/photo.png"), ("to", "site/img/photo.png")])
            ),
            "Copying assets/photo.png → site/img/photo.png"
        );
        assert_eq!(
            progress_detail("fs.copy", &string_args(&[("from", "logo.png")])),
            "Copying logo.png"
        );
        assert_eq!(progress_detail("fs.copy", &args(&[])), "Copying file");
        assert_eq!(
            progress_detail("process.run", &string_args(&[("program", "node")])),
            "Running node"
        );
        // Defect #38: a background deploy must read as a deploy in the feed.
        assert_eq!(
            progress_detail(
                "process.run",
                &args(&[
                    ("program", Value::String("node".to_owned())),
                    (
                        "args",
                        Value::Array(vec![Value::String("server.js".to_owned())])
                    ),
                    ("background", Value::Bool(true))
                ])
            ),
            "Starting node in background"
        );
        assert_eq!(
            progress_detail(
                "browser.act",
                &string_args(&[("action", "navigate"), ("url", "https://example.com")])
            ),
            "Browser: navigate https://example.com"
        );
        assert_eq!(
            progress_detail("browser.act", &string_args(&[("action", "click")])),
            "Browser: click"
        );
        assert_eq!(
            progress_detail("workspace.search", &args(&[])),
            "Searching the workspace"
        );
        assert_eq!(
            progress_detail("workspace.patch", &args(&[])),
            "Patching a workspace file"
        );
        // Unknown tools fall back to their canonical JSON, truncated.
        assert_eq!(
            progress_detail("custom.tool", &string_args(&[("k", "v")])),
            "custom.tool {\"k\":\"v\"}"
        );
        let long_value = "v".repeat(400);
        let big = string_args(&[("k", long_value.as_str())]);
        let line = progress_detail("custom.tool", &big);
        assert!(line.ends_with('…'));
        assert!(line.chars().count() <= "custom.tool ".chars().count() + 160 + 1);
    }

    /// 2026-09-14 live agent progress: only fs.write previews its contents,
    /// bounded at 240 chars so a huge write cannot flood the IPC channel.
    #[test]
    fn progress_code_preview_shows_only_bounded_fs_write_heads() {
        assert_eq!(
            progress_code_preview("fs.write", &string_args(&[("contents", "let x = 1;\n")])),
            Some("let x = 1;\n".to_owned())
        );
        let exact_240 = "h".repeat(240);
        assert_eq!(
            progress_code_preview(
                "fs.write",
                &string_args(&[("contents", exact_240.as_str())])
            ),
            Some("h".repeat(240))
        );
        let huge = "h".repeat(1000);
        assert_eq!(
            progress_code_preview("fs.write", &string_args(&[("contents", huge.as_str())])),
            Some("h".repeat(240)),
            "preview must be bounded at 240 chars"
        );
        assert_eq!(
            progress_code_preview("fs.write", &string_args(&[("contents", "")])).as_deref(),
            None
        );
        assert_eq!(progress_code_preview("fs.write", &args(&[])), None);
        assert!(progress_code_preview("fs.read", &string_args(&[("path", "x.txt")])).is_none());
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
        let tool = DesktopSearchTool::new(granted(&filesystem));
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
        let tool = DesktopSearchTool::new(granted(&filesystem));
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
        let tool = DesktopSearchTool::new(granted(&filesystem));
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
        let tool = DesktopPatchTool::new(granted(&filesystem));
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
        let tool = DesktopPatchTool::new(granted(&filesystem));
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
        let tool = DesktopPatchTool::new(granted(&filesystem));
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
    fn doc_create_writes_real_readable_documents_through_the_fence() {
        let (_dir, filesystem) = temp_workspace();
        let tool = DesktopDocCreateTool::new(granted(&filesystem));
        let broker = harness_broker(&filesystem);
        let cancel = CancellationToken::new();
        let context = tool_context(&filesystem, &cancel, &broker);

        // A real PDF: binary through the fence, readable by the real reader.
        let pdf_args = string_args(&[
            ("filename", "reports/field.pdf"),
            ("title", "Field Report"),
            (
                "content",
                "The agent wrote this PDF itself.\nSecond paragraph verifies the round trip.",
            ),
        ]);
        tool.execute(&pdf_args, &context).expect("create pdf");
        let pdf_bytes = std::fs::read(_dir.join("reports").join("field.pdf"))
            .expect("pdf landed inside the fence");
        assert!(pdf_bytes.starts_with(b"%PDF"), "the file is a real PDF");
        let copy = std::env::temp_dir().join(format!(
            "unoone-doc-create-{}.pdf",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&copy, &pdf_bytes).expect("stage for the reader");
        let text = crate::documents::extract_pdf_text(&copy).expect("reader round trip");
        assert!(text.contains("The agent wrote this PDF itself."));
        std::fs::remove_file(&copy).ok();

        // A real DOCX: readable by the real reader, one paragraph per line.
        let docx_args = string_args(&[
            ("filename", "reports/field.docx"),
            ("content", "Docx line one.\nDocx line two."),
        ]);
        tool.execute(&docx_args, &context).expect("create docx");
        let docx_bytes = std::fs::read(_dir.join("reports").join("field.docx"))
            .expect("docx landed inside the fence");
        let copy = std::env::temp_dir().join(format!(
            "unoone-doc-create-{}.docx",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&copy, &docx_bytes).expect("stage for the reader");
        let text = crate::documents::extract_docx_text(&copy).expect("reader round trip");
        assert!(text.contains("Docx line one."));
        assert!(text.contains("Docx line two."));
        std::fs::remove_file(&copy).ok();

        // Plain text: md and txt write the text verbatim.
        let txt_args = string_args(&[("filename", "notes/todo.md"), ("content", "# Todo\n- ship")]);
        tool.execute(&txt_args, &context).expect("create md");
        assert_eq!(
            filesystem.read_text("notes/todo.md").expect("re-read"),
            "# Todo\n- ship"
        );

        // The format guards refuse unknown formats and unknown arguments
        // without touching the filesystem.
        let bad_format = string_args(&[("filename", "x.pptx"), ("content", "no")]);
        assert!(tool.execute(&bad_format, &context).is_err());
        let unknown_arg = args(&[
            ("filename", Value::String("x.txt".to_owned())),
            ("content", Value::String("no".to_owned())),
            ("styles", Value::Bool(true)),
        ]);
        assert!(tool.validate_arguments(&unknown_arg).is_err());

        // Escape attempts die at the fence, outside the granted folders.
        let escape = string_args(&[("filename", "../outside.txt"), ("content", "no")]);
        assert!(tool.execute(&escape, &context).is_err());
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

    /// A provider with no UI app handle: enough to exercise the spawn
    /// tool's validation and the depth gate, which both run before any
    /// child harness is built.
    fn test_subagent_provider() -> Arc<PaiSubagentProvider> {
        Arc::new(PaiSubagentProvider::new(
            "test-model".to_owned(),
            1,
            "unused-root".to_owned(),
            Arc::new(Mutex::new(None)),
            "test-vault".to_owned(),
            Arc::new(Mutex::new(DesktopSafetyGuard::new(
                crate::safety::SecurityLevel::Off,
            ))),
            Arc::new(BrowserStateHolder::new()),
            None,
            CapabilitySet::all_local(),
            Arc::new(Mutex::new(Vec::new())),
        ))
    }

    /// Multi-agent lane (2026-09-15): the spawn tool's manifest is the
    /// model-facing contract. The defect-#38 lesson applies from day one
    /// — the schema must declare the task argument, and the tool must be
    /// L3-only behind the Subagent capability so no lower lane can spawn.
    #[test]
    fn agent_spawn_manifest_declares_the_contract() {
        let tool = AgentSpawnTool::new(test_subagent_provider());
        let manifest = tool.manifest();
        assert_eq!(manifest.id, "agent.spawn");
        assert!(
            manifest.input_schema.contains("task"),
            "the spawn schema must declare the task argument"
        );
        assert!(
            manifest.input_schema.contains("\"required\""),
            "the spawn schema must mark task required"
        );
        assert!(manifest
            .required_capabilities
            .contains(Capability::Subagent));
        assert_eq!(manifest.supported_levels, vec![ExecutionLevel::L3]);
        assert!(manifest.validate().is_ok());
    }

    /// A 12B model calls a simple schema reliably; validation must reject
    /// everything but a well-formed task string.
    #[test]
    fn agent_spawn_validation_rejects_malformed_calls() {
        let tool = AgentSpawnTool::new(test_subagent_provider());
        // missing task
        let missing = args(&[]);
        assert!(tool.validate_arguments(&missing).is_err());
        // empty task
        let empty = string_args(&[("task", "")]);
        assert!(tool.validate_arguments(&empty).is_err());
        // whitespace-only task
        let blank = string_args(&[("task", "   ")]);
        assert!(tool.validate_arguments(&blank).is_err());
        // oversized task
        let oversized = args(&[("task", Value::String("x".repeat(32 * 1024 + 1)))]);
        assert!(tool.validate_arguments(&oversized).is_err());
        // unsupported extra argument
        let extra = string_args(&[("task", "build it"), ("scope", "files")]);
        assert!(tool.validate_arguments(&extra).is_err());
        // a real task passes
        let ok = string_args(&[("task", "Write the test file for the counter module.")]);
        assert!(tool.validate_arguments(&ok).is_ok());
    }

    /// The depth chain: an actor at the ceiling (a depth-2 child) cannot
    /// spawn again — run_scoped_subagent refuses before any child harness
    /// is built, with no model involved.
    #[test]
    fn agent_spawn_refuses_depth_past_the_ceiling() {
        let (_dir, filesystem) = temp_workspace();
        let broker = harness_broker(&filesystem);
        let cancel = CancellationToken::new();
        let mut context = tool_context(&filesystem, &cancel, &broker);
        context.actor = "subagent-ab12cd34-d2";
        let tool = AgentSpawnTool::new(test_subagent_provider());
        let call = string_args(&[("task", "one more level down")]);
        let failure = tool
            .execute(&call, &context)
            .expect_err("a depth-2 child must not spawn again");
        assert_eq!(
            failure.code,
            ErrorCode::PermissionDenied,
            "depth overflow must be a permission denial"
        );

        // depth parsing: the actor stamp is the only depth source.
        assert_eq!(actor_depth("local-user"), 0);
        assert_eq!(actor_depth("subagent-ab12cd34-d1"), 1);
        assert_eq!(actor_depth("subagent-ab12cd34-d2"), 2);
        assert_eq!(actor_depth("bob"), 0);
        // a forged non-numeric depth stamp reads as top level and still
        // cannot exceed the ceiling: the child it spawns is depth 1.
        assert_eq!(actor_depth("subagent-ab12cd34-dNaN"), 0);
    }

    /// The budget must open the subagent lane: depth 2 matches the ceiling
    /// the spawn tool and run_scoped_subagent enforce.
    #[test]
    fn full_access_budget_opens_the_subagent_lane() {
        assert_eq!(
            full_access_budget().max_subagent_depth,
            SUBAGENT_DEPTH_CEILING,
            "the full-access budget must match the spawn-tool ceiling"
        );
        assert_eq!(SUBAGENT_DEPTH_CEILING, 2);
        // A depth-1 child's own budget has one level of headroom left
        // (ceiling 2 − depth 1); a depth-2 child has none.
        assert_eq!(
            subagent_budget(SUBAGENT_DEPTH_CEILING - 1).max_subagent_depth,
            1
        );
        assert_eq!(subagent_budget(0).max_subagent_depth, 0);
    }

    /// The sub-agent briefing must demand self-contained evidence — the
    /// parent is told to verify, so the child must make that possible.
    #[test]
    fn subagent_briefing_demands_a_verifiable_report() {
        let prompt = subagent_system_prefix();
        assert!(
            prompt.contains("ABSOLUTE path of every file you created or changed"),
            "the sub-agent briefing must force absolute paths in reports"
        );
        assert!(
            prompt.contains("the exact output of anything you ran"),
            "the sub-agent briefing must demand command output as evidence"
        );
        assert!(
            prompt.contains("You never talk to the user directly"),
            "the sub-agent briefing must pin the reporting relationship"
        );
    }
}
