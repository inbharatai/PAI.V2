//! Model-neutral providers and cross-cutting capability contracts.

use crate::cancel::CancellationToken;
use crate::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use crate::routing::ExecutionLevel;
use crate::value::Value;
#[cfg(any(test, feature = "test-providers"))]
use std::collections::VecDeque;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, Mutex};

/// Atomic authority dimensions understood by the trusted core.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(u8)]
pub enum Capability {
    Model = 0,
    FileRead = 1,
    FileWrite = 2,
    ProcessSpawn = 3,
    Network = 4,
    Credential = 5,
    Workspace = 6,
    Job = 7,
    Subagent = 8,
}

impl Capability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::FileRead => "file.read",
            Self::FileWrite => "file.write",
            Self::ProcessSpawn => "process.spawn",
            Self::Network => "network",
            Self::Credential => "credential",
            Self::Workspace => "workspace",
            Self::Job => "job",
            Self::Subagent => "subagent",
        }
    }
}

/// Compact deterministic capability set.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct CapabilitySet {
    bits: u64,
}

impl Debug for CapabilitySet {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.debug_list().entries(self.names()).finish()
    }
}

impl CapabilitySet {
    #[must_use]
    pub const fn new() -> Self {
        Self { bits: 0 }
    }

    #[must_use]
    pub fn all_local() -> Self {
        let mut set = Self::new();
        for capability in ALL_CAPABILITIES {
            set.insert(capability);
        }
        set
    }

    pub fn insert(&mut self, capability: Capability) {
        self.bits |= 1_u64 << (capability as u8);
    }

    pub fn remove(&mut self, capability: Capability) {
        self.bits &= !(1_u64 << (capability as u8));
    }

    #[must_use]
    pub const fn contains(&self, capability: Capability) -> bool {
        (self.bits & (1_u64 << (capability as u8))) != 0
    }

    #[must_use]
    pub const fn is_subset_of(&self, other: &Self) -> bool {
        self.bits & !other.bits == 0
    }

    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        ALL_CAPABILITIES
            .iter()
            .filter(|capability| self.contains(**capability))
            .map(|capability| capability.as_str())
            .collect()
    }

    #[must_use]
    pub fn from_slice(capabilities: &[Capability]) -> Self {
        let mut set = Self::new();
        for capability in capabilities {
            set.insert(*capability);
        }
        set
    }
}

const ALL_CAPABILITIES: [Capability; 9] = [
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

/// Secret reference. The secret value is never stored here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialRef {
    pub provider: String,
    pub key: String,
    pub scope: String,
}

impl CredentialRef {
    pub fn new(provider: &str, key: &str, scope: &str) -> HarnessResult<Self> {
        if !valid_identifier(provider) || !valid_identifier(key) || !valid_identifier(scope) {
            return Err(Failure::invalid(
                "credential_ref.new",
                "credential references use [A-Za-z0-9._-] and must be non-empty",
            ));
        }
        Ok(Self {
            provider: provider.to_owned(),
            key: key.to_owned(),
            scope: scope.to_owned(),
        })
    }

    #[must_use]
    pub fn redacted(&self) -> String {
        format!("{}://{}/***", self.provider, self.scope)
    }
}

/// Metadata only; attachment bytes live in a separate store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachmentMetadata {
    pub id: String,
    pub media_type: String,
    pub byte_len: u64,
    pub digest: String,
    pub display_name: Option<String>,
}

impl AttachmentMetadata {
    pub fn validate(&self) -> HarnessResult<()> {
        if !valid_identifier(&self.id) || self.byte_len > 64 * 1024 * 1024 {
            return Err(Failure::invalid(
                "attachment.validate",
                "attachment id or size is invalid",
            ));
        }
        if self.media_type.is_empty()
            || self.media_type.len() > 128
            || self.digest.is_empty()
            || self.digest.len() > 256
            || self
                .display_name
                .as_ref()
                .is_some_and(|name| name.is_empty() || name.len() > 255)
        {
            return Err(Failure::invalid(
                "attachment.validate",
                "attachment metadata is invalid",
            ));
        }
        Ok(())
    }
}

/// One immutable model request snapshot.
#[derive(Clone, Debug)]
pub struct ModelRequest {
    pub request_id: String,
    pub provider: String,
    pub model: String,
    pub system: String,
    pub messages: Vec<ModelMessage>,
    pub tools: Vec<ModelTool>,
    pub attachments: Vec<AttachmentMetadata>,
    pub max_output_bytes: usize,
}

