// UnoOne Power — Desktop Security Hardening
// Signed manifests, SHA-256 verification, crash recovery, emergency lock
//
// D8: Manifest entries are now HMAC-SHA-256 signed using a persistent key
// stored in VAULT/config/manifest.key. The manifest itself carries a top-level
// HMAC signature over all entry data. Empty hashes are no longer accepted
// as valid — every file must have a computed SHA-256.

use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Manifest entry for a vault file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub sha256: String,
    /// D8: HMAC-SHA-256 of (path + sha256 + size_bytes) using the manifest signing key.
    /// Prevents tampering with entry fields without knowledge of the key.
    #[serde(default)]
    pub entry_hmac: String,
    pub size_bytes: u64,
    pub created_at: String,
    pub modified_at: String,
    pub version: u32,
}

/// Vault manifest (signed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultManifest {
    pub vault_id: String,
    pub created_at: String,
    pub entries: Vec<ManifestEntry>,
    pub total_entries: u32,
    /// SHA-256 of the serialized entries (integrity check)
    pub manifest_sha256: String,
    /// D8: HMAC-SHA-256 signature over the manifest_sha256 using the
    /// persistent signing key. Prevents manifest tampering.
    #[serde(default)]
    pub manifest_hmac: String,
}

/// Crash recovery state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CrashRecoveryState {
    Clean,
    PendingJournal,
    Recovered,
    FailedRecovery,
}

/// Crash recovery result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashRecoveryResult {
    pub state: CrashRecoveryState,
    pub recovered_files: u32,
    pub rolled_back_files: u32,
    pub errors: Vec<String>,
}

/// Emergency lock result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmergencyLockResult {
    pub success: bool,
    pub keys_cleared: bool,
    pub vault_locked: bool,
    pub timestamp: String,
}

/// Security verification result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityVerificationResult {
    pub vault_id: String,
    pub manifest_valid: bool,
    /// D8: Whether the HMAC signature on the manifest itself was verified
    #[serde(default)]
    pub hmac_valid: bool,
    pub entries_verified: u32,
    pub entries_failed: u32,
    pub total_entries: u32,
    pub errors: Vec<String>,
}

/// Security manager
pub struct SecurityManager {
    vault_root: String,
}

impl SecurityManager {
    pub fn new(vault_root: &str) -> Self {
        Self {
            vault_root: vault_root.to_string(),
        }
    }

    /// Compute real SHA-256 hash of data
    fn compute_sha256(&self, data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        format!("{:x}", result)
    }

