//! Synthetic v2 interoperability fixtures. No keys/credentials are written.
use std::{fs, path::PathBuf};
use unoone_local_peer_sync::*;
use unoone_personal_agent_runtime::{Action, Ledger, Request, View};
use uuid::Uuid;
fn id() -> String {
    Uuid::new_v4().to_string()
}
fn edit(l: &mut Ledger, action: Action, tid: Option<&str>, text: &str) {
    l.apply(
        Request {
            operation_id: id(),
            expected_revision: l.view().unwrap().revision,
            expected_replica_id: l.replica_id.clone(),
            action: action.clone(),
            task_id: tid.map(str::to_owned),
            text: if action == Action::Delete {
                "".into()
            } else {
                text.into()
            },
            draft: if action == Action::Delete {
                "".into()
            } else {
                format!("draft {text}")
            },
            snooze_until_ms: if action == Action::Snooze {
                Some(90000)
            } else {
                None
            },
        },
        10000,
    )
    .unwrap();
}
fn projection(v: View) -> serde_json::Value {
    serde_json::json!({ "agent": v.agent, "persona": v.persona, "tasks": v.tasks.iter().map(|t| serde_json::json!({"task_id":t.spec.task_id,"goal":t.spec.goal,"draft":t.draft,"deadline":t.spec.deadline_ms,"snooze":t.snooze_until_ms,"status":t.status,"events":t.events,"owner":t.owner_replica_id,"epoch":t.owner_epoch,"claims":t.remote_claims})).collect::<Vec<_>>(), "conflicts": v.conflicts, "conflict_kinds": v.conflicts.iter().map(|s| s.split(':').next().unwrap()).collect::<Vec<_>>() })
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let dir = PathBuf::from(&args[2]);
    if args[1] == "emit" {
        let mut a = Ledger::fresh(&id()).unwrap();
        let mut b = Ledger::fresh(&id()).unwrap();
        let mut sa = State::fresh(&a).unwrap();
        let mut sb = State::fresh(&b).unwrap();
        let selection = Selection {
            persona: true,
            task_ids: vec!["00000000-0000-0000-0000-000000000000".into()],
        };
        sa.approve(
            sb.local.clone(),
            selection.clone(),
            IdentityChoice::UnifyArchive,
            true,
            &a,
        )
        .unwrap();
        sb.approve(
            sa.local.clone(),
            selection,
            IdentityChoice::UnifyArchive,
            true,
            &b,
        )
        .unwrap();
        sb = sb.receive(&sa.page(&a, 0).unwrap()).unwrap();
        sb.merge_into(&mut b).unwrap();
        sa = sa.receive(&sb.page(&b, 0).unwrap()).unwrap();
        sa.merge_into(&mut a).unwrap();
        let t = id();
        let deleted = id();
        edit(&mut a, Action::Create, Some(&t), "नमस्ते shared");
        edit(&mut a, Action::Create, Some(&deleted), "deleted");
        edit(&mut a, Action::Delete, Some(&deleted), "");
        sb = sb.receive(&sa.page(&a, 0).unwrap()).unwrap();
        sb.merge_into(&mut b).unwrap();
        edit(&mut a, Action::Edit, Some(&t), "Rust A branch");
        edit(&mut b, Action::Edit, Some(&t), "Rust B branch");
        edit(&mut a, Action::Persona, None, "A persona");
        edit(&mut b, Action::Persona, None, "B persona");
        sb = sb
            .receive(&sa.page(&a, sb.received.len() as u64).unwrap())
            .unwrap();
        sb.merge_into(&mut b).unwrap();
        sa = sa.receive(&sb.page(&b, 0).unwrap()).unwrap();
        sa.merge_into(&mut a).unwrap();
        assert_eq!(projection(a.view().unwrap()), projection(b.view().unwrap()));
        for (name, bytes) in [
            ("shared-a.json", a.bytes().unwrap()),
            ("shared-b.json", b.bytes().unwrap()),
            (
                "shared-projection.json",
                serde_json::to_vec(&projection(a.view().unwrap())).unwrap(),
            ),
            (
                "shared-offer-a.json",
                serde_json::to_vec(&sa.local).unwrap(),
            ),
            (
                "shared-offer-b.json",
                serde_json::to_vec(&sb.local).unwrap(),
            ),
        ] {
            fs::write(dir.join(name), bytes).unwrap();
        }
        println!("Shared fixtures: conflicts, Unicode drafts, deadline, deleted task, distinct vault/replica identities; no keys exported");
    } else {
        let read = |name: &str| {
            let bytes = fs::read(dir.join(name)).unwrap();
            let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            Ledger::decode(&bytes, v["local_vault_id"].as_str().unwrap()).unwrap()
        };
        let mut a = read("shared-a.json");
        let kotlin = read("shared-kotlin.json");
        let original = read("shared-b.json");
        assert_eq!(
            serde_json::to_value(&original.mutations).unwrap(),
            serde_json::to_value(&kotlin.mutations).unwrap()
        );
        let ops = &kotlin.shared.as_ref().unwrap().operations[&kotlin.replica_id];
        a.import_shared(&kotlin.replica_id, ops).unwrap();
        assert_eq!(
            projection(a.view().unwrap()),
            projection(kotlin.view().unwrap())
        );
        let expected: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("shared-kotlin-projection.json")).unwrap())
                .unwrap();
        assert_eq!(projection(a.view().unwrap()), expected);
        assert!(a.view().unwrap().conflicts.is_empty());
        println!("Rust -> Kotlin active projection -> Kotlin edits -> Rust import equal merged projection PASS");
    }
}