/// Provider-neutral model message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelMessage {
    pub role: ModelRole,
    pub content: String,
}

/// Provider-neutral role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Dynamic tool exposure snapshot sent to a model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelTool {
    pub id: String,
    pub description: String,
    pub input_schema: String,
}

/// Indexed stream fragments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelChunk {
    Start {
        block: u32,
    },
    TextDelta {
        block: u32,
        text: String,
    },
    ReasoningDelta {
        block: u32,
        text: String,
    },
    ToolCall {
        block: u32,
        call_id: String,
        tool_id: String,
        arguments: String,
    },
    End {
        block: u32,
    },
    Usage {
        input_units: u64,
        output_units: u64,
    },
    Finish {
        reason: FinishReason,
    },
}

/// Terminal stream reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    Cancelled,
    Error,
}

/// Assembled provider result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelResponse {
    pub text: String,
    pub finish: FinishReason,
    pub input_units: u64,
    pub output_units: u64,
    pub provider_request_id: Option<String>,
}

/// Synchronous streaming seam usable from native and FFI callers without an async runtime.
pub trait ModelProvider: Send + Sync {
    fn id(&self) -> &str;
    fn models(&self) -> Vec<String>;
    fn stream(
        &self,
        request: &ModelRequest,
        cancel: &CancellationToken,
        sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
    ) -> HarnessResult<ModelResponse>;
}

/// Provider registry with exact-id replacement protection.
#[derive(Default)]
pub struct ModelRegistry {
    providers: BTreeMap<String, Arc<dyn ModelProvider>>,
}

impl std::fmt::Debug for ModelRegistry {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModelRegistry")
            .field("provider_ids", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ModelRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Arc<dyn ModelProvider>) -> HarnessResult<()> {
        let id = provider.id().to_owned();
        let models = provider.models();
        let unique_models: BTreeSet<&str> = models.iter().map(String::as_str).collect();
        if !valid_identifier(&id)
            || self.providers.contains_key(&id)
            || models.is_empty()
            || models.len() > 10_000
            || unique_models.len() != models.len()
            || models.iter().any(|model| !valid_model_id(model))
        {
            return Err(Failure::new(
                ErrorCode::Conflict,
                FailureClass::Internal,
                "model.register",
                "provider id or advertised model catalogue is invalid or duplicated",
            ));
        }
        self.providers.insert(id, provider);
        Ok(())
    }

    /// Captures provider registration and request configuration for exactly one dispatch.
    pub fn prepare(&self, request: ModelRequest) -> HarnessResult<PreparedModelCall> {
        if !valid_identifier(&request.provider)
            || !valid_model_id(&request.model)
            || !valid_identifier(&request.request_id)
            || request.system.len() > 256 * 1024
            || request.messages.len() > 1_024
            || request
                .messages
                .iter()
                .any(|message| message.content.len() > 2 * 1024 * 1024)
            || request.tools.len() > 256
            || request.attachments.len() > 64
            || request.max_output_bytes == 0
            || request.max_output_bytes > 8 * 1024 * 1024
        {
            return Err(Failure::invalid(
                "model.prepare",
                "model request identifiers, collection sizes, or byte limits are invalid",
            ));
        }
        let provider = self
            .providers
            .get(&request.provider)
            .cloned()
            .ok_or_else(|| {
                Failure::new(
                    ErrorCode::CapabilityUnavailable,
                    FailureClass::Provider,
                    "model.prepare",
                    "model provider is not registered",
                )
            })?;
        if !provider
            .models()
            .iter()
            .any(|model| model == &request.model)
        {
            return Err(Failure::new(
                ErrorCode::CapabilityUnavailable,
                FailureClass::Provider,
                "model.prepare",
                "requested model is not advertised by the selected provider",
            )
            .with_detail("provider", &request.provider)
            .with_detail("model", &request.model));
        }
        Ok(PreparedModelCall {
            provider,
            request,
            dispatched: false,
        })
    }

    #[must_use]
    pub fn provider_ids(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }
}

/// Registration-bound one-shot model call.
pub struct PreparedModelCall {
    provider: Arc<dyn ModelProvider>,
    request: ModelRequest,
    dispatched: bool,
}

