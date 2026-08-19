//! Hierarchical cooperative cancellation with first-cause-wins semantics.

use crate::error::{Failure, HarnessResult};
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Stable cancellation causes propagated across subsystems.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelCause {
    User,
    Parent,
    Deadline,
    Policy,
    Shutdown,
    Disposed,
}

impl CancelCause {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Parent => "parent",
            Self::Deadline => "deadline",
            Self::Policy => "policy",
            Self::Shutdown => "shutdown",
            Self::Disposed => "disposed",
        }
    }
}

struct CancelState {
    cancelled: AtomicBool,
    cause: Mutex<Option<CancelCause>>,
    changed: Condvar,
}

/// Cloneable token. A child observes its parent but cannot cancel it.
#[derive(Clone)]
pub struct CancellationToken {
    state: Arc<CancelState>,
    ancestors: Arc<Vec<Arc<CancelState>>>,
}

impl Debug for CancellationToken {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CancellationToken")
            .field("cancelled", &self.is_cancelled())
            .field("cause", &self.cause())
            .finish_non_exhaustive()
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationToken {
    /// Creates an uncancelled root token.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(CancelState {
                cancelled: AtomicBool::new(false),
                cause: Mutex::new(None),
                changed: Condvar::new(),
            }),
            ancestors: Arc::new(Vec::new()),
        }
    }

    /// Creates a child token inheriting cancellation from this token.
    #[must_use]
    pub fn child(&self) -> Self {
        let mut ancestors = Vec::with_capacity(self.ancestors.len().saturating_add(1));
        ancestors.extend(self.ancestors.iter().cloned());
        ancestors.push(Arc::clone(&self.state));
        Self {
            state: Arc::new(CancelState {
                cancelled: AtomicBool::new(false),
                cause: Mutex::new(None),
                changed: Condvar::new(),
            }),
            ancestors: Arc::new(ancestors),
        }
    }

    /// Cancels this token. Returns true only for the first caller.
    pub fn cancel(&self, cause: CancelCause) -> bool {
        if self
            .state
            .cancelled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        if let Ok(mut stored) = self.state.cause.lock() {
            *stored = Some(cause);
        }
        self.state.changed.notify_all();
        true
    }

    /// Whether this token or an ancestor has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
            || self
                .ancestors
                .iter()
                .rev()
                .any(|state| state.cancelled.load(Ordering::Acquire))
    }

    /// First visible cancellation cause.
    #[must_use]
    pub fn cause(&self) -> Option<CancelCause> {
        if self.state.cancelled.load(Ordering::Acquire) {
            return self.state.cause.lock().ok().and_then(|guard| *guard);
        }
        for state in self.ancestors.iter().rev() {
            if state.cancelled.load(Ordering::Acquire) {
                return state.cause.lock().ok().and_then(|guard| *guard);
            }
        }
        None
    }

    /// Returns a structured error when cancellation is visible.
    pub fn check(&self, operation: &str) -> HarnessResult<()> {
        match self.cause() {
            Some(cause) => Err(Failure::cancelled(operation, cause.as_str())),
            None => Ok(()),
        }
    }

    /// Waits until local or parent cancellation, polling parents at a bounded interval.
    #[must_use]
    pub fn wait_cancelled(&self, timeout: Duration) -> Option<CancelCause> {
        let started = Instant::now();
        loop {
            if let Some(cause) = self.cause() {
                return Some(cause);
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return None;
            }
            let quantum = remaining.min(Duration::from_millis(20));
            let Ok(guard) = self.state.cause.lock() else {
                return self.cause();
            };
            let _wait_result = self.state.changed.wait_timeout(guard, quantum);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_parent_chain_is_iterative_and_preserves_cause() {
        let root = CancellationToken::new();
        let mut child = root.child();
        for _ in 0..10_000 {
            child = child.child();
        }
        assert!(root.cancel(CancelCause::Parent));
        assert!(child.is_cancelled());
        assert_eq!(child.cause(), Some(CancelCause::Parent));
    }

    #[test]
    fn maximum_wait_duration_does_not_overflow_instant() {
        let token = CancellationToken::new();
        assert!(token.cancel(CancelCause::User));
        assert_eq!(token.wait_cancelled(Duration::MAX), Some(CancelCause::User));
    }
}
