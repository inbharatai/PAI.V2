use std::{
    io::Write,
    net::TcpListener,
    sync::{Arc, Mutex},
};
use unoone_local_peer_sync::*;
use unoone_personal_agent_runtime::{self as runtime, Action, Ledger, Request};
use unoone_vault_core::Vault;
use uuid::Uuid;
fn ledger() -> Ledger {
    Ledger::fresh(&Uuid::new_v4().to_string()).unwrap()
}
fn pair(a: &Ledger, b: &Ledger) -> (State, State) {
    let mut sa = State::fresh(a).unwrap();
    let mut sb = State::fresh(b).unwrap();
    let sel = |l: &Ledger| Selection {
        persona: true,
        task_ids: l
            .mutations
            .iter()
            .filter_map(|m| m.request.as_ref().and_then(|r| r.task_id.clone()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
    };
    sa.approve(
        sb.local.clone(),
        sel(a),
        IdentityChoice::KeepSeparateReview,
        true,
        a,
    )
    .unwrap();
    sb.approve(
        sa.local.clone(),
        sel(b),
        IdentityChoice::KeepSeparateReview,
        true,
        b,
    )
    .unwrap();
    (sa, sb)
}
fn mutation(l: &mut Ledger, action: Action, task_id: &str) {
    let deleting = matches!(action, Action::Delete);
    l.apply(
        Request {
            operation_id: Uuid::new_v4().to_string(),
            expected_revision: l.mutations.len() as u64,
            expected_replica_id: l.replica_id.clone(),
            action,
            task_id: Some(task_id.into()),
            text: if deleting {
                String::new()
            } else {
                "private fixture goal".into()
            },
            draft: if deleting {
                String::new()
            } else {
                "private fixture draft".into()
            },
            snooze_until_ms: None,
        },
        10_000,
    )
    .unwrap();
}
#[test]
fn approval_identity_selection_and_revocation_are_explicit() {
    let a = ledger();
    let b = ledger();
    let mut sa = State::fresh(&a).unwrap();
    let sb = State::fresh(&b).unwrap();
    let selection = Selection {
        persona: false,
        task_ids: vec![],
    };
    assert!(sa
        .approve(
            sb.local.clone(),
            selection.clone(),
            IdentityChoice::KeepSeparateReview,
            false,
            &a
        )
        .is_err());
    assert!(sa
        .approve(
            sb.local.clone(),
            selection.clone(),
            IdentityChoice::SamePerson,
            true,
            &a
        )
        .unwrap_err()
        .contains("IDENTITY_CONFLICT"));
    sa.approve(
        sb.local.clone(),
        selection,
        IdentityChoice::KeepSeparateReview,
        true,
        &a,
    )
    .unwrap();
    assert!(sa
        .page(&a, 0)
        .unwrap()
        .changes
        .iter()
        .all(|c| c.payload.is_none()));
    sa.peer.as_mut().unwrap().revoked = true;
    assert!(sa.page(&a, 0).is_err());
    assert!(tls::server_config(&sa).is_err());
}
#[test]
fn replay_gap_collision_tamper_version_choice_and_delete_convergence() {
    let mut a = ledger();
    let b = ledger();
    let task = Uuid::new_v4().to_string();
    mutation(&mut a, Action::Create, &task);
    let (sa, sb) = pair(&a, &b);
    let page = sa.page(&a, 0).unwrap();
    let received = sb.receive(&page).unwrap();
    assert_eq!(received.received.len(), 2);
    assert_eq!(received.receive(&page).unwrap().received, received.received);
    let mut bad = page.clone();
    bad.version = 2;
    assert!(sb.receive(&bad).is_err());
    bad = page.clone();
    bad.choice = IdentityChoice::SamePerson;
    assert!(sb.receive(&bad).is_err());
    bad = page.clone();
    bad.after = 1;
    bad.changes.remove(0);
    assert!(sb.receive(&bad).is_err());
    bad = page.clone();
    bad.changes[1].content_hash = "0".repeat(64);
    assert!(sb.receive(&bad).is_err());
    bad = page.clone();
    bad.changes[0].operation_id = Uuid::new_v4().to_string();
    assert!(received.receive(&bad).is_err());
    mutation(&mut a, Action::Delete, &task);
    let deleted = received.receive(&sa.page(&a, 2).unwrap()).unwrap();
    let tombstone = &deleted.remote_tasks().unwrap()[0];
    assert!(tombstone.deleted);
    assert!(tombstone.draft.is_empty());
    assert!(!tombstone.execute_on_hydration);
    assert!(deleted.receive(&page).unwrap().remote_tasks().unwrap()[0].deleted);
    assert_eq!(b.mutations.len(), 1); // no foreign request replay / identity replacement
}
#[test]
fn bounded_http_rejects_truncation_size_chunking_public_and_dns() {
    for input in [
        b"POST /unoone-peer-v1 HTTP/1.1\r\nContent-Length: 3\r\n\r\nx".as_slice(),
        b"POST /unoone-peer-v1 HTTP/1.1\r\nContent-Length: 999999999\r\n\r\n",
        b"POST /unoone-peer-v1 HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
        b"POST /unoone-peer-v1 HTTP/1.1\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
    ] {
        assert!(tls::read_http(&mut &input[..], false).is_err());
    }
    for a in [
        "example.com:443",
        "8.8.8.8:443",
        "0.0.0.0:1",
        "[::]:1",
        "[::ffff:8.8.8.8]:443",
    ] {
        assert!(tls::address(a).is_err());
    }
    assert!(tls::address("192.168.1.12:43123").is_ok());
}
#[test]
fn real_tls_mutual_pins_spoofed_server_and_client_rejected() {
    let a = ledger();
    let b = ledger();
    let (sa, sb) = pair(&a, &b);
    for spoof_server in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let mut server = sb.clone();
        let mut client = sa.clone();
        if spoof_server {
            let (c, k) = tls::identity().unwrap();
            server.certificate = c;
            server.private_key = k;
        } else {
            let (c, k) = tls::identity().unwrap();
            client.certificate = c;
            client.private_key = k;
        }
        let t = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            tls::serve_socket(socket, &server, |_| panic!("spoof must not reach handler"))
        });
        assert!(tls::connect(
            &address,
            &client,
            &Exchange {
                page: sa.page(&a, 0).unwrap(),
                want_after: 0
            }
        )
        .is_err());
        assert!(t.join().unwrap().is_err());
    }
}
#[test]
fn two_real_encrypted_vaults_loopback_restart_persist_before_ack_and_failure() {
    let root = std::env::temp_dir().join(format!("peer-sync-{}", Uuid::new_v4()));
    let mut vaults = Vec::new();
    for name in ["a", "b"] {
        let p = root.join(name);
        std::fs::create_dir_all(&p).unwrap();
        Vault::create(&p, b"synthetic-test-password-only").unwrap();
        let mut v = Vault::open(&p).unwrap();
        v.unlock(b"synthetic-test-password-only").unwrap();
        vaults.push(v);
    }
    let mut va = vaults.remove(0);
    let mut vb = vaults.remove(0);
    let mut la = runtime::load(&mut va).unwrap();
    let lb = runtime::load(&mut vb).unwrap();
    let tid = Uuid::new_v4().to_string();
    mutation(&mut la, Action::Create, &tid);
    runtime::save(&mut va, &la).unwrap();
    let (sa, sb) = pair(&la, &lb);
    save(&mut va, &sa).unwrap();
    save(&mut vb, &sb).unwrap();
    let vb = Arc::new(Mutex::new(vb));
    for fail in [true, false, false] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let vault = vb.clone();
        let server = sb.clone();
        let source = lb.clone();
        let t = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            tls::serve_socket(socket, &server, |request| {
                let mut v = vault.lock().unwrap();
                let current = load(&mut v, &source)?;
                let next = current.receive(&request.page)?;
                if fail {
                    // Actual filesystem fault: record-directory path becomes a regular file.
                    let records = v.vault_root().join("VAULT/records");
                    let held = v.vault_root().join("VAULT/records-held");
                    std::fs::rename(&records, &held).unwrap();
                    std::fs::write(&records, b"fault-injected-not-a-directory").unwrap();
                    let result = save(&mut v, &next);
                    std::fs::remove_file(&records).unwrap();
                    std::fs::rename(&held, &records).unwrap();
                    return result.map(|_| unreachable!());
                }
                save(&mut v, &next)?;
                let persisted = load(&mut v, &source)?;
                assert_eq!(persisted.received.len(), 2);
                Ok(Reply {
                    page: persisted.page(&source, request.want_after)?,
                    acknowledged: persisted.received.len() as u64,
                })
            })
        });
        let response = tls::connect(
            &address,
            &sa,
            &Exchange {
                page: sa.page(&la, 0).unwrap(),
                want_after: 0,
            },
        );
        if fail {
            assert!(response.is_err());
            assert!(t.join().unwrap().is_err());
            assert!(load(&mut vb.lock().unwrap(), &lb)
                .unwrap()
                .received
                .is_empty());
        } else {
            let reply = response.unwrap();
            assert_eq!(reply.acknowledged, 2);
            let next = sa.receive(&reply.page).unwrap();
            save(&mut va, &next).unwrap();
            t.join().unwrap().unwrap();
        }
    }
    drop(vb);
    let mut reopened = Vault::open(&root.join("b")).unwrap();
    reopened.unlock(b"synthetic-test-password-only").unwrap();
    let incoming = load(&mut reopened, &lb).unwrap();
    assert_eq!(incoming.received.len(), 2);
    assert_eq!(
        incoming.remote_tasks().unwrap()[0].goal,
        "private fixture goal"
    );
    assert_eq!(runtime::load(&mut reopened).unwrap().mutations.len(), 1);
    let encrypted =
        std::fs::read(root.join(format!("b/VAULT/records/{STORE_ID}.enc.json"))).unwrap();
    assert!(!String::from_utf8_lossy(&encrypted).contains("private fixture"));
    std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(root.join(format!("b/VAULT/records/{STORE_ID}.enc.json")))
        .unwrap()
        .write_all(b"corrupt-retain")
        .unwrap();
    assert!(load(&mut reopened, &lb).is_err());
    drop(va);
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn authenticated_truncated_stream_never_reaches_persistence() {
    use rustls::{pki_types::ServerName, ClientConnection, StreamOwned};
    use std::net::{Shutdown, TcpStream};
    let a = ledger();
    let b = ledger();
    let (sa, sb) = pair(&a, &b);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let t = std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        tls::serve_socket(socket, &sb, |_| {
            panic!("Truncated request must not persist/ACK")
        })
    });
    let socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let mut stream = StreamOwned::new(
        ClientConnection::new(
            tls::client_config(&sa).unwrap(),
            ServerName::try_from("unoone.local").unwrap(),
        )
        .unwrap(),
        socket,
    );
    stream
        .write_all(b"POST /unoone-peer-v1 HTTP/1.1\r\nContent-Length: 100\r\n\r\n{")
        .unwrap();
    stream.flush().unwrap();
    stream.sock.shutdown(Shutdown::Both).unwrap();
    assert!(t.join().unwrap().is_err());
}
#[test]
fn paged_resume_selection_and_json_guards() {
    let tid = Uuid::new_v4().to_string();
    let mut a = ledger();
    mutation(&mut a, Action::Create, &tid);
    for _ in 0..17 {
        mutation(&mut a, Action::Edit, &tid);
    }
    let b = ledger();
    let (sa, mut received) = pair(&a, &b);
    while received.received.len() < a.mutations.len() {
        let page = sa.page(&a, received.received.len() as u64).unwrap();
        assert!(page.changes.len() <= PAGE);
        received = received.receive(&page).unwrap();
    }
    assert_eq!(received.received.len(), 19);
    let body = serde_json::to_string(&sa.page(&a, 0).unwrap()).unwrap();
    assert!(!body.contains(&a.local_vault_id));
    assert!(!body.contains("private_key"));
    assert!(json_guard::preflight(br#"{"a":1,"\u0061":2}"#, 100).is_err());
    assert!(json_guard::preflight(
        format!("{}{}", "[".repeat(17), "]".repeat(17)).as_bytes(),
        100
    )
    .is_err());
    assert!(tls::connect_guarded(
        "127.0.0.1:1",
        &sa,
        &Exchange {
            page: sa.page(&a, 0).unwrap(),
            want_after: 0
        },
        Arc::new(|| false)
    )
    .unwrap_err()
    .contains("Session closed"));
}

#[test]
fn tls_capture_contains_no_selected_plaintext_or_master_material() {
    use std::io::Read;
    use std::net::{Shutdown, TcpStream};
    let mut a = ledger();
    let tid = Uuid::new_v4().to_string();
    mutation(&mut a, Action::Create, &tid);
    let b = ledger();
    let (sa, sb) = pair(&a, &b);
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let server_addr = server.local_addr().unwrap();
    let responder = std::thread::spawn(move || {
        let (socket, _) = server.accept().unwrap();
        tls::serve_socket(socket, &sb, |request| {
            let next = sb.receive(&request.page)?;
            Ok(Reply {
                page: next.page(&b, request.want_after)?,
                acknowledged: next.received.len() as u64,
            })
        })
        .unwrap();
    });
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_addr = proxy.local_addr().unwrap();
    let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
    let cap = captured.clone();
    let relay = std::thread::spawn(move || {
        let (left, _) = proxy.accept().unwrap();
        let right = TcpStream::connect(server_addr).unwrap();
        let copy = |mut from: TcpStream, mut to: TcpStream, capture: Arc<Mutex<Vec<u8>>>| {
            from.set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut buf = [0u8; 8192];
            while let Ok(n) = from.read(&mut buf) {
                if n == 0 {
                    break;
                }
                {
                    let mut c = capture.lock().unwrap();
                    assert!(c.len() + n < 2 * 1024 * 1024);
                    c.extend_from_slice(&buf[..n]);
                }
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
            let _ = to.shutdown(Shutdown::Write);
        };
        let l = left.try_clone().unwrap();
        let r = right.try_clone().unwrap();
        let cap2 = cap.clone();
        let t = std::thread::spawn(move || copy(l, r, cap2));
        copy(right, left, cap);
        t.join().unwrap();
    });
    let reply = tls::connect(
        &proxy_addr.to_string(),
        &sa,
        &Exchange {
            page: sa.page(&a, 0).unwrap(),
            want_after: 0,
        },
    )
    .unwrap();
    assert_eq!(reply.acknowledged, 2);
    responder.join().unwrap();
    relay.join().unwrap();
    let bytes = captured.lock().unwrap();
    assert!(!bytes.is_empty());
    let text = String::from_utf8_lossy(&bytes);
    for secret in [
        "private fixture goal",
        "private fixture draft",
        &a.local_vault_id,
        "inbharat.pai.personal-ledger",
    ] {
        assert!(!text.contains(secret));
    }
}

#[test]
fn even_authenticated_peer_cannot_import_grants_or_provider_records() {
    let a = ledger();
    let b = ledger();
    let (sa, sb) = pair(&a, &b);
    for fixture in [
        include_str!("../../personal-agent-contracts/fixtures/capability_grant.json"),
        include_str!("../../personal-agent-contracts/fixtures/mail_account.json"),
        include_str!("../../personal-agent-contracts/fixtures/draft.json"),
    ] {
        let mut page = sa.page(&a, 0).unwrap();
        let payload = serde_json::to_string(&Payload {
            records: vec![serde_json::from_str(fixture).unwrap()],
            task_state: None,
        })
        .unwrap();
        page.changes[0].content_hash = hash(payload.as_bytes());
        page.changes[0].payload = Some(payload);
        assert!(sb.receive(&page).is_err());
    }
    assert!(sb.received.is_empty());
}
