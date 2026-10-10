//! Synthetic protocol fixtures only. Never stores/ships test TLS private keys.
use std::{fs, path::PathBuf};
use unoone_local_peer_sync::*;
use unoone_personal_agent_runtime::{Action, Ledger, Request};
use uuid::Uuid;
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let dir = PathBuf::from(&args[2]);
    if args[1] == "emit" {
        let mut a = Ledger::fresh(&Uuid::new_v4().to_string()).unwrap();
        let b = Ledger::fresh(&Uuid::new_v4().to_string()).unwrap();
        a.apply(
            Request {
                operation_id: Uuid::new_v4().to_string(),
                expected_revision: 1,
                expected_replica_id: a.replica_id.clone(),
                action: Action::Create,
                task_id: Some(Uuid::new_v4().to_string()),
                text: "fixture Rust goal नमस्ते".into(),
                draft: "fixture Rust draft".into(),
                snooze_until_ms: None,
            },
            10000,
        )
        .unwrap();
        let mut sa = State::fresh(&a).unwrap();
        let sb = State::fresh(&b).unwrap();
        sa.approve(
            sb.local.clone(),
            Selection {
                persona: true,
                task_ids: a
                    .mutations
                    .iter()
                    .filter_map(|m| m.request.as_ref().and_then(|r| r.task_id.clone()))
                    .collect(),
            },
            IdentityChoice::KeepSeparateReview,
            true,
            &a,
        )
        .unwrap();
        for (name, bytes) in [
            ("rust-local-a.json", a.bytes().unwrap()),
            ("rust-local-b.json", b.bytes().unwrap()),
            ("rust-offer-a.json", serde_json::to_vec(&sa.local).unwrap()),
            ("rust-offer-b.json", serde_json::to_vec(&sb.local).unwrap()),
            (
                "rust-page.json",
                serde_json::to_vec(&sa.page(&a, 0).unwrap()).unwrap(),
            ),
        ] {
            fs::write(dir.join(name), bytes).unwrap();
        }
        fs::write(dir.join("rust-vault-a.txt"), &a.local_vault_id).unwrap();
        fs::write(dir.join("rust-vault-b.txt"), &b.local_vault_id).unwrap();
        println!("Emitted synthetic selected record pages; no private TLS keys exported");
    } else {
        let a = Ledger::decode(
            &fs::read(dir.join("rust-local-a.json")).unwrap(),
            &fs::read_to_string(dir.join("rust-vault-a.txt")).unwrap(),
        )
        .unwrap();
        let mut s = State::fresh(&a).unwrap();
        s.local =
            serde_json::from_slice(&fs::read(dir.join("rust-offer-a.json")).unwrap()).unwrap();
        let peer: Offer =
            serde_json::from_slice(&fs::read(dir.join("rust-offer-b.json")).unwrap()).unwrap();
        s.approve(
            peer,
            Selection {
                persona: false,
                task_ids: vec![],
            },
            IdentityChoice::KeepSeparateReview,
            true,
            &a,
        )
        .unwrap();
        let bytes = fs::read(dir.join("kotlin-page.json")).unwrap();
        json_guard::preflight(&bytes, MAX_BODY).unwrap();
        let page: Page = serde_json::from_slice(&bytes).unwrap();
        let received = s.receive(&page).unwrap();
        let tasks = received.remote_tasks().unwrap();
        assert_eq!(tasks[0].goal, "समीक्षा local task");
        assert!(!tasks[0].execute_on_hydration);
        let original: Page =
            serde_json::from_slice(&fs::read(dir.join("rust-page.json")).unwrap()).unwrap();
        let round: Vec<Change> =
            serde_json::from_slice(&fs::read(dir.join("kotlin-received.json")).unwrap()).unwrap();
        assert_eq!(original.changes, round);
        println!("Rust -> Kotlin -> Rust selected changes and exact payload hashes PASS; no hydration execution");
    }
}
