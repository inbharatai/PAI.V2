//! Capability-aware tool manifests, dynamic exposure, and non-bypassable dispatch.

use crate::budget::Budget;
use crate::cancel::CancellationToken;
use crate::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use crate::execution::{ExecutionBroker, ProcessSpec};
use crate::providers::{
    Capability, CapabilitySet, ConfirmationOutcome, ConfirmationProvider, ConfirmationRequest,
    EnforcementQuality, ModelTool, PermissionDecision, PermissionProvider, SandboxProvider,
    SandboxRequest, VerificationProvider,
};
use crate::routing::ExecutionLevel;
use crate::value::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// String-keyed canonical tool arguments.
pub type ToolArguments = BTreeMap<String, Value>;

/// Determinism classification used by recovery policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Determinism {
    Deterministic,
    Idempotent,
    NonIdempotent,
}

/// Side-effect class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SideEffect {
    None,
    Read,
    Write,
    Process,
    Network,
}

/// Confirmation posture declared by a tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmationMode {
    Never,
    OnSideEffect,
    Always,
}

/// Immutable capability manifest for one tool version.
#[derive(Clone, Debug)]
pub struct ToolManifest {
    pub id: String,
    pub version: String,
    pub description: String,
    pub input_schema: String,
    pub output_schema: String,
    pub required_capabilities: CapabilitySet,
    pub supported_levels: Vec<ExecutionLevel>,
    pub determinism: Determinism,
    pub side_effect: SideEffect,
    pub confirmation: ConfirmationMode,
    pub concurrency_safe: bool,
    pub default_timeout: Duration,
    pub max_output_bytes: usize,
    pub verification: String,
    pub compensation: String,
}

impl ToolManifest {
    pub fn validate(&self) -> HarnessResult<()> {
        if !valid_tool_id(&self.id)
            || self.version.is_empty()
            || self.version.len() > 64
            || self.description.is_empty()
            || self.description.len() > 4096
            || self.input_schema.len() > 256 * 1024
            || self.output_schema.len() > 256 * 1024
            || self.supported_levels.is_empty()
            || self.default_timeout.is_zero()
            || self.max_output_bytes == 0
            || self.max_output_bytes > 8 * 1024 * 1024
            || self.verification.len() > 1024
            || self.compensation.len() > 1024
        {
            return Err(Failure::invalid(
                "tool.manifest",
                "tool manifest is incomplete or invalid",
            ));
        }
        if self.side_effect != SideEffect::None
            && self.confirmation == ConfirmationMode::Never
            && self.side_effect != SideEffect::Read
        {
            return Err(Failure::invalid(
                "tool.manifest",
                "mutating tools must declare a confirmation policy",
            ));
        }
        let input_schema = Value::parse_json(&self.input_schema).map_err(|message| {
            Failure::invalid(
                "tool.manifest",
                format!("invalid input schema JSON: {message}"),
            )
        })?;
        let output_schema = Value::parse_json(&self.output_schema).map_err(|message| {
            Failure::invalid(
                "tool.manifest",
                format!("invalid output schema JSON: {message}"),
            )
        })?;
        if input_schema.as_object().is_none() || output_schema.as_object().is_none() {
            return Err(Failure::invalid(
                "tool.manifest",
                "tool schemas must be JSON objects",
            ));
        }
        Ok(())
    }
}

/// Canonical result separated from bounded model presentation.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    pub value: Value,
    pub model_content: String,
    pub presentation: BTreeMap<String, String>,
}

/// Durable confirmation audit facts emitted by the non-bypassable dispatcher.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolAuditEvent {
    ConfirmationAsked(ConfirmationRequest),
    ConfirmationDecided {
        request_id: String,
        outcome: ConfirmationOutcome,
    },
}

/// Explicit dependencies passed to a tool body.
pub struct ToolContext<'a> {
    pub actor: &'a str,
    pub level: ExecutionLevel,
    pub execution: &'a dyn ExecutionBroker,
    pub cancel: &'a CancellationToken,
}

/// Built-in and statically linked tools implement this trait.
pub trait Tool: Send + Sync {
    fn manifest(&self) -> &ToolManifest;
    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()>;
    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput>;
}

