//! Failure-spend governance: Typed failure classification, circuit breakers, bounded retries.
//!
//! Based on ReliabilityBench findings: simple recovery policies outperform complex self-reflection.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Failure classification
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FailureType {
    RateLimit,         // Rate limiting
    Timeout,           // Timeout
    Network,           // Network error
    ProviderOutage,    // Provider outage
    MalformedStream,   // Malformed stream
    Authentication,    // Authentication failure
    Authorization,     // Authorization failure
    ResourceExhausted, // Resource exhausted
    InvalidInput,      // Invalid input
    InternalError,     // Internal error
    Unknown,           // Unknown error
}

/// Recovery action
#[derive(Clone, Debug, PartialEq)]
pub enum RecoveryAction {
    Retry,       // Retry the operation
    Backoff,     // Exponential backoff
    SwitchModel, // Switch to fallback model
    SwitchTool,  // Switch to fallback tool
    Abstain,     // Abstain and escalate
    Terminate,   // Terminate the operation
    Compensate,  // Compensate/rollback
}

/// Failure record
#[derive(Clone, Debug)]
pub struct FailureRecord {
    pub failure_id: String,
    pub failure_type: FailureType,
    pub operation: String,
    pub model: String,
    pub tool: Option<String>,
    pub timestamp: Instant,
    pub retry_count: usize,
    pub max_retries: usize,
    pub context: HashMap<String, String>,
}

/// Circuit breaker state
#[derive(Clone, Debug)]
pub struct CircuitBreaker {
    pub failure_threshold: usize,
    pub recovery_timeout: Duration,
    pub state: CircuitState,
    pub failure_count: usize,
    pub last_failure: Option<Instant>,
    pub success_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CircuitState {
    Closed,   // Normal operation
    Open,     // Circuit is open, reject requests
    HalfOpen, // Testing if service is back
}

/// Failure governance manager
pub struct FailureGovernance {
    failure_history: Arc<Mutex<Vec<FailureRecord>>>,
    circuit_breakers: Arc<Mutex<HashMap<String, CircuitBreaker>>>,
    recovery_policies: Arc<Mutex<HashMap<FailureType, RecoveryPolicy>>>,
    max_retries: usize,
    circuit_breaker_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct RecoveryPolicy {
    pub failure_type: FailureType,
    pub allowed_actions: Vec<RecoveryAction>,
    pub max_retries: usize,
    pub backoff_multiplier: f64,
    pub timeout: Duration,
    pub requires_approval: bool,
}

impl FailureGovernance {
    pub fn new(max_retries: usize) -> Self {
        Self {
            failure_history: Arc::new(Mutex::new(Vec::new())),
            circuit_breakers: Arc::new(Mutex::new(HashMap::new())),
            recovery_policies: Arc::new(Mutex::new(HashMap::new())),
            max_retries,
            circuit_breaker_enabled: true,
        }
    }

    /// Record a failure
    pub fn record_failure(&self, failure: FailureRecord) {
        // Add to history
        if let Ok(mut history) = self.failure_history.lock() {
            history.push(failure.clone());

            // Limit history size
            if history.len() > 1000 {
                history.remove(0);
            }
        }

        // Update circuit breaker
        if self.circuit_breaker_enabled {
            self.update_circuit_breaker(&failure);
        }
    }

    /// Determine recovery action for a failure
    pub fn determine_recovery(&self, failure: &FailureRecord) -> RecoveryAction {
        // Check circuit breaker
        if self.circuit_breaker_enabled && self.is_circuit_open(&failure.operation) {
            return RecoveryAction::Terminate;
        }

        // Check retry count against both the record's bound and the manager-wide bound
        if failure.retry_count >= failure.max_retries.min(self.max_retries) {
            return RecoveryAction::Abstain;
        }

        // Get recovery policy
        let Ok(policies) = self.recovery_policies.lock() else {
            return RecoveryAction::Abstain;
        };
        let policy = policies.get(&failure.failure_type);

        match policy {
            Some(policy) => {
                if policy.allowed_actions.contains(&RecoveryAction::Retry) {
                    if policy.allowed_actions.contains(&RecoveryAction::Backoff) {
                        RecoveryAction::Backoff
                    } else {
                        RecoveryAction::Retry
                    }
                } else if policy
                    .allowed_actions
                    .contains(&RecoveryAction::SwitchModel)
                {
                    RecoveryAction::SwitchModel
                } else if policy.allowed_actions.contains(&RecoveryAction::SwitchTool) {
                    RecoveryAction::SwitchTool
                } else {
                    RecoveryAction::Abstain
                }
            }
            None => RecoveryAction::Abstain,
        }
    }

    /// Check if circuit breaker is open
    pub fn is_circuit_open(&self, operation: &str) -> bool {
        let Ok(breakers) = self.circuit_breakers.lock() else {
            return false;
        };

        if let Some(breaker) = breakers.get(operation) {
            match breaker.state {
                CircuitState::Open => {
                    // Check if recovery timeout has passed
                    if let Some(last_failure) = breaker.last_failure {
                        if last_failure.elapsed() > breaker.recovery_timeout {
                            return false; // Transition to half-open
                        }
                    }
                    true
                }
                CircuitState::HalfOpen => false,
                CircuitState::Closed => false,
            }
        } else {
            false
        }
    }

