// UnoOne Vault Core — Canonical record schema (directive §13)
// Every record in the vault follows this structure to enable
// cross-platform continuity between Windows, macOS, and Android.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Schema version — increment when the record format changes
pub const RECORD_SCHEMA_VERSION: u32 = 1;

/// Encryption version — matches the vault header version
pub const ENCRYPTION_VERSION: u32 = 1;

/// Record types that the vault supports (directive §13)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RecordType {
    Conversation,
    Message,
    Memory,
    Preference,
    Task,
    TaskStep,
    ToolResult,
    Document,
    Recording,
    Transcript,
    TranscriptSegment,
    Summary,
    ActionItem,
    BrowserResearch,
    ContactReference,
    ContextSnapshot,
    AuditRecord,
    DeletionTombstone,
    MigrationRecord,
}

/// Privacy levels for records
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrivacyLevel {
    /// Full detail — available to the owner only
    Private,
    /// Summary only — no raw audio or transcript
    SummaryOnly,
    /// Metadata only — title, timestamps, type
    MetadataOnly,
}

/// Canonical record structure (directive §13)
/// Every record MUST contain these fields.
/// Filenames are NOT used as record identity — stable UUIDs are used instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    /// Stable unique identifier (UUID v4)
    pub record_id: String,
    /// Type of record
    pub record_type: RecordType,
    /// Schema version for forward compatibility
    pub schema_version: u32,
    /// Encryption version used for this record
    pub encryption_version: u32,
    /// Creation timestamp (ISO 8601)
    pub created_at: String,
    /// Last update timestamp (ISO 8601)
    pub updated_at: String,
    /// Revision number (incremented on each update)
    pub revision: u32,
    /// Platform that created this record
    pub origin_platform: String,
    /// Device that created this record
    pub origin_device_id: String,
    /// Transaction ID for journaling (directive §17)
    pub transaction_id: String,
    /// SHA-256 hash of the encrypted content
    pub content_hash: String,
    /// Parent record ID (for threads, task steps, etc.)
    pub parent_record_id: Option<String>,
    /// Source record IDs that this record was derived from
    pub source_record_ids: Vec<String>,
    /// Privacy level
    pub privacy_level: PrivacyLevel,
    /// Whether this record is a tombstone (deleted)
    pub tombstone: bool,
    /// When this record was deleted (if tombstoned)
    pub deleted_at: Option<String>,
}

impl Record {
    /// Create a new record with generated IDs and timestamps
    pub fn new(record_type: RecordType, origin_platform: &str, origin_device_id: &str) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            record_id: Uuid::new_v4().to_string(),
            record_type,
            schema_version: RECORD_SCHEMA_VERSION,
            encryption_version: ENCRYPTION_VERSION,
            created_at: now.clone(),
            updated_at: now,
            revision: 1,
            origin_platform: origin_platform.to_string(),
            origin_device_id: origin_device_id.to_string(),
            transaction_id: Uuid::new_v4().to_string(),
            content_hash: String::new(), // filled after encryption
            parent_record_id: None,
            source_record_ids: Vec::new(),
            privacy_level: PrivacyLevel::Private,
            tombstone: false,
            deleted_at: None,
        }
    }

    /// Create a tombstone record for deletion
    /// Tombstones propagate across platforms so deleted data does not return
    pub fn create_tombstone(
        original_record_id: &str,
        origin_platform: &str,
        origin_device_id: &str,
    ) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            record_id: Uuid::new_v4().to_string(),
            record_type: RecordType::DeletionTombstone,
            schema_version: RECORD_SCHEMA_VERSION,
            encryption_version: ENCRYPTION_VERSION,
            created_at: now.clone(),
            updated_at: now.clone(),
            revision: 1,
            origin_platform: origin_platform.to_string(),
            origin_device_id: origin_device_id.to_string(),
            transaction_id: Uuid::new_v4().to_string(),
            content_hash: String::new(),
            parent_record_id: Some(original_record_id.to_string()),
            source_record_ids: Vec::new(),
            privacy_level: PrivacyLevel::MetadataOnly,
            tombstone: true,
            deleted_at: Some(Utc::now().to_rfc3339()),
        }
    }

    /// Mark this record as updated
    pub fn mark_updated(&mut self) {
        self.updated_at = Utc::now().to_rfc3339();
        self.revision += 1;
    }
}

