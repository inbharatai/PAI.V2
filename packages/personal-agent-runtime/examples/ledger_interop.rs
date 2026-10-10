//! Synthetic contract interoperability probe; no production import or identity adoption API.
use unoone_personal_agent_runtime::*;
use uuid::Uuid;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = std::path::Path::new(&args[2]);
    if args[1] == "emit" {
        let mut l = Ledger::fresh(&Uuid::new_v4().to_string()).unwrap();
        let r = Request {
            operation_id: Uuid::new_v4().to_string(),
            expected_revision: 1,
            expected_replica_id: l.replica_id.clone(),
            action: Action::Create,
            task_id: Some(Uuid::new_v4().to_string()),
            text: "Shared Unicode goal नमस्ते".into(),
            draft: "Rust draft".into(),
            snooze_until_ms: None,
        };
        l.apply(r, 1791629205000).unwrap();
        std::fs::write(path.join("rust-ledger.json"), l.bytes().unwrap()).unwrap();
    } else {
        let original: Ledger =
            serde_json::from_slice(&std::fs::read(path.join("rust-ledger.json")).unwrap()).unwrap();
        let l = Ledger::decode(
            &std::fs::read(path.join("kotlin-ledger.json")).unwrap(),
            &original.local_vault_id,
        )
        .unwrap();
        let v = l.view().unwrap();
        assert_eq!(v.agent.agent_id, original.view().unwrap().agent.agent_id);
        assert_eq!(v.replica_id, original.replica_id);
        assert_eq!(v.tasks[0].draft, "Kotlin draft हिन्दी");
        assert_eq!(v.tasks[0].status, "READY_FOR_REVIEW");
        assert!(!v.tasks[0].execute_on_hydration);
        println!("Rust → Kotlin mutation → Rust: shared identity/contracts/causal ledger passed");
    }
}
