//! InBharat Harness trusted control-plane core.
//!
//! The crate deliberately uses only the Rust standard library.  It owns deterministic
//! routing, bounded execution, append-only sessions, authority checks, and provider
//! contracts.  Model, storage, safety, execution, and user-interaction implementations
//! remain replaceable behind narrow traits.

#![forbid(unsafe_code)]

pub mod budget;
pub mod cancel;
pub mod error;
pub mod execution;
pub mod jobs;
pub mod metrics;
pub mod providers;
pub mod routing;
pub mod runtime;
pub mod session;
pub mod tools;
pub mod value;

#[cfg(feature = "research-innovations")]
// Research innovation modules (v0.2 line). They are intentionally excluded
// from the default production surface until each module is wired into the
// authoritative runtime and has provider-level acceptance evidence.
pub mod innovation {
    /// Generate a short unique ID without external dependencies.
    ///
    /// Combines a nanosecond timestamp with a process-wide counter so IDs are
    /// unique within a process and across restarts. Not a UUID — just an opaque,
    /// collision-resistant token for internal handles (instruction ids, snapshot
    /// ids, etc.).
    pub(crate) fn short_id() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let value = (nanos as u64) ^ (seq.wrapping_mul(0x9E3779B97F4A7C15));
        format!("{:013x}{:04x}", value, seq & 0xFFFF)
    }

    pub mod compaction;
    pub mod context;
    pub mod degradation;
    pub mod disclosure;
    pub mod failure;
    pub mod flywheel;
    pub mod isa;
    pub mod routing;
    pub mod taint;
    pub mod transaction;
}

pub use budget::{Budget, BudgetLimits};
pub use cancel::{CancelCause, CancellationToken};
pub use error::{ErrorCode, Failure, FailureClass, HarnessResult};
pub use execution::{ExecutionBroker, LocalExecutionBroker, ProcessSpec, RootedFs};
pub use jobs::{JobId, JobRegistry, JobSnapshot, JobStatus, SubagentRequest, SubagentResult};
pub use metrics::{Metrics, MetricsSnapshot};
pub use providers::{
    AttachmentMetadata, Capability, CapabilitySet, ConfirmationOutcome, ConfirmationProvider,
    CredentialRef, DenyByDefaultPermission, InMemoryMemoryProvider, MemoryCapabilities,
    MemoryProvider, MemoryQuery, MemoryRecord, MemoryScope, ModelChunk, ModelProvider,
    ModelRequest, ModelResponse, PermissionDecision, PermissionProvider, ReplayEntry,
    ReplayModelProvider, SafetyDecision, SafetyProvider, SandboxProvider,
    StaticConfirmationProvider, VerificationProvider,
};
#[cfg(any(test, feature = "test-providers"))]
pub use providers::{EchoModelProvider, MockModelProvider, MockStep};
pub use routing::{
    EscalationCause, ExecutionLevel, RouteDecision, RoutePolicy, RouteReason, RouteRequest, Router,
};
pub use runtime::{Harness, HarnessBuilder, MemoryOptions, RunOptions, RunOutcome, TrajectoryMode};
pub use session::{Event, EventData, ReplayReport, Session, SessionId, SessionStore};
pub use tools::{
    ConfirmationMode, Determinism, SideEffect, Tool, ToolArguments, ToolAuditEvent, ToolContext,
    ToolManifest, ToolOutput, ToolRegistry,
};
pub use value::Value;
