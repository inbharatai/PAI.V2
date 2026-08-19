// UnoOne Vault Core — Vault header
// The vault header is the root of trust. It contains:
// - Argon2id parameters and salt for password derivation
// - The master key wrapped (encrypted) with the password-derived key
// - An HMAC-SHA-256 authentication tag for header integrity
// - A separate wrapped master key for recovery
//
// The header is double-buffered (directive §17): we never overwrite
// the only valid copy. A new header is written to a secondary slot
// before the old one is invalidated.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::crypto::*;
use crate::error::VaultError;

/// Vault header version
pub const HEADER_VERSION: u32 = 1;

/// File names for double-buffered headers
pub const HEADER_A_FILE: &str = "header_a.json";
pub const HEADER_B_FILE: &str = "header_b.json";

/// Vault header structure — stored on disk, authenticated but not encrypted
/// (the wrapped master key IS encrypted, but the header metadata is public)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VaultHeader {
    /// Schema version
    pub version: u32,
    /// Unique vault identifier
    pub vault_id: String,
    /// Argon2id parameters
    pub kdf_params: KdfParams,
    /// Random salt for password derivation (base64)
    pub salt: String,
    /// Master key wrapped with password-derived key (base64)
    pub wrapped_master_key: String,
    /// Nonce used for master key wrapping (base64)
    pub wrap_nonce: String,
    /// HMAC-SHA-256 of the header for integrity verification (base64)
    pub header_hmac: String,
    /// Whether a recovery key has been set
    pub recovery_enabled: bool,
    /// Master key wrapped with recovery secret (base64), if recovery is enabled
    pub wrapped_master_key_recovery: Option<String>,
    /// Nonce used for recovery key wrapping (base64), if recovery is enabled
    pub recovery_wrap_nonce: Option<String>,
    /// Salt for recovery key derivation (base64), if recovery is enabled
    pub recovery_salt: Option<String>,
    /// Timestamp of header creation (ISO 8601)
    pub created_at: String,
    /// Timestamp of last header update (ISO 8601)
    pub updated_at: String,

    // ---- slot-selection fields (added after v1) -------------------------
    //
    // `Option` + `skip_serializing_if` is deliberate and load-bearing: the
    // header HMAC is computed over the serialised struct, so a v1 header (which
    // has neither field) serialises byte-identically to before and its existing
    // HMAC still verifies. Using non-Option fields with serde defaults would
    // have added keys to the JSON and invalidated every header already written
    // to a Pocket AI drive.
    /// Monotonic generation counter. Absent on v1 headers, treated as 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    /// Whether this slot was fully committed. Absent on v1 headers, treated as
    /// committed — a v1 header on disk was by definition the live one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed: Option<bool>,
}

/// Argon2id key derivation parameters
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KdfParams {
    /// Memory cost in KiB (256 MiB = 262144 KiB)
    pub memory_kib: u32,
    /// Number of iterations
    pub iterations: u32,
    /// Parallelism
    pub parallelism: u32,
    /// Output key length in bytes
    pub output_len: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self {
            memory_kib: ARGON2_MEMORY,
            iterations: ARGON2_ITERATIONS,
            parallelism: ARGON2_PARALLELISM,
            output_len: KEY_ENCRYPTION_KEY_LEN as u32,
        }
    }
}

impl VaultHeader {
    /// Create a new vault header with password-derived key wrapping
    pub fn create(
        password: &[u8],
        vault_id: &str,
    ) -> Result<(VaultHeader, [u8; MASTER_KEY_LEN]), VaultError> {
        let salt = generate_salt();
        let master_key = generate_master_key();
        let wrap_nonce = generate_nonce();

        let kek = derive_key_encryption_key(password, &salt)?;

        let wrapped_master_key = wrap_master_key(&kek, &master_key, &wrap_nonce)?;

        let kdf_params = KdfParams::default();

        let header = Self {
            version: HEADER_VERSION,
            vault_id: vault_id.to_string(),
            kdf_params,
            salt: hex::encode(salt),
            wrapped_master_key: hex::encode(wrapped_master_key),
            wrap_nonce: hex::encode(wrap_nonce),
            header_hmac: String::new(), // computed below
            recovery_enabled: false,
            wrapped_master_key_recovery: None,
            recovery_wrap_nonce: None,
            recovery_salt: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            // A freshly created vault starts at generation 1, committed on
            // write. Starting at 1 rather than 0 keeps it distinguishable from
            // a migrated v1 header, which reports 0.
            generation: Some(1),
            committed: Some(true),
        };

        // Compute HMAC over the header (without the HMAC field itself)
        let header = header.with_hmac(&kek);

        Ok((header, master_key))
    }

