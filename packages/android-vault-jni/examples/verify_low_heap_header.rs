// Test-only synthetic low-Java-heap export from LowHeapNativeVault; no user secrets.
use unoone_vault_core::Vault;
fn main() {
    let root = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("synthetic low-heap export path"),
    );
    let mut vault = Vault::open(&root).expect("open actual low-heap Kotlin-created header");
    vault
        .unlock(b"synthetic low heap native vault phrase")
        .expect("authenticate unchanged header HMAC and unwrap");
    vault.lock().expect("lock");
    println!(
        "PASS: real Rust vault-core opens/authenticates actual JNI low-Java-heap Kotlin header"
    );
}
