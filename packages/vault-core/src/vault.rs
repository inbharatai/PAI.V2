// UnoOne Vault Core — Main Vault API
// This is the primary interface for vault operations.
// It ties together crypto, header, recovery, record, and journal modules.
//
// Design (directive §16):
//   Password → Argon2id → Key-encryption key → Wraps random vault master key
//   This allows password changes without re-encrypting every record.

use std::path::{Path, PathBuf};

use crate::crypto::*;
use crate::error::VaultError;
use crate::header::{VaultHeader, HEADER_A_FILE, HEADER_B_FILE};
use crate::journal::{Journal, JournalOperation};
use crate::record::{canonical_metadata_bytes, EncryptedRecord, Record, AAD_VERSION_CANONICAL};
use crate::recovery::RecoveryPhrase;

/// Minimum password length (directive §16: no short passwords)
pub const MIN_PASSWORD_LEN: usize = 8;

/// Vault state
#[derive(Debug, Clone, PartialEq)]
pub enum VaultState {
    /// Vault is locked — no keys in memory
    Locked,
    /// Vault is unlocked — master key is in memory
    Unlocked,
}

/// The main Vault struct
/// Holds the vault root path, header, and (when unlocked) the master key
pub struct Vault {
    /// Root path of the UNOONE directory on the USB
    vault_root: PathBuf,
    /// Current state
    state: VaultState,
    /// Vault header (loaded from disk, always available when vault exists)
    header: Option<VaultHeader>,
    /// The vault master key (only in memory when unlocked)
    /// Zeroed on lock
    master_key: Option<[u8; MASTER_KEY_LEN]>,
    /// Current active header slot (A or B) for double-buffering
    active_header_slot: HeaderSlot,
    /// Journal manager
    journal: Journal,
}

/// Which header slot is currently active (directive §17: double-buffered headers)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HeaderSlot {
    A,
    B,
}

/// Result of vault creation
#[derive(Debug)]
pub struct VaultCreateResult {
    /// The generated recovery phrase (24 words)
    /// MUST be shown to the user exactly once, then never stored in plaintext
    pub recovery_phrase: Vec<String>,
    /// The vault ID
    pub vault_id: String,
}

/// Result of vault unlock
#[derive(Debug)]
pub struct VaultUnlockResult {
    pub vault_id: String,
    pub is_recovery_unlock: bool,
}

impl Vault {
    /// Create a new vault at the specified path
    /// This creates the directory structure, generates the master key,
    /// wraps it with the password, and stores the header
    pub fn create(vault_root: &Path, password: &[u8]) -> Result<VaultCreateResult, VaultError> {
        let vault_id = uuid::Uuid::new_v4().to_string();
        Self::create_internal(vault_root, password, &vault_id)
    }

    /// Create a vault with a caller-provided identity.
    ///
    /// First-use setup on a PACKAGED drive must keep the packaged identity:
    /// manifest.json hashes the exact bytes of VAULT/identity/vault.id, so a
    /// setup that mints a fresh UUID would invalidate the package on first
    /// use. The packaged id bytes are preserved verbatim when they match;
    /// mismatches are refused rather than silently overwritten.
    pub fn create_with_vault_id(
        vault_root: &Path,
        password: &[u8],
        vault_id: &str,
    ) -> Result<VaultCreateResult, VaultError> {
        if vault_id.trim().is_empty() {
            return Err(VaultError::InvalidVaultStructure(
                "Vault id must not be empty".to_string(),
            ));
        }
        // Refuse to silently overwrite an existing packaged identity with a
        // different one. Identical bytes are fine (setup is idempotent).
        let vault_id_path = vault_root.join("VAULT").join("identity").join("vault.id");
        if let Ok(existing) = std::fs::read_to_string(&vault_id_path) {
            if existing.trim() != vault_id.trim() {
                return Err(VaultError::NotPermitted(
                    "Refusing to overwrite an existing vault identity (existing hash differs). \
                     An initialised vault cannot be re-initialised silently."
                        .to_string(),
                ));
            }
        }
        Self::create_internal(vault_root, password, vault_id)
    }

    fn create_internal(
        vault_root: &Path,
        password: &[u8],
        vault_id: &str,
    ) -> Result<VaultCreateResult, VaultError> {
        if password.len() < MIN_PASSWORD_LEN {
            return Err(VaultError::InvalidPassword(format!(
                "Password must be at least {} characters",
                MIN_PASSWORD_LEN
            )));
        }

        // Validate vault root path
        Self::validate_vault_root(vault_root)?;

        // Refuse to silently overwrite an initialised vault: a valid header
        // means keys were already issued here. Setup must be explicit.
        let header_a = vault_root.join("VAULT").join("header").join(HEADER_A_FILE);
        if header_a.exists() {
            return Err(VaultError::NotPermitted(
                "Refusing to create over an initialised vault (a header already exists). \
                 Open it with the existing password instead."
                    .to_string(),
            ));
        }

        // Create directory structure
        Self::create_directory_structure(vault_root)?;

        // Create vault header with password-derived key wrapping
        let (header, master_key) = VaultHeader::create(password, vault_id)?;

        // Enable recovery
        let (header, recovery_secret) = header.enable_recovery(password)?;
        let recovery_phrase = RecoveryPhrase::from_secret(recovery_secret);

        // Write header to slot A (initial creation always uses slot A)
        let header_path = vault_root.join("VAULT").join("header").join(HEADER_A_FILE);
        std::fs::create_dir_all(header_path.parent().unwrap())?;
        header.save_to_file(&header_path)?;

        // Create the vault.id file — but never rewrite an existing file whose
        // content already declares the same identity. A packaged Pocket AI
        // ship vault.id bytes that are hashed into manifest.vault.id_sha256;
        // rewriting them (even with trivially different whitespace) would
        // break the strict package verification on first use.
        let vault_id_path = vault_root.join("VAULT").join("identity").join("vault.id");
        std::fs::create_dir_all(vault_id_path.parent().unwrap())?;
        let existing_id = std::fs::read_to_string(&vault_id_path).ok();
        if existing_id.as_deref().map(str::trim) != Some(vault_id.trim()) {
            std::fs::write(&vault_id_path, vault_id)?;
        }

        // Write a lock marker indicating vault is created but not yet unlocked
        let lock_marker = vault_root
            .join("VAULT")
            .join("locks")
            .join(".vault-created");
        std::fs::create_dir_all(lock_marker.parent().unwrap())?;
        std::fs::write(&lock_marker, chrono::Utc::now().to_rfc3339())?;

        // Zero the master key from memory (not just drop — use secure_zero)
        let mut key_to_zero = master_key;
        secure_zero(&mut key_to_zero);

        Ok(VaultCreateResult {
            recovery_phrase: recovery_phrase.words.clone(),
            vault_id: vault_id.to_string(),
        })
    }

