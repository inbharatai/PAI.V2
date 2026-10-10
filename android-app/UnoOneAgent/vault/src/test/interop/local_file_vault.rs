// Host-only proof: Rust opens a freshly Kotlin-created header + live record + tombstone.
// Invoke with the synthetic export emitted by LocalFileVaultTest, NEVER real user data.
use unoone_vault_core::Vault;

fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("synthetic export path"));
    let mut vault = Vault::open(&root).expect("Rust opens Kotlin-created header");
    vault
        .unlock(b"synthetic local vault phrase only")
        .expect("same-format header HMAC and master-key wrap");
    let (live, content) = vault
        .read_record("577eb7bd-cab0-4f79-a217-e606239bfe63")
        .expect("Kotlin AES-GCM/AAD live record");
    assert!(!live.tombstone);
    assert_eq!(content, "Local Kotlin file vault — नमस्ते".as_bytes());
    assert!(
        matches!(
            vault.read_record("dc593827-0945-48c2-a322-f574cedf63aa"),
            Err(unoone_vault_core::VaultError::NotPermitted(_))
        ),
        "Rust must suppress the Kotlin tombstone"
    );
    vault.lock().expect("lock synthetic vault");
    println!("PASS: Rust Vault::open/unlock/read_record authenticated fresh Kotlin header and live Unicode payload; tombstone read suppressed");
}
