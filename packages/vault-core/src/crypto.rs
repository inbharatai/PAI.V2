// UnoOne Vault Core — Cryptographic primitives
// Argon2id key derivation, XChaCha20-Poly1305 authenticated encryption,
// HKDF-SHA-256 domain key derivation, secure random generation
//
// Design (directive §16):
//   Password → Argon2id → Key-encryption key → Wraps random vault master key
//   This allows password changes without re-encrypting every record.
//
//   Domain keys are derived from the master key via HKDF-SHA-256
//   so each vault domain (records, journal, indexes, etc.) has its own key.

use aes_gcm::{aead::Payload as AesPayload, Aes256Gcm, Nonce as AesNonce};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload as ChaChaPayload},
    XChaCha20Poly1305, XNonce,
};
use hkdf::Hkdf;
use hmac::Hmac;
use rand::RngCore;
use sha2::Sha256;
use zeroize::Zeroize;

use crate::error::VaultError;

/// Argon2id parameters for vault key derivation
/// Matches the Kotlin encrypted-vault specification:
/// memory: 256 MiB, iterations: 3, parallelism: 4
///
/// In test builds we use much lower parameters so the test suite finishes in
/// a reasonable time. Production parameters are unchanged.
#[cfg(not(test))]
pub const ARGON2_MEMORY: u32 = 256 * 1024; // 256 MiB in KiB
#[cfg(test)]
pub const ARGON2_MEMORY: u32 = 8 * 1024; // 8 MiB in KiB for fast tests
#[cfg(not(test))]
pub const ARGON2_ITERATIONS: u32 = 3;
#[cfg(test)]
pub const ARGON2_ITERATIONS: u32 = 1;
pub const ARGON2_PARALLELISM: u32 = 4;

/// Production Argon2id parameters — the cross-platform contract with the Kotlin
/// `encrypted-vault` package.
///
/// These are deliberately NOT `cfg`-swapped. `ARGON2_MEMORY` and
/// `ARGON2_ITERATIONS` above drop to tiny values under `cfg(test)` so the suite
/// runs quickly, which means **no test ever exercises the production
/// parameters**. Android/Windows vault interop depends on both platforms
/// deriving identical keys, so a silent edit to either side would break
/// cross-device unlock in the field while every test stayed green.
///
/// The `const` assertion below makes any such drift a compile error in
/// non-test builds.
pub const SPEC_ARGON2_MEMORY_KIB: u32 = 256 * 1024;
pub const SPEC_ARGON2_ITERATIONS: u32 = 3;
pub const SPEC_ARGON2_PARALLELISM: u32 = 4;

#[cfg(not(test))]
const _KDF_MATCHES_CROSS_PLATFORM_SPEC: () = {
    assert!(
        ARGON2_MEMORY == SPEC_ARGON2_MEMORY_KIB,
        "Argon2id memory cost drifted from the Kotlin vault spec; Android and Windows would derive different keys"
    );
    assert!(
        ARGON2_ITERATIONS == SPEC_ARGON2_ITERATIONS,
        "Argon2id iteration count drifted from the Kotlin vault spec; Android and Windows would derive different keys"
    );
    assert!(
        ARGON2_PARALLELISM == SPEC_ARGON2_PARALLELISM,
        "Argon2id parallelism drifted from the Kotlin vault spec; Android and Windows would derive different keys"
    );
};

/// Key sizes in bytes
pub const MASTER_KEY_LEN: usize = 32; // 256-bit
pub const KEY_ENCRYPTION_KEY_LEN: usize = 32; // 256-bit
pub const SALT_LEN: usize = 32; // 256-bit
pub const NONCE_LEN: usize = 24; // 192-bit for XChaCha20
pub const TAG_LEN: usize = 16; // 128-bit Poly1305 tag
pub const RECOVERY_SECRET_LEN: usize = 32; // 256-bit

/// AES-256-GCM specific sizes
pub const AES_GCM_NONCE_LEN: usize = 12; // 96-bit standard GCM nonce
pub const AES_GCM_TAG_LEN: usize = 16; // 128-bit GCM tag