impl std::fmt::Debug for PreparedModelCall {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedModelCall")
            .field("provider", &self.request.provider)
            .field("model", &self.request.model)
            .field("request_id", &self.request.request_id)
            .field("dispatched", &self.dispatched)
            .finish()
    }
}

impl PreparedModelCall {
    #[must_use]
    pub fn request(&self) -> &ModelRequest {
        &self.request
    }

    pub fn stream(
        &mut self,
        cancel: &CancellationToken,
        sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
    ) -> HarnessResult<ModelResponse> {
        if self.dispatched {
            return Err(Failure::new(
                ErrorCode::Conflict,
                FailureClass::Internal,
                "model.dispatch",
                "prepared model call is one-shot",
            ));
        }
        self.dispatched = true;
        self.provider.stream(&self.request, cancel, sink)
    }
}

#[cfg(any(test, feature = "test-providers"))]
/// Deterministic offline echo provider.
#[derive(Clone, Debug)]
pub struct EchoModelProvider {
    id: String,
    chunk_chars: usize,
}

#[cfg(any(test, feature = "test-providers"))]
impl Default for EchoModelProvider {
    fn default() -> Self {
        Self {
            id: "echo".to_owned(),
            chunk_chars: 16,
        }
    }
}

#[cfg(any(test, feature = "test-providers"))]
impl EchoModelProvider {
    #[must_use]
    pub fn with_chunk_chars(chunk_chars: usize) -> Self {
        Self {
            id: "echo".to_owned(),
            chunk_chars: chunk_chars.max(1),
        }
    }
}

#[cfg(any(test, feature = "test-providers"))]
impl ModelProvider for EchoModelProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn models(&self) -> Vec<String> {
        vec!["echo-v1".to_owned()]
    }

    fn stream(
        &self,
        request: &ModelRequest,
        cancel: &CancellationToken,
        sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
    ) -> HarnessResult<ModelResponse> {
        let source = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == ModelRole::User)
            .map(|message| message.content.as_str())
            .unwrap_or_default();
        let text = format!("echo: {source}");
        if text.len() > request.max_output_bytes {
            return Err(Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Resource,
                "model.echo",
                "echo output exceeds request limit",
            ));
        }
        sink(ModelChunk::Start { block: 0 })?;
        for piece in char_chunks(&text, self.chunk_chars) {
            cancel.check("model.echo")?;
            sink(ModelChunk::TextDelta {
                block: 0,
                text: piece,
            })?;
        }
        sink(ModelChunk::End { block: 0 })?;
        let input_units = u64::try_from(source.split_whitespace().count()).unwrap_or(u64::MAX);
        let output_units = u64::try_from(text.split_whitespace().count()).unwrap_or(u64::MAX);
        sink(ModelChunk::Usage {
            input_units,
            output_units,
        })?;
        sink(ModelChunk::Finish {
            reason: FinishReason::Stop,
        })?;
        Ok(ModelResponse {
            text,
            finish: FinishReason::Stop,
            input_units,
            output_units,
            provider_request_id: None,
        })
    }
}

#[cfg(any(test, feature = "test-providers"))]
/// Script entry for the deterministic mock provider.
#[derive(Clone, Debug)]
pub enum MockStep {
    Text(String),
    ToolCall {
        call_id: String,
        tool_id: String,
        arguments: String,
    },
    RetryableFailure(String),
    FatalFailure(String),
    WaitForCancellation,
}

#[cfg(any(test, feature = "test-providers"))]
/// Thread-safe scripted provider used by tests and replay.
#[derive(Clone, Debug)]
pub struct MockModelProvider {
    id: String,
    script: Arc<Mutex<VecDeque<MockStep>>>,
}

#[cfg(any(test, feature = "test-providers"))]
impl MockModelProvider {
    #[must_use]
    pub fn new(script: impl IntoIterator<Item = MockStep>) -> Self {
        Self {
            id: "mock".to_owned(),
            script: Arc::new(Mutex::new(script.into_iter().collect())),
        }
    }

    fn pop_step(&self) -> HarnessResult<MockStep> {
        let mut guard = self.script.lock().map_err(|_| {
            Failure::new(
                ErrorCode::Internal,
                FailureClass::Internal,
                "model.mock",
                "mock script lock poisoned",
            )
        })?;
        guard.pop_front().ok_or_else(|| {
            Failure::new(
                ErrorCode::ProviderFailed,
                FailureClass::Provider,
                "model.mock",
                "mock script exhausted",
            )
        })
    }
}