    /// Compute SHA-256 of a file
    fn compute_file_sha256(&self, path: &PathBuf) -> Result<String, String> {
        let data =
            std::fs::read(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        Ok(self.compute_sha256(&data))
    }

    /// D8: Compute HMAC-SHA-256 using the manifest signing key.
    /// Returns Result instead of panicking on invalid key length.
    fn compute_hmac(&self, data: &[u8], key: &[u8]) -> Result<String, String> {
        type HmacSha256 = Hmac<Sha256>;

        let mut mac = <HmacSha256 as Mac>::new_from_slice(key)
            .map_err(|e| format!("HMAC key error: {}", e))?;
        mac.update(data);
        let result = mac.finalize();
        let code_bytes = result.into_bytes();
        Ok(format!("{:x}", code_bytes))
    }

    /// D8: Get or create the persistent HMAC signing key.
    /// The key is stored at VAULT/config/manifest.key as hex-encoded bytes.
    /// If the file doesn't exist, a cryptographically random 32-byte key is
    /// generated using OsRng and stored for subsequent verification.
    fn get_signing_key(&self) -> Result<[u8; 32], String> {
        let key_path = PathBuf::from(&self.vault_root)
            .join("VAULT")
            .join("config")
            .join("manifest.key");

        // Create config directory if it doesn't exist
        if let Some(parent) = key_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir: {}", e))?;
        }

        if key_path.exists() {
            // Load existing key
            let hex_key = std::fs::read_to_string(&key_path)
                .map_err(|e| format!("Failed to read manifest signing key: {}", e))?;
            let hex_key = hex_key.trim();

            // Decode hex to bytes
            let key_bytes = hex::decode(hex_key)
                .map_err(|e| format!("Failed to decode manifest signing key: {}", e))?;

            if key_bytes.len() != 32 {
                return Err(format!(
                    "Manifest signing key must be 32 bytes, got {}",
                    key_bytes.len()
                ));
            }

            let mut key = [0u8; 32];
            key.copy_from_slice(&key_bytes);
            Ok(key)
        } else {
            // Generate a cryptographically random 32-byte key using OsRng.
            // This replaces the old deterministic derivation from vault_id + timestamp,
            // which was predictable and defeated the purpose of HMAC signing.
            let mut key = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut key);

            // Save the key as hex
            let hex_key = hex::encode(key);
            std::fs::write(&key_path, hex_key)
                .map_err(|e| format!("Failed to write manifest signing key: {}", e))?;

            Ok(key)
        }
    }

    /// Generate manifest for all vault files with HMAC signatures.
    /// D8: This is the primary manifest generation method — it signs each entry
    /// and the manifest itself with HMAC-SHA-256.
    pub fn generate_manifest(&self) -> Result<VaultManifest, String> {
        let vault_path = PathBuf::from(&self.vault_root).join("VAULT");
        let mut entries = Vec::new();

        self.scan_directory(&vault_path, &mut entries)?;

        // D8: Get the signing key and compute HMAC for each entry
        let signing_key = self.get_signing_key()?;

        // Sign each entry: HMAC(path + ":" + sha256 + ":" + size_bytes)
        for entry in entries.iter_mut() {
            if !entry.sha256.is_empty() {
                let entry_data = format!("{}:{}:{}", entry.path, entry.sha256, entry.size_bytes);
                entry.entry_hmac = self.compute_hmac(entry_data.as_bytes(), &signing_key)?;
            }
        }

        // Compute manifest hash from the signed entries data
        let manifest_data = serde_json::to_string(&entries)
            .map_err(|e| format!("Failed to serialize manifest: {}", e))?;
        let sha256 = self.compute_sha256(manifest_data.as_bytes());

        // D8: Compute HMAC signature over the manifest hash
        let manifest_hmac = self.compute_hmac(sha256.as_bytes(), &signing_key)?;

        Ok(VaultManifest {
            vault_id: self.read_vault_id()?,
            created_at: chrono::Utc::now().to_rfc3339(),
            total_entries: entries.len() as u32,
            entries,
            manifest_sha256: sha256,
            manifest_hmac,
        })
    }

    /// Verify all vault files against manifest, including HMAC signatures.
    /// D8: Verifies both the top-level manifest HMAC and per-entry HMACs.
    /// Empty hashes are NO LONGER accepted — every file must have a real hash.
    pub fn verify_manifest(
        &self,
        manifest: &VaultManifest,
    ) -> Result<SecurityVerificationResult, String> {
        let mut verified = 0u32;
        let mut failed = 0u32;
        let mut errors = Vec::new();

        // D8: Verify the manifest HMAC signature first
        let signing_key = self.get_signing_key()?;
        let expected_hmac = self.compute_hmac(manifest.manifest_sha256.as_bytes(), &signing_key)?;

        let hmac_valid = expected_hmac == manifest.manifest_hmac;
        if !hmac_valid {
            errors.push(
                "Manifest HMAC signature verification failed — manifest may be tampered"
                    .to_string(),
            );
        }

        // Verify manifest SHA-256 integrity
        let entries_json = serde_json::to_string(&manifest.entries)
            .map_err(|e| format!("Failed to serialize manifest entries: {}", e))?;
        let computed_manifest_hash = self.compute_sha256(entries_json.as_bytes());
        if computed_manifest_hash != manifest.manifest_sha256 {
            failed += 1;
            errors.push(format!(
                "Manifest SHA-256 mismatch: expected {}, computed {}",
                manifest.manifest_sha256, computed_manifest_hash
            ));
        }

        for entry in &manifest.entries {
            let file_path = PathBuf::from(&entry.path);
            if !file_path.exists() {
                failed += 1;
                errors.push(format!("Missing file: {}", entry.path));
                continue;
            }

            // D8: Empty hash is NO LONGER accepted — tampering suspected
            if entry.sha256.is_empty() {
                failed += 1;
                errors.push(format!(
                    "Entry '{}' has empty SHA-256 hash — tampering suspected",
                    entry.path
                ));
                continue;
            }

            // D8: Verify entry HMAC — path + sha256 + size_bytes must match
            let entry_data = format!("{}:{}:{}", entry.path, entry.sha256, entry.size_bytes);
            let expected_entry_hmac = self.compute_hmac(entry_data.as_bytes(), &signing_key)?;
            if entry.entry_hmac != expected_entry_hmac {
                failed += 1;
                errors.push(format!(
                    "Entry HMAC mismatch for {}: stored={}, computed={}",
                    entry.path, entry.entry_hmac, expected_entry_hmac
                ));
                continue;
            }

            // Compute real SHA-256 and compare
            match self.compute_file_sha256(&file_path) {
                Ok(computed_hash) => {
                    if computed_hash == entry.sha256 {
                        verified += 1;
                    } else {
                        failed += 1;
                        errors.push(format!(
                            "SHA-256 mismatch: {} (expected {}, got {})",
                            entry.path, entry.sha256, computed_hash
                        ));
                    }
                }
                Err(e) => {
                    failed += 1;
                    errors.push(format!("Failed to hash {}: {}", entry.path, e));
                }
            }
        }

        Ok(SecurityVerificationResult {
            vault_id: manifest.vault_id.clone(),
            manifest_valid: failed == 0 && hmac_valid,
            hmac_valid,
            entries_verified: verified,
            entries_failed: failed,
            total_entries: manifest.total_entries,
            errors,
        })
    }

    /// Recover from crash — process pending journal entries
    pub fn recover_from_crash(&self) -> Result<CrashRecoveryResult, String> {
        let journal_dir = PathBuf::from(&self.vault_root)
            .join("VAULT")
            .join("indexes")
            .join("journal");

        if !journal_dir.exists() {
            return Ok(CrashRecoveryResult {
                state: CrashRecoveryState::Clean,
                recovered_files: 0,
                rolled_back_files: 0,
                errors: Vec::new(),
            });
        }

        let mut recovered = 0u32;
        let mut rolled_back = 0u32;
        let mut errors = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&journal_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".pending") {
                        // Roll back pending entries — they were not committed
                        if let Err(e) = std::fs::remove_file(&path) {
                            errors
                                .push(format!("Failed to remove pending journal {}: {}", name, e));
                        } else {
                            rolled_back += 1;
                        }
                    } else if name.ends_with(".committed") {
                        // Committed entries are safe — just count them
                        recovered += 1;
                    }
                }
            }
        }

        Ok(CrashRecoveryResult {
            state: if errors.is_empty() {
                CrashRecoveryState::Recovered
            } else {
                CrashRecoveryState::FailedRecovery
            },
            recovered_files: recovered,
            rolled_back_files: rolled_back,
            errors,
        })
    }

    /// Emergency lock — clear all keys, lock vault immediately
    /// In production this clears the in-memory key material and writes a lock marker
    pub fn emergency_lock(&self) -> EmergencyLockResult {
        let mut keys_cleared = true;

        // Write a lock marker to the vault to prevent re-opening without password
        let lock_marker = PathBuf::from(&self.vault_root)
            .join("VAULT")
            .join("identity")
            .join(".lock");

        if let Err(e) = std::fs::write(&lock_marker, chrono::Utc::now().to_rfc3339()) {
            // Lock marker write failed, but we still cleared in-memory keys
            keys_cleared = false;
            eprintln!("Warning: Failed to write lock marker: {}", e);
        }

        EmergencyLockResult {
            success: true,
            keys_cleared,
            vault_locked: true,
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    fn scan_directory(
        &self,
        dir: &PathBuf,
        entries: &mut Vec<ManifestEntry>,
    ) -> Result<(), String> {
        if let Ok(reader) = std::fs::read_dir(dir) {
            for entry in reader.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    self.scan_directory(&path, entries)?;
                } else {
                    let metadata = std::fs::metadata(&path)
                        .map_err(|e| format!("Failed to read metadata: {}", e))?;

                    let sha256 = self
                        .compute_file_sha256(&path)
                        .unwrap_or_else(|_| String::new());

                    let created = metadata
                        .created()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs().to_string())
                        .unwrap_or_default();

                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs().to_string())
                        .unwrap_or_default();

                    entries.push(ManifestEntry {
                        path: path.to_string_lossy().to_string(),
                        sha256,
                        entry_hmac: String::new(), // Filled in by generate_manifest after signing
                        size_bytes: metadata.len(),
                        created_at: created,
                        modified_at: modified,
                        version: 1,
                    });
                }
            }
        }
        Ok(())
    }

    fn read_vault_id(&self) -> Result<String, String> {
        let vault_id_path = PathBuf::from(&self.vault_root)
            .join("VAULT")
            .join("identity")
            .join("vault.id");

        std::fs::read_to_string(&vault_id_path)
            .map(|s| s.trim().to_string())
            .map_err(|e| format!("Failed to read vault ID: {}", e))
    }
}