/// Vault domain names for domain-specific key derivation
pub const DOMAIN_RECORDS: &str = "records";
pub const DOMAIN_JOURNAL: &str = "journal";
pub const DOMAIN_INDEXES: &str = "indexes";
pub const DOMAIN_ATTACHMENTS: &str = "attachments";
pub const DOMAIN_CONFIG: &str = "config";
pub const DOMAIN_HEADER: &str = "header";

/// Canonical record cipher selection.
///
/// AES-256-GCM is the cross-platform default because Android has
/// hardware-accelerated AES-GCM and can read/write it efficiently.
/// XChaCha20-Poly1305 is kept for backward compatibility with older
/// desktop-only vaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherAlgorithm {
    Aes256Gcm,
    XChaCha20Poly1305,
}

impl CipherAlgorithm {
    /// Detect the cipher from the on-disk nonce length.
    /// AES-GCM uses a 12-byte nonce; XChaCha20 uses a 24-byte nonce.
    pub fn from_nonce_len(len: usize) -> Result<Self, VaultError> {
        match len {
            AES_GCM_NONCE_LEN => Ok(CipherAlgorithm::Aes256Gcm),
            NONCE_LEN => Ok(CipherAlgorithm::XChaCha20Poly1305),
            _ => Err(VaultError::DecryptionFailed(format!(
                "Unsupported nonce length {} — cannot determine cipher",
                len
            ))),
        }
    }

    /// Default cipher for newly created records.
    pub fn default_for_records() -> Self {
        CipherAlgorithm::Aes256Gcm
    }
}

/// Derive a key-encryption key from a password using Argon2id
///
/// This is the password-based key derivation specified in directive §16:
///   password → Argon2id(memory=256MiB, iterations=3, parallelism=4) → key-encryption key
///
/// The derived key is used to wrap (encrypt) the vault master key,
/// NOT to encrypt records directly. This allows password changes
/// without re-encrypting every record.
pub fn derive_key_encryption_key(
    password: &[u8],
    salt: &[u8; SALT_LEN],
) -> Result<[u8; KEY_ENCRYPTION_KEY_LEN], VaultError> {
    let params = Params::new(
        ARGON2_MEMORY,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        Some(KEY_ENCRYPTION_KEY_LEN),
    )
    .map_err(|e| VaultError::Crypto(format!("Invalid Argon2id params: {}", e)))?;

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut key = [0u8; KEY_ENCRYPTION_KEY_LEN];
    argon2
        .hash_password_into(password, salt, &mut key)
        .map_err(|e| VaultError::Crypto(format!("Argon2id derivation failed: {}", e)))?;

    Ok(key)
}