/// Owner-safe registry. Duplicate IDs fail rather than shadow silently.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolRegistry")
            .field("tool_ids", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) -> HarnessResult<()> {
        tool.manifest().validate()?;
        let id = tool.manifest().id.clone();
        if self.tools.contains_key(&id) {
            return Err(Failure::new(
                ErrorCode::Conflict,
                FailureClass::Internal,
                "tool.register",
                "tool id is already registered",
            )
            .with_detail("tool_id", id));
        }
        self.tools.insert(id, tool);
        Ok(())
    }

    /// Resolves only tools both visible and authorized by the capability snapshot.
    #[must_use]
    pub fn visible(
        &self,
        level: ExecutionLevel,
        capabilities: &CapabilitySet,
    ) -> Vec<&ToolManifest> {
        self.tools
            .values()
            .map(|tool| tool.manifest())
            .filter(|manifest| manifest.supported_levels.contains(&level))
            .filter(|manifest| manifest.required_capabilities.is_subset_of(capabilities))
            .collect()
    }

    /// Model-facing dynamic tool snapshot in stable tool-id order.
    #[must_use]
    pub fn model_tools(
        &self,
        level: ExecutionLevel,
        capabilities: &CapabilitySet,
    ) -> Vec<ModelTool> {
        self.visible(level, capabilities)
            .into_iter()
            .map(|manifest| ModelTool {
                id: manifest.id.clone(),
                description: manifest.description.clone(),
                input_schema: manifest.input_schema.clone(),
            })
            .collect()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(id).cloned()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// Full dispatch dependencies; every policy stage is mandatory.
pub struct ToolDispatch<'a> {
    pub registry: &'a ToolRegistry,
    pub permission: &'a dyn PermissionProvider,
    pub confirmation: &'a dyn ConfirmationProvider,
    pub verifier: &'a dyn VerificationProvider,
    pub sandbox: &'a dyn SandboxProvider,
    pub execution: &'a dyn ExecutionBroker,
}

