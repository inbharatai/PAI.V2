// The bidirectional vault seal — Rust half.
//
// The committed kotlin-authored-record.json is byte-for-byte what Android's
// MobileVaultRepository.writeRecord writes for the pinned fixture inputs
// (MobileVaultRepositoryTest asserts that byte equality on every Android CI
// run). This file proves the desktop reads that exact envelope through the
// REAL vault path — Vault::open on a directory holding the committed header,
// the real unlock (Argon2id KEK → header HMAC → master-key unwrap), and the
// real read_record with its aad_version-2 canonical-metadata authentication —
// not a re-implementation of any of it.
//
// Together the two halves are the difference between "the crypto primitives
// match on paper" and "a record written on the phone is provably readable on
// the laptop".

use unoone_vault_core::{Record, Vault, VaultError};

fn fixture_dir() -> std::path::PathBuf {
    std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-vectors/synthetic-vault"
    ))
    .to_path_buf()
}

fn read_fixture_json(name: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_dir().join(name)).unwrap()).unwrap()
}

/// Stage a vault directory holding the committed header and the committed
/// Kotlin-authored envelope, exactly as they sit in the repository.
fn stage_vault(tmp: &std::path::Path, fixture: &serde_json::Value) -> (std::path::PathBuf, String) {
    let root = tmp.join("UNOONE");
    for dir in [
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
    ] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    let header =
        std::fs::read(fixture_dir().join(fixture["header_path"].as_str().unwrap())).unwrap();
    std::fs::write(root.join("VAULT/header/header_a.json"), header).unwrap();

    let envelope =
        std::fs::read(fixture_dir().join(fixture["envelope_path"].as_str().unwrap())).unwrap();
    let record_id = fixture["record_id"].as_str().unwrap().to_string();
    std::fs::write(
        root.join("VAULT/records")
            .join(format!("{}.enc.json", record_id)),
        envelope,
    )
    .unwrap();
    (root, record_id)
}

#[test]
fn kotlin_authored_envelope_reads_through_the_real_vault_path() {
    let fixture = read_fixture_json("kotlin-envelope-fixture.json");
    let tmp = tempfile::tempdir().unwrap();
    let (root, record_id) = stage_vault(tmp.path(), &fixture);

    let mut vault = Vault::open(&root).expect("open vault from the committed header");
    vault
        .unlock(fixture["password_utf8"].as_str().unwrap().as_bytes())
        .expect("unlock with the committed fixture password");

    let (record, content) = vault
        .read_record(&record_id)
        .expect("the Kotlin-authored envelope must decrypt through read_record");

    // Content matches byte-for-byte (UTF-8 including Devanagari).
    assert_eq!(
        content,
        fixture["content_utf8"].as_str().unwrap().as_bytes()
    );

    // Every metadata field matches the pinned fixture. The fixture's `fields`
    // object is the serde view of the same schema, so deserialize it and
    // compare whole structs — any drifted field fails loudly.
    let expected: Record = serde_json::from_value(fixture["fields"].clone()).unwrap();
    assert_eq!(record, expected);
}

#[test]
fn tampered_metadata_on_the_kotlin_envelope_is_rejected() {
    let fixture = read_fixture_json("kotlin-envelope-fixture.json");
    let tmp = tempfile::tempdir().unwrap();
    let (root, record_id) = stage_vault(tmp.path(), &fixture);

    // Flip one plaintext-metadata value on disk after the write — the exact
    // attack the aad_version-2 metadata-authentication check exists to stop.
    // Without the tamper check this record would still decrypt cleanly.
    let path = root
        .join("VAULT/records")
        .join(format!("{}.enc.json", record_id));
    let envelope = std::fs::read_to_string(&path).unwrap();
    let tampered = envelope.replace("\"revision\":1", "\"revision\":9");
    assert_ne!(
        envelope, tampered,
        "tamper substitution must actually change the envelope"
    );
    std::fs::write(&path, tampered).unwrap();

    let mut vault = Vault::open(&root).unwrap();
    vault
        .unlock(fixture["password_utf8"].as_str().unwrap().as_bytes())
        .unwrap();

    match vault.read_record(&record_id) {
        Err(VaultError::DecryptionFailed(msg)) => {
            assert!(
                msg.contains("altered"),
                "expected the metadata-tamper rejection, got: {msg}"
            );
        }
        Err(other) => panic!("expected DecryptionFailed for tampered metadata, got: {other:?}"),
        Ok((_, content)) => panic!(
            "tampered metadata was accepted and decrypted {} bytes — the seal is broken",
            content.len()
        ),
    }
}
