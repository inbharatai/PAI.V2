//! Real bounded reqwest transport + restartable immutable bundle staging.
//! This module does NOT activate models, execute runtimes or change assets_ready.
//! The shipping IPC cannot call it until publisher trust/qualification is configured.
use crate::provisioning::{safe_regular_file, sync_directory};
use reqwest::{header, Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use unoone_model_admission::*;

static TRANSFER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const MAX_ARTIFACT: u64 = 32 * 1024 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum DownloadError {
    Cancelled,
    Policy(String),
    Transport(String),
    Integrity,
    Size,
    UnsafeDestination,
    Disk(String),
    InvalidRange,
    UnconfiguredBundle,
}
type Result<T> = std::result::Result<T, DownloadError>;
fn disk(e: std::io::Error) -> DownloadError {
    DownloadError::Disk(e.to_string())
}

/// Called before requests, each chunk and publication. Production implementation
/// refreshes native observations/current local consent, never a cached DTO grant.
pub trait TransferGuard {
    fn checkpoint(&self) -> impl std::future::Future<Output = Result<()>> + Send;
}
pub struct NativeTransferGate<'a> {
    pub candidate: &'a VerifiedCandidate,
    pub qualifications: &'a [ValidatedQualification],
    pub request: &'a AdmissionRequest,
    pub consent: &'a crate::provisioning::LocalConsentStore,
    pub root: &'a Path,
    pub purpose: DownloadPurpose,
    pub current_store_bytes: u64,
}
impl TransferGuard for NativeTransferGate<'_> {
    async fn checkpoint(&self) -> Result<()> {
        let probe = crate::provisioning_probe::probe(self.root).await;
        let decision = crate::provisioning::decision(
            self.candidate,
            self.qualifications,
            &probe,
            self.request,
        );
        if !decision.eligible_now {
            return Err(DownloadError::Policy(format!(
                "Admission: {:?}",
                decision.reasons
            )));
        }
        let grant = self
            .consent
            .grant()
            .map_err(|e| DownloadError::Policy(format!("Local consent: {e:?}")))?;
        // A reviewed native metered/network adapter is not yet available. NEVER
        // accept renderer Wi-Fi flags or infer unmetered from Internet reachability.
        let network = NetworkState {
            kind: NetworkKind::Unknown,
            metered: Observation::Unknown,
        };
        match check_download_policy(
            self.candidate,
            Some(&grant),
            &self.purpose,
            &network,
            self.current_store_bytes,
            crate::provisioning_probe::now_ms(),
        ) {
            PolicyDecision::AllowedWithinGrant => Ok(()),
            other => Err(DownloadError::Policy(format!("{other:?}"))),
        }
    }
}