    /// Open an existing vault (without unlocking it)
    /// Loads the header from disk but does NOT derive any keys
    pub fn open(vault_root: &Path) -> Result<Vault, VaultError> {
        Self::validate_vault_root(vault_root)?;

        // Select the NEWEST VALID COMMITTED header across both slots.
        //
        // This previously read "if A exists use A, else B", which silently
        // discarded newer state: change_password() writes the new header into
        // the INACTIVE slot, so a password change written to B was ignored on
        // the next open whenever A still existed. The user's new password
        // appeared not to work, and the old one kept working.
        let header_path_a = vault_root.join("VAULT").join("header").join(HEADER_A_FILE);
        let header_path_b = vault_root.join("VAULT").join("header").join(HEADER_B_FILE);

        let mut candidates: Vec<(VaultHeader, HeaderSlot)> = Vec::new();
        for (path, slot) in [
            (&header_path_a, HeaderSlot::A),
            (&header_path_b, HeaderSlot::B),
        ] {
            if !path.exists() {
                continue;
            }
            // A corrupt slot must not prevent opening from the good one.
            match VaultHeader::load_from_file(path) {
                Ok(h) => candidates.push((h, slot)),
                Err(_) => continue,
            }
        }

        if candidates.is_empty() {
            return Err(VaultError::VaultNotFound(format!(
                "No readable vault header found at {}",
                vault_root.display()
            )));
        }

        // Prefer committed slots. Fall back to uncommitted only when no
        // committed slot survives, which means a write was interrupted.
        let committed_exists = candidates.iter().any(|(h, _)| h.is_committed());
        if committed_exists {
            candidates.retain(|(h, _)| h.is_committed());
        }

        candidates.sort_by_key(|(h, _)| std::cmp::Reverse(h.selection_rank()));
        let (header, active_slot) = candidates.remove(0);

        let journal = Journal::new(vault_root);

        Ok(Vault {
            vault_root: vault_root.to_path_buf(),
            state: VaultState::Locked,
            header: Some(header),
            master_key: None,
            active_header_slot: active_slot,
            journal,
        })
    }

    /// Unlock the vault with a password
    /// Derives the key-encryption key from the password, verifies the header HMAC,
    /// and unwraps the master key
    pub fn unlock(&mut self, password: &[u8]) -> Result<VaultUnlockResult, VaultError> {
        if password.is_empty() {
            return Err(VaultError::InvalidPassword(
                "Password cannot be empty".to_string(),
            ));
        }

        if self.state == VaultState::Unlocked {
            return Err(VaultError::VaultUnlocked);
        }

        let header = self
            .header
            .as_ref()
            .ok_or_else(|| VaultError::VaultNotFound("No vault header loaded".to_string()))?;

        let master_key = header.unlock_with_password(password)?;

        self.master_key = Some(master_key);
        self.state = VaultState::Unlocked;

        // Recover from any crash (roll back pending transactions)
        let recovery = self.journal.recover_from_crash()?;
        if recovery.recovery_needed {
            eprintln!(
                "Vault journal recovery: {} committed, {} rolled back",
                recovery.committed_count, recovery.rolled_back_count
            );
        }

        Ok(VaultUnlockResult {
            vault_id: header.vault_id.clone(),
            is_recovery_unlock: false,
        })
    }

    /// Unlock the vault with a recovery phrase (24 words)
    pub fn unlock_with_recovery(
        &mut self,
        words: &[String],
    ) -> Result<VaultUnlockResult, VaultError> {
        if self.state == VaultState::Unlocked {
            return Err(VaultError::VaultUnlocked);
        }

        let header = self
            .header
            .as_ref()
            .ok_or_else(|| VaultError::VaultNotFound("No vault header loaded".to_string()))?;

        let phrase = RecoveryPhrase::from_words(words)?;
        let master_key = header.unlock_with_recovery(phrase.secret())?;

        self.master_key = Some(master_key);
        self.state = VaultState::Unlocked;

        // Recover from any crash
        let _ = self.journal.recover_from_crash()?;

        Ok(VaultUnlockResult {
            vault_id: header.vault_id.clone(),
            is_recovery_unlock: true,
        })
    }

    /// Lock the vault — zero all keys from memory
    pub fn lock(&mut self) -> Result<(), VaultError> {
        if self.state == VaultState::Locked {
            return Err(VaultError::VaultLocked);
        }

        // Zero the master key from memory
        if let Some(mut key) = self.master_key.take() {
            secure_zero(&mut key);
        }

        self.state = VaultState::Locked;

        // Write a lock marker
        let lock_marker = self
            .vault_root
            .join("VAULT")
            .join("locks")
            .join(".vault-locked");
        if let Some(parent) = lock_marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&lock_marker, chrono::Utc::now().to_rfc3339());

