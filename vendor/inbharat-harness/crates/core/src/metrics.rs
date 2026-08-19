//! Small in-process metrics ledger with no raw transcript capture.

use crate::routing::ExecutionLevel;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Thread-safe counters. Telemetry export is intentionally not built in.
#[derive(Debug, Default)]
pub struct Metrics {
    routed: [AtomicU64; 4],
    failures: AtomicU64,
    cancellations: AtomicU64,
    recoveries: AtomicU64,
    tool_calls: AtomicU64,
    model_calls: AtomicU64,
    routing_nanos: AtomicU64,
}

/// Stable point-in-time metric view.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MetricsSnapshot {
    pub routed: [u64; 4],
    pub failures: u64,
    pub cancellations: u64,
    pub recoveries: u64,
    pub tool_calls: u64,
    pub model_calls: u64,
    pub routing_nanos: u64,
}

impl Metrics {
    pub fn record_route(&self, level: ExecutionLevel, elapsed: Duration) {
        self.routed[usize::from(level as u8)].fetch_add(1, Ordering::Relaxed);
        self.routing_nanos.fetch_add(
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    pub fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_cancellation(&self) {
        self.cancellations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_recovery(&self) {
        self.recoveries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_tool_call(&self) {
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_model_call(&self) {
        self.model_calls.fetch_add(1, Ordering::Relaxed);
    }

    #[must_use]
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            routed: std::array::from_fn(|index| self.routed[index].load(Ordering::Relaxed)),
            failures: self.failures.load(Ordering::Relaxed),
            cancellations: self.cancellations.load(Ordering::Relaxed),
            recoveries: self.recoveries.load(Ordering::Relaxed),
            tool_calls: self.tool_calls.load(Ordering::Relaxed),
            model_calls: self.model_calls.load(Ordering::Relaxed),
            routing_nanos: self.routing_nanos.load(Ordering::Relaxed),
        }
    }
}