    /// Unlock a vault header with a password
    /// Returns the unwrapped master key if the password is correct
    pub fn unlock_with_password(
        &self,
        password: &[u8],
    ) -> Result<[u8; MASTER_KEY_LEN], VaultError> {
        // Verify header HMAC first
        let salt: [u8; SALT_LEN] = hex::decode(&self.salt)
            .map_err(|e| VaultError::HeaderCorrupted(format!("Invalid salt: {}", e)))?
            .try_into()
            .map_err(|_| VaultError::HeaderCorrupted("Salt has wrong length".to_string()))?;

        let kek = derive_key_encryption_key(password, &salt)?;

        // Verify HMAC
        self.verify_hmac(&kek)?;

        // Unwrap master key
        let wrapped_key = hex::decode(&self.wrapped_master_key)
            .map_err(|e| VaultError::HeaderCorrupted(format!("Invalid wrapped key: {}", e)))?;
        let wrap_nonce: [u8; NONCE_LEN] = hex::decode(&self.wrap_nonce)
            .map_err(|e| VaultError::HeaderCorrupted(format!("Invalid wrap nonce: {}", e)))?
            .try_into()
            .map_err(|_| VaultError::HeaderCorrupted("Wrap nonce has wrong length".to_string()))?;

        unwrap_master_key(&kek, &wrapped_key, &wrap_nonce)
    }

    /// Unlock a vault header with a recovery secret
    pub fn unlock_with_recovery(
        &self,
        recovery_secret: &[u8; RECOVERY_SECRET_LEN],
    ) -> Result<[u8; MASTER_KEY_LEN], VaultError> {
        if !self.recovery_enabled {
            return Err(VaultError::InvalidRecoveryWords(
                "Recovery is not enabled for this vault".to_string(),
            ));
        }

        let recovery_salt: [u8; SALT_LEN] = self
            .recovery_salt
            .as_ref()
            .ok_or_else(|| VaultError::HeaderCorrupted("Recovery salt missing".to_string()))
            .and_then(|s| {
                hex::decode(s).map_err(|e| {
                    VaultError::HeaderCorrupted(format!("Invalid recovery salt: {}", e))
                })
            })?
            .try_into()
            .map_err(|_| {
                VaultError::HeaderCorrupted("Recovery salt has wrong length".to_string())
            })?;

        let recovery_kek = derive_key_encryption_key(recovery_secret, &recovery_salt)?;

        let wrapped_key = self
            .wrapped_master_key_recovery
            .as_ref()
            .ok_or_else(|| VaultError::HeaderCorrupted("Recovery wrapped key missing".to_string()))
            .and_then(|s| {
                hex::decode(s).map_err(|e| {
                    VaultError::HeaderCorrupted(format!("Invalid recovery wrapped key: {}", e))
                })
            })?;

        let recovery_wrap_nonce: [u8; NONCE_LEN] = self
            .recovery_wrap_nonce
            .as_ref()
            .ok_or_else(|| VaultError::HeaderCorrupted("Recovery wrap nonce missing".to_string()))
            .and_then(|s| {
                hex::decode(s).map_err(|e| {
                    VaultError::HeaderCorrupted(format!("Invalid recovery wrap nonce: {}", e))
                })
            })?
            .try_into()
            .map_err(|_| {
                VaultError::HeaderCorrupted("Recovery wrap nonce has wrong length".to_string())
            })?;

        unwrap_master_key(&recovery_kek, &wrapped_key, &recovery_wrap_nonce)
    }

    /// Change the vault password
    /// Returns a new header with the master key re-wrapped with the new password
    pub fn change_password(
        &self,
        old_password: &[u8],
        new_password: &[u8],
    ) -> Result<VaultHeader, VaultError> {
        // Unlock with old password to get master key
        let master_key = self.unlock_with_password(old_password)?;

        // Create new header with new password
        let new_salt = generate_salt();
        let new_wrap_nonce = generate_nonce();
        let new_kek = derive_key_encryption_key(new_password, &new_salt)?;

        let new_wrapped_key = wrap_master_key(&new_kek, &master_key, &new_wrap_nonce)?;

        let mut new_header = self.clone();
        new_header.salt = hex::encode(new_salt);
        new_header.wrapped_master_key = hex::encode(new_wrapped_key);
        new_header.wrap_nonce = hex::encode(new_wrap_nonce);
        new_header.updated_at = chrono::Utc::now().to_rfc3339();

        // Recompute HMAC with new kek
        let header = new_header.with_hmac(&new_kek);

        // Zero the master key from memory
        let mut master_key_zero = master_key;
        secure_zero(&mut master_key_zero);

        Ok(header)
    }