#[cfg(any(test, feature = "test-providers"))]
impl ModelProvider for MockModelProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn models(&self) -> Vec<String> {
        vec!["mock-v1".to_owned()]
    }

    fn stream(
        &self,
        _request: &ModelRequest,
        cancel: &CancellationToken,
        sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
    ) -> HarnessResult<ModelResponse> {
        cancel.check("model.mock")?;
        match self.pop_step()? {
            MockStep::Text(text) => {
                sink(ModelChunk::Start { block: 0 })?;
                sink(ModelChunk::TextDelta {
                    block: 0,
                    text: text.clone(),
                })?;
                sink(ModelChunk::End { block: 0 })?;
                sink(ModelChunk::Finish {
                    reason: FinishReason::Stop,
                })?;
                Ok(ModelResponse {
                    output_units: u64::try_from(text.split_whitespace().count())
                        .unwrap_or(u64::MAX),
                    text,
                    finish: FinishReason::Stop,
                    input_units: 0,
                    provider_request_id: Some("mock-request".to_owned()),
                })
            }
            MockStep::ToolCall {
                call_id,
                tool_id,
                arguments,
            } => {
                sink(ModelChunk::ToolCall {
                    block: 0,
                    call_id,
                    tool_id,
                    arguments,
                })?;
                sink(ModelChunk::Finish {
                    reason: FinishReason::ToolCalls,
                })?;
                Ok(ModelResponse {
                    text: String::new(),
                    finish: FinishReason::ToolCalls,
                    input_units: 0,
                    output_units: 0,
                    provider_request_id: Some("mock-request".to_owned()),
                })
            }
            MockStep::RetryableFailure(message) => Err(Failure::new(
                ErrorCode::ProviderFailed,
                FailureClass::Provider,
                "model.mock",
                message,
            )
            .retryable(Some(1))),
            MockStep::FatalFailure(message) => Err(Failure::new(
                ErrorCode::ProviderFailed,
                FailureClass::Provider,
                "model.mock",
                message,
            )),
            MockStep::WaitForCancellation => loop {
                if let Some(cause) = cancel.wait_cancelled(std::time::Duration::from_millis(20)) {
                    break Err(Failure::cancelled("model.mock", cause.as_str()));
                }
            },
        }
    }
}

/// One explicit request-id-bound deterministic replay entry.
#[derive(Clone, Debug)]
pub struct ReplayEntry {
    pub request_id: String,
    pub chunks: Vec<ModelChunk>,
    pub response: ModelResponse,
}

/// Diagnostic model provider keyed by recorded request identity, never call order.
#[derive(Clone, Debug)]
pub struct ReplayModelProvider {
    id: String,
    entries: Arc<BTreeMap<String, ReplayEntry>>,
}

impl ReplayModelProvider {
    pub fn new(entries: impl IntoIterator<Item = ReplayEntry>) -> HarnessResult<Self> {
        let mut indexed = BTreeMap::new();
        for entry in entries {
            if entry.request_id.is_empty()
                || indexed.insert(entry.request_id.clone(), entry).is_some()
            {
                return Err(Failure::new(
                    ErrorCode::Conflict,
                    FailureClass::User,
                    "model.replay",
                    "replay request ids must be unique and non-empty",
                ));
            }
        }
        Ok(Self {
            id: "replay".to_owned(),
            entries: Arc::new(indexed),
        })
    }
}

impl ModelProvider for ReplayModelProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn models(&self) -> Vec<String> {
        vec!["recorded".to_owned()]
    }

    fn stream(
        &self,
        request: &ModelRequest,
        cancel: &CancellationToken,
        sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
    ) -> HarnessResult<ModelResponse> {
        let entry = self.entries.get(&request.request_id).ok_or_else(|| {
            Failure::new(
                ErrorCode::NotFound,
                FailureClass::Provider,
                "model.replay",
                "no replay entry matches the request id",
            )
            .with_detail("request_id", &request.request_id)
        })?;
        for chunk in &entry.chunks {
            cancel.check("model.replay")?;
            sink(chunk.clone())?;
        }
        Ok(entry.response.clone())
    }
}

/// Named memory scopes requested by the harness. `None` is a deliberate no-memory mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum MemoryScope {
    None,
    Conversation,
    Preferences,
    Relevant,
    Project,
    Document,
    Extended,
}