pub struct DownloadOrigin {
    base: Url,
    hosts: BTreeSet<String>,
    #[cfg(test)]
    transport_fixture: bool,
}
impl DownloadOrigin {
    pub fn https(base: &str, hosts: BTreeSet<String>) -> Result<Self> {
        let this = Self {
            base: Url::parse(base).map_err(|_| DownloadError::UnsafeDestination)?,
            hosts,
            #[cfg(test)]
            transport_fixture: false,
        };
        this.validate(&this.base)?;
        Ok(this)
    }
    fn validate(&self, url: &Url) -> Result<()> {
        #[cfg(test)]
        if self.transport_fixture && url.scheme() == "http" && url.host_str() == Some("127.0.0.1") {
            return Ok(());
        }
        if url.scheme() != "https"
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !self.hosts.contains(url.host_str().unwrap_or(""))
            || self.hosts.len() > 16
            || self.hosts.is_empty()
        {
            return Err(DownloadError::UnsafeDestination);
        }
        Ok(())
    }
    async fn client(&self, url: &Url) -> Result<Client> {
        self.validate(url)?;
        let host = url.host_str().ok_or(DownloadError::UnsafeDestination)?;
        let addresses: Vec<SocketAddr> = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host((
                host,
                url.port_or_known_default()
                    .ok_or(DownloadError::UnsafeDestination)?,
            )),
        )
        .await
        .map_err(|_| DownloadError::UnsafeDestination)?
        .map_err(|_| DownloadError::UnsafeDestination)?
        .take(17)
        .collect();
        if addresses.is_empty() || addresses.len() > 16 {
            return Err(DownloadError::UnsafeDestination);
        }
        let allow_private = {
            #[cfg(test)]
            {
                self.transport_fixture
            }
            #[cfg(not(test))]
            {
                false
            }
        };
        if !allow_private && addresses.iter().any(|a| !public_ip(a.ip())) {
            return Err(DownloadError::UnsafeDestination);
        }
        // Pin the checked DNS answer in the client: avoid DNS rebinding. Disable
        // ambient proxies; redirect budget is ZERO (including allowlisted hosts).
        Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|e| DownloadError::Transport(e.to_string()))
    }
}
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments(); // Only currently assigned global unicast, excluding special/documentation ranges.
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Resume {
    url: String,
    sha256: String,
    size: u64,
    etag: String,
}
fn strong_etag(s: &str) -> bool {
    s.starts_with('"')
        && s.ends_with('"')
        && s.len() >= 2
        && s.len() <= 256
        && !s.chars().any(char::is_control)
}
fn hash_file(
    path: &Path,
    expected_size: u64,
    expected_hash: &str,
    cancel: &AtomicBool,
) -> Result<()> {
    safe_regular_file(path).map_err(|_| DownloadError::UnsafeDestination)?;
    let mut f = File::open(path).map_err(disk)?;
    if f.metadata().map_err(disk)?.len() != expected_size {
        return Err(DownloadError::Size);
    }
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadError::Cancelled);
        }
        let n = f.read(&mut buf).map_err(disk)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    if hex::encode(h.finalize()) != expected_hash {
        return Err(DownloadError::Integrity);
    }
    Ok(())
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        Err(DownloadError::Cancelled)
    } else {
        Ok(())
    }
}
fn save_resume(path: &Path, r: &Resume) -> Result<()> {
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut f = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(disk)?;
    f.write_all(&serde_json::to_vec(r).map_err(|_| DownloadError::Integrity)?)
        .and_then(|_| f.sync_all())
        .map_err(disk)?;
    // Only written on starting a fresh transfer, not every chunk. Durable bytes
    // in the partial file itself define the resume offset; final hash is mandatory.
    fs::rename(&temp, path).map_err(disk)?;
    sync_directory(path.parent().unwrap()).map_err(DownloadError::Disk)
}
async fn transfer<G: TransferGuard>(
    origin: &DownloadOrigin,
    artifact: &Artifact,
    staging: &Path,
    guard: &G,
    cancel: &AtomicBool,
    progress: &impl Fn(u64, u64),
) -> Result<()> {
    if artifact.download_bytes == 0
        || artifact.download_bytes > MAX_ARTIFACT
        || artifact.installed_bytes != artifact.download_bytes
    {
        return Err(DownloadError::Size);
    }
    let url = origin
        .base
        .join(&artifact.catalog_path)
        .map_err(|_| DownloadError::UnsafeDestination)?;
    origin.validate(&url)?;
    let part = staging.join(format!("{}.part", artifact.id));
    let journal = staging.join(format!("{}.resume.json", artifact.id));
    let complete = staging.join(&artifact.id);
    if complete.exists() {
        return hash_file(&complete, artifact.download_bytes, &artifact.sha256, cancel);
    }
    for p in [&part, &journal, &complete] {
        crate::local_install::checked_path(p).map_err(|_| DownloadError::UnsafeDestination)?;
    }
    let mut offset = 0;
    let mut previous = None;
    if part.exists() {
        safe_regular_file(&part).map_err(|_| DownloadError::UnsafeDestination)?;
        offset = fs::metadata(&part).map_err(disk)?.len();
        if offset > artifact.download_bytes {
            return Err(DownloadError::Size);
        }
        if offset == artifact.download_bytes {
            hash_file(&part, artifact.download_bytes, &artifact.sha256, cancel)?;
            guard.checkpoint().await?;
            check_cancel(cancel)?;
            fs::rename(&part, &complete).map_err(disk)?;
            return sync_directory(staging).map_err(DownloadError::Disk);
        }
        if offset > 0 {
            safe_regular_file(&journal).map_err(|_| DownloadError::InvalidRange)?;
            let mut bytes = Vec::new();
            File::open(&journal)
                .map_err(disk)?
                .take(8193)
                .read_to_end(&mut bytes)
                .map_err(disk)?;
            if bytes.len() > 8192 {
                return Err(DownloadError::InvalidRange);
            }
            let r: Resume =
                serde_json::from_slice(&bytes).map_err(|_| DownloadError::InvalidRange)?;
            if r.url != url.as_str()
                || r.sha256 != artifact.sha256
                || r.size != artifact.download_bytes
                || !strong_etag(&r.etag)
            {
                return Err(DownloadError::InvalidRange);
            }
            previous = Some(r);
        }
    }
    guard.checkpoint().await?;
    check_cancel(cancel)?;
    let client = origin.client(&url).await?;
    let mut request = client
        .get(url.clone())
        .header(header::ACCEPT_ENCODING, "identity");
    if let Some(r) = &previous {
        request = request
            .header(header::RANGE, format!("bytes={offset}-"))
            .header(header::IF_RANGE, &r.etag);
    }
    let mut response = tokio::select! { r = request.send() => r.map_err(|e| DownloadError::Transport(e.to_string()))?, _ = cancelled(cancel) => return Err(DownloadError::Cancelled) };
    let etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .filter(|s| strong_etag(s))
        .map(str::to_string);
    if response
        .headers()
        .get(header::CONTENT_ENCODING)
        .is_some_and(|v| v != "identity")
    {
        return Err(DownloadError::Integrity);
    }
    if offset > 0 {
        let expected = format!(
            "bytes {offset}-{}/{}",
            artifact.download_bytes - 1,
            artifact.download_bytes
        );
        if response.status() != StatusCode::PARTIAL_CONTENT
            || response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                != Some(expected.as_str())
            || etag.as_deref() != previous.as_ref().map(|r| r.etag.as_str())
        {
            return Err(DownloadError::InvalidRange);
        }
    } else if response.status() != StatusCode::OK {
        return Err(DownloadError::Transport(format!(
            "HTTP {} (redirects forbidden)",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|n| n != artifact.download_bytes - offset)
    {
        return Err(DownloadError::Size);
    }
    if previous.is_none() {
        if let Some(etag) = etag {
            // A zero-byte interrupted attempt may have a prior journal. It
            // grants no range authority and can be safely replaced before IO.
            if journal.exists() {
                safe_regular_file(&journal).map_err(|_| DownloadError::UnsafeDestination)?;
                fs::remove_file(&journal).map_err(disk)?;
            }
            save_resume(
                &journal,
                &Resume {
                    url: url.to_string(),
                    sha256: artifact.sha256.clone(),
                    size: artifact.download_bytes,
                    etag,
                },
            )?;
        }
    }
    let mut file = if part.exists() {
        OpenOptions::new().append(true).open(&part)
    } else {
        OpenOptions::new().write(true).create_new(true).open(&part)
    }
    .map_err(disk)?;
    loop {
        guard.checkpoint().await?;
        check_cancel(cancel)?;
        let chunk = tokio::select! { r = response.chunk() => r.map_err(|e| DownloadError::Transport(e.to_string()))?, _ = cancelled(cancel) => return Err(DownloadError::Cancelled) };
        let Some(chunk) = chunk else {
            break;
        };
        let next = offset
            .checked_add(chunk.len() as u64)
            .ok_or(DownloadError::Size)?;
        if next > artifact.download_bytes {
            return Err(DownloadError::Size);
        }
        // Sync before reporting progress. ENOSPC is a disk failure, never ready.
        file.write_all(&chunk)
            .and_then(|_| file.sync_all())
            .map_err(disk)?;
        offset = next;
        progress(offset, artifact.download_bytes);
    }
    drop(file);
    hash_file(&part, artifact.download_bytes, &artifact.sha256, cancel)?;
    guard.checkpoint().await?;
    check_cancel(cancel)?;
    fs::rename(&part, &complete).map_err(disk)?;
    sync_directory(staging).map_err(DownloadError::Disk)
}
async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Immutable staging receipt, NOT a load/smoke/activation receipt.
pub struct StagedBundle {
    pub directory: PathBuf,
}
pub async fn stage_bundle<G: TransferGuard>(
    root: &Path,
    candidate: &VerifiedCandidate,
    origin: &DownloadOrigin,
    guard: &G,
    cancel: &AtomicBool,
    progress: impl Fn(u64, u64),
) -> Result<StagedBundle> {
    let _lease = tokio::select! { lease = TRANSFER_LOCK.lock() => lease, _ = cancelled(cancel) => return Err(DownloadError::Cancelled) };
    check_cancel(cancel)?;
    guard.checkpoint().await?;
    let c = candidate.candidate();
    // All runtime dependencies must be explicit signed artifacts. Never execute
    // an arbitrary ambient llama binary or mark weights-only bundles installable.
    if !c.artifacts.iter().any(|a| a.kind == ArtifactKind::Weights)
        || !c
            .artifacts
            .iter()
            .any(|a| a.kind == ArtifactKind::Other && a.format == "runtime-executable")
    {
        return Err(DownloadError::UnconfiguredBundle);
    }
    crate::local_install::private_owner(root).map_err(|_| DownloadError::UnsafeDestination)?;
    let store = root.join("model-provisioning-bundles");
    crate::local_install::private_dir(&store).map_err(DownloadError::Disk)?;
    let lock_path = store.join("transfer.lock");
    crate::local_install::checked_path(&lock_path).map_err(|_| DownloadError::UnsafeDestination)?;
    if lock_path.exists() {
        safe_regular_file(&lock_path).map_err(|_| DownloadError::UnsafeDestination)?;
    }
    let process_lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(disk)?;
    process_lease.try_lock().map_err(|_| {
        DownloadError::Policy("Another native process owns the transfer lease".into())
    })?;
    // OS advisory lock is automatically released on process death: no stale
    // lock-file deletion or unsafe PID guessing is needed for crash recovery.
    let mut names = BTreeSet::new();
    for a in &c.artifacts {
        for name in [
            a.id.clone(),
            format!("{}.part", a.id),
            format!("{}.resume.json", a.id),
        ] {
            if !names.insert(name) {
                return Err(DownloadError::UnconfiguredBundle);
            }
        }
    }
    let id = hex::encode(Sha256::digest(
        serde_json::to_vec(c).map_err(|_| DownloadError::Integrity)?,
    ));
    let staging = store.join(format!("{id}.partial"));
    let published = store.join(format!("{id}.staged"));
    if published.exists() {
        crate::local_install::private_owner(&published)
            .map_err(|_| DownloadError::UnsafeDestination)?;
        for a in &c.artifacts {
            hash_file(&published.join(&a.id), a.download_bytes, &a.sha256, cancel)?;
        }
        guard.checkpoint().await?;
        check_cancel(cancel)?;
        return Ok(StagedBundle {
            directory: published,
        });
    }
    let reservation = storage_reservation(c).ok_or(DownloadError::Size)?;
    match crate::provisioning_probe::usable_storage(&store)
        .await
        .measured()
    {
        Some(free) if *free >= reservation => {}
        _ => {
            return Err(DownloadError::Disk(
                "Free installation space is unknown or below the conservative bundle reservation"
                    .into(),
            ))
        }
    }
    crate::local_install::private_dir(&staging).map_err(DownloadError::Disk)?;
    for a in &c.artifacts {
        transfer(origin, a, &staging, guard, cancel, &progress).await?;
    }
    guard.checkpoint().await?;
    check_cancel(cancel)?;
    // Whole bundle publication is one same-filesystem rename; never touches old
    // versions, existing MODELS, manifests or activation state. No extraction.
    sync_directory(&staging).map_err(DownloadError::Disk)?;
    fs::rename(&staging, &published).map_err(disk)?;
    sync_directory(&store).map_err(DownloadError::Disk)?;
    Ok(StagedBundle {
        directory: published,
    })
}

#[cfg(test)]
#[path = "provisioning_download_tests.rs"]
mod tests;