/// Encrypted record as stored on disk
/// The plaintext content is encrypted with AES-256-GCM for newly written
/// records. Records written by older builds used XChaCha20-Poly1305 and remain
/// readable; the algorithm is identified on read by nonce length (12 bytes =
/// AES-GCM, 24 bytes = XChaCha20). See CipherAlgorithm in crypto.rs.
/// using a domain-specific key derived from the vault master key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedRecord {
    /// The record metadata (stored in plaintext for indexing)
    pub metadata: Record,
    /// Encrypted content (AES-256-GCM ciphertext + tag for new records, or
    /// legacy XChaCha20-Poly1305; base64)
    pub encrypted_content: String,
    /// Nonce used for encryption (base64)
    pub nonce: String,
    /// Associated data used for authentication (JSON of metadata fields, hex)
    pub associated_data: String,
    /// Which AAD construction produced `associated_data`.
    ///
    /// 0 (absent on older records) — legacy. The AAD was computed from the
    /// metadata BEFORE `content_hash` and `updated_at` were finalised, so the
    /// stored metadata does not match the authenticated bytes and cannot be
    /// checked against them.
    ///
    /// 2 — canonical. Metadata was fully finalised first, so the stored
    /// metadata re-serialises to exactly the authenticated bytes and IS
    /// verified on every read.
    #[serde(default)]
    pub aad_version: u8,
}

/// Current AAD construction version for newly written records.
pub const AAD_VERSION_CANONICAL: u8 = 2;

/// Canonical byte representation of record metadata, used as AEAD associated
/// data and re-derived on read to prove the plaintext metadata was not altered.
///
/// `Record` is a struct, so serde emits fields in declaration order — the
/// encoding is deterministic for a given schema.
pub fn canonical_metadata_bytes(record: &Record) -> Result<Vec<u8>, crate::error::VaultError> {
    serde_json::to_vec(record).map_err(|e| crate::error::VaultError::Serialization(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_record() {
        let record = Record::new(RecordType::Conversation, "DESKTOP", "device-001");
        assert!(!record.record_id.is_empty());
        assert_eq!(record.record_type, RecordType::Conversation);
        assert_eq!(record.schema_version, RECORD_SCHEMA_VERSION);
        assert_eq!(record.encryption_version, ENCRYPTION_VERSION);
        assert!(!record.tombstone);
        assert!(record.deleted_at.is_none());
    }

    #[test]
    fn test_create_tombstone() {
        let tombstone = Record::create_tombstone("original-id-123", "DESKTOP", "device-001");
        assert_eq!(tombstone.record_type, RecordType::DeletionTombstone);
        assert!(tombstone.tombstone);
        assert!(tombstone.deleted_at.is_some());
        assert_eq!(
            tombstone.parent_record_id,
            Some("original-id-123".to_string())
        );
    }

    #[test]
    fn test_record_serialization() {
        let record = Record::new(RecordType::Task, "MACOS", "device-002");
        let json = serde_json::to_string(&record).unwrap();
        let deserialized: Record = serde_json::from_str(&json).unwrap();
        assert_eq!(record, deserialized);
    }

    #[test]
    fn test_mark_updated() {
        let mut record = Record::new(RecordType::Memory, "DESKTOP", "device-001");
        assert_eq!(record.revision, 1);
        record.mark_updated();
        assert_eq!(record.revision, 2);
        assert!(record.updated_at >= record.created_at);
    }
}