impl MemoryScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Conversation => "conversation",
            Self::Preferences => "preferences",
            Self::Relevant => "relevant",
            Self::Project => "project",
            Self::Document => "document",
            Self::Extended => "extended",
        }
    }
}

/// Provider-advertised memory operations and limits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryCapabilities {
    pub scopes: Vec<MemoryScope>,
    pub can_retrieve: bool,
    pub can_search: bool,
    pub can_store: bool,
    pub can_update: bool,
    pub can_delete: bool,
    pub max_results: usize,
}

/// Provider-neutral memory record. Secret material must never be stored in this structure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryRecord {
    pub id: String,
    pub scope: MemoryScope,
    pub namespace: String,
    pub content: String,
    pub attributes: BTreeMap<String, String>,
}

impl MemoryRecord {
    pub fn validate(&self) -> HarnessResult<()> {
        if self.scope == MemoryScope::None {
            return Err(Failure::invalid(
                "memory.record",
                "the none scope cannot contain records",
            ));
        }
        if !valid_identifier(&self.id) {
            return Err(Failure::invalid(
                "memory.record",
                "memory id must be a bounded portable identifier",
            ));
        }
        if self.namespace.is_empty()
            || self.namespace.len() > 256
            || self.namespace.contains('\0')
            || self.content.len() > 1024 * 1024
            || self.content.contains('\0')
            || self.attributes.len() > 64
            || self.attributes.iter().any(|(key, value)| {
                !valid_identifier(key) || value.len() > 4096 || value.contains('\0')
            })
        {
            return Err(Failure::invalid(
                "memory.record",
                "memory record exceeds namespace, content, or attribute bounds",
            ));
        }
        Ok(())
    }

    fn key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}",
            self.scope.as_str(),
            self.namespace,
            self.id
        )
    }
}

/// Bounded search request. Providers may apply stronger privacy and ranking policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryQuery {
    pub scope: MemoryScope,
    pub namespace: Option<String>,
    pub text: String,
    pub limit: usize,
}

impl MemoryQuery {
    pub fn validate(&self) -> HarnessResult<()> {
        if self.scope == MemoryScope::None
            || self.text.len() > 64 * 1024
            || self.text.contains('\0')
            || self.limit == 0
            || self.limit > 10_000
            || self.namespace.as_ref().is_some_and(|namespace| {
                namespace.is_empty() || namespace.len() > 256 || namespace.contains('\0')
            })
        {
            return Err(Failure::invalid(
                "memory.query",
                "memory query has an invalid scope, namespace, text, or limit",
            ));
        }
        Ok(())
    }
}

/// Full provider seam. Implementations own storage, retention, redaction, and access policy.
pub trait MemoryProvider: Send + Sync {
    fn capabilities(&self) -> MemoryCapabilities;
    fn retrieve(
        &self,
        scope: MemoryScope,
        namespace: &str,
        id: &str,
    ) -> HarnessResult<Option<MemoryRecord>>;
    fn search(&self, query: &MemoryQuery) -> HarnessResult<Vec<MemoryRecord>>;
    fn store(&self, record: MemoryRecord) -> HarnessResult<()>;
    fn update(&self, record: MemoryRecord) -> HarnessResult<()>;
    fn delete(&self, scope: MemoryScope, namespace: &str, id: &str) -> HarnessResult<bool>;
}

/// Deterministic in-memory provider for standalone tests and embedding examples.
#[derive(Debug, Default)]
pub struct InMemoryMemoryProvider {
    records: Mutex<BTreeMap<String, MemoryRecord>>,
}

impl InMemoryMemoryProvider {
    fn key(scope: MemoryScope, namespace: &str, id: &str) -> String {
        format!("{}\u{1f}{namespace}\u{1f}{id}", scope.as_str())
    }

    fn validate_lookup(scope: MemoryScope, namespace: &str, id: &str) -> HarnessResult<()> {
        if scope == MemoryScope::None
            || namespace.is_empty()
            || namespace.len() > 256
            || namespace.contains('\0')
            || !valid_identifier(id)
        {
            return Err(Failure::invalid(
                "memory.lookup",
                "memory lookup has an invalid scope, namespace, or id",
            ));
        }
        Ok(())
    }

    fn poisoned(operation: &str) -> Failure {
        Failure::new(
            ErrorCode::Internal,
            FailureClass::Internal,
            operation,
            "memory provider lock is poisoned",
        )
    }
}

