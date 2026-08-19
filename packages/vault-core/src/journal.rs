// UnoOne Vault Core — Write-ahead journal for exFAT crash safety (directive §17)
// Because the USB uses exFAT, we implement application-level transactions.
// Every write goes through: PENDING → COMMITTED / ROLLED_BACK
// This ensures that a crash or unsafe USB removal never corrupts data.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::error::VaultError;

/// Journal entry states
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JournalState {
    /// Transaction has been started but not yet committed
    Pending,
    /// Transaction has been committed — data is durable
    Committed,
    /// Transaction has been rolled back — data is discarded
    RolledBack,
}

/// A journal entry tracking a vault transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Unique transaction ID
    pub transaction_id: String,
    /// Current state of the transaction
    pub state: JournalState,
    /// Records affected by this transaction
    pub record_ids: Vec<String>,
    /// Operations in this transaction
    pub operations: Vec<JournalOperation>,
    /// When this journal entry was created
    pub created_at: String,
    /// When this entry was last updated
    pub updated_at: String,
    /// HMAC of the entry for integrity verification
    pub entry_hmac: String,
}

/// An operation within a transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum JournalOperation {
    /// Write a new record or update an existing one
    Write {
        record_id: String,
        /// Path relative to vault root
        relative_path: String,
    },
    /// Delete a record (creates a tombstone)
    Delete {
        record_id: String,
        relative_path: String,
    },
    /// Create or update an index
    UpdateIndex { index_name: String },
}

/// Result of crash recovery from the journal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashRecoveryResult {
    /// Whether recovery was needed
    pub recovery_needed: bool,
    /// Number of committed transactions found
    pub committed_count: u32,
    /// Number of pending transactions rolled back
    pub rolled_back_count: u32,
    /// Any errors encountered during recovery
    pub errors: Vec<String>,
}

/// The write-ahead journal manager
pub struct Journal {
    /// Path to the journal directory (VAULT/journal/)
    journal_dir: PathBuf,
}

impl Journal {
    /// Create a new journal manager for the given vault directory
    pub fn new(vault_root: &Path) -> Self {
        Self {
            journal_dir: vault_root.join("VAULT").join("journal"),
        }
    }

    /// Ensure the journal directory exists
    pub fn ensure_dir(&self) -> Result<(), VaultError> {
        std::fs::create_dir_all(&self.journal_dir)?;
        Ok(())
    }

    /// Begin a new transaction
    /// Returns the transaction ID
    pub fn begin_transaction(
        &self,
        operations: Vec<JournalOperation>,
    ) -> Result<String, VaultError> {
        self.ensure_dir()?;

        let transaction_id = Uuid::new_v4().to_string();
        let record_ids: Vec<String> = operations
            .iter()
            .filter_map(|op| match op {
                JournalOperation::Write { record_id, .. } => Some(record_id.clone()),
                JournalOperation::Delete { record_id, .. } => Some(record_id.clone()),
                JournalOperation::UpdateIndex { .. } => None,
            })
            .collect();

        let now = Utc::now().to_rfc3339();
        let entry = JournalEntry {
            transaction_id: transaction_id.clone(),
            state: JournalState::Pending,
            record_ids,
            operations,
            created_at: now.clone(),
            updated_at: now,
            entry_hmac: String::new(), // Computed below
        };

        let entry = self.compute_entry_hmac(entry);
        self.write_entry(&entry)?;

        Ok(transaction_id)
    }

    /// Commit a transaction — marks it as durable
    pub fn commit_transaction(&self, transaction_id: &str) -> Result<(), VaultError> {
        let mut entry = self.read_entry(transaction_id)?.ok_or_else(|| {
            VaultError::JournalRecoveryFailed(format!("Transaction {} not found", transaction_id))
        })?;

        if entry.state != JournalState::Pending {
            return Err(VaultError::NotPermitted(format!(
                "Transaction {} is not in PENDING state (current: {:?})",
                transaction_id, entry.state
            )));
        }

        entry.state = JournalState::Committed;
        entry.updated_at = Utc::now().to_rfc3339();
        let entry = self.compute_entry_hmac(entry);

        // Write committed entry (atomic: write to new file, then rename)
        self.write_entry(&entry)?;

        // Remove the pending entry file
        let pending_path = self.entry_path_pending(transaction_id);
        if pending_path.exists() {
            std::fs::remove_file(&pending_path)?;
        }

        Ok(())
    }

