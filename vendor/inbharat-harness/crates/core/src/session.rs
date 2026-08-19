//! Version-1 append-only session events, JSONL persistence, repair, fork, and replay.

use crate::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use crate::routing::ExecutionLevel;
use crate::value::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Current durable envelope format.
pub const SESSION_FORMAT_VERSION: u32 = 1;
/// Current core event version.
pub const EVENT_VERSION: u32 = 1;

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Opaque local session identity.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Generates a process-unique, time-qualified local id.
    #[must_use]
    pub fn generate() -> Self {
        let time = now_millis();
        let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(format!(
            "s-{time:016x}-{:08x}-{counter:016x}",
            std::process::id()
        ))
    }

    /// Parses an id at a trust boundary.
    pub fn parse(value: &str) -> HarnessResult<Self> {
        if value.len() < 3
            || value.len() > 96
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(Failure::invalid("session.id", "invalid session id"));
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Durable event facts. Every model-visible or side-effecting operation has a variant.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum EventData {
    SessionStarted {
        parent: Option<SessionId>,
        source_boundary: Option<u64>,
    },
    RouteSelected {
        level: ExecutionLevel,
        reason: String,
        rule_id: String,
    },
    Escalated {
        from: ExecutionLevel,
        to: ExecutionLevel,
        cause: String,
    },
    TurnStart {
        turn: u32,
    },
    UserMessage {
        message_id: String,
        content: String,
    },
    StepStart {
        turn: u32,
        step: u32,
        attempt: u32,
    },
    RequestHeader {
        request_id: String,
        provider: String,
        model: String,
        tools: Vec<String>,
        system: String,
    },
    ModelChunk {
        request_id: String,
        index: u32,
        kind: String,
        data: String,
    },
    AssistantMessage {
        message_id: String,
        content: String,
        finish: String,
    },
    ToolCall {
        call_id: String,
        tool_id: String,
        arguments: Value,
    },
    ToolResult {
        call_id: String,
        tool_id: String,
        output: Value,
        synthesized: bool,
    },
    Verification {
        call_id: String,
        passed: bool,
        detail: String,
    },
    ApprovalAsked {
        request_id: String,
        actor: String,
        action: String,
        risk: String,
    },
    ApprovalDecided {
        request_id: String,
        outcome: String,
    },
    Failure {
        code: String,
        operation: String,
        message: String,
        retryable: bool,
        attempt: u32,
    },
    Cancellation {
        cause: String,
    },
    RecoverySynthesized {
        target: String,
        reason: String,
    },
    StepEnd {
        reason: String,
    },
    TurnEnd {
        reason: String,
    },
    Job {
        job_id: String,
        status: String,
    },
    Attachment {
        id: String,
        media_type: String,
        bytes: u64,
        digest: String,
    },
    CredentialReference {
        provider: String,
        scope: String,
    },
}

impl EventData {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SessionStarted { .. } => "session.started",
            Self::RouteSelected { .. } => "route.selected",
            Self::Escalated { .. } => "route.escalated",
            Self::TurnStart { .. } => "turn.start",
            Self::UserMessage { .. } => "user.message",
            Self::StepStart { .. } => "step.start",
            Self::RequestHeader { .. } => "request.header",
            Self::ModelChunk { .. } => "model.chunk",
            Self::AssistantMessage { .. } => "assistant.message",
            Self::ToolCall { .. } => "tool.call",
            Self::ToolResult { .. } => "tool.result",
            Self::Verification { .. } => "tool.verification",
            Self::ApprovalAsked { .. } => "approval.asked",
            Self::ApprovalDecided { .. } => "approval.decided",
            Self::Failure { .. } => "failure",
            Self::Cancellation { .. } => "cancellation",
            Self::RecoverySynthesized { .. } => "recovery.synthesized",
            Self::StepEnd { .. } => "step.end",
            Self::TurnEnd { .. } => "turn.end",
            Self::Job { .. } => "job.status",
            Self::Attachment { .. } => "attachment.metadata",
            Self::CredentialReference { .. } => "credential.reference",
        }
    }

    #[must_use]
    pub const fn required_for_replay(&self) -> bool {
        matches!(
            self,
            Self::SessionStarted { .. }
                | Self::RouteSelected { .. }
                | Self::TurnStart { .. }
                | Self::UserMessage { .. }
                | Self::StepStart { .. }
                | Self::RequestHeader { .. }
                | Self::AssistantMessage { .. }
                | Self::ToolCall { .. }
                | Self::ToolResult { .. }
                | Self::ApprovalAsked { .. }
                | Self::ApprovalDecided { .. }
                | Self::StepEnd { .. }
                | Self::TurnEnd { .. }
        )
    }

    fn to_value(&self) -> Value {
        let mut object = BTreeMap::new();
        match self {
            Self::SessionStarted {
                parent,
                source_boundary,
            } => {
                insert_optional_string(
                    &mut object,
                    "parent",
                    parent.as_ref().map(SessionId::as_str),
                );
                insert_optional_u64(&mut object, "source_boundary", *source_boundary);
            }
            Self::RouteSelected {
                level,
                reason,
                rule_id,
            } => {
                insert_string(&mut object, "level", level.as_str());
                insert_string(&mut object, "reason", reason);
                insert_string(&mut object, "rule_id", rule_id);
            }
            Self::Escalated { from, to, cause } => {
                insert_string(&mut object, "from", from.as_str());
                insert_string(&mut object, "to", to.as_str());
                insert_string(&mut object, "cause", cause);
            }
            Self::TurnStart { turn } => insert_u32(&mut object, "turn", *turn),
            Self::UserMessage {
                message_id,
                content,
            } => {
                insert_string(&mut object, "message_id", message_id);
                insert_string(&mut object, "content", content);
            }
            Self::StepStart {
                turn,
                step,
                attempt,
            } => {
                insert_u32(&mut object, "turn", *turn);
                insert_u32(&mut object, "step", *step);
                insert_u32(&mut object, "attempt", *attempt);
            }
            Self::RequestHeader {
                request_id,
                provider,
                model,
                tools,
                system,
            } => {
                insert_string(&mut object, "request_id", request_id);
                insert_string(&mut object, "provider", provider);
                insert_string(&mut object, "model", model);
                object.insert(
                    "tools".to_owned(),
                    Value::Array(tools.iter().cloned().map(Value::String).collect()),
                );
                insert_string(&mut object, "system", system);
            }
            Self::ModelChunk {
                request_id,
                index,
                kind,
                data,
            } => {
                insert_string(&mut object, "request_id", request_id);
                insert_u32(&mut object, "index", *index);
                insert_string(&mut object, "kind", kind);
                insert_string(&mut object, "data", data);
            }
            Self::AssistantMessage {
                message_id,
                content,
                finish,
            } => {
                insert_string(&mut object, "message_id", message_id);
                insert_string(&mut object, "content", content);
                insert_string(&mut object, "finish", finish);
            }
            Self::ToolCall {
                call_id,
                tool_id,
                arguments,
            } => {
                insert_string(&mut object, "call_id", call_id);
                insert_string(&mut object, "tool_id", tool_id);
                object.insert("arguments".to_owned(), arguments.clone());
            }
            Self::ToolResult {
                call_id,
                tool_id,
                output,
                synthesized,
            } => {
                insert_string(&mut object, "call_id", call_id);
                insert_string(&mut object, "tool_id", tool_id);
                object.insert("output".to_owned(), output.clone());
                object.insert("synthesized".to_owned(), Value::Bool(*synthesized));
            }
            Self::Verification {
                call_id,
                passed,
                detail,
            } => {
                insert_string(&mut object, "call_id", call_id);
                object.insert("passed".to_owned(), Value::Bool(*passed));
                insert_string(&mut object, "detail", detail);
            }
            Self::ApprovalAsked {
                request_id,
                actor,
                action,
                risk,
            } => {
                insert_string(&mut object, "request_id", request_id);
                insert_string(&mut object, "actor", actor);
                insert_string(&mut object, "action", action);
                insert_string(&mut object, "risk", risk);
            }
            Self::ApprovalDecided {
                request_id,
                outcome,
            } => {
                insert_string(&mut object, "request_id", request_id);
                insert_string(&mut object, "outcome", outcome);
            }
            Self::Failure {
                code,
                operation,
                message,
                retryable,
                attempt,
            } => {
                insert_string(&mut object, "code", code);
                insert_string(&mut object, "operation", operation);
                insert_string(&mut object, "message", message);
                object.insert("retryable".to_owned(), Value::Bool(*retryable));
                insert_u32(&mut object, "attempt", *attempt);
            }
            Self::Cancellation { cause } => insert_string(&mut object, "cause", cause),
            Self::RecoverySynthesized { target, reason } => {
                insert_string(&mut object, "target", target);
                insert_string(&mut object, "reason", reason);
            }
            Self::StepEnd { reason } | Self::TurnEnd { reason } => {
                insert_string(&mut object, "reason", reason);
            }
            Self::Job { job_id, status } => {
                insert_string(&mut object, "job_id", job_id);
                insert_string(&mut object, "status", status);
            }
            Self::Attachment {
                id,
                media_type,
                bytes,
                digest,
            } => {
                insert_string(&mut object, "id", id);
                insert_string(&mut object, "media_type", media_type);
                insert_u64(&mut object, "bytes", *bytes);
                insert_string(&mut object, "digest", digest);
            }
            Self::CredentialReference { provider, scope } => {
                insert_string(&mut object, "provider", provider);
                insert_string(&mut object, "scope", scope);
            }
        }
        Value::Object(object)
    }

    fn from_value(kind: &str, value: &Value) -> HarnessResult<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| corrupt("event data is not an object"))?;
        match kind {
            "session.started" => Ok(Self::SessionStarted {
                parent: optional_string(object, "parent")?
                    .map(SessionId::parse)
                    .transpose()?,
                source_boundary: optional_u64(object, "source_boundary")?,
            }),
            "route.selected" => Ok(Self::RouteSelected {
                level: parse_level(required_string(object, "level")?)?,
                reason: required_string(object, "reason")?.to_owned(),
                rule_id: required_string(object, "rule_id")?.to_owned(),
            }),
            "route.escalated" => Ok(Self::Escalated {
                from: parse_level(required_string(object, "from")?)?,
                to: parse_level(required_string(object, "to")?)?,
                cause: required_string(object, "cause")?.to_owned(),
            }),
            "turn.start" => Ok(Self::TurnStart {
                turn: required_u32(object, "turn")?,
            }),
            "user.message" => Ok(Self::UserMessage {
                message_id: required_string(object, "message_id")?.to_owned(),
                content: required_string(object, "content")?.to_owned(),
            }),
            "step.start" => Ok(Self::StepStart {
                turn: required_u32(object, "turn")?,
                step: required_u32(object, "step")?,
                attempt: required_u32(object, "attempt")?,
            }),
            "request.header" => Ok(Self::RequestHeader {
                request_id: required_string(object, "request_id")?.to_owned(),
                provider: required_string(object, "provider")?.to_owned(),
                model: required_string(object, "model")?.to_owned(),
                tools: required_strings(object, "tools")?,
                system: required_string(object, "system")?.to_owned(),
            }),
            "model.chunk" => Ok(Self::ModelChunk {
                request_id: required_string(object, "request_id")?.to_owned(),
                index: required_u32(object, "index")?,
                kind: required_string(object, "kind")?.to_owned(),
                data: required_string(object, "data")?.to_owned(),
            }),
            "assistant.message" => Ok(Self::AssistantMessage {
                message_id: required_string(object, "message_id")?.to_owned(),
                content: required_string(object, "content")?.to_owned(),
                finish: required_string(object, "finish")?.to_owned(),
            }),
            "tool.call" => Ok(Self::ToolCall {
                call_id: required_string(object, "call_id")?.to_owned(),
                tool_id: required_string(object, "tool_id")?.to_owned(),
                arguments: required_value(object, "arguments")?.clone(),
            }),
            "tool.result" => Ok(Self::ToolResult {
                call_id: required_string(object, "call_id")?.to_owned(),
                tool_id: required_string(object, "tool_id")?.to_owned(),
                output: required_value(object, "output")?.clone(),
                synthesized: required_bool(object, "synthesized")?,
            }),
            "tool.verification" => Ok(Self::Verification {
                call_id: required_string(object, "call_id")?.to_owned(),
                passed: required_bool(object, "passed")?,
                detail: required_string(object, "detail")?.to_owned(),
            }),
            "approval.asked" => Ok(Self::ApprovalAsked {
                request_id: required_string(object, "request_id")?.to_owned(),
                actor: required_string(object, "actor")?.to_owned(),
                action: required_string(object, "action")?.to_owned(),
                risk: required_string(object, "risk")?.to_owned(),
            }),
            "approval.decided" => Ok(Self::ApprovalDecided {
                request_id: required_string(object, "request_id")?.to_owned(),
                outcome: required_string(object, "outcome")?.to_owned(),
            }),
            "failure" => Ok(Self::Failure {
                code: required_string(object, "code")?.to_owned(),
                operation: required_string(object, "operation")?.to_owned(),
                message: required_string(object, "message")?.to_owned(),
                retryable: required_bool(object, "retryable")?,
                attempt: required_u32(object, "attempt")?,
            }),
            "cancellation" => Ok(Self::Cancellation {
                cause: required_string(object, "cause")?.to_owned(),
            }),
            "recovery.synthesized" => Ok(Self::RecoverySynthesized {
                target: required_string(object, "target")?.to_owned(),
                reason: required_string(object, "reason")?.to_owned(),
            }),
            "step.end" => Ok(Self::StepEnd {
                reason: required_string(object, "reason")?.to_owned(),
            }),
            "turn.end" => Ok(Self::TurnEnd {
                reason: required_string(object, "reason")?.to_owned(),
            }),
            "job.status" => Ok(Self::Job {
                job_id: required_string(object, "job_id")?.to_owned(),
                status: required_string(object, "status")?.to_owned(),
            }),
            "attachment.metadata" => Ok(Self::Attachment {
                id: required_string(object, "id")?.to_owned(),
                media_type: required_string(object, "media_type")?.to_owned(),
                bytes: required_u64(object, "bytes")?,
                digest: required_string(object, "digest")?.to_owned(),
            }),
            "credential.reference" => Ok(Self::CredentialReference {
                provider: required_string(object, "provider")?.to_owned(),
                scope: required_string(object, "scope")?.to_owned(),
            }),
            _ => Err(corrupt("unknown required event type")),
        }
    }
}

