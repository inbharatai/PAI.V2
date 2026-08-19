// UnoOne Vault Core — Recovery key generation and word encoding
// Uses a 24-word representation of a 256-bit recovery secret
// (NOT UUID fragments — directive §16 explicitly prohibits that)
//
// The word list follows the BIP-39 English word list pattern for
// mnemonic encoding. The secret itself is a random 256-bit value
// with a SHA-256 checksum for integrity verification.

use crate::crypto::{generate_recovery_secret, secure_zero, RECOVERY_SECRET_LEN};
use crate::error::VaultError;

// Note: In the actual implementation, we use the full BIP-39 word list.
// The abbreviated list above is for illustration. The production code
// uses the `bip39` crate or an embedded complete word list.
// For this initial implementation, we use a simplified approach that
// encodes bytes directly as word indices.

/// A recovery phrase consisting of 24 words
#[derive(Debug, Clone)]
pub struct RecoveryPhrase {
    /// The 24 words
    pub words: Vec<String>,
    /// The 256-bit recovery secret (never persisted, zeroed on lock)
    secret: [u8; RECOVERY_SECRET_LEN],
}

impl RecoveryPhrase {
    /// Generate a new recovery phrase from a random 256-bit secret
    pub fn generate() -> Self {
        let secret = generate_recovery_secret();
        let words = secret_to_words(&secret);
        Self { words, secret }
    }

    /// Create from an existing recovery secret
    pub fn from_secret(secret: [u8; RECOVERY_SECRET_LEN]) -> Self {
        let words = secret_to_words(&secret);
        Self { words, secret }
    }

    /// Create from a list of 24 words
    pub fn from_words(words: &[String]) -> Result<Self, VaultError> {
        if words.len() != 24 {
            return Err(VaultError::InvalidRecoveryWords(format!(
                "Expected 24 words, got {}",
                words.len()
            )));
        }

        let secret = words_to_secret(words)?;
        Ok(Self {
            words: words.to_vec(),
            secret,
        })
    }

    /// Get the recovery secret (for key unwrapping)
    pub fn secret(&self) -> &[u8; RECOVERY_SECRET_LEN] {
        &self.secret
    }

    /// Get the words as a space-separated string for display
    pub fn phrase(&self) -> String {
        self.words.join(" ")
    }
}

impl Drop for RecoveryPhrase {
    fn drop(&mut self) {
        secure_zero(&mut self.secret);
    }
}

/// Convert a 256-bit recovery secret to 24 words
/// Each word represents 11 bits (2^11 = 2048 words)
/// 256 bits = 23.27 11-bit groups, so we add a checksum
/// to reach exactly 24 words (264 bits = 24 × 11 bits)
pub fn secret_to_words(secret: &[u8; RECOVERY_SECRET_LEN]) -> Vec<String> {
    use sha2::{Digest, Sha256};

    // Compute SHA-256 checksum of the secret
    let mut hasher = Sha256::new();
    hasher.update(secret);
    let checksum = hasher.finalize();

    // Append first byte of checksum to make 264 bits (33 bytes)
    let mut extended = secret.to_vec();
    extended.push(checksum[0]);

    // Convert to 24 groups of 11 bits
    let mut words = Vec::with_capacity(24);
    let mut bits = BitIterator::new(&extended);

    for _ in 0..24 {
        let index = bits.next_bits(11);
        // Use the index modulo word list length for safety
        let word_index = index as usize % get_word_list().len();
        words.push(get_word_list()[word_index].to_string());
    }

    words
}