    /// Update circuit breaker state
    fn update_circuit_breaker(&self, failure: &FailureRecord) {
        if let Ok(mut breakers) = self.circuit_breakers.lock() {
            let breaker = breakers
                .entry(failure.operation.clone())
                .or_insert(CircuitBreaker {
                    failure_threshold: 5,
                    recovery_timeout: Duration::from_secs(60),
                    state: CircuitState::Closed,
                    failure_count: 0,
                    last_failure: None,
                    success_count: 0,
                });

            breaker.failure_count += 1;
            breaker.last_failure = Some(failure.timestamp);

            // Open circuit if failure threshold reached
            if breaker.failure_count >= breaker.failure_threshold {
                breaker.state = CircuitState::Open;
            }
        }
    }

    /// Record a success (resets circuit breaker)
    pub fn record_success(&self, operation: &str) {
        if !self.circuit_breaker_enabled {
            return;
        }

        if let Ok(mut breakers) = self.circuit_breakers.lock() {
            if let Some(breaker) = breakers.get_mut(operation) {
                breaker.success_count += 1;
                breaker.failure_count = 0;
                breaker.state = CircuitState::Closed;
            }
        }
    }

    /// Get failure statistics
    pub fn get_failure_stats(&self) -> FailureStats {
        let (Ok(history), Ok(breakers)) =
            (self.failure_history.lock(), self.circuit_breakers.lock())
        else {
            return FailureStats {
                total_failures: 0,
                recent_failures: 0,
                open_circuits: 0,
                failure_types: HashMap::new(),
                avg_retries: 0.0,
            };
        };

        let total_failures = history.len();
        let recent_failures = history
            .iter()
            .filter(|f| f.timestamp.elapsed() < Duration::from_secs(300))
            .count();

        let open_circuits = breakers
            .values()
            .filter(|b| b.state == CircuitState::Open)
            .count();

        let failure_types: HashMap<FailureType, usize> =
            history.iter().fold(HashMap::new(), |mut acc, f| {
                *acc.entry(f.failure_type.clone()).or_insert(0) += 1;
                acc
            });

        FailureStats {
            total_failures,
            recent_failures,
            open_circuits,
            failure_types,
            avg_retries: history.iter().map(|f| f.retry_count).sum::<usize>() as f64
                / total_failures.max(1) as f64,
        }
    }

    /// Register a recovery policy
    pub fn register_recovery_policy(&self, policy: RecoveryPolicy) {
        if let Ok(mut policies) = self.recovery_policies.lock() {
            policies.insert(policy.failure_type.clone(), policy);
        }
    }

    /// Check if an operation should be allowed
    pub fn should_allow_operation(&self, operation: &str) -> bool {
        !self.is_circuit_open(operation)
    }

    /// Reset circuit breaker for an operation
    pub fn reset_circuit_breaker(&self, operation: &str) {
        if let Ok(mut breakers) = self.circuit_breakers.lock() {
            if let Some(breaker) = breakers.get_mut(operation) {
                breaker.state = CircuitState::Closed;
                breaker.failure_count = 0;
                breaker.last_failure = None;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct FailureStats {
    pub total_failures: usize,
    pub recent_failures: usize,
    pub open_circuits: usize,
    pub failure_types: HashMap<FailureType, usize>,
    pub avg_retries: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_failure_governance() {
        let governance = FailureGovernance::new(3);

        // Register recovery policy
        governance.register_recovery_policy(RecoveryPolicy {
            failure_type: FailureType::RateLimit,
            allowed_actions: vec![RecoveryAction::Backoff, RecoveryAction::Retry],
            max_retries: 3,
            backoff_multiplier: 2.0,
            timeout: Duration::from_secs(30),
            requires_approval: false,
        });

        // Record a failure
        let failure = FailureRecord {
            failure_id: "failure-1".to_string(),
            failure_type: FailureType::RateLimit,
            operation: "test_operation".to_string(),
            model: "test-model".to_string(),
            tool: None,
            timestamp: Instant::now(),
            retry_count: 0,
            max_retries: 3,
            context: HashMap::new(),
        };

        governance.record_failure(failure.clone());

        // Determine recovery action
        let action = governance.determine_recovery(&failure);
        assert!(matches!(action, RecoveryAction::Backoff));

        // Record more failures to trigger circuit breaker
        for _ in 0..5 {
            governance.record_failure(failure.clone());
        }

        // Check if circuit is open
        assert!(governance.is_circuit_open("test_operation"));

        // Get failure stats
        let stats = governance.get_failure_stats();
        assert_eq!(stats.total_failures, 6);
        assert!(stats.open_circuits > 0);
    }

    #[test]
    fn test_circuit_boundary_and_recovery() {
        let governance = FailureGovernance::new(3);

        let failure = FailureRecord {
            failure_id: "f".to_string(),
            failure_type: FailureType::Timeout,
            operation: "op".to_string(),
            model: "m".to_string(),
            tool: None,
            timestamp: Instant::now(),
            retry_count: 0,
            max_retries: 3,
            context: HashMap::new(),
        };

        // Boundary: at exactly the threshold (5) the circuit is OPEN and the
        // operation is blocked. Below it, the operation is allowed.
        assert!(governance.should_allow_operation("op"));
        for _ in 0..4 {
            governance.record_failure(failure.clone());
        }
        assert!(!governance.is_circuit_open("op"));
        assert!(governance.should_allow_operation("op"));

        // 5th failure crosses the threshold -> circuit opens, op blocked.
        governance.record_failure(failure.clone());
        assert!(governance.is_circuit_open("op"));
        assert!(!governance.should_allow_operation("op"));

        // An open circuit forces Terminate regardless of retry budget.
        assert!(matches!(
            governance.determine_recovery(&failure),
            RecoveryAction::Terminate
        ));

        // record_success closes the circuit and unblocks the operation.
        governance.record_success("op");
        assert!(!governance.is_circuit_open("op"));
        assert!(governance.should_allow_operation("op"));
    }
}