/// Immutable accepted event envelope.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub format: u32,
    pub session_id: SessionId,
    pub seq: u64,
    pub time_ms: u64,
    pub event_version: u32,
    pub required_for_replay: bool,
    pub previous_chain: String,
    pub chain: String,
    pub data: EventData,
}

impl Event {
    #[must_use]
    pub fn to_json_line(&self) -> String {
        self.envelope_value(true).to_canonical_json()
    }

    fn envelope_value(&self, include_chain: bool) -> Value {
        let mut object = BTreeMap::from([
            ("format".to_owned(), Value::Integer(i64::from(self.format))),
            (
                "session_id".to_owned(),
                Value::String(self.session_id.as_str().to_owned()),
            ),
            ("seq".to_owned(), Value::String(self.seq.to_string())),
            (
                "time_ms".to_owned(),
                Value::String(self.time_ms.to_string()),
            ),
            (
                "type".to_owned(),
                Value::String(self.data.kind().to_owned()),
            ),
            (
                "event_version".to_owned(),
                Value::Integer(i64::from(self.event_version)),
            ),
            (
                "required_for_replay".to_owned(),
                Value::Bool(self.required_for_replay),
            ),
            (
                "previous_chain".to_owned(),
                Value::String(self.previous_chain.clone()),
            ),
            ("data".to_owned(), self.data.to_value()),
        ]);
        if include_chain {
            object.insert("chain".to_owned(), Value::String(self.chain.clone()));
        }
        Value::Object(object)
    }