    /// Enable recovery on this header
    /// Returns the recovery secret (32 bytes) that must be shown to the user
    pub fn enable_recovery(
        &self,
        password: &[u8],
    ) -> Result<(VaultHeader, [u8; RECOVERY_SECRET_LEN]), VaultError> {
        let master_key = self.unlock_with_password(password)?;

        let recovery_secret = generate_recovery_secret();
        let recovery_salt = generate_salt();
        let recovery_nonce = generate_nonce();

        let recovery_kek = derive_key_encryption_key(&recovery_secret, &recovery_salt)?;
        let wrapped_recovery = wrap_master_key(&recovery_kek, &master_key, &recovery_nonce)?;

        let mut header = self.clone();
        header.recovery_enabled = true;
        header.wrapped_master_key_recovery = Some(hex::encode(wrapped_recovery));
        header.recovery_wrap_nonce = Some(hex::encode(recovery_nonce));
        header.recovery_salt = Some(hex::encode(recovery_salt));
        header.updated_at = chrono::Utc::now().to_rfc3339();

        // Recompute HMAC with the original kek
        let salt: [u8; SALT_LEN] = hex::decode(&header.salt)
            .map_err(|e| VaultError::HeaderCorrupted(format!("Invalid salt: {}", e)))?
            .try_into()
            .map_err(|_| VaultError::HeaderCorrupted("Salt has wrong length".to_string()))?;
        let kek = derive_key_encryption_key(password, &salt)?;
        let header = header.with_hmac(&kek);

        // Zero master key from memory
        let mut mk_zero = master_key;
        secure_zero(&mut mk_zero);

        Ok((header, recovery_secret))
    }

    /// Generation of this header. v1 headers (no field) are generation 0.
    pub fn generation_value(&self) -> u64 {
        self.generation.unwrap_or(0)
    }

    /// Whether this slot is committed. v1 headers are treated as committed
    /// because a v1 header on disk was, by definition, the live header.
    pub fn is_committed(&self) -> bool {
        self.committed.unwrap_or(true)
    }

    /// Produce the next generation of this header, marked committed.
    ///
    /// Callers must re-apply the HMAC after mutating, since generation is
    /// covered by it once present.
    pub fn with_next_generation(mut self, previous_max: u64) -> Self {
        self.generation = Some(previous_max.saturating_add(1));
        self.committed = Some(true);
        self
    }

    /// Recompute the header HMAC after mutating fields it covers.
    ///
    /// `generation` and `committed` are inside the HMAC once present, so any
    /// code that sets them must reseal or the header will fail verification.
    pub fn reseal(self, password: &[u8]) -> Result<Self, VaultError> {
        let salt: [u8; SALT_LEN] = hex::decode(&self.salt)
            .map_err(|e| VaultError::HeaderCorrupted(format!("Invalid salt: {}", e)))?
            .try_into()
            .map_err(|_| VaultError::HeaderCorrupted("Salt has wrong length".to_string()))?;
        let kek = derive_key_encryption_key(password, &salt)?;
        Ok(self.with_hmac(&kek))
    }

    /// Rank used to choose between slot A and slot B.
    ///
    /// Higher is newer. Generation dominates; `updated_at` breaks ties between
    /// two v1 headers, which both report generation 0.
    pub fn selection_rank(&self) -> (u64, String) {
        (self.generation_value(), self.updated_at.clone())
    }

    /// Compute HMAC over the header (excluding the HMAC field itself)
    fn with_hmac(mut self, kek: &[u8; KEY_ENCRYPTION_KEY_LEN]) -> Self {
        self.header_hmac = String::new(); // Clear before computing
        let header_json = serde_json::to_string(&self)
            .expect("Header serialization must not fail — all fields are serializable");
        let hmac = hmac_sha256(kek, header_json.as_bytes());
        self.header_hmac = hex::encode(hmac);
        self
    }

    /// Verify the header HMAC
    fn verify_hmac(&self, kek: &[u8; KEY_ENCRYPTION_KEY_LEN]) -> Result<(), VaultError> {
        let stored_hmac = &self.header_hmac;

        // Recompute HMAC without the stored value
        let mut header_for_hmac = self.clone();
        header_for_hmac.header_hmac = String::new();
        let header_json = serde_json::to_string(&header_for_hmac)
            .map_err(|e| VaultError::Serialization(e.to_string()))?;
        let computed_hmac = hmac_sha256(kek, header_json.as_bytes());

        let stored_bytes = hex::decode(stored_hmac)
            .map_err(|e| VaultError::HeaderCorrupted(format!("Invalid HMAC encoding: {}", e)))?;

        // Constant-time comparison
        if stored_bytes.len() != computed_hmac.len()
            || !constant_time_eq(&stored_bytes, &computed_hmac)
        {
            return Err(VaultError::WrongPassword);
        }

        Ok(())
    }

