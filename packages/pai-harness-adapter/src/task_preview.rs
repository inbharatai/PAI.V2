//! Stage 5 managed dynamic preview (owner C).
//!
//! The dev server runs INSIDE `isolation::workspace` service containment with
//! a private network namespace (only `lo`). The in-sandbox relay dials OUT over
//! a read-only-bound Unix socket; this module is the host side: a minimal
//! HTTP/1.1 reverse proxy on `127.0.0.1:<ephemeral>` that
//! - requires a one-time capability token (`/__pai/open?t=`) exchanged for a
//!   per-session HttpOnly SameSite=Strict cookie; anything else gets 403 and is
//!   NEVER forwarded; token/cookie values are never forwarded or logged;
//! - rejects DNS-rebinding Host headers (421), CONNECT, Upgrade, any
//!   Transfer-Encoding (chunked => 411), bad/oversized Content-Length, Expect;
//! - re-serializes the request head (no raw splice), strips hop-by-hop headers
//!   and the `pai_preview_*` cookie, forces `Connection: close`;
//! - appends CSP / nosniff / no-referrer / CORP headers to every response;
//! - bounds everything: 16 KiB heads, 64 headers, 1 MiB request bodies, 8 MiB
//!   responses, 2 s tunnel wait (503 on pool exhaustion, never a hang), a
//!   64 KiB log ring and a 1,024-record request ring with truncation metadata;
//! - bounds connection slots: absolute 10 s head / 30 s body deadlines (408),
//!   at most 16 of the 32 slots held by connections without a valid
//!   capability (a newcomer evicts the oldest of them instead of being
//!   refused), every live connection shut down on stop / idle stop.
//!
//! Evidence produced here is HTTP-LEVEL only (`EvidenceLevel::HttpLevel`): it
//! proves status/bytes for the requested methods and paths, NOT browser
//! rendering, DOM, JavaScript or console behaviour.