    fn from_json_line(line: &str) -> HarnessResult<Self> {
        let value = Value::parse_json(line).map_err(|message| corrupt(&message))?;
        let object = value
            .as_object()
            .ok_or_else(|| corrupt("event envelope is not an object"))?;
        let format = required_u32(object, "format")?;
        if format != SESSION_FORMAT_VERSION {
            return Err(corrupt("unsupported session format"));
        }
        let event_version = required_u32(object, "event_version")?;
        if event_version != EVENT_VERSION {
            return Err(corrupt("unsupported event version"));
        }
        let kind = required_string(object, "type")?;
        let data = EventData::from_value(kind, required_value(object, "data")?)?;
        let required_for_replay = required_bool(object, "required_for_replay")?;
        if required_for_replay != data.required_for_replay() {
            return Err(corrupt(
                "required-for-replay flag does not match event type",
            ));
        }
        Ok(Self {
            format,
            session_id: SessionId::parse(required_string(object, "session_id")?)?,
            seq: required_u64(object, "seq")?,
            time_ms: required_u64(object, "time_ms")?,
            event_version,
            required_for_replay,
            previous_chain: required_string(object, "previous_chain")?.to_owned(),
            chain: required_string(object, "chain")?.to_owned(),
            data,
        })
    }
}

/// Append-only live session with optional durable JSONL backing.
#[derive(Debug)]
pub struct Session {
    id: SessionId,
    events: Vec<Event>,
    path: Option<PathBuf>,
    open_turn: bool,
    open_step: bool,
    pending_tools: BTreeMap<String, String>,
    seen_tools: BTreeSet<String>,
    pending_approvals: BTreeSet<String>,
    seen_approvals: BTreeSet<String>,
}

