//! TRANSPORT FIXTURES ONLY: loopback HTTP is compile-time test-only. These are
//! real filesystem/socket tests, not model qualification or live asset downloads.
use super::*;
use base64::Engine;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::sync::atomic::AtomicUsize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct Allow;
impl TransferGuard for Allow {
    async fn checkpoint(&self) -> Result<()> {
        Ok(())
    }
}
struct PauseAfter {
    calls: AtomicUsize,
    after: usize,
    reason: &'static str,
}
impl TransferGuard for PauseAfter {
    async fn checkpoint(&self) -> Result<()> {
        if self.calls.fetch_add(1, Ordering::SeqCst) >= self.after {
            Err(DownloadError::Policy(self.reason.into()))
        } else {
            Ok(())
        }
    }
}
fn artifact(bytes: &[u8]) -> Artifact {
    Artifact {
        id: "fixture".into(),
        kind: ArtifactKind::Weights,
        format: "test-bytes".into(),
        sha256: hex::encode(Sha256::digest(bytes)),
        download_bytes: bytes.len() as u64,
        installed_bytes: bytes.len() as u64,
        source_revision: "TRANSPORT-FIXTURE-NOT-MODEL".into(),
        catalog_path: "fixture".into(),
    }
}
async fn server(responses: Vec<Vec<u8>>) -> (DownloadOrigin, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let mut requests = vec![];
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![];
            while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 8192 {
                let mut b = [0];
                if stream.read(&mut b).await.unwrap_or(0) == 0 {
                    break;
                }
                bytes.push(b[0]);
            }
            requests.push(String::from_utf8_lossy(&bytes).to_string());
            let _ = stream.write_all(&response).await;
        }
        requests
    });
    (
        DownloadOrigin {
            base: Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap(),
            hosts: BTreeSet::new(),
            transport_fixture: true,
        },
        task,
    )
}
fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut r = format!("HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n").into_bytes();
    r.extend(body);
    r
}
fn private() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    d
}
#[test]
fn public_address_and_origin_boundaries() {
    for ip in [
        "127.0.0.1",
        "10.1.2.3",
        "169.254.169.254",
        "100.64.0.1",
        "192.0.2.1",
        "198.18.0.1",
        "::1",
        "::ffff:8.8.8.8",
        "fc00::1",
        "2001:db8::1",
        "2002:7f00:1::1",
    ] {
        assert!(!public_ip(ip.parse().unwrap()), "{ip}");
    }
    assert!(public_ip("8.8.8.8".parse().unwrap()));
    let hosts = BTreeSet::from(["models.example.invalid".into()]);
    assert!(DownloadOrigin::https("http://models.example.invalid/", hosts.clone()).is_err());
    assert!(DownloadOrigin::https("https://evil.invalid/", hosts.clone()).is_err());
    assert!(DownloadOrigin::https("https://user@models.example.invalid/", hosts.clone()).is_err());
    assert!(DownloadOrigin::https("https://models.example.invalid:444/", hosts.clone()).is_err());
    assert!(DownloadOrigin::https("https://models.example.invalid/", hosts).is_ok());
}
#[tokio::test]
async fn actual_transport_hash_size_and_no_activation() {
    let d = private();
    fs::write(d.path().join("old-active"), b"working").unwrap();
    let (origin, server) = server(vec![response(
        "200 OK",
        "Content-Length: 4\r\nETag: \"v1\"\r\n",
        b"test",
    )])
    .await;
    transfer(
        &origin,
        &artifact(b"test"),
        d.path(),
        &Allow,
        &AtomicBool::new(false),
        &|_, _| {},
    )
    .await
    .unwrap();
    assert_eq!(fs::read(d.path().join("fixture")).unwrap(), b"test");
    assert_eq!(fs::read(d.path().join("old-active")).unwrap(), b"working");
    assert_eq!(server.await.unwrap().len(), 1);
}
#[tokio::test]
async fn rejects_bad_hash_size_redirect_and_encoding() {
    for (headers, status, bytes, expected) in [
        (
            "Content-Length: 4\r\n",
            "200 OK",
            &b"evil"[..],
            DownloadError::Integrity,
        ),
        (
            "Content-Length: 5\r\n",
            "200 OK",
            &b"test!"[..],
            DownloadError::Size,
        ),
        (
            "Content-Length: 4\r\nContent-Encoding: gzip\r\n",
            "200 OK",
            &b"test"[..],
            DownloadError::Integrity,
        ),
    ] {
        let d = private();
        let (origin, s) = server(vec![response(status, headers, bytes)]).await;
        assert_eq!(
            transfer(
                &origin,
                &artifact(b"test"),
                d.path(),
                &Allow,
                &AtomicBool::new(false),
                &|_, _| {}
            )
            .await
            .unwrap_err(),
            expected
        );
        s.await.unwrap();
        assert!(!d.path().join("fixture").exists());
    }
    let d = private();
    let (origin, s) = server(vec![response(
        "302 Found",
        "Content-Length: 0\r\nLocation: http://127.0.0.1:9/private\r\n",
        b"",
    )])
    .await;
    assert!(matches!(
        transfer(
            &origin,
            &artifact(b"test"),
            d.path(),
            &Allow,
            &AtomicBool::new(false),
            &|_, _| {}
        )
        .await,
        Err(DownloadError::Transport(_))
    ));
    s.await.unwrap();
}
#[tokio::test]
async fn interrupted_transfer_resumes_with_exact_range_validator() {
    let d = private();
    let (origin, s) = server(vec![
        response("200 OK", "Content-Length: 8\r\nETag: \"v1\"\r\n", b"test"),
        response(
            "206 Partial Content",
            "Content-Length: 4\r\nETag: \"v1\"\r\nContent-Range: bytes 4-7/8\r\n",
            b"rest",
        ),
    ])
    .await;
    let a = artifact(b"testrest");
    let c = AtomicBool::new(false);
    assert!(transfer(&origin, &a, d.path(), &Allow, &c, &|_, _| {})
        .await
        .is_err());
    assert_eq!(
        fs::metadata(d.path().join("fixture.part")).unwrap().len(),
        4
    );
    transfer(&origin, &a, d.path(), &Allow, &c, &|_, _| {})
        .await
        .unwrap();
    let requests = s.await.unwrap();
    assert!(requests[1].to_lowercase().contains("range: bytes=4-"));
    assert!(requests[1].to_lowercase().contains("if-range: \"v1\""));
    assert_eq!(fs::read(d.path().join("fixture")).unwrap(), b"testrest");
}
#[tokio::test]
async fn changed_etag_or_ignored_range_never_appends() {
    for status in ["200 OK", "206 Partial Content"] {
        let d = private();
        let (origin, s) = server(vec![response(
            status,
            "Content-Length: 4\r\nETag: \"changed\"\r\nContent-Range: bytes 4-7/8\r\n",
            b"rest",
        )])
        .await;
        fs::write(d.path().join("fixture.part"), b"test").unwrap();
        let a = artifact(b"testrest");
        save_resume(
            &d.path().join("fixture.resume.json"),
            &Resume {
                url: origin.base.join("fixture").unwrap().to_string(),
                sha256: a.sha256.clone(),
                size: 8,
                etag: "\"v1\"".into(),
            },
        )
        .unwrap();
        assert_eq!(
            transfer(
                &origin,
                &a,
                d.path(),
                &Allow,
                &AtomicBool::new(false),
                &|_, _| {}
            )
            .await
            .unwrap_err(),
            DownloadError::InvalidRange
        );
        s.await.unwrap();
        assert_eq!(fs::read(d.path().join("fixture.part")).unwrap(), b"test");
    }
}
#[tokio::test]
async fn cancel_and_stale_policy_preserve_partial_and_working_model() {
    let d = private();
    fs::write(d.path().join("old-active"), b"old").unwrap();
    let (origin, s) = server(vec![response(
        "200 OK",
        "Content-Length: 4\r\nETag: \"v1\"\r\n",
        b"test",
    )])
    .await;
    let c = AtomicBool::new(false);
    let err = transfer(
        &origin,
        &artifact(b"test"),
        d.path(),
        &Allow,
        &c,
        &|_, _| {
            c.store(true, Ordering::SeqCst);
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err, DownloadError::Cancelled);
    s.await.unwrap();
    assert!(!d.path().join("fixture").exists());
    // Restart sees complete partial, but rechecks policy before publication.
    let guard = PauseAfter {
        calls: AtomicUsize::new(0),
        after: 0,
        reason: "revoked or expired",
    };
    assert!(matches!(
        transfer(
            &origin,
            &artifact(b"test"),
            d.path(),
            &guard,
            &AtomicBool::new(false),
            &|_, _| {}
        )
        .await,
        Err(DownloadError::Policy(_))
    ));
    assert_eq!(fs::read(d.path().join("old-active")).unwrap(), b"old");
}
#[tokio::test]
async fn disk_destination_failure_is_not_ready() {
    let d = private();
    fs::create_dir(d.path().join("fixture.part")).unwrap();
    let origin = DownloadOrigin::https(
        "https://models.example.invalid/",
        BTreeSet::from(["models.example.invalid".into()]),
    )
    .unwrap();
    assert_eq!(
        transfer(
            &origin,
            &artifact(b"test"),
            d.path(),
            &Allow,
            &AtomicBool::new(false),
            &|_, _| {}
        )
        .await
        .unwrap_err(),
        DownloadError::UnsafeDestination
    );
}
#[cfg(unix)]
#[tokio::test]
async fn rejects_symlink_and_hardlink_partial() {
    let d = private();
    let old = d.path().join("old");
    fs::write(&old, b"old").unwrap();
    let origin = DownloadOrigin::https(
        "https://models.example.invalid/",
        BTreeSet::from(["models.example.invalid".into()]),
    )
    .unwrap();
    std::os::unix::fs::symlink(&old, d.path().join("fixture.part")).unwrap();
    assert!(transfer(
        &origin,
        &artifact(b"test"),
        d.path(),
        &Allow,
        &AtomicBool::new(false),
        &|_, _| {}
    )
    .await
    .is_err());
    fs::remove_file(d.path().join("fixture.part")).unwrap();
    fs::hard_link(&old, d.path().join("fixture.part")).unwrap();
    assert!(transfer(
        &origin,
        &artifact(b"test"),
        d.path(),
        &Allow,
        &AtomicBool::new(false),
        &|_, _| {}
    )
    .await
    .is_err());
    assert_eq!(fs::read(old).unwrap(), b"old");
}
fn signed_bundle() -> VerifiedCandidate {
    signed_bundle_headroom(None)
}
fn signed_bundle_headroom(headroom: Option<u64>) -> VerifiedCandidate {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../packages/model-admission/tests/fixtures/admission-v1.json"
    ))
    .unwrap();
    let mut c: CatalogCandidate = serde_json::from_value(fixture["candidate"].clone()).unwrap();
    for a in &mut c.artifacts {
        a.download_bytes = 4;
        a.installed_bytes = 4;
        a.sha256 = hex::encode(Sha256::digest(b"test"));
    }
    let mut runtime = artifact(b"test");
    runtime.id = "runtime".into();
    runtime.kind = ArtifactKind::Other;
    runtime.format = "runtime-executable".into();
    c.artifacts.push(runtime);
    if let Some(bytes) = headroom {
        c.memory.disk_headroom_bytes = bytes;
    }
    let bytes = serde_json::to_vec(&c).unwrap();
    let key = Ed25519KeyPair::from_seed_unchecked(&[43; 32]).unwrap();
    let verifier =
        crate::provisioning::RingVerifier::configured(vec![crate::provisioning::TrustedPayload {
            domain: SignedDomain::CatalogCandidateV1,
            key_id: "fixture-only".into(),
            public_key: key.public_key().as_ref().try_into().unwrap(),
            payload_sha256: Sha256::digest(&bytes).into(),
        }]);
    verify_candidate(
        &bytes,
        &Attestation {
            key_id: "fixture-only".into(),
            algorithm: "Ed25519".into(),
            signature: base64::engine::general_purpose::STANDARD.encode(key.sign(&bytes).as_ref()),
        },
        &verifier,
    )
    .unwrap()
}
#[tokio::test]
async fn actual_disk_free_reservation_refuses_before_network() {
    let d = private();
    let c = signed_bundle_headroom(Some(u64::MAX / 2));
    let origin = DownloadOrigin::https(
        "https://models.example.invalid/",
        BTreeSet::from(["models.example.invalid".into()]),
    )
    .unwrap();
    assert!(matches!(
        stage_bundle(
            d.path(),
            &c,
            &origin,
            &Allow,
            &AtomicBool::new(false),
            |_, _| {}
        )
        .await,
        Err(DownloadError::Disk(_))
    ));
}
#[tokio::test]
async fn os_file_lease_refuses_other_owner_and_releases_on_drop() {
    let d = private();
    let store = d.path().join("model-provisioning-bundles");
    crate::local_install::private_dir(&store).unwrap();
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(store.join("transfer.lock"))
        .unwrap();
    lease.try_lock().unwrap();
    let c = signed_bundle_headroom(Some(u64::MAX / 2));
    let origin = DownloadOrigin::https(
        "https://models.example.invalid/",
        BTreeSet::from(["models.example.invalid".into()]),
    )
    .unwrap();
    assert!(matches!(
        stage_bundle(
            d.path(),
            &c,
            &origin,
            &Allow,
            &AtomicBool::new(false),
            |_, _| {}
        )
        .await,
        Err(DownloadError::Policy(_))
    ));
    drop(lease);
    assert!(matches!(
        stage_bundle(
            d.path(),
            &c,
            &origin,
            &Allow,
            &AtomicBool::new(false),
            |_, _| {}
        )
        .await,
        Err(DownloadError::Disk(_))
    ));
}
#[tokio::test]
async fn whole_bundle_atomic_publication_and_restart_integrity() {
    let d = private();
    fs::write(d.path().join("active-model"), b"previous").unwrap();
    let c = signed_bundle();
    let (origin, s) = server(
        (0..c.candidate().artifacts.len())
            .map(|_| response("200 OK", "Content-Length: 4\r\n", b"test"))
            .collect(),
    )
    .await;
    let staged = stage_bundle(
        d.path(),
        &c,
        &origin,
        &Allow,
        &AtomicBool::new(false),
        |_, _| {},
    )
    .await
    .unwrap();
    s.await.unwrap();
    assert!(staged
        .directory
        .ends_with(staged.directory.file_name().unwrap()));
    assert!(staged.directory.to_string_lossy().ends_with(".staged"));
    for a in &c.candidate().artifacts {
        assert_eq!(fs::read(staged.directory.join(&a.id)).unwrap(), b"test");
    }
    stage_bundle(
        d.path(),
        &c,
        &origin,
        &Allow,
        &AtomicBool::new(false),
        |_, _| {},
    )
    .await
    .unwrap(); // No network on verified restart.
    fs::write(staged.directory.join("weights"), b"evil").unwrap();
    assert!(matches!(
        stage_bundle(
            d.path(),
            &c,
            &origin,
            &Allow,
            &AtomicBool::new(false),
            |_, _| {}
        )
        .await,
        Err(DownloadError::Integrity)
    ));
    assert_eq!(
        fs::read(d.path().join("active-model")).unwrap(),
        b"previous"
    );
}