impl ToolDispatch<'_> {
    /// validate -> authorize -> confirm -> budget -> sandbox -> execute -> validate -> verify.
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &self,
        id: &str,
        invocation_id: &str,
        arguments: &ToolArguments,
        actor: &str,
        level: ExecutionLevel,
        capabilities: &CapabilitySet,
        budget: &mut Budget,
        cancel: &CancellationToken,
        audit: &mut dyn FnMut(ToolAuditEvent) -> HarnessResult<()>,
    ) -> HarnessResult<ToolOutput> {
        if invocation_id.is_empty() || invocation_id.len() > 128 {
            return Err(Failure::invalid(
                "tool.dispatch",
                "tool invocation id is invalid",
            ));
        }
        cancel.check("tool.dispatch")?;
        let tool = self.registry.get(id).ok_or_else(|| {
            Failure::new(
                ErrorCode::NotFound,
                FailureClass::User,
                "tool.resolve",
                "tool is not visible",
            )
        })?;
        let manifest = tool.manifest();
        if !manifest.supported_levels.contains(&level)
            || !manifest.required_capabilities.is_subset_of(capabilities)
        {
            return Err(Failure::new(
                ErrorCode::PermissionDenied,
                FailureClass::Policy,
                "tool.resolve",
                "tool is outside the current capability set",
            ));
        }
        tool.validate_arguments(arguments)?;
        // Each capability that requires an Ask gets its OWN approval prompt,
        // named by capability. The old code confirmed once and let that single
        // approval cover every remaining required capability in the dispatch —
        // an over-grant for any multi-capability tool. There is no shared
        // `confirmed` shortcut now.
        let mut any_asked = false;
        for capability in manifest.required_capabilities.names() {
            let capability = capability_from_name(capability).ok_or_else(|| {
                Failure::new(
                    ErrorCode::Internal,
                    FailureClass::Internal,
                    "tool.authorize",
                    "unknown capability in manifest",
                )
            })?;
            match self.permission.authorize(actor, capability, &manifest.id)? {
                PermissionDecision::Allow => {}
                PermissionDecision::Ask => {
                    any_asked = true;
                    self.require_confirmation(
                        manifest,
                        invocation_id,
                        actor,
                        Some(capability),
                        cancel,
                        audit,
                    )?;
                }
                PermissionDecision::Deny { rule_id, reason } => {
                    return Err(Failure::new(
                        ErrorCode::PermissionDenied,
                        FailureClass::Policy,
                        "tool.authorize",
                        reason,
                    )
                    .with_detail("rule_id", rule_id));
                }
            }
        }
        // A manifest-level confirmation (ConfirmationMode::Always / side-effect)
        // is still required even if every capability returned Allow, but it is
        // skipped if an Ask prompt was already presented for this invocation —
        // one tool-level approval is enough when a capability prompt fired.
        if !any_asked
            && (manifest.confirmation == ConfirmationMode::Always
                || (manifest.confirmation == ConfirmationMode::OnSideEffect
                    && !matches!(manifest.side_effect, SideEffect::None | SideEffect::Read)))
        {
            self.require_confirmation(manifest, invocation_id, actor, None, cancel, audit)?;
        }
        budget.reserve_tool_call()?;
        let grant = self.sandbox.resolve(&SandboxRequest {
            world_id: self.execution.world_id().to_owned(),
            capabilities: manifest.required_capabilities.clone(),
            require_security_boundary: matches!(
                manifest.side_effect,
                SideEffect::Process | SideEffect::Network
            ),
        })?;
        if grant.world_id != self.execution.world_id()
            || !manifest.required_capabilities.is_subset_of(&grant.granted)
        {
            return Err(Failure::new(
                ErrorCode::SandboxUnavailable,
                FailureClass::Policy,
                "tool.sandbox",
                "sandbox grant does not match execution world",
            ));
        }
        if matches!(
            manifest.side_effect,
            SideEffect::Process | SideEffect::Network
        ) && grant.quality == EnforcementQuality::InProcessFence
        {
            return Err(Failure::new(
                ErrorCode::SandboxUnavailable,
                FailureClass::Policy,
                "tool.sandbox",
                "requested effect requires an OS security boundary",
            ));
        }
        let output = tool.execute(
            arguments,
            &ToolContext {
                actor,
                level,
                execution: self.execution,
                cancel,
            },
        )?;
        let canonical_len = output.value.to_canonical_json().len();
        let retained_len = output.model_content.len().saturating_add(canonical_len);
        if output.model_content.len() > manifest.max_output_bytes
            || canonical_len > manifest.max_output_bytes
        {
            return Err(Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Resource,
                "tool.output",
                "tool canonical value or presentation exceeds manifest limit",
            ));
        }
        budget.account_output(retained_len)?;
        self.verifier
            .verify(id, &Value::Object(arguments.clone()), &output.value)?;
        Ok(output)
    }

    fn require_confirmation(
        &self,
        manifest: &ToolManifest,
        invocation_id: &str,
        actor: &str,
        capability: Option<Capability>,
        cancel: &CancellationToken,
        audit: &mut dyn FnMut(ToolAuditEvent) -> HarnessResult<()>,
    ) -> HarnessResult<()> {
        cancel.check("tool.confirm")?;
        // When the prompt is for a specific capability, the action and summary
        // name it so the approver knows exactly which authority is being
        // granted — a tool-level string alone would hide which capability the
        // approval covers.
        let (action, summary) = match capability {
            Some(cap) => (
                format!("{}#{}", manifest.id, cap.as_str()),
                format!("{} (capability: {})", manifest.description, cap.as_str()),
            ),
            None => (manifest.id.clone(), manifest.description.clone()),
        };
        let request = ConfirmationRequest {
            request_id: format!("confirm-{invocation_id}"),
            actor: actor.to_owned(),
            action,
            risk: format!("{:?}", manifest.side_effect).to_ascii_lowercase(),
            summary,
        };
        audit(ToolAuditEvent::ConfirmationAsked(request.clone()))?;
        let outcome = match self.confirmation.confirm(&request) {
            Ok(outcome) => outcome,
            Err(failure) => {
                audit(ToolAuditEvent::ConfirmationDecided {
                    request_id: request.request_id,
                    outcome: ConfirmationOutcome::Unavailable,
                })?;
                return Err(failure);
            }
        };
        audit(ToolAuditEvent::ConfirmationDecided {
            request_id: request.request_id,
            outcome,
        })?;
        cancel.check("tool.confirm")?;
        match outcome {
            ConfirmationOutcome::AllowedOnce => Ok(()),
            ConfirmationOutcome::Denied => Err(Failure::new(
                ErrorCode::PermissionDenied,
                FailureClass::Policy,
                "tool.confirm",
                "user denied tool execution",
            )),
            ConfirmationOutcome::Unavailable => Err(Failure::new(
                ErrorCode::ConfirmationRequired,
                FailureClass::Policy,
                "tool.confirm",
                "confirmation provider is unavailable",
            )),
        }
    }
}

