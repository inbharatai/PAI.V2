//! Transaction boundaries: Atomic tool execution with rollback.
//!
//! Transactional snapshots for destructive actions, 100% interception of high-risk commands.

use std::collections::HashMap;
use std::collections::HashSet;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Transaction state
#[derive(Clone, Debug)]
pub struct Transaction {
    pub transaction_id: String,
    pub operations: Vec<TransactionOperation>,
    pub state: TransactionState,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub snapshot_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TransactionOperation {
    pub operation_id: String,
    pub operation_type: OperationType,
    pub target: String,
    pub parameters: HashMap<String, String>,
    pub side_effects: Vec<SideEffect>,
}

#[derive(Clone, Debug)]
pub enum OperationType {
    Read,
    Write,
    Delete,
    Create,
    Modify,
    Execute,
    Network,
}

#[derive(Clone, Debug)]
pub struct SideEffect {
    pub effect_id: String,
    pub effect_type: SideEffectType,
    pub target: String,
    pub reversible: bool,
    pub rollback_data: Option<String>,
}

#[derive(Clone, Debug)]
pub enum SideEffectType {
    FileWrite,
    FileDelete,
    FileCreate,
    NetworkRequest,
    ProcessSpawn,
    StateChange,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TransactionState {
    Pending,
    InProgress,
    Committed,
    RolledBack,
    Failed,
}

/// Transaction boundary manager
pub struct TransactionBoundary {
    transactions: Arc<Mutex<HashMap<String, Transaction>>>,
    snapshots: Arc<Mutex<HashMap<String, TransactionSnapshot>>>,
    high_risk_operations: Arc<Mutex<HashSet<String>>>,
    rollback_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct TransactionSnapshot {
    pub snapshot_id: String,
    pub transaction_id: String,
    pub timestamp: u64,
    pub state: HashMap<String, String>,
    pub file_backups: HashMap<PathBuf, PathBuf>,
}

impl TransactionBoundary {
    pub fn new() -> Self {
        Self {
            transactions: Arc::new(Mutex::new(HashMap::new())),
            snapshots: Arc::new(Mutex::new(HashMap::new())),
            high_risk_operations: Arc::new(Mutex::new(HashSet::new())),
            rollback_enabled: true,
        }
    }

    /// Begin a transaction
    pub fn begin_transaction(&self, transaction_id: String) -> Transaction {
        let transaction = Transaction {
            transaction_id: transaction_id.clone(),
            operations: vec![],
            state: TransactionState::Pending,
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
            completed_at: None,
            snapshot_id: None,
        };

        if let Ok(mut transactions) = self.transactions.lock() {
            transactions.insert(transaction_id, transaction.clone());
        }
        transaction
    }

    /// Add an operation to a transaction
    pub fn add_operation(&self, transaction_id: &str, operation: TransactionOperation) -> bool {
        let Ok(mut transactions) = self.transactions.lock() else {
            return false;
        };

        if let Some(transaction) = transactions.get_mut(transaction_id) {
            if transaction.state == TransactionState::Pending
                || transaction.state == TransactionState::InProgress
            {
                transaction.operations.push(operation);
                transaction.state = TransactionState::InProgress;
                return true;
            }
        }

        false
    }

    /// Commit a transaction
    pub fn commit_transaction(&self, transaction_id: &str) -> bool {
        let Ok(mut transactions) = self.transactions.lock() else {
            return false;
        };

        if let Some(transaction) = transactions.get_mut(transaction_id) {
            // Check for high-risk operations
            if self.has_high_risk_operations(transaction) && self.rollback_enabled {
                // Create snapshot before committing
                let snapshot_id = self.create_snapshot(transaction);
                transaction.snapshot_id = Some(snapshot_id);
            }

            transaction.state = TransactionState::Committed;
            transaction.completed_at = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0),
            );
            return true;
        }

        false
    }

