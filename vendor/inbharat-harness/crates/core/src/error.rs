//! Structured, provider-neutral failure vocabulary.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};

/// Stable machine-readable failure identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
#[non_exhaustive]
pub enum ErrorCode {
    InvalidInput,
    RouteDenied,
    PermissionDenied,
    ConfirmationRequired,
    CapabilityUnavailable,
    BudgetExceeded,
    Cancelled,
    Timeout,
    ProviderFailed,
    ToolFailed,
    VerificationFailed,
    SandboxUnavailable,
    FilesystemDenied,
    SubprocessDenied,
    SessionCorrupt,
    RecoveryExhausted,
    Conflict,
    NotFound,
    Internal,
}

impl ErrorCode {
    /// Stable snake-case representation used by logs and the C ABI.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::RouteDenied => "route_denied",
            Self::PermissionDenied => "permission_denied",
            Self::ConfirmationRequired => "confirmation_required",
            Self::CapabilityUnavailable => "capability_unavailable",
            Self::BudgetExceeded => "budget_exceeded",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::ProviderFailed => "provider_failed",
            Self::ToolFailed => "tool_failed",
            Self::VerificationFailed => "verification_failed",
            Self::SandboxUnavailable => "sandbox_unavailable",
            Self::FilesystemDenied => "filesystem_denied",
            Self::SubprocessDenied => "subprocess_denied",
            Self::SessionCorrupt => "session_corrupt",
            Self::RecoveryExhausted => "recovery_exhausted",
            Self::Conflict => "conflict",
            Self::NotFound => "not_found",
            Self::Internal => "internal",
        }
    }
}

/// Broad ownership class for a failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureClass {
    User,
    Policy,
    Resource,
    Provider,
    Execution,
    Persistence,
    Internal,
}

/// A bounded structured failure safe to persist and present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub code: ErrorCode,
    pub class: FailureClass,
    pub operation: String,
    pub message: String,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
    pub attempt: u32,
    pub details: BTreeMap<String, String>,
}

impl Failure {
    /// Constructs a non-retryable failure with no sensitive details.
    #[must_use]
    pub fn new(
        code: ErrorCode,
        class: FailureClass,
        operation: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            class,
            operation: operation.into(),
            message: bound(message.into(), 1_024),
            retryable: false,
            retry_after_ms: None,
            attempt: 1,
            details: BTreeMap::new(),
        }
    }

    /// Marks the failure as retryable and optionally supplies backoff guidance.
    #[must_use]
    pub const fn retryable(mut self, retry_after_ms: Option<u64>) -> Self {
        self.retryable = true;
        self.retry_after_ms = retry_after_ms;
        self
    }

    /// Records a one-based attempt number.
    #[must_use]
    pub const fn at_attempt(mut self, attempt: u32) -> Self {
        self.attempt = attempt;
        self
    }

    /// Adds a bounded diagnostic detail. Secret values must never be supplied here.
    #[must_use]
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        if self.details.len() < 16 {
            self.details
                .insert(bound(key.into(), 64), bound(value.into(), 512));
        }
        self
    }

    /// Convenience constructor for invalid caller input.
    #[must_use]
    pub fn invalid(operation: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::InvalidInput,
            FailureClass::User,
            operation,
            message,
        )
    }

    /// Convenience constructor for cancellation.
    #[must_use]
    pub fn cancelled(operation: impl Into<String>, cause: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::Cancelled,
            FailureClass::Resource,
            operation,
            cause,
        )
    }
}

fn bound(mut value: String, max: usize) -> String {
    if value.len() <= max {
        return value;
    }
    let mut boundary = max;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

impl Display for Failure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}:{}: {}",
            self.code.as_str(),
            self.operation,
            self.message
        )
    }
}

impl Error for Failure {}

/// Result alias used throughout the core.
pub type HarnessResult<T> = Result<T, Failure>;