/// Encrypt data using XChaCha20-Poly1305 with authenticated associated data (AAD)
///
/// This is the primary encryption primitive for vault records and the vault header.
/// AAD is authenticated but NOT encrypted — used for header metadata that must be
/// readable without decryption.
pub fn encrypt(
    key: &[u8; KEY_ENCRYPTION_KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, VaultError> {
    let cipher = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| VaultError::EncryptionFailed(format!("Invalid key: {}", e)))?;

    let xnonce = XNonce::from_slice(nonce);
    let payload = ChaChaPayload {
        msg: plaintext,
        aad,
    };
    cipher
        .encrypt(xnonce, payload)
        .map_err(|e| VaultError::EncryptionFailed(format!("Encryption failed: {}", e)))
}

/// Decrypt data using XChaCha20-Poly1305 with AAD verification
///
/// Returns the plaintext only if both the ciphertext and AAD are authentic.
/// Any modification to ciphertext, nonce, or AAD will cause decryption to fail.
pub fn decrypt(
    key: &[u8; KEY_ENCRYPTION_KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, VaultError> {
    let cipher = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| VaultError::DecryptionFailed(format!("Invalid key: {}", e)))?;

    let xnonce = XNonce::from_slice(nonce);
    let payload = ChaChaPayload {
        msg: ciphertext,
        aad,
    };
    cipher.decrypt(xnonce, payload).map_err(|e| {
        VaultError::DecryptionFailed(format!(
            "Decryption failed (wrong password or tampered data): {}",
            e
        ))
    })
}

/// Encrypt data using AES-256-GCM with authenticated associated data (AAD)
fn encrypt_aes256gcm(
    key: &[u8; KEY_ENCRYPTION_KEY_LEN],
    nonce: &[u8; AES_GCM_NONCE_LEN],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, VaultError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| VaultError::EncryptionFailed(format!("Invalid AES key: {}", e)))?;
    let aes_nonce = AesNonce::from_slice(nonce);
    let payload = AesPayload {
        msg: plaintext,
        aad,
    };
    cipher
        .encrypt(aes_nonce, payload)
        .map_err(|e| VaultError::EncryptionFailed(format!("AES-256-GCM encryption failed: {}", e)))
}

/// Decrypt data using AES-256-GCM with AAD verification
fn decrypt_aes256gcm(
    key: &[u8; KEY_ENCRYPTION_KEY_LEN],
    nonce: &[u8; AES_GCM_NONCE_LEN],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, VaultError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| VaultError::DecryptionFailed(format!("Invalid AES key: {}", e)))?;
    let aes_nonce = AesNonce::from_slice(nonce);
    let payload = AesPayload {
        msg: ciphertext,
        aad,
    };
    cipher.decrypt(aes_nonce, payload).map_err(|e| {
        VaultError::DecryptionFailed(format!(
            "AES-256-GCM decryption failed (wrong key or tampered data): {}",
            e
        ))
    })
}

