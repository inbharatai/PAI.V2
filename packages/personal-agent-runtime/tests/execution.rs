//! Actual encrypted file effects and native task authority; no model accuracy claim.
use unoone_personal_agent_runtime::{execution::*, *};
use unoone_vault_core::Vault;
use uuid::Uuid;
fn id() -> String {
    Uuid::new_v4().to_string()
}
const NOW: u64 = 1791629205000;
fn apply(l: &mut Ledger, action: Action, tid: &str, text: &str) {
    let request = Request {
        operation_id: id(),
        expected_revision: l.view().unwrap().revision,
        expected_replica_id: l.replica_id.clone(),
        action,
        task_id: Some(tid.into()),
        text: text.into(),
        draft: String::new(),
        snooze_until_ms: None,
    };
    l.apply(request, NOW).unwrap();
}
fn accepted(l: &mut Ledger) -> String {
    let tid = id();
    apply(l, Action::Create, &tid, "Prepare a private draft");
    apply(l, Action::Accept, &tid, "");
    tid
}
#[test]
fn actual_vault_attempt_survives_restart_no_replay_no_verified_promotion() {
    let root = std::env::temp_dir().join(format!("personal-execution-{}", id()));
    std::fs::create_dir_all(&root).unwrap();
    Vault::create(&root, b"test-native-draft-password").unwrap();
    let mut vault = Vault::open(&root).unwrap();
    vault.unlock(b"test-native-draft-password").unwrap();
    let mut ledger = load(&mut vault).unwrap();
    let tid = accepted(&mut ledger);
    let view = ledger.view().unwrap();
    let permit = approve_draft(&view, &tid, view.revision, NOW, 7).unwrap();
    assert_eq!(permit.grant().grant().budget.max_tool_calls, 0);
    assert!(permit.grant().grant().scopes.data.is_empty());
    ledger
        .record_draft_attempt(
            view.revision,
            &tid,
            DraftPhase::Started,
            "",
            &permit,
            NOW,
            7,
        )
        .unwrap();
    save(&mut vault, &ledger).unwrap();
    vault.lock().unwrap();
    assert!(load(&mut vault).is_err());
    vault.unlock(b"test-native-draft-password").unwrap();
    let mut restored = load(&mut vault).unwrap();
    let view = restored.view().unwrap();
    assert_eq!(view.tasks[0].status, "IN_PROGRESS");
    assert!(!view.tasks[0].execute_on_hydration);
    assert!(approve_draft(&view, &tid, view.revision, NOW + 1, 7).is_err());
    assert!(restored
        .record_draft_attempt(
            view.revision,
            &tid,
            DraftPhase::Responded,
            "discard me",
            &permit,
            NOW + 1,
            8
        )
        .is_err());
    restored
        .record_draft_attempt(
            view.revision,
            &tid,
            DraftPhase::Responded,
            "TEST MODEL output — facts remain unverified",
            &permit,
            NOW + 1,
            7,
        )
        .unwrap();
    save(&mut vault, &restored).unwrap();
    drop(vault);
    let encrypted =
        std::fs::read_to_string(root.join(format!("VAULT/records/{RECORD_ID}.enc.json"))).unwrap();
    assert!(!encrypted.contains("TEST MODEL"));
    let mut vault = Vault::open(&root).unwrap();
    vault.unlock(b"test-native-draft-password").unwrap();
    let view = load(&mut vault).unwrap().view().unwrap();
    assert_eq!(view.tasks[0].status, "AWAITING_VERIFICATION");
    assert!(view.tasks[0].draft.contains("RESPONDED"));
    assert!(view.tasks[0]
        .events
        .iter()
        .all(|e| e.transition != unoone_personal_agent_contracts::TaskTransition::Verified));
    drop(vault);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn changed_foreign_conflicted_expired_and_cross_task_permits_fail_closed() {
    let mut ledger = Ledger::fresh(&id()).unwrap();
    let first = accepted(&mut ledger);
    let second = accepted(&mut ledger);
    let mut view = ledger.view().unwrap();
    let permit = approve_draft(&view, &first, view.revision, NOW, 1).unwrap();
    assert!(ledger
        .record_draft_attempt(
            view.revision,
            &second,
            DraftPhase::Started,
            "",
            &permit,
            NOW,
            1
        )
        .is_err());
    assert!(approve_draft(&view, &first, view.revision - 1, NOW, 1).is_err());
    assert!(approve_draft(&view, &first, view.revision, NOW + 86_400_001, 1).is_err());
    view.conflicts.push("conflict".into());
    assert!(approve_draft(&view, &first, view.revision, NOW, 1).is_err());
    view.conflicts.clear();
    let index = view
        .tasks
        .iter()
        .position(|t| t.spec.task_id == first)
        .unwrap();
    view.tasks[index].owner_replica_id = Some(id());
    assert!(approve_draft(&view, &first, view.revision, NOW, 1).is_err());
}
#[test]
fn shared_owner_attempt_is_causal_and_remote_replica_cannot_execute() {
    use unoone_personal_agent_runtime::shared::SharedIdentity;
    let mut a = Ledger::fresh(&id()).unwrap();
    let mut b = Ledger::fresh(&id()).unwrap();
    let identity = SharedIdentity {
        person_id: id(),
        agent_id: id(),
        founder_replica_id: a.replica_id.clone(),
        replicas: [
            (a.replica_id.clone(), "a".repeat(64)),
            (b.replica_id.clone(), "b".repeat(64)),
        ]
        .into(),
    };
    a.adopt(identity.clone(), true).unwrap();
    b.adopt(identity, true).unwrap();
    let tid = accepted(&mut a);
    let v = a.view().unwrap();
    let permit = approve_draft(&v, &tid, v.revision, NOW, 1).unwrap();
    a.record_draft_attempt(v.revision, &tid, DraftPhase::Started, "", &permit, NOW, 1)
        .unwrap();
    let v = a.view().unwrap();
    a.record_draft_attempt(
        v.revision,
        &tid,
        DraftPhase::Responded,
        "TEST draft",
        &permit,
        NOW + 1,
        1,
    )
    .unwrap();
    b.shared.as_mut().unwrap().operations = a.shared.as_ref().unwrap().operations.clone();
    let v = b.view().unwrap();
    assert_eq!(v.tasks[0].status, "AWAITING_VERIFICATION");
    assert!(approve_draft(&v, &tid, v.revision, NOW + 2, 1).is_err());
    assert!(!v.tasks[0].execute_on_hydration);
}

#[test]
fn guardian_note_uses_existing_edit_outbox_and_rejects_unprefixed_or_oversized_notes() {
    let mut ledger = Ledger::fresh(&id()).unwrap();
    let tid = accepted(&mut ledger);
    let revision = ledger.view().unwrap().revision;
    assert!(ledger
        .record_guardian_note(revision, &tid, "free text pretending to be a receipt", NOW)
        .is_err());
    assert!(ledger
        .record_guardian_note(
            revision,
            &tid,
            &format!("GUARDIAN RECEIPT v1 {}", "x".repeat(4000)),
            NOW
        )
        .is_err());
    assert!(ledger
        .record_guardian_note(revision + 1, &tid, "GUARDIAN RECEIPT v1 · stale", NOW)
        .is_err());
    ledger
        .record_guardian_note(
            revision,
            &tid,
            "GUARDIAN RECEIPT v1 · SEND_MESSAGE · Warn · NOT PERFORMED\n{}",
            NOW,
        )
        .unwrap();
    let view = ledger.view().unwrap();
    let task = view.tasks.iter().find(|t| t.spec.task_id == tid).unwrap();
    assert!(task.draft.starts_with("GUARDIAN RECEIPT v1"));
    assert_eq!(task.spec.goal, "Prepare a private draft", "goal untouched");
    assert_eq!(
        task.status, "BLOCKED",
        "a receipt on a reviewed task holds it for review; no grant"
    );
    assert_eq!(view.revision, revision + 1);
    ledger
        .record_guardian_note(
            revision + 1,
            &tid,
            "GUARDIAN CORRECTION v1 · FalseAlarm · fp\n{}",
            NOW,
        )
        .unwrap();
    assert!(ledger.view().unwrap().tasks[0]
        .draft
        .starts_with("GUARDIAN CORRECTION v1"));
}