impl MemoryProvider for InMemoryMemoryProvider {
    fn capabilities(&self) -> MemoryCapabilities {
        MemoryCapabilities {
            scopes: vec![
                MemoryScope::Conversation,
                MemoryScope::Preferences,
                MemoryScope::Relevant,
                MemoryScope::Project,
                MemoryScope::Document,
                MemoryScope::Extended,
            ],
            can_retrieve: true,
            can_search: true,
            can_store: true,
            can_update: true,
            can_delete: true,
            max_results: 10_000,
        }
    }

    fn retrieve(
        &self,
        scope: MemoryScope,
        namespace: &str,
        id: &str,
    ) -> HarnessResult<Option<MemoryRecord>> {
        Self::validate_lookup(scope, namespace, id)?;
        let records = self
            .records
            .lock()
            .map_err(|_| Self::poisoned("memory.retrieve"))?;
        Ok(records.get(&Self::key(scope, namespace, id)).cloned())
    }

    fn search(&self, query: &MemoryQuery) -> HarnessResult<Vec<MemoryRecord>> {
        query.validate()?;
        let needle = query.text.to_lowercase();
        let records = self
            .records
            .lock()
            .map_err(|_| Self::poisoned("memory.search"))?;
        let mut matches = Vec::new();
        for record in records.values() {
            if record.scope != query.scope
                || query
                    .namespace
                    .as_ref()
                    .is_some_and(|namespace| namespace != &record.namespace)
            {
                continue;
            }
            let searchable = format!(
                "{} {} {} {}",
                record.id,
                record.namespace,
                record.content,
                record
                    .attributes
                    .iter()
                    .map(|(key, value)| format!("{key} {value}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .to_lowercase();
            if needle.is_empty() || searchable.contains(&needle) {
                matches.push(record.clone());
                if !needle.is_empty() && matches.len() >= query.limit {
                    break;
                }
            }
        }
        if needle.is_empty() && query.scope == MemoryScope::Conversation {
            matches.sort_by_key(|record| std::cmp::Reverse(memory_conversation_order_key(record)));
            matches.truncate(query.limit);
            matches.sort_by_key(memory_conversation_order_key);
        } else {
            matches.truncate(query.limit);
        }
        Ok(matches)
    }

    fn store(&self, record: MemoryRecord) -> HarnessResult<()> {
        record.validate()?;
        let key = record.key();
        let mut records = self
            .records
            .lock()
            .map_err(|_| Self::poisoned("memory.store"))?;
        if records.contains_key(&key) {
            return Err(Failure::new(
                ErrorCode::Conflict,
                FailureClass::User,
                "memory.store",
                "memory record already exists",
            ));
        }
        records.insert(key, record);
        Ok(())
    }

    fn update(&self, record: MemoryRecord) -> HarnessResult<()> {
        record.validate()?;
        let key = record.key();
        let mut records = self
            .records
            .lock()
            .map_err(|_| Self::poisoned("memory.update"))?;
        if !records.contains_key(&key) {
            return Err(Failure::new(
                ErrorCode::NotFound,
                FailureClass::User,
                "memory.update",
                "memory record does not exist",
            ));
        }
        records.insert(key, record);
        Ok(())
    }

    fn delete(&self, scope: MemoryScope, namespace: &str, id: &str) -> HarnessResult<bool> {
        Self::validate_lookup(scope, namespace, id)?;
        let mut records = self
            .records
            .lock()
            .map_err(|_| Self::poisoned("memory.delete"))?;
        Ok(records.remove(&Self::key(scope, namespace, id)).is_some())
    }
}

fn memory_conversation_order_key(record: &MemoryRecord) -> (u128, u32, String) {
    let timestamp = record
        .attributes
        .get("created_at_ms")
        .and_then(|value| value.parse::<u128>().ok())
        .unwrap_or(0);
    let role_order = record
        .attributes
        .get("role_order")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    (timestamp, role_order, record.id.clone())
}

/// Safety decision for untrusted input and planned effects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SafetyDecision {
    Allow,
    Narrow { reason: String },
    Deny { reason: String },
}

pub trait SafetyProvider: Send + Sync {
    fn assess(&self, input: &str, level: ExecutionLevel) -> HarnessResult<SafetyDecision>;
}

/// Authority check result. A provider may narrow but never widen core policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny { rule_id: String, reason: String },
}

pub trait PermissionProvider: Send + Sync {
    fn authorize(
        &self,
        actor: &str,
        capability: Capability,
        resource: &str,
    ) -> HarnessResult<PermissionDecision>;
}

/// Explicit human confirmation audit request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmationRequest {
    pub request_id: String,
    pub actor: String,
    pub action: String,
    pub risk: String,
    pub summary: String,
}