/// Generate a cryptographically secure AES-GCM nonce (96 bits)
pub fn generate_aes_gcm_nonce() -> [u8; AES_GCM_NONCE_LEN] {
    let mut nonce = [0u8; AES_GCM_NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    nonce
}

/// Encrypt data with the selected cipher algorithm.
///
/// Output layout is always `[nonce][ciphertext + tag]`.
/// The nonce length reveals the algorithm:
///   - 12 bytes → AES-256-GCM
///   - 24 bytes → XChaCha20-Poly1305
pub fn encrypt_with_algorithm(
    algorithm: CipherAlgorithm,
    key: &[u8; KEY_ENCRYPTION_KEY_LEN],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), VaultError> {
    match algorithm {
        CipherAlgorithm::Aes256Gcm => {
            let nonce = generate_aes_gcm_nonce();
            let ciphertext = encrypt_aes256gcm(key, &nonce, plaintext, aad)?;
            Ok((nonce.to_vec(), ciphertext))
        }
        CipherAlgorithm::XChaCha20Poly1305 => {
            let nonce = generate_nonce();
            let ciphertext = encrypt(key, &nonce, plaintext, aad)?;
            Ok((nonce.to_vec(), ciphertext))
        }
    }
}

/// Decrypt data with the selected cipher algorithm.
///
/// The nonce length determines which AEAD is used.
pub fn decrypt_with_algorithm(
    algorithm: CipherAlgorithm,
    key: &[u8; KEY_ENCRYPTION_KEY_LEN],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, VaultError> {
    match algorithm {
        CipherAlgorithm::Aes256Gcm => {
            let nonce: &[u8; AES_GCM_NONCE_LEN] = nonce.try_into().map_err(|_| {
                VaultError::DecryptionFailed("AES-GCM nonce must be 12 bytes".to_string())
            })?;
            decrypt_aes256gcm(key, nonce, ciphertext, aad)
        }
        CipherAlgorithm::XChaCha20Poly1305 => {
            let nonce: &[u8; NONCE_LEN] = nonce.try_into().map_err(|_| {
                VaultError::DecryptionFailed("XChaCha20 nonce must be 24 bytes".to_string())
            })?;
            decrypt(key, nonce, ciphertext, aad)
        }
    }
}

/// Derive a domain-specific key from the vault master key using HKDF-SHA-256
///
/// Each vault domain (records, journal, indexes, etc.) gets its own key
/// derived from the master key. This provides domain isolation:
/// compromising one domain's key does not compromise others.
///
/// The info parameter is the domain name (e.g., "records", "journal").
pub fn derive_domain_key(master_key: &[u8; MASTER_KEY_LEN], domain: &str) -> [u8; MASTER_KEY_LEN] {
    let hkdf = Hkdf::<Sha256>::new(Some(b"unoone-vault-domain"), master_key);
    let mut domain_key = [0u8; MASTER_KEY_LEN];
    // HKDF expand is infallible when output length <= HashLen (32 bytes for SHA-256)
    hkdf.expand(domain.as_bytes(), &mut domain_key)
        .expect("HKDF expand failed — output length exceeds hash length");
    domain_key
}

/// Wrap (encrypt) the vault master key using the password-derived key-encryption key
///
/// This encrypts the master key so it can be stored in the vault header.
/// The master key is never stored in plaintext.
pub fn wrap_master_key(
    kek: &[u8; KEY_ENCRYPTION_KEY_LEN],
    master_key: &[u8; MASTER_KEY_LEN],
    nonce: &[u8; NONCE_LEN],
) -> Result<Vec<u8>, VaultError> {
    // AAD for master key wrapping includes the purpose
    let aad = b"unoone-vault-master-key-wrap";
    encrypt(kek, nonce, master_key, aad)
}

/// Unwrap (decrypt) the vault master key using the password-derived key-encryption key
///
/// This decrypts the wrapped master key from the vault header.
/// Fails if the password is wrong (AAD authentication will fail).
pub fn unwrap_master_key(
    kek: &[u8; KEY_ENCRYPTION_KEY_LEN],
    wrapped_key: &[u8],
    nonce: &[u8; NONCE_LEN],
) -> Result<[u8; MASTER_KEY_LEN], VaultError> {
    let aad = b"unoone-vault-master-key-wrap";
    let plaintext = decrypt(kek, nonce, wrapped_key, aad)?;

    let mut master_key = [0u8; MASTER_KEY_LEN];
    master_key.copy_from_slice(&plaintext[..MASTER_KEY_LEN]);
    Ok(master_key)
}

// REMOVED: wrap_master_key_with_recovery / unwrap_master_key_with_recovery
//
// These derived the recovery key-encryption key with an ALL-ZERO salt:
//     derive_key_encryption_key(recovery_secret, &[0u8; SALT_LEN])
//
// They were dead code — nothing in the workspace called them. The real and
// correct recovery path is VaultHeader::enable_recovery /
// VaultHeader::unlock_with_recovery, which uses a per-vault random
// `recovery_salt` persisted in the header.
//
// They are deleted rather than left in place because an unused-but-public weak
// primitive is a loaded gun: the next caller to reach for the obviously-named
// function would silently get a globally-fixed salt instead of the per-vault
// one. A fixed salt is identical on every Pocket AI device ever shipped, so any
// future reduction in recovery-secret entropy would become precomputable across
// the entire install base at once.
//
// If a standalone recovery wrap is ever needed again, take the salt as a
// parameter and source it from the vault header.

/// Compute HMAC-SHA-256 for header authentication
///
/// This is used to verify the integrity of the vault header.
/// The header is authenticated but NOT encrypted (some fields are public).
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    use hmac::Mac;
    type HmacSha256 = Hmac<Sha256>;

    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC key length is valid");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Generate a cryptographically secure random salt (32 bytes)
pub fn generate_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    salt
}

/// Generate a cryptographically secure random master key (256 bits)
pub fn generate_master_key() -> [u8; MASTER_KEY_LEN] {
    let mut key = [0u8; MASTER_KEY_LEN];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

/// Generate a cryptographically secure random nonce (192 bits for XChaCha20)
pub fn generate_nonce() -> [u8; NONCE_LEN] {
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    nonce
}

/// Generate a cryptographically secure recovery secret (256 bits)
pub fn generate_recovery_secret() -> [u8; RECOVERY_SECRET_LEN] {
    let mut secret = [0u8; RECOVERY_SECRET_LEN];
    rand::thread_rng().fill_bytes(&mut secret);
    secret
}

/// Zero a byte array securely
///
/// This is used to clear cryptographic keys from memory when the vault is locked.
/// Uses the zeroize crate to prevent compiler optimizations from removing the zeroing.
pub fn secure_zero(data: &mut [u8]) {
    data.zeroize();
}

#[cfg(test)]
mod tests {
    /// The Argon2id parameters are a CROSS-PLATFORM CONTRACT with the Kotlin
    /// encrypted-vault package. If Rust and Kotlin disagree, a vault written on
    /// Android cannot be unlocked on Windows and vice versa — and because the
    /// test build deliberately uses reduced parameters, no other test in this
    /// suite would notice.
    #[test]
    fn production_kdf_parameters_match_cross_platform_spec() {
        assert_eq!(SPEC_ARGON2_MEMORY_KIB, 256 * 1024);
        assert_eq!(SPEC_ARGON2_ITERATIONS, 3);
        assert_eq!(SPEC_ARGON2_PARALLELISM, 4);
        assert_eq!(ARGON2_PARALLELISM, SPEC_ARGON2_PARALLELISM);
    }

    /// Salts must be random per call. A fixed salt would make derived keys
    /// identical across every Pocket AI device.
    #[test]
    fn generated_salts_are_random_and_nonzero() {
        let a = generate_salt();
        let b = generate_salt();
        assert_ne!(a, b);
        assert_ne!(a, [0u8; SALT_LEN]);
    }

    use super::*;

    #[test]
    fn test_derive_key_encryption_key() {
        let password = b"test-password-12345678";
        let salt = generate_salt();
        let key = derive_key_encryption_key(password, &salt).unwrap();
        assert_eq!(key.len(), KEY_ENCRYPTION_KEY_LEN);

        // Same password + same salt = same key
        let key2 = derive_key_encryption_key(password, &salt).unwrap();
        assert_eq!(key, key2);

        // Different password = different key
        let key3 = derive_key_encryption_key(b"wrong-password-12345", &salt).unwrap();
        assert_ne!(key, key3);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = generate_master_key();
        let nonce = generate_nonce();
        let plaintext = b"Hello, UnoOne vault!";
        let aad = b"test-associated-data";

        let ciphertext = encrypt(&key, &nonce, plaintext, aad).unwrap();
        assert_ne!(ciphertext, plaintext);

        let decrypted = decrypt(&key, &nonce, &ciphertext, aad).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_fails_with_wrong_key() {
        let key = generate_master_key();
        let wrong_key = generate_master_key();
        let nonce = generate_nonce();
        let plaintext = b"secret data";
        let aad = b"test-aad";

        let ciphertext = encrypt(&key, &nonce, plaintext, aad).unwrap();

        // Decrypting with wrong key must fail
        let result = decrypt(&wrong_key, &nonce, &ciphertext, aad);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_fails_with_modified_ciphertext() {
        let key = generate_master_key();
        let nonce = generate_nonce();
        let plaintext = b"secret data";
        let aad = b"test-aad";

        let mut ciphertext = encrypt(&key, &nonce, plaintext, aad).unwrap();
        // Flip one bit
        ciphertext[0] ^= 0x01;

        let result = decrypt(&key, &nonce, &ciphertext, aad);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_fails_with_modified_aad() {
        let key = generate_master_key();
        let nonce = generate_nonce();
        let plaintext = b"secret data";
        let aad = b"original-aad";

        let ciphertext = encrypt(&key, &nonce, plaintext, aad).unwrap();

        let result = decrypt(&key, &nonce, &ciphertext, b"modified-aad");
        assert!(result.is_err());
    }

    #[test]
    fn test_aes256gcm_roundtrip() {
        let key = generate_master_key();
        let plaintext = b"Hello from AES-256-GCM!";
        let aad = b"test-aad";

        let (nonce, ciphertext) =
            encrypt_with_algorithm(CipherAlgorithm::Aes256Gcm, &key, plaintext, aad).unwrap();

        assert_eq!(nonce.len(), AES_GCM_NONCE_LEN);
        assert!(!ciphertext.is_empty());

        let decrypted =
            decrypt_with_algorithm(CipherAlgorithm::Aes256Gcm, &key, &nonce, &ciphertext, aad)
                .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_aes256gcm_rejects_wrong_key() {
        let key1 = generate_master_key();
        let key2 = generate_master_key();
        let plaintext = b"secret data";

        let (nonce, ciphertext) =
            encrypt_with_algorithm(CipherAlgorithm::Aes256Gcm, &key1, plaintext, b"").unwrap();

        let result =
            decrypt_with_algorithm(CipherAlgorithm::Aes256Gcm, &key2, &nonce, &ciphertext, b"");
        assert!(result.is_err());
    }

    #[test]
    fn test_aes256gcm_rejects_modified_ciphertext() {
        let key = generate_master_key();
        let plaintext = b"secret data";

        let (nonce, mut ciphertext) =
            encrypt_with_algorithm(CipherAlgorithm::Aes256Gcm, &key, plaintext, b"").unwrap();

        ciphertext[0] ^= 0x01;

        let result =
            decrypt_with_algorithm(CipherAlgorithm::Aes256Gcm, &key, &nonce, &ciphertext, b"");
        assert!(result.is_err());
    }

    #[test]
    fn test_cipher_algorithm_from_nonce_len() {
        assert_eq!(
            CipherAlgorithm::from_nonce_len(AES_GCM_NONCE_LEN).unwrap(),
            CipherAlgorithm::Aes256Gcm
        );
        assert_eq!(
            CipherAlgorithm::from_nonce_len(NONCE_LEN).unwrap(),
            CipherAlgorithm::XChaCha20Poly1305
        );
        assert!(CipherAlgorithm::from_nonce_len(16).is_err());
    }

    #[test]
    fn test_both_cipher_algorithms_roundtrip() {
        let key = generate_master_key();
        let plaintext = b"cross-platform cipher test";
        let aad = b"domain-records";

        for algorithm in [
            CipherAlgorithm::Aes256Gcm,
            CipherAlgorithm::XChaCha20Poly1305,
        ] {
            let (nonce, ciphertext) =
                encrypt_with_algorithm(algorithm, &key, plaintext, aad).unwrap();
            let decrypted =
                decrypt_with_algorithm(algorithm, &key, &nonce, &ciphertext, aad).unwrap();
            assert_eq!(decrypted, plaintext, "{:?} roundtrip failed", algorithm);
        }
    }

    #[test]
    fn test_domain_key_derivation() {
        let master_key = generate_master_key();
        let records_key = derive_domain_key(&master_key, DOMAIN_RECORDS);
        let journal_key = derive_domain_key(&master_key, DOMAIN_JOURNAL);

        // Different domains produce different keys
        assert_ne!(records_key, journal_key);

        // Same domain produces same key
        let records_key2 = derive_domain_key(&master_key, DOMAIN_RECORDS);
        assert_eq!(records_key, records_key2);

        // Different master key produces different domain keys
        let other_master = generate_master_key();
        let other_records_key = derive_domain_key(&other_master, DOMAIN_RECORDS);
        assert_ne!(records_key, other_records_key);
    }

    /// Cross-language regression vector: Kotlin `Argon2idKdf.deriveDomainKey`
    /// and Rust `derive_domain_key` must produce identical output for the same
    /// master key and domain.
    #[test]
    fn test_domain_key_matches_kotlin_reference() {
        let master_key = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let expected =
            hex::decode("2620d0380a68bffda15bb83301337751729230cef7252c1704e879e119775e5f")
                .unwrap();
        let actual = derive_domain_key(&master_key, DOMAIN_RECORDS);
        assert_eq!(expected, actual, "Rust HKDF must match Kotlin reference");
    }

    /// Deterministic AES-256-GCM cross-platform vector.
    ///
    /// This is the canonical record encryption for cross-platform vault sync.
    /// Kotlin/Android must be able to decrypt `[nonce || ciphertext]` using
    /// the same AES-256-GCM raw-domain-key path.
    #[test]
    fn test_aes256gcm_cross_platform_vector() {
        let key = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let nonce = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b,
        ];
        let plaintext = b"UnoOne cross-platform vault sync";
        let aad = b"records";

        let ciphertext = encrypt_aes256gcm(&key, &nonce, plaintext, aad).unwrap();

        // Self-consistency check
        let decrypted = decrypt_aes256gcm(&key, &nonce, &ciphertext, aad).unwrap();
        assert_eq!(decrypted, plaintext);

        println!(
            "AES-256-GCM cross-platform vector (key=00..1f, nonce=00..0b):\n  nonce:  {}\n  aad:    {}\n  ct+tag: {}",
            hex::encode(nonce),
            String::from_utf8_lossy(aad),
            hex::encode(&ciphertext)
        );
    }

    #[test]
    fn test_wrap_unwrap_master_key() {
        let master_key = generate_master_key();
        let password = b"test-password-12345678";
        let salt = generate_salt();
        let nonce = generate_nonce();

        let kek = derive_key_encryption_key(password, &salt).unwrap();
        let wrapped = wrap_master_key(&kek, &master_key, &nonce).unwrap();

        // Wrapped key is different from plaintext master key
        assert_ne!(wrapped.as_slice(), master_key.as_slice());

        // Unwrap with correct password succeeds
        let unwrapped = unwrap_master_key(&kek, &wrapped, &nonce).unwrap();
        assert_eq!(master_key, unwrapped);

        // Unwrap with wrong password fails
        let wrong_kek = derive_key_encryption_key(b"wrong-password-1234", &salt).unwrap();
        let result = unwrap_master_key(&wrong_kek, &wrapped, &nonce);
        assert!(result.is_err());
    }

    #[test]
    fn test_hmac_sha256() {
        let key = b"hmac-key";
        let data = b"test data";
        let mac1 = hmac_sha256(key, data);
        let mac2 = hmac_sha256(key, data);
        assert_eq!(mac1, mac2);

        // Different data produces different MAC
        let mac3 = hmac_sha256(key, b"other data");
        assert_ne!(mac1, mac3);
    }

    #[test]
    fn test_secure_zero() {
        let mut key = [1u8, 2, 3, 4, 5, 6, 7, 8];
        secure_zero(&mut key);
        assert_eq!(key, [0u8; 8]);
    }

    #[test]
    fn test_empty_password_fails() {
        let result = derive_key_encryption_key(b"", &generate_salt());
        // Argon2id with empty password should still produce a key
        // (the password validation is at a higher level)
        assert!(result.is_ok());
    }
}

/// Fixed-nonce AES-256-GCM helpers for the cross-platform vector suites only
/// (tests/cross_platform_vectors.rs and the mirrored Kotlin test). Real code
/// must use the random-nonce generators. NOT for production use.
#[doc(hidden)]
pub mod test_support {
    use super::*;

    pub fn encrypt_aes256gcm_with_nonce(
        key: &[u8; KEY_ENCRYPTION_KEY_LEN],
        nonce: &[u8; AES_GCM_NONCE_LEN],
        plaintext: &[u8],
        aad: &[u8],
    ) -> Vec<u8> {
        encrypt_aes256gcm(key, nonce, plaintext, aad).expect("test encrypt")
    }

    pub fn decrypt_aes256gcm_with_nonce(
        key: &[u8; KEY_ENCRYPTION_KEY_LEN],
        nonce: &[u8; AES_GCM_NONCE_LEN],
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, VaultError> {
        decrypt_aes256gcm(key, nonce, ciphertext, aad)
    }
}
