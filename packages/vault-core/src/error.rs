// UnoOne Vault Core — Error types

use std::fmt;

/// All vault operations return Result<T, VaultError>
#[derive(Debug)]
pub enum VaultError {
    /// Password was incorrect (argon2id derivation succeeded but header authentication failed)
    WrongPassword,

    /// Password was empty or too short
    InvalidPassword(String),

    /// Recovery words are invalid or checksum failed
    InvalidRecoveryWords(String),

    /// Vault header is corrupted or tampered
    HeaderCorrupted(String),

    /// Vault header version is not supported
    UnsupportedVersion(u32),

    /// Record decryption or authentication failed
    DecryptionFailed(String),

    /// Record encryption failed
    EncryptionFailed(String),

    /// Vault is already locked
    VaultLocked,

    /// Vault is already unlocked
    VaultUnlocked,

    /// Journal recovery failed
    JournalRecoveryFailed(String),

    /// IO error (file not found, permission denied, etc.)
    Io(std::io::Error),

    /// Serialization error
    Serialization(String),

    /// Cryptographic error
    Crypto(String),

    /// The vault directory structure is invalid
    InvalidVaultStructure(String),

    /// Vault not found at the specified path
    VaultNotFound(String),

    /// Path traversal detected
    PathTraversal(String),

    /// Operation not permitted in current state
    NotPermitted(String),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VaultError::WrongPassword => write!(f, "Wrong password"),
            VaultError::InvalidPassword(msg) => write!(f, "Invalid password: {}", msg),
            VaultError::InvalidRecoveryWords(msg) => write!(f, "Invalid recovery words: {}", msg),
            VaultError::HeaderCorrupted(msg) => write!(f, "Vault header corrupted: {}", msg),
            VaultError::UnsupportedVersion(v) => write!(f, "Unsupported vault version: {}", v),
            VaultError::DecryptionFailed(msg) => write!(f, "Decryption failed: {}", msg),
            VaultError::EncryptionFailed(msg) => write!(f, "Encryption failed: {}", msg),
            VaultError::VaultLocked => write!(f, "Vault is locked"),
            VaultError::VaultUnlocked => write!(f, "Vault is already unlocked"),
            VaultError::JournalRecoveryFailed(msg) => write!(f, "Journal recovery failed: {}", msg),
            VaultError::Io(e) => write!(f, "IO error: {}", e),
            VaultError::Serialization(msg) => write!(f, "Serialization error: {}", msg),
            VaultError::Crypto(msg) => write!(f, "Cryptographic error: {}", msg),
            VaultError::InvalidVaultStructure(msg) => write!(f, "Invalid vault structure: {}", msg),
            VaultError::VaultNotFound(msg) => write!(f, "Vault not found: {}", msg),
            VaultError::PathTraversal(msg) => write!(f, "Path traversal detected: {}", msg),
            VaultError::NotPermitted(msg) => write!(f, "Not permitted: {}", msg),
        }
    }
}

impl std::error::Error for VaultError {}

impl From<std::io::Error> for VaultError {
    fn from(e: std::io::Error) -> Self {
        VaultError::Io(e)
    }
}

impl From<serde_json::Error> for VaultError {
    fn from(e: serde_json::Error) -> Self {
        VaultError::Serialization(e.to_string())
    }
}