/// One-shot confirmation outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmationOutcome {
    AllowedOnce,
    Denied,
    Unavailable,
}

pub trait ConfirmationProvider: Send + Sync {
    fn confirm(&self, request: &ConfirmationRequest) -> HarnessResult<ConfirmationOutcome>;
}

/// Deterministic postcondition verifier.
pub trait VerificationProvider: Send + Sync {
    fn verify(&self, tool_id: &str, arguments: &Value, output: &Value) -> HarnessResult<()>;
}

/// Requested dimensions for an execution world.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxRequest {
    pub world_id: String,
    pub capabilities: CapabilitySet,
    pub require_security_boundary: bool,
}

/// Enforcement quality must be reported, never silently upgraded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnforcementQuality {
    Full,
    Partial,
    InProcessFence,
}

/// Sandbox resolution returned before tool dispatch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SandboxGrant {
    pub world_id: String,
    pub backend: String,
    pub quality: EnforcementQuality,
    pub granted: CapabilitySet,
}

pub trait SandboxProvider: Send + Sync {
    fn resolve(&self, request: &SandboxRequest) -> HarnessResult<SandboxGrant>;
}

/// Credential resolver returns bytes only to an authorized provider call.
pub trait CredentialProvider: Send + Sync {
    fn resolve(&self, reference: &CredentialRef) -> HarnessResult<Vec<u8>>;
    fn describe(&self, reference: &CredentialRef) -> HarnessResult<BTreeMap<String, String>>;
}

/// Strict local defaults used by the release candidate.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyByDefaultPermission;

impl PermissionProvider for DenyByDefaultPermission {
    fn authorize(
        &self,
        _actor: &str,
        capability: Capability,
        _resource: &str,
    ) -> HarnessResult<PermissionDecision> {
        Ok(match capability {
            Capability::Model | Capability::FileRead => PermissionDecision::Allow,
            _ => PermissionDecision::Deny {
                rule_id: "secure-default.v1".to_owned(),
                reason: "capability denied by secure default".to_owned(),
            },
        })
    }
}

/// Fixed one-shot confirmation provider useful for noninteractive embedding policy.
#[derive(Clone, Copy, Debug)]
pub struct StaticConfirmationProvider {
    pub outcome: ConfirmationOutcome,
}

impl Default for StaticConfirmationProvider {
    fn default() -> Self {
        Self {
            outcome: ConfirmationOutcome::Unavailable,
        }
    }
}

impl ConfirmationProvider for StaticConfirmationProvider {
    fn confirm(&self, _request: &ConfirmationRequest) -> HarnessResult<ConfirmationOutcome> {
        Ok(self.outcome)
    }
}

/// Deterministic verifier that accepts canonical values after contract validation.
#[derive(Clone, Copy, Debug, Default)]
pub struct CanonicalVerificationProvider;

impl VerificationProvider for CanonicalVerificationProvider {
    fn verify(&self, _tool_id: &str, _arguments: &Value, output: &Value) -> HarnessResult<()> {
        if output.to_canonical_json().len() > 8 * 1024 * 1024 {
            return Err(Failure::new(
                ErrorCode::VerificationFailed,
                FailureClass::Resource,
                "verification.canonical",
                "canonical tool output exceeds verifier limit",
            ));
        }
        Ok(())
    }
}

/// In-process root fence. It fails requests that require an OS security boundary.
#[derive(Clone, Debug)]
pub struct LocalFenceSandboxProvider {
    pub granted: CapabilitySet,
}

impl Default for LocalFenceSandboxProvider {
    fn default() -> Self {
        Self {
            granted: CapabilitySet::from_slice(&[Capability::FileRead, Capability::Model]),
        }
    }
}