/// Read one bounded UTF-8 file.
pub struct ReadFileTool {
    manifest: ToolManifest,
}

impl Default for ReadFileTool {
    fn default() -> Self {
        Self {
            manifest: manifest(
                "fs.read",
                "Read one UTF-8 file inside the configured root",
                CapabilitySet::from_slice(&[Capability::FileRead]),
                vec![ExecutionLevel::L1, ExecutionLevel::L2, ExecutionLevel::L3],
                SideEffect::Read,
                ConfirmationMode::Never,
            ),
        }
    }
}

impl Tool for ReadFileTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        require_exact_string(arguments, &["path"])
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        let path = argument_string(arguments, "path")?;
        let text = context.execution.read_text(Path::new(path))?;
        Ok(ToolOutput {
            value: Value::String(text.clone()),
            model_content: text,
            presentation: BTreeMap::from([("kind".to_owned(), "file".to_owned())]),
        })
    }
}

/// Stable one-directory listing.
pub struct ListFilesTool {
    manifest: ToolManifest,
}

impl Default for ListFilesTool {
    fn default() -> Self {
        Self {
            manifest: manifest(
                "fs.list",
                "List one directory inside the configured root",
                CapabilitySet::from_slice(&[Capability::FileRead]),
                vec![ExecutionLevel::L1, ExecutionLevel::L2, ExecutionLevel::L3],
                SideEffect::Read,
                ConfirmationMode::Never,
            ),
        }
    }
}

impl Tool for ListFilesTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        require_exact_string(arguments, &["path"])
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        let path = argument_string(arguments, "path")?;
        let entries = context.execution.list(Path::new(path))?;
        Ok(ToolOutput {
            value: Value::Array(entries.iter().cloned().map(Value::String).collect()),
            model_content: entries.join("\n"),
            presentation: BTreeMap::from([("kind".to_owned(), "directory".to_owned())]),
        })
    }
}

/// Atomic bounded UTF-8 write.
pub struct WriteFileTool {
    manifest: ToolManifest,
}

impl Default for WriteFileTool {
    fn default() -> Self {
        Self {
            manifest: manifest(
                "fs.write",
                "Atomically write one UTF-8 file inside the configured root",
                CapabilitySet::from_slice(&[Capability::FileWrite]),
                vec![ExecutionLevel::L1, ExecutionLevel::L2, ExecutionLevel::L3],
                SideEffect::Write,
                ConfirmationMode::OnSideEffect,
            ),
        }
    }
}

impl Tool for WriteFileTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        require_exact_string(arguments, &["path", "content"])
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        let path = argument_string(arguments, "path")?;
        let content = argument_string(arguments, "content")?;
        context
            .execution
            .write_text_atomic(Path::new(path), content)?;
        let value = Value::Object(BTreeMap::from([
            ("path".to_owned(), Value::String(path.to_owned())),
            (
                "bytes".to_owned(),
                Value::Integer(i64::try_from(content.len()).unwrap_or(i64::MAX)),
            ),
        ]));
        Ok(ToolOutput {
            model_content: format!("wrote {} bytes to {path}", content.len()),
            value,
            presentation: BTreeMap::from([("kind".to_owned(), "file-write".to_owned())]),
        })
    }
}

/// Allowlisted direct-argv subprocess tool. Never invokes a shell.
pub struct RunProcessTool {
    manifest: ToolManifest,
}