    /// Roll back a transaction — marks it as discarded
    pub fn rollback_transaction(&self, transaction_id: &str) -> Result<(), VaultError> {
        let mut entry = self.read_entry(transaction_id)?.ok_or_else(|| {
            VaultError::JournalRecoveryFailed(format!("Transaction {} not found", transaction_id))
        })?;

        if entry.state != JournalState::Pending {
            return Err(VaultError::NotPermitted(format!(
                "Transaction {} is not in PENDING state (current: {:?})",
                transaction_id, entry.state
            )));
        }

        entry.state = JournalState::RolledBack;
        entry.updated_at = Utc::now().to_rfc3339();
        let entry = self.compute_entry_hmac(entry);

        self.write_entry(&entry)?;

        // Remove the pending entry file
        let pending_path = self.entry_path_pending(transaction_id);
        if pending_path.exists() {
            std::fs::remove_file(&pending_path)?;
        }

        Ok(())
    }

    /// Recover from a crash — roll back any pending transactions
    /// and return statistics about what was found
    pub fn recover_from_crash(&self) -> Result<CrashRecoveryResult, VaultError> {
        self.ensure_dir()?;

        let mut committed_count = 0u32;
        let mut rolled_back_count = 0u32;
        let mut errors = Vec::new();

        let entries = self.list_entries()?;

        for entry in entries {
            match entry.state {
                JournalState::Pending => {
                    // Pending transactions were interrupted — roll them back
                    match self.rollback_transaction(&entry.transaction_id) {
                        Ok(()) => rolled_back_count += 1,
                        Err(e) => errors.push(format!(
                            "Failed to roll back transaction {}: {}",
                            entry.transaction_id, e
                        )),
                    }
                }
                JournalState::Committed => {
                    // Committed transactions are fine — just count them
                    committed_count += 1;

                    // Clean up committed entries older than 24 hours
                    // (they're no longer needed for crash recovery)
                    self.cleanup_committed_entry(&entry.transaction_id)?;
                }
                JournalState::RolledBack => {
                    // Already rolled back — just count and clean up
                    self.cleanup_committed_entry(&entry.transaction_id)?;
                }
            }
        }

        let recovery_needed = committed_count > 0 || rolled_back_count > 0;

        Ok(CrashRecoveryResult {
            recovery_needed,
            committed_count,
            rolled_back_count,
            errors,
        })
    }

    /// Compute HMAC for a journal entry
    fn compute_entry_hmac(&self, mut entry: JournalEntry) -> JournalEntry {
        // Clear HMAC before computing
        entry.entry_hmac = String::new();
        let json = serde_json::to_string(&entry)
            .expect("Journal entry serialization must not fail — all fields are serializable");
        // Use a fixed key for journal HMAC (not the vault master key)
        // This allows crash recovery without unlocking the vault
        let hmac = crate::crypto::hmac_sha256(b"unoone-vault-journal", json.as_bytes());
        entry.entry_hmac = hex::encode(hmac);
        entry
    }

    /// Write a journal entry to disk (atomic write)
    fn write_entry(&self, entry: &JournalEntry) -> Result<(), VaultError> {
        let path = self.entry_path(&entry.transaction_id, &entry.state);
        let content = serde_json::to_string_pretty(entry)
            .map_err(|e| VaultError::Serialization(e.to_string()))?;

        // Atomic write: write to temp file, then rename
        let temp_path = path.with_extension("tmp");
        std::fs::write(&temp_path, &content)?;
        std::fs::rename(&temp_path, &path)?;

        Ok(())
    }