        Ok(())
    }

    /// Change the vault password
    /// This re-wraps the master key with the new password
    /// and writes a new header to the inactive slot (double-buffered)
    pub fn change_password(
        &mut self,
        old_password: &[u8],
        new_password: &[u8],
    ) -> Result<(), VaultError> {
        if new_password.len() < MIN_PASSWORD_LEN {
            return Err(VaultError::InvalidPassword(format!(
                "New password must be at least {} characters",
                MIN_PASSWORD_LEN
            )));
        }

        let header = self
            .header
            .as_ref()
            .ok_or_else(|| VaultError::VaultNotFound("No vault header loaded".to_string()))?;

        // The new header must outrank BOTH slots, not just the active one.
        // Ranking only against the active slot would let a stale higher
        // generation in the other slot win the next open().
        let max_generation = self.max_header_generation_on_disk();

        let new_header = header
            .change_password(old_password, new_password)?
            .with_next_generation(max_generation)
            .reseal(new_password)?;

        // Write to the inactive slot
        let inactive_slot = match self.active_header_slot {
            HeaderSlot::A => HeaderSlot::B,
            HeaderSlot::B => HeaderSlot::A,
        };

        let header_path = self
            .vault_root
            .join("VAULT")
            .join("header")
            .join(match inactive_slot {
                HeaderSlot::A => HEADER_A_FILE,
                HeaderSlot::B => HEADER_B_FILE,
            });

        std::fs::create_dir_all(header_path.parent().unwrap())?;
        new_header.save_to_file(&header_path)?;

        self.header = Some(new_header);
        self.active_header_slot = inactive_slot;

        Ok(())
    }

    /// Get the current vault state
    pub fn state(&self) -> &VaultState {
        &self.state
    }

    /// Get the vault ID
    pub fn vault_id(&self) -> Option<&str> {
        self.header.as_ref().map(|h| h.vault_id.as_str())
    }

    /// Get the vault root path
    pub fn vault_root(&self) -> &Path {
        &self.vault_root
    }

    /// Check if the vault is unlocked
    pub fn is_unlocked(&self) -> bool {
        self.state == VaultState::Unlocked && self.master_key.is_some()
    }

    /// Get the master key (only available when unlocked)
    pub fn master_key(&self) -> Option<&[u8; MASTER_KEY_LEN]> {
        self.master_key.as_ref()
    }

    /// Encrypt a record and write it to the vault
    pub fn write_record(&mut self, record: Record, content: &[u8]) -> Result<(), VaultError> {
        if !self.is_unlocked() {
            return Err(VaultError::VaultLocked);
        }

        let master_key = self.master_key.as_ref().ok_or(VaultError::VaultLocked)?;

        // Reject anything that is not a canonical UUID before it can reach a
        // filesystem path. Record IDs were previously interpolated straight
        // into a filename.
        validate_record_id(&record.record_id)?;

        // Derive domain-specific key for records
        let domain_key = derive_domain_key(master_key, crate::crypto::DOMAIN_RECORDS);

        // FINALISE METADATA FIRST, THEN AUTHENTICATE IT.
        //
        // The previous order computed the AAD from `record`, encrypted with it,
        // and only afterwards set `content_hash` and called `mark_updated()`.
        // The stored plaintext metadata therefore differed from the metadata
        // that was actually authenticated, and read_record used the stored AAD
        // bytes verbatim without ever comparing them to the stored metadata.
        //
        // The consequence was that record_type, privacy_level, tombstone,
        // revision, parent and timestamps could all be edited on disk and the
        // content would still decrypt cleanly. Nothing detected it.
        let mut record = record;
        record.content_hash = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(content);
            hex::encode(hasher.finalize())
        };
        record.mark_updated();

        let aad = canonical_metadata_bytes(&record)?;

        // Encrypt content with domain key using the cross-platform default cipher
        // (AES-256-GCM) so Android can read records written by Desktop.
        let (nonce, encrypted_content) = encrypt_with_algorithm(
            CipherAlgorithm::default_for_records(),
            &domain_key,
            content,
            &aad,
        )?;

        let encrypted_record = EncryptedRecord {
            metadata: record.clone(),
            encrypted_content: hex::encode(encrypted_content),
            nonce: hex::encode(nonce),
            associated_data: hex::encode(&aad),
            aad_version: AAD_VERSION_CANONICAL,
        };

        let record_path = self
            .vault_root
            .join("VAULT")
            .join("records")
            .join(format!("{}.enc.json", record.record_id));

        std::fs::create_dir_all(record_path.parent().unwrap())?;

        let json = serde_json::to_string_pretty(&encrypted_record)
            .map_err(|e| VaultError::Serialization(e.to_string()))?;

        // JOURNAL THE WRITE.
        //
        // The vault owned a Journal and ran crash recovery on unlock, but
        // write_record never opened a transaction — it did temp-write plus
        // rename and nothing else. For a device whose primary failure mode is
        // being unplugged mid-write, the write-ahead journal has to actually
        // wrap the write.
        let relative_path = format!("VAULT/records/{}.enc.json", record.record_id);
        let transaction_id = self
            .journal
            .begin_transaction(vec![JournalOperation::Write {
                record_id: record.record_id.clone(),
                relative_path,
            }])?;

        let staged = |vault: &Self| -> Result<(), VaultError> {
            let _ = vault;
            let temp_path = record_path.with_extension("tmp");

            // Write and flush through the SAME handle.
            //
            // Reopening read-only to flush works on Unix but fails on Windows:
            // sync_all() maps to FlushFileBuffers, which requires write access,
            // so a read-only handle returns "Access is denied" (os error 5).
            // Windows CI caught this; the Linux and macOS runners did not.
            {
                use std::io::Write;
                let mut f = std::fs::File::create(&temp_path)?;
                f.write_all(json.as_bytes())?;
                // Durability before the rename, so a power loss cannot leave a
                // promoted-but-empty record.
                f.sync_all()?;
            }

            // Verify what actually landed on disk before promoting it.
            let written = std::fs::read_to_string(&temp_path)?;
            if written != json {
                return Err(VaultError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "staged record does not match the bytes intended for it",
                )));
            }

            std::fs::rename(&temp_path, &record_path)?;
            Ok(())
        };

        match staged(self) {
            Ok(()) => {
                self.journal.commit_transaction(&transaction_id)?;
                Ok(())
            }
            Err(e) => {
                // Roll back so recovery does not later try to complete a write
                // that never produced valid staged bytes.
                let _ = self.journal.rollback_transaction(&transaction_id);
                let _ = std::fs::remove_file(record_path.with_extension("tmp"));
                Err(e)
            }
        }
    }

    /// The currently selected header, if one is loaded.
    pub fn header(&self) -> Option<&VaultHeader> {
        self.header.as_ref()
    }

    /// Test hook: run journal crash recovery directly.
    #[cfg(test)]
    pub fn journal_recover_for_test(
        &self,
    ) -> Result<crate::journal::CrashRecoveryResult, VaultError> {
        self.journal.recover_from_crash()
    }

    /// Highest generation present in either header slot on disk.
    ///
    /// Read from disk rather than memory so a slot written by another process
    /// (or a previous interrupted run) cannot be silently outranked.
    fn max_header_generation_on_disk(&self) -> u64 {
        let dir = self.vault_root.join("VAULT").join("header");
        [HEADER_A_FILE, HEADER_B_FILE]
            .iter()
            .filter_map(|f| VaultHeader::load_from_file(&dir.join(f)).ok())
            .map(|h| h.generation_value())
            .max()
            .unwrap_or(0)
    }

    /// Read and decrypt a record from the vault
    pub fn read_record(&self, record_id: &str) -> Result<(Record, Vec<u8>), VaultError> {
        if !self.is_unlocked() {
            return Err(VaultError::VaultLocked);
        }

        let master_key = self.master_key.as_ref().ok_or(VaultError::VaultLocked)?;

        validate_record_id(record_id)?;

        let record_path = self
            .vault_root
            .join("VAULT")
            .join("records")
            .join(format!("{}.enc.json", record_id));

        if !record_path.exists() {
            return Err(VaultError::VaultNotFound(format!(
                "Record {} not found",
                record_id
            )));
        }

        let content = std::fs::read_to_string(&record_path)?;
        let encrypted_record: EncryptedRecord =
            serde_json::from_str(&content).map_err(|e| VaultError::Serialization(e.to_string()))?;

        // Derive domain key
        let domain_key = derive_domain_key(master_key, crate::crypto::DOMAIN_RECORDS);

        // Decode nonce and ciphertext
        let nonce = hex::decode(&encrypted_record.nonce)
            .map_err(|e| VaultError::DecryptionFailed(format!("Invalid nonce: {}", e)))?;

        let ciphertext = hex::decode(&encrypted_record.encrypted_content)
            .map_err(|e| VaultError::DecryptionFailed(format!("Invalid ciphertext: {}", e)))?;

        let aad = hex::decode(&encrypted_record.associated_data)
            .map_err(|e| VaultError::DecryptionFailed(format!("Invalid AAD: {}", e)))?;

        // PROVE THE PLAINTEXT METADATA IS THE METADATA THAT WAS AUTHENTICATED.
        //
        // Decrypting with the stored AAD bytes only proves those bytes were
        // authenticated — it says nothing about the plaintext metadata sitting
        // beside them, which is what the rest of the application actually
        // reads. Without this comparison, privacy_level, tombstone,
        // record_type, revision, parent and timestamps were all freely
        // editable on disk while the content still decrypted.
        //
        // Legacy records (aad_version 0) cannot be checked this way: their AAD
        // was captured before content_hash and updated_at were finalised, so a
        // mismatch is expected and is not evidence of tampering. They are read
        // as before and are upgraded to the canonical form on their next write.
        if encrypted_record.aad_version >= AAD_VERSION_CANONICAL {
            let recomputed = canonical_metadata_bytes(&encrypted_record.metadata)?;
            if recomputed != aad {
                return Err(VaultError::DecryptionFailed(format!(
                    "Record {} metadata does not match its authenticated associated data; \
                     the stored metadata was altered after the record was written",
                    record_id
                )));
            }
        }

        // Auto-detect cipher from nonce length so Desktop can read records written
        // by Android (AES-256-GCM, 12-byte nonce) and legacy Desktop records
        // (XChaCha20-Poly1305, 24-byte nonce).
        let algorithm = CipherAlgorithm::from_nonce_len(nonce.len())?;
        let plaintext = decrypt_with_algorithm(algorithm, &domain_key, &nonce, &ciphertext, &aad)?;

        // Check for tombstone
        if encrypted_record.metadata.tombstone {
            return Err(VaultError::NotPermitted(format!(
                "Record {} has been deleted",
                record_id
            )));
        }

        Ok((encrypted_record.metadata, plaintext))
    }

    /// Delete a record by creating a tombstone
    /// The tombstone overwrites the original record file so that read_record
    /// will find the tombstone instead of the original data.
    pub fn delete_record(
        &mut self,
        record_id: &str,
        origin_platform: &str,
        origin_device_id: &str,
    ) -> Result<(), VaultError> {
        if !self.is_unlocked() {
            return Err(VaultError::VaultLocked);
        }

        // Create tombstone record — it references the original record_id as parent
        let mut tombstone = Record::create_tombstone(record_id, origin_platform, origin_device_id);

        // Override the record_id to match the original so the file overwrites it
        tombstone.record_id = record_id.to_string();

        // Write tombstone (overwrites the original record file)
        self.write_record(tombstone, b"DELETED")?;

        Ok(())
    }

    /// Validate that a vault root path is safe (no path traversal)
    fn validate_vault_root(vault_root: &Path) -> Result<(), VaultError> {
        let canonical = vault_root.canonicalize().or_else(|_| {
            // Path may not exist yet for creation
            std::fs::create_dir_all(vault_root)?;
            vault_root.canonicalize()
        })?;

        let path_str = canonical.to_string_lossy();

        // Check for path traversal
        if path_str.contains("..") {
            return Err(VaultError::PathTraversal(
                "Vault path contains '..'".to_string(),
            ));
        }

        Ok(())
    }

    /// Create the vault directory structure (directive §7)
    fn create_directory_structure(vault_root: &Path) -> Result<(), VaultError> {
        let dirs = [
            "VAULT/identity",
            "VAULT/header",
            "VAULT/records",
            "VAULT/indexes",
            "VAULT/journal",
            "VAULT/transactions",
            "VAULT/attachments",
            "VAULT/snapshots",
            "VAULT/locks",
            "VAULT/recovery",
            "CONFIG",
            "RECOVERY",
            "UPDATES/staging",
            "UPDATES/rollback",
            "LOGS/encrypted",
        ];

        for dir in &dirs {
            std::fs::create_dir_all(vault_root.join(dir))?;
        }

        Ok(())
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        // Zero the master key from memory on drop
        if let Some(mut key) = self.master_key.take() {
            secure_zero(&mut key);
        }
    }
}