impl Session {
    /// Creates an in-memory session and appends its start event.
    pub fn in_memory() -> HarnessResult<Self> {
        Self::new_inner(SessionId::generate(), None, None, None)
    }

    fn new_inner(
        id: SessionId,
        path: Option<PathBuf>,
        parent: Option<SessionId>,
        source_boundary: Option<u64>,
    ) -> HarnessResult<Self> {
        let mut session = Self {
            id,
            events: Vec::new(),
            path,
            open_turn: false,
            open_step: false,
            pending_tools: BTreeMap::new(),
            seen_tools: BTreeSet::new(),
            pending_approvals: BTreeSet::new(),
            seen_approvals: BTreeSet::new(),
        };
        session.append(EventData::SessionStarted {
            parent,
            source_boundary,
        })?;
        Ok(session)
    }

    #[must_use]
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    #[must_use]
    pub fn is_turn_open(&self) -> bool {
        self.open_turn
    }

    #[must_use]
    pub fn is_step_open(&self) -> bool {
        self.open_step
    }

    /// Validates, snapshots, durably writes, then publishes one event.
    pub fn append(&mut self, data: EventData) -> HarnessResult<&Event> {
        self.validate_transition(&data)?;
        if self.events.len() >= 1_000_000
            || data.to_value().to_canonical_json().len() > 8 * 1024 * 1024
        {
            return Err(Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Resource,
                "session.append",
                "session event count or event byte limit exceeded",
            ));
        }
        let seq = u64::try_from(self.events.len()).map_err(|_| {
            Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Resource,
                "session.append",
                "event sequence exhausted",
            )
        })?;
        let previous_chain = self.events.last().map_or_else(
            || "0000000000000000".to_owned(),
            |event| event.chain.clone(),
        );
        let mut event = Event {
            format: SESSION_FORMAT_VERSION,
            session_id: self.id.clone(),
            seq,
            time_ms: now_millis(),
            event_version: EVENT_VERSION,
            required_for_replay: data.required_for_replay(),
            previous_chain,
            chain: String::new(),
            data,
        };
        let payload = event.envelope_value(false).to_canonical_json();
        event.chain = format!("{:016x}", fnv1a64(payload.as_bytes()));
        if let Some(path) = &self.path {
            append_line(path, &event.to_json_line())?;
        }
        self.apply_transition(&event.data);
        self.events.push(event);
        self.events.last().ok_or_else(|| {
            Failure::new(
                ErrorCode::Internal,
                FailureClass::Internal,
                "session.append",
                "accepted event disappeared",
            )
        })
    }

    /// Forces a durable checkpoint before model and top-level side effects.
    pub fn checkpoint(&self) -> HarnessResult<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| io_failure("session.checkpoint", "cannot open session log", error))?;
        file.sync_all()
            .map_err(|error| io_failure("session.checkpoint", "cannot sync session log", error))
    }

    /// Synthesizes unknown tool results and closes interrupted boundaries.
    pub fn recover(&mut self, max_synthesized: usize) -> HarnessResult<usize> {
        let pending: Vec<(String, String)> = self
            .pending_tools
            .iter()
            .map(|(call_id, tool_id)| (call_id.clone(), tool_id.clone()))
            .collect();
        let pending_approvals: Vec<String> = self.pending_approvals.iter().cloned().collect();
        let closers = pending
            .len()
            .saturating_mul(2)
            .saturating_add(pending_approvals.len().saturating_mul(2))
            .saturating_add(usize::from(self.open_step))
            .saturating_add(usize::from(self.open_turn));
        if closers > max_synthesized {
            return Err(Failure::new(
                ErrorCode::RecoveryExhausted,
                FailureClass::Persistence,
                "session.recover",
                "recovery event bound exceeded",
            ));
        }
        let mut count = 0;
        for request_id in pending_approvals {
            self.append(EventData::RecoverySynthesized {
                target: request_id.clone(),
                reason: "approval_outcome_unavailable".to_owned(),
            })?;
            self.append(EventData::ApprovalDecided {
                request_id,
                outcome: "unavailable".to_owned(),
            })?;
            count += 2;
        }
        for (call_id, tool_id) in pending {
            self.append(EventData::RecoverySynthesized {
                target: call_id.clone(),
                reason: "tool_outcome_unknown".to_owned(),
            })?;
            self.append(EventData::ToolResult {
                call_id,
                tool_id,
                output: Value::Object(BTreeMap::from([(
                    "error".to_owned(),
                    Value::String("TOOL_OUTCOME_UNKNOWN".to_owned()),
                )])),
                synthesized: true,
            })?;
            count += 2;
        }
        if self.open_step {
            self.append(EventData::StepEnd {
                reason: "interrupted".to_owned(),
            })?;
            count += 1;
        }
        if self.open_turn {
            self.append(EventData::TurnEnd {
                reason: "interrupted".to_owned(),
            })?;
            count += 1;
        }
        Ok(count)
    }

    /// Verifies chain, lifecycle, and replay-correlated tool events.
    pub fn replay(&self) -> HarnessResult<ReplayReport> {
        let mut expected_chain = "0000000000000000".to_owned();
        let mut assistant_messages = Vec::new();
        let mut calls = BTreeSet::new();
        let mut results = BTreeSet::new();
        let mut approvals_asked = BTreeSet::new();
        let mut approvals_decided = BTreeSet::new();
        let mut open_turn = false;
        let mut open_step = false;
        for (index, event) in self.events.iter().enumerate() {
            if event.seq != u64::try_from(index).unwrap_or(u64::MAX)
                || event.session_id != self.id
                || event.previous_chain != expected_chain
            {
                return Err(corrupt(
                    "event sequence, session id, or chain predecessor mismatch",
                ));
            }
            let payload = event.envelope_value(false).to_canonical_json();
            let chain = format!("{:016x}", fnv1a64(payload.as_bytes()));
            if chain != event.chain {
                return Err(corrupt("event chain checksum mismatch"));
            }
            expected_chain = event.chain.clone();
            match &event.data {
                EventData::TurnStart { .. } if !open_turn => open_turn = true,
                EventData::StepStart { .. } if open_turn && !open_step => open_step = true,
                EventData::StepEnd { .. } if open_step => open_step = false,
                EventData::TurnEnd { .. } if open_turn && !open_step => open_turn = false,
                EventData::AssistantMessage { content, .. } => {
                    assistant_messages.push(content.clone());
                }
                EventData::ToolCall { call_id, .. } => {
                    if !calls.insert(call_id.clone()) {
                        return Err(corrupt("duplicate tool call id"));
                    }
                }
                EventData::ToolResult { call_id, .. }
                    if !calls.contains(call_id) || !results.insert(call_id.clone()) =>
                {
                    return Err(corrupt("unmatched or duplicate tool result"));
                }
                EventData::ApprovalAsked { request_id, .. } => {
                    if !approvals_asked.insert(request_id.clone()) {
                        return Err(corrupt("duplicate approval request id"));
                    }
                }
                EventData::ApprovalDecided { request_id, .. }
                    if !approvals_asked.contains(request_id)
                        || !approvals_decided.insert(request_id.clone()) =>
                {
                    return Err(corrupt("unmatched or duplicate approval decision"));
                }
                _ => {}
            }
        }
        Ok(ReplayReport {
            event_count: self.events.len(),
            assistant_messages,
            tool_calls: calls.len(),
            tool_results: results.len(),
            balanced: !open_turn
                && !open_step
                && calls == results
                && approvals_asked == approvals_decided,
            final_chain: expected_chain,
        })
    }

    fn validate_transition(&self, data: &EventData) -> HarnessResult<()> {
        let valid = match data {
            EventData::SessionStarted { .. } => self.events.is_empty(),
            EventData::TurnStart { .. } => !self.open_turn && !self.open_step,
            EventData::StepStart { .. } => self.open_turn && !self.open_step,
            EventData::RequestHeader { .. }
            | EventData::ModelChunk { .. }
            | EventData::AssistantMessage { .. }
            | EventData::Verification { .. } => self.open_step,
            EventData::Failure { .. } => self.open_turn,
            EventData::ApprovalAsked { request_id, .. } => {
                self.open_turn && !self.seen_approvals.contains(request_id)
            }
            EventData::ApprovalDecided { request_id, .. } => {
                self.open_turn && self.pending_approvals.contains(request_id)
            }
            EventData::ToolCall { call_id, .. } => {
                self.open_step && !self.seen_tools.contains(call_id)
            }
            EventData::ToolResult { call_id, .. } => {
                self.open_step && self.pending_tools.contains_key(call_id)
            }
            EventData::StepEnd { .. } => {
                self.open_step && self.pending_tools.is_empty() && self.pending_approvals.is_empty()
            }
            EventData::TurnEnd { .. } => {
                self.open_turn && !self.open_step && self.pending_approvals.is_empty()
            }
            EventData::UserMessage { .. }
            | EventData::RouteSelected { .. }
            | EventData::Escalated { .. }
            | EventData::Cancellation { .. }
            | EventData::RecoverySynthesized { .. }
            | EventData::Job { .. }
            | EventData::Attachment { .. }
            | EventData::CredentialReference { .. } => true,
        };
        if valid {
            Ok(())
        } else {
            Err(corrupt("event violates turn/step/tool lifecycle"))
        }
    }

    fn apply_transition(&mut self, data: &EventData) {
        match data {
            EventData::TurnStart { .. } => self.open_turn = true,
            EventData::StepStart { .. } => self.open_step = true,
            EventData::ToolCall {
                call_id, tool_id, ..
            } => {
                self.seen_tools.insert(call_id.clone());
                self.pending_tools.insert(call_id.clone(), tool_id.clone());
            }
            EventData::ToolResult { call_id, .. } => {
                self.pending_tools.remove(call_id);
            }
            EventData::ApprovalAsked { request_id, .. } => {
                self.seen_approvals.insert(request_id.clone());
                self.pending_approvals.insert(request_id.clone());
            }
            EventData::ApprovalDecided { request_id, .. } => {
                self.pending_approvals.remove(request_id);
            }
            EventData::StepEnd { .. } => self.open_step = false,
            EventData::TurnEnd { .. } => self.open_turn = false,
            _ => {}
        }
    }
}