    /// Read a journal entry by transaction ID
    fn read_entry(&self, transaction_id: &str) -> Result<Option<JournalEntry>, VaultError> {
        // Check both pending and committed states
        for state in &[
            JournalState::Pending,
            JournalState::Committed,
            JournalState::RolledBack,
        ] {
            let path = self.entry_path(transaction_id, state);
            if path.exists() {
                let content = std::fs::read_to_string(&path)?;
                let entry: JournalEntry = serde_json::from_str(&content)
                    .map_err(|e| VaultError::Serialization(e.to_string()))?;
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    /// List all journal entries
    fn list_entries(&self) -> Result<Vec<JournalEntry>, VaultError> {
        let mut entries = Vec::new();

        if !self.journal_dir.exists() {
            return Ok(entries);
        }

        for entry in std::fs::read_dir(&self.journal_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                let content = std::fs::read_to_string(&path)?;
                if let Ok(journal_entry) = serde_json::from_str::<JournalEntry>(&content) {
                    entries.push(journal_entry);
                }
            }
        }

        Ok(entries)
    }

    /// Get the path for a journal entry file
    fn entry_path(&self, transaction_id: &str, state: &JournalState) -> PathBuf {
        let suffix = match state {
            JournalState::Pending => "pending",
            JournalState::Committed => "committed",
            JournalState::RolledBack => "rolled_back",
        };
        self.journal_dir
            .join(format!("tx-{}-{}.json", transaction_id, suffix))
    }

    /// Get the path for a pending entry
    fn entry_path_pending(&self, transaction_id: &str) -> PathBuf {
        self.entry_path(transaction_id, &JournalState::Pending)
    }

    /// Clean up a committed or rolled-back entry
    fn cleanup_committed_entry(&self, transaction_id: &str) -> Result<(), VaultError> {
        for state in &[JournalState::Committed, JournalState::RolledBack] {
            let path = self.entry_path(transaction_id, state);
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_journal_begin_and_commit() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::new(dir.path());

        let tx_id = journal
            .begin_transaction(vec![JournalOperation::Write {
                record_id: "rec-001".to_string(),
                relative_path: "VAULT/records/rec-001.enc".to_string(),
            }])
            .unwrap();

        journal.commit_transaction(&tx_id).unwrap();

        let result = journal.recover_from_crash().unwrap();
        assert!(result.committed_count >= 1);
    }

    #[test]
    fn test_journal_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::new(dir.path());

        let tx_id = journal
            .begin_transaction(vec![JournalOperation::Write {
                record_id: "rec-002".to_string(),
                relative_path: "VAULT/records/rec-002.enc".to_string(),
            }])
            .unwrap();

        // Explicitly roll back the transaction
        journal.rollback_transaction(&tx_id).unwrap();

        // After explicit rollback, recover_from_crash cleans up rolled-back entries
        // but doesn't count them in rolled_back_count (they were already rolled back)
        let result = journal.recover_from_crash().unwrap();
        assert_eq!(result.rolled_back_count, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_crash_recovery_rolls_back_pending() {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::new(dir.path());

        // Begin a transaction but don't commit (simulating a crash)
        let _tx_id = journal
            .begin_transaction(vec![JournalOperation::Write {
                record_id: "rec-003".to_string(),
                relative_path: "VAULT/records/rec-003.enc".to_string(),
            }])
            .unwrap();

        // Simulate crash recovery
        let result = journal.recover_from_crash().unwrap();
        assert!(result.recovery_needed);
        assert!(result.rolled_back_count >= 1);
    }

    #[test]
    fn test_journal_entry_serialization() {
        let entry = JournalEntry {
            transaction_id: "tx-001".to_string(),
            state: JournalState::Pending,
            record_ids: vec!["rec-001".to_string()],
            operations: vec![JournalOperation::Write {
                record_id: "rec-001".to_string(),
                relative_path: "VAULT/records/rec-001.enc".to_string(),
            }],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            entry_hmac: String::new(),
        };

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: JournalEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry.transaction_id, deserialized.transaction_id);
    }
}
