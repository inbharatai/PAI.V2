use unoone_personal_agent_runtime::{load, RECORD_ID};
use unoone_vault_core::Vault;
use uuid::Uuid;

#[test]
fn independent_real_vaults_reject_foreign_ciphertext_and_never_reset_corruption() {
    let root = std::env::temp_dir().join(format!("personal-independent-{}", Uuid::new_v4()));
    let left = root.join("left");
    let right = root.join("right");
    std::fs::create_dir_all(&left).unwrap();
    std::fs::create_dir_all(&right).unwrap();
    Vault::create(&left, b"first-independent-password").unwrap();
    Vault::create(&right, b"second-independent-password").unwrap();
    let mut a = Vault::open(&left).unwrap();
    a.unlock(b"first-independent-password").unwrap();
    let mut b = Vault::open(&right).unwrap();
    b.unlock(b"second-independent-password").unwrap();
    let la = load(&mut a).unwrap();
    let lb = load(&mut b).unwrap();
    assert_ne!(la.local_vault_id, lb.local_vault_id);
    assert_ne!(la.replica_id, lb.replica_id);
    assert_ne!(
        la.view().unwrap().agent.person_id,
        lb.view().unwrap().agent.person_id
    );
    let rel = format!("VAULT/records/{RECORD_ID}.enc.json");
    // Deliberately foreign ciphertext, NOT a supported import or key-copy path.
    let foreign = std::fs::read(left.join(&rel)).unwrap();
    std::fs::write(right.join(&rel), &foreign).unwrap();
    assert!(load(&mut b).is_err());
    assert_eq!(std::fs::read(right.join(&rel)).unwrap(), foreign);
    std::fs::write(right.join(&rel), b"malformed retained ledger").unwrap();
    assert!(load(&mut b).is_err());
    assert_eq!(
        std::fs::read(right.join(&rel)).unwrap(),
        b"malformed retained ledger"
    );
    b.lock().unwrap();
    assert!(load(&mut b).is_err());
    drop(a);
    drop(b);
    std::fs::remove_dir_all(root).unwrap();
}