/// Deterministic replay summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayReport {
    pub event_count: usize,
    pub assistant_messages: Vec<String>,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub balanced: bool,
    pub final_chain: String,
}

/// Portable local JSONL store with atomic session creation.
#[derive(Clone, Debug)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn open(root: impl AsRef<Path>) -> HarnessResult<Self> {
        fs::create_dir_all(root.as_ref())
            .map_err(|error| io_failure("session.store", "cannot create session store", error))?;
        let canonical = fs::canonicalize(root.as_ref()).map_err(|error| {
            io_failure("session.store", "cannot canonicalize session store", error)
        })?;
        Ok(Self { root: canonical })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn create(&self) -> HarnessResult<Session> {
        let id = SessionId::generate();
        let path = self.path_for(&id);
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| io_failure("session.create", "cannot create session log", error))?;
        Session::new_inner(id, Some(path), None, None)
    }

    pub fn resume(&self, id: &SessionId) -> HarnessResult<Session> {
        let path = self.path_for(id);
        let file = File::open(&path)
            .map_err(|error| io_failure("session.resume", "cannot open session log", error))?;
        let mut reader = BufReader::new(file);
        let mut events = Vec::new();
        let mut line_index = 0_usize;
        let mut valid_bytes = 0_u64;
        let mut torn_tail = false;
        loop {
            let mut bytes = Vec::new();
            let read = Read::by_ref(&mut reader)
                .take((8 * 1024 * 1024 + 1) as u64)
                .read_until(b'\n', &mut bytes)
                .map_err(|error| {
                    io_failure("session.resume", "cannot read session record", error)
                })?;
            if read == 0 {
                break;
            }
            line_index = line_index.saturating_add(1);
            if bytes.len() > 8 * 1024 * 1024 {
                return Err(corrupt("session record exceeds physical byte limit")
                    .with_detail("line", line_index.to_string()));
            }
            let terminated = bytes.last() == Some(&b'\n');
            if terminated {
                bytes.pop();
            }
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            let line = match std::str::from_utf8(&bytes) {
                Ok(line) => line,
                Err(_error) if !terminated => {
                    torn_tail = true;
                    break;
                }
                Err(_error) => return Err(corrupt("session record is not UTF-8")),
            };
            if line.trim().is_empty() {
                valid_bytes = valid_bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
                continue;
            }
            match Event::from_json_line(line) {
                Ok(event) => {
                    if events.len() >= 1_000_000 {
                        return Err(corrupt("session event count exceeds limit"));
                    }
                    events.push(event);
                }
                Err(_failure) if !terminated => {
                    torn_tail = true;
                    break;
                }
                Err(failure) => {
                    return Err(failure.with_detail("line", line_index.to_string()));
                }
            }
            valid_bytes = valid_bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        }
        drop(reader);
        if torn_tail {
            // Repair durability: truncate the torn tail AND make the repair
            // durable before the resumed session appends to the same file.
            // Without an fsync, a crash immediately after resume could
            // re-expose the torn bytes on filesystems that buffer metadata.
            // We sync the truncated file and then the parent directory so the
            // directory entry's recorded length is also flushed.
            OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|file| {
                    file.set_len(valid_bytes)?;
                    file.sync_all()
                })
                .map_err(|error| {
                    io_failure("session.resume", "cannot truncate torn session tail", error)
                })?;
            // Best-effort directory sync: not all filesystems support opening a
            // directory, and the file-level sync_all already guarantees the data
            // and inode are durable; the directory sync covers name->inode
            // metadata on filesystems that need it. Failure to open/sync the
            // directory must not fail the resume.
            if let Some(parent) = path.parent() {
                if let Ok(dir) = fs::File::open(parent) {
                    let _ = dir.sync_all();
                }
            }
        }
        if events.is_empty() {
            return Err(corrupt("session log is empty"));
        }
        if !matches!(
            events.first().map(|event| &event.data),
            Some(EventData::SessionStarted { .. })
        ) {
            return Err(corrupt("session log must begin with session.started"));
        }
        let mut session = Session {
            id: id.clone(),
            events: Vec::new(),
            path: Some(path),
            open_turn: false,
            open_step: false,
            pending_tools: BTreeMap::new(),
            seen_tools: BTreeSet::new(),
            pending_approvals: BTreeSet::new(),
            seen_approvals: BTreeSet::new(),
        };
        for event in events {
            if event.session_id != *id
                || event.seq != u64::try_from(session.events.len()).unwrap_or(u64::MAX)
            {
                return Err(corrupt("session id or sequence mismatch"));
            }
            session.validate_transition(&event.data)?;
            session.apply_transition(&event.data);
            session.events.push(event);
        }
        let _report = session.replay()?;
        Ok(session)
    }

    /// Forks a balanced completed-turn prefix under an optimistic source revision.
    pub fn fork(
        &self,
        source: &Session,
        boundary: u64,
        expected_revision: u64,
    ) -> HarnessResult<Session> {
        let revision = source
            .events
            .last()
            .map_or(0, |event| event.seq.saturating_add(1));
        if revision != expected_revision {
            return Err(Failure::new(
                ErrorCode::Conflict,
                FailureClass::Persistence,
                "session.fork",
                "source revision changed",
            ));
        }
        let boundary_index = usize::try_from(boundary)
            .map_err(|_| Failure::invalid("session.fork", "fork boundary is out of range"))?;
        let Some(boundary_event) = source.events.get(boundary_index) else {
            return Err(Failure::invalid(
                "session.fork",
                "fork boundary is out of range",
            ));
        };
        if !matches!(boundary_event.data, EventData::TurnEnd { .. }) {
            return Err(Failure::invalid(
                "session.fork",
                "fork boundary must be a completed turn end",
            ));
        }
        let id = SessionId::generate();
        let path = self.path_for(&id);
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| io_failure("session.fork", "cannot create fork log", error))?;
        let mut fork = Session::new_inner(id, Some(path), Some(source.id.clone()), Some(boundary))?;
        // Preserve source model-visible facts as explicit imported context, not hidden state.
        let mut transcript = source
            .events
            .iter()
            .take(boundary_index + 1)
            .filter_map(|event| match &event.data {
                EventData::UserMessage { content, .. } => Some(format!("user: {content}")),
                EventData::AssistantMessage { content, .. } => {
                    Some(format!("assistant: {content}"))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if truncate_utf8(&mut transcript, 512 * 1024) {
            transcript.push_str("\n[import truncated]");
        }
        fork.append(EventData::UserMessage {
            message_id: "fork-import-1".to_owned(),
            content: format!(
                "Untrusted completed transcript imported from {} at seq {}:\n{}",
                source.id, boundary, transcript
            ),
        })?;
        Ok(fork)
    }

    fn path_for(&self, id: &SessionId) -> PathBuf {
        self.root.join(format!("{}.jsonl", id.as_str()))
    }
}

fn append_line(path: &Path, line: &str) -> HarnessResult<()> {
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| {
            io_failure(
                "session.append",
                "cannot open session log for append",
                error,
            )
        })?;
    file.write_all(line.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|error| io_failure("session.append", "cannot append session event", error))?;
    Ok(())
}

