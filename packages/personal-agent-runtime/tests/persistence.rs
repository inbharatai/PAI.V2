use unoone_personal_agent_contracts::{
    self as c,
    fold::{fold_task, FoldState},
    TaskTransition,
};
use unoone_personal_agent_runtime::*;
use unoone_vault_core::Vault;
use uuid::Uuid;
fn id() -> String {
    Uuid::new_v4().to_string()
}
const NOW: u64 = 1791629205000;
fn request(l: &Ledger, action: Action, task: Option<String>, text: &str) -> Request {
    Request {
        operation_id: id(),
        expected_revision: l.mutations.len() as u64,
        expected_replica_id: l.replica_id.clone(),
        action,
        task_id: task,
        text: text.into(),
        draft: "".into(),
        snooze_until_ms: None,
    }
}
#[test]
fn real_encrypted_vault_restart_persona_drafts_and_deletion() {
    let path = std::env::temp_dir().join(format!("personal-runtime-{}", id()));
    std::fs::create_dir_all(&path).unwrap();
    Vault::create(&path, b"private-ledger-test-password").unwrap();
    let mut vault = Vault::open(&path).unwrap();
    vault.unlock(b"private-ledger-test-password").unwrap();
    let mut ledger = load(&mut vault).unwrap();
    let initial = ledger.view().unwrap();
    let mut persona = request(&ledger, Action::Persona, None, "My private assistant");
    persona.draft = "Private preference marker: संक्षिप्त जवाब".into();
    ledger.apply(persona, NOW).unwrap();
    let tid = id();
    let mut create = request(
        &ledger,
        Action::Create,
        Some(tid.clone()),
        "Private task marker 7b",
    );
    create.draft = "Private draft marker 92".into();
    ledger.apply(create.clone(), NOW).unwrap();
    ledger.apply(create, NOW).unwrap(); // same user retry is no second task/event
    let accept = request(&ledger, Action::Accept, Some(tid.clone()), "");
    ledger.apply(accept, NOW).unwrap();
    save(&mut vault, &ledger).unwrap();
    let envelope =
        std::fs::read_to_string(path.join(format!("VAULT/records/{RECORD_ID}.enc.json"))).unwrap();
    for secret in [
        "My private assistant",
        "Private preference",
        "Private task",
        "Private draft",
        &initial.agent.person_id,
        &initial.replica_id,
    ] {
        assert!(!envelope.contains(secret));
    }
    vault.lock().unwrap();
    drop(vault);
    let mut vault = Vault::open(&path).unwrap();
    vault.unlock(b"private-ledger-test-password").unwrap();
    let mut reopened = load(&mut vault).unwrap();
    let v = reopened.view().unwrap();
    assert_eq!(initial.agent.agent_id, v.agent.agent_id);
    assert_eq!(initial.agent.person_id, v.agent.person_id);
    assert_eq!(initial.replica_id, v.replica_id);
    assert_eq!(v.tasks[0].draft, "Private draft marker 92");
    assert_eq!(v.tasks[0].status, "READY_FOR_REVIEW");
    assert!(!v.tasks[0].execute_on_hydration);
    let clear = request(&reopened, Action::ClearPersona, None, "");
    reopened.apply(clear, NOW).unwrap();
    let delete = request(&reopened, Action::Delete, Some(tid.clone()), "");
    reopened.apply(delete, NOW).unwrap();
    save(&mut vault, &reopened).unwrap();
    drop(vault);
    let mut vault = Vault::open(&path).unwrap();
    vault.unlock(b"private-ledger-test-password").unwrap();
    let mut deleted = load(&mut vault).unwrap();
    assert!(deleted.view().unwrap().tasks.is_empty());
    assert!(deleted.view().unwrap().persona.deleted);
    let recreate = request(&deleted, Action::Create, Some(tid), "stale resurrection");
    assert!(deleted.apply(recreate, NOW).is_err());
    drop(vault);
    std::fs::remove_dir_all(path).unwrap();
}
#[test]
fn independent_identity_no_adoption_and_stale_revision_collision() {
    let vault = id();
    let mut a = Ledger::fresh(&vault).unwrap();
    let b = Ledger::fresh(&id()).unwrap();
    assert_ne!(a.replica_id, b.replica_id);
    assert_ne!(
        a.view().unwrap().agent.person_id,
        b.view().unwrap().agent.person_id
    );
    assert!(Ledger::decode(&a.bytes().unwrap(), &b.local_vault_id).is_err());
    let tid = id();
    let r = request(&a, Action::Create, Some(tid), "manual");
    let stale = request(&a, Action::Persona, None, "stale");
    let mut foreign = r.clone();
    foreign.expected_replica_id = b.replica_id.clone();
    assert!(a.apply(foreign, NOW).is_err());
    a.apply(r.clone(), NOW).unwrap();
    let bytes = a.bytes().unwrap();
    assert!(a.apply(stale, NOW).is_err());
    let mut collision = r.clone();
    collision.text = "different".into();
    assert!(a.apply(collision, NOW).is_err());
    a.apply(r, NOW).unwrap();
    assert_eq!(a.bytes().unwrap(), bytes);
}
#[test]
fn edit_revokes_review_snooze_cancel_and_outbox_atomicity() {
    let mut l = Ledger::fresh(&id()).unwrap();
    let tid = id();
    let r = request(&l, Action::Create, Some(tid.clone()), "goal");
    l.apply(r, NOW).unwrap();
    let r = request(&l, Action::Accept, Some(tid.clone()), "");
    l.apply(r, NOW).unwrap();
    let r = request(&l, Action::Edit, Some(tid.clone()), "edited");
    l.apply(r, NOW).unwrap();
    assert_eq!(l.view().unwrap().tasks[0].status, "BLOCKED");
    let mut r = request(&l, Action::Snooze, Some(tid.clone()), "");
    r.snooze_until_ms = Some(NOW + 500);
    l.apply(r, NOW).unwrap();
    let roundtrip = Ledger::decode(&l.bytes().unwrap(), &l.local_vault_id).unwrap();
    assert_eq!(
        roundtrip.view().unwrap().tasks[0].snooze_until_ms,
        Some(NOW + 500)
    );
    let r = request(&l, Action::Cancel, Some(tid.clone()), "");
    l.apply(r, NOW).unwrap();
    let r = request(&l, Action::Accept, Some(tid), "");
    let before = l.bytes().unwrap();
    assert!(l.apply(r, NOW).is_err());
    assert_eq!(before, l.bytes().unwrap());
    assert_eq!(
        l.view().unwrap().pending_mutations as u64,
        l.view().unwrap().revision
    );
}
#[test]
fn causal_dedup_conflict_gaps_and_claimed_verification_are_inert() {
    let mut l = Ledger::fresh(&id()).unwrap();
    let tid = id();
    let r = request(&l, Action::Create, Some(tid.clone()), "goal");
    l.apply(r, NOW).unwrap();
    let r = request(&l, Action::Accept, Some(tid.clone()), "");
    l.apply(r, NOW).unwrap();
    let mut events = l.view().unwrap().tasks[0].events.clone();
    events.push(events[0].clone());
    let p = fold_task(&tid, &events, Some(&l.replica_id), &[]).unwrap();
    assert_eq!(
        p.state,
        FoldState::Transition(TaskTransition::ReadyForReview)
    );
    assert!(!p.execute_on_hydration);
    events.pop();
    for transition in [
        TaskTransition::InProgress,
        TaskTransition::AwaitingVerification,
        TaskTransition::Verified,
    ] {
        let previous = events.last().unwrap();
        let mut e = previous.clone();
        e.event_id = id();
        e.operation_id = id();
        e.predecessor_event_id = Some(previous.event_id.clone());
        e.step += 1;
        e.transition = transition;
        if transition == TaskTransition::Verified {
            e.evidence_ref = Some(id());
        }
        events.push(e);
    }
    assert_eq!(
        fold_task(&tid, &events, Some(&l.replica_id), &[])
            .unwrap()
            .state,
        FoldState::Transition(TaskTransition::AwaitingVerification)
    );
    let mut fork = events[1].clone();
    fork.event_id = id();
    fork.operation_id = id();
    events.push(fork);
    assert_eq!(
        fold_task(&tid, &events, Some(&l.replica_id), &[])
            .unwrap()
            .state,
        FoldState::Conflict
    );
    events.pop();
    events.remove(1);
    assert_eq!(
        fold_task(&tid, &events, Some(&l.replica_id), &[])
            .unwrap()
            .state,
        FoldState::MissingPredecessor
    );
    l.mutations[1].sequence = 99;
    assert!(l.view().is_err());
    // Encoding/decoding a receipt would still be a claim, not a native verification wrapper.
    let _ = c::SCHEMA;
}
