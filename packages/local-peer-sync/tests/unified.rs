use std::{net::TcpListener, sync::Arc};
use unoone_local_peer_sync::*;
use unoone_personal_agent_runtime::{self as runtime, Action, Ledger, Request};
use unoone_vault_core::Vault;
use uuid::Uuid;
fn id() -> String {
    Uuid::new_v4().to_string()
}
fn edit(l: &mut Ledger, action: Action, tid: Option<&str>, text: &str, draft: &str) {
    l.apply(
        Request {
            operation_id: id(),
            expected_revision: l.view().unwrap().revision,
            expected_replica_id: l.replica_id.clone(),
            action: action.clone(),
            task_id: tid.map(str::to_owned),
            text: text.into(),
            draft: draft.into(),
            snooze_until_ms: if action == Action::Snooze {
                Some(90_000)
            } else {
                None
            },
        },
        10_000,
    )
    .unwrap();
}
fn pair(a: &Ledger, b: &Ledger) -> (State, State) {
    let mut sa = State::fresh(a).unwrap();
    let mut sb = State::fresh(b).unwrap();
    let selected = Selection {
        persona: true,
        task_ids: vec!["00000000-0000-0000-0000-000000000000".into()],
    };
    sa.approve(
        sb.local.clone(),
        selected.clone(),
        IdentityChoice::UnifyArchive,
        true,
        a,
    )
    .unwrap();
    sb.approve(
        sa.local.clone(),
        selected,
        IdentityChoice::UnifyArchive,
        true,
        b,
    )
    .unwrap();
    (sa, sb)
}
fn projection(l: &Ledger) -> serde_json::Value {
    let v = l.view().unwrap();
    serde_json::json!({"agent":v.agent,"persona":v.persona,"tasks":v.tasks,"conflicts":v.conflicts})
}
fn exchange(
    a: &mut Vault,
    b: &mut Vault,
    fail_between_stores: bool,
    partial_ack: bool,
) -> Result<()> {
    let la = runtime::load(a)?;
    let sa = load(a, &la)?;
    let lb = runtime::load(b)?;
    let sb = load(b, &lb)?;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::scope(|scope| {
        let server = scope.spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            tls::serve_socket(socket, &sb, |request| {
                let mut ledger = runtime::load(b)?;
                let current = load(b, &ledger)?;
                let mut next = current.receive(&request.page)?;
                next.merge_into(&mut ledger)?;
                runtime::save(b, &ledger)?;
                if fail_between_stores {
                    return Err(
                        "injected interruption after runtime commit before cursor ACK".into(),
                    );
                }
                let page = next.page(&ledger, request.want_after)?;
                next.sent_ack = next.sent_ack.max(request.want_after);
                save(b, &next)?;
                Ok(Reply {
                    page,
                    acknowledged: if partial_ack {
                        request.page.after
                    } else {
                        next.received.len() as u64
                    },
                })
            })
        });
        let page = sa.page(&la, sa.sent_ack)?;
        let last = page.changes.last().map_or(page.after, |c| c.sequence);
        let reply = tls::connect_guarded(
            &addr,
            &sa,
            &Exchange {
                page,
                want_after: sa.received.len() as u64,
            },
            Arc::new(|| true),
        );
        let server_result = server.join().unwrap();
        if fail_between_stores {
            assert!(reply.is_err() && server_result.is_err());
            return Ok(());
        }
        server_result?;
        let reply = reply?;
        require(
            reply.acknowledged >= sa.sent_ack && reply.acknowledged <= last,
            "ACK bound",
        )?;
        let mut ledger = runtime::load(a)?;
        let current = load(a, &ledger)?;
        let mut next = current.receive(&reply.page)?;
        next.sent_ack = reply.acknowledged;
        next.merge_into(&mut ledger)?;
        runtime::save(a, &ledger)?;
        save(a, &next)
    })
}
#[test]
fn real_two_tls_vaults_adoption_edit_both_reconnect_conflicts_tombstones_partial_ack_and_revocation(
) {
    let root = std::env::temp_dir().join(format!("unified-{}", id()));
    let mut vaults = vec![];
    for name in ["a", "b"] {
        let p = root.join(name);
        std::fs::create_dir_all(&p).unwrap();
        Vault::create(&p, b"fixture-password").unwrap();
        let mut v = Vault::open(&p).unwrap();
        v.unlock(b"fixture-password").unwrap();
        vaults.push(v);
    }
    let mut a = vaults.remove(0);
    let mut b = vaults.remove(0);
    let mut la = runtime::load(&mut a).unwrap();
    let mut lb = runtime::load(&mut b).unwrap();
    edit(
        &mut la,
        Action::Persona,
        None,
        "old A identity",
        "old local-only note",
    );
    let archived_task = id();
    edit(
        &mut lb,
        Action::Create,
        Some(&archived_task),
        "old B task",
        "never rebound",
    );
    let archived_a = serde_json::to_value(&la.mutations).unwrap();
    let archived_b = serde_json::to_value(&lb.mutations).unwrap();
    runtime::save(&mut a, &la).unwrap();
    runtime::save(&mut b, &lb).unwrap();
    let (sa, sb) = pair(&la, &lb);
    save(&mut a, &sa).unwrap();
    save(&mut b, &sb).unwrap();
    assert!(la.shared.is_none() && lb.shared.is_none()); // approval alone does NOT adopt
    assert!(sa.shared_identity().unwrap().replicas.len() == 2);
    exchange(&mut a, &mut b, false, false).unwrap();
    la = runtime::load(&mut a).unwrap();
    lb = runtime::load(&mut b).unwrap();
    assert_eq!(la.version, 2);
    assert_ne!(la.replica_id, lb.replica_id);
    assert_ne!(la.local_vault_id, lb.local_vault_id);
    assert_eq!(serde_json::to_value(&la.mutations).unwrap(), archived_a);
    assert_eq!(serde_json::to_value(&lb.mutations).unwrap(), archived_b);
    assert_eq!(projection(&la), projection(&lb));
    assert!(la.view().unwrap().tasks.is_empty());
    let ta = id();
    let tb = id();
    edit(
        &mut la,
        Action::Create,
        Some(&ta),
        "A shared task",
        "private unified fixture α",
    );
    edit(
        &mut lb,
        Action::Create,
        Some(&tb),
        "B shared task",
        "draft β",
    );
    runtime::save(&mut a, &la).unwrap();
    runtime::save(&mut b, &lb).unwrap();
    exchange(&mut a, &mut b, true, false).unwrap(); // runtime import committed, peer cursor not ACKed
    assert_eq!(load(&mut b, &lb).unwrap().received.len(), 0);
    exchange(&mut a, &mut b, false, true).unwrap(); // valid partial ACK causes safe retry
    exchange(&mut a, &mut b, false, false).unwrap();
    la = runtime::load(&mut a).unwrap();
    lb = runtime::load(&mut b).unwrap();
    assert_eq!(projection(&la), projection(&lb));
    assert_eq!(la.view().unwrap().tasks.len(), 2);
    let deadline = la
        .view()
        .unwrap()
        .tasks
        .iter()
        .find(|t| t.spec.task_id == ta)
        .unwrap()
        .spec
        .deadline_ms;
    // Both independently edit the SAME draft and persona while disconnected.
    edit(&mut la, Action::Edit, Some(&ta), "A edit", "A branch note");
    edit(&mut lb, Action::Edit, Some(&ta), "B edit", "B branch note");
    edit(&mut la, Action::Persona, None, "A name", "A preference");
    edit(&mut lb, Action::Persona, None, "B name", "B preference");
    runtime::save(&mut a, &la).unwrap();
    runtime::save(&mut b, &lb).unwrap();
    exchange(&mut a, &mut b, false, false).unwrap();
    la = runtime::load(&mut a).unwrap();
    lb = runtime::load(&mut b).unwrap();
    assert_eq!(projection(&la), projection(&lb));
    assert!(la
        .view()
        .unwrap()
        .conflicts
        .iter()
        .any(|s| s.contains("A branch note") && s.contains("B branch note")));
    assert!(la
        .view()
        .unwrap()
        .conflicts
        .iter()
        .any(|s| s.starts_with("PERSONA_CONFLICT")));
    // Explicit subsequent corrections observe both heads; no last-writer-wins happened.
    edit(
        &mut la,
        Action::Persona,
        None,
        "Reviewed name",
        "reviewed preference",
    );
    edit(
        &mut la,
        Action::Edit,
        Some(&ta),
        "Reviewed goal",
        "reviewed draft",
    );
    runtime::save(&mut a, &la).unwrap();
    exchange(&mut a, &mut b, false, false).unwrap();
    la = runtime::load(&mut a).unwrap();
    lb = runtime::load(&mut b).unwrap();
    assert_eq!(projection(&la), projection(&lb));
    assert!(la.view().unwrap().conflicts.is_empty());
    edit(&mut lb, Action::Snooze, Some(&tb), "", "");
    runtime::save(&mut b, &lb).unwrap();
    exchange(&mut a, &mut b, false, false).unwrap();
    la = runtime::load(&mut a).unwrap();
    lb = runtime::load(&mut b).unwrap();
    assert_eq!(projection(&la), projection(&lb));
    assert!(la
        .view()
        .unwrap()
        .tasks
        .iter()
        .any(|t| t.snooze_until_ms == Some(90_000)));
    assert_eq!(
        la.view()
            .unwrap()
            .tasks
            .iter()
            .find(|t| t.spec.task_id == ta)
            .unwrap()
            .spec
            .deadline_ms,
        deadline
    );
    edit(&mut la, Action::Delete, Some(&tb), "", "");
    edit(
        &mut lb,
        Action::Edit,
        Some(&tb),
        "stale edit",
        "must not resurrect",
    );
    runtime::save(&mut a, &la).unwrap();
    runtime::save(&mut b, &lb).unwrap();
    exchange(&mut a, &mut b, false, false).unwrap();
    drop(a);
    drop(b);
    let mut a = Vault::open(&root.join("a")).unwrap();
    let mut b = Vault::open(&root.join("b")).unwrap();
    a.unlock(b"fixture-password").unwrap();
    b.unlock(b"fixture-password").unwrap();
    la = runtime::load(&mut a).unwrap();
    lb = runtime::load(&mut b).unwrap();
    assert_eq!(projection(&la), projection(&lb));
    assert_eq!(la.view().unwrap().tasks.len(), 1);
    assert!(la
        .view()
        .unwrap()
        .tasks
        .iter()
        .all(|t| !t.execute_on_hydration && t.owner_epoch == 1));
    let mut state = load(&mut a, &la).unwrap();
    state.peer.as_mut().unwrap().revoked = true;
    save(&mut a, &state).unwrap();
    assert!(state.page(&la, 0).is_err());
    assert!(tls::client_config(&state).is_err());
    let encrypted =
        std::fs::read(root.join(format!("b/VAULT/records/{}.enc.json", runtime::RECORD_ID)))
            .unwrap();
    assert!(!String::from_utf8_lossy(&encrypted).contains("private unified fixture"));
    drop(a);
    drop(b);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn adoption_requires_both_matching_choices_collision_and_foreign_grants_hold() {
    let mut a = Ledger::fresh(&id()).unwrap();
    let mut b = Ledger::fresh(&id()).unwrap();
    let (sa, sb) = pair(&a, &b);
    assert!(a.adopt(sa.shared_identity().unwrap(), false).is_err());
    let mut page = sa.page(&a, 0).unwrap();
    page.choice = IdentityChoice::KeepSeparateReview;
    assert!(sb.receive(&page).is_err());
    let mut bs = sb.receive(&sa.page(&a, 0).unwrap()).unwrap();
    bs.merge_into(&mut b).unwrap();
    sa.receive(&sb.page(&b, 0).unwrap())
        .unwrap()
        .merge_into(&mut a)
        .unwrap();
    let tid = id();
    edit(&mut a, Action::Create, Some(&tid), "goal", "draft");
    let page = sa.page(&a, 0).unwrap();
    bs = bs.receive(&page).unwrap();
    bs.merge_into(&mut b).unwrap();
    let before = b.bytes().unwrap();
    let mut bad = page.clone();
    let mut op: runtime::shared::SharedOperation =
        serde_json::from_str(bad.changes[0].payload.as_ref().unwrap()).unwrap();
    op.body.as_mut().unwrap().draft = "collision".into();
    let raw = serde_json::to_string(&op).unwrap();
    bad.changes[0].content_hash = hash(raw.as_bytes());
    bad.changes[0].payload = Some(raw);
    assert!(bs.receive(&bad).is_err());
    assert_eq!(b.bytes().unwrap(), before);
    // Unknown local scope, even from a pinned sender, is not imported.
    let mut op = a.shared.as_ref().unwrap().operations[&a.replica_id][0].clone();
    op.body.as_mut().unwrap().records[0]["record"]["scopes"]["hosts"] = serde_json::json!([id()]);
    assert!(b.import_shared(&a.replica_id, &[op]).is_err());
}

#[test]
fn receipt_and_handoff_wire_claims_stay_observed_not_native_verified_and_causal_cycles_hold() {
    let mut a = Ledger::fresh(&id()).unwrap();
    let mut b = Ledger::fresh(&id()).unwrap();
    let (sa, sb) = pair(&a, &b);
    sb.receive(&sa.page(&a, 0).unwrap())
        .unwrap()
        .merge_into(&mut b)
        .unwrap();
    sa.receive(&sb.page(&b, 0).unwrap())
        .unwrap()
        .merge_into(&mut a)
        .unwrap();
    let tid = id();
    edit(&mut a, Action::Create, Some(&tid), "receipt task", "notes");
    let mut records = vec![];
    for fixture in [
        include_str!("../../personal-agent-contracts/fixtures/task_receipt.json"),
        include_str!("../../personal-agent-contracts/fixtures/handoff.json"),
    ] {
        let mut v: serde_json::Value = serde_json::from_str(fixture).unwrap();
        v["payload"]["task_id"] = serde_json::json!(tid);
        if v["kind"] == "TASK_RECEIPT" {
            v["payload"]["replica_id"] = serde_json::json!(a.replica_id);
        } else {
            v["payload"]["from_replica_id"] = serde_json::json!(a.replica_id);
            v["payload"]["target_replica_id"] = serde_json::json!(b.replica_id);
        }
        records.push(v);
    }
    let request = Request {
        operation_id: id(),
        expected_revision: a.view().unwrap().revision,
        expected_replica_id: a.replica_id.clone(),
        action: Action::Accept,
        task_id: Some(tid.clone()),
        text: "".into(),
        draft: "".into(),
        snooze_until_ms: None,
    };
    a.shared
        .as_mut()
        .unwrap()
        .append(&a.replica_id, request, records)
        .unwrap();
    let received = sb.receive(&sa.page(&a, 0).unwrap()).unwrap();
    received.merge_into(&mut b).unwrap();
    let v = b.view().unwrap();
    let t = &v.tasks[0];
    assert_eq!(t.remote_claims.len(), 2);
    assert_eq!(t.status, "PLANNED");
    assert!(!t.execute_on_hydration);
    assert_eq!(t.owner_replica_id.as_ref(), Some(&a.replica_id));
    assert_eq!(t.owner_epoch, 1);
    assert_eq!(t.remote_claims[0]["payload"]["source"], "NATIVE");
    assert_eq!(t.remote_claims[0]["payload"]["outcome"], "ACTION_VERIFIED"); // preserve wire claim, no VerifiedReceipt fabricated
    let mut corrupt = b.clone();
    let source = corrupt
        .shared
        .as_mut()
        .unwrap()
        .operations
        .get_mut(&a.replica_id)
        .unwrap();
    source[0].context.insert(a.replica_id.clone(), 1);
    assert!(corrupt.bytes().is_err());
    let mut payload = a.shared.as_ref().unwrap().operations[&a.replica_id][0].clone();
    payload.body.as_mut().unwrap().records[0]["payload"]["scopes"]["hosts"] =
        serde_json::json!(["local-native-authority"]);
    let mut wire = sa.page(&a, 0).unwrap();
    let raw = serde_json::to_string(&payload).unwrap();
    wire.changes[0].content_hash = hash(raw.as_bytes());
    wire.changes[0].payload = Some(raw);
    assert!(sb.receive(&wire).is_err());
}
#[test]
fn shared_selection_markers_paged_reconnect_and_symmetric_causal_gap_hold() {
    let mut a = Ledger::fresh(&id()).unwrap();
    let mut b = Ledger::fresh(&id()).unwrap();
    let (mut sa, mut sb) = pair(&a, &b);
    sb.receive(&sa.page(&a, 0).unwrap())
        .unwrap()
        .merge_into(&mut b)
        .unwrap();
    sa.receive(&sb.page(&b, 0).unwrap())
        .unwrap()
        .merge_into(&mut a)
        .unwrap();
    let t = id();
    edit(&mut a, Action::Create, Some(&t), "first", "draft");
    for n in 0..18 {
        edit(
            &mut a,
            Action::Edit,
            Some(&t),
            &format!("edit {n}"),
            "draft",
        );
    }
    assert_eq!(sa.page(&a, 0).unwrap().changes.len(), 8);
    while sb.received.len() < a.outbound_len() {
        let page = sa.page(&a, sb.received.len() as u64).unwrap();
        sb = sb.receive(&page).unwrap();
        sb.merge_into(&mut b).unwrap();
    }
    assert_eq!(projection(&a), projection(&b));
    let original = b.bytes().unwrap();
    let mut gap = sa.page(&a, 0).unwrap();
    gap.after = 3;
    assert!(sb.receive(&gap).is_err());
    assert_eq!(b.bytes().unwrap(), original);
    // Fixed selection transmits causal slots but no unapproved prose/identity archive.
    sa.peer.as_mut().unwrap().selection = Selection {
        persona: false,
        task_ids: vec![],
    };
    let page = sa.page(&a, 0).unwrap();
    for ch in page.changes {
        let op: runtime::shared::SharedOperation =
            serde_json::from_str(ch.payload.as_ref().unwrap()).unwrap();
        assert!(op.body.is_none());
        assert!(!ch.payload.unwrap().contains("draft"));
    }
}
