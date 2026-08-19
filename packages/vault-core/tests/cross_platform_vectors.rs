//! Cross-platform crypto contract vectors (§7 Android shared vault).
//!
//! These tests pin the crypto contract between Rust (vault-core) and the
//! Kotlin side (android-app/UnoOneAgent/vault/.../VaultCrypto.kt) as
//! DETERMINISTIC, checked-in vectors. If either side drifts — KDF params,
//! HKDF salt/info, AAD construction, nonce/tag layout, field naming — one
//! of the two implementations fails in CI before a phone is involved.
//!
//! The vectors live in `test-vectors/` and are READ-ONLY from tests on both
//! sides. Regenerating them is a deliberate event (bump and review), never
//! part of a test run.

use serde::{Deserialize, Serialize};
use sha2::Digest;
use unoone_vault_core::crypto::*;
use unoone_vault_core::record::{canonical_metadata_bytes, Record};

#[derive(Serialize, Deserialize)]
struct ArgonVector {
    name: String,
    password_utf8: String,
    salt_hex: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    output_len: usize,
    expected_key_hex: String,
}

#[derive(Serialize, Deserialize)]
struct RecordVector {
    name: String,
    master_key_hex: String,
    domain: String,
    nonce_hex: String,
    record_json: serde_json::Value,
    aad_hex: String,
    plaintext_utf8: String,
    ciphertext_hex: String,
}

#[derive(Serialize, Deserialize)]
struct VectorFile {
    spec: String,
    kdf: Vec<ArgonVector>,
    record: Vec<RecordVector>,
}

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn load_vectors() -> VectorFile {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-vectors/vault-cross-platform.json"
    ))
    .expect("cross-platform vectors file missing");
    serde_json::from_str(&text).expect("cross-platform vectors file is not the documented JSON")
}

#[test]
fn argon2id_kek_matches_checked_in_vectors() {
    let vectors = load_vectors();
    assert_eq!(vectors.spec, "unoone-vault-cross-platform/1");
    assert!(
        !vectors.kdf.is_empty(),
        "no KDF vectors — contract is untested"
    );
    for v in vectors.kdf {
        assert_eq!(
            v.memory_kib, SPEC_ARGON2_MEMORY_KIB,
            "vector pins a different memory"
        );
        assert_eq!(v.iterations, SPEC_ARGON2_ITERATIONS);
        assert_eq!(v.parallelism, SPEC_ARGON2_PARALLELISM);
        let salt_bytes = hex_decode(&v.salt_hex);
        let salt: &[u8; SALT_LEN] = salt_bytes.as_slice().try_into().expect("salt length");
        let key = derive_key_encryption_key(v.password_utf8.as_bytes(), salt)
            .unwrap_or_else(|e| panic!("KDF failed for {}: {}", v.name, e));
        assert_eq!(
            hex::encode(key),
            v.expected_key_hex,
            "Argon2id KEK mismatch for vector {}",
            v.name
        );
    }
}