impl Default for RunProcessTool {
    fn default() -> Self {
        Self {
            manifest: manifest(
                "process.run",
                "Run one allowlisted program with direct argv in the configured root",
                CapabilitySet::from_slice(&[Capability::ProcessSpawn]),
                vec![ExecutionLevel::L1, ExecutionLevel::L2, ExecutionLevel::L3],
                SideEffect::Process,
                ConfirmationMode::Always,
            ),
        }
    }
}

impl Tool for RunProcessTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    fn validate_arguments(&self, arguments: &ToolArguments) -> HarnessResult<()> {
        if arguments.len() != 2 {
            return Err(Failure::invalid(
                "tool.arguments",
                "expected only program and args",
            ));
        }
        let _program = argument_string(arguments, "program")?;
        let Some(Value::Array(args)) = arguments.get("args") else {
            return Err(Failure::invalid("tool.arguments", "args must be an array"));
        };
        if args.len() > 256 || args.iter().any(|value| value.as_str().is_none()) {
            return Err(Failure::invalid(
                "tool.arguments",
                "args must be a bounded string array",
            ));
        }
        Ok(())
    }

    fn execute(
        &self,
        arguments: &ToolArguments,
        context: &ToolContext<'_>,
    ) -> HarnessResult<ToolOutput> {
        let program = argument_string(arguments, "program")?;
        let args = match arguments.get("args") {
            Some(Value::Array(values)) => values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        };
        // Plumb the manifest's output cap into the process spec so the pipe
        // reader cap and the post-hoc output check (in ToolDispatch) enforce
        // the same limit — instead of relying on two independent constants
        // agreeing.
        let spec =
            ProcessSpec::new(program, args).with_max_output_bytes(self.manifest.max_output_bytes);
        let output = context.execution.run_process(&spec, context.cancel)?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let value = Value::Object(BTreeMap::from([
            (
                "status".to_owned(),
                output
                    .status
                    .map_or(Value::Null, |status| Value::Integer(i64::from(status))),
            ),
            ("stdout".to_owned(), Value::String(stdout.clone())),
            ("stderr".to_owned(), Value::String(stderr.clone())),
            ("truncated".to_owned(), Value::Bool(output.truncated)),
        ]));
        Ok(ToolOutput {
            model_content: format!(
                "status={:?}\nstdout:\n{}\nstderr:\n{}",
                output.status, stdout, stderr
            ),
            value,
            presentation: BTreeMap::from([("kind".to_owned(), "process".to_owned())]),
        })
    }
}

/// Registers all local built-ins.
pub fn register_builtin_tools(registry: &mut ToolRegistry) -> HarnessResult<()> {
    registry.register(Arc::new(ReadFileTool::default()))?;
    registry.register(Arc::new(ListFilesTool::default()))?;
    registry.register(Arc::new(WriteFileTool::default()))?;
    registry.register(Arc::new(RunProcessTool::default()))?;
    Ok(())
}

fn manifest(
    id: &str,
    description: &str,
    required_capabilities: CapabilitySet,
    supported_levels: Vec<ExecutionLevel>,
    side_effect: SideEffect,
    confirmation: ConfirmationMode,
) -> ToolManifest {
    let (input_schema, output_schema) = schemas(id);
    ToolManifest {
        id: id.to_owned(),
        version: "1.0.0".to_owned(),
        description: description.to_owned(),
        input_schema: input_schema.to_owned(),
        output_schema: output_schema.to_owned(),
        required_capabilities,
        supported_levels,
        determinism: if matches!(side_effect, SideEffect::None | SideEffect::Read) {
            Determinism::Deterministic
        } else {
            Determinism::NonIdempotent
        },
        side_effect,
        confirmation,
        concurrency_safe: matches!(side_effect, SideEffect::None | SideEffect::Read),
        default_timeout: Duration::from_secs(10),
        max_output_bytes: 256 * 1024,
        verification: "provider-required".to_owned(),
        compensation: "none".to_owned(),
    }
}