fn truncate_utf8(value: &mut String, max: usize) -> bool {
    if value.len() <= max {
        return false;
    }
    let mut boundary = max;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    true
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn insert_string(object: &mut BTreeMap<String, Value>, key: &str, value: &str) {
    object.insert(key.to_owned(), Value::String(value.to_owned()));
}

fn insert_optional_string(object: &mut BTreeMap<String, Value>, key: &str, value: Option<&str>) {
    object.insert(
        key.to_owned(),
        value.map_or(Value::Null, |value| Value::String(value.to_owned())),
    );
}

fn insert_u32(object: &mut BTreeMap<String, Value>, key: &str, value: u32) {
    object.insert(key.to_owned(), Value::Integer(i64::from(value)));
}

fn insert_u64(object: &mut BTreeMap<String, Value>, key: &str, value: u64) {
    object.insert(key.to_owned(), Value::String(value.to_string()));
}

fn insert_optional_u64(object: &mut BTreeMap<String, Value>, key: &str, value: Option<u64>) {
    object.insert(
        key.to_owned(),
        value.map_or(Value::Null, |value| Value::String(value.to_string())),
    );
}

fn required_value<'a>(object: &'a BTreeMap<String, Value>, key: &str) -> HarnessResult<&'a Value> {
    object
        .get(key)
        .ok_or_else(|| corrupt("required event field is missing"))
}