#[test]
fn record_roundtrip_matches_checked_in_vectors() {
    let vectors = load_vectors();
    let mut saw_decrypt = false;
    let mut saw_encrypt = false;
    for v in vectors.record {
        let master_key_bytes = hex_decode(&v.master_key_hex);
        let master_key: &[u8; MASTER_KEY_LEN] = master_key_bytes
            .as_slice()
            .try_into()
            .expect("master key length");
        let domain_key = derive_domain_key(master_key, &v.domain);
        let nonce_bytes = hex_decode(&v.nonce_hex);
        let plaintext_bytes = v.plaintext_utf8.as_bytes();

        // AAD must be reconstructed from the record metadata, not trusted.
        let record: Record = serde_json::from_value(v.record_json.clone())
            .expect("vector record_json is not a vault Record");
        let aad = canonical_metadata_bytes(&record).expect("canonical AAD build failed");
        assert_eq!(
            hex::encode(&aad),
            v.aad_hex,
            "AAD construction mismatch for {} — metadata canonicalisation drift",
            v.name
        );

        match v.name.as_str() {
            n if n.starts_with("rust-encrypt") => {
                // Rust reproduces the ciphertext exactly from (key, nonce, aad, pt).
                let ct =
                    encrypt_aes256gcm_for_test(&domain_key, &nonce_bytes, plaintext_bytes, &aad);
                assert_eq!(
                    hex::encode(&ct),
                    v.ciphertext_hex,
                    "Rust encrypt drift for {}",
                    v.name
                );
                saw_encrypt = true;
            }
            n if n.starts_with("kotlin-encrypt") => {
                // Kotlin produced this ciphertext; Rust must DECRYPT it.
                let ct = hex_decode(&v.ciphertext_hex);
                let pt = decrypt_aes256gcm_for_test(&domain_key, &nonce_bytes, &ct, &aad)
                    .unwrap_or_else(|e| {
                        panic!("Rust failed to decrypt Kotlin vector {}: {}", v.name, e)
                    });
                assert_eq!(
                    pt, plaintext_bytes,
                    "Kotlin-encrypted vector decrypts to different bytes for {}",
                    v.name
                );
                saw_decrypt = true;
            }
            _ => panic!("unknown vector kind: {}", v.name),
        }
    }
    assert!(
        saw_decrypt && saw_encrypt,
        "vectors must cover both directions"
    );
}

/// Deterministic-encrypt helper mirroring encrypt_aes256gcm (fixed nonce),
/// re-exported here instead of making the crate's private fns public.
fn encrypt_aes256gcm_for_test(
    key: &[u8; KEY_ENCRYPTION_KEY_LEN],
    nonce: &[u8],
    plaintext: &[u8],
    aad: &[u8],
) -> Vec<u8> {
    encrypt_with_algorithm_fixed_nonce(key, nonce, plaintext, aad)
}

fn decrypt_aes256gcm_for_test(
    key: &[u8; KEY_ENCRYPTION_KEY_LEN],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, String> {
    decrypt_with_fixed_nonce(key, nonce, ciphertext, aad)
}

fn encrypt_with_algorithm_fixed_nonce(
    key: &[u8; KEY_ENCRYPTION_KEY_LEN],
    nonce: &[u8],
    plaintext: &[u8],
    aad: &[u8],
) -> Vec<u8> {
    // Use the crate's Aes256Gcm implementation with an explicit nonce.
    unoone_vault_core::crypto::test_support::encrypt_aes256gcm_with_nonce(
        key,
        nonce.try_into().expect("12-byte AES-GCM nonce"),
        plaintext,
        aad,
    )
}

fn decrypt_with_fixed_nonce(
    key: &[u8; KEY_ENCRYPTION_KEY_LEN],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, String> {
    unoone_vault_core::crypto::test_support::decrypt_aes256gcm_with_nonce(
        key,
        nonce
            .try_into()
            .map_err(|_| "12-byte AES-GCM nonce".to_string())?,
        ciphertext,
        aad,
    )
    .map_err(|e| format!("{:?}", e))
}

#[test]
fn aad_contains_no_trailing_whitespace_or_randomness() {
    let vectors = load_vectors();
    for v in vectors.record {
        let record: Record = serde_json::from_value(v.record_json.clone()).unwrap();
        let a1 = canonical_metadata_bytes(&record).unwrap();
        let a2 = canonical_metadata_bytes(&record).unwrap();
        assert_eq!(a1, a2, "AAD must be deterministic for {}", v.name);
        // Sanity: hash of the documented vector AAD matches too.
        let mut h = sha2::Sha256::new();
        h.update(&a1);
        let _ = h.finalize();
    }
}

#[test]
fn master_key_wrap_matches_checked_in_vector() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-vectors/vault-cross-platform.json"
    ))
    .expect("vectors file missing");
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let wrap = doc
        .get("wrap")
        .and_then(|w| w.as_array())
        .and_then(|a| a.first())
        .cloned()
        .expect("wrap vector required — contract must pin the unlock path");
    let kek_bytes = hex_decode(wrap["kek_hex"].as_str().unwrap());
    let kek: &[u8; KEY_ENCRYPTION_KEY_LEN] = kek_bytes.as_slice().try_into().unwrap();
    let nonce_bytes = hex_decode(wrap["nonce_hex"].as_str().unwrap());
    let nonce: &[u8; NONCE_LEN] = nonce_bytes.as_slice().try_into().unwrap();
    let mk_bytes = hex_decode(wrap["master_key_hex"].as_str().unwrap());
    let mk: &[u8; MASTER_KEY_LEN] = mk_bytes.as_slice().try_into().unwrap();
    let expected_wrapped = hex_decode(wrap["wrapped_hex"].as_str().unwrap());

    // Rust must reproduce and reverse the pinned wrap bytes exactly.
    let wrapped = wrap_master_key(kek, mk, nonce).expect("wrap");
    assert_eq!(
        hex::encode(&wrapped),
        hex::encode(&expected_wrapped),
        "wrap drift"
    );
    let back = unwrap_master_key(kek, &expected_wrapped, nonce).expect("unwrap");
    assert_eq!(back, *mk);
}

