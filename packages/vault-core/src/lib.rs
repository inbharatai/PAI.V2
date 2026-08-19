// UnoOne Vault Core — Encrypted vault library
// Argon2id + XChaCha20-Poly1305 + HKDF-SHA-256
// Password-only authentication, no username, no email, no cloud
//
// This library implements the vault encryption layer specified in
// the UnoOne Pocket AI production directive (sections 15–17).
// It must produce byte-identical results to the Kotlin encrypted-vault
// package for cross-platform vault compatibility.

pub mod bip39_words;
pub mod crypto;
pub mod error;
pub mod header;
pub mod journal;
pub mod record;
pub mod recovery;
pub mod vault;

pub use error::VaultError;
pub use record::{EncryptedRecord, PrivacyLevel, Record, RecordType};
pub use vault::Vault;