fn required_string<'a>(object: &'a BTreeMap<String, Value>, key: &str) -> HarnessResult<&'a str> {
    required_value(object, key)?
        .as_str()
        .ok_or_else(|| corrupt("event field must be a string"))
}

fn optional_string<'a>(
    object: &'a BTreeMap<String, Value>,
    key: &str,
) -> HarnessResult<Option<&'a str>> {
    match required_value(object, key)? {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value)),
        _ => Err(corrupt("optional event field must be null or string")),
    }
}

fn required_bool(object: &BTreeMap<String, Value>, key: &str) -> HarnessResult<bool> {
    match required_value(object, key)? {
        Value::Bool(value) => Ok(*value),
        _ => Err(corrupt("event field must be a boolean")),
    }
}

fn required_u32(object: &BTreeMap<String, Value>, key: &str) -> HarnessResult<u32> {
    match required_value(object, key)? {
        Value::Integer(value) => {
            u32::try_from(*value).map_err(|_| corrupt("u32 field out of range"))
        }
        Value::String(value) => value.parse().map_err(|_| corrupt("invalid u32 string")),
        _ => Err(corrupt("event field must be an integer")),
    }
}

fn required_u64(object: &BTreeMap<String, Value>, key: &str) -> HarnessResult<u64> {
    match required_value(object, key)? {
        Value::Integer(value) => {
            u64::try_from(*value).map_err(|_| corrupt("u64 field out of range"))
        }
        Value::String(value) => value.parse().map_err(|_| corrupt("invalid u64 string")),
        _ => Err(corrupt("event field must be an integer string")),
    }
}

fn optional_u64(object: &BTreeMap<String, Value>, key: &str) -> HarnessResult<Option<u64>> {
    match required_value(object, key)? {
        Value::Null => Ok(None),
        Value::Integer(value) => u64::try_from(*value)
            .map(Some)
            .map_err(|_| corrupt("optional u64 out of range")),
        Value::String(value) => value
            .parse()
            .map(Some)
            .map_err(|_| corrupt("invalid optional u64 string")),
        _ => Err(corrupt("optional u64 must be null or integer string")),
    }
}