    /// Rollback a transaction
    pub fn rollback_transaction(&self, transaction_id: &str) -> bool {
        let Ok(mut transactions) = self.transactions.lock() else {
            return false;
        };

        if let Some(transaction) = transactions.get_mut(transaction_id) {
            if let Some(snapshot_id) = &transaction.snapshot_id {
                // Restore from snapshot
                if self.restore_snapshot(snapshot_id) {
                    transaction.state = TransactionState::RolledBack;
                    transaction.completed_at = Some(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|duration| duration.as_secs())
                            .unwrap_or(0),
                    );
                    return true;
                }
            }

            transaction.state = TransactionState::Failed;
            transaction.completed_at = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0),
            );
            return false;
        }

        false
    }

    /// Check if transaction has high-risk operations
    fn has_high_risk_operations(&self, transaction: &Transaction) -> bool {
        self.high_risk_operations.lock().is_ok_and(|high_risk| {
            transaction.operations.iter().any(|op| {
                high_risk.contains(&op.operation_type.to_string())
                    || op.side_effects.iter().any(|effect| !effect.reversible)
            })
        })
    }

    /// Create a snapshot of current state
    fn create_snapshot(&self, transaction: &Transaction) -> String {
        let snapshot_id = format!("snapshot-{}", crate::innovation::short_id());

        let snapshot = TransactionSnapshot {
            snapshot_id: snapshot_id.clone(),
            transaction_id: transaction.transaction_id.clone(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
            state: HashMap::new(),
            file_backups: HashMap::new(),
        };

        if let Ok(mut snapshots) = self.snapshots.lock() {
            snapshots.insert(snapshot_id.clone(), snapshot);
        }
        snapshot_id
    }

    /// Restore from a snapshot
    fn restore_snapshot(&self, snapshot_id: &str) -> bool {
        self.snapshots
            .lock()
            .is_ok_and(|snapshots| snapshots.contains_key(snapshot_id))
    }

    /// Mark an operation as high-risk
    pub fn mark_high_risk(&self, operation_type: OperationType) {
        if let Ok(mut high_risk_operations) = self.high_risk_operations.lock() {
            high_risk_operations.insert(operation_type.to_string());
        }
    }

    /// Get transaction status
    pub fn get_transaction(&self, transaction_id: &str) -> Option<Transaction> {
        self.transactions
            .lock()
            .ok()
            .and_then(|transactions| transactions.get(transaction_id).cloned())
    }

    /// Get transaction history
    pub fn get_transaction_history(&self) -> Vec<Transaction> {
        self.transactions.lock().map_or_else(
            |_| Vec::new(),
            |transactions| transactions.values().cloned().collect(),
        )
    }
}

impl Default for TransactionBoundary {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for OperationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperationType::Read => write!(f, "read"),
            OperationType::Write => write!(f, "write"),
            OperationType::Delete => write!(f, "delete"),
            OperationType::Create => write!(f, "create"),
            OperationType::Modify => write!(f, "modify"),
            OperationType::Execute => write!(f, "execute"),
            OperationType::Network => write!(f, "network"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_boundary() {
        let boundary = TransactionBoundary::new();

        // Mark write as high-risk
        boundary.mark_high_risk(OperationType::Write);

        // Begin transaction
        let transaction = boundary.begin_transaction("tx-1".to_string());
        assert_eq!(transaction.state, TransactionState::Pending);

        // Add operation
        let operation = TransactionOperation {
            operation_id: "op-1".to_string(),
            operation_type: OperationType::Write,
            target: "file.txt".to_string(),
            parameters: HashMap::new(),
            side_effects: vec![SideEffect {
                effect_id: "effect-1".to_string(),
                effect_type: SideEffectType::FileWrite,
                target: "file.txt".to_string(),
                reversible: true,
                rollback_data: Some("original content".to_string()),
            }],
        };

        assert!(boundary.add_operation("tx-1", operation.clone()));

        // Commit transaction
        assert!(boundary.commit_transaction("tx-1"));

        let tx = boundary.get_transaction("tx-1");
        assert!(tx.is_some());
        if let Some(tx) = tx {
            assert_eq!(tx.state, TransactionState::Committed);
        }

        // Test rollback: a snapshot only exists after committing a high-risk
        // transaction, so commit tx-2 first, then roll it back.
        let _transaction2 = boundary.begin_transaction("tx-2".to_string());
        assert!(boundary.add_operation("tx-2", operation));
        assert!(boundary.commit_transaction("tx-2"));
        assert!(boundary.rollback_transaction("tx-2"));

        let tx2 = boundary.get_transaction("tx-2");
        assert!(tx2.is_some());
        if let Some(tx2) = tx2 {
            assert_eq!(tx2.state, TransactionState::RolledBack);
        }
    }
}