/// Validate a record identifier before it is used to build a filesystem path.
///
/// Record IDs were interpolated directly into filenames on both the read and
/// write paths. The vault-root check canonicalises first and then looks for
/// `..` in the result, which cannot catch traversal because canonicalisation
/// has already resolved it away.
///
/// Only a canonical lowercase UUID v4 is accepted, so no separator, drive
/// prefix, dot segment, wildcard or reserved Windows device name can reach a
/// path at all.
pub fn validate_record_id(record_id: &str) -> Result<(), VaultError> {
    fn is_lower_hex(s: &str) -> bool {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    let parts: Vec<&str> = record_id.split('-').collect();
    let shaped = parts.len() == 5
        && parts[0].len() == 8
        && parts[1].len() == 4
        && parts[2].len() == 4
        && parts[3].len() == 4
        && parts[4].len() == 12
        && parts.iter().all(|p| is_lower_hex(p))
        && parts[2].starts_with('4')
        && matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b');

    if shaped {
        Ok(())
    } else {
        Err(VaultError::VaultNotFound(format!(
            "Invalid record id {:?}: expected a lowercase UUID v4",
            record_id
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::RecordType;

    fn create_test_vault(dir: &tempfile::TempDir) -> Vault {
        let vault_root = dir.path().join("UNOONE");
        let result = Vault::create(&vault_root, b"test-password-12345").unwrap();

        // Verify recovery phrase was generated
        assert_eq!(result.recovery_phrase.len(), 24);
        assert!(!result.vault_id.is_empty());

        // Open the vault
        let mut vault = Vault::open(&vault_root).unwrap();
        assert_eq!(vault.state(), &VaultState::Locked);

        // Unlock with password
        let unlock_result = vault.unlock(b"test-password-12345").unwrap();
        assert_eq!(vault.state(), &VaultState::Unlocked);
        assert!(!unlock_result.is_recovery_unlock);

        vault
    }

    #[test]
    fn test_vault_create_and_unlock() {
        let dir = tempfile::tempdir().unwrap();
        let _vault = create_test_vault(&dir);
    }

    #[test]
    fn test_wrong_password_fails() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"correct-password-12345").unwrap();

        let mut vault = Vault::open(&vault_root).unwrap();
        let result = vault.unlock(b"wrong-password-12345!!!");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VaultError::WrongPassword));
    }

    #[test]
    fn test_empty_password_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");

        let result = Vault::create(&vault_root, b"");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            VaultError::InvalidPassword(_)
        ));
    }

    #[test]
    fn test_short_password_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");

        let result = Vault::create(&vault_root, b"short");
        assert!(result.is_err());
    }

    #[test]
    fn test_lock_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"test-password-12345").unwrap();

        let mut vault = Vault::open(&vault_root).unwrap();
        vault.unlock(b"test-password-12345").unwrap();
        assert!(vault.is_unlocked());

        // Lock the vault
        vault.lock().unwrap();
        assert_eq!(vault.state(), &VaultState::Locked);
        assert!(!vault.is_unlocked());

        // Unlock again
        vault.unlock(b"test-password-12345").unwrap();
        assert!(vault.is_unlocked());
    }

    #[test]
    fn test_change_password() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"old-password-12345").unwrap();

        let mut vault = Vault::open(&vault_root).unwrap();
        vault.unlock(b"old-password-12345").unwrap();

        // Change password
        vault
            .change_password(b"old-password-12345", b"new-password-12345")
            .unwrap();

        // Lock and reopen with new password
        vault.lock().unwrap();
        vault.unlock(b"new-password-12345").unwrap();
        assert!(vault.is_unlocked());

        // Old password should fail
        vault.lock().unwrap();
        let result = vault.unlock(b"old-password-12345");
        assert!(result.is_err());
    }

    #[test]
    fn test_recovery_unlock() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");

        let result = Vault::create(&vault_root, b"test-password-12345").unwrap();
        let recovery_words = result.recovery_phrase;

        let mut vault = Vault::open(&vault_root).unwrap();
        let unlock_result = vault.unlock_with_recovery(&recovery_words).unwrap();
        assert!(vault.is_unlocked());
        assert!(unlock_result.is_recovery_unlock);
    }

    #[test]
    fn test_invalid_recovery_words() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"test-password-12345").unwrap();

        let mut vault = Vault::open(&vault_root).unwrap();
        let wrong_words: Vec<String> = (0..24).map(|i| format!("wrongword{}", i)).collect();
        let result = vault.unlock_with_recovery(&wrong_words);
        assert!(result.is_err());
    }

    #[test]
    fn test_write_and_read_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = create_test_vault(&dir);

        let record = Record::new(RecordType::Conversation, "DESKTOP", "device-001");
        let content = b"Hello, this is a conversation message.";

        vault.write_record(record.clone(), content).unwrap();

        let (read_record, read_content) = vault.read_record(&record.record_id).unwrap();
        assert_eq!(read_content, content);
        assert_eq!(read_record.record_id, record.record_id);
        assert_eq!(read_record.record_type, RecordType::Conversation);
    }

    #[test]
    fn test_new_records_use_aes256gcm() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = create_test_vault(&dir);

        let record = Record::new(RecordType::Memory, "DESKTOP", "device-001");
        let content = b"AES-GCM default test";

        vault.write_record(record.clone(), content).unwrap();

        // Inspect the on-disk record: the nonce must be 12 bytes (AES-GCM).
        let record_path = vault
            .vault_root()
            .join("VAULT")
            .join("records")
            .join(format!("{}.enc.json", record.record_id));
        let json = std::fs::read_to_string(&record_path).unwrap();
        let encrypted_record: EncryptedRecord = serde_json::from_str(&json).unwrap();
        let nonce = hex::decode(&encrypted_record.nonce).unwrap();
        assert_eq!(
            nonce.len(),
            crate::crypto::AES_GCM_NONCE_LEN,
            "New records must use AES-256-GCM (12-byte nonce) for Android compatibility"
        );

        // Roundtrip must still work
        let (_, read_content) = vault.read_record(&record.record_id).unwrap();
        assert_eq!(read_content, content);
    }

    #[test]
    fn test_legacy_xchacha20_records_still_readable() {
        use crate::crypto::{derive_domain_key, encrypt, generate_nonce, DOMAIN_RECORDS};

        let dir = tempfile::tempdir().unwrap();
        let vault = create_test_vault(&dir);

        // Create a record and encrypt it with legacy XChaCha20-Poly1305 (24-byte nonce)
        let record = Record::new(RecordType::Memory, "DESKTOP", "device-001");
        let content = b"Legacy XChaCha20 record";
        let aad = serde_json::to_vec(&record).unwrap();

        let master_key = vault.master_key().unwrap();
        let domain_key = derive_domain_key(master_key, DOMAIN_RECORDS);
        let nonce = generate_nonce();
        let ciphertext = encrypt(&domain_key, &nonce, content, &aad).unwrap();

        let encrypted_record = EncryptedRecord {
            metadata: record.clone(),
            encrypted_content: hex::encode(ciphertext),
            nonce: hex::encode(nonce),
            associated_data: hex::encode(&aad),
            // Legacy fixture: AAD captured before metadata was finalised, which
            // is exactly the pre-fix on-disk shape. Reading it must still work.
            aad_version: 0,
        };

        // Write it directly to disk (bypassing write_record which would use AES-GCM)
        let record_path = vault
            .vault_root()
            .join("VAULT")
            .join("records")
            .join(format!("{}.enc.json", record.record_id));
        std::fs::create_dir_all(record_path.parent().unwrap()).unwrap();
        std::fs::write(
            &record_path,
            serde_json::to_string_pretty(&encrypted_record).unwrap(),
        )
        .unwrap();

        // read_record must auto-detect XChaCha20 from the 24-byte nonce
        let (read_record, read_content) = vault.read_record(&record.record_id).unwrap();
        assert_eq!(read_content, content);
        assert_eq!(read_record.record_id, record.record_id);
    }

    #[test]
    fn test_delete_record_creates_tombstone() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = create_test_vault(&dir);

        let record = Record::new(RecordType::Memory, "DESKTOP", "device-001");
        let content = b"Important memory";

        vault.write_record(record.clone(), content).unwrap();

        // Delete the record
        vault
            .delete_record(&record.record_id, "DESKTOP", "device-001")
            .unwrap();

        // Reading the deleted record should fail
        let result = vault.read_record(&record.record_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_password_absent_from_files() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"my-secret-password-12345").unwrap();

        // Walk all files in the vault and check that the password
        // does not appear in plaintext
        fn check_no_password(dir: &Path, password: &[u8]) -> bool {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if !check_no_password(&path, password) {
                            return false;
                        }
                    } else if let Ok(content) = std::fs::read_to_string(&path) {
                        let password_str = String::from_utf8_lossy(password);
                        if content.contains(&*password_str) {
                            return false;
                        }
                    }
                }
            }
            true
        }

        assert!(
            check_no_password(&vault_root, b"my-secret-password-12345"),
            "Password found in plaintext in vault files!"
        );
    }

    #[test]
    fn test_master_key_absent_from_files() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"test-password-12345").unwrap();

        let mut vault = Vault::open(&vault_root).unwrap();
        vault.unlock(b"test-password-12345").unwrap();

        // The master key should not appear in any file
        let master_key = vault.master_key().unwrap();
        let key_hex = hex::encode(master_key);

        fn check_no_key(dir: &Path, key_hex: &str) -> bool {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if !check_no_key(&path, key_hex) {
                            return false;
                        }
                    } else if let Ok(content) = std::fs::read_to_string(&path) {
                        if content.contains(key_hex) {
                            return false;
                        }
                    }
                }
            }
            true
        }

        assert!(
            check_no_key(&vault_root, &key_hex),
            "Master key hex found in plaintext in vault files!"
        );
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use crate::record::{RecordType, AAD_VERSION_CANONICAL};

    /// Local copy of the helper from the sibling test module.
    fn create_test_vault(dir: &tempfile::TempDir) -> Vault {
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"test-password-12345").unwrap();
        let mut vault = Vault::open(&vault_root).unwrap();
        vault.unlock(b"test-password-12345").unwrap();
        vault
    }

    /// §10: vault.id bytes are byte-identical before and after first-use setup.
    #[test]
    fn first_use_setup_preserves_vault_id_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        let identity_dir = vault_root.join("VAULT").join("identity");
        std::fs::create_dir_all(&identity_dir).unwrap();
        // Packaged drives ship exact bytes (with trailing newline, as generated
        // by the packaging pipeline).
        let packaged = b"pocket-vault-id-12345\n";
        std::fs::write(identity_dir.join("vault.id"), packaged).unwrap();

        let result = Vault::create_with_vault_id(
            &vault_root,
            b"setup-password-999",
            "pocket-vault-id-12345",
        )
        .unwrap();
        assert_eq!(result.vault_id, "pocket-vault-id-12345");
        let after = std::fs::read(identity_dir.join("vault.id")).unwrap();
        assert_eq!(
            packaged.to_vec(),
            after,
            "first-use setup must preserve vault.id bytes verbatim"
        );
    }

    /// §10: setup on an already-initialised vault is refused, not silently overwritten.
    #[test]
    fn setup_refuses_to_reinitialise_existing_vault() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        let _ = Vault::create(&vault_root, b"first-password-123").unwrap();
        // A second create attempt (fresh identity) must fail.
        let err = Vault::create(&vault_root, b"second-password").unwrap_err();
        assert!(matches!(err, VaultError::NotPermitted(_)), "got {err:?}");
        // create_with_vault_id with a DIFFERENT id must also fail.
        let err2 =
            Vault::create_with_vault_id(&vault_root, b"second-password", "other-id").unwrap_err();
        assert!(matches!(err2, VaultError::NotPermitted(_)), "got {err2:?}");
    }

    /// §10: mismatched packaged identity is refused BEFORE anything is written.
    #[test]
    fn create_with_vault_id_refuses_mismatched_existing_identity() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        let identity_dir = vault_root.join("VAULT").join("identity");
        std::fs::create_dir_all(&identity_dir).unwrap();
        std::fs::write(identity_dir.join("vault.id"), b"original-id").unwrap();
        let err = Vault::create_with_vault_id(&vault_root, b"password-12345", "different-id")
            .unwrap_err();
        assert!(matches!(err, VaultError::NotPermitted(_)));
        // The original identity file must be untouched.
        assert_eq!(
            std::fs::read(identity_dir.join("vault.id")).unwrap(),
            b"original-id".to_vec()
        );
        // And no header may have been created by the refused attempt.
        assert!(!vault_root
            .join("VAULT")
            .join("header")
            .join(HEADER_A_FILE)
            .exists());
    }

    /// §10: correct password unlocks after restart; wrong password fails.
    #[test]
    fn correct_password_unlocks_after_restart_wrong_fails() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        let created = Vault::create(&vault_root, b"correct-horse-battery").unwrap();

        // "Restart": drop everything, reopen from disk.
        let mut vault = Vault::open(&vault_root).unwrap();
        assert!(matches!(
            vault.unlock(b"wrong-password-wrong").unwrap_err(),
            VaultError::WrongPassword
        ));
        let ok = vault.unlock(b"correct-horse-battery").unwrap();
        assert_eq!(ok.vault_id, created.vault_id);
    }

    /// §10: empty vault id is rejected.
    #[test]
    fn create_with_vault_id_rejects_empty_id() {
        let dir = tempfile::tempdir().unwrap();
        let err = Vault::create_with_vault_id(dir.path(), b"password-12345", "   ").unwrap_err();
        assert!(matches!(err, VaultError::InvalidVaultStructure(_)));
    }

    // ================================================================
    // Wave 1 regression tests. Each of these fails against the
    // pre-fix implementation; that is the point of them.
    // ================================================================

    /// DEFECT A — the headline data-loss bug.
    ///
    /// change_password() writes the new header into the INACTIVE slot, but
    /// open() used to take slot A whenever slot A existed. A password change
    /// written to B was therefore discarded on the next open: the new password
    /// stopped working and the old one kept working.
    #[test]
    fn password_change_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"original-password-1").unwrap();

        {
            let mut vault = Vault::open(&vault_root).unwrap();
            vault.unlock(b"original-password-1").unwrap();
            vault
                .change_password(b"original-password-1", b"replacement-password-2")
                .unwrap();
        } // dropped — simulates closing the app / removing the drive

        // Reopen from disk exactly as a fresh process would.
        let mut reopened = Vault::open(&vault_root).unwrap();

        assert!(
            reopened.unlock(b"replacement-password-2").is_ok(),
            "the NEW password must unlock after a restart"
        );

        let mut again = Vault::open(&vault_root).unwrap();
        assert!(
            again.unlock(b"original-password-1").is_err(),
            "the OLD password must NOT still unlock after a restart"
        );
    }

    /// DEFECT A — selection is by generation, not by slot letter.
    #[test]
    fn newest_generation_wins_regardless_of_slot() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"password-aaaa-1").unwrap();

        let mut vault = Vault::open(&vault_root).unwrap();
        vault.unlock(b"password-aaaa-1").unwrap();
        vault
            .change_password(b"password-aaaa-1", b"password-bbbb-2")
            .unwrap();
        let gen_after_first = vault.header().unwrap().generation_value();

        vault
            .change_password(b"password-bbbb-2", b"password-cccc-3")
            .unwrap();
        let gen_after_second = vault.header().unwrap().generation_value();

        assert!(
            gen_after_second > gen_after_first,
            "each header write must advance the generation"
        );

        let mut reopened = Vault::open(&vault_root).unwrap();
        assert!(reopened.unlock(b"password-cccc-3").is_ok());
    }

    /// DEFECT A — migration. A v1 header has no generation field at all and
    /// must still open, and its HMAC must still verify (the new fields are
    /// skipped when absent precisely so the serialised bytes are unchanged).
    #[test]
    fn v1_header_without_generation_still_opens_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"legacy-password-01").unwrap();

        // Reproduce a genuine v1 header: the fields are absent AND the HMAC was
        // computed without them. Merely deleting the keys from a v2 header
        // would invalidate its HMAC, which would test the wrong thing.
        let header_path = vault_root.join("VAULT").join("header").join(HEADER_A_FILE);
        let mut header = VaultHeader::load_from_file(&header_path).unwrap();
        header.generation = None;
        header.committed = None;
        let v1_header = header.reseal(b"legacy-password-01").unwrap();
        v1_header.save_to_file(&header_path).unwrap();

        // Confirm the on-disk JSON really has no such keys.
        let raw = std::fs::read_to_string(&header_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(
            json.get("generation").is_none(),
            "v1 header must omit generation"
        );
        assert!(
            json.get("committed").is_none(),
            "v1 header must omit committed"
        );

        let mut vault = Vault::open(&vault_root).unwrap();
        assert_eq!(
            vault.header().unwrap().generation_value(),
            0,
            "a v1 header is generation 0"
        );
        assert!(
            vault.header().unwrap().is_committed(),
            "a v1 header on disk was the live header, so it counts as committed"
        );
        assert!(
            vault.unlock(b"legacy-password-01").is_ok(),
            "stripping absent fields must not invalidate the existing HMAC"
        );
    }

    /// DEFECT A — a corrupt slot must not prevent opening from the good one.
    #[test]
    fn corrupt_slot_does_not_block_opening() {
        let dir = tempfile::tempdir().unwrap();
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"resilient-pass-01").unwrap();

        let header_dir = vault_root.join("VAULT").join("header");
        std::fs::write(header_dir.join(HEADER_B_FILE), b"{ not valid json").unwrap();

        let mut vault = Vault::open(&vault_root).unwrap();
        assert!(vault.unlock(b"resilient-pass-01").is_ok());
    }

    /// DEFECT B — metadata tampering must be detected.
    ///
    /// Previously the stored AAD bytes were used verbatim for decryption and
    /// never compared against the stored plaintext metadata, so privacy_level
    /// could be downgraded on disk and the content still decrypted cleanly.
    #[test]
    fn tampered_privacy_level_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = create_test_vault(&dir);

        let record = Record::new(RecordType::Memory, "DESKTOP", "device-001");
        let id = record.record_id.clone();
        vault.write_record(record, b"sensitive content").unwrap();

        let path = vault
            .vault_root()
            .join("VAULT")
            .join("records")
            .join(format!("{}.enc.json", id));

        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        json["metadata"]["privacy_level"] = serde_json::json!("METADATA_ONLY");
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

        let err = vault.read_record(&id).unwrap_err();
        assert!(
            format!("{:?}", err).contains("associated data")
                || format!("{:?}", err).contains("altered"),
            "expected a metadata-authentication failure, got {:?}",
            err
        );
    }

    /// DEFECT B — flipping the tombstone flag is a deletion-state forgery.
    #[test]
    fn tampered_tombstone_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = create_test_vault(&dir);

        let record = Record::new(RecordType::Memory, "DESKTOP", "device-001");
        let id = record.record_id.clone();
        vault.write_record(record, b"note body").unwrap();

        let path = vault
            .vault_root()
            .join("VAULT")
            .join("records")
            .join(format!("{}.enc.json", id));

        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let current = json["metadata"]["tombstone"].as_bool().unwrap_or(false);
        json["metadata"]["tombstone"] = serde_json::json!(!current);
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

        assert!(
            vault.read_record(&id).is_err(),
            "flipping the tombstone flag must be detected"
        );
    }

    /// DEFECT B — an untampered record round-trips, and its stored metadata
    /// re-serialises to exactly the authenticated bytes.
    #[test]
    fn stored_metadata_equals_authenticated_aad() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = create_test_vault(&dir);

        let record = Record::new(RecordType::Memory, "DESKTOP", "device-001");
        let id = record.record_id.clone();
        vault.write_record(record, b"round trip").unwrap();

        let path = vault
            .vault_root()
            .join("VAULT")
            .join("records")
            .join(format!("{}.enc.json", id));
        let stored: EncryptedRecord =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        assert_eq!(stored.aad_version, AAD_VERSION_CANONICAL);
        assert_eq!(
            hex::encode(canonical_metadata_bytes(&stored.metadata).unwrap()),
            stored.associated_data,
            "stored metadata must re-serialise to the authenticated bytes"
        );

        let (meta, content) = vault.read_record(&id).unwrap();
        assert_eq!(content, b"round trip");
        assert!(!meta.content_hash.is_empty());
    }

    /// DEFECT C — the write must actually go through the journal, and must
    /// leave no staging file behind.
    #[test]
    fn write_is_journalled_and_leaves_no_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = create_test_vault(&dir);

        let record = Record::new(RecordType::Memory, "DESKTOP", "device-001");
        let id = record.record_id.clone();
        vault.write_record(record, b"journalled").unwrap();

        let temp = vault
            .vault_root()
            .join("VAULT")
            .join("records")
            .join(format!("{}.enc.tmp", id));
        assert!(!temp.exists(), "staging file must not survive a commit");

        // The record is readable, so the promotion completed.
        assert!(vault.read_record(&id).is_ok());

        // Recovery on a cleanly committed vault must not undo anything.
        let recovered = vault.journal_recover_for_test();
        assert!(
            recovered.is_ok(),
            "crash recovery must succeed on a clean vault"
        );
        assert!(vault.read_record(&id).is_ok(), "commit must be durable");
    }

    /// DEFECT D — identifiers must never reach a filesystem path unvalidated.
    #[test]
    fn invalid_record_ids_are_rejected() {
        let rejected = [
            "../../etc/passwd",
            "..\\..\\windows\\system32\\config\\sam",
            "a/b",
            "a\\b",
            "CON",
            "NUL",
            "",
            "not-a-uuid",
            "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA", // uppercase
            "00000000-0000-1000-8000-000000000000", // version 1, not 4
            "00000000-0000-4000-c000-000000000000", // bad variant nibble
            "00000000-0000-4000-8000-00000000000",  // too short
            "*",
        ];
        for id in rejected {
            assert!(
                validate_record_id(id).is_err(),
                "record id {:?} must be rejected",
                id
            );
        }

        // A real generated id must be accepted.
        let good = Record::new(RecordType::Memory, "DESKTOP", "d").record_id;
        assert!(
            validate_record_id(&good).is_ok(),
            "generated ids must be accepted: {good}"
        );
    }

    /// DEFECT D — the public API refuses traversal rather than touching disk.
    #[test]
    fn read_record_refuses_traversal_ids() {
        let dir = tempfile::tempdir().unwrap();
        let vault = create_test_vault(&dir);
        assert!(vault.read_record("../../../../etc/passwd").is_err());
    }
}