use crate::isolation::workspace::{
    ServiceEvent, ServiceHandle, ServiceLink, ServiceSocketDir, ServiceSpec, StagedTree,
    StopReport, Stream, Tunnel, WatchdogEvent, WorkspaceIsolation,
};
use crate::isolation::{hash, nonce, Cancellation, IsolationError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Combined stdout+stderr bytes retained per preview session.
pub const LOG_RING_BYTES: usize = 65_536;
pub const REQUEST_RING_RECORDS: usize = 1024;
pub const MAX_SESSIONS: usize = 2;
pub const PREVIEW_CSP: &str = "default-src 'self' 'unsafe-inline' 'unsafe-eval' data: blob:; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'self'";
const SECURITY_HEADERS: [(&str, &str); 4] = [
    ("Content-Security-Policy", PREVIEW_CSP),
    ("X-Content-Type-Options", "nosniff"),
    ("Referrer-Policy", "no-referrer"),
    ("Cross-Origin-Resource-Policy", "same-origin"),
];
const HEAD_MAX: usize = 16 * 1024;
const MAX_HEADERS: usize = 64;
const BODY_MAX: u64 = 1024 * 1024;
const RESPONSE_MAX: u64 = 8 * 1024 * 1024;
const CHECK_BODY_MAX: usize = 1024 * 1024;
const TUNNEL_WAIT: Duration = Duration::from_secs(2);
/// Per-read idle bound for client reads (never past the absolute deadlines).
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
/// Absolute bound, from accept, for receiving a complete request head: a
/// client trickling bytes cannot hold a connection slot longer (408).
const HEAD_DEADLINE: Duration = Duration::from_secs(10);
/// Absolute bound for receiving a request body (cookie holders only) once
/// its head was accepted (408).
const BODY_DEADLINE: Duration = Duration::from_secs(30);
const UPSTREAM_HEAD_TIMEOUT: Duration = Duration::from_secs(10);
const UPSTREAM_BODY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONNECTIONS: usize = 32;
/// Connections that have not (yet) presented a valid cookie or one-time
/// token hold at most this many of the MAX_CONNECTIONS slots; the rest stay
/// reserved for authenticated exchanges. A newcomer that finds this share
/// (or the whole table) full evicts the OLDEST pre-auth connection instead
/// of being refused.
const MAX_PREAUTH: usize = 16;
/// Hard cap on connection threads (evicted threads exit asynchronously).
const MAX_CONN_THREADS: usize = MAX_CONNECTIONS + MAX_PREAUTH;
/// How long a stopping bridge waits for its connection threads to exit
/// after shutting their sockets down.
const CONN_DRAIN_WAIT: Duration = Duration::from_secs(3);
/// Bound for `stop` / `stop_all` waiting on an in-flight idle reap.
const REAP_WAIT: Duration = Duration::from_secs(15);
/// Reaped idle sessions whose final record was not yet collected by `stop`
/// (the oldest are dropped beyond this).
const MAX_RETIRED: usize = 8;
/// The only method strings ever written to the request log; any other
/// client-chosen method (which could carry a secret) is logged as "OTHER".
const LOGGABLE_METHODS: [&str; 9] = [
    "GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "CONNECT", "TRACE",
];
const PATH_LOG_MAX: usize = 512;
const STOP_GRACE: Duration = Duration::from_secs(3);
const OPEN_PATH: &str = "/__pai/open";
const LINGER_MAX: Duration = Duration::from_millis(500);
const LINGER_STEP: Duration = Duration::from_millis(100);
const LINGER_BYTES: usize = 2 * 1024 * 1024;
const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// Owner A types (task_ledger): the task identity and the lock-epoch guard
// captured at admission (`BootInfo::guard`) and re-checked here.
pub use crate::task_ledger::{EpochGuard, TaskId};
fn epoch_current(epoch: &EpochGuard) -> bool {
    epoch.check().is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewSpec {
    pub service: ServiceSpec,
    /// Default "/".
    pub readiness_path: String,
    /// Default 200..=399.
    pub ready_status: (u16, u16),
    /// 500..=30_000.
    pub startup_timeout_ms: u64,
    /// Default 600_000 (1_000..=86_400_000).
    pub idle_stop_ms: u64,
    /// <= 16.
    pub http_checks: Vec<HttpCheck>,
}
impl PreviewSpec {
    pub fn with_defaults(service: ServiceSpec) -> Self {
        Self {
            service,
            readiness_path: "/".into(),
            ready_status: (200, 399),
            startup_timeout_ms: 10_000,
            idle_stop_ms: 600_000,
            http_checks: Vec::new(),
        }
    }
    fn validate(&self) -> Result<(), PreviewError> {
        let status_ok = |(lo, hi): (u16, u16)| (100..=599).contains(&lo) && lo <= hi && hi <= 599;
        let mut ids = BTreeSet::new();
        if !valid_path(&self.readiness_path)
            || !status_ok(self.ready_status)
            || !(500..=30_000).contains(&self.startup_timeout_ms)
            || !(1_000..=86_400_000).contains(&self.idle_stop_ms)
            || self.http_checks.len() > 16
            || self.http_checks.iter().any(|c| {
                !ids.insert(c.id.clone())
                    || !(1..=64).contains(&c.id.len())
                    || !c
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                    || !valid_path(&c.path)
                    || !status_ok(c.expect_status)
                    || c.body.as_ref().is_some_and(|b| b.len() > 4096)
                    || (c.method == HttpMethod::Get && c.body.is_some())
                    || c.expect_content_type_prefix
                        .as_ref()
                        .is_some_and(|p| p.len() > 128)
                    || c.expect_body_contains.len() > 8
                    || c.expect_body_contains.iter().any(|s| s.len() > 256)
                    || c.expect_json_equals.as_ref().is_some_and(|v| {
                        serde_json::to_vec(v).map_or(true, |b| b.len() > 16 * 1024)
                    })
            })
        {
            return Err(PreviewError::InvalidSpec);
        }
        Ok(())
    }
}
/// Origin-form path: starts with '/', not '//', printable ASCII without
/// spaces, <= 512 bytes, never the reserved /__pai/ namespace.
fn valid_path(p: &str) -> bool {
    p.starts_with('/')
        && !p.starts_with("//")
        && !p.starts_with("/__pai/")
        && p.len() <= PATH_LOG_MAX
        && p.bytes().all(|b| b.is_ascii_graphic())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpCheck {
    pub id: String,
    pub method: HttpMethod,
    pub path: String,
    pub body: Option<String>,
    pub expect_status: (u16, u16),
    pub expect_content_type_prefix: Option<String>,
    pub expect_body_contains: Vec<String>,
    pub expect_json_equals: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpMethod {
    Get,
    Post,
}
impl HttpMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLevel {
    /// Host-bridge HTTP status/bytes only; NOT browser rendering.
    HttpLevel,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpCheckRecord {
    pub tree_sha256: String,
    pub service_id: String,
    pub evidence_level: EvidenceLevel,
    pub results: Vec<HttpCheckResult>,
    pub at_ms: u64,
}
impl HttpCheckRecord {
    /// A record from another server or other content is Stale, never current.
    pub fn is_current(&self, tree_sha256: &str, service_id: &str) -> bool {
        self.tree_sha256 == tree_sha256 && self.service_id == service_id
    }
    pub fn all_passed(&self) -> bool {
        !self.results.is_empty() && self.results.iter().all(|r| r.passed)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpCheckResult {
    pub id: String,
    pub status: Option<u16>,
    pub passed: bool,
    pub body_sha256: Option<String>,
    /// <= 1 KiB, lossy UTF-8, untrusted server output.
    pub excerpt: String,
    pub elapsed_ms: u64,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewStart {
    pub descriptor: ServiceDescriptor,
    /// `http://127.0.0.1:<port>/__pai/open?t=<32 hex>`; only when Ready.
    pub capability_url: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceDescriptor {
    pub service_id: String,
    pub task_id: String,
    pub tree_sha256: String,
    pub spec_sha256: String,
    pub workspace_profile_sha256: String,
    /// 0 when no bridge was opened (startup failed).
    pub bridge_port: u16,
    pub started_at_ms: u64,
    pub ready: ReadyState,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadyState {
    Ready { after_ms: u64 },
    StartupFailed { reason: StartupFailure },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupFailure {
    Timeout,
    /// `None`: killed by the host (e.g. soft watchdog) before readiness.
    Exited {
        status: Option<i32>,
    },
    Http {
        status: u16,
    },
    RunnerFailure,
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};
    pub(super) fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::isolation::hex(b))
    }
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        if s.len() % 2 != 0 {
            return Err(serde::de::Error::custom("odd hex"));
        }
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(serde::de::Error::custom)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogRecord {
    pub seq: u64,
    pub at_ms: u64,
    pub stream: Stream,
    #[serde(with = "hex_bytes")]
    pub bytes: Vec<u8>,
}
/// Host-memory-bounded log ring (64 KiB combined stdout+stderr).
#[derive(Debug, Clone)]
pub struct LogRing {
    cap_bytes: usize,
    records: VecDeque<LogRecord>,
    retained: usize,
    next_seq: u64,
    /// Bytes/records evicted from this ring (oldest first).
    pub dropped_bytes: u64,
    pub dropped_records: u64,
    pub first_retained_seq: u64,
    /// Bytes dropped before reaching the host (supervisor rate limiter or the
    /// bounded host event queue).
    pub supervisor_dropped_bytes: u64,
}
impl Default for LogRing {
    fn default() -> Self {
        Self::new()
    }
}
impl LogRing {
    pub fn new() -> Self {
        Self::with_cap(LOG_RING_BYTES)
    }
    /// `cap` is clamped to 1..=LOG_RING_BYTES.
    pub fn with_cap(cap: usize) -> Self {
        Self {
            cap_bytes: cap.clamp(1, LOG_RING_BYTES),
            records: VecDeque::new(),
            retained: 0,
            next_seq: 0,
            dropped_bytes: 0,
            dropped_records: 0,
            first_retained_seq: 0,
            supervisor_dropped_bytes: 0,
        }
    }
    pub fn push(&mut self, stream: Stream, mut bytes: Vec<u8>, at_ms: u64) {
        if bytes.len() > self.cap_bytes {
            self.dropped_bytes += (bytes.len() - self.cap_bytes) as u64;
            bytes.drain(..bytes.len() - self.cap_bytes);
        }
        self.retained += bytes.len();
        self.records.push_back(LogRecord {
            seq: self.next_seq,
            at_ms,
            stream,
            bytes,
        });
        self.next_seq += 1;
        while self.retained > self.cap_bytes {
            if let Some(old) = self.records.pop_front() {
                self.retained -= old.bytes.len();
                self.dropped_bytes += old.bytes.len() as u64;
                self.dropped_records += 1;
            }
        }
        self.first_retained_seq = self.records.front().map_or(self.next_seq, |r| r.seq);
    }
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }
    pub fn cap_bytes(&self) -> usize {
        self.cap_bytes
    }
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }
    pub fn chunk(&self, cursor: u64, limit: u32) -> LogChunk {
        let records: Vec<LogRecord> = self
            .records
            .iter()
            .filter(|r| r.seq >= cursor)
            .take(limit as usize)
            .cloned()
            .collect();
        LogChunk {
            next_cursor: records
                .last()
                .map_or(cursor.max(self.first_retained_seq), |r| r.seq + 1),
            truncated_before_cursor: cursor < self.first_retained_seq,
            records,
            first_retained_seq: self.first_retained_seq,
            retained_bytes: self.retained as u64,
            cap_bytes: self.cap_bytes as u64,
            dropped_bytes: self.dropped_bytes,
            dropped_records: self.dropped_records,
            supervisor_dropped_bytes: self.supervisor_dropped_bytes,
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogChunk {
    pub records: Vec<LogRecord>,
    pub next_cursor: u64,
    /// The cursor pointed at records already evicted from the ring.
    pub truncated_before_cursor: bool,
    pub first_retained_seq: u64,
    pub retained_bytes: u64,
    pub cap_bytes: u64,
    pub dropped_bytes: u64,
    pub dropped_records: u64,
    pub supervisor_dropped_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestRecord {
    pub seq: u64,
    pub at_ms: u64,
    pub method: String,
    /// <= 512 bytes; capability tokens replaced by "[redacted]".
    pub path: String,
    pub status: Option<u16>,
    pub response_bytes: u64,
    pub elapsed_ms: u64,
    pub outcome: RequestOutcome,
    /// Extension of §6.2: who issued the request (browser bridge vs. the
    /// product's own HTTP checks), so check traffic is never mistaken for
    /// browser traffic.
    pub source: RequestSource,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestSource {
    Bridge,
    HttpCheck,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    Ok,
    Rejected { reason: RejectReason },
    UpstreamClosed,
    Timeout,
    ResponseTruncated,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    Malformed,
    HeadTooLarge,
    ClientTimeout,
    MethodNotAllowed,
    ConnectRejected,
    UpgradeRejected,
    ChunkedRejected,
    TransferEncodingRejected,
    BadContentLength,
    BodyTooLarge,
    ExpectRejected,
    AbsoluteFormRejected,
    HostRejected,
    CapabilityMissing,
    TokenInRequest,
    ReservedPath,
    PoolExhausted,
    TooManyConnections,
    BadUpstreamResponse,
}
#[derive(Debug, Default)]
struct RequestRing {
    records: VecDeque<RequestRecord>,
    next_seq: u64,
}
impl RequestRing {
    fn push(&mut self, mut r: RequestRecord) {
        r.seq = self.next_seq;
        self.next_seq += 1;
        self.records.push_back(r);
        while self.records.len() > REQUEST_RING_RECORDS {
            self.records.pop_front();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReasonKind {
    User,
    Idle,
    Locked,
    Shutdown,
    TaskClosed,
    StartupFailed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopRecord {
    pub service_id: String,
    pub task_id: String,
    pub tree_sha256: String,
    pub reason: StopReasonKind,
    pub report: StopReport,
    pub socket_dir_removed: bool,
    pub bridge_closed: bool,
    pub at_ms: u64,
    /// Final LogRing snapshot (A persists it as a PreviewLog blob).
    pub logs: LogChunk,
    /// Final request-ring snapshot (A persists it as a RequestLog blob).
    pub requests: Vec<RequestRecord>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewStatusView {
    pub task_id: String,
    pub state: PreviewState,
    pub service_id: Option<String>,
    pub tree_sha256: Option<String>,
    pub bridge_port: Option<u16>,
    pub running: bool,
    pub log_next_seq: u64,
    pub requests_total: u64,
    pub evidence_level: EvidenceLevel,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewState {
    NotStarted,
    StartupFailed { reason: StartupFailure },
    Ready { after_ms: u64 },
    Exited { status: Option<i32> },
    RunnerFailure,
    WatchdogKilled { event: WatchdogEvent },
    IdleStopped,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewError {
    IsolationUnavailable,
    Locked,
    Busy,
    AlreadyRunning,
    NotRunning,
    InvalidSpec,
    Isolation(IsolationError),
    Io,
}
impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "preview: {self:?}")
    }
}
impl std::error::Error for PreviewError {}
impl From<IsolationError> for PreviewError {
    fn from(e: IsolationError) -> Self {
        match e {
            IsolationError::IsolationUnavailable => Self::IsolationUnavailable,
            IsolationError::InvalidInput => Self::InvalidSpec,
            e => Self::Isolation(e),
        }
    }
}

// ------------------------------------------------------------ the manager

#[derive(Default)]
struct ServiceState {
    exited: Option<Option<i32>>,
    runner_failure: bool,
    watchdog: Option<WatchdogEvent>,
}
struct SessionShared {
    link: ServiceLink,
    logs: Mutex<LogRing>,
    requests: Mutex<RequestRing>,
    state: Mutex<ServiceState>,
    stop: AtomicBool,
    last_activity: Mutex<Instant>,
    idle_stopped: AtomicBool,
}
impl SessionShared {
    fn record(&self, r: RequestRecord) {
        lock(&self.requests).push(r);
    }
}
struct PreviewSession {
    descriptor: ServiceDescriptor,
    spec: PreviewSpec,
    shared: Arc<SessionShared>,
    handle: Option<ServiceHandle>,
    cancel: Cancellation,
    sock: Option<ServiceSocketDir>,
    /// Host path of the socket dir (checked for removal at stop).
    sock_path: Option<PathBuf>,
    bridge: Option<Bridge>,
    pump: Option<thread::JoinHandle<()>>,
    /// Kill report when the service was already halted (startup failure):
    /// a service that never became Ready is never left running.
    halted: Option<StopReport>,
}
/// An idle-stopped session whose service, socket dir, bridge and pump the
/// reaper released (`record` set) or is releasing (`record` None). Once
/// released it no longer holds a MAX_SESSIONS slot; its views and final
/// record stay until `stop` collects them (at most MAX_RETIRED are kept).
struct Retired {
    descriptor: ServiceDescriptor,
    shared: Arc<SessionShared>,
    record: Option<StopRecord>,
    seq: u64,
}
#[derive(Default)]
struct Sessions {
    live: BTreeMap<TaskId, PreviewSession>,
    starting: BTreeSet<TaskId>,
    retired: BTreeMap<TaskId, Retired>,
    retired_seq: u64,
}
impl Sessions {
    /// Retired sessions whose resources are still being released.
    fn reaping(&self) -> usize {
        self.retired.values().filter(|r| r.record.is_none()).count()
    }
    fn shared(&self, task: &TaskId) -> Option<&Arc<SessionShared>> {
        self.live
            .get(task)
            .map(|s| &s.shared)
            .or_else(|| self.retired.get(task).map(|r| &r.shared))
    }
}
/// Given to a session's bridge so its idle stop can reap the session.
struct ReapHook {
    sessions: Weak<Mutex<Sessions>>,
    task: TaskId,
    service_id: String,
}
/// At most MAX_SESSIONS concurrent previews; every session owns its service
/// (owner thread + Child), socket dir, bridge listener and log rings.
pub struct PreviewManager {
    sessions: Arc<Mutex<Sessions>>,
    scratch: PathBuf,
}
impl PreviewManager {
    /// `scratch` is controller-private scratch (short path: it hosts ctl.sock).
    pub fn new(scratch: &Path) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(Sessions::default())),
            scratch: scratch.to_path_buf(),
        }
    }
    pub fn start(
        &self,
        iso: &WorkspaceIsolation,
        task: &TaskId,
        tree: &StagedTree,
        spec: &PreviewSpec,
        epoch: &EpochGuard,
    ) -> Result<PreviewStart, PreviewError> {
        if !cfg!(target_os = "linux") {
            return Err(PreviewError::IsolationUnavailable);
        }
        if !epoch_current(epoch) {
            return Err(PreviewError::Locked);
        }
        spec.validate()?;
        {
            let mut s = lock(&self.sessions);
            // A reaped idle session keeps its task until `stop` collects its
            // record, but no longer counts against MAX_SESSIONS.
            if s.live.contains_key(task)
                || s.starting.contains(task)
                || s.retired.contains_key(task)
            {
                return Err(PreviewError::AlreadyRunning);
            }
            if s.live.len() + s.starting.len() + s.reaping() >= MAX_SESSIONS {
                return Err(PreviewError::Busy);
            }
            s.starting.insert(task.clone());
        }
        let started = self.start_session(iso, task, tree, spec, epoch);
        let mut s = lock(&self.sessions);
        s.starting.remove(task);
        let (session, start) = started?;
        s.live.insert(task.clone(), session);
        Ok(start)
    }
    fn start_session(
        &self,
        iso: &WorkspaceIsolation,
        task: &TaskId,
        tree: &StagedTree,
        spec: &PreviewSpec,
        epoch: &EpochGuard,
    ) -> Result<(PreviewSession, PreviewStart), PreviewError> {
        let started_at_ms = now_ms();
        let sock = ServiceSocketDir::create(&self.scratch)?;
        let sock_path = socket_dir_path(&sock);
        let cancel = Cancellation::default();
        let handle = iso.start_service(tree, &spec.service, &sock, cancel.clone())?;
        let shared = Arc::new(SessionShared {
            link: handle.link(),
            logs: Mutex::new(LogRing::new()),
            requests: Mutex::new(RequestRing::default()),
            state: Mutex::new(ServiceState::default()),
            stop: AtomicBool::new(false),
            last_activity: Mutex::new(Instant::now()),
            idle_stopped: AtomicBool::new(false),
        });
        let pump_shared = shared.clone();
        let pump = thread::spawn(move || pump_main(pump_shared));
        let mut session = PreviewSession {
            descriptor: ServiceDescriptor {
                service_id: handle.id().to_string(),
                task_id: task.as_str().to_string(),
                tree_sha256: tree.tree_sha256().to_string(),
                spec_sha256: hash(
                    &serde_json::to_vec(spec).map_err(|_| PreviewError::InvalidSpec)?,
                ),
                workspace_profile_sha256: iso.profile_sha256(),
                bridge_port: 0,
                started_at_ms,
                ready: ReadyState::StartupFailed {
                    reason: StartupFailure::Timeout,
                },
            },
            spec: spec.clone(),
            shared: shared.clone(),
            handle: Some(handle),
            cancel: cancel.clone(),
            sock: Some(sock),
            sock_path,
            bridge: None,
            pump: Some(pump),
            halted: None,
        };
        session.descriptor.ready = wait_ready(&shared, spec);
        if matches!(session.descriptor.ready, ReadyState::StartupFailed { .. }) {
            // Never leave a not-ready server running without a bridge; the
            // session keeps its logs until `stop` returns the StopRecord.
            session.halted = Some(halt(&mut session));
        }
        if !epoch_current(epoch) {
            stop_session(session, StopReasonKind::Locked);
            return Err(PreviewError::Locked);
        }
        let mut capability_url = None;
        if matches!(session.descriptor.ready, ReadyState::Ready { .. }) {
            let reap = ReapHook {
                sessions: Arc::downgrade(&self.sessions),
                task: task.clone(),
                service_id: session.descriptor.service_id.clone(),
            };
            match Bridge::start(
                shared,
                cancel,
                &session.descriptor.service_id,
                spec.idle_stop_ms,
                reap,
            ) {
                Ok((bridge, url)) => {
                    session.descriptor.bridge_port = bridge.port;
                    session.bridge = Some(bridge);
                    capability_url = Some(url);
                }
                Err(e) => {
                    stop_session(session, StopReasonKind::StartupFailed);
                    return Err(e);
                }
            }
        }
        let start = PreviewStart {
            descriptor: session.descriptor.clone(),
            capability_url,
        };
        Ok((session, start))
    }
    pub fn status(&self, task: &TaskId) -> PreviewStatusView {
        let s = lock(&self.sessions);
        if let Some(sess) = s.live.get(task) {
            return status_view(
                task,
                &sess.descriptor,
                &sess.shared,
                sess.descriptor.bridge_port,
            );
        }
        if let Some(r) = s.retired.get(task) {
            // Reaped after an idle stop: the bridge is closed.
            return status_view(task, &r.descriptor, &r.shared, 0);
        }
        PreviewStatusView {
            task_id: task.as_str().to_string(),
            state: PreviewState::NotStarted,
            service_id: None,
            tree_sha256: None,
            bridge_port: None,
            running: false,
            log_next_seq: 0,
            requests_total: 0,
            evidence_level: EvidenceLevel::HttpLevel,
        }
    }
    pub fn logs(&self, task: &TaskId, cursor: u64, limit: u32) -> LogChunk {
        let s = lock(&self.sessions);
        s.shared(task)
            .map(|shared| lock(&shared.logs).chunk(cursor, limit))
            .unwrap_or_default()
    }
    pub fn requests(&self, task: &TaskId, cursor: u64, limit: u32) -> Vec<RequestRecord> {
        let s = lock(&self.sessions);
        s.shared(task)
            .map(|shared| {
                lock(&shared.requests)
                    .records
                    .iter()
                    .filter(|r| r.seq >= cursor)
                    .take(limit as usize)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Host-side checks through the internal client, bound to the running
    /// service id and tree. HTTP-level evidence only.
    pub fn run_http_checks(&self, task: &TaskId) -> Result<HttpCheckRecord, PreviewError> {
        let (shared, checks, descriptor) = {
            let s = lock(&self.sessions);
            let sess = s.live.get(task).ok_or(PreviewError::NotRunning)?;
            if !matches!(sess.descriptor.ready, ReadyState::Ready { .. })
                || !sess.shared.link.is_running()
            {
                return Err(PreviewError::NotRunning);
            }
            (
                sess.shared.clone(),
                sess.spec.http_checks.clone(),
                sess.descriptor.clone(),
            )
        };
        let results = checks.iter().map(|c| run_check(&shared, c)).collect();
        Ok(HttpCheckRecord {
            tree_sha256: descriptor.tree_sha256,
            service_id: descriptor.service_id,
            evidence_level: EvidenceLevel::HttpLevel,
            results,
            at_ms: now_ms(),
        })
    }
    /// Kill only this task's owned namespace, close the bridge listener and
    /// its connections, remove the socket directory and return the final
    /// snapshots. For a session already reaped after an idle stop this
    /// collects its final record (waiting for an in-flight reap, bounded).
    pub fn stop(&self, task: &TaskId, reason: StopReasonKind) -> Option<StopRecord> {
        let deadline = Instant::now() + REAP_WAIT;
        loop {
            let mut s = lock(&self.sessions);
            if let Some(session) = s.live.remove(task) {
                drop(s);
                return Some(stop_session(session, reason));
            }
            let reaped = s.retired.get(task)?.record.is_some();
            if reaped {
                let mut record = s.retired.remove(task)?.record?;
                record.reason = reason;
                return Some(record);
            }
            if Instant::now() >= deadline {
                return None;
            }
            drop(s);
            thread::sleep(Duration::from_millis(10));
        }
    }
    /// Vault lock / shutdown. Returns after in-flight idle reaps finished
    /// (bounded); uncollected reaped records are discarded.
    pub fn stop_all(&self, reason: StopReasonKind) {
        let sessions = std::mem::take(&mut lock(&self.sessions).live);
        for (_, s) in sessions {
            stop_session(s, reason);
        }
        let deadline = Instant::now() + REAP_WAIT;
        loop {
            let mut s = lock(&self.sessions);
            if s.reaping() == 0 || Instant::now() >= deadline {
                s.retired.clear();
                return;
            }
            drop(s);
            thread::sleep(Duration::from_millis(10));
        }
    }
}
fn status_view(
    task: &TaskId,
    descriptor: &ServiceDescriptor,
    shared: &SessionShared,
    bridge_port: u16,
) -> PreviewStatusView {
    let log_next_seq = lock(&shared.logs).next_seq();
    let requests_total = lock(&shared.requests).next_seq;
    let st = lock(&shared.state);
    let state = if let ReadyState::StartupFailed { reason } = &descriptor.ready {
        PreviewState::StartupFailed {
            reason: reason.clone(),
        }
    } else if shared.idle_stopped.load(Ordering::SeqCst) {
        PreviewState::IdleStopped
    } else if let Some(w) = &st.watchdog {
        PreviewState::WatchdogKilled { event: w.clone() }
    } else if st.runner_failure {
        PreviewState::RunnerFailure
    } else if let Some(status) = st.exited {
        PreviewState::Exited { status }
    } else if let ReadyState::Ready { after_ms } = descriptor.ready {
        PreviewState::Ready { after_ms }
    } else {
        PreviewState::NotStarted
    };
    drop(st);
    PreviewStatusView {
        task_id: task.as_str().to_string(),
        state,
        service_id: Some(descriptor.service_id.clone()),
        tree_sha256: Some(descriptor.tree_sha256.clone()),
        bridge_port: (bridge_port != 0).then_some(bridge_port),
        running: shared.link.is_running(),
        log_next_seq,
        requests_total,
        evidence_level: EvidenceLevel::HttpLevel,
    }
}
/// Idle stop (spawned by the bridge acceptor): release the session's service,
/// socket dir, bridge connections and pump exactly like `stop`, free its
/// MAX_SESSIONS slot, and keep its views and final record until `stop`
/// collects them.
fn reap_idle(hook: ReapHook) {
    let Some(sessions) = hook.sessions.upgrade() else {
        return;
    };
    let waited = Instant::now();
    let session = loop {
        let mut s = lock(&sessions);
        let current = s
            .live
            .get(&hook.task)
            .map(|sess| sess.descriptor.service_id == hook.service_id);
        match current {
            Some(true) => {
                let Some(sess) = s.live.remove(&hook.task) else {
                    return;
                };
                s.retired_seq += 1;
                let seq = s.retired_seq;
                s.retired.insert(
                    hook.task.clone(),
                    Retired {
                        descriptor: sess.descriptor.clone(),
                        shared: sess.shared.clone(),
                        record: None,
                        seq,
                    },
                );
                break sess;
            }
            // `start` registers the session right after its bridge started.
            None if s.starting.contains(&hook.task) && waited.elapsed() < REAP_WAIT => {}
            // Already stopped, or the task now runs another service.
            _ => return,
        }
        drop(s);
        thread::sleep(Duration::from_millis(10));
    };
    let record = stop_session(session, StopReasonKind::Idle);
    let mut s = lock(&sessions);
    if let Some(r) = s
        .retired
        .get_mut(&hook.task)
        .filter(|r| r.descriptor.service_id == hook.service_id)
    {
        r.record = Some(record);
    }
    loop {
        let collected = s.retired.values().filter(|r| r.record.is_some()).count();
        let oldest = s
            .retired
            .iter()
            .filter(|(_, r)| r.record.is_some())
            .min_by_key(|(_, r)| r.seq)
            .map(|(t, _)| t.clone());
        match oldest {
            Some(t) if collected > MAX_RETIRED => {
                s.retired.remove(&t);
            }
            _ => break,
        }
    }
}
impl Drop for PreviewManager {
    fn drop(&mut self) {
        self.stop_all(StopReasonKind::Shutdown);
    }
}

#[cfg(target_os = "linux")]
fn socket_dir_path(d: &ServiceSocketDir) -> Option<PathBuf> {
    Some(d.path().to_path_buf())
}
#[cfg(not(target_os = "linux"))]
fn socket_dir_path(_: &ServiceSocketDir) -> Option<PathBuf> {
    None
}
/// Kill the owned namespace (only this session's Child) and remove the
/// socket directory.
fn halt(s: &mut PreviewSession) -> StopReport {
    s.cancel.cancel();
    let report = s.handle.take().map_or(
        StopReport {
            killed: false,
            exit_status: None,
            descendants_alive_after: 0,
            elapsed_ms: 0,
        },
        |h| h.stop(STOP_GRACE),
    );
    // Dropping the ServiceSocketDir closes the listener and removes the dir.
    s.sock = None;
    report
}
fn stop_session(mut s: PreviewSession, reason: StopReasonKind) -> StopRecord {
    // Listener first (pre-auth connections are shut down, authenticated ones
    // stop reading so an in-flight exchange can still answer), then the
    // service, then every remaining bridge connection.
    let mut bridge = s.bridge.take();
    let listener_closed = bridge.as_mut().is_none_or(Bridge::close_listener);
    let report = match s.halted.take() {
        Some(r) => r,
        None => halt(&mut s),
    };
    s.shared.stop.store(true, Ordering::SeqCst);
    if let Some(p) = s.pump.take() {
        let _ = p.join();
    }
    let bridge_closed = bridge.is_none_or(Bridge::drain) && listener_closed;
    let logs = lock(&s.shared.logs).chunk(0, u32::MAX);
    let requests = lock(&s.shared.requests).records.iter().cloned().collect();
    StopRecord {
        service_id: s.descriptor.service_id,
        task_id: s.descriptor.task_id,
        tree_sha256: s.descriptor.tree_sha256,
        reason,
        report,
        socket_dir_removed: s.sock_path.as_ref().is_none_or(|p| !p.exists()),
        bridge_closed,
        at_ms: now_ms(),
        logs,
        requests,
    }
}

fn pump_main(shared: Arc<SessionShared>) {
    loop {
        let events = shared.link.poll_events(4096);
        let got = !events.is_empty();
        for e in events {
            apply_event(&shared, e);
        }
        if shared.stop.load(Ordering::SeqCst) || (!got && !shared.link.is_running()) {
            for e in shared.link.poll_events(usize::MAX) {
                apply_event(&shared, e);
            }
            return;
        }
        if !got {
            thread::sleep(Duration::from_millis(25));
        }
    }
}
fn apply_event(shared: &SessionShared, e: ServiceEvent) {
    match e {
        ServiceEvent::Log { stream, bytes } => lock(&shared.logs).push(stream, bytes, now_ms()),
        ServiceEvent::Dropped { bytes, .. } => lock(&shared.logs).supervisor_dropped_bytes += bytes,
        ServiceEvent::Exited { status } => lock(&shared.state).exited = Some(status),
        ServiceEvent::RunnerFailure => lock(&shared.state).runner_failure = true,
        ServiceEvent::Watchdog(w) => lock(&shared.state).watchdog = Some(w),
    }
}
fn startup_failure(shared: &SessionShared) -> Option<StartupFailure> {
    let st = lock(&shared.state);
    if st.runner_failure {
        Some(StartupFailure::RunnerFailure)
    } else if st.watchdog.is_some() {
        Some(StartupFailure::Exited { status: None })
    } else {
        st.exited.map(|status| StartupFailure::Exited { status })
    }
}
/// Ready on the first `ready_status` answer to GET readiness_path (internal
/// path, no token) before the timeout; exit / runner failure / timeout are
/// StartupFailed and never ready.
fn wait_ready(shared: &SessionShared, spec: &PreviewSpec) -> ReadyState {
    let t0 = Instant::now();
    let deadline = t0 + Duration::from_millis(spec.startup_timeout_ms);
    let mut last_http = None;
    loop {
        if let Some(reason) = startup_failure(shared) {
            return ReadyState::StartupFailed { reason };
        }
        if !shared.link.is_running() {
            // Let the pump apply the final events, then decide.
            thread::sleep(Duration::from_millis(100));
            let reason = startup_failure(shared).unwrap_or(StartupFailure::RunnerFailure);
            return ReadyState::StartupFailed { reason };
        }
        let now = Instant::now();
        if now >= deadline {
            let reason = last_http.map_or(StartupFailure::Timeout, |status| StartupFailure::Http {
                status,
            });
            return ReadyState::StartupFailed { reason };
        }
        let left = deadline - now;
        if let Ok(r) = internal_request(
            &shared.link,
            "GET",
            &spec.readiness_path,
            None,
            left.min(TUNNEL_WAIT),
        ) {
            if (spec.ready_status.0..=spec.ready_status.1).contains(&r.status) {
                return ReadyState::Ready {
                    after_ms: t0.elapsed().as_millis() as u64,
                };
            }
            last_http = Some(r.status);
        }
        thread::sleep(
            Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

// ------------------------------------------------------ HTTP plumbing

#[derive(Debug, PartialEq, Eq)]
enum HeadError {
    TooLarge,
    Timeout,
    Closed,
    /// The bridge is stopping.
    Stopped,
}
/// Read until CRLFCRLF; the head INCLUDING its terminator must be <= max
/// bytes, however the bytes are chunked. Returns (head, bytes read past it).
fn read_head<R: Read>(r: &mut R, max: usize) -> Result<(Vec<u8>, Vec<u8>), HeadError> {
    read_head_with(r, max, |_| Ok(()))
}
/// `read_head` with `arm` run before EVERY read (stop flag, absolute
/// deadline, read-timeout arming); an `arm` error ends the read with it, so
/// a peer trickling bytes cannot extend the read past the caller's deadline.
fn read_head_with<R: Read>(
    r: &mut R,
    max: usize,
    mut arm: impl FnMut(&mut R) -> Result<(), HeadError>,
) -> Result<(Vec<u8>, Vec<u8>), HeadError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            if end + 4 > max {
                return Err(HeadError::TooLarge);
            }
            let rest = buf.split_off(end + 4);
            return Ok((buf, rest));
        }
        if buf.len() > max {
            return Err(HeadError::TooLarge);
        }
        arm(r)?;
        match r.read(&mut chunk) {
            Ok(0) => return Err(HeadError::Closed),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err(HeadError::Timeout)
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Err(HeadError::Closed),
        }
    }
}
struct ParsedRequest {
    method: String,
    path: String,
    headers: Vec<(String, Vec<u8>)>,
}
impl ParsedRequest {
    fn values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.headers
            .iter()
            .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_slice())
    }
    fn tokens(&self, name: &str) -> BTreeSet<String> {
        self.values(name)
            .flat_map(|v| {
                String::from_utf8_lossy(v)
                    .split(',')
                    .map(|t| t.trim().to_ascii_lowercase())
                    .collect::<Vec<_>>()
            })
            .filter(|t| !t.is_empty())
            .collect()
    }
}
fn parse_request(head: &[u8]) -> Result<ParsedRequest, RejectReason> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut headers);
    match req.parse(head) {
        Ok(httparse::Status::Complete(_)) => {}
        Err(httparse::Error::TooManyHeaders) => return Err(RejectReason::HeadTooLarge),
        _ => return Err(RejectReason::Malformed),
    }
    let (Some(method), Some(path), Some(_)) = (req.method, req.path, req.version) else {
        return Err(RejectReason::Malformed);
    };
    Ok(ParsedRequest {
        method: method.to_string(),
        path: path.to_string(),
        headers: req
            .headers
            .iter()
            .map(|h| (h.name.to_string(), h.value.to_vec()))
            .collect(),
    })
}
/// Response body delimitation (RFC 9112 §6.3): `Some(n)` for a
/// Content-Length body (0 for HEAD/1xx/204/304), `None` for a body that runs
/// until the upstream closes (e.g. chunked, passed through verbatim).
/// Invalid or conflicting Content-Length values are a bad upstream response.
fn response_framing(
    method: &str,
    code: u16,
    headers: &[httparse::Header<'_>],
) -> Result<Option<u64>, ()> {
    if method == "HEAD" || code == 204 || code == 304 || (100..200).contains(&code) {
        return Ok(Some(0));
    }
    if headers
        .iter()
        .any(|h| h.name.eq_ignore_ascii_case("transfer-encoding"))
    {
        return Ok(None);
    }
    let parse = |v: &[u8]| -> Option<u64> {
        (!v.is_empty() && v.len() <= 19 && v.iter().all(u8::is_ascii_digit))
            .then(|| String::from_utf8_lossy(v).parse().ok())
            .flatten()
    };
    let mut declared = None;
    for h in headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("content-length"))
    {
        match (parse(h.value), declared) {
            (Some(n), None) => declared = Some(n),
            (Some(n), Some(d)) if n == d => {}
            _ => return Err(()),
        }
    }
    Ok(declared)
}
/// `Set-Cookie` value whose cookie name is in the reserved `pai_preview_`
/// namespace (case-insensitive, leading whitespace ignored).
fn is_bridge_cookie(set_cookie: &[u8]) -> bool {
    let v = String::from_utf8_lossy(set_cookie);
    let name = v.split(['=', ';']).next().unwrap_or("").trim();
    name.to_ascii_lowercase().starts_with("pai_preview_")
}
fn simple_response(code: u16, reason: &str, extra: &[(&str, String)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {code} {reason}\r\n").into_bytes();
    for (k, v) in extra {
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    out.extend_from_slice(b"Content-Type: text/plain; charset=utf-8\r\n");
    out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    for (k, v) in SECURITY_HEADERS {
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    out.extend_from_slice(b"Cache-Control: no-store\r\nConnection: close\r\n\r\n");
    out.extend_from_slice(body);
    out
}
fn status_text(code: u16) -> &'static str {
    match code {
        302 => "Found",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Content Too Large",
        417 => "Expectation Failed",
        421 => "Misdirected Request",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    }
}

// ---------------------------------------------------------------- bridge

/// One accepted bridge connection. `sock` is a clone of the client socket
/// used only to shut it down (eviction / stop).
struct ConnSlot {
    sock: TcpStream,
    /// The complete request head was received.
    head_done: bool,
    /// Presented a valid cookie or one-time token (never evicted).
    authed: bool,
}
#[derive(Default)]
struct ConnTable {
    next_id: u64,
    /// Keyed by a monotonic id, i.e. ordered oldest first.
    slots: BTreeMap<u64, ConnSlot>,
}
struct BridgeCtx {
    shared: Arc<SessionShared>,
    port: u16,
    cookie_name: String,
    open_token: String,
    open_used: AtomicBool,
    cookie_token: String,
    stop: Arc<AtomicBool>,
    /// Live connection threads (<= MAX_CONN_THREADS).
    active: AtomicUsize,
    /// Connection slots (<= MAX_CONNECTIONS, pre-auth <= MAX_PREAUTH).
    conns: Mutex<ConnTable>,
    idle_stop: Duration,
    cancel: Cancellation,
    reap: Mutex<Option<ReapHook>>,
}
struct Bridge {
    ctx: Arc<BridgeCtx>,
    acceptor: Option<thread::JoinHandle<()>>,
    port: u16,
}
impl Bridge {
    fn start(
        shared: Arc<SessionShared>,
        cancel: Cancellation,
        service_id: &str,
        idle_stop_ms: u64,
        reap: ReapHook,
    ) -> Result<(Self, String), PreviewError> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| PreviewError::Io)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| PreviewError::Io)?;
        let port = listener.local_addr().map_err(|_| PreviewError::Io)?.port();
        let open_token = nonce().map_err(|_| PreviewError::Io)?;
        let stop = Arc::new(AtomicBool::new(false));
        let ctx = Arc::new(BridgeCtx {
            shared,
            port,
            cookie_name: format!("pai_preview_{}", &service_id[..16.min(service_id.len())]),
            open_token: open_token.clone(),
            open_used: AtomicBool::new(false),
            cookie_token: nonce().map_err(|_| PreviewError::Io)?,
            stop,
            active: AtomicUsize::new(0),
            idle_stop: Duration::from_millis(idle_stop_ms),
            cancel,
            conns: Mutex::new(ConnTable::default()),
            reap: Mutex::new(Some(reap)),
        });
        let acceptor_ctx = ctx.clone();
        let acceptor = thread::Builder::new()
            .name("pai-preview-bridge".into())
            .spawn(move || accept_main(listener, acceptor_ctx))
            .map_err(|_| PreviewError::Io)?;
        Ok((
            Self {
                ctx,
                acceptor: Some(acceptor),
                port,
            },
            format!("http://127.0.0.1:{port}{OPEN_PATH}?t={open_token}"),
        ))
    }
    /// Stop accepting (the joined acceptor drops the listener), shut every
    /// pre-auth connection down and stop reading from authenticated ones
    /// (an in-flight upstream exchange may still answer). True when closed.
    fn close_listener(&mut self) -> bool {
        self.ctx.stop.store(true, Ordering::SeqCst);
        let joined = self.acceptor.take().is_none_or(|a| a.join().is_ok());
        self.ctx.shutdown_conns(false);
        joined
    }
    /// Shut every remaining connection down and wait (bounded) for all
    /// connection threads to exit; true when none is left.
    fn drain(self) -> bool {
        self.ctx.shutdown_conns(true);
        let deadline = Instant::now() + CONN_DRAIN_WAIT;
        while self.ctx.active.load(Ordering::SeqCst) > 0 {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
        true
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.ctx.stop.store(true, Ordering::SeqCst);
        if let Some(a) = self.acceptor.take() {
            let _ = a.join();
        }
        self.ctx.shutdown_conns(true);
    }
}
impl BridgeCtx {
    /// Admission of a new connection: its slot id, or None (503). Pre-auth
    /// connections may hold at most MAX_PREAUTH slots. When that share, or
    /// the whole table, is full the OLDEST pre-auth connection (preferring
    /// one still sending its head) is shut down to make room, so
    /// unauthenticated clients can neither refuse a newcomer nor touch the
    /// slots reserved for authenticated exchanges. Returns whether a
    /// connection was evicted.
    fn admit(&self, stream: &TcpStream) -> (Option<u64>, bool) {
        let Ok(sock) = stream.try_clone() else {
            return (None, false);
        };
        let mut t = lock(&self.conns);
        let preauth = t.slots.values().filter(|c| !c.authed).count();
        let mut evicted = false;
        if preauth >= MAX_PREAUTH || t.slots.len() >= MAX_CONNECTIONS {
            let victim = t
                .slots
                .iter()
                .find(|(_, c)| !c.authed && !c.head_done)
                .or_else(|| t.slots.iter().find(|(_, c)| !c.authed))
                .map(|(id, _)| *id);
            if let Some(c) = victim.and_then(|id| t.slots.remove(&id)) {
                let _ = c.sock.shutdown(Shutdown::Both);
                evicted = true;
            }
        }
        if t.slots.len() >= MAX_CONNECTIONS
            || self.active.load(Ordering::SeqCst) >= MAX_CONN_THREADS
        {
            return (None, evicted);
        }
        let id = t.next_id;
        t.next_id += 1;
        t.slots.insert(
            id,
            ConnSlot {
                sock,
                head_done: false,
                authed: false,
            },
        );
        self.active.fetch_add(1, Ordering::SeqCst);
        (Some(id), evicted)
    }
    fn head_done(&self, id: u64) {
        if let Some(c) = lock(&self.conns).slots.get_mut(&id) {
            c.head_done = true;
        }
    }
    /// Move a connection into the authenticated share (never evicted).
    /// False when it was evicted meanwhile or the bridge is stopping.
    fn promote(&self, id: u64) -> bool {
        if self.stop.load(Ordering::SeqCst) {
            return false;
        }
        match lock(&self.conns).slots.get_mut(&id) {
            Some(c) => {
                c.authed = true;
                true
            }
            None => false,
        }
    }
    fn release(&self, id: u64) {
        lock(&self.conns).slots.remove(&id);
    }
    /// `all`: shut every connection down; otherwise pre-auth connections
    /// fully and authenticated ones for reading only.
    fn shutdown_conns(&self, all: bool) {
        for c in lock(&self.conns).slots.values() {
            let how = if all || !c.authed {
                Shutdown::Both
            } else {
                Shutdown::Read
            };
            let _ = c.sock.shutdown(how);
        }
    }
}
fn accept_main(listener: TcpListener, ctx: Arc<BridgeCtx>) {
    loop {
        if ctx.stop.load(Ordering::SeqCst) {
            return;
        }
        if lock(&ctx.shared.last_activity).elapsed() >= ctx.idle_stop {
            ctx.shared.idle_stopped.store(true, Ordering::SeqCst);
            ctx.cancel.cancel();
            // Reap on another thread: stopping the session joins this one.
            if let Some(hook) = lock(&ctx.reap).take() {
                let _ = thread::Builder::new()
                    .name("pai-preview-reap".into())
                    .spawn(move || reap_idle(hook));
            }
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let (slot, evicted) = ctx.admit(&stream);
                if evicted {
                    ctx.shared.record(RequestRecord {
                        seq: 0,
                        at_ms: now_ms(),
                        method: "-".into(),
                        path: "-".into(),
                        status: None,
                        response_bytes: 0,
                        elapsed_ms: 0,
                        outcome: RequestOutcome::Rejected {
                            reason: RejectReason::TooManyConnections,
                        },
                        source: RequestSource::Bridge,
                    });
                }
                let Some(id) = slot else {
                    reject_fast(&ctx, stream);
                    continue;
                };
                let worker = ctx.clone();
                let spawned = thread::Builder::new()
                    .name("pai-preview-conn".into())
                    .spawn(move || {
                        handle_conn(&worker, stream, id);
                        worker.release(id);
                        worker.active.fetch_sub(1, Ordering::SeqCst);
                    });
                if spawned.is_err() {
                    ctx.release(id);
                    ctx.active.fetch_sub(1, Ordering::SeqCst);
                }
            }
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
}
/// Over the connection cap: answer 503 from the acceptor without blocking on
/// the client (drain only bytes already received so the close is a FIN, not
/// an RST that would destroy the 503 before the client reads it).
fn reject_fast(ctx: &BridgeCtx, mut stream: TcpStream) {
    let drain = |s: &mut TcpStream| {
        let _ = s.set_nonblocking(true);
        let mut sink = [0u8; 8192];
        for _ in 0..8 {
            if !matches!(s.read(&mut sink), Ok(n) if n > 0) {
                break;
            }
        }
    };
    drain(&mut stream);
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let _ = stream.write_all(&simple_response(503, status_text(503), &[], b"busy\n"));
    let _ = stream.shutdown(std::net::Shutdown::Write);
    drain(&mut stream);
    ctx.shared.record(RequestRecord {
        seq: 0,
        at_ms: now_ms(),
        method: "-".into(),
        path: "-".into(),
        status: Some(503),
        response_bytes: 0,
        elapsed_ms: 0,
        outcome: RequestOutcome::Rejected {
            reason: RejectReason::TooManyConnections,
        },
        source: RequestSource::Bridge,
    });
}
struct Exchange<'a> {
    ctx: &'a BridgeCtx,
    client: TcpStream,
    /// Connection slot id (see `BridgeCtx::admit`).
    conn: u64,
    t0: Instant,
    method: String,
    path: String,
}
impl Exchange<'_> {
    fn log(&self, status: Option<u16>, response_bytes: u64, outcome: RequestOutcome) {
        self.ctx.shared.record(RequestRecord {
            seq: 0,
            at_ms: now_ms(),
            method: self.method.clone(),
            path: self.path.clone(),
            status,
            response_bytes,
            elapsed_ms: self.t0.elapsed().as_millis() as u64,
            outcome,
            source: RequestSource::Bridge,
        });
    }
    fn reject(&mut self, code: u16, reason: RejectReason) {
        let body = format!("{} ({reason:?})\n", status_text(code));
        let _ = self.client.write_all(&simple_response(
            code,
            status_text(code),
            &[],
            body.as_bytes(),
        ));
        self.log(Some(code), 0, RequestOutcome::Rejected { reason });
    }
}
/// Lingering close: half-close, then discard what the client still sends
/// (bounded in bytes and time) so a rejected request's unread body cannot turn
/// the close into a TCP RST that destroys the response before it is read.
impl Drop for Exchange<'_> {
    fn drop(&mut self) {
        let _ = self.client.shutdown(std::net::Shutdown::Write);
        let _ = self.client.set_read_timeout(Some(LINGER_STEP));
        let deadline = Instant::now() + LINGER_MAX;
        let mut drained = 0usize;
        let mut sink = [0u8; 8192];
        while drained < LINGER_BYTES && Instant::now() < deadline {
            match self.client.read(&mut sink) {
                Ok(0) => break,
                Ok(n) => drained += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    }
}
/// Request-log method: one of LOGGABLE_METHODS verbatim, otherwise "OTHER"
/// (never client-chosen text, which could carry a secret).
fn log_method(method: &str) -> String {
    LOGGABLE_METHODS
        .into_iter()
        .find(|m| *m == method)
        .unwrap_or("OTHER")
        .to_string()
}
/// Replace every ASCII-case-insensitive occurrence of the ASCII `needle`.
fn replace_ci(hay: &str, needle: &str, with: &str) -> String {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() || !n.is_ascii() {
        return hay.to_string();
    }
    let mut out = String::with_capacity(hay.len());
    let (mut last, mut i) = (0, 0);
    while i + n.len() <= h.len() {
        // An ASCII match starts and ends on char boundaries.
        if h[i..i + n.len()].eq_ignore_ascii_case(n) {
            out.push_str(&hay[last..i]);
            out.push_str(with);
            i += n.len();
            last = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&hay[last..]);
    out
}
/// Before every client read: stop / absolute-deadline checks and a read
/// timeout of at most CLIENT_TIMEOUT that never reaches past `deadline`.
fn arm_client_read(
    ctx: &BridgeCtx,
    client: &mut TcpStream,
    deadline: Instant,
) -> Result<(), HeadError> {
    if ctx.stop.load(Ordering::SeqCst) {
        return Err(HeadError::Stopped);
    }
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(HeadError::Timeout);
    }
    client
        .set_read_timeout(Some(left.min(CLIENT_TIMEOUT)))
        .map_err(|_| HeadError::Closed)
}
impl BridgeCtx {
    fn redact(&self, path: &str) -> String {
        let mut p = if path == OPEN_PATH || path.starts_with(&format!("{OPEN_PATH}?")) {
            format!("{OPEN_PATH}?t=[redacted]")
        } else {
            replace_ci(
                &replace_ci(path, &self.open_token, "[redacted]"),
                &self.cookie_token,
                "[redacted]",
            )
        };
        if p.len() > PATH_LOG_MAX {
            let mut cut = PATH_LOG_MAX;
            while !p.is_char_boundary(cut) {
                cut -= 1;
            }
            p.truncate(cut);
        }
        p
    }
    /// Either capability value anywhere in `bytes`, ASCII case-insensitively
    /// (both are lowercase hex).
    fn contains_token(&self, bytes: &[u8]) -> bool {
        [&self.open_token, &self.cookie_token].iter().any(|t| {
            bytes
                .windows(t.len())
                .any(|w| w.eq_ignore_ascii_case(t.as_bytes()))
        })
    }
    fn cookie_valid(&self, req: &ParsedRequest) -> bool {
        req.values("cookie").any(|v| {
            String::from_utf8_lossy(v).split(';').any(|kv| {
                kv.trim().split_once('=').is_some_and(|(k, val)| {
                    k == self.cookie_name && ct_eq(val.as_bytes(), self.cookie_token.as_bytes())
                })
            })
        })
    }
    fn host_ok(&self, req: &ParsedRequest) -> bool {
        let hosts: Vec<&[u8]> = req.values("host").collect();
        if hosts.len() != 1 {
            return false;
        }
        let host = String::from_utf8_lossy(hosts[0]).to_ascii_lowercase();
        host == format!("127.0.0.1:{}", self.port) || host == format!("localhost:{}", self.port)
    }
}
fn handle_conn(ctx: &BridgeCtx, client: TcpStream, conn: u64) {
    if ctx.stop.load(Ordering::SeqCst) {
        return;
    }
    let _ = client.set_nonblocking(false);
    let _ = client.set_write_timeout(Some(Duration::from_secs(10)));
    let mut x = Exchange {
        ctx,
        client,
        conn,
        t0: Instant::now(),
        method: "-".into(),
        path: "-".into(),
    };
    // Absolute from accept: trickling bytes never extends it.
    let deadline = x.t0 + HEAD_DEADLINE;
    let (head, rest) = match read_head_with(&mut x.client, HEAD_MAX, |c| {
        arm_client_read(ctx, c, deadline)
    }) {
        Ok(v) => v,
        Err(HeadError::TooLarge) => return x.reject(431, RejectReason::HeadTooLarge),
        Err(HeadError::Timeout) => return x.reject(408, RejectReason::ClientTimeout),
        Err(HeadError::Closed | HeadError::Stopped) => return,
    };
    if ctx.stop.load(Ordering::SeqCst) {
        return;
    }
    ctx.head_done(conn);
    let req = match parse_request(&head) {
        Ok(r) => r,
        Err(RejectReason::HeadTooLarge) => return x.reject(431, RejectReason::HeadTooLarge),
        Err(reason) => return x.reject(400, reason),
    };
    x.method = log_method(&req.method);
    x.path = ctx.redact(&req.path);
    if req.method == "CONNECT" {
        return x.reject(405, RejectReason::ConnectRejected);
    }
    if req.values("upgrade").next().is_some() || req.tokens("connection").contains("upgrade") {
        return x.reject(501, RejectReason::UpgradeRejected);
    }
    let te = req.tokens("transfer-encoding");
    if req.values("transfer-encoding").next().is_some() {
        return if te.contains("chunked") {
            x.reject(411, RejectReason::ChunkedRejected)
        } else {
            x.reject(501, RejectReason::TransferEncodingRejected)
        };
    }
    let lengths: Vec<&[u8]> = req.values("content-length").collect();
    let content_length = match lengths.as_slice() {
        [] => 0u64,
        [first, others @ ..] => {
            let parse = |v: &[u8]| -> Option<u64> {
                (!v.is_empty() && v.len() <= 19 && v.iter().all(u8::is_ascii_digit))
                    .then(|| String::from_utf8_lossy(v).parse().ok())
                    .flatten()
            };
            match parse(first) {
                Some(n) if others.iter().all(|o| parse(o) == Some(n)) => n,
                _ => return x.reject(400, RejectReason::BadContentLength),
            }
        }
    };
    if content_length > BODY_MAX {
        return x.reject(413, RejectReason::BodyTooLarge);
    }
    if req.values("expect").next().is_some() {
        return x.reject(417, RejectReason::ExpectRejected);
    }
    if !req.path.starts_with('/') || req.path.starts_with("//") {
        return x.reject(400, RejectReason::AbsoluteFormRejected);
    }
    // DNS-rebinding defence before any capability decision.
    if !ctx.host_ok(&req) {
        return x.reject(421, RejectReason::HostRejected);
    }
    if req.path == OPEN_PATH || req.path.starts_with(&format!("{OPEN_PATH}?")) {
        return open_capability(&mut x, &req);
    }
    if req.path.starts_with("/__pai/") {
        return x.reject(404, RejectReason::ReservedPath);
    }
    if !ctx.cookie_valid(&req) {
        return x.reject(403, RejectReason::CapabilityMissing);
    }
    // Authenticated: leave the pre-auth share (evicted meanwhile => gone).
    if !ctx.promote(x.conn) {
        return;
    }
    if ctx.contains_token(req.method.as_bytes()) || ctx.contains_token(req.path.as_bytes()) {
        return x.reject(400, RejectReason::TokenInRequest);
    }
    forward(&mut x, &req, rest, content_length);
}
/// One-time token -> per-session cookie. A reused token only works for a
/// browser that already holds the cookie.
fn open_capability(x: &mut Exchange<'_>, req: &ParsedRequest) {
    let ctx = x.ctx;
    if req.method != "GET" {
        return x.reject(405, RejectReason::MethodNotAllowed);
    }
    let token = req
        .path
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("t="))
        .unwrap_or("");
    let token_ok =
        ct_eq(token.as_bytes(), ctx.open_token.as_bytes()) && !ctx.open_used.load(Ordering::SeqCst);
    let cookie_ok = ctx.cookie_valid(req);
    if !token_ok && !cookie_ok {
        return x.reject(403, RejectReason::CapabilityMissing);
    }
    // Leave the pre-auth share BEFORE consuming the one-time token, so an
    // eviction can never burn it.
    if !ctx.promote(x.conn) {
        return;
    }
    let fresh = token_ok && !ctx.open_used.swap(true, Ordering::SeqCst);
    let mut extra = vec![("Location", "/".to_string())];
    if fresh {
        extra.push((
            "Set-Cookie",
            format!(
                "{}={}; HttpOnly; SameSite=Strict; Path=/",
                ctx.cookie_name, ctx.cookie_token
            ),
        ));
    } else if !cookie_ok {
        return x.reject(403, RejectReason::CapabilityMissing);
    }
    // Only authenticated traffic keeps the preview alive (idle stop).
    *lock(&ctx.shared.last_activity) = Instant::now();
    let _ = x
        .client
        .write_all(&simple_response(302, status_text(302), &extra, b""));
    x.log(Some(302), 0, RequestOutcome::Ok);
}
fn forward(x: &mut Exchange<'_>, req: &ParsedRequest, rest: Vec<u8>, content_length: u64) {
    let ctx = x.ctx;
    *lock(&ctx.shared.last_activity) = Instant::now();
    // Request body: exactly Content-Length bytes; pipelined extras are dropped.
    // Absolute deadline: a trickled body cannot hold the slot longer (408).
    let mut body = rest;
    body.truncate(content_length as usize);
    let deadline = Instant::now() + BODY_DEADLINE;
    let mut chunk = [0u8; 8192];
    while (body.len() as u64) < content_length {
        match arm_client_read(ctx, &mut x.client, deadline) {
            Ok(()) => {}
            Err(HeadError::Timeout) => return x.reject(408, RejectReason::ClientTimeout),
            Err(HeadError::Stopped) => return,
            Err(_) => return x.reject(400, RejectReason::BadContentLength),
        }
        let want = (content_length as usize - body.len()).min(chunk.len());
        match x.client.read(&mut chunk[..want]) {
            Ok(0) => return x.reject(400, RejectReason::BadContentLength),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return x.reject(408, RejectReason::ClientTimeout)
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return x.reject(400, RejectReason::BadContentLength),
        }
    }
    if ctx.stop.load(Ordering::SeqCst) {
        return;
    }
    let Some(mut tunnel) = ctx.shared.link.take_tunnel(TUNNEL_WAIT) else {
        return x.reject(503, RejectReason::PoolExhausted);
    };
    let connection_named = req.tokens("connection");
    let mut up = format!("{} {} HTTP/1.1\r\n", req.method, req.path).into_bytes();
    for (name, value) in &req.headers {
        let lname = name.to_ascii_lowercase();
        if HOP_BY_HOP.contains(&lname.as_str())
            || connection_named.contains(&lname)
            || lname == "content-length"
        {
            continue;
        }
        let value: Vec<u8> = if lname == "cookie" {
            let kept: Vec<String> = String::from_utf8_lossy(value)
                .split(';')
                .map(|kv| kv.trim().to_string())
                .filter(|kv| !kv.is_empty() && !kv.starts_with("pai_preview_"))
                .collect();
            if kept.is_empty() {
                continue;
            }
            kept.join("; ").into_bytes()
        } else {
            value.clone()
        };
        // Never forward a capability value in any header name or value
        // (e.g. Referer), in any letter case.
        if ctx.contains_token(name.as_bytes()) || ctx.contains_token(&value) {
            continue;
        }
        up.extend_from_slice(name.as_bytes());
        up.extend_from_slice(b": ");
        up.extend_from_slice(&value);
        up.extend_from_slice(b"\r\n");
    }
    if content_length > 0 || req.method == "POST" || req.method == "PUT" {
        up.extend_from_slice(format!("Content-Length: {content_length}\r\n").as_bytes());
    }
    up.extend_from_slice(b"Connection: close\r\n\r\n");
    let _ = tunnel.set_timeouts(Some(UPSTREAM_HEAD_TIMEOUT), Some(Duration::from_secs(10)));
    if tunnel
        .write_all(b"G")
        .and_then(|_| tunnel.write_all(&up))
        .and_then(|_| tunnel.write_all(&body))
        .is_err()
    {
        let _ = x.client.write_all(&simple_response(
            502,
            status_text(502),
            &[],
            b"upstream closed\n",
        ));
        return x.log(Some(502), 0, RequestOutcome::UpstreamClosed);
    }
    let (rhead, rrest) = match read_head(&mut tunnel, HEAD_MAX) {
        Ok(v) => v,
        Err(HeadError::Closed | HeadError::Stopped) => {
            let _ = x.client.write_all(&simple_response(
                502,
                status_text(502),
                &[],
                b"upstream closed\n",
            ));
            return x.log(Some(502), 0, RequestOutcome::UpstreamClosed);
        }
        Err(HeadError::Timeout) => {
            let _ = x.client.write_all(&simple_response(
                504,
                status_text(504),
                &[],
                b"upstream timeout\n",
            ));
            return x.log(Some(504), 0, RequestOutcome::Timeout);
        }
        Err(HeadError::TooLarge) => return x.reject(502, RejectReason::BadUpstreamResponse),
    };
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut resp = httparse::Response::new(&mut headers);
    let code = match resp.parse(&rhead) {
        Ok(httparse::Status::Complete(_)) => resp.code.unwrap_or(0),
        _ => 0,
    };
    if !(200..=599).contains(&code) {
        return x.reject(502, RejectReason::BadUpstreamResponse);
    }
    let Ok(declared) = response_framing(&req.method, code, resp.headers) else {
        return x.reject(502, RejectReason::BadUpstreamResponse);
    };
    let mut out = format!("HTTP/1.1 {code} {}\r\n", resp.reason.unwrap_or("")).into_bytes();
    for h in resp.headers.iter() {
        // One response per tunnel: the bridge owns connection management.
        // A server may never set (or clobber) a bridge capability cookie.
        let lname = h.name.to_ascii_lowercase();
        if matches!(
            lname.as_str(),
            "connection" | "keep-alive" | "proxy-connection"
        ) || (lname == "set-cookie" && is_bridge_cookie(h.value))
        {
            continue;
        }
        out.extend_from_slice(h.name.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(h.value);
        out.extend_from_slice(b"\r\n");
    }
    for (k, v) in SECURITY_HEADERS {
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    out.extend_from_slice(b"Connection: close\r\n\r\n");
    if x.client.write_all(&out).is_err() {
        return x.log(Some(code), 0, RequestOutcome::Ok);
    }
    let mut sent = 0u64;
    let mut outcome = RequestOutcome::Ok;
    let mut pending = rrest;
    let _ = tunnel.set_timeouts(Some(UPSTREAM_BODY_TIMEOUT), Some(Duration::from_secs(10)));
    let mut chunk = vec![0u8; 16 * 1024];
    // A Content-Length body ends after its bytes even if the server keeps the
    // connection open; otherwise the body runs until the upstream closes.
    let limit = declared.map_or(RESPONSE_MAX, |n| n.min(RESPONSE_MAX));
    loop {
        if !pending.is_empty() {
            let take = pending.len().min((limit - sent) as usize);
            if x.client.write_all(&pending[..take]).is_err() {
                break;
            }
            sent += take as u64;
            if take < pending.len() && declared.is_none() {
                outcome = RequestOutcome::ResponseTruncated;
                break;
            }
            pending.clear();
        }
        if declared.is_some() && sent >= limit {
            if declared.is_some_and(|n| n > RESPONSE_MAX) {
                outcome = RequestOutcome::ResponseTruncated;
            }
            break;
        }
        match tunnel.read(&mut chunk) {
            Ok(0) => {
                if declared.is_some() {
                    outcome = RequestOutcome::UpstreamClosed;
                }
                break;
            }
            Ok(n) => pending.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                outcome = RequestOutcome::Timeout;
                break;
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    x.log(Some(code), sent, outcome);
}

// ------------------------------------------------- internal client + checks

struct InternalResponse {
    status: u16,
    content_type: Option<String>,
    body: Vec<u8>,
    truncated: bool,
}
#[derive(Debug)]
enum InternalError {
    NoTunnel,
    Closed,
    Timeout,
    BadResponse,
}
/// Host-internal request through a tunnel (no capability check: readiness and
/// product HTTP checks only, never reachable from the bridge listener).
fn internal_request(
    link: &ServiceLink,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    wait: Duration,
) -> Result<InternalResponse, InternalError> {
    let mut tunnel: Tunnel = link.take_tunnel(wait).ok_or(InternalError::NoTunnel)?;
    let timeout = wait
        .max(Duration::from_millis(200))
        .min(UPSTREAM_HEAD_TIMEOUT);
    let _ = tunnel.set_timeouts(Some(timeout), Some(timeout));
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUser-Agent: unoone-preview-check/1\r\nAccept: */*\r\nConnection: close\r\n",
        link.listen_port()
    )
    .into_bytes();
    if let Some(b) = body {
        let ct = if serde_json::from_slice::<serde_json::Value>(b).is_ok() {
            "application/json"
        } else {
            "text/plain; charset=utf-8"
        };
        req.extend_from_slice(
            format!("Content-Type: {ct}\r\nContent-Length: {}\r\n", b.len()).as_bytes(),
        );
    }
    req.extend_from_slice(b"\r\n");
    req.extend_from_slice(body.unwrap_or_default());
    tunnel
        .write_all(b"G")
        .and_then(|_| tunnel.write_all(&req))
        .map_err(|_| InternalError::Closed)?;
    let (head, mut rest) = read_head(&mut tunnel, HEAD_MAX).map_err(|e| match e {
        HeadError::Timeout => InternalError::Timeout,
        HeadError::Closed | HeadError::Stopped => InternalError::Closed,
        HeadError::TooLarge => InternalError::BadResponse,
    })?;
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut resp = httparse::Response::new(&mut headers);
    let status = match resp.parse(&head) {
        Ok(httparse::Status::Complete(_)) => resp.code.ok_or(InternalError::BadResponse)?,
        _ => return Err(InternalError::BadResponse),
    };
    let content_type = resp
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| String::from_utf8_lossy(h.value).into_owned());
    let declared =
        response_framing(method, status, resp.headers).map_err(|_| InternalError::BadResponse)?;
    let mut chunk = [0u8; 8192];
    loop {
        if let Some(n) = declared {
            if rest.len() as u64 >= n {
                rest.truncate(n as usize);
                break;
            }
        }
        if rest.len() > CHECK_BODY_MAX {
            break;
        }
        match tunnel.read(&mut chunk) {
            Ok(0) if declared.is_some() => return Err(InternalError::Closed),
            Ok(0) => break,
            Ok(n) => rest.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Err(InternalError::Timeout),
        }
    }
    let truncated = rest.len() > CHECK_BODY_MAX;
    rest.truncate(CHECK_BODY_MAX);
    Ok(InternalResponse {
        status,
        content_type,
        body: rest,
        truncated,
    })
}
fn excerpt(body: &[u8]) -> String {
    let mut s = String::from_utf8_lossy(&body[..body.len().min(1024)]).into_owned();
    while s.len() > 1024 {
        s.pop();
    }
    s
}
fn run_check(shared: &SessionShared, c: &HttpCheck) -> HttpCheckResult {
    *lock(&shared.last_activity) = Instant::now();
    let t0 = Instant::now();
    let r = internal_request(
        &shared.link,
        c.method.as_str(),
        &c.path,
        c.body.as_deref().map(str::as_bytes),
        UPSTREAM_HEAD_TIMEOUT,
    );
    let elapsed_ms = t0.elapsed().as_millis() as u64;
    let (result, record_status, bytes, outcome) = match r {
        Err(e) => (
            HttpCheckResult {
                id: c.id.clone(),
                status: None,
                passed: false,
                body_sha256: None,
                excerpt: String::new(),
                elapsed_ms,
                failure: Some(format!("no response: {e:?}")),
            },
            None,
            0,
            match e {
                InternalError::Timeout => RequestOutcome::Timeout,
                InternalError::NoTunnel => RequestOutcome::Rejected {
                    reason: RejectReason::PoolExhausted,
                },
                _ => RequestOutcome::UpstreamClosed,
            },
        ),
        Ok(resp) => {
            let failure = check_failure(c, &resp);
            (
                HttpCheckResult {
                    id: c.id.clone(),
                    status: Some(resp.status),
                    passed: failure.is_none(),
                    body_sha256: Some(hash(&resp.body)),
                    excerpt: excerpt(&resp.body),
                    elapsed_ms,
                    failure,
                },
                Some(resp.status),
                resp.body.len() as u64,
                if resp.truncated {
                    RequestOutcome::ResponseTruncated
                } else {
                    RequestOutcome::Ok
                },
            )
        }
    };
    shared.record(RequestRecord {
        seq: 0,
        at_ms: now_ms(),
        method: c.method.as_str().into(),
        path: c.path.clone(),
        status: record_status,
        response_bytes: bytes,
        elapsed_ms,
        outcome,
        source: RequestSource::HttpCheck,
    });
    result
}
fn check_failure(c: &HttpCheck, r: &InternalResponse) -> Option<String> {
    if !(c.expect_status.0..=c.expect_status.1).contains(&r.status) {
        return Some(format!(
            "status {} not in {}..={}",
            r.status, c.expect_status.0, c.expect_status.1
        ));
    }
    if r.truncated {
        return Some("response body exceeded the 1 MiB check bound".into());
    }
    if let Some(prefix) = &c.expect_content_type_prefix {
        let ok = r.content_type.as_ref().is_some_and(|ct| {
            ct.to_ascii_lowercase()
                .starts_with(&prefix.to_ascii_lowercase())
        });
        if !ok {
            return Some(format!("content-type does not start with {prefix:?}"));
        }
    }
    for needle in &c.expect_body_contains {
        if !needle.is_empty() && !r.body.windows(needle.len()).any(|w| w == needle.as_bytes()) {
            return Some(format!("body does not contain {needle:?}"));
        }
    }
    if let Some(expected) = &c.expect_json_equals {
        match serde_json::from_slice::<serde_json::Value>(&r.body) {
            Ok(v) if &v == expected => {}
            Ok(_) => return Some("JSON body differs from the expected value".into()),
            Err(_) => return Some("body is not JSON".into()),
        }
    }
    None
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    //! Pure unit tests (no sandbox; compile on every target).
    use super::*;

    #[test]
    fn log_ring_unit_cap_eviction_and_cursor_metadata() {
        let mut ring = LogRing::with_cap(100);
        assert_eq!(LogRing::new().cap_bytes(), LOG_RING_BYTES);
        assert_eq!(
            LogRing::with_cap(10 * LOG_RING_BYTES).cap_bytes(),
            LOG_RING_BYTES
        );
        for i in 0..3u8 {
            ring.push(Stream::Stdout, vec![b'a' + i; 40], 1);
        }
        // 120 > 100: the oldest record is evicted, metadata says so.
        assert_eq!(ring.retained_bytes(), 80);
        assert_eq!(
            (
                ring.dropped_bytes,
                ring.dropped_records,
                ring.first_retained_seq
            ),
            (40, 1, 1)
        );
        let c = ring.chunk(0, 10);
        assert!(c.truncated_before_cursor);
        assert_eq!(
            c.records.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!((c.next_cursor, c.retained_bytes, c.cap_bytes), (3, 80, 100));
        let c = ring.chunk(1, 1);
        assert!(!c.truncated_before_cursor);
        assert_eq!((c.records.len(), c.next_cursor), (1, 2));
        assert!(ring.chunk(3, 10).records.is_empty());
        assert_eq!(ring.chunk(3, 10).next_cursor, 3);
        // An oversize record keeps only its newest `cap` bytes.
        let mut big = vec![b'x'; 50];
        big.extend(vec![b'y'; 100]);
        ring.push(Stream::Stderr, big, 2);
        assert_eq!(ring.retained_bytes(), 100);
        let c = ring.chunk(0, 10);
        assert_eq!(c.records.len(), 1);
        assert_eq!(c.records[0].bytes, vec![b'y'; 100]);
        assert_eq!(c.records[0].seq, 3);
        assert_eq!(c.dropped_bytes, 40 + 50 + 80);
        assert_eq!(c.dropped_records, 3);
        // Serialized log records are hex (no raw control bytes in JSON).
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(&"79".repeat(100)));
        let back: LogChunk = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn request_parsing_and_helpers() {
        assert!(
            matches!(parse_request(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n"), Ok(r) if r.method == "GET" && r.path == "/")
        );
        assert_eq!(
            parse_request(b"GET / HTTP/1.1\r\nBad Header\r\n\r\n").err(),
            Some(RejectReason::Malformed)
        );
        let many: String = (0..80).map(|i| format!("X-{i}: v\r\n")).collect();
        let head = format!("GET / HTTP/1.1\r\n{many}\r\n");
        assert_eq!(
            parse_request(head.as_bytes()).err(),
            Some(RejectReason::HeadTooLarge)
        );
        assert!(is_bridge_cookie(b"pai_preview_0123=abc; Path=/"));
        assert!(is_bridge_cookie(b"  PAI_PREVIEW_x=1"));
        assert!(!is_bridge_cookie(b"site=pai_preview_x"));
        assert!(
            valid_path("/api/trees?x=1") && !valid_path("//evil") && !valid_path("/__pai/open")
        );
        assert!(
            !valid_path("relative")
                && !valid_path("/a b")
                && !valid_path(&format!("/{}", "a".repeat(600)))
        );
        assert_eq!(
            TaskId::parse(&"a".repeat(32)).map(|t| t.as_str().len()),
            Ok(32)
        );
        assert!(TaskId::parse("ABC").is_err() && TaskId::parse(&"g".repeat(32)).is_err());
        let mut r = &b"GET / HTTP/1.1\r\n\r\nrest"[..];
        assert_eq!(
            read_head(&mut r, HEAD_MAX),
            Ok((b"GET / HTTP/1.1\r\n\r\n".to_vec(), b"rest".to_vec()))
        );
        let big = vec![b'a'; HEAD_MAX + 4096];
        assert_eq!(read_head(&mut &big[..], HEAD_MAX), Err(HeadError::TooLarge));
        // Regression: a complete head just over the bound must not slip
        // through because its terminator arrived in the last 4 KiB chunk.
        let sized = |n: usize| {
            let mut h = b"GET / HTTP/1.1\r\nX-Pad: ".to_vec();
            h.resize(n - 4, b'p');
            h.extend_from_slice(b"\r\n\r\n");
            h
        };
        for over in [1, 100, 4000] {
            assert_eq!(
                read_head(&mut &sized(HEAD_MAX + over)[..], HEAD_MAX),
                Err(HeadError::TooLarge),
                "{over}"
            );
        }
        let exact = sized(HEAD_MAX);
        assert_eq!(
            read_head(&mut &exact[..], HEAD_MAX).map(|(h, r)| (h.len(), r.len())),
            Ok((HEAD_MAX, 0))
        );
        assert_eq!(
            read_head(&mut &b"GET /"[..], HEAD_MAX),
            Err(HeadError::Closed)
        );
    }

    #[test]
    fn read_head_with_enforces_absolute_deadline_and_stop() {
        /// One byte per read, forever (a slowloris peer that never ends its head).
        struct Trickle;
        impl Read for Trickle {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                thread::sleep(Duration::from_millis(1));
                b[0] = b'a';
                Ok(1)
            }
        }
        // Each read succeeds, so only the armed deadline can end it.
        let t0 = Instant::now();
        let deadline = t0 + Duration::from_millis(150);
        let mut reads = 0usize;
        let r = read_head_with(&mut Trickle, HEAD_MAX, |_| {
            reads += 1;
            if Instant::now() >= deadline {
                Err(HeadError::Timeout)
            } else {
                Ok(())
            }
        });
        assert_eq!(r, Err(HeadError::Timeout));
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
        assert!(reads > 1 && reads < HEAD_MAX, "{reads}");
        assert_eq!(
            read_head_with(&mut Trickle, HEAD_MAX, |_| Err(HeadError::Stopped)),
            Err(HeadError::Stopped)
        );
        // The arm runs before every read, also for a head split over reads.
        let mut calls = 0;
        let mut split = std::io::Read::chain(&b"GET / HT"[..], &b"TP/1.1\r\n\r\n"[..]);
        assert!(read_head_with(&mut split, HEAD_MAX, |_| {
            calls += 1;
            Ok(())
        })
        .is_ok());
        assert!(calls >= 2, "{calls}");
    }

    #[test]
    fn request_log_method_allowlist_and_case_insensitive_redaction() {
        for m in LOGGABLE_METHODS {
            assert_eq!(log_method(m), m);
        }
        for m in [
            "get",
            "PROPFIND",
            "2369155376D49C8D2369155376D49C8D",
            "",
            "GETX",
        ] {
            assert_eq!(log_method(m), "OTHER", "{m}");
        }
        let tok = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            replace_ci(
                &format!(
                    "/x/{}/y?t={tok}&u={}",
                    tok.to_ascii_uppercase(),
                    "0123456789ABCdef0123456789abcDEF"
                ),
                tok,
                "[redacted]"
            ),
            "/x/[redacted]/y?t=[redacted]&u=[redacted]"
        );
        assert_eq!(replace_ci("é/ab/é", "ab", "[r]"), "é/[r]/é");
        assert_eq!(replace_ci("/plain", tok, "[r]"), "/plain");
    }

    #[test]
    fn source_scan_no_process_spawning_in_preview() {
        // Command construction lives ONLY in isolation/workspace.rs (§7).
        let src = include_str!("task_preview.rs");
        for needle in [
            ["Command", "::new("].concat(),
            ["process", "::Command"].concat(),
            ["share", "-net"].concat(),
        ] {
            assert!(!src.contains(&needle), "{needle}");
        }
        assert!(src.contains("HttpOnly; SameSite=Strict; Path=/"));
    }

    #[test]
    fn preview_spec_validation_bounds() {
        let spec = |f: &dyn Fn(&mut PreviewSpec)| {
            let mut s = PreviewSpec {
                service: crate::isolation::workspace::ServiceSpec {
                    schema: crate::isolation::workspace::SERVICE_SPEC_SCHEMA.into(),
                    command: crate::isolation::workspace::ServiceCommand::PythonStaticServer {
                        dir: String::new(),
                    },
                    listen_port: 8000,
                    tunnels_pool: 2,
                    tunnels_max: 4,
                    limits: crate::isolation::workspace::WorkspaceLimits {
                        cpu_seconds: 60,
                        memory_bytes: 256 << 20,
                        processes: 32,
                        work_tmpfs_bytes: 16 << 20,
                        file_size_bytes: 1 << 20,
                        open_files: 64,
                        total_timeout_ms: 120_000,
                        rss_watchdog_bytes: 1 << 30,
                    },
                    log_rate_bytes_per_s: 65_536,
                },
                readiness_path: "/".into(),
                ready_status: (200, 399),
                startup_timeout_ms: 10_000,
                idle_stop_ms: 600_000,
                http_checks: Vec::new(),
            };
            f(&mut s);
            s.validate()
        };
        let check = |id: &str| HttpCheck {
            id: id.into(),
            method: HttpMethod::Get,
            path: "/".into(),
            body: None,
            expect_status: (200, 200),
            expect_content_type_prefix: None,
            expect_body_contains: Vec::new(),
            expect_json_equals: None,
        };
        assert_eq!(spec(&|_| {}), Ok(()));
        assert_eq!(
            spec(&|s| s.startup_timeout_ms = 100),
            Err(PreviewError::InvalidSpec)
        );
        assert_eq!(
            spec(&|s| s.startup_timeout_ms = 30_001),
            Err(PreviewError::InvalidSpec)
        );
        assert_eq!(
            spec(&|s| s.ready_status = (400, 300)),
            Err(PreviewError::InvalidSpec)
        );
        assert_eq!(
            spec(&|s| s.readiness_path = "/__pai/open".into()),
            Err(PreviewError::InvalidSpec)
        );
        assert_eq!(
            spec(&|s| s.http_checks = (0..17).map(|i| check(&format!("c{i}"))).collect()),
            Err(PreviewError::InvalidSpec)
        );
        assert_eq!(
            spec(&|s| s.http_checks = vec![check("dup"), check("dup")]),
            Err(PreviewError::InvalidSpec)
        );
        assert_eq!(
            spec(&|s| s.http_checks = vec![check("bad id")]),
            Err(PreviewError::InvalidSpec)
        );
        assert_eq!(
            spec(&|s| {
                let mut c = check("get-body");
                c.body = Some("x".into());
                s.http_checks = vec![c];
            }),
            Err(PreviewError::InvalidSpec)
        );
        assert_eq!(
            spec(&|s| {
                let mut c = check("post-big");
                c.method = HttpMethod::Post;
                c.body = Some("x".repeat(4097));
                s.http_checks = vec![c];
            }),
            Err(PreviewError::InvalidSpec)
        );
        assert_eq!(
            spec(&|s| {
                let mut c = check("needles");
                c.expect_body_contains = vec!["n".into(); 9];
                s.http_checks = vec![c];
            }),
            Err(PreviewError::InvalidSpec)
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    //! Real bwrap, serial (`--test-threads=1`). The dev server runs ONLY
    //! through `WorkspaceIsolation::start_service` via `PreviewManager`; the
    //! host-side test client speaks raw HTTP/1.1 to the bridge listener.
    //! Fixtures differ from the held-out suite (/api/trees site).
    use super::*;
    use crate::isolation::workspace::{
        ServiceCommand, TreeLimits, WorkspaceLimits, SERVICE_SPEC_SCHEMA,
    };
    use crate::isolation::LinuxIsolation;
    use crate::task_ledger::BootInfo;
    use std::net::Shutdown;
    use std::sync::atomic::AtomicU64;

    /// Lock-epoch guard over a test-owned counter (owner A's `BootInfo::guard`).
    fn guard(epoch: &Arc<AtomicU64>) -> EpochGuard {
        BootInfo::new(epoch.clone()).guard()
    }

    const MIB: u64 = 1024 * 1024;
    const SITE: &[u8] = br#"import http.server,json,os,sys,socket,errno,time
port=int(sys.argv[1]); mode=sys.argv[2] if len(sys.argv)>2 else 'ok'
hostport=int(sys.argv[3]) if len(sys.argv)>3 else 0
if mode=='exit3':
    print('exiting with 3',flush=True); sys.exit(3)
if mode=='never':
    print('never listening',flush=True)
    while True: time.sleep(1)
info={}
if hostport:
    s=socket.socket()
    try:
        s.settimeout(2); s.connect(('127.0.0.1',hostport)); info['host_loopback']='connected'
    except OSError as e: info['host_loopback']=errno.errorcode.get(e.errno,str(e))
print('INFO '+json.dumps(info),flush=True)
if mode=='noisy':
    for i in range(40): sys.stdout.write('N%02d'%i+'n'*3996+'\n')
    sys.stdout.write('NOISE-END\n'); sys.stdout.flush()
TREES=[{'name':'oak'},{'name':'birch'},{'name':'cedar'}]
if os.path.exists('/work/variant.txt'): TREES=[{'name':open('/work/variant.txt').read().strip()}]
def probe(p):
    s=socket.socket()
    try:
        s.settimeout(2); s.connect(('127.0.0.1',p)); return 'connected'
    except OSError as e: return errno.errorcode.get(e.errno,str(e))
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,f,*a):
        sys.stderr.write('REQ %s %s\n'%(self.command,self.path)); sys.stderr.flush()
    def send(self,code,body,ct,extra=()):
        self.send_response(code); self.send_header('Content-Type',ct); self.send_header('Content-Length',str(len(body)))
        for k,v in extra: self.send_header(k,v)
        self.end_headers(); self.wfile.write(body)
    def do_GET(self):
        p=self.path
        if mode=='http500': return self.send(500,b'broken','text/plain')
        if p=='/api/trees': return self.send(200,json.dumps(TREES).encode(),'application/json')
        if p=='/headers':
            items=[[k,v] for k,v in self.headers.items()]
            sys.stderr.write('HDRS '+json.dumps(items)+'\n'); sys.stderr.flush()
            return self.send(200,json.dumps(items).encode(),'application/json')
        if p.startswith('/probe/'): return self.send(200,probe(int(p[7:])).encode(),'text/plain')
        if p=='/slow':
            time.sleep(4); return self.send(200,b'slow','text/plain')
        if p=='/big': return self.send(200,b'b'*(9*1024*1024),'application/octet-stream')
        if p=='/own-headers':
            return self.send(200,b'own','text/html',[('Content-Security-Policy',"script-src 'none'"),('X-Trees-Server','1'),('Set-Cookie','pai_preview_evil=x; Path=/'),('Set-Cookie','site=1; Path=/'),('Connection','keep-alive')])
        return self.send(200,b'<h1>Trees</h1>','text/html')
    def do_POST(self):
        n=int(self.headers.get('Content-Length','0')); self.send(201,self.rfile.read(n),'application/octet-stream')
http.server.HTTPServer(('127.0.0.1',port),H).serve_forever()
"#;

    fn backend() -> WorkspaceIsolation {
        let base = LinuxIsolation::new().expect("real Stage 4 isolation is a test precondition");
        WorkspaceIsolation::new(&base)
            .expect("real Stage 5 workspace isolation is a test precondition, never skip")
    }
    fn site_tree(scratch: &Path, variant: Option<&str>) -> StagedTree {
        let mut files = BTreeMap::new();
        files.insert("server.py".to_string(), SITE.to_vec());
        if let Some(v) = variant {
            files.insert("variant.txt".to_string(), v.as_bytes().to_vec());
        }
        StagedTree::from_files(scratch, &files, &TreeLimits::default()).unwrap()
    }
    fn spec(args: &[&str]) -> PreviewSpec {
        PreviewSpec::with_defaults(ServiceSpec {
            schema: SERVICE_SPEC_SCHEMA.into(),
            command: ServiceCommand::PythonScript {
                script: "server.py".into(),
                args: args.iter().map(|s| s.to_string()).collect(),
            },
            listen_port: 8000,
            tunnels_pool: 2,
            tunnels_max: 4,
            limits: WorkspaceLimits {
                cpu_seconds: 60,
                memory_bytes: 256 * MIB,
                processes: 32,
                work_tmpfs_bytes: 16 * MIB,
                file_size_bytes: MIB,
                open_files: 64,
                total_timeout_ms: 120_000,
                rss_watchdog_bytes: 1024 * MIB,
            },
            log_rate_bytes_per_s: 256 * 1024,
        })
    }
    fn marker() -> String {
        format!("pai-s5-preview-{}", nonce().unwrap())
    }
    /// Live (non-zombie) host processes whose cmdline mentions `marker`
    /// (bwrap, prlimit, supervisor and server all carry it in their argv).
    fn live_with(marker: &str) -> usize {
        let mut n = 0;
        for e in std::fs::read_dir("/proc").unwrap().flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if pid == std::process::id() {
                continue;
            }
            let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|s| {
                    s.rsplit_once(')')
                        .map(|(_, r)| r.trim_start().starts_with('Z'))
                })
                .unwrap_or(true);
            if let Ok(cmd) = std::fs::read(format!("/proc/{pid}/cmdline")) {
                if !zombie && cmd.windows(marker.len()).any(|w| w == marker.as_bytes()) {
                    n += 1;
                }
            }
        }
        n
    }
    fn wait_gone(marker: &str, wait: Duration) -> usize {
        let t = Instant::now();
        loop {
            let n = live_with(marker);
            if n == 0 || t.elapsed() > wait {
                return n;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    fn svc_dirs(scratch: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(scratch)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("pai-svc-"))
            })
            .collect();
        v.sort();
        v
    }

    #[derive(Debug)]
    struct Resp {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        elapsed: Duration,
    }
    impl Resp {
        fn all(&self, name: &str) -> Vec<&str> {
            self.headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
                .collect()
        }
        fn one(&self, name: &str) -> Option<&str> {
            match self.all(name).as_slice() {
                [v] => Some(*v),
                _ => None,
            }
        }
    }
    /// Raw HTTP/1.1 exchange with the bridge: write `req`, read to EOF.
    fn raw(port: u16, req: &[u8]) -> Resp {
        let t0 = Instant::now();
        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("bridge listening");
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        s.write_all(req).unwrap();
        let mut buf = Vec::new();
        let _ = s.read_to_end(&mut buf);
        let elapsed = t0.elapsed();
        let split = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or_else(|| panic!("no response head: {:?}", String::from_utf8_lossy(&buf)));
        let head = String::from_utf8_lossy(&buf[..split]).into_owned();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .unwrap()
            .split(' ')
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .filter_map(|l| {
                l.split_once(':')
                    .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            })
            .collect();
        Resp {
            status,
            headers,
            body: buf[split + 4..].to_vec(),
            elapsed,
        }
    }
    fn get(port: u16, path: &str, cookie: Option<&str>) -> Resp {
        let c = cookie
            .map(|c| format!("Cookie: {c}\r\n"))
            .unwrap_or_default();
        raw(
            port,
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{c}\r\n").as_bytes(),
        )
    }
    /// Validate the capability URL shape and exchange it for the cookie.
    fn url_parts(url: &str) -> (u16, String) {
        let rest = url.strip_prefix("http://127.0.0.1:").expect(url);
        let (port, q) = rest.split_once("/__pai/open?t=").expect(url);
        assert!(
            (1..=5).contains(&port.len()) && port.bytes().all(|b| b.is_ascii_digit()),
            "{url}"
        );
        assert!(
            q.len() == 32
                && q.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{url}"
        );
        (port.parse().unwrap(), q.to_string())
    }
    fn open_cookie(url: &str) -> (u16, String, String) {
        let (port, token) = url_parts(url);
        let r = get(port, &format!("/__pai/open?t={token}"), None);
        assert_eq!(r.status, 302, "{r:?}");
        assert_eq!(r.one("Location"), Some("/"));
        let sc = r.one("Set-Cookie").expect("one Set-Cookie").to_string();
        assert!(sc.ends_with("; HttpOnly; SameSite=Strict; Path=/"), "{sc}");
        let pair = sc.split(';').next().unwrap().to_string();
        assert!(pair.starts_with("pai_preview_"), "{pair}");
        (port, token, pair)
    }

    struct Session {
        _scratch: tempfile::TempDir,
        mgr: PreviewManager,
        task: TaskId,
        start: PreviewStart,
        epoch: Arc<AtomicU64>,
        _tree: StagedTree,
        _tree_dir: tempfile::TempDir,
    }
    fn session(ws: &WorkspaceIsolation, spec: &PreviewSpec) -> Session {
        let scratch = tempfile::tempdir().unwrap();
        let tree_dir = tempfile::tempdir().unwrap();
        let tree = site_tree(tree_dir.path(), None);
        let mgr = PreviewManager::new(scratch.path());
        let task = TaskId::random();
        let epoch = Arc::new(AtomicU64::new(7));
        let start = mgr.start(ws, &task, &tree, spec, &guard(&epoch)).unwrap();
        Session {
            _scratch: scratch,
            mgr,
            task,
            start,
            epoch,
            _tree: tree,
            _tree_dir: tree_dir,
        }
    }
    fn server_log(mgr: &PreviewManager, task: &TaskId) -> String {
        let c = mgr.logs(task, 0, u32::MAX);
        let bytes: Vec<u8> = c.records.iter().flat_map(|r| r.bytes.clone()).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
    fn wait_server_log(mgr: &PreviewManager, task: &TaskId, needle: &str) -> String {
        let t = Instant::now();
        loop {
            let log = server_log(mgr, task);
            if log.contains(needle) || t.elapsed() > Duration::from_secs(5) {
                return log;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn capability_token_and_cookie_required() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "capability_token_and_cookie_required",
        ) {
            return;
        }
        let ws = backend();
        let s = session(&ws, &spec(&["{port}"]));
        assert!(
            matches!(s.start.descriptor.ready, ReadyState::Ready { .. }),
            "{:?}",
            s.start
        );
        let url = s
            .start
            .capability_url
            .clone()
            .expect("capability url when Ready");
        let (port, token) = url_parts(&url);
        assert_eq!(port, s.start.descriptor.bridge_port);
        // No cookie / wrong token / wrong cookie value / foreign cookie name: 403, never forwarded.
        assert_eq!(get(port, "/api/trees?unauth=1", None).status, 403);
        assert_eq!(
            get(port, "/__pai/open?t=00000000000000000000000000000000", None).status,
            403
        );
        assert_eq!(get(port, "/__pai/open", None).status, 403);
        let (_, _, pair) = open_cookie(&url);
        let (name, value) = pair.split_once('=').unwrap();
        assert_ne!(value, token, "cookie token must differ from the URL token");
        assert_eq!(
            get(
                port,
                "/api/trees?wrongcookie=1",
                Some(&format!("{name}={}", "0".repeat(32)))
            )
            .status,
            403
        );
        assert_eq!(
            get(
                port,
                "/api/trees?foreign=1",
                Some(&format!("pai_preview_0000000000000000={value}"))
            )
            .status,
            403
        );
        assert_eq!(
            get(
                port,
                "/api/trees?prefix=1",
                Some(&format!("{name}={}", &value[..31]))
            )
            .status,
            403
        );
        // The URL token is one-time: reuse without the cookie is refused.
        assert_eq!(
            get(port, &format!("/__pai/open?t={token}"), None).status,
            403
        );
        let again = get(port, &format!("/__pai/open?t={token}"), Some(&pair));
        assert_eq!(again.status, 302);
        assert!(
            again.all("Set-Cookie").is_empty(),
            "token reuse must not mint a cookie"
        );
        assert_eq!(
            raw(port, format!("POST /__pai/open?t={token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\n\r\n").as_bytes()).status,
            405
        );
        // With the cookie: forwarded; the server never sees either token.
        let ok = get(
            port,
            "/api/trees",
            Some(&format!("site=1; {pair}; theme=dark")),
        );
        assert_eq!(ok.status, 200);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&ok.body).unwrap()[0]["name"],
            "oak"
        );
        let hdrs = raw(
            port,
            format!(
                "GET /headers HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: site=1; {pair}; theme=dark\r\nConnection: keep-alive, X-Hop\r\nX-Hop: 1\r\nKeep-Alive: timeout=5\r\nTE: trailers\r\nProxy-Authorization: Basic Zm9vOmJhcg==\r\nReferer: http://127.0.0.1:{port}/__pai/open?t={token}\r\nX-Custom: kept\r\n\r\n"
            )
            .as_bytes(),
        );
        assert_eq!(hdrs.status, 200, "{hdrs:?}");
        let seen: Vec<(String, String)> = serde_json::from_slice(&hdrs.body).unwrap();
        println!("STAGE5_PREVIEW_FORWARDED_HEADERS {seen:?}");
        let names: Vec<String> = seen.iter().map(|(k, _)| k.to_ascii_lowercase()).collect();
        let val = |n: &str| {
            seen.iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case(n))
                .map(|(_, v)| v.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(val("cookie"), vec!["site=1; theme=dark".to_string()]);
        assert_eq!(val("connection"), vec!["close".to_string()]);
        assert_eq!(val("x-custom"), vec!["kept".to_string()]);
        assert_eq!(val("host"), vec![format!("127.0.0.1:{port}")]);
        for gone in [
            "x-hop",
            "keep-alive",
            "te",
            "proxy-authorization",
            "referer",
        ] {
            assert!(
                !names.contains(&gone.to_string()),
                "{gone} forwarded: {seen:?}"
            );
        }
        let log = wait_server_log(&s.mgr, &s.task, "HDRS");
        let reqs = serde_json::to_string(&s.mgr.requests(&s.task, 0, u32::MAX)).unwrap();
        for secret in [token.as_str(), value] {
            assert!(
                !String::from_utf8_lossy(&hdrs.body).contains(secret),
                "token forwarded"
            );
            assert!(!log.contains(secret), "token reached server logs");
            assert!(!reqs.contains(secret), "token in request log");
        }
        for never in [
            "unauth=1",
            "wrongcookie=1",
            "foreign=1",
            "prefix=1",
            "__pai",
        ] {
            assert!(!log.contains(never), "{never} was forwarded: {log}");
        }
        let records = s.mgr.requests(&s.task, 0, u32::MAX);
        let rejected = records
            .iter()
            .filter(|r| {
                r.outcome
                    == RequestOutcome::Rejected {
                        reason: RejectReason::CapabilityMissing,
                    }
            })
            .count();
        assert_eq!(rejected, 7, "{records:?}");
        let stop = s.mgr.stop(&s.task, StopReasonKind::User).unwrap();
        let stop_json = serde_json::to_string(&stop).unwrap();
        assert!(!stop_json.contains(&token) && !stop_json.contains(value));
        assert!(!serde_json::to_string(&s.start.descriptor)
            .unwrap()
            .contains(&token));
    }

    #[test]
    fn host_header_rebinding_421() {
        if !crate::isolation::test_support::isolation_or_ci_skip("host_header_rebinding_421") {
            return;
        }
        let ws = backend();
        let s = session(&ws, &spec(&["{port}"]));
        let url = s.start.capability_url.clone().unwrap();
        let (port, token) = url_parts(&url);
        let open = format!("/__pai/open?t={token}");
        let with_host = |host: &str, path: &str, cookie: Option<&str>| {
            let c = cookie
                .map(|c| format!("Cookie: {c}\r\n"))
                .unwrap_or_default();
            raw(
                port,
                format!("GET {path} HTTP/1.1\r\n{host}{c}\r\n").as_bytes(),
            )
        };
        // A rebinding attempt never consumes the one-time token.
        for host in [
            "Host: evil.example\r\n".to_string(),
            format!("Host: evil.example:{port}\r\n"),
            "Host: 127.0.0.1\r\n".to_string(),
            format!("Host: 127.0.0.1:{}\r\n", port.wrapping_add(1)),
            format!("Host: 127.0.0.1:{port}\r\nHost: evil.example\r\n"),
            String::new(),
            format!("Host: 127.0.0.1:{port}.evil.example\r\n"),
            format!("Host: [::1]:{port}\r\n"),
        ] {
            let r = with_host(&host, &open, None);
            assert_eq!(r.status, 421, "{host:?}: {r:?}");
            assert!(r.all("Set-Cookie").is_empty());
        }
        let (_, _, pair) = open_cookie(&url);
        let r = with_host("Host: evil.example\r\n", "/api/trees?rebind=1", Some(&pair));
        assert_eq!(r.status, 421);
        assert_eq!(
            with_host(
                &format!("Host: localhost:{port}\r\n"),
                "/api/trees",
                Some(&pair)
            )
            .status,
            200
        );
        assert_eq!(
            with_host(
                &format!("Host: LOCALHOST:{port}\r\n"),
                "/api/trees?upper=1",
                Some(&pair)
            )
            .status,
            200
        );
        // Absolute-form targets are refused as well (no proxying elsewhere).
        let abs = raw(port, format!("GET http://evil.example/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {pair}\r\n\r\n").as_bytes());
        assert_eq!(abs.status, 400);
        let log = wait_server_log(&s.mgr, &s.task, "upper=1");
        assert!(!log.contains("rebind=1") && !log.contains("evil"), "{log}");
        let reasons: Vec<RequestOutcome> = s
            .mgr
            .requests(&s.task, 0, u32::MAX)
            .iter()
            .map(|r| r.outcome)
            .collect();
        assert_eq!(
            reasons
                .iter()
                .filter(|o| **o
                    == RequestOutcome::Rejected {
                        reason: RejectReason::HostRejected
                    })
                .count(),
            9
        );
        assert!(reasons.contains(&RequestOutcome::Rejected {
            reason: RejectReason::AbsoluteFormRejected
        }));
    }

    #[test]
    fn csp_and_security_headers_appended() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "csp_and_security_headers_appended",
        ) {
            return;
        }
        let ws = backend();
        let s = session(&ws, &spec(&["{port}"]));
        let (port, _, pair) = open_cookie(s.start.capability_url.as_deref().unwrap());
        let assert_security = |r: &Resp| {
            assert!(
                r.all("Content-Security-Policy").contains(&PREVIEW_CSP),
                "{r:?}"
            );
            assert_eq!(r.one("X-Content-Type-Options"), Some("nosniff"));
            assert_eq!(r.one("Referrer-Policy"), Some("no-referrer"));
            assert_eq!(r.one("Cross-Origin-Resource-Policy"), Some("same-origin"));
            assert_eq!(r.all("Connection"), vec!["close"], "{r:?}");
        };
        let r = get(port, "/own-headers", Some(&pair));
        println!(
            "STAGE5_PREVIEW_HEADERS elapsed_ms={} {:?}",
            r.elapsed.as_millis(),
            r.headers
        );
        assert_eq!((r.status, r.body.as_slice()), (200, &b"own"[..]));
        // The server answered "Connection: keep-alive" and kept its socket
        // open: the Content-Length body still ends the exchange promptly.
        assert!(r.elapsed < Duration::from_secs(3), "{:?}", r.elapsed);
        assert_security(&r);
        // The server's own headers are kept (both CSPs are enforced) ...
        let csp = r.all("Content-Security-Policy");
        assert_eq!(csp.len(), 2);
        assert!(csp.contains(&"script-src 'none'"));
        assert_eq!(r.one("X-Trees-Server"), Some("1"));
        assert_eq!(r.one("Content-Type"), Some("text/html"));
        // ... except connection management and bridge capability cookies.
        assert_eq!(r.all("Set-Cookie"), vec!["site=1; Path=/"]);
        assert_security(&get(port, "/api/trees", Some(&pair)));
        // Bridge-generated answers carry the same headers.
        assert_security(&get(port, "/api/trees", None));
        assert_security(&get(port, "/__pai/other", Some(&pair)));
        let (_, token) = url_parts(s.start.capability_url.as_deref().unwrap());
        assert_security(&get(port, &format!("/__pai/open?t={token}"), Some(&pair)));
    }

    #[test]
    fn upgrade_connect_chunked_rejected() {
        if !crate::isolation::test_support::isolation_or_ci_skip("upgrade_connect_chunked_rejected")
        {
            return;
        }
        let ws = backend();
        let s = session(&ws, &spec(&["{port}"]));
        let (port, _, pair) = open_cookie(s.start.capability_url.as_deref().unwrap());
        let h = format!("Host: 127.0.0.1:{port}\r\nCookie: {pair}\r\n");
        let cases: Vec<(String, u16, RejectReason)> = vec![
            (format!("CONNECT 127.0.0.1:22 HTTP/1.1\r\n{h}\r\n"), 405, RejectReason::ConnectRejected),
            (format!("GET /ws?u=1 HTTP/1.1\r\n{h}Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"), 501, RejectReason::UpgradeRejected),
            (format!("GET /ws?u=2 HTTP/1.1\r\n{h}Connection: keep-alive, upgrade\r\n\r\n"), 501, RejectReason::UpgradeRejected),
            (format!("GET /ws?u=3 HTTP/1.1\r\n{h}Upgrade: h2c\r\n\r\n"), 501, RejectReason::UpgradeRejected),
            (format!("POST /echo?c=1 HTTP/1.1\r\n{h}Transfer-Encoding: chunked\r\n\r\n4\r\npine\r\n0\r\n\r\n"), 411, RejectReason::ChunkedRejected),
            (format!("POST /echo?c=2 HTTP/1.1\r\n{h}Transfer-Encoding: gzip, chunked\r\n\r\n"), 411, RejectReason::ChunkedRejected),
            (format!("POST /echo?c=3 HTTP/1.1\r\n{h}Transfer-Encoding: gzip\r\n\r\n"), 501, RejectReason::TransferEncodingRejected),
            (format!("POST /echo?c=4 HTTP/1.1\r\n{h}Content-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\npine"), 411, RejectReason::ChunkedRejected),
            (format!("POST /echo?l=1 HTTP/1.1\r\n{h}Content-Length: abc\r\n\r\n"), 400, RejectReason::BadContentLength),
            (format!("POST /echo?l=2 HTTP/1.1\r\n{h}Content-Length: 4\r\nContent-Length: 5\r\n\r\npine"), 400, RejectReason::BadContentLength),
            (format!("POST /echo?l=3 HTTP/1.1\r\n{h}Content-Length: -1\r\n\r\n"), 400, RejectReason::BadContentLength),
            (format!("POST /echo?l=4 HTTP/1.1\r\n{h}Content-Length: 2000000\r\n\r\n"), 413, RejectReason::BodyTooLarge),
            (format!("POST /echo?e=1 HTTP/1.1\r\n{h}Content-Length: 4\r\nExpect: 100-continue\r\n\r\n"), 417, RejectReason::ExpectRejected),
            (format!("GET /big-head?h=1 HTTP/1.1\r\n{h}X-Pad: {}\r\n\r\n", "p".repeat(20_000)), 431, RejectReason::HeadTooLarge),
            (format!("GET /many?h=2 HTTP/1.1\r\n{h}{}\r\n", (0..70).map(|i| format!("X-{i}: v\r\n")).collect::<String>()), 431, RejectReason::HeadTooLarge),
            ("GARBAGE\r\n\r\n".to_string(), 400, RejectReason::Malformed),
        ];
        for (req, code, reason) in &cases {
            let r = raw(port, req.as_bytes());
            assert_eq!(r.status, *code, "{reason:?}: {r:?}");
            let records = s.mgr.requests(&s.task, 0, u32::MAX);
            assert_eq!(
                records.last().unwrap().outcome,
                RequestOutcome::Rejected { reason: *reason }
            );
        }
        // A short body is a client error, never forwarded half-read.
        let mut short = TcpStream::connect(("127.0.0.1", port)).unwrap();
        short
            .write_all(
                format!("POST /echo?short=1 HTTP/1.1\r\n{h}Content-Length: 10\r\n\r\npine")
                    .as_bytes(),
            )
            .unwrap();
        short.shutdown(Shutdown::Write).unwrap();
        let mut buf = Vec::new();
        let _ = short.read_to_end(&mut buf);
        assert!(
            buf.starts_with(b"HTTP/1.1 400 "),
            "{:?}",
            String::from_utf8_lossy(&buf)
        );
        // Control: a well-formed body request is forwarded.
        let ok = raw(
            port,
            format!("POST /echo?ok=1 HTTP/1.1\r\n{h}Content-Length: 4\r\n\r\npine").as_bytes(),
        );
        assert_eq!((ok.status, ok.body.as_slice()), (201, &b"pine"[..]));
        let log = wait_server_log(&s.mgr, &s.task, "REQ POST /echo?ok=1");
        assert!(log.contains("REQ POST /echo?ok=1"), "{log}");
        for never in [
            "u=1", "u=2", "u=3", "c=1", "c=2", "c=3", "c=4", "l=1", "l=2", "l=3", "l=4", "e=1",
            "h=1", "h=2", "short=1", "CONNECT",
        ] {
            assert!(!log.contains(never), "{never} forwarded: {log}");
        }
    }

    #[test]
    fn request_log_redacts_and_bounds() {
        if !crate::isolation::test_support::isolation_or_ci_skip("request_log_redacts_and_bounds") {
            return;
        }
        let ws = backend();
        let s = session(&ws, &spec(&["{port}"]));
        let url = s.start.capability_url.clone().unwrap();
        let (port, token, pair) = open_cookie(&url);
        let value = pair.split_once('=').unwrap().1.to_string();
        // A capability value smuggled into a forwarded path is refused and redacted.
        assert_eq!(
            get(port, &format!("/api/trees?leak={token}"), Some(&pair)).status,
            400
        );
        assert_eq!(get(port, &format!("/x/{value}/y"), Some(&pair)).status, 400);
        let long = format!("/{}", "w".repeat(2000));
        assert_eq!(get(port, &long, Some(&pair)).status, 200);
        // Responses are bounded at 8 MiB and recorded as truncated.
        let big = get(port, "/big", Some(&pair));
        assert_eq!(big.status, 200);
        assert_eq!(big.body.len() as u64, RESPONSE_MAX);
        let records = s.mgr.requests(&s.task, 0, u32::MAX);
        println!(
            "STAGE5_PREVIEW_REQUESTS {}",
            serde_json::to_string(&records[..records.len().min(6)]).unwrap()
        );
        assert_eq!(records[0].path, "/__pai/open?t=[redacted]");
        assert_eq!(records[0].status, Some(302));
        assert_eq!(records[1].path, "/api/trees?leak=[redacted]");
        assert_eq!(
            records[1].outcome,
            RequestOutcome::Rejected {
                reason: RejectReason::TokenInRequest
            }
        );
        assert_eq!(records[2].path, "/x/[redacted]/y");
        assert_eq!(records[3].path.len(), PATH_LOG_MAX);
        assert_eq!(
            (records[3].status, records[3].outcome),
            (Some(200), RequestOutcome::Ok)
        );
        assert_eq!(records[4].outcome, RequestOutcome::ResponseTruncated);
        assert_eq!(records[4].response_bytes, RESPONSE_MAX);
        assert!(records.iter().all(|r| r.source == RequestSource::Bridge));
        // The request ring is bounded at 1,024 records; totals stay exact.
        for i in 0..1100 {
            assert_eq!(
                get(port, &format!("/flood/{i}?t={token}"), None).status,
                403
            );
        }
        let all = s.mgr.requests(&s.task, 0, u32::MAX);
        let total = s.mgr.status(&s.task).requests_total;
        println!("STAGE5_PREVIEW_RING retained={} total={total}", all.len());
        assert_eq!(all.len(), REQUEST_RING_RECORDS);
        assert_eq!(total, 5 + 1100);
        assert_eq!(
            all.first().unwrap().seq,
            total - REQUEST_RING_RECORDS as u64
        );
        assert!(all.windows(2).all(|w| w[1].seq == w[0].seq + 1));
        assert_eq!(all.last().unwrap().path, "/flood/1099?t=[redacted]");
        assert_eq!(s.mgr.requests(&s.task, total - 3, 10).len(), 3);
        let json = serde_json::to_string(&all).unwrap();
        assert!(!json.contains(&token) && !json.contains(&value));
        let log = server_log(&s.mgr, &s.task);
        assert!(!log.contains(&token) && !log.contains(&value) && !log.contains("flood"));
    }

    #[test]
    fn log_ring_64k_cap_with_truncation_metadata() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "log_ring_64k_cap_with_truncation_metadata",
        ) {
            return;
        }
        let ws = backend();
        let s = session(&ws, &spec(&["{port}", "noisy"]));
        assert!(matches!(s.start.descriptor.ready, ReadyState::Ready { .. }));
        let log = wait_server_log(&s.mgr, &s.task, "NOISE-END");
        assert!(
            log.contains("NOISE-END"),
            "{}",
            &log[log.len().saturating_sub(200)..]
        );
        let c = s.mgr.logs(&s.task, 0, u32::MAX);
        let retained: usize = c.records.iter().map(|r| r.bytes.len()).sum();
        println!(
            "STAGE5_PREVIEW_LOGRING retained={} cap={} dropped_bytes={} dropped_records={} first_seq={} supervisor_dropped={} truncated_before_cursor={}",
            c.retained_bytes, c.cap_bytes, c.dropped_bytes, c.dropped_records, c.first_retained_seq, c.supervisor_dropped_bytes, c.truncated_before_cursor
        );
        assert_eq!(c.cap_bytes, LOG_RING_BYTES as u64);
        assert_eq!(c.retained_bytes, retained as u64);
        assert!(retained <= LOG_RING_BYTES && retained > LOG_RING_BYTES / 2);
        assert!(c.dropped_records > 0 && c.dropped_bytes > 0 && c.first_retained_seq > 0);
        assert!(c.truncated_before_cursor);
        assert_eq!(c.records[0].seq, c.first_retained_seq);
        assert!(c.records.windows(2).all(|w| w[1].seq == w[0].seq + 1));
        assert!(c.retained_bytes + c.dropped_bytes + c.supervisor_dropped_bytes >= 40 * 4000);
        // Cursor paging over the retained window.
        let page = s.mgr.logs(&s.task, c.first_retained_seq, 2);
        assert_eq!(page.records.len(), 2);
        assert!(!page.truncated_before_cursor);
        assert_eq!(page.next_cursor, c.first_retained_seq + 2);
        let tail = s.mgr.logs(&s.task, c.next_cursor, 10);
        assert!(tail.records.is_empty() || tail.records[0].seq >= c.next_cursor);
        assert!(s.mgr.status(&s.task).log_next_seq >= c.next_cursor);
        // The final snapshot in the StopRecord keeps the metadata.
        let stop = s.mgr.stop(&s.task, StopReasonKind::User).unwrap();
        assert!(
            stop.logs.retained_bytes <= LOG_RING_BYTES as u64
                && stop.logs.dropped_records >= c.dropped_records
        );
    }

    #[test]
    fn readiness_timeout_exit_and_http_failure_states() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "readiness_timeout_exit_and_http_failure_states",
        ) {
            return;
        }
        let ws = backend();
        let scratch = tempfile::tempdir().unwrap();
        let tree_dir = tempfile::tempdir().unwrap();
        let tree = site_tree(tree_dir.path(), None);
        let mgr = PreviewManager::new(scratch.path());
        let epoch = Arc::new(AtomicU64::new(1));
        let run = |mode: &str, timeout_ms: u64, ready: (u16, u16)| {
            let m = marker();
            let mut sp = spec(&["{port}", mode, "0", &m]);
            sp.startup_timeout_ms = timeout_ms;
            sp.ready_status = ready;
            let task = TaskId::random();
            let t0 = Instant::now();
            let start = mgr.start(&ws, &task, &tree, &sp, &guard(&epoch)).unwrap();
            let elapsed = t0.elapsed();
            let live = live_with(&m);
            println!(
                "STAGE5_PREVIEW_READY mode={mode} ready={:?} elapsed_ms={} live_after_start={live}",
                start.descriptor.ready,
                elapsed.as_millis()
            );
            (task, start, elapsed, live, m)
        };
        // Timeout: the server never listens.
        let (task, start, elapsed, _, m) = run("never", 1500, (200, 399));
        assert_eq!(
            start.descriptor.ready,
            ReadyState::StartupFailed {
                reason: StartupFailure::Timeout
            }
        );
        assert!(
            elapsed >= Duration::from_millis(1500) && elapsed < Duration::from_secs(8),
            "{elapsed:?}"
        );
        assert!(start.capability_url.is_none());
        assert_eq!(start.descriptor.bridge_port, 0);
        assert_eq!(
            wait_gone(&m, Duration::from_secs(3)),
            0,
            "a never-ready server must not keep running"
        );
        let st = mgr.status(&task);
        assert_eq!(
            st.state,
            PreviewState::StartupFailed {
                reason: StartupFailure::Timeout
            }
        );
        assert!(!st.running && st.bridge_port.is_none());
        assert_eq!(mgr.run_http_checks(&task), Err(PreviewError::NotRunning));
        assert!(server_log(&mgr, &task).contains("never listening"));
        let rec = mgr.stop(&task, StopReasonKind::StartupFailed).unwrap();
        assert!(
            rec.report.killed && rec.socket_dir_removed && rec.report.descendants_alive_after == 0
        );
        assert!(svc_dirs(scratch.path()).is_empty());
        // Exited before readiness, with its real status.
        let (task, start, elapsed, _, m) = run("exit3", 10_000, (200, 399));
        assert_eq!(
            start.descriptor.ready,
            ReadyState::StartupFailed {
                reason: StartupFailure::Exited { status: Some(3) }
            }
        );
        assert!(elapsed < Duration::from_secs(8));
        assert!(start.capability_url.is_none());
        assert_eq!(wait_gone(&m, Duration::from_secs(3)), 0);
        assert!(server_log(&mgr, &task).contains("exiting with 3"));
        mgr.stop(&task, StopReasonKind::StartupFailed).unwrap();
        // HTTP failure: answers, but never with a ready status.
        let (task, start, _, _, m) = run("http500", 1500, (200, 399));
        assert_eq!(
            start.descriptor.ready,
            ReadyState::StartupFailed {
                reason: StartupFailure::Http { status: 500 }
            }
        );
        assert!(start.capability_url.is_none());
        assert_eq!(wait_gone(&m, Duration::from_secs(3)), 0);
        assert!(server_log(&mgr, &task).contains("REQ GET /"));
        mgr.stop(&task, StopReasonKind::StartupFailed).unwrap();
        // The ready predicate is exactly ready_status (500 accepted when asked).
        let (task, start, _, _, m) = run("http500", 5000, (500, 500));
        assert!(
            matches!(start.descriptor.ready, ReadyState::Ready { .. }),
            "{start:?}"
        );
        assert!(live_with(&m) > 0);
        mgr.stop(&task, StopReasonKind::User).unwrap();
        assert_eq!(wait_gone(&m, Duration::from_secs(3)), 0);
        // Ready.
        let (task, start, _, _, m) = run("ok", 10_000, (200, 399));
        let ReadyState::Ready { after_ms } = start.descriptor.ready else {
            panic!("{start:?}")
        };
        assert!(after_ms < 10_000);
        assert!(start.capability_url.is_some() && start.descriptor.bridge_port != 0);
        assert_eq!(mgr.status(&task).state, PreviewState::Ready { after_ms });
        assert!(live_with(&m) > 0);
        drop(mgr);
        assert_eq!(
            wait_gone(&m, Duration::from_secs(3)),
            0,
            "manager drop stops every session"
        );
        assert!(svc_dirs(scratch.path()).is_empty());
    }

    #[test]
    fn http_checks_bound_to_tree_and_service_ids() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "http_checks_bound_to_tree_and_service_ids",
        ) {
            return;
        }
        let ws = backend();
        let mut sp = spec(&["{port}"]);
        let check = |id: &str, method: HttpMethod, path: &str| HttpCheck {
            id: id.into(),
            method,
            path: path.into(),
            body: None,
            expect_status: (200, 200),
            expect_content_type_prefix: None,
            expect_body_contains: Vec::new(),
            expect_json_equals: None,
        };
        let trees = serde_json::json!([{"name":"oak"},{"name":"birch"},{"name":"cedar"}]);
        sp.http_checks = vec![
            HttpCheck {
                expect_content_type_prefix: Some("application/json".into()),
                expect_json_equals: Some(trees.clone()),
                ..check("trees-json", HttpMethod::Get, "/api/trees")
            },
            HttpCheck {
                expect_content_type_prefix: Some("text/html".into()),
                expect_body_contains: vec!["<h1>Trees</h1>".into()],
                ..check("home-html", HttpMethod::Get, "/")
            },
            HttpCheck {
                body: Some("maple".into()),
                expect_status: (201, 201),
                expect_body_contains: vec!["maple".into()],
                ..check("post-echo", HttpMethod::Post, "/echo")
            },
            HttpCheck {
                expect_status: (404, 404),
                ..check("wrong-status", HttpMethod::Get, "/api/trees")
            },
            HttpCheck {
                expect_json_equals: Some(serde_json::json!([])),
                ..check("wrong-json", HttpMethod::Get, "/api/trees")
            },
        ];
        let s = session(&ws, &sp);
        let tree_sha = s.start.descriptor.tree_sha256.clone();
        let sid = s.start.descriptor.service_id.clone();
        let rec = s.mgr.run_http_checks(&s.task).unwrap();
        println!(
            "STAGE5_PREVIEW_CHECKS {}",
            serde_json::to_string(&rec).unwrap()
        );
        assert_eq!(
            (rec.tree_sha256.as_str(), rec.service_id.as_str()),
            (tree_sha.as_str(), sid.as_str())
        );
        assert_eq!(rec.evidence_level, EvidenceLevel::HttpLevel);
        assert!(rec.is_current(&tree_sha, &sid));
        let passed: Vec<(&str, bool)> = rec
            .results
            .iter()
            .map(|r| (r.id.as_str(), r.passed))
            .collect();
        assert_eq!(
            passed,
            vec![
                ("trees-json", true),
                ("home-html", true),
                ("post-echo", true),
                ("wrong-status", false),
                ("wrong-json", false)
            ]
        );
        assert!(!rec.all_passed());
        // Small bodies: the excerpt is the whole body, so its hash must match.
        assert_eq!(
            rec.results[0].body_sha256,
            Some(hash(rec.results[0].excerpt.as_bytes()))
        );
        assert_eq!(rec.results[2].excerpt, "maple");
        assert_eq!(
            rec.results[3].failure.as_deref(),
            Some("status 200 not in 404..=404")
        );
        assert_eq!(
            rec.results[4].failure.as_deref(),
            Some("JSON body differs from the expected value")
        );
        let check_records: Vec<_> = s
            .mgr
            .requests(&s.task, 0, u32::MAX)
            .into_iter()
            .filter(|r| r.source == RequestSource::HttpCheck)
            .collect();
        assert_eq!(check_records.len(), 5);
        assert_eq!(check_records[2].method, "POST");
        // Same tree, new server: the old record is stale.
        let first = s.mgr.stop(&s.task, StopReasonKind::User).unwrap();
        assert_eq!(first.service_id, sid);
        assert_eq!(
            s.mgr.run_http_checks(&s.task),
            Err(PreviewError::NotRunning)
        );
        let restarted = s
            .mgr
            .start(&ws, &s.task, &s._tree, &sp, &guard(&s.epoch))
            .unwrap();
        assert_eq!(restarted.descriptor.tree_sha256, tree_sha);
        assert_ne!(restarted.descriptor.service_id, sid);
        assert!(!rec.is_current(
            &restarted.descriptor.tree_sha256,
            &restarted.descriptor.service_id
        ));
        let rec2 = s.mgr.run_http_checks(&s.task).unwrap();
        assert!(rec2.is_current(&tree_sha, &restarted.descriptor.service_id));
        s.mgr.stop(&s.task, StopReasonKind::User).unwrap();
        // Different content: different tree identity, checks see the new bytes.
        let other_dir = tempfile::tempdir().unwrap();
        let other = site_tree(other_dir.path(), Some("willow"));
        assert_ne!(other.tree_sha256(), tree_sha);
        let third = s
            .mgr
            .start(&ws, &s.task, &other, &sp, &guard(&s.epoch))
            .unwrap();
        let rec3 = s.mgr.run_http_checks(&s.task).unwrap();
        assert!(!rec.is_current(&third.descriptor.tree_sha256, &third.descriptor.service_id));
        assert!(!rec2.is_current(&third.descriptor.tree_sha256, &third.descriptor.service_id));
        assert!(rec3.is_current(other.tree_sha256(), &third.descriptor.service_id));
        assert!(!rec3.results[0].passed, "{:?}", rec3.results[0]);
        assert!(rec3.results[0].excerpt.contains("willow"));
        assert_ne!(rec3.results[0].body_sha256, rec.results[0].body_sha256);
        assert!(s.mgr.stop(&s.task, StopReasonKind::TaskClosed).is_some());
    }

    #[test]
    fn stop_removes_socket_dir_and_listeners() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "stop_removes_socket_dir_and_listeners",
        ) {
            return;
        }
        let ws = backend();
        let scratch = tempfile::tempdir().unwrap();
        let tree_dir = tempfile::tempdir().unwrap();
        let tree = site_tree(tree_dir.path(), None);
        let mgr = PreviewManager::new(scratch.path());
        let epoch = Arc::new(AtomicU64::new(3));
        let (ma, mb) = (marker(), marker());
        let (a, b) = (TaskId::random(), TaskId::random());
        let sa = mgr
            .start(
                &ws,
                &a,
                &tree,
                &spec(&["{port}", "ok", "0", &ma]),
                &guard(&epoch),
            )
            .unwrap();
        let sb = mgr
            .start(
                &ws,
                &b,
                &tree,
                &spec(&["{port}", "ok", "0", &mb]),
                &guard(&epoch),
            )
            .unwrap();
        assert_eq!(svc_dirs(scratch.path()).len(), 2);
        // Admission: at most two sessions, one per task, epoch current, valid spec.
        let c = TaskId::random();
        let spawns = crate::isolation::workspace::spawns_here();
        assert_eq!(
            mgr.start(&ws, &c, &tree, &spec(&["{port}"]), &guard(&epoch))
                .err(),
            Some(PreviewError::Busy)
        );
        assert_eq!(
            mgr.start(&ws, &a, &tree, &spec(&["{port}"]), &guard(&epoch))
                .err(),
            Some(PreviewError::AlreadyRunning)
        );
        let stale = guard(&epoch);
        epoch.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            mgr.start(&ws, &c, &tree, &spec(&["{port}"]), &stale).err(),
            Some(PreviewError::Locked)
        );
        assert_eq!(
            crate::isolation::workspace::spawns_here(),
            spawns,
            "denied before spawn"
        );
        let (pa, _, ca) = open_cookie(sa.capability_url.as_deref().unwrap());
        let (pb, _, cb) = open_cookie(sb.capability_url.as_deref().unwrap());
        assert_eq!(get(pa, "/api/trees", Some(&ca)).status, 200);
        assert!(live_with(&ma) > 0 && live_with(&mb) > 0);
        let dir_a = svc_dirs(scratch.path());
        let rec = mgr.stop(&a, StopReasonKind::User).unwrap();
        println!(
            "STAGE5_PREVIEW_STOP report={:?} socket_dir_removed={} bridge_closed={}",
            rec.report, rec.socket_dir_removed, rec.bridge_closed
        );
        assert_eq!(rec.reason, StopReasonKind::User);
        assert!(rec.report.killed && rec.report.descendants_alive_after == 0);
        assert!(rec.socket_dir_removed && rec.bridge_closed);
        assert_eq!(rec.service_id, sa.descriptor.service_id);
        assert!(rec
            .requests
            .iter()
            .any(|r| r.path == "/api/trees" && r.status == Some(200)));
        assert!(
            TcpStream::connect(("127.0.0.1", pa)).is_err(),
            "bridge listener survived stop"
        );
        assert_eq!(wait_gone(&ma, Duration::from_secs(3)), 0);
        let left = svc_dirs(scratch.path());
        assert_eq!(left.len(), 1);
        assert!(dir_a
            .iter()
            .filter(|d| !left.contains(d))
            .all(|d| !d.exists()));
        assert_eq!(mgr.status(&a).state, PreviewState::NotStarted);
        assert!(mgr.stop(&a, StopReasonKind::User).is_none());
        // The neighbour is untouched.
        assert!(live_with(&mb) > 0);
        assert_eq!(get(pb, "/api/trees", Some(&cb)).status, 200);
        // Idle stop: no authenticated traffic for idle_stop_ms.
        let mut idle = spec(&["{port}"]);
        idle.idle_stop_ms = 1000;
        let si = mgr.start(&ws, &c, &tree, &idle, &guard(&epoch)).unwrap();
        let (pi, token_i) = url_parts(si.capability_url.as_deref().unwrap());
        let t = Instant::now();
        while mgr.status(&c).state != PreviewState::IdleStopped
            && t.elapsed() < Duration::from_secs(6)
        {
            // Unauthenticated noise never keeps a preview alive.
            let _ = TcpStream::connect(("127.0.0.1", pi))
                .map(|mut s| s.write_all(b"GET / HTTP/1.1\r\n\r\n"));
            thread::sleep(Duration::from_millis(200));
        }
        assert_eq!(mgr.status(&c).state, PreviewState::IdleStopped);
        let t = Instant::now();
        while mgr.status(&c).running && t.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(!mgr.status(&c).running);
        assert!(TcpStream::connect(("127.0.0.1", pi)).is_err());
        let rec = mgr.stop(&c, StopReasonKind::Idle).unwrap();
        assert!(rec.socket_dir_removed && rec.report.descendants_alive_after == 0);
        assert!(!serde_json::to_string(&rec).unwrap().contains(&token_i));
        // Vault lock: stop_all.
        mgr.stop_all(StopReasonKind::Locked);
        assert!(TcpStream::connect(("127.0.0.1", pb)).is_err());
        assert_eq!(wait_gone(&mb, Duration::from_secs(3)), 0);
        assert!(svc_dirs(scratch.path()).is_empty());
        assert_eq!(mgr.status(&b).state, PreviewState::NotStarted);
    }

    #[test]
    fn pool_exhaustion_returns_503_not_hang() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "pool_exhaustion_returns_503_not_hang",
        ) {
            return;
        }
        let ws = backend();
        let mut sp = spec(&["{port}"]);
        sp.service.tunnels_pool = 1;
        sp.service.tunnels_max = 1;
        let s = session(&ws, &sp);
        let (port, _, pair) = open_cookie(s.start.capability_url.as_deref().unwrap());
        let slow_pair = pair.clone();
        let slow = thread::spawn(move || get(port, "/slow", Some(&slow_pair)));
        thread::sleep(Duration::from_millis(500));
        let r = get(port, "/api/trees?while-busy=1", Some(&pair));
        println!(
            "STAGE5_PREVIEW_POOL status={} elapsed_ms={}",
            r.status,
            r.elapsed.as_millis()
        );
        assert_eq!(r.status, 503);
        assert!(
            r.elapsed >= Duration::from_millis(1900) && r.elapsed < Duration::from_millis(3500),
            "{:?}",
            r.elapsed
        );
        let slow = slow.join().unwrap();
        assert_eq!((slow.status, slow.body.as_slice()), (200, &b"slow"[..]));
        // The pool recovers once the tunnel is replaced.
        let t = Instant::now();
        let mut code = 0;
        while t.elapsed() < Duration::from_secs(5) {
            code = get(port, "/api/trees", Some(&pair)).status;
            if code == 200 {
                break;
            }
        }
        assert_eq!(code, 200);
        let records = s.mgr.requests(&s.task, 0, u32::MAX);
        assert!(records.iter().any(|r| r.path == "/api/trees?while-busy=1"
            && r.outcome
                == RequestOutcome::Rejected {
                    reason: RejectReason::PoolExhausted
                }));
        // Unauthenticated connection flood: 32 idle connections (no bytes)
        // never refuse a cookie holder (the oldest pre-auth connection is
        // evicted; review R1 F1) and never take the authenticated share.
        let idle: Vec<TcpStream> = (0..MAX_CONNECTIONS)
            .map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap())
            .collect();
        thread::sleep(Duration::from_millis(400));
        let r = get(port, "/api/trees?unauth-flood=1", Some(&pair));
        println!(
            "STAGE5_PREVIEW_UNAUTH_FLOOD status={} elapsed_ms={}",
            r.status,
            r.elapsed.as_millis()
        );
        assert_eq!(r.status, 200, "{r:?}");
        assert!(r.elapsed < Duration::from_secs(1));
        drop(idle);
        // Connection flood: with every slot held by an authenticated exchange
        // (cookie-holding POSTs whose body is pending), the 33rd concurrent
        // connection gets 503 at once.
        let held: Vec<TcpStream> = (0..MAX_CONNECTIONS)
            .map(|i| {
                let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
                c.write_all(
                    format!("POST /echo?held={i} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {pair}\r\nContent-Length: 10\r\n\r\n")
                        .as_bytes(),
                )
                .unwrap();
                thread::sleep(Duration::from_millis(30));
                c
            })
            .collect();
        thread::sleep(Duration::from_millis(400));
        let r = get(port, "/api/trees?flood=1", Some(&pair));
        println!(
            "STAGE5_PREVIEW_FLOOD status={} elapsed_ms={}",
            r.status,
            r.elapsed.as_millis()
        );
        assert_eq!(r.status, 503);
        assert!(r.elapsed < Duration::from_secs(1));
        drop(held);
        let t = Instant::now();
        let mut code = 0;
        while t.elapsed() < Duration::from_secs(8) {
            code = get(port, "/api/trees", Some(&pair)).status;
            if code == 200 {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(code, 200);
        assert!(s
            .mgr
            .requests(&s.task, 0, u32::MAX)
            .iter()
            .any(|r| r.status == Some(503)
                && r.outcome
                    == RequestOutcome::Rejected {
                        reason: RejectReason::TooManyConnections
                    }));
    }

    #[test]
    fn preview_sandbox_cannot_reach_host_loopback() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "preview_sandbox_cannot_reach_host_loopback",
        ) {
            return;
        }
        let ws = backend();
        let host = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        host.set_nonblocking(true).unwrap();
        let hp = host.local_addr().unwrap().port();
        let s = session(&ws, &spec(&["{port}", "ok", &hp.to_string()]));
        let log = wait_server_log(&s.mgr, &s.task, "INFO ");
        println!(
            "STAGE5_PREVIEW_LOOPBACK {}",
            log.lines().find(|l| l.starts_with("INFO ")).unwrap_or("")
        );
        assert!(
            log.contains(r#"INFO {"host_loopback": "ECONNREFUSED"}"#),
            "{log}"
        );
        let (port, _, pair) = open_cookie(s.start.capability_url.as_deref().unwrap());
        // Neither an unrelated host listener nor the bridge itself is reachable.
        for target in [hp, port] {
            let r = get(port, &format!("/probe/{target}"), Some(&pair));
            assert_eq!(
                (r.status, String::from_utf8_lossy(&r.body).into_owned()),
                (200, "ECONNREFUSED".to_string()),
                "target {target}"
            );
        }
        assert!(
            host.accept().is_err(),
            "host listener was reached from the sandbox"
        );
        drop(s);
    }

    // ---- Review R1 regressions (F1 slowloris, P09 stop residue, O1, O4) ----

    /// `n` unauthenticated connections that send a partial head, then 1 byte
    /// per `every` (review R1 `slowloris`). The handle returns how many of
    /// them the bridge still kept open (no data, no EOF) when stopped.
    fn trickle(
        port: u16,
        n: usize,
        every: Duration,
    ) -> (Arc<AtomicBool>, thread::JoinHandle<usize>) {
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let mut conns = Vec::new();
        for _ in 0..n {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(b"GET / HTTP/1.1\r\nX-Slow: ").unwrap();
            conns.push(s);
        }
        let h = thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                for s in conns.iter_mut() {
                    let _ = s.write_all(b"a");
                }
                let t = Instant::now();
                while t.elapsed() < every && !st.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(50));
                }
            }
            let mut open = 0;
            for s in conns.iter_mut() {
                let _ = s.set_nonblocking(true);
                let mut b = [0u8; 64];
                if matches!(s.read(&mut b), Err(e) if e.kind() == ErrorKind::WouldBlock) {
                    open += 1;
                }
            }
            open
        });
        (stop, h)
    }
    /// Bridge connection threads alive in this process ("pai-preview-conn",
    /// truncated to 15 bytes by the kernel).
    fn conn_threads() -> usize {
        std::fs::read_dir("/proc/self/task")
            .unwrap()
            .flatten()
            .filter(|e| {
                std::fs::read_to_string(e.path().join("comm"))
                    .is_ok_and(|c| c.trim() == "pai-preview-con")
            })
            .count()
    }
    /// Raw exchange that tolerates a connection closed without a response.
    fn raw_status(port: u16, req: &[u8], timeout: Duration) -> (Option<u16>, Duration) {
        let t0 = Instant::now();
        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("bridge listening");
        s.set_read_timeout(Some(timeout)).unwrap();
        let _ = s.write_all(req);
        let mut buf = Vec::new();
        let _ = s.read_to_end(&mut buf);
        let status = String::from_utf8_lossy(&buf)
            .split(' ')
            .nth(1)
            .and_then(|c| c.parse().ok());
        (status, t0.elapsed())
    }

    /// Review R1 P05b (+ P05a timeline), finding F1: 32 unauthenticated
    /// clients trickling 1 byte / 2 s must neither refuse a cookie holder
    /// during the attack nor hold any slot past the absolute head deadline.
    #[test]
    fn slowloris_preauth_slots_bounded_and_never_starve_cookie_holder() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "slowloris_preauth_slots_bounded_and_never_starve_cookie_holder",
        ) {
            return;
        }
        let ws = backend();
        let s = session(&ws, &spec(&["{port}"]));
        let (port, _, pair) = open_cookie(s.start.capability_url.as_deref().unwrap());
        let (stop, trickler) = trickle(port, MAX_CONNECTIONS, Duration::from_secs(2));
        let t0 = Instant::now();
        thread::sleep(Duration::from_secs(1));
        let early = get(port, "/api/trees?during-attack=1", Some(&pair));
        while t0.elapsed() < Duration::from_secs(12) {
            thread::sleep(Duration::from_millis(50));
        }
        let late = get(port, "/api/trees?after-12s=1", Some(&pair));
        let held_s = t0.elapsed().as_secs_f64();
        stop.store(true, Ordering::SeqCst);
        let still_open = trickler.join().unwrap();
        let records = s.mgr.requests(&s.task, 0, u32::MAX);
        let timeouts = records
            .iter()
            .filter(|r| {
                r.status == Some(408)
                    && r.outcome
                        == RequestOutcome::Rejected {
                            reason: RejectReason::ClientTimeout,
                        }
            })
            .count();
        println!(
            "STAGE5_PREVIEW_SLOWLORIS early={} early_ms={} late={} late_ms={} held_s={held_s:.1} attacker_open_at_end={still_open} timeouts_408={timeouts}",
            early.status,
            early.elapsed.as_millis(),
            late.status,
            late.elapsed.as_millis()
        );
        assert_eq!(
            early.status, 200,
            "unauthenticated tricklers refused a cookie holder: {early:?}"
        );
        assert!(
            early.elapsed < Duration::from_secs(2),
            "{:?}",
            early.elapsed
        );
        assert_eq!(
            late.status, 200,
            "slots still held after {held_s:.1}s: {late:?}"
        );
        assert_eq!(
            still_open, 0,
            "attacker connections outlived the absolute head deadline"
        );
        assert!(timeouts >= 1, "no 408 at the head deadline: {records:?}");
        // The dev server writes its access-log line after replying; wait for it
        // (bounded) instead of a single read that can race the log append.
        let log = wait_server_log(&s.mgr, &s.task, "after-12s=1");
        assert!(
            log.contains("after-12s=1") && !log.contains("X-Slow"),
            "{log}"
        );
    }

    /// Review R1 P09 tail of F1: an accepted, unauthenticated, still-trickling
    /// connection and its thread must not survive `stop`; an in-flight
    /// request must end (no hang).
    #[test]
    fn stop_shuts_down_lingering_bridge_connections() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "stop_shuts_down_lingering_bridge_connections",
        ) {
            return;
        }
        let ws = backend();
        let threads_before = conn_threads();
        let s = session(&ws, &spec(&["{port}"]));
        let (port, _, pair) = open_cookie(s.start.capability_url.as_deref().unwrap());
        let slow_req =
            format!("GET /slow HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {pair}\r\n\r\n");
        let slow =
            thread::spawn(move || raw_status(port, slow_req.as_bytes(), Duration::from_secs(20)));
        let (tstop, trickler) = trickle(port, 1, Duration::from_secs(1));
        thread::sleep(Duration::from_millis(1000));
        let t0 = Instant::now();
        let rec = s.mgr.stop(&s.task, StopReasonKind::User).unwrap();
        let stop_ms = t0.elapsed().as_millis();
        let (slow_status, slow_elapsed) = slow.join().unwrap();
        // Keep trickling past the old per-read CLIENT_TIMEOUT window.
        thread::sleep(Duration::from_millis(1500));
        let threads_after = conn_threads();
        tstop.store(true, Ordering::SeqCst);
        let still_open = trickler.join().unwrap();
        println!(
            "STAGE5_PREVIEW_STOP_LINGER stop_ms={stop_ms} bridge_closed={} inflight={slow_status:?}/{}ms trickler_open_after_stop={still_open} conn_threads_before={threads_before} after={threads_after}",
            rec.bridge_closed,
            slow_elapsed.as_millis()
        );
        assert!(rec.bridge_closed && rec.socket_dir_removed && rec.report.killed);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
        assert!(slow_elapsed < Duration::from_secs(12), "{slow_elapsed:?}");
        assert_eq!(still_open, 0, "trickling connection survived stop");
        assert!(
            threads_after <= threads_before,
            "bridge connection threads survived stop: {threads_after} > {threads_before}"
        );
    }

    /// Review R1 O1: the cookie holder mis-encoding its own secrets into the
    /// request line (method, upper-cased path) or header names/values must not
    /// put them into the request ring / StopRecord or forward them upstream.
    #[test]
    fn request_log_never_records_secret_bearing_request_line_parts() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "request_log_never_records_secret_bearing_request_line_parts",
        ) {
            return;
        }
        let ws = backend();
        let s = session(&ws, &spec(&["{port}"]));
        let url = s.start.capability_url.clone().unwrap();
        let (port, token, pair) = open_cookie(&url);
        let value = pair.split_once('=').unwrap().1.to_string();
        let (up_value, up_token) = (value.to_ascii_uppercase(), token.to_ascii_uppercase());
        let h = format!("Host: 127.0.0.1:{port}\r\nCookie: {pair}\r\n");
        for req in [
            format!("{up_value} / HTTP/1.1\r\n{h}\r\n"),
            format!("{up_token} /api/trees HTTP/1.1\r\n{h}\r\n"),
            format!("PROPFIND{up_value} / HTTP/1.1\r\n{h}\r\n"),
            format!("GET /x/{up_value}/y HTTP/1.1\r\n{h}\r\n"),
            format!("GET /api/trees?t={up_token} HTTP/1.1\r\n{h}\r\n"),
            format!("GET /headers HTTP/1.1\r\n{h}X-{up_value}: 1\r\nX-Up: {up_token}\r\n\r\n"),
            format!("get /api/trees?lower-method=1 HTTP/1.1\r\n{h}\r\n"),
        ] {
            let r = raw(port, req.as_bytes());
            assert!(r.status >= 200, "{r:?}");
        }
        assert_eq!(get(port, "/api/trees?control=1", Some(&pair)).status, 200);
        let records = s.mgr.requests(&s.task, 0, u32::MAX);
        println!(
            "STAGE5_PREVIEW_O1 methods={:?}",
            records
                .iter()
                .map(|r| r.method.as_str())
                .collect::<Vec<_>>()
        );
        const LOGGABLE: [&str; 11] = [
            "GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "CONNECT", "TRACE",
            "OTHER", "-",
        ];
        for r in &records {
            assert!(LOGGABLE.contains(&r.method.as_str()), "{r:?}");
        }
        let log = wait_server_log(&s.mgr, &s.task, "control=1").to_ascii_lowercase();
        let ring = serde_json::to_string(&records)
            .unwrap()
            .to_ascii_lowercase();
        let stop = serde_json::to_string(&s.mgr.stop(&s.task, StopReasonKind::User).unwrap())
            .unwrap()
            .to_ascii_lowercase();
        for secret in [&token, &value] {
            for (what, hay) in [
                ("ring", &ring),
                ("stop record", &stop),
                ("server log", &log),
            ] {
                let leaked = (0..=secret.len() - 8).find(|&i| hay.contains(&secret[i..i + 8]));
                assert!(
                    leaked.is_none(),
                    "8+ chars of a capability secret in the {what}: {hay}"
                );
            }
        }
    }

    /// Review R1 O4: an idle-stopped session releases its service, socket
    /// dir, ctl.sock and MAX_SESSIONS slot without an explicit stop; its
    /// status and final record stay available until `stop` collects them.
    #[test]
    fn idle_stopped_session_is_reaped() {
        if !crate::isolation::test_support::isolation_or_ci_skip("idle_stopped_session_is_reaped") {
            return;
        }
        let ws = backend();
        let scratch = tempfile::tempdir().unwrap();
        let tree_dir = tempfile::tempdir().unwrap();
        let tree = site_tree(tree_dir.path(), None);
        let mgr = PreviewManager::new(scratch.path());
        let epoch = Arc::new(AtomicU64::new(5));
        let m = marker();
        let (a, b, c) = (TaskId::random(), TaskId::random(), TaskId::random());
        let mut idle = spec(&["{port}", "ok", "0", &m]);
        idle.idle_stop_ms = 1000;
        let sa = mgr.start(&ws, &a, &tree, &idle, &guard(&epoch)).unwrap();
        let (pa, token_a, ca) = open_cookie(sa.capability_url.as_deref().unwrap());
        assert_eq!(get(pa, "/api/trees?before-idle=1", Some(&ca)).status, 200);
        assert_eq!(svc_dirs(scratch.path()).len(), 1);
        let t = Instant::now();
        while mgr.status(&a).state != PreviewState::IdleStopped
            && t.elapsed() < Duration::from_secs(6)
        {
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(mgr.status(&a).state, PreviewState::IdleStopped);
        let t = Instant::now();
        while !svc_dirs(scratch.path()).is_empty() && t.elapsed() < Duration::from_secs(8) {
            thread::sleep(Duration::from_millis(50));
        }
        let dirs = svc_dirs(scratch.path());
        println!(
            "STAGE5_PREVIEW_REAP svc_dirs_after_idle={} reaped_after_ms={}",
            dirs.len(),
            t.elapsed().as_millis()
        );
        assert!(
            dirs.is_empty(),
            "idle-stopped session kept its socket dir: {dirs:?}"
        );
        assert_eq!(wait_gone(&m, Duration::from_secs(3)), 0);
        assert!(TcpStream::connect(("127.0.0.1", pa)).is_err());
        // Both MAX_SESSIONS slots are free again.
        mgr.start(&ws, &b, &tree, &spec(&["{port}"]), &guard(&epoch))
            .unwrap();
        mgr.start(&ws, &c, &tree, &spec(&["{port}"]), &guard(&epoch))
            .expect("an idle-stopped session must not hold a session slot");
        assert_eq!(svc_dirs(scratch.path()).len(), 2);
        // Views and the final record survive until stop collects them.
        let st = mgr.status(&a);
        assert_eq!(st.state, PreviewState::IdleStopped);
        assert!(!st.running && st.bridge_port.is_none());
        assert!(mgr
            .requests(&a, 0, u32::MAX)
            .iter()
            .any(|r| r.path == "/api/trees?before-idle=1" && r.status == Some(200)));
        assert!(mgr.logs(&a, 0, u32::MAX).retained_bytes > 0);
        assert_eq!(
            mgr.start(&ws, &a, &tree, &idle, &guard(&epoch)).err(),
            Some(PreviewError::AlreadyRunning)
        );
        let rec = mgr.stop(&a, StopReasonKind::Idle).unwrap();
        assert_eq!(rec.service_id, sa.descriptor.service_id);
        assert_eq!(rec.reason, StopReasonKind::Idle);
        assert!(rec.report.killed && rec.report.descendants_alive_after == 0);
        assert!(rec.socket_dir_removed && rec.bridge_closed);
        assert!(rec.requests.iter().any(|r| r.status == Some(200)));
        assert!(!serde_json::to_string(&rec).unwrap().contains(&token_a));
        assert!(mgr.stop(&a, StopReasonKind::Idle).is_none());
        assert_eq!(mgr.status(&a).state, PreviewState::NotStarted);
        mgr.stop_all(StopReasonKind::Locked);
        assert!(svc_dirs(scratch.path()).is_empty());
    }
}