impl SandboxProvider for LocalFenceSandboxProvider {
    fn resolve(&self, request: &SandboxRequest) -> HarnessResult<SandboxGrant> {
        if request.require_security_boundary {
            return Err(Failure::new(
                ErrorCode::SandboxUnavailable,
                FailureClass::Policy,
                "sandbox.resolve",
                "local in-process fence is not an OS security boundary",
            ));
        }
        if !request.capabilities.is_subset_of(&self.granted) {
            return Err(Failure::new(
                ErrorCode::PermissionDenied,
                FailureClass::Policy,
                "sandbox.resolve",
                "sandbox capability is not granted",
            ));
        }
        Ok(SandboxGrant {
            world_id: request.world_id.clone(),
            backend: "rooted-fs-fence".to_owned(),
            quality: EnforcementQuality::InProcessFence,
            granted: self.granted.clone(),
        })
    }
}

/// Minimal safety policy rejecting control bytes and oversized prompts.
#[derive(Clone, Copy, Debug, Default)]
pub struct BasicSafetyProvider;

impl SafetyProvider for BasicSafetyProvider {
    fn assess(&self, input: &str, _level: ExecutionLevel) -> HarnessResult<SafetyDecision> {
        if input.len() > 1024 * 1024 || input.chars().any(|character| character == '\0') {
            return Ok(SafetyDecision::Deny {
                reason: "input exceeds safe text bounds".to_owned(),
            });
        }
        Ok(SafetyDecision::Allow)
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/:".contains(&byte))
}

#[cfg(any(test, feature = "test-providers"))]
fn char_chunks(value: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for character in value.chars() {
        current.push(character);
        if current.chars().count() >= max_chars {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_streams_deterministically() -> HarnessResult<()> {
        let provider = EchoModelProvider::with_chunk_chars(3);
        let request = ModelRequest {
            request_id: "r1".to_owned(),
            provider: "echo".to_owned(),
            model: "echo-v1".to_owned(),
            system: String::new(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: "hello".to_owned(),
            }],
            tools: Vec::new(),
            attachments: Vec::new(),
            max_output_bytes: 100,
        };
        let mut chunks = Vec::new();
        let response = provider.stream(&request, &CancellationToken::new(), &mut |chunk| {
            chunks.push(chunk);
            Ok(())
        })?;
        assert_eq!(response.text, "echo: hello");
        assert!(chunks.len() >= 4);
        Ok(())
    }

    #[test]
    fn in_memory_memory_provider_supports_full_lifecycle() -> HarnessResult<()> {
        let provider = InMemoryMemoryProvider::default();
        let mut record = MemoryRecord {
            id: "preference-1".to_owned(),
            scope: MemoryScope::Preferences,
            namespace: "user-1".to_owned(),
            content: "Prefers concise technical explanations".to_owned(),
            attributes: BTreeMap::from([("source".to_owned(), "explicit".to_owned())]),
        };
        provider.store(record.clone())?;
        assert!(provider.store(record.clone()).is_err());
        let found = provider.search(&MemoryQuery {
            scope: MemoryScope::Preferences,
            namespace: Some("user-1".to_owned()),
            text: "concise".to_owned(),
            limit: 10,
        })?;
        assert_eq!(found, vec![record.clone()]);
        record.content = "Prefers concise, evidence-backed explanations".to_owned();
        provider.update(record.clone())?;
        assert_eq!(
            provider.retrieve(MemoryScope::Preferences, "user-1", "preference-1")?,
            Some(record)
        );
        assert!(provider.delete(MemoryScope::Preferences, "user-1", "preference-1")?);
        assert!(!provider.delete(MemoryScope::Preferences, "user-1", "preference-1")?);
        assert!(
            provider
                .retrieve(MemoryScope::None, "user-1", "preference-1")
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn registry_rejects_unadvertised_models_and_unbounded_requests() -> HarnessResult<()> {
        let mut registry = ModelRegistry::new();
        registry.register(Arc::new(EchoModelProvider::default()))?;
        let request = ModelRequest {
            request_id: "request-1".to_owned(),
            provider: "echo".to_owned(),
            model: "not-advertised".to_owned(),
            system: String::new(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: "hello".to_owned(),
            }],
            tools: Vec::new(),
            attachments: Vec::new(),
            max_output_bytes: 1024,
        };
        assert!(registry.prepare(request).is_err());
        let oversized = ModelRequest {
            request_id: "request-2".to_owned(),
            provider: "echo".to_owned(),
            model: "echo-v1".to_owned(),
            system: "x".repeat(256 * 1024 + 1),
            messages: Vec::new(),
            tools: Vec::new(),
            attachments: Vec::new(),
            max_output_bytes: 1024,
        };
        assert!(registry.prepare(oversized).is_err());
        Ok(())
    }
}