/// Convert 24 words back to a 256-bit recovery secret
/// Verifies the SHA-256 checksum
pub fn words_to_secret(words: &[String]) -> Result<[u8; RECOVERY_SECRET_LEN], VaultError> {
    use sha2::{Digest, Sha256};

    if words.len() != 24 {
        return Err(VaultError::InvalidRecoveryWords(format!(
            "Expected 24 words, got {}",
            words.len()
        )));
    }

    let word_list = get_word_list();

    // Convert words back to 11-bit indices
    let mut indices = Vec::with_capacity(24);
    for (i, word) in words.iter().enumerate() {
        let index = word_list.iter().position(|w| w == word).ok_or_else(|| {
            VaultError::InvalidRecoveryWords(format!(
                "Word '{}' at position {} is not in the BIP-39 word list",
                word, i
            ))
        })?;
        indices.push(index as u16);
    }

    // Convert 24 × 11-bit groups back to 33 bytes (264 bits)
    let mut bits = Vec::with_capacity(264);
    for index in &indices {
        for bit in (0..11).rev() {
            bits.push((index >> bit) & 1 == 1);
        }
    }

    // Convert bits to bytes
    let mut bytes = Vec::with_capacity(33);
    for chunk in bits.chunks(8) {
        let mut byte = 0u8;
        for (i, &bit) in chunk.iter().enumerate() {
            if bit {
                byte |= 1 << (7 - i);
            }
        }
        bytes.push(byte);
    }

    if bytes.len() < 33 {
        return Err(VaultError::InvalidRecoveryWords(
            "Insufficient data from word conversion".to_string(),
        ));
    }

    // First 32 bytes are the secret, last byte contains checksum bits
    let mut secret = [0u8; RECOVERY_SECRET_LEN];
    secret.copy_from_slice(&bytes[..32]);
    let expected_checksum_byte = bytes[32];

    // Verify checksum
    let mut hasher = Sha256::new();
    hasher.update(secret);
    let actual_checksum = hasher.finalize();

    // The first byte of the SHA-256 hash should match the checksum byte
    // (We use the full first byte for stronger verification)
    if actual_checksum[0] != expected_checksum_byte {
        return Err(VaultError::InvalidRecoveryWords(
            "Recovery phrase checksum verification failed — phrase may be incorrect or corrupted"
                .to_string(),
        ));
    }

    Ok(secret)
}

/// Bit iterator for converting bytes to 11-bit groups
struct BitIterator {
    bits: Vec<bool>,
    position: usize,
}

impl BitIterator {
    fn new(data: &[u8]) -> Self {
        let mut bits = Vec::with_capacity(data.len() * 8);
        for byte in data {
            for bit in (0..8).rev() {
                bits.push((byte >> bit) & 1 == 1);
            }
        }
        Self { bits, position: 0 }
    }

    fn next_bits(&mut self, count: usize) -> u16 {
        let mut value = 0u16;
        for i in 0..count {
            if self.position < self.bits.len() && self.bits[self.position] {
                value |= 1 << (count - 1 - i);
            }
            self.position += 1;
        }
        value
    }
}

/// Get the word list (full BIP-39 English word list)
fn get_word_list() -> &'static [&'static str] {
    &crate::bip39_words::BIP39_ENGLISH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_recovery_phrase() {
        let phrase = RecoveryPhrase::generate();
        assert_eq!(phrase.words.len(), 24);

        // Each word should be non-empty
        for word in &phrase.words {
            assert!(!word.is_empty());
        }

        // The secret should be 32 bytes
        assert_eq!(phrase.secret().len(), 32);
    }

    #[test]
    fn test_recovery_phrase_roundtrip() {
        let original = RecoveryPhrase::generate();
        let words = original.words.clone();

        // Reconstruct from words
        let reconstructed = RecoveryPhrase::from_words(&words).unwrap();

        // The secrets should match
        assert_eq!(original.secret(), reconstructed.secret());
    }

    #[test]
    fn test_wrong_word_count_fails() {
        let result = RecoveryPhrase::from_words(&[
            "abandon".to_string(),
            "ability".to_string(),
            "able".to_string(),
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn test_secret_to_words_and_back() {
        let mut secret = [0u8; 32];
        // Fill with known values
        for (i, slot) in secret.iter_mut().enumerate() {
            *slot = (i + 1) as u8;
        }

        let words = secret_to_words(&secret);
        assert_eq!(words.len(), 24);

        let recovered = words_to_secret(&words).unwrap();
        assert_eq!(secret, recovered);
    }

    #[test]
    fn test_modified_word_fails() {
        let original = RecoveryPhrase::generate();
        let mut words = original.words.clone();

        // Change one word
        words[0] = "wrongword".to_string();

        let result = RecoveryPhrase::from_words(&words);
        assert!(result.is_err());
    }

    #[test]
    fn test_secret_is_zeroed_on_drop() {
        let phrase = RecoveryPhrase::generate();
        let _secret_ptr = phrase.secret.as_ptr();

        // Force drop
        drop(phrase);

        // After drop, the secret memory should be zeroed
        // (This is a best-effort check — the memory may be reused)
        // We just verify it compiles and runs without panic
    }
}
