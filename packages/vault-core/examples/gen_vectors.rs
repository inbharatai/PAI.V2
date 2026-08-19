#![allow(clippy::all)]
//! One-off generator for the checked-in cross-platform vectors.
//! Run deliberately, never in CI: `cargo run -p unoone-vault-core --example gen_vectors`
use unoone_vault_core::crypto::*;
use unoone_vault_core::record::{canonical_metadata_bytes, Record, RecordType};

fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn main() {
    // `cargo run -p unoone-vault-core --example gen_vectors -- kotlin-envelope-only`
    // regenerates ONLY the Kotlin-authored envelope fixture, deriving from the
    // COMMITTED synthetic-vault header so no other fixture churns. Run with no
    // argument to regenerate everything coherently (the envelope re-derives
    // from the freshly generated header at the end).
    if std::env::args().any(|a| a == "kotlin-envelope-only") {
        gen_kotlin_envelope_fixture();
        return;
    }

    let out_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-vectors/vault-cross-platform.json"
    );

    // -- KDF vector (synthetic credentials) --
    let password = "synthetic-cross-platform-password";
    let salt: Vec<u8> = (0..SALT_LEN).map(|i| (i as u8) * 7 + 3).collect();
    let mut salt_arr = [0u8; SALT_LEN];
    salt_arr.copy_from_slice(&salt);
    let kek = derive_key_encryption_key(password.as_bytes(), &salt_arr).unwrap();

    // -- record vector (rust-encrypt) --
    let master_key: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(13) ^ 0xA5).collect();
    let mut mk = [0u8; 32];
    mk.copy_from_slice(&master_key);
    let domain_key = derive_domain_key(&mk, DOMAIN_RECORDS);

    let mut record = Record::new(RecordType::Document, "DESKTOP", "vector-gen");
    record.record_id = "1d0f2b3c-4b5a-4c6d-8e9f-0a1b2c3d4e5f".to_string();
    let aad = canonical_metadata_bytes(&record).unwrap();
    let nonce: [u8; 12] = [9, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0xA, 0xB];
    let plaintext = "cross-platform contract check — सभी एन्क्रिप्शन स्थानीय है";

    let ct =
        test_support::encrypt_aes256gcm_with_nonce(&domain_key, &nonce, plaintext.as_bytes(), &aad);
    // self-check
    let back = test_support::decrypt_aes256gcm_with_nonce(&domain_key, &nonce, &ct, &aad).unwrap();
    assert_eq!(back, plaintext.as_bytes());

    // placeholder for the kotlin-encrypt direction; replaced by the Kotlin
    // producer on first run. Until then a Rust-generated pair stands in with
    // a DIFFERENT nonce so both directions are pinned.
    let nonce2: [u8; 12] = [0xC, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22];
    let ct2 = test_support::encrypt_aes256gcm_with_nonce(
        &domain_key,
        &nonce2,
        plaintext.as_bytes(),
        &aad,
    );

    // -- master-key wrap vector (unlock path, XChaCha20-Poly1305) --
    let wrap_nonce: [u8; NONCE_LEN] = {
        let mut n = [0u8; NONCE_LEN];
        for (i, b) in n.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(3).wrapping_add(11);
        }
        n
    };
    let wrapped = wrap_master_key(&kek, &mk, &wrap_nonce).unwrap();
    let unwrapped = unwrap_master_key(&kek, &wrapped, &wrap_nonce).unwrap();
    assert_eq!(unwrapped, mk);

    // -- escaping vectors (canonical-AAD string contract) --
    // Inputs are pinned here; the expected serialisation is DERIVED from real
    // serde_json at generation time, exactly as the Rust test re-derives it,
    // so this section can never drift from Rust's actual behaviour. Emitting
    // it here keeps a full regeneration from silently dropping the section
    // (it was first added directly to the JSON, not to this generator).
    let escaping_cases: [(&str, &str); 10] = [
        ("ascii", "plain-ascii_0123"),
        ("quote_backslash", "a\"b\\c"),
        ("newline_tab_cr", "a\nb\tc\rd"),
        ("backspace_formfeed", "a\u{0008}b\u{000c}c"),
        ("other_c0", "x\u{0001}y\u{001f}z"),
        ("nul", "a\u{0000}b"),
        ("del_7f", "a\u{007f}b"),
        ("devanagari", "नमस्ते"),
        ("emoji_zwj", "👨\u{200d}👩\u{200d}👧\u{200d}👦"),
        (
            "all_c0",
            "\u{0000}\u{0001}\u{0002}\u{0003}\u{0004}\u{0005}\u{0006}\u{0007}\u{0008}\u{0009}\u{000a}\u{000b}\u{000c}\u{000d}\u{000e}\u{000f}\u{0010}\u{0011}\u{0012}\u{0013}\u{0014}\u{0015}\u{0016}\u{0017}\u{0018}\u{0019}\u{001a}\u{001b}\u{001c}\u{001d}\u{001e}\u{001f}",
        ),
    ];
    let escaping_json: Vec<serde_json::Value> = escaping_cases
        .iter()
        .map(|(name, input)| {
            serde_json::json!({
                "name": name,
                "input_utf8_hex": hex_encode(input.as_bytes()),
                "serde_json": serde_json::to_string(input).unwrap(),
            })
        })
        .collect();

    // NOTE: serde_json::Value orders object keys alphabetically, so a full
    // regeneration writes the top-level sections in canonical alphabetical
    // order. Consumers parse JSON and never depend on key order.
    let json = serde_json::json!({
        "spec": "unoone-vault-cross-platform/1",
        "kdf": [{
            "name": "kdf-argon2id-pin",
            "password_utf8": password,
            "salt_hex": hex_encode(&salt),
            "memory_kib": SPEC_ARGON2_MEMORY_KIB,
            "iterations": SPEC_ARGON2_ITERATIONS,
            "parallelism": SPEC_ARGON2_PARALLELISM,
            "output_len": 32,
            "expected_key_hex": hex_encode(&kek),
        }],
        "record": [{
            "name": "rust-encrypt-pinned",
            "master_key_hex": hex_encode(&master_key),
            "domain": "records",
            "nonce_hex": hex_encode(&nonce),
            "record_json": serde_json::to_value(&record).unwrap(),
            "aad_hex": hex_encode(&aad),
            "plaintext_utf8": plaintext,
            "ciphertext_hex": hex_encode(&ct),
        }, {
            "name": "kotlin-encrypt-pinned",
            "master_key_hex": hex_encode(&master_key),
            "domain": "records",
            "nonce_hex": hex_encode(&nonce2),
            "record_json": serde_json::to_value(&record).unwrap(),
            "aad_hex": hex_encode(&aad),
            "plaintext_utf8": plaintext,
            "ciphertext_hex": hex_encode(&ct2),
        }],
        "wrap": [{
            "name": "wrap-master-key-xchacha20",
            "kek_hex": hex_encode(&kek),
            "aad_utf8": "unoone-vault-master-key-wrap",
            "nonce_hex": hex_encode(&wrap_nonce),
            "master_key_hex": hex_encode(&mk),
            "wrapped_hex": hex_encode(&wrapped),
        }],
        "escaping": escaping_json,
    });
    std::fs::write(out_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    println!("vectors written to {}", out_path);
    gen_synthetic_vault_fixture();
    // The envelope derives from the header written just above, so it must
    // run after the synthetic vault regenerates.
    gen_kotlin_envelope_fixture();
}

