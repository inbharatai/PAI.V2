//! Scoped execution owner's fixture: real encrypted stores, no account/tokens/network.
use unoone_personal_agent_runtime::{
    self as runtime, execution::reviewed_task_body, Action, Request,
};
use unoone_personal_provider_adapters::{store, *};
use unoone_vault_core::Vault;
#[test]
fn exact_local_task_draft_handoff_is_typed_prepared_never_verified_sent() {
    let dir = tempfile::tempdir().unwrap();
    Vault::create(dir.path(), b"test-prepared-handoff").unwrap();
    let mut vault = Vault::open(dir.path()).unwrap();
    vault.unlock(b"test-prepared-handoff").unwrap();
    let mut ledger = runtime::load(&mut vault).unwrap();
    let task = uuid::Uuid::new_v4().to_string();
    let now = now_ms();
    ledger
        .apply(
            Request {
                operation_id: uuid::Uuid::new_v4().to_string(),
                expected_revision: ledger.view().unwrap().revision,
                expected_replica_id: ledger.replica_id.clone(),
                action: Action::Create,
                task_id: Some(task.clone()),
                text: "Draft to test recipient".into(),
                draft: "TEST ONLY body from task".into(),
                snooze_until_ms: None,
            },
            now,
        )
        .unwrap();
    runtime::save(&mut vault, &ledger).unwrap();
    let view = ledger.view().unwrap();
    let body = view.tasks[0].draft.clone();
    reviewed_task_body(&view, &task, view.revision, &body).unwrap();
    let review = Review {
        operation_id: uuid::Uuid::new_v4().to_string(),
        task_id: task.clone(),
        account: "fixture@example.test".into(),
        container: "INBOX".into(),
        owner_replica: view.replica_id,
        prepared_ms: now,
        mutation: Mutation::SaveDraft {
            draft: Draft {
                to: vec!["recipient@example.test".into()],
                subject: "Fixture only".into(),
                body: body.clone(),
                reply: None,
            },
        },
    };
    let mut local = store::load(&mut vault).unwrap();
    local.prepare(review.clone(), now).unwrap();
    store::task_note(
        &mut vault,
        &review,
        "PREPARED from exact task; no external effect",
    )
    .unwrap();
    store::save(&mut vault, &local).unwrap();
    assert_eq!(local.entries[0].status, CommitStatus::Prepared);
    assert!(local.entries[0].receipt.is_none());
    assert!(local.tokens.is_none());
    drop(vault);
    let mut reopened = Vault::open(dir.path()).unwrap();
    reopened.unlock(b"test-prepared-handoff").unwrap();
    let entry = store::load(&mut reopened).unwrap().entries.remove(0);
    assert_eq!(entry.review.task_id, task);
    assert!(matches!(entry.review.mutation, Mutation::SaveDraft { .. }));
    assert_eq!(entry.status, CommitStatus::Prepared);
    assert!(entry.receipt.is_none());
    assert!(!std::fs::read_to_string(
        dir.path()
            .join(format!("VAULT/records/{}.enc.json", store::RECORD_ID))
    )
    .unwrap()
    .contains(&body));
}