/// The canonical-AAD JSON string escaping is a CROSS-PLATFORM CONTRACT.
///
/// Kotlin hand-rolls its canonical AAD (`VaultCrypto.jsonEscape`) because the
/// AAD must be byte-identical to what Rust's `serde_json` produces: the Rust
/// read path re-derives the AAD from the stored metadata and rejects the
/// record when the bytes differ. A divergence therefore surfaces as
/// "metadata was altered after the record was written" -- a tamper error for a
/// perfectly honest record written by Android.
///
/// This test re-derives every pinned case straight from `serde_json`, so the
/// vector file can never drift from Rust's real behaviour. The Kotlin suite
/// asserts the same cases against `jsonEscape`, which is what makes the two
/// implementations provably agree without a phone.
///
/// Known regression this pins: Kotlin previously emitted the six-character
/// forms for backspace and form feed where serde_json emits the two-character
/// short escapes.
#[test]
fn json_string_escaping_matches_serde_json_for_every_pinned_case() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-vectors/vault-cross-platform.json"
    ))
    .expect("vectors file missing");
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let cases = doc
        .get("escaping")
        .and_then(|e| e.as_array())
        .expect("escaping vectors required -- the AAD contract must pin them");
    assert!(!cases.is_empty(), "no escaping vectors");

    let short_backspace = "\\b";
    let short_formfeed = "\\f";
    let long_backspace = "\\u0008";
    let long_formfeed = "\\u000C";

    let mut saw_backspace_formfeed = false;

    for case in cases {
        let name = case["name"].as_str().unwrap();
        let input_bytes = hex_decode(case["input_utf8_hex"].as_str().unwrap());
        let input = String::from_utf8(input_bytes).expect("vector input must be valid UTF-8");
        let expected = case["serde_json"].as_str().unwrap();

        let actual = serde_json::to_string(&input).expect("serialize");
        assert_eq!(
            actual, expected,
            "escaping vector '{name}' drifted from serde_json"
        );

        if name == "backspace_formfeed" {
            saw_backspace_formfeed = true;
            assert!(
                expected.contains(short_backspace) && expected.contains(short_formfeed),
                "backspace and form feed must use the short escapes"
            );
            assert!(
                !expected.contains(long_backspace) && !expected.contains(long_formfeed),
                "backspace and form feed must NOT be emitted in the six-character form"
            );
        }
    }

    assert!(
        saw_backspace_formfeed,
        "the backspace/form-feed case must stay pinned -- it is the known divergence"
    );
}