// Appended: synthetic-vault fixture for the Kotlin repository test.
// Real vault created with SYNTHETIC credentials — the unlock path is the
// contract being proven, so the fixture must come from Rust.
#[allow(dead_code)]
fn gen_synthetic_vault_fixture() {
    use unoone_vault_core::{Record, RecordType};

    let out_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/test-vectors/synthetic-vault");
    std::fs::create_dir_all(format!("{}/", out_dir)).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let vault_root = tmp.path().join("UNOONE");
    let password = b"synthetic-fixture-password-2026";
    let _created = unoone_vault_core::Vault::create(vault_root.as_path(), password).unwrap();
    let mut vault = unoone_vault_core::Vault::open(vault_root.as_path()).unwrap();
    vault.unlock(password).unwrap();

    // one text record (MEMORY) and one document-like record (DOCUMENT)
    let mem_id = {
        let rec = Record::new(RecordType::Memory, "DESKTOP", "fixture-gen");
        let id = rec.record_id.clone();
        vault
            .write_record(rec, b"synthetic memory bytes: turmeric cardamom")
            .unwrap();
        id
    };
    let doc_id = {
        let rec = Record::new(RecordType::Document, "DESKTOP", "fixture-gen");
        let id = rec.record_id.clone();
        vault
            .write_record(rec, b"synthetic document bytes: meeting with the supplier")
            .unwrap();
        id
    };

    let header_bytes = std::fs::read(vault_root.join("VAULT/header/header_a.json")).unwrap();
    std::fs::write(format!("{}/header_a.json", out_dir), header_bytes).unwrap();
    for (name, id) in [
        ("memory-record.json", mem_id),
        ("document-record.json", doc_id),
    ] {
        let data = std::fs::read(
            vault_root
                .join("VAULT/records")
                .join(format!("{}.enc.json", id)),
        )
        .unwrap();
        std::fs::write(format!("{}/{}", out_dir, name), data).unwrap();
    }
    std::fs::write(
        format!("{}/fixture.json", out_dir),
        serde_json::to_string_pretty(&serde_json::json!({
            "spec": "unoone-vault-synthetic-fixture/1",
            "password_utf8": String::from_utf8_lossy(password).to_string(),
            "header_path": "header_a.json",
            "records": [{
                "path": "memory-record.json",
                "record_type": "MEMORY",
                "expected_content_utf8": "synthetic memory bytes: turmeric cardamom",
            }, {
                "path": "document-record.json",
                "record_type": "DOCUMENT",
                "expected_content_utf8": "synthetic document bytes: meeting with the supplier",
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    println!("synthetic-vault fixture written to {}", out_dir);
}

// Kotlin-authored envelope fixture — the bidirectional vault seal.
//
// The committed kotlin-authored-record.json is byte-for-byte what Android's
// MobileVaultRepository.writeRecord writes for the pinned inputs. Two CI
// tests hold the seal closed:
//   - Kotlin (MobileVaultRepositoryTest): unlocks the committed header and
//     asserts writeRecord with the pinned nonce reproduces the envelope
//     byte-for-byte — Kotlin AUTHORS these bytes on every Android CI run.
//   - Rust (tests/kotlin_envelope_read.rs): stages the same header + envelope
//     in a vault directory and reads it through the REAL Vault::open →
//     unlock → read_record path — Rust READS those bytes on every Desktop
//     CI run, including the aad_version-2 metadata-authentication check.
//
// This function only BOOTSTRAPS the bytes. If it ever emitted anything the
// real Kotlin writer would not produce, Android CI fails the byte equality;
// if it emitted anything the real Rust reader rejects, Desktop CI fails.
//
// It derives from the COMMITTED header (unwrapping the master key with the
// fixture password, exactly as unlock does), so running it alone does not
// churn the other fixtures.
#[allow(dead_code)]
fn gen_kotlin_envelope_fixture() {
    use unoone_vault_core::{EncryptedRecord, PrivacyLevel};

    let fixture_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/test-vectors/synthetic-vault");

    let header: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}/header_a.json", fixture_dir)).unwrap(),
    )
    .unwrap();
    let fixture: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}/fixture.json", fixture_dir)).unwrap(),
    )
    .unwrap();
    let password = fixture["password_utf8"].as_str().unwrap();

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    // Unwrap the fixture master key exactly as the unlock path does.
    let salt_v = hex_decode(header["salt"].as_str().unwrap());
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&salt_v);
    let kek = derive_key_encryption_key(password.as_bytes(), &salt).unwrap();
    let wrapped = hex_decode(header["wrapped_master_key"].as_str().unwrap());
    let wrap_nonce_v = hex_decode(header["wrap_nonce"].as_str().unwrap());
    let mut wrap_nonce = [0u8; NONCE_LEN];
    wrap_nonce.copy_from_slice(&wrap_nonce_v);
    let master = unwrap_master_key(&kek, &wrapped, &wrap_nonce).unwrap();
    let domain_key = derive_domain_key(&master, DOMAIN_RECORDS);

    // Self-check the unwrap against a committed record BEFORE authoring
    // anything with the derived key.
    {
        let rec0 = &fixture["records"][0];
        let env0: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/{}",
                fixture_dir,
                rec0["path"].as_str().unwrap()
            ))
            .unwrap(),
        )
        .unwrap();
        let n = hex_decode(env0["nonce"].as_str().unwrap());
        let mut n12 = [0u8; AES_GCM_NONCE_LEN];
        n12.copy_from_slice(&n);
        let ct = hex_decode(env0["encrypted_content"].as_str().unwrap());
        let aad0 = hex_decode(env0["associated_data"].as_str().unwrap());
        let pt = test_support::decrypt_aes256gcm_with_nonce(&domain_key, &n12, &ct, &aad0)
            .expect("master-key unwrap self-check: committed record must decrypt");
        assert_eq!(
            pt,
            rec0["expected_content_utf8"].as_str().unwrap().as_bytes(),
            "master-key unwrap self-check failed against the committed fixture record"
        );
    }

    // Pinned inputs. Content exercises UTF-8 passthrough through encryption;
    // metadata stays ASCII (string escaping is pinned by the `escaping`
    // vectors — this fixture seals the envelope plumbing, not escaping).
    let content = "android-authored bytes: नमस्ते from the phone vault";
    let content_hash = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(content.as_bytes());
        hex_encode(&h.finalize())
    };
    let record = Record {
        record_id: "7e9b2f4a-1c3d-4e5f-9a8b-2c4d6e8f0a1b".to_string(),
        record_type: RecordType::Memory,
        schema_version: 1,
        encryption_version: 1,
        created_at: "2026-08-01T13:00:00+00:00".to_string(),
        updated_at: "2026-08-01T13:00:00+00:00".to_string(),
        revision: 1,
        origin_platform: "ANDROID".to_string(),
        origin_device_id: "synthetic-android-fixture".to_string(),
        transaction_id: "3f8a1b2c-4d5e-4f60-8a9b-0c1d2e3f4a5b".to_string(),
        content_hash,
        parent_record_id: None,
        source_record_ids: Vec::new(),
        privacy_level: PrivacyLevel::Private,
        tombstone: false,
        deleted_at: None,
    };

    let aad = canonical_metadata_bytes(&record).unwrap();
    let nonce: [u8; AES_GCM_NONCE_LEN] = [
        0x0b, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4,
    ];
    let ct =
        test_support::encrypt_aes256gcm_with_nonce(&domain_key, &nonce, content.as_bytes(), &aad);
    let back = test_support::decrypt_aes256gcm_with_nonce(&domain_key, &nonce, &ct, &aad).unwrap();
    assert_eq!(back, content.as_bytes());

    // The envelope EXACTLY as MobileVaultRepository.writeRecord builds it:
    // metadata is the canonical AAD string itself, hex for all binary fields,
    // aad_version 2, single line, no trailing newline.
    let envelope = format!(
        "{{\"metadata\":{},\"encrypted_content\":\"{}\",\"nonce\":\"{}\",\"associated_data\":\"{}\",\"aad_version\":2}}",
        String::from_utf8(aad.clone()).unwrap(),
        hex_encode(&ct),
        hex_encode(&nonce),
        hex_encode(&aad),
    );

    // The read_record tamper check must hold on the authored bytes: parsing
    // the envelope and re-deriving the canonical bytes must reproduce the AAD.
    {
        let parsed: EncryptedRecord = serde_json::from_str(&envelope).unwrap();
        let recomputed = canonical_metadata_bytes(&parsed.metadata).unwrap();
        assert_eq!(
            recomputed, aad,
            "canonical metadata bytes must survive a parse round-trip"
        );
    }

    std::fs::write(
        format!("{}/kotlin-authored-record.json", fixture_dir),
        envelope.as_bytes(),
    )
    .unwrap();
    std::fs::write(
        format!("{}/kotlin-envelope-fixture.json", fixture_dir),
        serde_json::to_string_pretty(&serde_json::json!({
            "spec": "unoone-vault-kotlin-envelope/1",
            "purpose": "bidirectional seal: Kotlin authors this envelope byte-for-byte (Android CI), Rust reads it via the real read_record (Desktop CI)",
            "password_utf8": password,
            "header_path": "header_a.json",
            "envelope_path": "kotlin-authored-record.json",
            "record_id": record.record_id,
            "nonce_hex": hex_encode(&nonce),
            "content_utf8": content,
            "fields": serde_json::to_value(&record).unwrap(),
        }))
        .unwrap(),
    )
    .unwrap();
    println!("kotlin-envelope fixture written to {}", fixture_dir);
}