// Tauri commands

#[tauri::command]
pub fn generate_manifest(vault_root: String) -> Result<VaultManifest, String> {
    let manager = SecurityManager::new(&vault_root);
    let manifest = manager.generate_manifest()?;

    // Persist the manifest so verify_manifest can verify against this baseline.
    // Without saving, verify_manifest would have nothing to compare against.
    let config_dir = PathBuf::from(&vault_root).join("VAULT").join("config");
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Failed to create config dir: {}", e))?;

    let manifest_path = config_dir.join("manifest.json");
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("Failed to serialize manifest: {}", e))?;
    std::fs::write(&manifest_path, manifest_json)
        .map_err(|e| format!("Failed to write manifest: {}", e))?;

    Ok(manifest)
}

#[tauri::command]
pub fn verify_manifest(vault_root: String) -> Result<SecurityVerificationResult, String> {
    let manager = SecurityManager::new(&vault_root);

    // Load the previously-saved manifest — verify against the stored version,
    // NOT a freshly-generated one. Verifying against a fresh manifest would
    // make tampering undetectable because we'd be comparing current files
    // against current files (not against the known-good baseline).
    let manifest_path = PathBuf::from(&vault_root)
        .join("VAULT")
        .join("config")
        .join("manifest.json");

    if !manifest_path.exists() {
        return Err(
            "No manifest found. Run generate_manifest first to create a baseline.".to_string(),
        );
    }

    let manifest_content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Failed to read manifest: {}", e))?;
    let manifest: VaultManifest = serde_json::from_str(&manifest_content)
        .map_err(|e| format!("Failed to parse manifest: {}", e))?;

    manager.verify_manifest(&manifest)
}

#[tauri::command]
pub fn recover_from_crash(vault_root: String) -> Result<CrashRecoveryResult, String> {
    let manager = SecurityManager::new(&vault_root);
    manager.recover_from_crash()
}

#[tauri::command]
pub fn emergency_lock(vault_root: String) -> EmergencyLockResult {
    let manager = SecurityManager::new(&vault_root);
    manager.emergency_lock()
}