fn required_strings(object: &BTreeMap<String, Value>, key: &str) -> HarnessResult<Vec<String>> {
    let Value::Array(values) = required_value(object, key)? else {
        return Err(corrupt("event field must be an array"));
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| corrupt("event array member must be a string"))
        })
        .collect()
}

fn parse_level(value: &str) -> HarnessResult<ExecutionLevel> {
    ExecutionLevel::parse(value).ok_or_else(|| corrupt("invalid execution level"))
}

fn corrupt(message: &str) -> Failure {
    Failure::new(
        ErrorCode::SessionCorrupt,
        FailureClass::Persistence,
        "session.validate",
        message,
    )
}

fn io_failure(operation: &str, message: &str, error: std::io::Error) -> Failure {
    Failure::new(
        ErrorCode::SessionCorrupt,
        FailureClass::Persistence,
        operation,
        message,
    )
    .with_detail("io_kind", format!("{:?}", error.kind()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_replay_is_balanced() -> HarnessResult<()> {
        let mut session = Session::in_memory()?;
        session.append(EventData::TurnStart { turn: 1 })?;
        session.append(EventData::UserMessage {
            message_id: "u1".to_owned(),
            content: "hello".to_owned(),
        })?;
        session.append(EventData::StepStart {
            turn: 1,
            step: 1,
            attempt: 1,
        })?;
        session.append(EventData::RequestHeader {
            request_id: "r1".to_owned(),
            provider: "mock".to_owned(),
            model: "mock-v1".to_owned(),
            tools: Vec::new(),
            system: String::new(),
        })?;
        session.append(EventData::AssistantMessage {
            message_id: "a1".to_owned(),
            content: "hi".to_owned(),
            finish: "stop".to_owned(),
        })?;
        session.append(EventData::StepEnd {
            reason: "complete".to_owned(),
        })?;
        session.append(EventData::TurnEnd {
            reason: "complete".to_owned(),
        })?;
        let report = session.replay()?;
        assert!(report.balanced);
        assert_eq!(report.assistant_messages, vec!["hi"]);
        Ok(())
    }

    #[test]
    fn recovery_marks_unknown_tools() -> HarnessResult<()> {
        let mut session = Session::in_memory()?;
        session.append(EventData::TurnStart { turn: 1 })?;
        session.append(EventData::StepStart {
            turn: 1,
            step: 1,
            attempt: 1,
        })?;
        session.append(EventData::ToolCall {
            call_id: "c1".to_owned(),
            tool_id: "side.effect".to_owned(),
            arguments: Value::Null,
        })?;
        let count = session.recover(8)?;
        assert_eq!(count, 4);
        assert!(session.replay()?.balanced);
        Ok(())
    }

    #[test]
    fn torn_tail_is_truncated_and_repair_persists_across_resumes() -> HarnessResult<()> {
        // Build a real on-disk session store in a unique temp dir.
        let dir = std::env::temp_dir().join(format!("inbharat-session-{}", std::process::id()));
        let _cleanup = SessionDirCleanup(dir.clone());
        let store = SessionStore::open(&dir)?;

        // Create a session and append a balanced turn so the log is valid.
        let mut session = store.create()?;
        let id = session.id().clone();
        session.append(EventData::TurnStart { turn: 1 })?;
        session.append(EventData::TurnEnd {
            reason: "complete".to_owned(),
        })?;
        drop(session);

        // Append a torn tail directly to the log file: a partial record with no
        // terminating newline, simulating a crash mid-write.
        let log_path = store.path_for(&id);
        {
            use std::io::Write as _;
            let mut file = OpenOptions::new()
                .append(true)
                .open(&log_path)
                .map_err(|e| io_failure("test.setup", "cannot open log for torn append", e))?;
            file.write_all(b"{\"format\":1,\"session\":")
                .and_then(|()| file.sync_all())
                .map_err(|e| io_failure("test.setup", "cannot write torn tail", e))?;
        }
        let size_with_torn = fs::metadata(&log_path)
            .map_err(|e| io_failure("test.setup", "cannot stat log", e))?
            .len();

        // First resume: the torn tail is detected, truncated, and the repair is
        // made durable. The resumed session must contain only the valid prefix.
        let resumed = store.resume(&id)?;
        assert!(resumed.replay()?.balanced);
        let size_after_repair = fs::metadata(&log_path)
            .map_err(|e| io_failure("test.resume", "cannot stat repaired log", e))?
            .len();
        assert!(
            size_after_repair < size_with_torn,
            "torn tail must be truncated (was {size_with_torn}, now {size_after_repair})"
        );

        // Second resume: proves the repair persisted on the live filesystem (a
        // re-read sees the same repaired length, no re-tearing). Note this proves
        // CONSISTENCY of the repair, not power-loss crash-durability — the fsync
        // in resume() targets the latter but cannot be exercised in a unit test.
        let resumed_again = store.resume(&id)?;
        assert!(resumed_again.replay()?.balanced);
        assert_eq!(
            fs::metadata(&log_path)
                .map_err(|e| io_failure("test.resume", "cannot stat repaired log", e))?
                .len(),
            size_after_repair,
            "a second resume must see the same repaired length (no re-tearing)"
        );
        Ok(())
    }

    /// Best-effort removal of the test session directory on scope exit.
    struct SessionDirCleanup(PathBuf);
    impl Drop for SessionDirCleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