fn schemas(id: &str) -> (&'static str, &'static str) {
    match id {
        "fs.read" => (
            r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,
            r#"{"type":"string"}"#,
        ),
        "fs.list" => (
            r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,
            r#"{"type":"array","items":{"type":"string"}}"#,
        ),
        "fs.write" => (
            r#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}"#,
            r#"{"type":"object","properties":{"path":{"type":"string"},"bytes":{"type":"integer"}},"required":["path","bytes"],"additionalProperties":false}"#,
        ),
        "process.run" => (
            r#"{"type":"object","properties":{"program":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},"required":["program","args"],"additionalProperties":false}"#,
            r#"{"type":"object","properties":{"status":{"type":["integer","null"]},"stdout":{"type":"string"},"stderr":{"type":"string"},"truncated":{"type":"boolean"}},"required":["status","stdout","stderr","truncated"],"additionalProperties":false}"#,
        ),
        _ => (r#"{"type":"object","additionalProperties":false}"#, r#"{}"#),
    }
}

fn valid_tool_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

fn require_exact_string(arguments: &ToolArguments, names: &[&str]) -> HarnessResult<()> {
    if arguments.len() != names.len() {
        return Err(Failure::invalid(
            "tool.arguments",
            "unexpected or missing tool arguments",
        ));
    }
    for name in names {
        let _value = argument_string(arguments, name)?;
    }
    Ok(())
}

fn argument_string<'a>(arguments: &'a ToolArguments, name: &str) -> HarnessResult<&'a str> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Failure::invalid("tool.arguments", format!("{name} must be a string")))
}