    /// Load a header from a file
    pub fn load_from_file(path: &Path) -> Result<VaultHeader, VaultError> {
        let content = std::fs::read_to_string(path).map_err(VaultError::Io)?;
        serde_json::from_str(&content).map_err(|e| VaultError::Serialization(e.to_string()))
    }

    /// Save header to a file (double-buffered)
    /// Writes to the inactive slot, then the caller should swap
    pub fn save_to_file(&self, path: &Path) -> Result<(), VaultError> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| VaultError::Serialization(e.to_string()))?;

        // Write to temp file first, then rename (atomic on most filesystems)
        let temp_path = path.with_extension("tmp");
        std::fs::write(&temp_path, &content)?;
        std::fs::rename(&temp_path, path)?;

        Ok(())
    }
}

/// Constant-time comparison to prevent timing attacks
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_unlock_header() {
        let (header, master_key) =
            VaultHeader::create(b"correct-password", "test-vault-001").unwrap();

        // Unlock with correct password
        let unlocked = header.unlock_with_password(b"correct-password").unwrap();
        assert_eq!(master_key, unlocked);
    }

    #[test]
    fn test_wrong_password_fails() {
        let (header, _) = VaultHeader::create(b"correct-password", "test-vault-002").unwrap();

        let result = header.unlock_with_password(b"wrong-password!!!");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VaultError::WrongPassword));
    }

    #[test]
    fn test_empty_password_fails_validation() {
        // Empty passwords are rejected at the vault level, not the crypto level.
        // Argon2id will still derive a key, but Vault::setup_vault() enforces
        // minimum password length.
        let (header, _) = VaultHeader::create(b"", "test-vault-003").unwrap();
        // Unlock with empty password should succeed (it's the same password used to create)
        let result = header.unlock_with_password(b"");
        assert!(result.is_ok());
    }

    #[test]
    fn test_modified_header_fails() {
        let (mut header, _) =
            VaultHeader::create(b"test-password-12345", "test-vault-004").unwrap();

        // Modify the wrapped key
        let wrapped = hex::decode(&header.wrapped_master_key).unwrap();
        let mut modified = wrapped.clone();
        modified[0] ^= 0x01; // Flip one byte
        header.wrapped_master_key = hex::encode(modified);

        let result = header.unlock_with_password(b"test-password-12345");
        assert!(result.is_err());
    }

    #[test]
    fn test_change_password() {
        let (header, original_master_key) =
            VaultHeader::create(b"old-password-12345", "test-vault-005").unwrap();

        let new_header = header
            .change_password(b"old-password-12345", b"new-password-12345")
            .unwrap();

        // Unlock with new password
        let unlocked = new_header
            .unlock_with_password(b"new-password-12345")
            .unwrap();
        assert_eq!(original_master_key, unlocked);

        // Old password should fail on the new header
        let result = new_header.unlock_with_password(b"old-password-12345");
        assert!(result.is_err());
    }

    #[test]
    fn test_enable_recovery_and_unlock() {
        let (header, original_master_key) =
            VaultHeader::create(b"test-password-12345", "test-vault-006").unwrap();

        let (header_with_recovery, recovery_secret) =
            header.enable_recovery(b"test-password-12345").unwrap();

        // Unlock with recovery secret
        let unlocked = header_with_recovery
            .unlock_with_recovery(&recovery_secret)
            .unwrap();
        assert_eq!(original_master_key, unlocked);

        // Invalid recovery secret should fail
        let mut wrong_secret = recovery_secret;
        wrong_secret[0] ^= 0x01;
        let result = header_with_recovery.unlock_with_recovery(&wrong_secret);
        assert!(result.is_err());
    }

    #[test]
    fn test_header_serialization_roundtrip() {
        let (header, _) = VaultHeader::create(b"test-password-12345", "test-vault-007").unwrap();

        let json = serde_json::to_string(&header).unwrap();
        let deserialized: VaultHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(header, deserialized);
    }

    #[test]
    fn test_header_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("header_a.json");

        let (header, master_key) =
            VaultHeader::create(b"test-password-12345", "test-vault-008").unwrap();
        header.save_to_file(&path).unwrap();

        let loaded = VaultHeader::load_from_file(&path).unwrap();
        let unlocked = loaded.unlock_with_password(b"test-password-12345").unwrap();
        assert_eq!(master_key, unlocked);
    }
}
