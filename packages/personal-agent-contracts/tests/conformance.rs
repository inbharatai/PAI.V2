use serde_json::{json, Value};
use unoone_personal_agent_contracts::{authority::*, fold::*, *};
fn fixture(name: &str) -> Document {
    decode(
        &std::fs::read(format!(
            "{}/fixtures/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap()
}
fn event() -> TaskEvent {
    let Record::TaskEvent(x) = fixture("task_event").record else {
        panic!()
    };
    x
}
fn receipt() -> TaskReceipt {
    let Record::TaskReceipt(x) = fixture("task_receipt").record else {
        panic!()
    };
    x
}
fn grant() -> CapabilityGrant {
    let Record::CapabilityGrant(x) = fixture("capability_grant").record else {
        panic!()
    };
    x
}
fn child() -> AgentSpec {
    let Record::AgentSpec(x) = fixture("agent_spec").record else {
        panic!()
    };
    x
}
fn raw(name: &str) -> Value {
    serde_json::to_value(fixture(name)).unwrap()
}
fn accepted(v: &Value) -> bool {
    decode(&serde_json::to_vec(v).unwrap()).is_ok()
}
#[test]
fn every_shared_golden_roundtrips_both_directions() {
    let mut count = 0;
    for entry in std::fs::read_dir(format!("{}/fixtures", env!("CARGO_MANIFEST_DIR"))).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json")
            || path.file_name().unwrap() == "negative_cases.json"
        {
            continue;
        }
        let bytes = std::fs::read(path).unwrap();
        let doc = decode(&bytes).unwrap();
        assert!(!doc.may_execute_on_hydration());
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            serde_json::from_slice::<Value>(&encode(&doc).unwrap()).unwrap()
        );
        assert_eq!(doc, decode(&encode(&doc).unwrap()).unwrap());
        count += 1;
    }
    assert_eq!(count, 14);
}
#[test]
fn shared_negative_mutations() {
    let cases: Value =
        serde_json::from_str(include_str!("../fixtures/negative_cases.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let mut v = raw(case["fixture"].as_str().unwrap());
        let pointer = case["pointer"].as_str().unwrap();
        if case["remove"].as_bool() == Some(true) {
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            v.pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
        } else if let Some(slot) = v.pointer_mut(pointer) {
            *slot = case["value"].clone();
        } else {
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            v.pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(key.into(), case["value"].clone());
        }
        assert!(!accepted(&v), "{}", case["name"]);
    }
}
#[test]
fn hard_limits_and_duplicate_keys() {
    let mut v = raw("task_spec");
    v["payload"]["goal"] = json!("a".repeat(4097));
    assert!(!accepted(&v));
    v = raw("personal_agent");
    v["payload"]["conversation_refs"] = json!(vec!["thread"; 65]);
    assert!(!accepted(&v));
    assert!(decode(&vec![b' '; 65537]).is_err());
    assert!(decode(&[0xff]).is_err());
    v = raw("personal_agent");
    v["payload"]["conversation_refs"] = json!(vec!["a".repeat(4096); 64]);
    assert!(!accepted(&v));
    assert!(decode(format!("{}0{}", "[".repeat(13), "]".repeat(13)).as_bytes()).is_err());
    let input = String::from_utf8(encode(&fixture("personal_agent")).unwrap()).unwrap();
    assert!(decode(
        input
            .replacen("\"agent_id\":", "\"agent_id\":\"evil\",\"agent_id\":", 1)
            .as_bytes()
    )
    .is_err());
    assert!(decode(
        input
            .replacen(
                "\"agent_id\":",
                "\"agent_id\":\"evil\",\"agent_\\u0069d\":",
                1
            )
            .as_bytes()
    )
    .is_err());
}
#[test]
fn read_only_defaults_are_not_missing_authority_defaults() {
    let mut v = raw("capability_grant");
    let scopes = v["payload"]["scopes"].as_object_mut().unwrap();
    scopes.remove("delegation");
    scopes.remove("network");
    let doc = decode(&serde_json::to_vec(&v).unwrap()).unwrap();
    let Record::CapabilityGrant(g) = doc.record else {
        panic!()
    };
    assert_eq!(g.scopes.delegation, DelegationLevel::ReadAndSuggest);
    assert_eq!(g.scopes.network, NetworkPolicy::OfflineOnly);
    v["payload"]["scopes"]["operations"] = json!(["SEND"]);
    assert!(!accepted(&v));
}
#[test]
fn child_intersection_and_budget_expiry_attenuation() {
    let g = grant();
    let local = approve_locally(g.clone(), "phone-1", "human-approval-1", 2000, 0).unwrap();
    let mut requested = child();
    requested.budget.max_steps = 100;
    requested.budget.max_bytes = 100000;
    requested.expires_at_ms = 100000;
    requested.scopes.tools.push("unapproved_tool".into());
    requested
        .scopes
        .recipients
        .push("stranger@example.invalid".into());
    let out = attenuate_child(&requested, &local, &g.scopes, "phone-1", 2000, 0).unwrap();
    assert_eq!(out.scopes.tools, g.scopes.tools);
    assert!(out.scopes.recipients.is_empty());
    assert_eq!(out.budget.max_steps, 8);
    assert_eq!(out.budget.max_bytes, 8192);
    assert_eq!(out.budget.max_duration_ms, 59000);
    assert_eq!(out.expires_at_ms, 61000);
    assert!(attenuate_child(&requested, &local, &g.scopes, "power-1", 2000, 0).is_err());
    assert!(attenuate_child(&requested, &local, &g.scopes, "phone-1", 61000, 0).is_err());
    assert!(attenuate_child(&requested, &local, &g.scopes, "phone-1", 2000, 1).is_err());
    let mut revoked = g;
    revoked.revoked = true;
    assert!(approve_locally(revoked, "phone-1", "approval", 2000, 0).is_err());
}
#[test]
fn data_scope_intersection_is_exact_and_operation_bounded() {
    let mut a = grant().scopes;
    a.delegation = DelegationLevel::ActWithinScope;
    a.operations = vec![Operation::Read, Operation::Send];
    a.data[0].operations = a.operations.clone();
    let mut b = a.clone();
    b.operations = vec![Operation::Read];
    b.data[0].operations = b.operations.clone();
    assert_eq!(a.intersect(&b).data[0].operations, vec![Operation::Read]);
    b.data[0].resource_id = "mail-1/subfolder".into();
    assert!(a.intersect(&b).data.is_empty());
}
fn native(r: &TaskReceipt) -> NativeObservation {
    NativeObservation {
        task_id: r.task_id.clone(),
        operation_id: r.operation_id.clone(),
        replica_id: r.replica_id.clone(),
        evidence_ref: r.after_evidence_refs[0].clone(),
        postcondition_matched: true,
        external_object_id: r.external_object_id.clone(),
    }
}
#[test]
fn verified_claim_is_not_native_verification() {
    let mut r = receipt();
    assert!(verify_native(&r, native(&r), "phone-1").is_err()); // composer only
    r.dispatch_intent = DispatchIntent::ProviderMutation;
    r.external_object_id = Some("saved-object-1".into());
    r.source = ProvenanceSource::Model;
    assert!(verify_native(&r, native(&r), "phone-1").is_err());
    r.source = ProvenanceSource::Native;
    let verified = verify_native(&r, native(&r), "phone-1").unwrap();
    assert_eq!(verified.receipt().outcome, ReceiptOutcome::Verified);
    let mut mismatch = native(&r);
    mismatch.external_object_id = Some("different".into());
    assert!(verify_native(&r, mismatch, "phone-1").is_err());
    r.external_object_id = None;
    assert!(verify_native(&r, native(&r), "phone-1").is_err());
}
fn chain() -> Vec<TaskEvent> {
    let root = event();
    let mut events = vec![root.clone()];
    for (n, t) in [
        TaskTransition::InProgress,
        TaskTransition::AwaitingVerification,
        TaskTransition::Verified,
    ]
    .into_iter()
    .enumerate()
    {
        let mut e = root.clone();
        e.event_id = format!("event-{}", n + 2);
        e.operation_id = format!("op-{}", n + 2);
        e.predecessor_event_id = Some(events.last().unwrap().event_id.clone());
        e.step = (n + 1) as u64;
        e.transition = t;
        if t == TaskTransition::Verified {
            e.evidence_ref = Some("evidence-1".into());
        }
        events.push(e);
    }
    events
}
#[test]
fn causal_fold_is_permutation_invariant_idempotent_and_inert() {
    let mut events = chain();
    let expected = fold_task("task-1", &events, Some("phone-1"), &[]).unwrap();
    assert_eq!(
        expected.state,
        FoldState::Transition(TaskTransition::AwaitingVerification)
    );
    events.reverse();
    events.push(events[0].clone());
    assert_eq!(
        expected,
        fold_task("task-1", &events, Some("phone-1"), &[]).unwrap()
    );
    assert!(!expected.execute_on_hydration);
    assert_eq!(
        FoldState::WaitingForOwner,
        fold_task("task-1", &events, None, &[]).unwrap().state
    );
    events[0].assigned_replica_id = Some("power-1".into());
    events.pop();
    assert_eq!(
        FoldState::WaitingForOwner,
        fold_task("task-1", &events, Some("phone-1"), &[])
            .unwrap()
            .state
    );
}
#[test]
fn fold_conflicts_missing_predecessor_and_illegal_transitions() {
    let e = event();
    let mut collision = e.clone();
    collision.transition = TaskTransition::Blocked;
    assert_eq!(
        FoldState::Conflict,
        fold_task("task-1", &[e.clone(), collision], Some("phone-1"), &[])
            .unwrap()
            .state
    );
    let mut fork = chain();
    let mut side = fork[1].clone();
    side.event_id = "branch".into();
    side.operation_id = "branch-op".into();
    fork.push(side);
    assert_eq!(
        FoldState::Conflict,
        fold_task("task-1", &fork, Some("phone-1"), &[])
            .unwrap()
            .state
    );
    assert_eq!(
        FoldState::MissingPredecessor,
        fold_task("task-1", &chain()[1..], Some("phone-1"), &[])
            .unwrap()
            .state
    );
    let mut illegal = chain();
    illegal[1].transition = TaskTransition::Cancelled;
    assert_eq!(
        FoldState::Conflict,
        fold_task("task-1", &illegal, Some("phone-1"), &[])
            .unwrap()
            .state
    );
    let mut operation_collision = e.clone();
    operation_collision.event_id = "different".into();
    assert_eq!(
        FoldState::Conflict,
        fold_task("task-1", &[e, operation_collision], Some("phone-1"), &[])
            .unwrap()
            .state
    );
}
#[test]
fn only_native_wrapper_can_raise_projection_to_verified() {
    let events = chain();
    let mut r = receipt();
    r.operation_id = "op-4".into();
    r.dispatch_intent = DispatchIntent::LocalMutation;
    let verified = verify_native(&r, native(&r), "phone-1").unwrap();
    let projected = fold_task("task-1", &events, Some("phone-1"), &[verified]).unwrap();
    assert_eq!(
        projected.state,
        FoldState::Transition(TaskTransition::Verified)
    );
    assert!(!projected.execute_on_hydration);
}
#[test]
fn existing_tool_id_is_not_renamed_by_contract() {
    let tools: Value =
        serde_json::from_str(include_str!("../../tool-contracts/tools.v1.json")).unwrap();
    for id in grant().scopes.tools {
        assert!(tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == id));
    }
}