fn capability_from_name(name: &str) -> Option<Capability> {
    match name {
        "model" => Some(Capability::Model),
        "file.read" => Some(Capability::FileRead),
        "file.write" => Some(Capability::FileWrite),
        "process.spawn" => Some(Capability::ProcessSpawn),
        "network" => Some(Capability::Network),
        "credential" => Some(Capability::Credential),
        "workspace" => Some(Capability::Workspace),
        "job" => Some(Capability::Job),
        "subagent" => Some(Capability::Subagent),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn exposure_is_capability_scoped() -> HarnessResult<()> {
        let mut registry = ToolRegistry::new();
        register_builtin_tools(&mut registry)?;
        let read = CapabilitySet::from_slice(&[Capability::FileRead]);
        let visible = registry.visible(ExecutionLevel::L1, &read);
        assert_eq!(visible.len(), 2);
        assert!(
            visible
                .iter()
                .all(|manifest| manifest.id.starts_with("fs."))
        );
        Ok(())
    }

    #[test]
    fn builtin_manifest_schemas_are_valid_and_specific() -> HarnessResult<()> {
        let mut registry = ToolRegistry::new();
        register_builtin_tools(&mut registry)?;
        for manifest in registry.visible(ExecutionLevel::L3, &CapabilitySet::all_local()) {
            Value::parse_json(&manifest.input_schema).map_err(|message| {
                Failure::invalid("test.schema", format!("{} input: {message}", manifest.id))
            })?;
            Value::parse_json(&manifest.output_schema).map_err(|message| {
                Failure::invalid("test.schema", format!("{} output: {message}", manifest.id))
            })?;
            assert!(manifest.input_schema.contains("properties"));
        }
        Ok(())
    }

    #[test]
    fn invalid_or_oversized_manifests_are_rejected() {
        let mut invalid = manifest(
            "test.invalid",
            "invalid schema",
            CapabilitySet::new(),
            vec![ExecutionLevel::L1],
            SideEffect::None,
            ConfirmationMode::Never,
        );
        invalid.input_schema = "{".to_owned();
        assert!(invalid.validate().is_err());
        invalid.input_schema = r#"{"type":"object"}"#.to_owned();
        invalid.description = "x".repeat(4097);
        assert!(invalid.validate().is_err());
    }

    // --- Multi-capability approval: each Ask capability gets its own prompt ---

    /// A tool requiring TWO capabilities, used to prove the dispatcher asks for
    /// each Ask capability separately rather than letting one approval cover all.
    struct TwoCapTool {
        manifest: ToolManifest,
    }

    impl TwoCapTool {
        fn new() -> Self {
            Self {
                manifest: manifest(
                    "test.two-cap",
                    "two capability tool",
                    CapabilitySet::from_slice(&[Capability::FileWrite, Capability::ProcessSpawn]),
                    vec![ExecutionLevel::L1, ExecutionLevel::L2, ExecutionLevel::L3],
                    SideEffect::None,
                    ConfirmationMode::Never,
                ),
            }
        }
    }

    impl Tool for TwoCapTool {
        fn manifest(&self) -> &ToolManifest {
            &self.manifest
        }
        fn validate_arguments(&self, _arguments: &ToolArguments) -> HarnessResult<()> {
            Ok(())
        }
        fn execute(
            &self,
            _arguments: &ToolArguments,
            _context: &ToolContext<'_>,
        ) -> HarnessResult<ToolOutput> {
            Ok(ToolOutput {
                value: Value::Null,
                model_content: String::new(),
                presentation: BTreeMap::new(),
            })
        }
    }

    /// Permission provider that answers Ask for every capability.
    struct AskAllPermission;
    impl PermissionProvider for AskAllPermission {
        fn authorize(
            &self,
            _actor: &str,
            _capability: Capability,
            _resource: &str,
        ) -> HarnessResult<PermissionDecision> {
            Ok(PermissionDecision::Ask)
        }
    }

    /// Confirmation provider that counts how many prompts it received.
    struct CountingConfirmation {
        asked: Mutex<usize>,
    }
    impl ConfirmationProvider for CountingConfirmation {
        fn confirm(&self, _request: &ConfirmationRequest) -> HarnessResult<ConfirmationOutcome> {
            if let Ok(mut n) = self.asked.lock() {
                *n += 1;
            }
            Ok(ConfirmationOutcome::AllowedOnce)
        }
    }

    /// Sandbox provider that grants the requested capabilities in the same world
    /// (test-only stand-in for a full OS boundary). SideEffect::None tools do not
    /// require a security boundary, so this is sufficient for this dispatch.
    struct GrantAllSandbox {
        world_id: String,
    }
    impl SandboxProvider for GrantAllSandbox {
        fn resolve(
            &self,
            request: &SandboxRequest,
        ) -> HarnessResult<crate::providers::SandboxGrant> {
            Ok(crate::providers::SandboxGrant {
                world_id: self.world_id.clone(),
                backend: "test-grant-all".to_owned(),
                quality: crate::providers::EnforcementQuality::Full,
                granted: request.capabilities.clone(),
            })
        }
    }

    #[test]
    fn multi_capability_tool_prompts_per_capability() -> HarnessResult<()> {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TwoCapTool::new()))?;

        let permission = AskAllPermission;
        let confirmation = CountingConfirmation {
            asked: Mutex::new(0),
        };
        let verifier = crate::providers::CanonicalVerificationProvider;
        let root_fs = crate::execution::RootedFs::new(".")?;
        let execution = crate::execution::LocalExecutionBroker::new(root_fs, Vec::new());

        let sandbox = GrantAllSandbox {
            world_id: execution.world_id().to_owned(),
        };
        let dispatch = ToolDispatch {
            registry: &registry,
            permission: &permission,
            confirmation: &confirmation,
            verifier: &verifier,
            sandbox: &sandbox,
            execution: &execution,
        };

        let capabilities =
            CapabilitySet::from_slice(&[Capability::FileWrite, Capability::ProcessSpawn]);
        let mut budget =
            crate::budget::Budget::new(crate::budget::BudgetLimits::for_level(ExecutionLevel::L3));
        let cancel = crate::cancel::CancellationToken::new();
        let arguments = ToolArguments::new();
        let mut audit = |_event: ToolAuditEvent| -> HarnessResult<()> { Ok(()) };

        // SideEffect::None with ConfirmationMode::Never means the ONLY prompts are
        // the per-capability Ask approvals. Two Ask capabilities -> two prompts.
        let _output = dispatch.execute(
            "test.two-cap",
            "inv-1",
            &arguments,
            "actor",
            ExecutionLevel::L1,
            &capabilities,
            &mut budget,
            &cancel,
            &mut audit,
        )?;
        let asked = confirmation.asked.lock().map(|n| *n).unwrap_or(0);
        assert_eq!(
            asked, 2,
            "two Ask capabilities must produce two approval prompts, not one"
        );
        Ok(())
    }
}
