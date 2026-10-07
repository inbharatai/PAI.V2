//! Stage 5 observable coding workspace: gate mode (controller-built Python
//! commands over a staged copy) and service mode (a managed dev server reached
//! through a reverse-dial relay). Linux only; every other target fails with
//! `IsolationUnavailable` BEFORE any `Command` is built.
//!
//! Containment is the Stage 4 flag family (setpriv drop caps + no_new_privs,
//! bwrap --unshare-all --unshare-user --disable-userns, read-only `/` and `/dev`,
//! 16 MiB `/tmp`, prlimit, inherited seccomp SysV-IPC/x32/foreign-arch deny, no
//! `/proc`) with exactly two differences: a size-bounded writable `/work` tmpfs
//! and the staged tree bound read-only at `/src` (service mode additionally binds
//! the controller socket directory read-only at `/run/pai-preview`).
//!
//! Honest limits: per-process rlimits plus a host-side SOFT aggregate RSS
//! watchdog (sampled every 250 ms from host `/proc`); there is NO cgroup quota
//! (`WorkspaceProfile::cgroup_quota == false`). Generated code can only produce
//! (a) bounded hex bytes inside the trusted supervisor's receipt/frames and
//! (b) loopback HTTP through the relay. It never writes a host tree.

use super::{
    admission, bounded_wait_capped, hash, io, json, log_hash, nonce, program_hash, readonly,
    relative, writable, ActualRun, Cancellation, IsolationError, LinuxIsolation, Result,
    RuntimeProfile, SnapshotPolicy, Termination, BWRAP, PRLIMIT, PYTHON, SETPRIV,
    WRITABLE_TMPFS_BYTES,
};
use crate::knowledge_retrieval::TrustedSource;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

pub const WORKSPACE_PROFILE: &str =
    "linux-dropcaps-bwrap-no-proc-rofs-noipc-nouserns-prlimit-workspace-v1";
pub const SERVICE_PROFILE: &str =
    "linux-dropcaps-bwrap-no-proc-rofs-noipc-nouserns-prlimit-service-v1";
pub const SELECTION_SCHEMA: &str = "inbharat.pai.stage5.selection.v1";
pub const GATE_PLAN_SCHEMA: &str = "inbharat.pai.stage5.gate-plan.v1";
pub const SERVICE_SPEC_SCHEMA: &str = "inbharat.pai.stage5.service-spec.v1";
pub const WORKSPACE_PROFILE_SCHEMA: &str = "pai.workspace-profile.v1";
/// Upper bound of the per-run `/work` tmpfs; the readiness probe asserts a
/// `/work` of exactly this size and each plan/spec picks `work_tmpfs_bytes <= W`.
pub const MAX_WORK_TMPFS_BYTES: u64 = 64 * MIB;
/// Mount point of the controller socket directory inside service sandboxes.
pub const IN_SANDBOX_SOCKET_DIR: &str = "/run/pai-preview";

const MIB: u64 = 1024 * 1024;
const MAX_TREE_FILES: usize = 32;
const MAX_TREE_FILE_BYTES: usize = 256 * 1024;
const MAX_TREE_TOTAL_BYTES: usize = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_SELECTION_FILES: usize = 16;
const MAX_SELECTION_TOTAL: usize = 1024 * 1024;
// read_metadata is consumed by task_workspace (owner B) after the Pass-2 merge.
#[cfg_attr(any(not(test), not(target_os = "linux")), allow(dead_code))]
const MAX_METADATA_FILES: usize = 4;
#[cfg_attr(any(not(test), not(target_os = "linux")), allow(dead_code))]
const MAX_METADATA_BYTES: usize = 4096;
const MAX_COMMANDS: usize = 8;
const MAX_ARGS: usize = 16;
const MAX_ARG_BYTES: usize = 256;
const MAX_COMPILE_FILES: usize = 32;
const MIN_OUTPUT_BYTES: u32 = 64;
const MAX_OUTPUT_BYTES: u32 = 64 * 1024;
const MAX_COPY_FILES: u32 = 64;
const MAX_COPY_TOTAL: u64 = MIB;
const MAX_COPY_FILE: u64 = 256 * 1024;
const MAX_IGNORES: usize = 16;
const MAX_REJECTED: usize = 64;
const MAX_REJECTED_NAME: usize = 512;
const MAX_REQUEST_BYTES: usize = 100_000;
const RECEIPT_MAX: usize = 4 * 1024 * 1024;
const RECEIPT_OVERHEAD: usize = 128 * 1024;
const GATE_GRACE_MS: u64 = 5_000;
const WATCHDOG_PERIOD: Duration = Duration::from_millis(250);
const DEFAULT_KILL_GRACE: Duration = Duration::from_secs(2);
const MAX_LOG_RATE: u32 = 256 * 1024;
const MIN_LOG_RATE: u32 = 1024;

/// Canonical content identity used by StagedTree, WorkingSet and the ledger:
/// sha256(serde_json::to_vec(BTreeMap<relative path, lowercase sha256 of bytes>)).
pub fn manifest_sha256(manifest: &BTreeMap<String, String>) -> String {
    hash(&serde_json::to_vec(manifest).unwrap_or_default())
}

fn unavailable<E>(_: E) -> IsolationError {
    IsolationError::IsolationUnavailable
}
fn depth_ok(path: &str) -> bool {
    path.split('/').count() <= MAX_DEPTH
}

// ---------------------------------------------------------------- selection

/// "repo:" + sha256(canonical root path). A label, not an attestation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionLabel {
    pub source_id: String,
}
impl SelectionLabel {
    pub fn for_root(root: &Path) -> Result<Self> {
        let root = io(fs::canonicalize(root))?;
        Ok(Self {
            source_id: format!("repo:{}", hash(root.to_string_lossy().as_bytes())),
        })
    }
}
/// A Stage 4 snapshot of the selected files plus their captured bytes. The
/// snapshot's source_commit is manifest_sha256 of the selection (a CONTENT
/// identity of the selected bytes, NOT a Git commit attestation).
pub struct CapturedSelection {
    pub snapshot: super::ProjectSnapshot,
    pub files: BTreeMap<String, Vec<u8>>,
}

/// fd-safe copy-in of an explicit selection (phase 1 private fd admission,
/// phase 2 public SnapshotPolicy::capture, phase 3 binding == phase-1 manifest).
pub fn capture_selection(
    policy: &SnapshotPolicy,
    root: &Path,
    label: &SelectionLabel,
    primary: &str,
    files: &[String],
) -> Result<CapturedSelection> {
    let canonical = io(fs::canonicalize(root))?;
    let root_id = policy
        .roots
        .iter()
        .find(|(r, _)| *r == canonical)
        .map(|(_, id)| *id)
        .ok_or(IsolationError::DeniedRoot)?;
    let unique: BTreeSet<&String> = files.iter().collect();
    if files.is_empty()
        || files.len() > MAX_SELECTION_FILES
        || unique.len() != files.len()
        || !files.iter().any(|f| f == primary)
    {
        return Err(IsolationError::InvalidInput);
    }
    for f in files {
        relative(f)?;
    }
    let (root_fd, opened) = admission::open_dir(&canonical)?;
    if opened != root_id {
        return Err(IsolationError::DeniedRoot);
    }
    let mut bytes = BTreeMap::new();
    let mut total = 0usize;
    for f in files {
        let b = admission::read_beneath(&root_fd, root_id, f)?;
        total += b.len();
        if total > MAX_SELECTION_TOTAL {
            return Err(IsolationError::InvalidInput);
        }
        bytes.insert(f.clone(), b);
    }
    let manifest: BTreeMap<String, String> =
        bytes.iter().map(|(p, b)| (p.clone(), hash(b))).collect();
    let source = TrustedSource {
        source_id: label.source_id.clone(),
        source_version: SELECTION_SCHEMA.into(),
        source_commit: manifest_sha256(&manifest),
        file_digest: manifest[primary].clone(),
    };
    let snapshot = policy.capture(&canonical, source, primary, files)?;
    if snapshot.binding().files != manifest {
        return Err(IsolationError::HashMismatch);
    }
    Ok(CapturedSelection {
        snapshot,
        files: bytes,
    })
}

/// Bounded, fd-safe metadata read beneath an approved root (no staging, no
/// execution). Used for the unverified .git/HEAD branch label.
#[cfg_attr(any(not(test), not(target_os = "linux")), allow(dead_code))]
pub(crate) fn read_metadata(
    policy: &SnapshotPolicy,
    root: &Path,
    files: &[String],
) -> Result<BTreeMap<String, Vec<u8>>> {
    let canonical = io(fs::canonicalize(root))?;
    let root_id = policy
        .roots
        .iter()
        .find(|(r, _)| *r == canonical)
        .map(|(_, id)| *id)
        .ok_or(IsolationError::DeniedRoot)?;
    if files.is_empty() || files.len() > MAX_METADATA_FILES {
        return Err(IsolationError::InvalidInput);
    }
    let (root_fd, opened) = admission::open_dir(&canonical)?;
    if opened != root_id {
        return Err(IsolationError::DeniedRoot);
    }
    let mut out = BTreeMap::new();
    for f in files {
        relative(f)?;
        let b = admission::read_beneath(&root_fd, root_id, f)?;
        if b.len() > MAX_METADATA_BYTES || out.insert(f.clone(), b).is_some() {
            return Err(IsolationError::InvalidInput);
        }
    }
    Ok(out)
}

// -------------------------------------------------------------- staged tree

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeLimits {
    pub max_files: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
}
impl Default for TreeLimits {
    fn default() -> Self {
        Self {
            max_files: MAX_TREE_FILES,
            max_file_bytes: MAX_TREE_FILE_BYTES,
            max_total_bytes: MAX_TREE_TOTAL_BYTES,
        }
    }
}
impl TreeLimits {
    fn validate(&self) -> Result<()> {
        if !(1..=MAX_TREE_FILES).contains(&self.max_files)
            || !(1..=MAX_TREE_FILE_BYTES).contains(&self.max_file_bytes)
            || !(1..=MAX_TREE_TOTAL_BYTES).contains(&self.max_total_bytes)
        {
            return Err(IsolationError::InvalidInput);
        }
        Ok(())
    }
}

/// Private read-only staged copy built by the CONTROLLER from working-set bytes
/// (0700 directory, files created with create_new and mode 0400, directory
/// identity (st_dev, st_ino) pinned). Bound read-only at /src.
pub struct StagedTree {
    dir: PathBuf,
    dir_id: admission::DirId,
    manifest: BTreeMap<String, String>,
    sizes: BTreeMap<String, u64>,
    tree_sha256: String,
}
impl StagedTree {
    pub fn from_files(
        scratch: &Path,
        files: &BTreeMap<String, Vec<u8>>,
        limits: &TreeLimits,
    ) -> Result<Self> {
        limits.validate()?;
        if files.is_empty() || files.len() > limits.max_files {
            return Err(IsolationError::InvalidInput);
        }
        let mut total = 0usize;
        for (path, bytes) in files {
            relative(path)?;
            total += bytes.len();
            if !depth_ok(path)
                || bytes.len() > limits.max_file_bytes
                || total > limits.max_total_bytes
                // A file name may not also be a directory prefix of another file.
                || files.keys().any(|o| o.starts_with(&format!("{path}/")))
            {
                return Err(IsolationError::InvalidInput);
            }
        }
        let scratch = io(fs::canonicalize(scratch))?;
        let dir = scratch.join(format!("pai-stage-{}", nonce()?));
        make_private_dir(&dir)?;
        let dir_id = match admission::open_dir(&dir) {
            Ok((_, id)) => id,
            Err(e) => {
                let _ = fs::remove_dir(&dir);
                return Err(e);
            }
        };
        let mut tree = StagedTree {
            dir,
            dir_id,
            manifest: BTreeMap::new(),
            sizes: BTreeMap::new(),
            tree_sha256: String::new(),
        };
        for (path, bytes) in files {
            let target = tree.dir.join(path);
            let parent = target.parent().ok_or(IsolationError::InvalidInput)?;
            make_private_dirs(parent)?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o400);
            }
            io(io(options.open(&target))?.write_all(bytes))?;
            tree.manifest.insert(path.clone(), hash(bytes));
            tree.sizes.insert(path.clone(), bytes.len() as u64);
        }
        tree.tree_sha256 = manifest_sha256(&tree.manifest);
        readonly(&tree.dir)?;
        tree.validate()?;
        Ok(tree)
    }
    /// == manifest_sha256(manifest)
    pub fn tree_sha256(&self) -> &str {
        &self.tree_sha256
    }
    pub fn manifest(&self) -> &BTreeMap<String, String> {
        &self.manifest
    }
    /// fd-relative re-hash of every staged file beneath the pinned directory.
    pub fn validate(&self) -> Result<()> {
        let (dir, id) = admission::open_dir(&self.dir)?;
        if id != self.dir_id {
            return Err(IsolationError::HashMismatch);
        }
        for (path, digest) in &self.manifest {
            if hash(&admission::read_beneath(&dir, id, path)?) != *digest {
                return Err(IsolationError::HashMismatch);
            }
        }
        if manifest_sha256(&self.manifest) != self.tree_sha256 {
            return Err(IsolationError::HashMismatch);
        }
        Ok(())
    }
    fn total_bytes(&self) -> u64 {
        self.sizes.values().sum()
    }
    fn largest_file(&self) -> u64 {
        self.sizes.values().copied().max().unwrap_or(0)
    }
    fn is_file(&self, path: &str) -> bool {
        self.manifest.contains_key(path)
    }
    fn is_dir_prefix(&self, dir: &str) -> bool {
        dir.is_empty()
            || (relative(dir).is_ok()
                && self
                    .manifest
                    .keys()
                    .any(|p| p.starts_with(&format!("{dir}/"))))
    }
}
impl Drop for StagedTree {
    fn drop(&mut self) {
        let _ = writable(&self.dir);
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn make_private_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        io(fs::DirBuilder::new().mode(0o700).create(dir))
    }
    #[cfg(not(unix))]
    {
        io(fs::create_dir(dir))
    }
}
fn make_private_dirs(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        io(fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(dir))
    }
    #[cfg(not(unix))]
    {
        io(fs::create_dir_all(dir))
    }
}

// --------------------------------------------------------------- gate types

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLimits {
    /// RLIMIT_CPU per process: 1..=120 (gate); 1..=600 (service).
    pub cpu_seconds: u32,
    /// RLIMIT_AS per process, 64 MiB..=512 MiB.
    pub memory_bytes: u64,
    /// RLIMIT_NPROC, 8..=64.
    pub processes: u32,
    /// The /work tmpfs, 8..=64 MiB (page multiple, >= staged bytes).
    pub work_tmpfs_bytes: u64,
    /// RLIMIT_FSIZE, 256 KiB..=4 MiB (>= largest staged file).
    pub file_size_bytes: u64,
    /// RLIMIT_NOFILE, 32..=256.
    pub open_files: u32,
    /// Gate 100..=600_000; service lifetime 1_000..=3_600_000.
    pub total_timeout_ms: u64,
    /// Host-side SOFT aggregate RSS kill threshold, 64 MiB..=2 GiB.
    pub rss_watchdog_bytes: u64,
}
impl WorkspaceLimits {
    fn validate(&self, service: bool) -> Result<()> {
        let (cpu, total) = if service {
            (1..=600, 1_000..=3_600_000)
        } else {
            (1..=120, 100..=600_000)
        };
        if !cpu.contains(&self.cpu_seconds)
            || !(64 * MIB..=512 * MIB).contains(&self.memory_bytes)
            || !(8..=64).contains(&self.processes)
            || !(8 * MIB..=MAX_WORK_TMPFS_BYTES).contains(&self.work_tmpfs_bytes)
            || !self.work_tmpfs_bytes.is_multiple_of(4096)
            || !(256 * 1024..=4 * MIB).contains(&self.file_size_bytes)
            || !(32..=256).contains(&self.open_files)
            || !total.contains(&self.total_timeout_ms)
            || !(64 * MIB..=2048 * MIB).contains(&self.rss_watchdog_bytes)
        {
            return Err(IsolationError::InvalidInput);
        }
        Ok(())
    }
    fn fits(&self, tree: &StagedTree) -> Result<()> {
        if self.file_size_bytes < tree.largest_file()
            || self.work_tmpfs_bytes < tree.total_bytes() + MIB
        {
            return Err(IsolationError::InvalidInput);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum GateCommand {
    PythonScript {
        id: String,
        role: GateRole,
        script: String,
        args: Vec<String>,
        timeout_ms: u64,
        output_bytes: u32,
    },
    PythonUnittest {
        id: String,
        role: GateRole,
        start_dir: String,
        pattern: String,
        timeout_ms: u64,
        output_bytes: u32,
    },
    PythonCompile {
        id: String,
        role: GateRole,
        files: Vec<String>,
        timeout_ms: u64,
        output_bytes: u32,
    },
}
impl GateCommand {
    pub fn id(&self) -> &str {
        match self {
            Self::PythonScript { id, .. }
            | Self::PythonUnittest { id, .. }
            | Self::PythonCompile { id, .. } => id,
        }
    }
    pub fn role(&self) -> GateRole {
        match self {
            Self::PythonScript { role, .. }
            | Self::PythonUnittest { role, .. }
            | Self::PythonCompile { role, .. } => *role,
        }
    }
    fn timeout_ms(&self) -> u64 {
        match self {
            Self::PythonScript { timeout_ms, .. }
            | Self::PythonUnittest { timeout_ms, .. }
            | Self::PythonCompile { timeout_ms, .. } => *timeout_ms,
        }
    }
    fn output_bytes(&self) -> u32 {
        match self {
            Self::PythonScript { output_bytes, .. }
            | Self::PythonUnittest { output_bytes, .. }
            | Self::PythonCompile { output_bytes, .. } => *output_bytes,
        }
    }
}
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateRole {
    Build,
    Test,
    Lint,
    Oracle,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatePlan {
    pub schema: String,
    pub commands: Vec<GateCommand>,
    pub stop_on_failure: bool,
    pub copy_out: CopyOutRequest,
    pub limits: WorkspaceLimits,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyOutMode {
    Off,
    HashesOnly,
    Contents,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyOutRequest {
    pub mode: CopyOutMode,
    pub ignore_dir_names: BTreeSet<String>,
    pub max_files: u32,
    pub max_total_bytes: u64,
}
impl CopyOutRequest {
    /// {"__pycache__", ".pytest_cache"}
    pub fn default_ignores() -> BTreeSet<String> {
        ["__pycache__", ".pytest_cache"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfile {
    pub schema: String,
    pub base: RuntimeProfile,
    pub workspace_backend: String,
    pub service_backend: String,
    pub supervisor_v3_sha256: String,
    pub runner_sha256: String,
    /// Exactly {"/tmp": 16777216, "/work": MAX_WORK_TMPFS_BYTES (upper bound;
    /// the per-run size is the plan/spec `work_tmpfs_bytes`, bound by its hash)}.
    pub writable: BTreeMap<String, u64>,
    pub proc_mounted: bool,
    /// Always false: no cgroup quota; only rlimits + a soft RSS watchdog.
    pub cgroup_quota: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateRunResult {
    pub workspace_profile_sha256: String,
    pub tree_sha256: String,
    pub plan_sha256: String,
    pub commands: Vec<CommandResult>,
    pub skipped: Vec<String>,
    pub copy_out: CopyOutReport,
    pub termination: Termination,
    pub watchdog: Option<WatchdogEvent>,
    pub elapsed_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandResult {
    pub id: String,
    pub role: GateRole,
    pub argv: Vec<String>,
    pub status: Option<i32>,
    pub termination: Termination,
    pub stdout: BoundedOutput,
    pub stderr: BoundedOutput,
    pub log_sha256: String,
    pub elapsed_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundedOutput {
    pub retained: Vec<u8>,
    pub total_bytes: u64,
    pub truncated: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CopyOutReport {
    pub changed: Vec<ReportedFile>,
    pub created: Vec<ReportedFile>,
    pub deleted: Vec<String>,
    /// Display names (<=512 bytes) with the reason; never used as host paths.
    /// Names that are not canonical UTF-8 relative paths are reported as
    /// `Special`; a walk beyond 4096 entries is the single entry ("*", OverCount).
    pub rejected: Vec<(String, CopyOutReject)>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportedFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub bytes: Option<Vec<u8>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyOutReject {
    Symlink,
    Special,
    Oversize,
    OverCount,
    Ignored,
}
/// Host-side soft watchdog kill (NOT a quota). Termination is `Cancelled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchdogEvent {
    pub rss_bytes: u64,
    pub threshold_bytes: u64,
    pub processes: u32,
    pub at_ms: u64,
}

// ------------------------------------------------------------ service types

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceSpec {
    pub schema: String,
    pub command: ServiceCommand,
    /// In-sandbox loopback port, 1024..=65535.
    pub listen_port: u16,
    /// Idle reverse-dial tunnels, 1..=4.
    pub tunnels_pool: u8,
    /// Idle + active tunnels, pool..=8.
    pub tunnels_max: u8,
    pub limits: WorkspaceLimits,
    /// Supervisor-side log rate limit, 1 KiB/s..=256 KiB/s.
    pub log_rate_bytes_per_s: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum ServiceCommand {
    /// `args` may contain the literal placeholder "{port}".
    PythonScript { script: String, args: Vec<String> },
    /// http.server (single-threaded HTTPServer) over /work/<dir> ("" = /work).
    PythonStaticServer { dir: String },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceEvent {
    Log {
        stream: Stream,
        bytes: Vec<u8>,
    },
    /// Bytes/records dropped before reaching the host (supervisor rate limiter
    /// or host event-queue bound).
    Dropped {
        bytes: u64,
        records: u64,
    },
    /// `None` when the host killed the sandbox (stop/cancel/lifetime/watchdog).
    Exited {
        status: Option<i32>,
    },
    RunnerFailure,
    Watchdog(WatchdogEvent),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopReport {
    pub killed: bool,
    pub exit_status: Option<i32>,
    pub descendants_alive_after: u32,
    pub elapsed_ms: u64,
}

/// Controller socket directory: 0700 dir in controller scratch, host-LISTENING
/// `ctl.sock` (0600). Bound read-only into the service sandbox.
pub struct ServiceSocketDir {
    #[cfg(target_os = "linux")]
    path: PathBuf,
    #[cfg(target_os = "linux")]
    dir_id: admission::DirId,
    #[cfg(target_os = "linux")]
    listener: std::os::unix::net::UnixListener,
}
impl ServiceSocketDir {
    pub fn create(scratch: &Path) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            let scratch = io(fs::canonicalize(scratch))?;
            let path = scratch.join(format!("pai-svc-{}", nonce()?));
            // sun_path is 108 bytes; keep scratch short.
            if path.join("ctl.sock").as_os_str().len() >= 100 {
                return Err(IsolationError::InvalidInput);
            }
            make_private_dir(&path)?;
            let built = (|| {
                let (_, dir_id) = admission::open_dir(&path)?;
                let sock = path.join("ctl.sock");
                let listener = io(std::os::unix::net::UnixListener::bind(&sock))?;
                io(fs::set_permissions(
                    &sock,
                    fs::Permissions::from_mode(0o600),
                ))?;
                Ok((dir_id, listener))
            })();
            match built {
                Ok((dir_id, listener)) => Ok(Self {
                    path,
                    dir_id,
                    listener,
                }),
                Err(e) => {
                    let _ = fs::remove_dir_all(&path);
                    Err(e)
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = scratch;
            Err(IsolationError::IsolationUnavailable)
        }
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn listener(&self) -> &std::os::unix::net::UnixListener {
        &self.listener
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    #[cfg(target_os = "linux")]
    fn validate(&self) -> Result<()> {
        let (_, id) = admission::open_dir(&self.path)?;
        if id != self.dir_id {
            return Err(IsolationError::HashMismatch);
        }
        Ok(())
    }
}
#[cfg(target_os = "linux")]
impl Drop for ServiceSocketDir {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.path.join("ctl.sock"));
        let _ = fs::remove_dir(&self.path);
    }
}

/// One reverse-dialled, hello-authenticated connection to the in-sandbox relay.
#[cfg(target_os = "linux")]
pub(crate) struct Tunnel(std::os::unix::net::UnixStream);
#[cfg(not(target_os = "linux"))]
pub(crate) enum Tunnel {}
#[cfg(target_os = "linux")]
impl Tunnel {
    pub(crate) fn set_timeouts(
        &self,
        read: Option<Duration>,
        write: Option<Duration>,
    ) -> std::io::Result<()> {
        self.0.set_read_timeout(read)?;
        self.0.set_write_timeout(write)
    }
}
#[cfg(not(target_os = "linux"))]
impl Tunnel {
    pub(crate) fn set_timeouts(
        &self,
        _: Option<Duration>,
        _: Option<Duration>,
    ) -> std::io::Result<()> {
        match *self {}
    }
}
impl Read for Tunnel {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(target_os = "linux")]
        {
            self.0.read(buf)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = buf;
            match *self {}
        }
    }
}
impl Write for Tunnel {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        #[cfg(target_os = "linux")]
        {
            self.0.write(buf)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = buf;
            match *self {}
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.0.flush()
        }
        #[cfg(not(target_os = "linux"))]
        {
            match *self {}
        }
    }
}

#[derive(Default)]
struct EventQueue {
    q: VecDeque<ServiceEvent>,
    log_bytes: usize,
    dropped_bytes: u64,
    dropped_records: u64,
}
impl EventQueue {
    #[cfg(target_os = "linux")]
    fn push_log(&mut self, stream: Stream, bytes: Vec<u8>) {
        if self.log_bytes + bytes.len() > svc::EVENT_QUEUE_LOG_BYTES {
            self.dropped_bytes += bytes.len() as u64;
            self.dropped_records += 1;
        } else {
            self.log_bytes += bytes.len();
            self.q.push_back(ServiceEvent::Log { stream, bytes });
        }
    }
    fn pop(&mut self, max: usize) -> Vec<ServiceEvent> {
        let mut out = Vec::new();
        if max == 0 {
            return out;
        }
        if self.dropped_bytes > 0 || self.dropped_records > 0 {
            out.push(ServiceEvent::Dropped {
                bytes: self.dropped_bytes,
                records: self.dropped_records,
            });
            self.dropped_bytes = 0;
            self.dropped_records = 0;
        }
        while out.len() < max {
            match self.q.pop_front() {
                Some(e) => {
                    if let ServiceEvent::Log { bytes, .. } = &e {
                        self.log_bytes -= bytes.len();
                    }
                    out.push(e);
                }
                None => break,
            }
        }
        out
    }
}
struct ServiceShared {
    id: String,
    listen_port: u16,
    stop: AtomicBool,
    running: AtomicBool,
    grace_ms: AtomicU64,
    events: Mutex<EventQueue>,
    pool: Mutex<VecDeque<Tunnel>>,
    pool_cv: Condvar,
    report: Mutex<Option<StopReport>>,
}
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Crate-internal shared view of a running service (event drain, tunnels).
#[derive(Clone)]
pub(crate) struct ServiceLink {
    shared: Arc<ServiceShared>,
}
impl ServiceLink {
    pub(crate) fn id(&self) -> &str {
        &self.shared.id
    }
    pub(crate) fn listen_port(&self) -> u16 {
        self.shared.listen_port
    }
    pub(crate) fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }
    pub(crate) fn poll_events(&self, max: usize) -> Vec<ServiceEvent> {
        lock(&self.shared.events).pop(max)
    }
    /// Pop an idle authenticated tunnel, waiting at most `wait` (pool
    /// exhaustion returns None; the bridge answers 503, it never hangs).
    pub(crate) fn take_tunnel(&self, wait: Duration) -> Option<Tunnel> {
        let deadline = Instant::now() + wait;
        let mut pool = lock(&self.shared.pool);
        loop {
            if let Some(t) = pool.pop_front() {
                return Some(t);
            }
            let now = Instant::now();
            if !self.is_running() || now >= deadline {
                return None;
            }
            pool = self
                .shared
                .pool_cv
                .wait_timeout(pool, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// Owned service: a dedicated owner thread that lives as long as this handle
/// holds the `std::process::Child` (never a stored PID).
pub struct ServiceHandle {
    link: ServiceLink,
    owner: Option<thread::JoinHandle<()>>,
}
impl ServiceHandle {
    pub fn id(&self) -> &str {
        self.link.id()
    }
    pub fn poll_events(&self, max: usize) -> Vec<ServiceEvent> {
        self.link.poll_events(max)
    }
    pub fn is_running(&self) -> bool {
        self.link.is_running()
    }
    /// Kill the namespace via the owned Child, wait, then check host /proc for
    /// surviving descendants for at most `grace`.
    pub fn stop(mut self, grace: Duration) -> StopReport {
        let start = Instant::now();
        let mut report = self.shutdown(grace);
        report.elapsed_ms = start.elapsed().as_millis() as u64;
        report
    }
    pub(crate) fn link(&self) -> ServiceLink {
        self.link.clone()
    }
    fn shutdown(&mut self, grace: Duration) -> StopReport {
        let shared = &self.link.shared;
        shared
            .grace_ms
            .store(grace.as_millis() as u64, Ordering::SeqCst);
        shared.stop.store(true, Ordering::SeqCst);
        if let Some(owner) = self.owner.take() {
            let _ = owner.join();
        }
        lock(&shared.report).take().unwrap_or(StopReport {
            killed: false,
            exit_status: None,
            descendants_alive_after: 0,
            elapsed_ms: 0,
        })
    }
}
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        if self.owner.is_some() {
            let _ = self.shutdown(DEFAULT_KILL_GRACE);
        }
    }
}

// ------------------------------------------------------------ the backend

pub struct WorkspaceIsolation {
    profile: WorkspaceProfile,
    #[cfg(test)]
    receipt_cap_override: Option<usize>,
}
impl WorkspaceIsolation {
    /// Linux only. Requires a constructed LinuxIsolation (Stage 4 readiness +
    /// program hashes), then runs its OWN readiness probes for the gate and the
    /// service mount plans. Non-Linux: IsolationUnavailable before any Command.
    pub fn new(base: &LinuxIsolation) -> Result<Self> {
        if !cfg!(target_os = "linux") {
            return Err(IsolationError::IsolationUnavailable);
        }
        let mut writable_set = BTreeMap::new();
        writable_set.insert("/tmp".to_string(), WRITABLE_TMPFS_BYTES);
        writable_set.insert("/work".to_string(), MAX_WORK_TMPFS_BYTES);
        let ws = Self {
            profile: WorkspaceProfile {
                schema: WORKSPACE_PROFILE_SCHEMA.into(),
                base: base.profile().clone(),
                workspace_backend: WORKSPACE_PROFILE.into(),
                service_backend: SERVICE_PROFILE.into(),
                supervisor_v3_sha256: hash(SUPERVISOR_V3.as_bytes()),
                runner_sha256: hash(RUNNER.as_bytes()),
                writable: writable_set,
                proc_mounted: false,
                cgroup_quota: false,
            },
            #[cfg(test)]
            receipt_cap_override: None,
        };
        ws.preflight()?;
        ws.readiness_probe(false)?;
        ws.readiness_probe(true)?;
        Ok(ws)
    }
    pub fn profile(&self) -> &WorkspaceProfile {
        &self.profile
    }
    pub fn profile_sha256(&self) -> String {
        hash(&serde_json::to_vec(&self.profile).unwrap_or_default())
    }
    /// Re-hash the four programs against the profile and the trusted constants.
    pub fn preflight(&self) -> Result<()> {
        let hashes = &self.profile.base.program_hashes;
        if !cfg!(target_os = "linux") || hashes.len() != 4 {
            return Err(IsolationError::IsolationUnavailable);
        }
        for p in [SETPRIV, BWRAP, PRLIMIT, PYTHON] {
            if hashes.get(p) != Some(&program_hash(p)?) {
                return Err(IsolationError::IsolationUnavailable);
            }
        }
        if self.profile.supervisor_v3_sha256 != hash(SUPERVISOR_V3.as_bytes())
            || self.profile.runner_sha256 != hash(RUNNER.as_bytes())
            || self.profile.cgroup_quota
            || self.profile.proc_mounted
        {
            return Err(IsolationError::IsolationUnavailable);
        }
        Ok(())
    }
    /// The ONLY place generated-code processes are built: setpriv -> bwrap ->
    /// prlimit -> python3. Shared mount plan for gate and service mode.
    fn command(
        &self,
        tree_dir: &Path,
        sock_dir: Option<&Path>,
        limits: &WorkspaceLimits,
        work_bytes: u64,
    ) -> Command {
        let mut c = Command::new(SETPRIV);
        c.env_clear().stdin(Stdio::null());
        c.args([
            "--no-new-privs",
            "--inh-caps=-all",
            "--ambient-caps=-all",
            "--bounding-set=-all",
            BWRAP,
            "--unshare-all",
            "--unshare-user",
            "--disable-userns",
            "--die-with-parent",
            "--new-session",
            "--ro-bind",
            "/usr",
            "/usr",
            "--ro-bind",
            "/lib",
            "/lib",
            "--ro-bind",
            "/lib64",
            "/lib64",
            "--dev",
            "/dev",
            "--remount-ro",
            "/dev",
        ]);
        c.arg("--size")
            .arg(WRITABLE_TMPFS_BYTES.to_string())
            .args(["--tmpfs", "/tmp"]);
        // Second (and last) writable filesystem.
        c.arg("--size")
            .arg(work_bytes.to_string())
            .args(["--tmpfs", "/work"]);
        c.arg("--ro-bind").arg(tree_dir).arg("/src");
        if let Some(sock) = sock_dir {
            c.arg("--ro-bind").arg(sock).arg(IN_SANDBOX_SOCKET_DIR);
        }
        c.args([
            "--remount-ro",
            "/",
            "--chdir",
            "/work",
            "--clearenv",
            "--setenv",
            "PATH",
            "/usr/bin",
            "--setenv",
            "HOME",
            "/tmp",
            "--setenv",
            "MALLOC_ARENA_MAX",
            "2",
            "--",
            PRLIMIT,
        ]);
        c.arg(format!("--cpu={0}:{0}", limits.cpu_seconds))
            .arg(format!("--as={0}:{0}", limits.memory_bytes))
            .arg(format!("--nproc={0}:{0}", limits.processes))
            .arg(format!("--fsize={0}:{0}", limits.file_size_bytes))
            .arg(format!("--nofile={0}:{0}", limits.open_files))
            .args(["--core=0:0", "--", PYTHON]);
        c
    }
    fn readiness_probe(&self, service: bool) -> Result<()> {
        let scratch = fs::canonicalize(std::env::temp_dir()).map_err(unavailable)?;
        let mut files = BTreeMap::new();
        files.insert("probe.txt".to_string(), b"probe".to_vec());
        let tree = StagedTree::from_files(&scratch, &files, &TreeLimits::default())
            .map_err(unavailable)?;
        let limits = WorkspaceLimits {
            cpu_seconds: 2,
            memory_bytes: 64 * MIB,
            processes: 16,
            work_tmpfs_bytes: MAX_WORK_TMPFS_BYTES,
            file_size_bytes: 256 * 1024,
            open_files: 32,
            total_timeout_ms: 3000,
            rss_watchdog_bytes: 64 * MIB,
        };
        #[cfg(target_os = "linux")]
        let sock = if service {
            Some(ServiceSocketDir::create(&scratch).map_err(unavailable)?)
        } else {
            None
        };
        #[cfg(target_os = "linux")]
        let sock_path = sock.as_ref().map(|s| s.path().to_path_buf());
        #[cfg(not(target_os = "linux"))]
        let sock_path: Option<PathBuf> = None;
        let mut cmd = self.command(
            &tree.dir,
            sock_path.as_deref(),
            &limits,
            MAX_WORK_TMPFS_BYTES,
        );
        let work = MAX_WORK_TMPFS_BYTES.to_string();
        let mode = if service { "service" } else { "gate" };
        cmd.args(["-I", "-c", PROBE, &work, mode])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = spawn(&mut cmd)?;
        #[cfg(target_os = "linux")]
        if let Some(s) = &sock {
            svc::probe_echo(s.listener(), Duration::from_secs(3));
        }
        let probe = bounded_wait_capped(
            child,
            Duration::from_secs(4),
            &Cancellation::default(),
            4096,
        )?;
        if probe.0 != Termination::Completed || probe.1 != Some(0) || probe.2 != b"READY\n" {
            return Err(IsolationError::IsolationUnavailable);
        }
        Ok(())
    }

    /// Synchronous gate run over the staged tree. Pre-spawn: preflight, plan +
    /// limits validation, tree.validate(). Post-run: strict receipt parse,
    /// argv equality, hex/hash bounds, empty supervisor stderr, tree.validate(),
    /// host-side log_sha256. Any violation => RunnerFailure, no results trusted.
    pub fn run_gate(
        &self,
        tree: &StagedTree,
        plan: &GatePlan,
        cancel: &Cancellation,
    ) -> Result<GateRunResult> {
        if !cfg!(target_os = "linux") {
            return Err(IsolationError::IsolationUnavailable);
        }
        self.preflight()?;
        let argvs = validate_plan(plan, tree)?;
        tree.validate()?;
        let request = serde_json::json!({
            "mode": "gate",
            "program_sha256": self.profile.base.program_hashes[PYTHON],
            "manifest": tree.manifest,
            "copy_in_max_bytes": tree.total_bytes(),
            "commands": plan.commands.iter().zip(&argvs).map(|(c, argv)| serde_json::json!({
                "id": c.id(), "role": c.role(), "argv": argv, "timeout_ms": c.timeout_ms(), "output_bytes": c.output_bytes(),
            })).collect::<Vec<_>>(),
            "stop_on_failure": plan.stop_on_failure,
            "total_timeout_ms": plan.limits.total_timeout_ms,
            "copy_out": {
                "mode": plan.copy_out.mode,
                "ignore": plan.copy_out.ignore_dir_names,
                "max_files": plan.copy_out.max_files,
                "max_total_bytes": plan.copy_out.max_total_bytes,
                "max_file_bytes": MAX_COPY_FILE,
            },
        })
        .to_string();
        if request.len() > MAX_REQUEST_BYTES {
            return Err(IsolationError::InvalidInput);
        }
        let cap = self.receipt_cap(plan, request.len());
        let start = Instant::now();
        let mut result = GateRunResult {
            workspace_profile_sha256: self.profile_sha256(),
            tree_sha256: tree.tree_sha256.clone(),
            plan_sha256: hash(&json(plan)?),
            commands: Vec::new(),
            skipped: plan.commands.iter().map(|c| c.id().to_string()).collect(),
            copy_out: CopyOutReport::default(),
            termination: Termination::Cancelled,
            watchdog: None,
            elapsed_ms: 0,
        };
        if cancel.is_cancelled() {
            return Ok(result);
        }
        let mut cmd = self.command(&tree.dir, None, &plan.limits, plan.limits.work_tmpfs_bytes);
        cmd.args(["-I", "-c", SUPERVISOR_V3, &request])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = spawn(&mut cmd)?;
        let inner = Cancellation::default();
        let watchdog = Watchdog::start(
            child.id(),
            plan.limits.rss_watchdog_bytes,
            cancel.clone(),
            inner.clone(),
        );
        let waited = bounded_wait_capped(
            child,
            Duration::from_millis(plan.limits.total_timeout_ms + GATE_GRACE_MS),
            &inner,
            cap,
        );
        result.watchdog = watchdog.finish();
        let (termination, status, out, err) = waited?;
        result.termination = match termination {
            Termination::Completed if status == Some(0) && err.is_empty() => {
                match parse_gate_receipt(&out, plan, &argvs, &tree.manifest) {
                    Ok((commands, skipped, copy_out)) => {
                        result.commands = commands;
                        result.skipped = skipped;
                        result.copy_out = copy_out;
                        Termination::Completed
                    }
                    Err(_) => Termination::RunnerFailure,
                }
            }
            Termination::Cancelled => Termination::Cancelled,
            Termination::Timeout => Termination::Timeout,
            // Dead supervisor, non-empty supervisor stderr, receipt overflow.
            _ => Termination::RunnerFailure,
        };
        tree.validate()?;
        result.elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }
    /// min(4 MiB, 2 x (sum of output caps + copy-out cap) + 128 KiB + 2 x request).
    fn receipt_cap(&self, plan: &GatePlan, request_len: usize) -> usize {
        #[cfg(test)]
        if let Some(cap) = self.receipt_cap_override {
            return cap;
        }
        let outputs: u64 = plan
            .commands
            .iter()
            .map(|c| 2 * u64::from(c.output_bytes()))
            .sum();
        let copy = if plan.copy_out.mode == CopyOutMode::Contents {
            plan.copy_out.max_total_bytes
        } else {
            0
        };
        let cap = 2 * (outputs + copy) as usize + RECEIPT_OVERHEAD + 2 * request_len;
        cap.min(RECEIPT_MAX)
    }

    /// Start a managed service. The Child is spawned by, and owned by, a
    /// dedicated thread that lives as long as the returned handle.
    pub fn start_service(
        &self,
        tree: &StagedTree,
        spec: &ServiceSpec,
        sock: &ServiceSocketDir,
        cancel: Cancellation,
    ) -> Result<ServiceHandle> {
        if !cfg!(target_os = "linux") {
            return Err(IsolationError::IsolationUnavailable);
        }
        self.preflight()?;
        let argv = validate_service(spec, tree)?;
        tree.validate()?;
        let secret = nonce()?;
        let request = serde_json::json!({
            "mode": "service",
            "program_sha256": self.profile.base.program_hashes[PYTHON],
            "manifest": tree.manifest,
            "copy_in_max_bytes": tree.total_bytes(),
            "argv": argv,
            "port": spec.listen_port,
            "pool": spec.tunnels_pool,
            "max": spec.tunnels_max,
            "secret": secret,
            "rate": spec.log_rate_bytes_per_s,
            "lifetime_ms": spec.limits.total_timeout_ms,
        })
        .to_string();
        if request.len() > MAX_REQUEST_BYTES {
            return Err(IsolationError::InvalidInput);
        }
        let shared = Arc::new(ServiceShared {
            id: nonce()?,
            listen_port: spec.listen_port,
            stop: AtomicBool::new(false),
            running: AtomicBool::new(false),
            grace_ms: AtomicU64::new(DEFAULT_KILL_GRACE.as_millis() as u64),
            events: Mutex::new(EventQueue::default()),
            pool: Mutex::new(VecDeque::new()),
            pool_cv: Condvar::new(),
            report: Mutex::new(None),
        });
        let launch = Launch {
            request,
            secret,
            shared: shared.clone(),
            cancel,
        };
        let owner = self.spawn_owner(tree, spec, sock, launch)?;
        Ok(ServiceHandle {
            link: ServiceLink { shared },
            owner: Some(owner),
        })
    }
    #[cfg(target_os = "linux")]
    fn spawn_owner(
        &self,
        tree: &StagedTree,
        spec: &ServiceSpec,
        sock: &ServiceSocketDir,
        launch: Launch,
    ) -> Result<thread::JoinHandle<()>> {
        let Launch {
            request,
            secret,
            shared,
            cancel,
        } = launch;
        sock.validate()?;
        let listener = io(sock.listener().try_clone())?;
        let mut cmd = self.command(
            &tree.dir,
            Some(sock.path()),
            &spec.limits,
            spec.limits.work_tmpfs_bytes,
        );
        cmd.args(["-I", "-c", SUPERVISOR_V3, &request])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        svc::spawn_owner(
            cmd,
            listener,
            shared,
            svc::OwnerConfig {
                hello: format!("PAI1{secret}\n").into_bytes(),
                tunnels_max: usize::from(spec.tunnels_max),
                rss_threshold: spec.limits.rss_watchdog_bytes,
                lifetime: Duration::from_millis(spec.limits.total_timeout_ms + GATE_GRACE_MS),
            },
            cancel,
        )
    }
    #[cfg(not(target_os = "linux"))]
    fn spawn_owner(
        &self,
        _: &StagedTree,
        _: &ServiceSpec,
        _: &ServiceSocketDir,
        launch: Launch,
    ) -> Result<thread::JoinHandle<()>> {
        let _ = (launch.request, launch.secret, launch.shared, launch.cancel);
        Err(IsolationError::IsolationUnavailable)
    }
}
/// Everything the owner thread needs besides the command itself.
struct Launch {
    request: String,
    secret: String,
    shared: Arc<ServiceShared>,
    cancel: Cancellation,
}

// Test-only spawn counter, per thread: every spawn happens synchronously on
// the requesting thread, so a test asserting exact deltas on its own thread is
// unaffected by sandbox tests running in parallel.
#[cfg(test)]
thread_local! {
    static SPAWNS_HERE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
/// Spawns performed so far by the calling thread (Linux sandbox tests only).
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn spawns_here() -> u64 {
    SPAWNS_HERE.with(|c| c.get())
}
/// Single spawn point for every sandbox process (counted in tests).
fn spawn(cmd: &mut Command) -> Result<Child> {
    #[cfg(test)]
    SPAWNS_HERE.with(|c| c.set(c.get() + 1));
    cmd.spawn()
        .map_err(|_| IsolationError::IsolationUnavailable)
}

// ------------------------------------------------------ plan validation

fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn valid_args(args: &[String]) -> bool {
    args.len() <= MAX_ARGS
        && args
            .iter()
            .all(|a| a.len() <= MAX_ARG_BYTES && !a.contains('\0'))
}
fn work_path(rel: &str) -> String {
    if rel.is_empty() {
        "/work".into()
    } else {
        format!("/work/{rel}")
    }
}
fn runner_prefix(mode: &str) -> Vec<String> {
    [PYTHON, "-I", "-B", "-c", RUNNER, mode]
        .iter()
        .map(|s| s.to_string())
        .collect()
}
/// Validate a plan against the staged tree and build the controller argv of
/// every command. Model strings never become argv without these checks.
fn validate_plan(plan: &GatePlan, tree: &StagedTree) -> Result<Vec<Vec<String>>> {
    plan.limits.validate(false)?;
    plan.limits.fits(tree)?;
    let co = &plan.copy_out;
    if plan.schema != GATE_PLAN_SCHEMA
        || plan.commands.is_empty()
        || plan.commands.len() > MAX_COMMANDS
        || co.max_files > MAX_COPY_FILES
        || co.max_total_bytes > MAX_COPY_TOTAL
        || co.ignore_dir_names.len() > MAX_IGNORES
        || co
            .ignore_dir_names
            .iter()
            .any(|n| relative(n).is_err() || n.contains('/'))
    {
        return Err(IsolationError::InvalidInput);
    }
    let mut ids = BTreeSet::new();
    let mut argvs = Vec::new();
    for c in &plan.commands {
        if !valid_id(c.id())
            || !ids.insert(c.id().to_string())
            || !(100..=plan.limits.total_timeout_ms).contains(&c.timeout_ms())
            || !(MIN_OUTPUT_BYTES..=MAX_OUTPUT_BYTES).contains(&c.output_bytes())
        {
            return Err(IsolationError::InvalidInput);
        }
        let argv = match c {
            GateCommand::PythonScript { script, args, .. } => {
                if relative(script).is_err() || !tree.is_file(script) || !valid_args(args) {
                    return Err(IsolationError::InvalidInput);
                }
                let mut v = runner_prefix("script");
                v.push(work_path(script));
                v.extend(args.iter().cloned());
                v
            }
            GateCommand::PythonUnittest {
                start_dir, pattern, ..
            } => {
                let pattern_ok = (1..=64).contains(&pattern.len())
                    && pattern
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_*.-".contains(&b));
                if !tree.is_dir_prefix(start_dir) || !pattern_ok {
                    return Err(IsolationError::InvalidInput);
                }
                let mut v = runner_prefix("unittest");
                v.push(work_path(start_dir));
                v.push(pattern.clone());
                v
            }
            GateCommand::PythonCompile { files, .. } => {
                let unique: BTreeSet<&String> = files.iter().collect();
                if files.is_empty()
                    || files.len() > MAX_COMPILE_FILES
                    || unique.len() != files.len()
                    || files
                        .iter()
                        .any(|f| relative(f).is_err() || !tree.is_file(f))
                {
                    return Err(IsolationError::InvalidInput);
                }
                let mut v = runner_prefix("compile");
                v.extend(files.iter().map(|f| work_path(f)));
                v
            }
        };
        argvs.push(argv);
    }
    Ok(argvs)
}
fn validate_service(spec: &ServiceSpec, tree: &StagedTree) -> Result<Vec<String>> {
    spec.limits.validate(true)?;
    spec.limits.fits(tree)?;
    if spec.schema != SERVICE_SPEC_SCHEMA
        || spec.listen_port < 1024
        || !(1..=4).contains(&spec.tunnels_pool)
        || !(spec.tunnels_pool..=8).contains(&spec.tunnels_max)
        || !(MIN_LOG_RATE..=MAX_LOG_RATE).contains(&spec.log_rate_bytes_per_s)
    {
        return Err(IsolationError::InvalidInput);
    }
    let port = spec.listen_port.to_string();
    Ok(match &spec.command {
        ServiceCommand::PythonScript { script, args } => {
            if relative(script).is_err() || !tree.is_file(script) || !valid_args(args) {
                return Err(IsolationError::InvalidInput);
            }
            let mut v = runner_prefix("script");
            v.push(work_path(script));
            v.extend(args.iter().map(|a| a.replace("{port}", &port)));
            if !valid_args(&v[7..]) {
                return Err(IsolationError::InvalidInput);
            }
            v
        }
        ServiceCommand::PythonStaticServer { dir } => {
            if !tree.is_dir_prefix(dir) {
                return Err(IsolationError::InvalidInput);
            }
            let mut v = runner_prefix("static");
            v.push(work_path(dir));
            v.push(port);
            v
        }
    })
}

// --------------------------------------------------------- receipt parsing

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGate {
    commands: Vec<RawCommand>,
    skipped: Vec<String>,
    copy_out: RawCopyOut,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCommand {
    id: String,
    argv: Vec<String>,
    status: Option<i32>,
    termination: Termination,
    stdout_hex: String,
    stdout_total: u64,
    stderr_hex: String,
    stderr_total: u64,
    elapsed_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCopyOut {
    changed: Vec<RawFile>,
    created: Vec<RawFile>,
    deleted: Vec<String>,
    rejected: Vec<(String, CopyOutReject)>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    path: String,
    sha256: String,
    size: u64,
    hex: Option<String>,
}
fn unhex(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2)
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(IsolationError::InvalidInput);
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16))
        .collect::<std::result::Result<Vec<u8>, _>>()
        .map_err(|_| IsolationError::InvalidInput)
}
fn bounded(hexed: &str, total: u64, cap: u32) -> Result<BoundedOutput> {
    let retained = unhex(hexed)?;
    let expected = total.min(u64::from(cap));
    if retained.len() as u64 != expected {
        return Err(IsolationError::InvalidInput);
    }
    Ok(BoundedOutput {
        truncated: total > retained.len() as u64,
        retained,
        total_bytes: total,
    })
}
/// Strict host-side validation of the single supervisor receipt line.
fn parse_gate_receipt(
    out: &[u8],
    plan: &GatePlan,
    argvs: &[Vec<String>],
    manifest: &BTreeMap<String, String>,
) -> Result<(Vec<CommandResult>, Vec<String>, CopyOutReport)> {
    let bad = IsolationError::InvalidInput;
    let raw: RawGate = serde_json::from_slice(out).map_err(|_| bad.clone())?;
    let n = raw.commands.len();
    if n == 0 || n + raw.skipped.len() != plan.commands.len() {
        return Err(bad);
    }
    let expected_skipped: Vec<String> = plan.commands[n..]
        .iter()
        .map(|c| c.id().to_string())
        .collect();
    if raw.skipped != expected_skipped {
        return Err(bad);
    }
    let failed = |c: &RawCommand| c.termination != Termination::Completed || c.status != Some(0);
    let mut results = Vec::new();
    for (i, c) in raw.commands.iter().enumerate() {
        let planned = &plan.commands[i];
        if c.id != planned.id()
            || c.argv != argvs[i]
            || !matches!(c.termination, Termination::Completed | Termination::Timeout)
            || c.status.is_none()
            || c.elapsed_ms > plan.limits.total_timeout_ms + GATE_GRACE_MS
            || (plan.stop_on_failure && i + 1 < n && failed(c))
        {
            return Err(bad);
        }
        let stdout = bounded(&c.stdout_hex, c.stdout_total, planned.output_bytes())?;
        let stderr = bounded(&c.stderr_hex, c.stderr_total, planned.output_bytes())?;
        let mut run = ActualRun {
            argv: c.argv.clone(),
            status: c.status,
            termination: c.termination,
            stdout_hex: c.stdout_hex.clone(),
            stderr_hex: c.stderr_hex.clone(),
            log_sha256: String::new(),
            output_files: BTreeMap::new(),
            elapsed_ms: c.elapsed_ms,
        };
        run.log_sha256 = log_hash(&run)?;
        results.push(CommandResult {
            id: c.id.clone(),
            role: planned.role(),
            argv: c.argv.clone(),
            status: c.status,
            termination: c.termination,
            stdout,
            stderr,
            log_sha256: run.log_sha256,
            elapsed_ms: c.elapsed_ms,
        });
    }
    if !raw.skipped.is_empty() {
        let last = &raw.commands[n - 1];
        if !failed(last) || !(plan.stop_on_failure || last.termination == Termination::Timeout) {
            return Err(bad);
        }
    }
    let copy_out = validate_copy_out(raw.copy_out, &plan.copy_out, manifest)?;
    Ok((results, raw.skipped, copy_out))
}
fn validate_copy_out(
    raw: RawCopyOut,
    req: &CopyOutRequest,
    manifest: &BTreeMap<String, String>,
) -> Result<CopyOutReport> {
    let bad = IsolationError::InvalidInput;
    if req.mode == CopyOutMode::Off {
        if !raw.changed.is_empty()
            || !raw.created.is_empty()
            || !raw.deleted.is_empty()
            || !raw.rejected.is_empty()
        {
            return Err(bad);
        }
        return Ok(CopyOutReport::default());
    }
    if raw.changed.len() + raw.created.len() > req.max_files as usize
        || raw.rejected.len() > MAX_REJECTED + 1
        || raw.rejected.iter().any(|(name, _)| {
            name.is_empty() || name.len() > MAX_REJECTED_NAME || name.contains('\0')
        })
    {
        return Err(bad);
    }
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    let mut file = |f: RawFile, changed: bool| -> Result<ReportedFile> {
        relative(&f.path)?;
        let ignored = f.path.split('/').any(|c| req.ignore_dir_names.contains(c));
        let in_manifest = manifest.get(&f.path);
        if ignored
            || !seen.insert(f.path.clone())
            || !unoone_capability_contracts::knowledge::valid_digest(&f.sha256)
            || f.size > MAX_COPY_FILE
            || (changed && in_manifest.is_none_or(|h| *h == f.sha256))
            || (!changed && in_manifest.is_some())
        {
            return Err(IsolationError::InvalidInput);
        }
        let bytes = match (req.mode, f.hex) {
            (CopyOutMode::Contents, Some(h)) => {
                let b = unhex(&h)?;
                total += b.len() as u64;
                if b.len() as u64 != f.size || hash(&b) != f.sha256 || total > req.max_total_bytes {
                    return Err(IsolationError::InvalidInput);
                }
                Some(b)
            }
            (CopyOutMode::HashesOnly, None) => None,
            _ => return Err(IsolationError::InvalidInput),
        };
        Ok(ReportedFile {
            path: f.path,
            sha256: f.sha256,
            size: f.size,
            bytes,
        })
    };
    let mut report = CopyOutReport::default();
    for f in raw.changed {
        report.changed.push(file(f, true)?);
    }
    for f in raw.created {
        report.created.push(file(f, false)?);
    }
    for d in &raw.deleted {
        if !manifest.contains_key(d) || !seen.insert(d.clone()) {
            return Err(bad);
        }
    }
    report.deleted = raw.deleted;
    report.rejected = raw.rejected;
    Ok(report)
}

// ------------------------------------------------------ host /proc watchdog

/// (pid, starttime) of `root` and all its descendants, from HOST /proc.
fn process_tree(root: u32) -> Vec<(u32, u64)> {
    let mut children: BTreeMap<u32, Vec<(u32, u64)>> = BTreeMap::new();
    let mut root_start = None;
    if let Ok(entries) = fs::read_dir("/proc") {
        for e in entries.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if let Some((ppid, start, state)) = proc_stat(pid) {
                if state == 'Z' {
                    continue;
                }
                if pid == root {
                    root_start = Some(start);
                }
                children.entry(ppid).or_default().push((pid, start));
            }
        }
    }
    let Some(start) = root_start else {
        return Vec::new();
    };
    let mut out = vec![(root, start)];
    let mut i = 0;
    while i < out.len() {
        if let Some(kids) = children.get(&out[i].0) {
            out.extend(kids.iter().copied());
        }
        i += 1;
    }
    out
}
/// (ppid, starttime, state) parsed after the LAST ')' of /proc/<pid>/stat.
fn proc_stat(pid: u32) -> Option<(u32, u64, char)> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &s[s.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let state = f.first()?.chars().next()?;
    Some((f.get(1)?.parse().ok()?, f.get(19)?.parse().ok()?, state))
}
fn rss_bytes(pid: u32) -> u64 {
    fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}
#[cfg(target_os = "linux")]
fn alive(pid: u32, start: u64) -> bool {
    matches!(proc_stat(pid), Some((_, s, state)) if s == start && state != 'Z')
}
fn sample(root: u32, threshold: u64, since: Instant) -> Option<WatchdogEvent> {
    let tree = process_tree(root);
    let rss: u64 = tree.iter().map(|(p, _)| rss_bytes(*p)).sum();
    (rss > threshold).then(|| WatchdogEvent {
        rss_bytes: rss,
        threshold_bytes: threshold,
        processes: tree.len() as u32,
        at_ms: since.elapsed().as_millis() as u64,
    })
}
struct Watchdog {
    done: Arc<AtomicBool>,
    handle: thread::JoinHandle<Option<WatchdogEvent>>,
}
impl Watchdog {
    /// Bridges the caller's cancellation into `inner` and kills (via `inner`)
    /// above the soft aggregate RSS threshold.
    fn start(root: u32, threshold: u64, user: Cancellation, inner: Cancellation) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let handle = thread::spawn(move || {
            let since = Instant::now();
            let mut next = since;
            let mut event = None;
            while !flag.load(Ordering::SeqCst) {
                if user.is_cancelled() {
                    inner.cancel();
                }
                if event.is_none() && Instant::now() >= next {
                    next += WATCHDOG_PERIOD;
                    if let Some(e) = sample(root, threshold, since) {
                        event = Some(e);
                        inner.cancel();
                    }
                }
                thread::sleep(Duration::from_millis(10));
            }
            event
        });
        Self { done, handle }
    }
    fn finish(self) -> Option<WatchdogEvent> {
        self.done.store(true, Ordering::SeqCst);
        self.handle.join().unwrap_or(None)
    }
}

// ------------------------------------------------------- service owner (Linux)

#[cfg(target_os = "linux")]
mod svc {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::process::{ChildStderr, ChildStdout};
    use std::sync::mpsc;

    pub(super) const LOG_CHUNK: usize = 4096;
    pub(super) const EVENT_QUEUE_LOG_BYTES: usize = 256 * 1024;
    const HELLO_WAIT: Duration = Duration::from_secs(1);
    const MAX_PENDING: usize = 16;
    const FRAME_MAX_LINE: usize = 16 * 1024;

    pub(super) struct OwnerConfig {
        pub(super) hello: Vec<u8>,
        pub(super) tunnels_max: usize,
        pub(super) rss_threshold: u64,
        pub(super) lifetime: Duration,
    }
    /// Readiness probe peer: accept one connection, answer 'p' with 'P'.
    pub(super) fn probe_echo(listener: &UnixListener, wait: Duration) {
        let _ = listener.set_nonblocking(true);
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            if let Ok((mut s, _)) = listener.accept() {
                let _ = s.set_nonblocking(false);
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let mut b = [0u8; 1];
                if s.read_exact(&mut b).is_ok() && b[0] == b'p' {
                    let _ = s.write_all(b"P");
                }
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    pub(super) fn spawn_owner(
        cmd: Command,
        listener: UnixListener,
        shared: Arc<ServiceShared>,
        cfg: OwnerConfig,
        cancel: Cancellation,
    ) -> Result<thread::JoinHandle<()>> {
        let (tx, rx) = mpsc::channel();
        let owner_shared = shared.clone();
        let handle = thread::Builder::new()
            .name("pai-service-owner".into())
            .spawn(move || owner_main(cmd, listener, owner_shared, cfg, cancel, tx))
            .map_err(|_| IsolationError::IsolationUnavailable)?;
        match rx.recv() {
            Ok(Ok(())) => Ok(handle),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                let _ = handle.join();
                Err(IsolationError::IsolationUnavailable)
            }
        }
    }
    #[derive(Deserialize)]
    #[serde(tag = "t", rename_all = "snake_case", deny_unknown_fields)]
    enum Frame {
        Log { s: String, b: String },
        Drop { bytes: u64, records: u64 },
        Exit { status: Option<i32> },
    }
    enum Msg {
        Frame(Frame),
        Bad,
        StdoutEof,
        Stderr,
        StderrEof,
    }
    fn read_frames(mut out: ChildStdout, tx: mpsc::Sender<Msg>) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match out.read(&mut chunk) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Msg::StdoutEof);
                    return;
                }
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
            while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let msg = serde_json::from_slice::<Frame>(&line)
                    .map(Msg::Frame)
                    .unwrap_or(Msg::Bad);
                if tx.send(msg).is_err() {
                    return;
                }
            }
            if buf.len() > FRAME_MAX_LINE {
                buf.clear();
                if tx.send(Msg::Bad).is_err() {
                    return;
                }
            }
        }
    }
    fn read_stderr(mut err: ChildStderr, tx: mpsc::Sender<Msg>) {
        let mut chunk = [0u8; 4096];
        let mut reported = false;
        loop {
            match err.read(&mut chunk) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Msg::StderrEof);
                    return;
                }
                Ok(_) if !reported => {
                    reported = true;
                    if tx.send(Msg::Stderr).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
            }
        }
    }
    struct Owner {
        shared: Arc<ServiceShared>,
        child: Child,
        pid: u32,
        killed: bool,
        failure: bool,
        exit_frame: Option<Option<i32>>,
        watchdog: Option<WatchdogEvent>,
        victims: Vec<(u32, u64)>,
    }
    impl Owner {
        fn kill(&mut self) {
            if !self.killed {
                self.killed = true;
                self.victims = process_tree(self.pid);
                let _ = self.child.kill();
            }
        }
        fn fail(&mut self) {
            if !self.failure {
                self.failure = true;
                lock(&self.shared.events)
                    .q
                    .push_back(ServiceEvent::RunnerFailure);
            }
            self.kill();
        }
        fn frame(&mut self, f: Frame) {
            if self.exit_frame.is_some() {
                return self.fail();
            }
            match f {
                Frame::Log { s, b } => {
                    let stream = match s.as_str() {
                        "o" => Stream::Stdout,
                        "e" => Stream::Stderr,
                        _ => return self.fail(),
                    };
                    match unhex(&b) {
                        Ok(bytes) if !bytes.is_empty() && bytes.len() <= LOG_CHUNK => {
                            lock(&self.shared.events).push_log(stream, bytes)
                        }
                        _ => self.fail(),
                    }
                }
                Frame::Drop { bytes, records } => lock(&self.shared.events)
                    .q
                    .push_back(ServiceEvent::Dropped { bytes, records }),
                Frame::Exit { status } => self.exit_frame = Some(status),
            }
        }
    }
    fn owner_main(
        mut cmd: Command,
        listener: UnixListener,
        shared: Arc<ServiceShared>,
        cfg: OwnerConfig,
        cancel: Cancellation,
        ready: mpsc::Sender<Result<()>>,
    ) {
        let mut child = match spawn(&mut cmd) {
            Ok(c) => c,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            let _ = ready.send(Err(IsolationError::Io));
            return;
        };
        shared.running.store(true, Ordering::SeqCst);
        let _ = ready.send(Ok(()));
        let (tx, rx) = mpsc::channel();
        let tx2 = tx.clone();
        let readers = [
            thread::spawn(move || read_frames(stdout, tx)),
            thread::spawn(move || read_stderr(stderr, tx2)),
        ];
        let _ = listener.set_nonblocking(true);
        let pid = child.id();
        let mut o = Owner {
            shared: shared.clone(),
            child,
            pid,
            killed: false,
            failure: false,
            exit_frame: None,
            watchdog: None,
            victims: Vec::new(),
        };
        let started = Instant::now();
        let mut next_sample = started;
        let mut pending: Vec<(UnixStream, Vec<u8>, Instant)> = Vec::new();
        let mut eofs = 0;
        let status = loop {
            match rx.recv_timeout(Duration::from_millis(5)) {
                Ok(m) => handle_msg(&mut o, m, &mut eofs),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    thread::sleep(Duration::from_millis(5))
                }
            }
            while let Ok(m) = rx.try_recv() {
                handle_msg(&mut o, m, &mut eofs);
            }
            if shared.stop.load(Ordering::SeqCst) || cancel.is_cancelled() {
                o.kill();
            }
            if started.elapsed() >= cfg.lifetime {
                o.kill();
            }
            if !o.killed && Instant::now() >= next_sample {
                next_sample += WATCHDOG_PERIOD;
                if let Some(e) = sample(o.pid, cfg.rss_threshold, started) {
                    o.watchdog = Some(e);
                    o.kill();
                }
            }
            accept_tunnels(&listener, &mut pending, &shared, &cfg);
            match o.child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
        };
        // Drain the remaining frames (pipes close when the namespace dies).
        let drain_until = Instant::now() + Duration::from_secs(1);
        while eofs < 2 && Instant::now() < drain_until {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(m) => handle_msg(&mut o, m, &mut eofs),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        drop(rx);
        for r in readers {
            let _ = r.join();
        }
        lock(&shared.pool).clear();
        {
            let mut events = lock(&shared.events);
            if let Some(w) = o.watchdog.clone() {
                events.q.push_back(ServiceEvent::Watchdog(w));
            }
            if !o.failure {
                match (o.exit_frame, o.killed) {
                    (Some(status), _) => events.q.push_back(ServiceEvent::Exited { status }),
                    (None, true) => events.q.push_back(ServiceEvent::Exited { status: None }),
                    (None, false) => events.q.push_back(ServiceEvent::RunnerFailure),
                }
            }
        }
        let grace = Duration::from_millis(shared.grace_ms.load(Ordering::SeqCst));
        let deadline = Instant::now() + grace;
        let victims: Vec<(u32, u64)> = o
            .victims
            .iter()
            .copied()
            .filter(|(p, _)| *p != o.pid)
            .collect();
        let mut alive_after = victims.iter().filter(|(p, s)| alive(*p, *s)).count();
        while alive_after > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
            alive_after = victims.iter().filter(|(p, s)| alive(*p, *s)).count();
        }
        *lock(&shared.report) = Some(StopReport {
            killed: o.killed,
            exit_status: status.and_then(|s| s.code()),
            descendants_alive_after: alive_after as u32,
            elapsed_ms: 0,
        });
        shared.running.store(false, Ordering::SeqCst);
        shared.pool_cv.notify_all();
    }
    fn handle_msg(o: &mut Owner, m: Msg, eofs: &mut u32) {
        match m {
            Msg::Frame(f) => o.frame(f),
            Msg::Bad | Msg::Stderr => o.fail(),
            Msg::StdoutEof | Msg::StderrEof => *eofs += 1,
        }
    }
    /// Accept relay dials; pool only connections that present the per-service
    /// secret hello (in-sandbox generated code never sees the secret). Idle
    /// tunnels are bounded by tunnels_max; extras are closed.
    fn accept_tunnels(
        listener: &UnixListener,
        pending: &mut Vec<(UnixStream, Vec<u8>, Instant)>,
        shared: &ServiceShared,
        cfg: &OwnerConfig,
    ) {
        while let Ok((s, _)) = listener.accept() {
            if pending.len() < MAX_PENDING && s.set_nonblocking(true).is_ok() {
                pending.push((s, Vec::new(), Instant::now()));
            }
        }
        let mut i = 0;
        while i < pending.len() {
            let (s, buf, at) = &mut pending[i];
            let mut chunk = [0u8; 64];
            let want = cfg.hello.len() - buf.len();
            let verdict = match s.read(&mut chunk[..want]) {
                Ok(0) => Some(false),
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if !cfg.hello.starts_with(buf) {
                        Some(false)
                    } else if buf.len() == cfg.hello.len() {
                        Some(true)
                    } else {
                        None
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    (at.elapsed() > HELLO_WAIT).then_some(false)
                }
                Err(_) => Some(false),
            };
            match verdict {
                None => i += 1,
                Some(ok) => {
                    let (s, _, _) = pending.swap_remove(i);
                    if ok && s.set_nonblocking(false).is_ok() {
                        let mut pool = lock(&shared.pool);
                        if pool.len() < cfg.tunnels_max {
                            pool.push_back(Tunnel(s));
                            shared.pool_cv.notify_one();
                        }
                    }
                }
            }
        }
    }
}

// --------------------------------------------------- trusted Python constants

/// Trusted readiness probe (not generated code), run under the exact mount plan.
const PROBE: &str = r#"import os,sys,socket,ctypes
assert not os.path.exists('/proc') and not os.path.exists('/agent') and not os.path.exists('/home')
assert os.getuid()!=0
assert set(os.environ)<={'PATH','HOME','PWD','LC_CTYPE','MALLOC_ARENA_MAX'}
assert all(os.statvfs(d).f_flag&1 for d in ('/','/dev','/dev/shm','/src','/usr'))
t=os.statvfs('/tmp'); assert not t.f_flag&1 and t.f_blocks*t.f_frsize==16777216
w=os.statvfs('/work'); assert not w.f_flag&1 and w.f_blocks*w.f_frsize==int(sys.argv[1])
with open('/work/.pai-probe','w') as f: f.write('x')
os.unlink('/work/.pai-probe')
assert open('/src/probe.txt').read()=='probe'
assert ctypes.CDLL(None,use_errno=True).unshare(0x10000000)!=0
if sys.argv[2]=='service':
    assert os.statvfs('/run/pai-preview').f_flag&1 and os.listdir('/run/pai-preview')==['ctl.sock']
    try:
        open('/run/pai-preview/x','w'); raise SystemExit('socket dir writable')
    except OSError: pass
    s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); s.settimeout(2)
    s.connect('/run/pai-preview/ctl.sock'); s.sendall(b'p'); assert s.recv(1)==b'P'
else:
    assert not os.path.exists('/run/pai-preview')
print('READY')
"#;

/// Trusted runner: every gate/service command is `python3 -I -B -c RUNNER <mode> ...`.
/// Every stdlib module the runner itself uses is imported BEFORE /work goes on
/// sys.path (review R3 C9 f3): a file planted in /work by an earlier command
/// (e.g. /work/unittest.py) cannot replace the runner's own machinery.
const RUNNER: &str = r#"import sys,os,runpy,py_compile,unittest,unittest.main,unittest.loader,unittest.runner,unittest.case,unittest.suite,unittest.result
sys.dont_write_bytecode=True
m=sys.argv[1]
if m=='static':
    import functools,http.server
sys.path.insert(0,'/work')
if m=='script':
    p=sys.argv[2]; sys.argv=[p]+sys.argv[3:]
    runpy.run_path(p,run_name='__main__')
elif m=='unittest':
    unittest.main(module=None,argv=['unittest','discover','-s',sys.argv[2],'-p',sys.argv[3],'-t','/work'])
elif m=='compile':
    rc=0
    for f in sys.argv[2:]:
        try: py_compile.compile(f,cfile='/tmp/pai-compile.pyc',doraise=True)
        except py_compile.PyCompileError as e:
            sys.stderr.write(str(e.msg)+'\n'); rc=1
    sys.exit(rc)
elif m=='static':
    h=functools.partial(http.server.SimpleHTTPRequestHandler,directory=sys.argv[2])
    http.server.HTTPServer(('127.0.0.1',int(sys.argv[3])),h).serve_forever()
else:
    sys.exit(2)
"#;

/// SUPERVISOR_V3: trusted, SINGLE-THREADED (selectors only; Appendix A P2/P3),
/// runs inside containment. Same prologue as the Stage 4 supervisor. Its stdout
/// is the receipt/frame channel; every child gets its own pipes (close_fds) and
/// child output is only ever hex-encoded inside receipts/frames.
/// Oracle-role gate commands (review R3 C9 f1a/f1b): right before each one,
/// every /src file is compared with its /work copy; on any difference the
/// command is not run and is recorded as completed with status 126 and stderr
/// `pai: oracle tampered: [paths]`. An Oracle-role unittest whose stderr says
/// `Ran 0 tests` and that exited 0 is recorded with status 5 and stderr
/// prefixed `pai: oracle ran zero tests`.
const SUPERVISOR_V3: &str = r#"import os,sys,json,subprocess,selectors,socket,time,hashlib,stat,ctypes,platform,signal,errno
T0=time.monotonic()
libc=ctypes.CDLL(None,use_errno=True)
assert libc.prctl(4,0,0,0,0)==0 and libc.prctl(3,0,0,0,0)==0
assert platform.machine()=='x86_64'
class SF(ctypes.Structure):_fields_=[('code',ctypes.c_ushort),('jt',ctypes.c_ubyte),('jf',ctypes.c_ubyte),('k',ctypes.c_uint32)]
class SP(ctypes.Structure):_fields_=[('len',ctypes.c_ushort),('filter',ctypes.POINTER(SF))]
prog=(SF*9)(SF(0x20,0,0,4),SF(0x15,0,6,0xc000003e),SF(0x20,0,0,0),SF(0x35,4,0,0x40000000),SF(0x15,3,0,29),SF(0x15,2,0,64),SF(0x15,1,0,68),SF(0x06,0,0,0x7fff0000),SF(0x06,0,0,0x00050001))
fprog=SP(9,ctypes.cast(prog,ctypes.POINTER(SF)))
assert libc.prctl(ctypes.c_int(22),ctypes.c_ulong(2),ctypes.byref(fprog),ctypes.c_ulong(0),ctypes.c_ulong(0))==0
r=json.loads(sys.argv[1])
assert hashlib.sha256(open('/usr/bin/python3','rb').read()).hexdigest()==r['program_sha256']
ENV={'PATH':'/usr/bin','HOME':'/tmp','MALLOC_ARENA_MAX':'2'}
def fail(m):
    sys.stderr.write('supervisor: '+m+'\n'); sys.stderr.flush(); os._exit(3)
def rethrow(e):
    raise e
def emit(o):
    sys.stdout.write(json.dumps(o,separators=(',',':'))+'\n'); sys.stdout.flush()
def kill_all():
    # Every process of this PID namespace except bwrap's init and ourselves.
    try: os.kill(-1,signal.SIGKILL)
    except OSError: pass
def copy_in():
    man=r['manifest']; cap=r['copy_in_max_bytes']; seen=set(); total=0
    for d,ds,fs in os.walk('/src',onerror=rethrow):
        for n in ds:
            if not stat.S_ISDIR(os.lstat(os.path.join(d,n)).st_mode): fail('copy-in: non-directory entry')
        for n in fs:
            p=os.path.join(d,n); rel=p[5:]; st=os.lstat(p)
            if not stat.S_ISREG(st.st_mode) or rel not in man or rel in seen or st.st_size>cap: fail('copy-in: unexpected entry')
            with os.fdopen(os.open(p,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK),'rb') as f: b=f.read(st.st_size+1)
            total+=len(b)
            if len(b)!=st.st_size or total>cap or hashlib.sha256(b).hexdigest()!=man[rel]: fail('copy-in: content mismatch')
            t='/work/'+rel; os.makedirs(os.path.dirname(t),exist_ok=True)
            with os.fdopen(os.open(t,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o644),'wb') as f: f.write(b)
            seen.add(rel)
    if seen!=set(man): fail('copy-in: missing files')
ZERO=b'Ran 0 tests'
def run_cmd(c,deadline):
    t0=time.monotonic(); cap=c['output_bytes']; bufs=[bytearray(),bytearray()]; tot=[0,0]
    # Oracle-role unittest: scan ALL stderr (not only the retained cap) for a zero-test run.
    zt=c['role']=='oracle' and c['argv'][5]=='unittest'; zs=[b'',False]
    p=subprocess.Popen(c['argv'],cwd='/work',env=ENV,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,close_fds=True)
    sel=selectors.DefaultSelector()
    for i,f in enumerate((p.stdout,p.stderr)):
        os.set_blocking(f.fileno(),False); sel.register(f,selectors.EVENT_READ,i)
    def pump(w):
        for k,_ in sel.select(w):
            try: b=os.read(k.fd,65536)
            except BlockingIOError: continue
            if not b:
                sel.unregister(k.fileobj); continue
            i=k.data; tot[i]+=len(b); room=cap-len(bufs[i])
            if room>0: bufs[i]+=b[:room]
            if zt and i==1:
                s=zs[0]+b; zs[0]=s[1-len(ZERO):]
                if ZERO in s: zs[1]=True
    end=min(t0+c['timeout_ms']/1000.0,deadline); term='completed'
    while p.poll() is None:
        now=time.monotonic()
        if now>=end:
            term='timeout'; break
        pump(min(0.05,end-now))
    # The main process is done (or timed out): no descendant may outlive it.
    kill_all(); p.wait(); stop=time.monotonic()+1.0
    while sel.get_map() and time.monotonic()<stop: pump(0.05)
    sel.close(); p.stdout.close(); p.stderr.close()
    x={'id':c['id'],'argv':c['argv'],'status':p.returncode,'termination':term,'stdout_hex':bytes(bufs[0]).hex(),'stdout_total':tot[0],'stderr_hex':bytes(bufs[1]).hex(),'stderr_total':tot[1],'elapsed_ms':int((time.monotonic()-t0)*1000)}
    if zs[1] and p.returncode==0:
        e=b'pai: oracle ran zero tests\n'
        x.update(status=5,stderr_hex=(e+bytes(bufs[1]))[:cap].hex(),stderr_total=len(e)+tot[1])
    return x
def refuse(c,t0,status,msg):
    e=msg.encode('utf-8','backslashreplace')
    return {'id':c['id'],'argv':c['argv'],'status':status,'termination':'completed','stdout_hex':'','stdout_total':0,'stderr_hex':e[:c['output_bytes']].hex(),'stderr_total':len(e),'elapsed_ms':int((time.monotonic()-t0)*1000)}
def tampered():
    # /work copies of the read-only /src tree that differ, are missing, are not
    # regular files, or sit under a /work path that is not a real directory.
    bad=[]
    for d,ds,fs in os.walk('/src',onerror=rethrow):
        for n in ds:
            rel=os.path.join(d,n)[5:]
            try: ok=stat.S_ISDIR(os.lstat('/work/'+rel).st_mode)
            except OSError: ok=False
            if not ok: bad.append(rel+'/')
        for n in fs:
            p=os.path.join(d,n); rel=p[5:]; t='/work/'+rel
            try:
                with os.fdopen(os.open(p,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK),'rb') as f: a=f.read()
                ok=stat.S_ISREG(os.lstat(t).st_mode)
                if ok:
                    with os.fdopen(os.open(t,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK),'rb') as f: ok=f.read(len(a)+1)==a
            except OSError: ok=False
            if not ok: bad.append(rel)
    return sorted(bad)
def valid(rel):
    return len(rel.encode())<=256 and '\\' not in rel and '\x00' not in rel and all(c not in ('','.','..') for c in rel.split('/'))
def copy_out():
    co=r['copy_out']; out={'changed':[],'created':[],'deleted':[],'rejected':[]}
    if co['mode']=='off': return out
    man=r['manifest']; ign=set(co['ignore']); rej=out['rejected']; present=set(); found={}; entries=0
    def reject(name,why):
        if len(rej)<64: rej.append([name[:256],why])
        elif len(rej)==64: rej.append(['*','over_count'])
    for d,ds,fs in os.walk(b'/work',onerror=rethrow):
        keep=[]
        for n in ds+fs:
            entries+=1
            if entries>4096: return {'changed':[],'created':[],'deleted':[],'rejected':[['*','over_count']]}
            p=os.path.join(d,n); raw=p[6:]; name=raw.decode('utf-8','backslashreplace'); st=os.lstat(p)
            try: rel=raw.decode('utf-8')
            except UnicodeDecodeError: rel=None
            if stat.S_ISDIR(st.st_mode):
                if n.decode('utf-8','backslashreplace') in ign: reject(name,'ignored')
                else: keep.append(n)
                continue
            if rel is not None: present.add(rel)
            if stat.S_ISLNK(st.st_mode): reject(name,'symlink')
            elif not stat.S_ISREG(st.st_mode) or rel is None or not valid(rel): reject(name,'special')
            elif st.st_size>co['max_file_bytes']: reject(name,'oversize')
            else:
                with os.fdopen(os.open(p,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK),'rb') as f: b=f.read(st.st_size+1)
                if len(b)>co['max_file_bytes']: reject(name,'oversize')
                else: found[rel]=b
        ds[:]=keep
    n=0; total=0
    for rel in sorted(found):
        b=found[rel]; h=hashlib.sha256(b).hexdigest()
        if man.get(rel)==h: continue
        kind='changed' if rel in man else 'created'
        if n>=co['max_files']:
            reject(rel,'over_count'); continue
        e={'path':rel,'sha256':h,'size':len(b),'hex':None}
        if co['mode']=='contents':
            if total+len(b)>co['max_total_bytes']:
                reject(rel,'oversize'); continue
            total+=len(b); e['hex']=b.hex()
        n+=1; out[kind].append(e)
    out['deleted']=sorted(x for x in man if x not in present)
    return out
def gate():
    copy_in(); deadline=T0+r['total_timeout_ms']/1000.0; res=[]; skipped=[]; stop=False
    for c in r['commands']:
        if stop:
            skipped.append(c['id']); continue
        # Oracle role: the /work copy must still equal /src (no earlier command
        # rewrote it); otherwise the oracle is NOT run and is recorded as failed.
        t0=time.monotonic(); bad=tampered() if c['role']=='oracle' else []
        if bad: x=refuse(c,t0,126,'pai: oracle tampered: '+json.dumps(bad)+'\n')
        else: x=run_cmd(c,deadline)
        res.append(x)
        bad=x['termination']!='completed' or x['status']!=0
        if (bad and r['stop_on_failure']) or (x['termination']=='timeout' and time.monotonic()>=deadline): stop=True
    emit({'commands':res,'skipped':skipped,'copy_out':copy_out()})
def service():
    copy_in()
    port=r['port']; pool=r['pool']; mx=r['max']; hello=('PAI1'+r['secret']+'\n').encode(); rate=float(r['rate'])
    p=subprocess.Popen(r['argv'],cwd='/work',env=ENV,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,close_fds=True)
    sel=selectors.DefaultSelector(); idle=set(); conns=set(); end=T0+r['lifetime_ms']/1000.0
    st={'bucket':rate,'last':time.monotonic(),'db':0,'dr':0,'tick':time.monotonic(),'dial':0.0}
    for s,f in (('o',p.stdout),('e',p.stderr)):
        os.set_blocking(f.fileno(),False); sel.register(f,selectors.EVENT_READ,('log',s))
    def log(s,b):
        if st['bucket']>=len(b):
            st['bucket']-=len(b); emit({'t':'log','s':s,'b':b.hex()})
        else:
            st['db']+=len(b); st['dr']+=1
    def flush_drops():
        if st['db'] or st['dr']:
            emit({'t':'drop','bytes':st['db'],'records':st['dr']}); st['db']=0; st['dr']=0
    def read_log(k):
        try: b=os.read(k.fd,4096)
        except BlockingIOError: return
        if b: log(k.data[1],b)
        else: sel.unregister(k.fileobj)
    class Conn:
        # a = tunnel to the host bridge, b = in-sandbox loopback upstream.
        def __init__(s,t,u):
            s.a=t; s.b=u; s.ab=bytearray(); s.ba=bytearray(); s.ae=s.be=s.aw=s.bw=False; s.up=False; s.reg={}
        def mask(s,x):
            if not s.up: return selectors.EVENT_WRITE if x is s.b else 0
            if x is s.a: return (selectors.EVENT_READ if not s.ae and len(s.ab)<65536 else 0)|(selectors.EVENT_WRITE if s.ba else 0)
            return (selectors.EVENT_READ if not s.be and len(s.ba)<65536 else 0)|(selectors.EVENT_WRITE if s.ab else 0)
        def update(s):
            for x in (s.a,s.b):
                m=s.mask(x); old=s.reg.get(x,0)
                if m==old: continue
                if old and m: sel.modify(x,m,('conn',s))
                elif m: sel.register(x,m,('conn',s))
                else: sel.unregister(x)
                s.reg[x]=m
        def close(s):
            for x in (s.a,s.b):
                if s.reg.get(x): sel.unregister(x)
                x.close()
            s.reg={}; conns.discard(s)
        def event(s,x,ev):
            try:
                if not s.up:
                    if s.b.getsockopt(socket.SOL_SOCKET,socket.SO_ERROR): return s.close()
                    s.up=True; return s.update()
                if x is s.a: inb,outb=s.ab,s.ba
                else: inb,outb=s.ba,s.ab
                if ev&selectors.EVENT_READ and len(inb)<65536:
                    d=x.recv(65536-len(inb))
                    if d: inb.extend(d)
                    elif x is s.a: s.ae=True
                    else: s.be=True
                if ev&selectors.EVENT_WRITE and outb:
                    n=x.send(outb); del outb[:n]
                if s.ae and not s.ab and not s.bw:
                    s.bw=True; s.b.shutdown(socket.SHUT_WR)
                if s.be and not s.ba and not s.aw:
                    s.aw=True; s.a.shutdown(socket.SHUT_WR)
                if s.ae and s.be and not s.ab and not s.ba: return s.close()
                s.update()
            except (BlockingIOError,InterruptedError): s.update()
            except OSError: s.close()
    while True:
        now=time.monotonic()
        if p.poll() is not None: break
        if now>=end:
            kill_all(); p.wait(); break
        st['bucket']=min(rate,st['bucket']+rate*(now-st['last'])); st['last']=now
        if now-st['tick']>=1.0:
            st['tick']=now; flush_drops()
        while len(idle)<pool and len(idle)+len(conns)<mx and now>=st['dial']:
            t=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); t.setblocking(False)
            try:
                if t.connect_ex('/run/pai-preview/ctl.sock')!=0: raise OSError()
                t.send(hello)
            except OSError:
                t.close(); st['dial']=now+0.1; break
            idle.add(t); sel.register(t,selectors.EVENT_READ,('idle',t))
        for k,ev in sel.select(0.05):
            kind=k.data[0]
            if kind=='log': read_log(k)
            elif kind=='idle':
                t=k.data[1]
                try: b=t.recv(1)
                except BlockingIOError: continue
                except OSError: b=b''
                sel.unregister(t); idle.discard(t)
                if b!=b'G':
                    t.close(); continue
                u=socket.socket(socket.AF_INET,socket.SOCK_STREAM); u.setblocking(False)
                if u.connect_ex(('127.0.0.1',port)) not in (0,errno.EINPROGRESS):
                    u.close(); t.close(); continue
                c=Conn(t,u); conns.add(c); c.update()
            elif k.fileobj in k.data[1].reg:
                k.data[1].event(k.fileobj,ev)
    stop=time.monotonic()+0.5
    while time.monotonic()<stop and any(k.data[0]=='log' for k in sel.get_map().values()):
        for k,_ in sel.select(0.05):
            if k.data[0]=='log': read_log(k)
    flush_drops(); emit({'t':'exit','status':p.returncode}); kill_all()
if r['mode']=='gate': gate()
elif r['mode']=='service': service()
else: fail('unknown mode')
"#;

#[cfg(all(test, target_os = "linux"))]
mod tests {
    //! Real bwrap, serial (`--test-threads=1`). Generated code runs ONLY through
    //! `WorkspaceIsolation::{run_gate, start_service}`; host-side test code only
    //! creates staged bytes, sockets and reads host /proc. Fixtures differ from
    //! the held-out suite (temperature/duration modules, /api/trees site).
    use super::*;
    use crate::isolation::hex;
    use std::net::TcpListener;

    fn backend() -> WorkspaceIsolation {
        let base = LinuxIsolation::new().expect("real Stage 4 isolation is a test precondition");
        WorkspaceIsolation::new(&base)
            .expect("real Stage 5 workspace isolation is a test precondition, never skip")
    }
    fn staged(files: &[(&str, &[u8])]) -> (tempfile::TempDir, StagedTree) {
        let t = tempfile::tempdir().unwrap();
        let map = files
            .iter()
            .map(|(p, b)| (p.to_string(), b.to_vec()))
            .collect();
        let tree = StagedTree::from_files(t.path(), &map, &TreeLimits::default()).unwrap();
        (t, tree)
    }
    fn limits() -> WorkspaceLimits {
        WorkspaceLimits {
            cpu_seconds: 30,
            memory_bytes: 256 * MIB,
            processes: 32,
            work_tmpfs_bytes: 16 * MIB,
            file_size_bytes: MIB,
            open_files: 64,
            total_timeout_ms: 60_000,
            rss_watchdog_bytes: 1024 * MIB,
        }
    }
    fn script(id: &str, script: &str, args: &[&str]) -> GateCommand {
        GateCommand::PythonScript {
            id: id.into(),
            role: GateRole::Test,
            script: script.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            timeout_ms: 30_000,
            output_bytes: 65536,
        }
    }
    fn plan(commands: Vec<GateCommand>, stop: bool, mode: CopyOutMode) -> GatePlan {
        GatePlan {
            schema: GATE_PLAN_SCHEMA.into(),
            commands,
            stop_on_failure: stop,
            copy_out: CopyOutRequest {
                mode,
                ignore_dir_names: CopyOutRequest::default_ignores(),
                max_files: 64,
                max_total_bytes: MIB,
            },
            limits: limits(),
        }
    }
    fn out_json(c: &CommandResult) -> serde_json::Value {
        serde_json::from_slice(&c.stdout.retained).unwrap_or_else(|e| {
            panic!(
                "{e}: stdout={} stderr={}",
                String::from_utf8_lossy(&c.stdout.retained),
                String::from_utf8_lossy(&c.stderr.retained)
            )
        })
    }
    fn run_one(ws: &WorkspaceIsolation, tree: &StagedTree, p: &GatePlan) -> GateRunResult {
        let r = ws.run_gate(tree, p, &Cancellation::default()).unwrap();
        assert_eq!(r.termination, Termination::Completed, "{r:?}");
        r
    }
    /// Host /proc scan for live processes whose cmdline contains `<marker>-`
    /// (the grandchildren spawned by fixtures get `<marker>-<i>` as an argv
    /// element; sandbox wrappers only carry the bare marker inside JSON).
    fn marker_procs(marker: &str) -> usize {
        let marker = format!("{marker}-");
        let mut n = 0;
        for e in fs::read_dir("/proc").unwrap().flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if pid == std::process::id() {
                continue;
            }
            if let Ok(cmd) = fs::read(format!("/proc/{pid}/cmdline")) {
                let live = !matches!(proc_stat(pid), Some((_, _, 'Z')) | None);
                if live && cmd.windows(marker.len()).any(|w| w == marker.as_bytes()) {
                    n += 1;
                }
            }
        }
        n
    }
    fn wait_markers_gone(marker: &str, wait: Duration) -> usize {
        let start = Instant::now();
        loop {
            let n = marker_procs(marker);
            if n == 0 || start.elapsed() > wait {
                return n;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    fn marker() -> String {
        format!("pai-s5-marker-{}", nonce().unwrap())
    }

    // ------------------------------------------------------------ gate mode

    const WALK: &[u8] = br#"import os,json
CAP=17*1024*1024
def fill(d):
    n=0
    try:
        while n*4096<CAP:
            p=os.path.join(d,'pai-cap-%d'%n)
            fd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
            try:
                if os.write(fd,b'x'*4096)!=4096: break
            finally: os.close(fd)
            n+=1
    except OSError: pass
    for i in range(n+1):
        try: os.unlink(os.path.join(d,'pai-cap-%d'%i))
        except OSError: pass
    return n*4096
r={'writable':{},'statvfs':{},'dirs':0}
for d,ds,fs in os.walk('/',followlinks=False):
    r['dirs']+=1
    b=fill(d)
    if b: r['writable'][d]=b
for d in ['/','/dev','/dev/shm','/tmp','/work','/src','/usr','/lib']:
    s=os.statvfs(d); r['statvfs'][d]=[s.f_blocks*s.f_frsize,bool(s.f_flag&1)]
print(json.dumps(r))
"#;

    /// Appendix A P1: the writable set is exactly /tmp (16 MiB) and /work (W).
    #[test]
    fn writable_set_exact_tmp_and_work() {
        if !crate::isolation::test_support::isolation_or_ci_skip("writable_set_exact_tmp_and_work")
        {
            return;
        }
        let ws = backend();
        let (_t, tree) = staged(&[("walk.py", WALK), ("pkg/mod.py", b"X = 1\n")]);
        let r = run_one(
            &ws,
            &tree,
            &plan(vec![script("walk", "walk.py", &[])], true, CopyOutMode::Off),
        );
        let c = &r.commands[0];
        assert_eq!(
            c.status,
            Some(0),
            "{:?}",
            String::from_utf8_lossy(&c.stderr.retained)
        );
        let v = out_json(c);
        println!("STAGE5_P1_WRITABLE {}", v["writable"]);
        let writable = v["writable"].as_object().unwrap();
        assert!(
            v["dirs"].as_u64().unwrap() > 100,
            "walk did not cover the sandbox"
        );
        assert!(
            writable.contains_key("/tmp") && writable.contains_key("/work"),
            "{writable:?}"
        );
        for (dir, bytes) in writable {
            let bytes = bytes.as_u64().unwrap();
            if dir == "/tmp" {
                assert!(bytes <= WRITABLE_TMPFS_BYTES);
            } else {
                assert!(
                    dir == "/work" || dir.starts_with("/work/"),
                    "writable {dir}"
                );
                assert!(bytes <= 16 * MIB);
            }
        }
        for d in ["/", "/dev", "/dev/shm", "/src", "/usr", "/lib"] {
            assert_eq!(v["statvfs"][d][1], true, "{d} must be read-only");
        }
        assert_eq!(
            v["statvfs"]["/tmp"],
            serde_json::json!([WRITABLE_TMPFS_BYTES, false])
        );
        assert_eq!(v["statvfs"]["/work"], serde_json::json!([16 * MIB, false]));
        assert_eq!(ws.profile().writable["/tmp"], WRITABLE_TMPFS_BYTES);
        assert!(!ws.profile().cgroup_quota && !ws.profile().proc_mounted);
    }

    #[test]
    fn src_ro_no_host_paths_no_proc_env_scrubbed() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "src_ro_no_host_paths_no_proc_env_scrubbed",
        ) {
            return;
        }
        std::env::set_var("PAI_STAGE5_CANARY", "stage5-host-canary-7f3a");
        let ws = backend();
        let probe: &[u8] = br#"import os,sys,json,errno
r={}
for p in ['/proc','/agent','/home','/etc','/root','/vercel','/run','/src/probe.py','/work/probe.py']:
    r[p]=os.path.exists(p)
r['env']=sorted(os.environ)
r['canary']=any('canary' in v for v in os.environ.values())
r['uid']=os.getuid(); r['cwd']=os.getcwd(); r['sys_path0']=sys.path[0]; r['nobytecode']=sys.dont_write_bytecode
r['src']=sorted(os.listdir('/src')); r['same']=open('/src/temp.py').read()==open('/work/temp.py').read()
try:
    open('/src/temp.py','a').write('x'); r['src_write']='ok'
except OSError as e: r['src_write']=errno.errorcode[e.errno]
try:
    open('/src/new.py','w').write('x'); r['src_create']='ok'
except OSError as e: r['src_create']=errno.errorcode[e.errno]
r['src_rdonly']=bool(os.statvfs('/src').f_flag&1)
open('/work/temp.py','a').write('# edited in /work only\n')
print(json.dumps(r))
"#;
        let (_t, tree) = staged(&[
            ("probe.py", probe),
            ("temp.py", b"def c_to_f(c):\n    return c * 9 / 5 + 32\n"),
        ]);
        let r = run_one(
            &ws,
            &tree,
            &plan(
                vec![script("probe", "probe.py", &[])],
                true,
                CopyOutMode::HashesOnly,
            ),
        );
        let v = out_json(&r.commands[0]);
        println!("STAGE5_SRC_ENV {v}");
        for p in [
            "/proc", "/agent", "/home", "/etc", "/root", "/vercel", "/run",
        ] {
            assert_eq!(v[p], false, "{p} visible");
        }
        assert_eq!(v["/src/probe.py"], true);
        assert_eq!(v["/work/probe.py"], true);
        let env: BTreeSet<String> = serde_json::from_value(v["env"].clone()).unwrap();
        let allowed: BTreeSet<String> = ["PATH", "HOME", "PWD", "LC_CTYPE", "MALLOC_ARENA_MAX"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(env.is_subset(&allowed), "{env:?}");
        assert_eq!(v["canary"], false);
        assert_ne!(v["uid"], 0);
        assert_eq!(v["cwd"], "/work");
        assert_eq!(v["sys_path0"], "/work");
        assert_eq!(v["nobytecode"], true);
        assert_eq!(v["same"], true);
        // The staged files are 0400/0500 as well, so the kernel may report the
        // permission check (EACCES) before the read-only mount (EROFS).
        for k in ["src_write", "src_create"] {
            assert!(v[k] == "EROFS" || v[k] == "EACCES", "{k}: {}", v[k]);
        }
        assert_eq!(v["src_rdonly"], true);
        // The /work edit is reported by copy-out; the staged tree is untouched.
        assert_eq!(r.copy_out.changed.len(), 1);
        assert_eq!(r.copy_out.changed[0].path, "temp.py");
        assert!(r.copy_out.changed[0].bytes.is_none());
        tree.validate().unwrap();
        let receipt = serde_json::to_string(&r).unwrap();
        assert!(!receipt.contains("stage5-host-canary-7f3a"));
        std::env::remove_var("PAI_STAGE5_CANARY");
    }

    const TEMP: &[u8] = b"def c_to_f(c):\n    return c * 9 / 5 + 32\n";
    const DURATION: &[u8] = b"def minutes_to_seconds(m):\n    return m * 60\n";
    const TEST_OK: &[u8] = b"import unittest\nfrom temp import c_to_f\nfrom duration import minutes_to_seconds\nclass T(unittest.TestCase):\n    def test_temp(self):\n        self.assertEqual(c_to_f(100), 212)\n    def test_duration(self):\n        self.assertEqual(minutes_to_seconds(2), 120)\n";
    const TEST_BAD: &[u8] = b"import unittest\nfrom temp import c_to_f\nclass T(unittest.TestCase):\n    def test_temp(self):\n        self.assertEqual(c_to_f(0), 33)\n";

    #[test]
    fn gate_exit_codes_per_command_and_skip_after_failure() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "gate_exit_codes_per_command_and_skip_after_failure",
        ) {
            return;
        }
        let ws = backend();
        let (_t, tree) = staged(&[
            ("temp.py", TEMP),
            ("duration.py", DURATION),
            ("tests/__init__.py", b""),
            ("tests/test_units.py", TEST_OK),
            ("bad/__init__.py", b""),
            ("bad/test_bad.py", TEST_BAD),
            ("broken.py", b"def x(:\n"),
            (
                "fail3.py",
                b"import sys\nprint('failing with 3')\nsys.exit(3)\n",
            ),
            ("after.py", b"print('ran after')\n"),
        ]);
        let compile = |id: &str, files: &[&str]| GateCommand::PythonCompile {
            id: id.into(),
            role: GateRole::Build,
            files: files.iter().map(|s| s.to_string()).collect(),
            timeout_ms: 20_000,
            output_bytes: 4096,
        };
        let unittest = |id: &str, dir: &str| GateCommand::PythonUnittest {
            id: id.into(),
            role: GateRole::Test,
            start_dir: dir.into(),
            pattern: "test_*.py".into(),
            timeout_ms: 20_000,
            output_bytes: 4096,
        };
        let stop = plan(
            vec![
                compile("build", &["temp.py", "duration.py"]),
                unittest("tests", "tests"),
                script("fail", "fail3.py", &[]),
                script("after", "after.py", &[]),
            ],
            true,
            CopyOutMode::Off,
        );
        let r = run_one(&ws, &tree, &stop);
        let statuses: Vec<_> = r.commands.iter().map(|c| c.status).collect();
        println!(
            "STAGE5_GATE_STOP statuses={statuses:?} skipped={:?} stderr={:?}",
            r.skipped,
            r.commands
                .iter()
                .map(|c| String::from_utf8_lossy(&c.stderr.retained).into_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(statuses, vec![Some(0), Some(0), Some(3)]);
        assert_eq!(r.skipped, vec!["after".to_string()]);
        assert_eq!(r.commands[0].role, GateRole::Build);
        assert_eq!(r.commands[2].stdout.retained, b"failing with 3\n");
        assert!(r
            .commands
            .iter()
            .all(|c| c.termination == Termination::Completed));
        assert_eq!(r.commands[1].argv[..5], [PYTHON, "-I", "-B", "-c", RUNNER]);
        assert_eq!(
            r.commands[1].argv[5..],
            ["unittest", "/work/tests", "test_*.py"]
        );
        assert_eq!(r.tree_sha256, tree.tree_sha256());
        assert_eq!(r.plan_sha256, hash(&json(&stop).unwrap()));
        assert_eq!(r.workspace_profile_sha256, ws.profile_sha256());
        let go_on = plan(
            vec![
                compile("build", &["broken.py"]),
                unittest("bad", "bad"),
                script("fail", "fail3.py", &[]),
                script("after", "after.py", &[]),
            ],
            false,
            CopyOutMode::Off,
        );
        let r = run_one(&ws, &tree, &go_on);
        let statuses: Vec<_> = r.commands.iter().map(|c| c.status).collect();
        println!("STAGE5_GATE_CONTINUE statuses={statuses:?}");
        assert_eq!(statuses, vec![Some(1), Some(1), Some(3), Some(0)]);
        assert!(r.skipped.is_empty());
        assert!(!r.commands[0].stderr.retained.is_empty());
        assert_eq!(r.commands[3].stdout.retained, b"ran after\n");
        // log_sha256 is recomputed on the host in the Stage 4 typed form.
        for c in &r.commands {
            let run = ActualRun {
                argv: c.argv.clone(),
                status: c.status,
                termination: c.termination,
                stdout_hex: hex(&c.stdout.retained),
                stderr_hex: hex(&c.stderr.retained),
                log_sha256: String::new(),
                output_files: BTreeMap::new(),
                elapsed_ms: c.elapsed_ms,
            };
            assert_eq!(c.log_sha256, log_hash(&run).unwrap());
        }
    }

    #[test]
    fn bounded_output_truncation_metadata() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "bounded_output_truncation_metadata",
        ) {
            return;
        }
        let ws = backend();
        let (_t, tree) = staged(&[(
            "loud.py",
            b"import sys\nsys.stdout.write('x'*100000)\nsys.stderr.write('err')\n",
        )]);
        let mut p = plan(vec![script("loud", "loud.py", &[])], true, CopyOutMode::Off);
        if let GateCommand::PythonScript { output_bytes, .. } = &mut p.commands[0] {
            *output_bytes = 1024;
        }
        let r = run_one(&ws, &tree, &p);
        let c = &r.commands[0];
        assert_eq!(c.status, Some(0));
        assert_eq!(c.stdout.retained, vec![b'x'; 1024]);
        assert_eq!(c.stdout.total_bytes, 100000);
        assert!(c.stdout.truncated);
        assert_eq!(c.stderr.retained, b"err");
        assert_eq!(c.stderr.total_bytes, 3);
        assert!(!c.stderr.truncated);
        assert_eq!(c.termination, Termination::Completed);
    }

    // ------------- review R3 C9 (f1a, f1b, f3): oracle execution hardening
    fn oracle_unittest(id: &str, dir: &str, pattern: &str, output_bytes: u32) -> GateCommand {
        GateCommand::PythonUnittest {
            id: id.into(),
            role: GateRole::Oracle,
            start_dir: dir.into(),
            pattern: pattern.into(),
            timeout_ms: 20_000,
            output_bytes,
        }
    }
    fn text(b: &BoundedOutput) -> String {
        String::from_utf8_lossy(&b.retained).into_owned()
    }
    /// SCRIPTED hostile "generated code" (not a model): plants top-level
    /// modules in /work that shadow the RUNNER's own stdlib imports.
    const PLANTER: &[u8] = br#"
open('/work/unittest.py','w').write("import sys\nsys.stderr.write('PAI-FAKE-UNITTEST\\n')\ndef main(*a,**k):\n    sys.exit(0)\n")
open('/work/runpy.py','w').write("def run_path(*a,**k):\n    print('PAI-FAKE-RUNPY')\n")
open('/work/py_compile.py','w').write("class PyCompileError(Exception):\n    pass\ndef compile(*a,**k):\n    print('PAI-FAKE-PY-COMPILE')\n")
print('planted')
"#;

    /// R3 C9 f3 (L3): an earlier command plants /work/unittest.py (and
    /// runpy.py, py_compile.py). The RUNNER imports its stdlib modules before
    /// /work is on sys.path, so the real oracle runs (and fails) in every mode.
    #[test]
    fn planted_work_stdlib_modules_do_not_shadow_runner() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "planted_work_stdlib_modules_do_not_shadow_runner",
        ) {
            return;
        }
        let ws = backend();
        let (_t, tree) = staged(&[
            ("plant.py", PLANTER),
            ("temp.py", TEMP),
            ("duration.py", DURATION),
            ("tests/__init__.py", b""),
            ("tests/test_units.py", TEST_OK),
            ("bad/__init__.py", b""),
            ("bad/test_bad.py", TEST_BAD),
            ("broken.py", b"def x(:\n"),
            ("fail3.py", b"import sys\nsys.exit(3)\n"),
        ]);
        let p = plan(
            vec![
                script("plant", "plant.py", &[]),
                oracle_unittest("oracle", "bad", "test_*.py", 4096),
                GateCommand::PythonUnittest {
                    id: "visible".into(),
                    role: GateRole::Test,
                    start_dir: "tests".into(),
                    pattern: "test_*.py".into(),
                    timeout_ms: 20_000,
                    output_bytes: 4096,
                },
                script("fail", "fail3.py", &[]),
                GateCommand::PythonCompile {
                    id: "build".into(),
                    role: GateRole::Build,
                    files: vec!["broken.py".into()],
                    timeout_ms: 20_000,
                    output_bytes: 4096,
                },
            ],
            false,
            CopyOutMode::HashesOnly,
        );
        let r = run_one(&ws, &tree, &p);
        let statuses: Vec<_> = r.commands.iter().map(|c| c.status).collect();
        let outputs: Vec<(String, String)> = r
            .commands
            .iter()
            .map(|c| (text(&c.stdout), text(&c.stderr)))
            .collect();
        println!("STAGE5_R3F3_SHADOW statuses={statuses:?} outputs={outputs:?}");
        // The planting itself happened (and is visible to copy-out).
        let created: BTreeSet<&str> = r.copy_out.created.iter().map(|f| f.path.as_str()).collect();
        assert!(
            ["unittest.py", "runpy.py", "py_compile.py"]
                .iter()
                .all(|p| created.contains(p)),
            "{created:?}"
        );
        assert_eq!(statuses, vec![Some(0), Some(1), Some(0), Some(3), Some(1)]);
        for (out, err) in &outputs {
            assert!(
                !out.contains("PAI-FAKE") && !err.contains("PAI-FAKE"),
                "{out} {err}"
            );
        }
        assert!(outputs[1].1.contains("Ran 1 test") && outputs[1].1.contains("FAILED"));
        assert!(outputs[2].1.contains("Ran 2 tests") && outputs[2].1.contains("OK"));
        assert!(
            !outputs[4].1.is_empty(),
            "the real py_compile reported the error"
        );
    }

    const ORACLE_HELPER: &[u8] = b"F = 213\n";
    const ORACLE_TEST: &[u8] = b"import unittest\nfrom temp import c_to_f\nfrom tests.vectors import F\nclass T(unittest.TestCase):\n    def test_temp(self):\n        self.assertEqual(c_to_f(100), F)\n";
    /// SCRIPTED hostile "generated code" (not a model) run by an EARLIER,
    /// non-oracle command: alters the /work copy of the oracle.
    const ORACLE_ATTACK: &[u8] = br#"import os,sys
v=sys.argv[1]
OK="import unittest\nclass T(unittest.TestCase):\n    def test_ok(self):\n        print('PAI-REWRITTEN-ORACLE-RAN')\n"
t='/work/tests/test_units.py'
if v=='rewrite': open(t,'w').write(OK)
elif v=='delete': os.unlink(t)
elif v=='symlink':
    open('/tmp/fake_test.py','w').write(OK); os.unlink(t); os.symlink('/tmp/fake_test.py',t)
elif v=='helper': open('/work/tests/vectors.py','w').write('F = 212\n')
elif v=='dir':
    os.rename('/work/tests','/work/tests_real'); os.symlink('/work/tests_real','/work/tests')
print('attack',v)
"#;

    /// R3 C9 f1a (copy-out Off) and f1b (copy-out Contents): an earlier
    /// command rewrites, deletes or re-links the /work copy of an oracle test
    /// or helper. The supervisor compares /src with /work right before every
    /// Oracle-role command and records the oracle as FAILED without running it.
    #[test]
    fn oracle_tampered_in_work_by_earlier_command_is_refused() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "oracle_tampered_in_work_by_earlier_command_is_refused",
        ) {
            return;
        }
        let ws = backend();
        let (_t, tree) = staged(&[
            ("attack.py", ORACLE_ATTACK),
            ("after.py", b"print('ran after')\n"),
            ("temp.py", TEMP),
            ("tests/__init__.py", b""),
            ("tests/vectors.py", ORACLE_HELPER),
            ("tests/test_units.py", ORACLE_TEST),
        ]);
        let gate = |variant: &str, mode: CopyOutMode| {
            let p = plan(
                vec![
                    script("attack", "attack.py", &[variant]),
                    oracle_unittest("oracle", "tests", "test_*.py", 4096),
                    script("after", "after.py", &[]),
                ],
                true,
                mode,
            );
            run_one(&ws, &tree, &p)
        };
        for mode in [CopyOutMode::Off, CopyOutMode::Contents] {
            // Control: untouched oracle runs for real and fails (212 != 213).
            let r = gate("none", mode);
            let o = &r.commands[1];
            assert_eq!(o.status, Some(1), "{}", text(&o.stderr));
            assert!(text(&o.stderr).contains("Ran 1 test"));
            assert_eq!(r.skipped, vec!["after".to_string()]);
            for (variant, path) in [
                ("rewrite", "tests/test_units.py"),
                ("delete", "tests/test_units.py"),
                ("symlink", "tests/test_units.py"),
                ("helper", "tests/vectors.py"),
                ("dir", "tests/"),
            ] {
                let r = gate(variant, mode);
                assert_eq!(r.commands[0].status, Some(0), "{variant}");
                let o = &r.commands[1];
                let err = text(&o.stderr);
                println!(
                    "STAGE5_R3F1_TAMPER mode={mode:?} variant={variant} status={:?} stderr={err:?}",
                    o.status
                );
                assert_eq!(o.role, GateRole::Oracle);
                assert_eq!(o.termination, Termination::Completed);
                assert_eq!(o.status, Some(126), "{variant}: {err}");
                assert!(err.starts_with("pai: oracle tampered:"), "{variant}: {err}");
                assert!(err.contains(path), "{variant}: {err}");
                assert!(
                    o.stdout.retained.is_empty(),
                    "{variant}: the oracle never ran"
                );
                assert!(!err.contains("PAI-REWRITTEN-ORACLE-RAN"));
                // A failure like any other: stop_on_failure skips the rest.
                assert_eq!(r.skipped, vec!["after".to_string()]);
                if mode == CopyOutMode::Contents && variant == "rewrite" {
                    assert!(r.copy_out.changed.iter().any(|f| f.path == path));
                }
            }
        }
        // Scope: only Oracle-role commands are checked; a Test-role run over
        // the same rewritten /work is unchanged behaviour.
        let p = plan(
            vec![
                script("attack", "attack.py", &["rewrite"]),
                GateCommand::PythonUnittest {
                    id: "visible".into(),
                    role: GateRole::Test,
                    start_dir: "tests".into(),
                    pattern: "test_*.py".into(),
                    timeout_ms: 20_000,
                    output_bytes: 4096,
                },
            ],
            true,
            CopyOutMode::Off,
        );
        let r = run_one(&ws, &tree, &p);
        assert_eq!(r.commands[1].status, Some(0));
        assert!(text(&r.commands[1].stdout).contains("PAI-REWRITTEN-ORACLE-RAN"));
        tree.validate().unwrap();
    }

    /// R3 C9 (zero-test oracle): a PythonUnittest Oracle that ran zero tests
    /// is a failure, also when "Ran 0 tests" lies past the retained output.
    #[test]
    fn oracle_unittest_zero_tests_is_failure() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "oracle_unittest_zero_tests_is_failure",
        ) {
            return;
        }
        let ws = backend();
        let (_t, tree) = staged(&[
            ("temp.py", TEMP),
            ("duration.py", DURATION),
            ("tests/__init__.py", b""),
            ("tests/test_units.py", TEST_OK),
            (
                "tests/test_loud.py",
                b"import sys\nsys.stderr.write('n' * 10000)\n",
            ),
        ]);
        let p = plan(
            vec![
                oracle_unittest("none", "tests", "nomatch_*.py", 4096),
                GateCommand::PythonUnittest {
                    id: "visible-none".into(),
                    role: GateRole::Test,
                    start_dir: "tests".into(),
                    pattern: "nomatch_*.py".into(),
                    timeout_ms: 20_000,
                    output_bytes: 4096,
                },
                oracle_unittest("real", "tests", "test_units.py", 4096),
                oracle_unittest("loud", "tests", "test_loud.py", 64),
            ],
            false,
            CopyOutMode::Off,
        );
        let r = run_one(&ws, &tree, &p);
        let statuses: Vec<_> = r.commands.iter().map(|c| c.status).collect();
        let errs: Vec<String> = r.commands.iter().map(|c| text(&c.stderr)).collect();
        println!("STAGE5_R3_ZERO_TESTS statuses={statuses:?} stderr={errs:?}");
        assert_eq!(statuses, vec![Some(5), Some(0), Some(0), Some(5)]);
        assert!(r
            .commands
            .iter()
            .all(|c| c.termination == Termination::Completed));
        const MARK: &str = "pai: oracle ran zero tests\n";
        assert!(errs[0].starts_with(MARK) && errs[0].contains("Ran 0 tests"));
        assert!(errs[1].contains("Ran 0 tests") && !errs[1].contains("pai:"));
        assert!(errs[2].contains("Ran 2 tests") && !errs[2].contains("pai:"));
        let loud = &r.commands[3].stderr;
        assert!(errs[3].starts_with(MARK), "{}", errs[3]);
        assert!(loud.truncated && loud.retained.len() == 64);
        assert!(loud.total_bytes > 10_000 + MARK.len() as u64);
    }

    const SPAWNER: &[u8] = br#"import subprocess,sys,time
m=sys.argv[1]
ps=[subprocess.Popen(['/usr/bin/python3','-c','import time\nwhile True: time.sleep(1)','%s-%d'%(m,i)],start_new_session=True) for i in range(2)]
open('/work/pids.txt','w').write(' '.join(str(p.pid) for p in ps))
print('spawned',flush=True)
time.sleep(60)
"#;
    const CHECKER: &[u8] = br#"import os,json
dead=[]
for p in open('/work/pids.txt').read().split():
    try:
        os.kill(int(p),0); dead.append(False)
    except ProcessLookupError: dead.append(True)
print(json.dumps(dead))
"#;

    #[test]
    fn timeout_and_cancel_kill_descendants() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "timeout_and_cancel_kill_descendants",
        ) {
            return;
        }
        let ws = backend();
        let (_t, tree) = staged(&[("spawn.py", SPAWNER), ("check.py", CHECKER)]);
        // Per-command timeout: the command and its setsid() descendants die.
        let m = marker();
        let mut p = plan(
            vec![
                script("spawn", "spawn.py", &[&m]),
                script("check", "check.py", &[]),
            ],
            false,
            CopyOutMode::Off,
        );
        if let GateCommand::PythonScript { timeout_ms, .. } = &mut p.commands[0] {
            *timeout_ms = 1500;
        }
        let seen = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let watcher = {
            let (seen, stop, m) = (seen.clone(), stop.clone(), m.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    seen.fetch_max(marker_procs(&m) as u64, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(50));
                }
            })
        };
        let r = run_one(&ws, &tree, &p);
        stop.store(true, Ordering::SeqCst);
        watcher.join().unwrap();
        let c = &r.commands[0];
        println!(
            "STAGE5_TIMEOUT termination={:?} status={:?} seen={} after={}",
            c.termination,
            c.status,
            seen.load(Ordering::SeqCst),
            marker_procs(&m)
        );
        assert_eq!(c.termination, Termination::Timeout);
        assert_eq!(c.status, Some(-9));
        assert_eq!(c.stdout.retained, b"spawned\n");
        assert!(
            seen.load(Ordering::SeqCst) >= 2,
            "descendants never observed"
        );
        assert_eq!(out_json(&r.commands[1]), serde_json::json!([true, true]));
        assert_eq!(wait_markers_gone(&m, Duration::from_secs(2)), 0);
        // Whole-run cancellation from another thread kills the namespace.
        let m = marker();
        let p = plan(
            vec![script("spawn", "spawn.py", &[&m])],
            true,
            CopyOutMode::Off,
        );
        let cancel = Cancellation::default();
        let canceller = {
            let (cancel, m) = (cancel.clone(), m.clone());
            thread::spawn(move || {
                let start = Instant::now();
                while marker_procs(&m) < 2 && start.elapsed() < Duration::from_secs(10) {
                    thread::sleep(Duration::from_millis(20));
                }
                cancel.cancel();
            })
        };
        let started = Instant::now();
        let r = ws.run_gate(&tree, &p, &cancel).unwrap();
        canceller.join().unwrap();
        println!(
            "STAGE5_CANCEL termination={:?} elapsed_ms={}",
            r.termination,
            started.elapsed().as_millis()
        );
        assert_eq!(r.termination, Termination::Cancelled);
        assert!(
            r.commands.is_empty(),
            "no command results trusted after cancel"
        );
        assert_eq!(r.skipped, vec!["spawn".to_string()]);
        assert!(started.elapsed() < Duration::from_secs(15));
        assert_eq!(wait_markers_gone(&m, Duration::from_secs(2)), 0);
        // Cancelled before admission: nothing is spawned.
        let before = spawns_here();
        let r = ws.run_gate(&tree, &p, &cancel).unwrap();
        assert_eq!(r.termination, Termination::Cancelled);
        assert_eq!(spawns_here(), before);
    }

    const MUTATOR: &[u8] = br#"import os
open('/work/a.py','w').write('A = 2\n')
os.unlink('/work/c.py')
os.makedirs('/work/new',exist_ok=True); open('/work/new/b.txt','w').write('created\n')
os.symlink('/etc/passwd','/work/link')
os.symlink('/tmp','/work/dirlink')
os.mkfifo('/work/fifo')
open('/work/big.bin','wb').write(b'x'*300000)
os.makedirs('/work/__pycache__'); open('/work/__pycache__/x.pyc','wb').write(b'pyc')
"#;

    #[test]
    fn copy_out_changes_created_deleted_symlink_rejected() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "copy_out_changes_created_deleted_symlink_rejected",
        ) {
            return;
        }
        let ws = backend();
        let (_t, tree) = staged(&[
            ("mut.py", MUTATOR),
            ("a.py", b"A = 1\n"),
            ("c.py", b"C = 3\n"),
            ("keep.txt", b"same\n"),
            ("pkg/m.py", b"M = 0\n"),
        ]);
        let run = |mode: CopyOutMode, max_files: u32| {
            let mut p = plan(vec![script("mut", "mut.py", &[])], true, mode);
            p.copy_out.max_files = max_files;
            run_one(&ws, &tree, &p)
        };
        let r = run(CopyOutMode::Contents, 64);
        println!(
            "STAGE5_COPY_OUT {}",
            serde_json::to_string(&r.copy_out).unwrap()
        );
        assert_eq!(r.commands[0].status, Some(0));
        let co = &r.copy_out;
        assert_eq!(co.changed.len(), 1);
        assert_eq!(co.changed[0].path, "a.py");
        assert_eq!(co.changed[0].bytes.as_deref(), Some(&b"A = 2\n"[..]));
        assert_eq!(co.changed[0].sha256, hash(b"A = 2\n"));
        assert_eq!(co.created.len(), 1);
        assert_eq!(co.created[0].path, "new/b.txt");
        assert_eq!(co.created[0].bytes.as_deref(), Some(&b"created\n"[..]));
        assert_eq!(co.deleted, vec!["c.py".to_string()]);
        let rejected: BTreeMap<String, CopyOutReject> = co.rejected.iter().cloned().collect();
        assert_eq!(rejected.get("link"), Some(&CopyOutReject::Symlink));
        assert_eq!(rejected.get("dirlink"), Some(&CopyOutReject::Symlink));
        assert_eq!(rejected.get("fifo"), Some(&CopyOutReject::Special));
        assert_eq!(rejected.get("big.bin"), Some(&CopyOutReject::Oversize));
        assert_eq!(rejected.get("__pycache__"), Some(&CopyOutReject::Ignored));
        assert_eq!(rejected.len(), 5, "{rejected:?}");
        let r = run(CopyOutMode::HashesOnly, 64);
        assert_eq!(r.copy_out.changed[0].bytes, None);
        assert_eq!(r.copy_out.created[0].sha256, hash(b"created\n"));
        let r = run(CopyOutMode::Off, 64);
        assert_eq!(r.copy_out, CopyOutReport::default());
        let r = run(CopyOutMode::Contents, 1);
        assert_eq!(r.copy_out.changed.len() + r.copy_out.created.len(), 1);
        assert!(r
            .copy_out
            .rejected
            .contains(&("new/b.txt".to_string(), CopyOutReject::OverCount)));
        tree.validate().unwrap();
    }

    #[test]
    fn receipt_overflow_is_runner_failure() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "receipt_overflow_is_runner_failure",
        ) {
            return;
        }
        let mut ws = backend();
        let (_t, tree) = staged(&[("loud.py", b"print('y'*20000)\n")]);
        let p = plan(vec![script("loud", "loud.py", &[])], true, CopyOutMode::Off);
        let control = run_one(&ws, &tree, &p);
        assert_eq!(control.commands[0].stdout.total_bytes, 20001);
        ws.receipt_cap_override = Some(2048);
        let r = ws.run_gate(&tree, &p, &Cancellation::default()).unwrap();
        println!("STAGE5_RECEIPT_OVERFLOW termination={:?}", r.termination);
        assert_eq!(r.termination, Termination::RunnerFailure);
        assert!(r.commands.is_empty());
        assert_eq!(r.skipped, vec!["loud".to_string()]);
        assert_eq!(r.copy_out, CopyOutReport::default());
    }

    #[test]
    fn forged_stdout_in_child_does_not_fake_receipt() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "forged_stdout_in_child_does_not_fake_receipt",
        ) {
            return;
        }
        let ws = backend();
        let forged = br#"{"commands":[{"id":"forge","argv":[],"status":0,"termination":"completed","stdout_hex":"","stdout_total":0,"stderr_hex":"","stderr_total":0,"elapsed_ms":1}],"skipped":[],"copy_out":{"changed":[],"created":[],"deleted":[],"rejected":[]}}"#;
        let mut forge = b"import sys,os\nline=".to_vec();
        forge.extend_from_slice(format!("{:?}", String::from_utf8_lossy(forged)).as_bytes());
        forge.extend_from_slice(b"\nprint(line,flush=True)\nfor fd in (3,4,5,6,7,8,9):\n    try: os.write(fd,(line+'\\n').encode())\n    except OSError: pass\nfor p in ('/dev/stdout','/proc/self/fd/1','/dev/tty'):\n    try: open(p,'w').write(line+'\\n')\n    except OSError: pass\nsys.exit(7)\n");
        let (_t, tree) = staged(&[
            ("forge.py", &forge),
            ("killer.py", b"import os,signal\nprint('{\"verified\": true}',flush=True)\nos.kill(os.getppid(),signal.SIGKILL)\n"),
        ]);
        let r = run_one(
            &ws,
            &tree,
            &plan(
                vec![script("forge", "forge.py", &[])],
                true,
                CopyOutMode::Off,
            ),
        );
        let c = &r.commands[0];
        println!(
            "STAGE5_FORGED status={:?} stdout_len={}",
            c.status,
            c.stdout.retained.len()
        );
        assert_eq!(c.status, Some(7));
        assert_eq!(c.id, "forge");
        assert!(c.stdout.retained.starts_with(forged));
        // Killing the supervisor can only ever produce RunnerFailure.
        let r = ws
            .run_gate(
                &tree,
                &plan(
                    vec![script("killer", "killer.py", &[])],
                    true,
                    CopyOutMode::Off,
                ),
                &Cancellation::default(),
            )
            .unwrap();
        println!("STAGE5_SUPERVISOR_KILLED termination={:?}", r.termination);
        assert_eq!(r.termination, Termination::RunnerFailure);
        assert!(r.commands.is_empty());
    }

    #[test]
    fn fsize_and_work_tmpfs_enospc() {
        if !crate::isolation::test_support::isolation_or_ci_skip("fsize_and_work_tmpfs_enospc") {
            return;
        }
        let ws = backend();
        let code: &[u8] = br#"import os,json,errno
r={}
try:
    open('/work/big','wb').write(b'x'*300000); r['fsize']='ok'
except OSError as e: r['fsize']=errno.errorcode[e.errno]
def fill(d):
    n=0
    try:
        while True:
            with open(os.path.join(d,'f%d'%n),'wb') as f: f.write(b'x'*131072)
            n+=1
    except OSError as e: return n*131072,errno.errorcode[e.errno]
r['work']=fill('/work'); r['tmp']=fill('/tmp')
print(json.dumps(r))
"#;
        let (_t, tree) = staged(&[("fill.py", code)]);
        let mut p = plan(vec![script("fill", "fill.py", &[])], true, CopyOutMode::Off);
        p.limits.file_size_bytes = 256 * 1024;
        p.limits.work_tmpfs_bytes = 8 * MIB;
        let v = out_json(&run_one(&ws, &tree, &p).commands[0]);
        println!("STAGE5_FSIZE_ENOSPC {v}");
        assert_eq!(v["fsize"], "EFBIG");
        assert_eq!(v["work"][1], "ENOSPC");
        assert!(v["work"][0].as_u64().unwrap() <= 8 * MIB);
        assert_eq!(v["tmp"][1], "ENOSPC");
        assert!(v["tmp"][0].as_u64().unwrap() <= WRITABLE_TMPFS_BYTES);
    }

    #[test]
    fn rss_watchdog_kills_namespace() {
        if !crate::isolation::test_support::isolation_or_ci_skip("rss_watchdog_kills_namespace") {
            return;
        }
        let ws = backend();
        let m = marker();
        let code: &[u8] = br#"import os,sys,time
for i in range(3):
    if os.fork()==0:
        b=b'\x01'*(60*1024*1024)
        time.sleep(60); os._exit(0)
time.sleep(60)
"#;
        let (_t, tree) = staged(&[("hog.py", code)]);
        let mut p = plan(vec![script("hog", "hog.py", &[&m])], true, CopyOutMode::Off);
        p.limits.rss_watchdog_bytes = 64 * MIB;
        let start = Instant::now();
        let r = ws.run_gate(&tree, &p, &Cancellation::default()).unwrap();
        println!(
            "STAGE5_WATCHDOG termination={:?} event={:?} elapsed_ms={}",
            r.termination,
            r.watchdog,
            start.elapsed().as_millis()
        );
        assert_eq!(r.termination, Termination::Cancelled);
        let w = r.watchdog.expect("soft watchdog event recorded");
        assert!(w.rss_bytes > 64 * MIB && w.threshold_bytes == 64 * MIB);
        assert!(r.commands.is_empty());
        assert!(start.elapsed() < Duration::from_secs(20));
        assert_eq!(wait_markers_gone(&m, Duration::from_secs(2)), 0);
        assert!(
            !ws.profile().cgroup_quota,
            "watchdog is soft, never a quota claim"
        );
    }

    #[test]
    fn preflight_hash_mismatch_denies_before_spawn() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "preflight_hash_mismatch_denies_before_spawn",
        ) {
            return;
        }
        let ws = backend();
        let (_t, tree) = staged(&[("ok.py", b"print(1)\n")]);
        let p = plan(vec![script("ok", "ok.py", &[])], true, CopyOutMode::Off);
        let before = spawns_here();
        let mut tampered = WorkspaceIsolation {
            profile: ws.profile.clone(),
            receipt_cap_override: None,
        };
        tampered
            .profile
            .base
            .program_hashes
            .insert(PYTHON.into(), "0".repeat(64));
        assert_eq!(
            tampered
                .run_gate(&tree, &p, &Cancellation::default())
                .unwrap_err(),
            IsolationError::IsolationUnavailable
        );
        let sock_dir = tempfile::tempdir().unwrap();
        let sock = ServiceSocketDir::create(sock_dir.path()).unwrap();
        assert!(matches!(
            tampered.start_service(&tree, &static_spec(), &sock, Cancellation::default()),
            Err(IsolationError::IsolationUnavailable)
        ));
        let mut tampered = WorkspaceIsolation {
            profile: ws.profile.clone(),
            receipt_cap_override: None,
        };
        tampered.profile.supervisor_v3_sha256 = "0".repeat(64);
        assert_eq!(
            tampered
                .run_gate(&tree, &p, &Cancellation::default())
                .unwrap_err(),
            IsolationError::IsolationUnavailable
        );
        tampered.profile = ws.profile.clone();
        tampered.profile.base.program_hashes.remove(BWRAP);
        assert_eq!(
            tampered
                .run_gate(&tree, &p, &Cancellation::default())
                .unwrap_err(),
            IsolationError::IsolationUnavailable
        );
        // Invalid controller input is refused before spawn as well.
        let invalid = [
            script("x", "missing.py", &[]),
            script("x", "/abs/ok.py", &[]),
            script("x", "../ok.py", &[]),
            script("x", "ok.py", &["nul\0arg"]),
            script("bad id", "ok.py", &[]),
            GateCommand::PythonUnittest {
                id: "u".into(),
                role: GateRole::Test,
                start_dir: "nowhere".into(),
                pattern: "test_*.py".into(),
                timeout_ms: 1000,
                output_bytes: 1024,
            },
            GateCommand::PythonUnittest {
                id: "u".into(),
                role: GateRole::Test,
                start_dir: "".into(),
                pattern: "test;rm".into(),
                timeout_ms: 1000,
                output_bytes: 1024,
            },
            GateCommand::PythonCompile {
                id: "c".into(),
                role: GateRole::Build,
                files: vec![],
                timeout_ms: 1000,
                output_bytes: 1024,
            },
            GateCommand::PythonCompile {
                id: "c".into(),
                role: GateRole::Build,
                files: vec!["ok.py".into(), "ok.py".into()],
                timeout_ms: 1000,
                output_bytes: 1024,
            },
        ];
        for cmd in invalid {
            let p = plan(vec![cmd.clone()], true, CopyOutMode::Off);
            assert_eq!(
                ws.run_gate(&tree, &p, &Cancellation::default())
                    .unwrap_err(),
                IsolationError::InvalidInput,
                "{cmd:?}"
            );
        }
        let mut many = plan(vec![], true, CopyOutMode::Off);
        assert!(ws.run_gate(&tree, &many, &Cancellation::default()).is_err());
        many.commands = (0..9)
            .map(|i| script(&format!("c{i}"), "ok.py", &[]))
            .collect();
        assert!(ws.run_gate(&tree, &many, &Cancellation::default()).is_err());
        let mut lim = plan(vec![script("ok", "ok.py", &[])], true, CopyOutMode::Off);
        lim.limits.memory_bytes = 4096 * MIB;
        assert!(ws.run_gate(&tree, &lim, &Cancellation::default()).is_err());
        let mut schema = plan(vec![script("ok", "ok.py", &[])], true, CopyOutMode::Off);
        schema.schema = "other".into();
        assert!(ws
            .run_gate(&tree, &schema, &Cancellation::default())
            .is_err());
        // A staged byte changed on the host after staging: HashMismatch, no spawn.
        {
            use std::os::unix::fs::PermissionsExt;
            let staged_file = tree.dir.join("ok.py");
            fs::set_permissions(&staged_file, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&staged_file, b"print(2)\n").unwrap();
            assert_eq!(
                ws.run_gate(&tree, &p, &Cancellation::default())
                    .unwrap_err(),
                IsolationError::HashMismatch
            );
            fs::write(&staged_file, b"print(1)\n").unwrap();
            fs::set_permissions(&staged_file, fs::Permissions::from_mode(0o400)).unwrap();
        }
        assert_eq!(spawns_here(), before, "a denied run spawned a process");
        run_one(&ws, &tree, &p);
        assert_eq!(spawns_here(), before + 1);
    }

    #[test]
    fn receipt_parser_rejects_tampering() {
        let (_t, tree) = staged(&[("a.py", b"A = 1\n"), ("b.py", b"B = 1\n")]);
        let mut p = plan(
            vec![script("one", "a.py", &[]), script("two", "b.py", &[])],
            true,
            CopyOutMode::Contents,
        );
        let argvs = validate_plan(&p, &tree).unwrap();
        let cmd = |id: &str, argv: &[String], status: i32, out: &str| {
            serde_json::json!({"id": id, "argv": argv, "status": status, "termination": "completed",
                "stdout_hex": hex(out.as_bytes()), "stdout_total": out.len(), "stderr_hex": "", "stderr_total": 0, "elapsed_ms": 5})
        };
        let empty =
            serde_json::json!({"changed": [], "created": [], "deleted": [], "rejected": []});
        let good = serde_json::json!({"commands": [cmd("one", &argvs[0], 0, "ok"), cmd("two", &argvs[1], 0, "ok")], "skipped": [], "copy_out": empty});
        let parse = |v: &serde_json::Value, p: &GatePlan| {
            parse_gate_receipt(v.to_string().as_bytes(), p, &argvs, &tree.manifest)
        };
        assert!(parse(&good, &p).is_ok());
        let mut cases: Vec<(&str, serde_json::Value)> = Vec::new();
        let mut v = good.clone();
        v["extra"] = true.into();
        cases.push(("unknown field", v));
        let mut v = good.clone();
        v["commands"][0]["argv"][6] = "/work/b.py".into();
        cases.push(("argv differs", v));
        let mut v = good.clone();
        v["commands"][0]["stdout_hex"] = "zz".into();
        cases.push(("bad hex", v));
        let mut v = good.clone();
        v["commands"][0]["stdout_total"] = 1.into();
        cases.push(("total/retained mismatch", v));
        let mut v = good.clone();
        v["commands"][0]["termination"] = "output_limit".into();
        cases.push(("unexpected termination", v));
        let mut v = good.clone();
        v["commands"][0]["status"] = 1.into();
        cases.push(("continued after failure with stop_on_failure", v));
        let mut v = good.clone();
        v["commands"][1]["id"] = "zzz".into();
        cases.push(("wrong id", v));
        let mut v = good.clone();
        v["commands"] = serde_json::json!([cmd("one", &argvs[0], 0, "ok")]);
        v["skipped"] = serde_json::json!(["two"]);
        cases.push(("skipped after success", v));
        let mut v = good.clone();
        v["copy_out"]["changed"] = serde_json::json!([{"path": "a.py", "sha256": hash(b"A = 9\n"), "size": 6, "hex": hex(b"A = 8\n")}]);
        cases.push(("copy-out sha mismatch", v));
        let mut v = good.clone();
        v["copy_out"]["created"] = serde_json::json!([{"path": "../x", "sha256": hash(b"x"), "size": 1, "hex": hex(b"x")}]);
        cases.push(("copy-out traversal", v));
        let mut v = good.clone();
        v["copy_out"]["created"] = serde_json::json!([{"path": "a.py", "sha256": hash(b"x"), "size": 1, "hex": hex(b"x")}]);
        cases.push(("created path in manifest", v));
        let mut v = good.clone();
        v["copy_out"]["changed"] = serde_json::json!([{"path": "a.py", "sha256": hash(b"A = 1\n"), "size": 6, "hex": hex(b"A = 1\n")}]);
        cases.push(("unchanged reported as changed", v));
        let mut v = good.clone();
        v["copy_out"]["deleted"] = serde_json::json!(["zzz.py"]);
        cases.push(("deleted not in manifest", v));
        let mut v = good.clone();
        v["copy_out"]["created"] = serde_json::json!([{"path": "__pycache__/x.pyc", "sha256": hash(b"x"), "size": 1, "hex": hex(b"x")}]);
        cases.push(("ignored dir reported", v));
        let mut v = good.clone();
        v["copy_out"]["created"] =
            serde_json::json!([{"path": "n.txt", "sha256": hash(b"x"), "size": 1, "hex": null}]);
        cases.push(("contents mode without bytes", v));
        for (name, v) in &cases {
            assert!(parse(v, &p).is_err(), "accepted tampered receipt: {name}");
        }
        p.copy_out.mode = CopyOutMode::Off;
        let mut v = good.clone();
        v["copy_out"]["deleted"] = serde_json::json!(["a.py"]);
        assert!(parse(&v, &p).is_err(), "copy-out reported while off");
        println!("STAGE5_RECEIPT_TAMPER rejected={}", cases.len() + 1);
    }

    #[test]
    fn staged_tree_selection_and_metadata_fences() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("repo");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("src/temp.py"), TEMP).unwrap();
        fs::write(root.join("src/duration.py"), DURATION).unwrap();
        fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::write(root.join(".git/big"), vec![b'x'; 5000]).unwrap();
        let outside = t.path().join("private");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("marker"), b"PRIVATE-MARKER").unwrap();
        std::os::unix::fs::symlink(outside.join("marker"), root.join("src/link.py")).unwrap();
        let scratch = t.path().join("scratch");
        fs::create_dir(&scratch).unwrap();
        let policy = SnapshotPolicy::new(vec![root.clone()], scratch.clone()).unwrap();
        let label = SelectionLabel::for_root(&root).unwrap();
        assert!(label.source_id.starts_with("repo:") && label.source_id.len() == 69);
        let files = vec!["src/temp.py".to_string(), "src/duration.py".to_string()];
        let sel = capture_selection(&policy, &root, &label, "src/temp.py", &files).unwrap();
        let manifest: BTreeMap<String, String> = sel
            .files
            .iter()
            .map(|(p, b)| (p.clone(), hash(b)))
            .collect();
        let b = sel.snapshot.binding();
        assert_eq!(b.files, manifest);
        assert_eq!(b.source.source_version, SELECTION_SCHEMA);
        assert_eq!(b.source.source_commit, manifest_sha256(&manifest));
        assert_eq!(b.source.file_digest, hash(TEMP));
        for bad in [
            vec!["src/link.py".to_string()],
            vec!["../private/marker".to_string()],
            vec![outside.join("marker").to_string_lossy().into_owned()],
            vec!["src//temp.py".to_string()],
        ] {
            let err = capture_selection(&policy, &root, &label, &bad[0], &bad);
            assert!(err.is_err(), "{bad:?}");
        }
        assert!(
            capture_selection(&policy, &outside, &label, "marker", &["marker".into()]).is_err()
        );
        let meta = read_metadata(&policy, &root, &[".git/HEAD".into()]).unwrap();
        assert_eq!(meta[".git/HEAD"], b"ref: refs/heads/main\n");
        assert!(read_metadata(&policy, &root, &[".git/big".into()]).is_err());
        assert!(read_metadata(
            &policy,
            &root,
            &["a".into(), "b".into(), "c".into(), "d".into(), "e".into()]
        )
        .is_err());
        // StagedTree: content identity, read-only files, tamper detection, cleanup.
        let tree = StagedTree::from_files(&scratch, &sel.files, &TreeLimits::default()).unwrap();
        assert_eq!(tree.tree_sha256(), manifest_sha256(&manifest));
        assert_eq!(tree.manifest(), &manifest);
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(tree.dir.join("src/temp.py"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o400);
            let dmode = fs::metadata(&tree.dir).unwrap().permissions().mode();
            assert_eq!(dmode & 0o077, 0);
        }
        let dir = tree.dir.clone();
        drop(tree);
        assert!(!dir.exists());
        let mut too_big = BTreeMap::new();
        too_big.insert("x.py".to_string(), vec![0u8; MAX_TREE_FILE_BYTES + 1]);
        assert!(StagedTree::from_files(&scratch, &too_big, &TreeLimits::default()).is_err());
        let mut alias = BTreeMap::new();
        alias.insert("a".to_string(), b"1".to_vec());
        alias.insert("a/b".to_string(), b"2".to_vec());
        assert!(StagedTree::from_files(&scratch, &alias, &TreeLimits::default()).is_err());
        let mut deep = BTreeMap::new();
        deep.insert("a/b/c/d/e/f/g/h/i.py".to_string(), b"1".to_vec());
        assert!(StagedTree::from_files(&scratch, &deep, &TreeLimits::default()).is_err());
    }

    #[test]
    fn frames_and_plans_are_strict_serde() {
        // Model/tool JSON cannot smuggle extra fields into a plan or a spec.
        let p = plan(vec![script("a", "a.py", &[])], true, CopyOutMode::Off);
        let mut v = serde_json::to_value(&p).unwrap();
        v["commands"][0]["shell"] = "rm -rf /".into();
        assert!(serde_json::from_value::<GatePlan>(v).is_err());
        let mut v = serde_json::to_value(static_spec()).unwrap();
        v["share_net"] = true.into();
        assert!(serde_json::from_value::<ServiceSpec>(v).is_err());
        assert!(
            serde_json::from_str::<svc_frame_probe::Frame>(r#"{"t":"exit","status":0,"x":1}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<svc_frame_probe::Frame>(r#"{"t":"exit","status":0}"#).is_ok()
        );
    }
    mod svc_frame_probe {
        // Same shape as the private svc::Frame (checked separately for serde
        // internally-tagged + deny_unknown_fields behaviour).
        #[derive(serde::Deserialize)]
        #[serde(tag = "t", rename_all = "snake_case", deny_unknown_fields)]
        #[allow(dead_code)]
        pub(super) enum Frame {
            Log { s: String, b: String },
            Drop { bytes: u64, records: u64 },
            Exit { status: Option<i32> },
        }
    }

    #[test]
    fn source_scan_command_new_only_setpriv_and_non_linux_first() {
        let src = include_str!("workspace.rs");
        let needle = ["Command", "::new("].concat();
        let uses: Vec<&str> = src
            .match_indices(&needle)
            .map(|(i, _)| &src[i..i + needle.len() + 8])
            .collect();
        assert!(!uses.is_empty());
        for u in &uses {
            assert!(u.ends_with("SETPRIV)"), "{u}");
        }
        for f in [
            "pub fn new(base: &LinuxIsolation)",
            "pub fn run_gate(",
            "pub fn start_service(",
        ] {
            let body = &src[src.find(f).unwrap()..];
            let first_check = body.find("if !cfg!(target_os = \"linux\")").unwrap();
            let first_command = body.find("self.command(").unwrap_or(usize::MAX);
            let first_preflight = body.find("self.preflight()").unwrap();
            assert!(
                first_check < first_preflight && first_check < first_command,
                "{f}"
            );
        }
        assert!(!SUPERVISOR_V3.contains("threading") && !SUPERVISOR_V3.contains("Thread"));
        assert!(!SUPERVISOR_V3.contains("share-net"));
    }

    // --------------------------------------------------------- service mode

    const TREES: &[u8] = br#"import http.server,json,os,sys,socket,errno,subprocess,time
port=int(sys.argv[1])
info={}
try:
    open('/run/pai-preview/x','w'); info['sockdir']='writable'
except OSError as e: info['sockdir']=errno.errorcode[e.errno]
info['listing']=os.listdir('/run/pai-preview')
if len(sys.argv)>2 and sys.argv[2]!='-':
    s=socket.socket()
    try:
        s.settimeout(2); s.connect(('127.0.0.1',int(sys.argv[2]))); info['host_loopback']='connected'
    except OSError as e: info['host_loopback']=errno.errorcode.get(e.errno,str(e))
if len(sys.argv)>3:
    for i in range(2): subprocess.Popen(['/usr/bin/python3','-c','import time\nwhile True: time.sleep(1)','%s-%d'%(sys.argv[3],i)])
print('INFO '+json.dumps(info),flush=True)
TREES=[{'name':'oak'},{'name':'birch'},{'name':'cedar'}]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,f,*a):
        sys.stderr.write('REQ %s %s\n'%(self.command,self.path)); sys.stderr.flush()
    def send(self,code,body,ct):
        self.send_response(code); self.send_header('Content-Type',ct); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_GET(self):
        if self.path=='/api/trees': self.send(200,json.dumps(TREES).encode(),'application/json')
        elif self.path=='/slow':
            time.sleep(4); self.send(200,b'slow','text/plain')
        else: self.send(200,b'<h1>Trees</h1>','text/html')
    def do_POST(self):
        n=int(self.headers.get('Content-Length','0')); self.send(201,self.rfile.read(n),'application/octet-stream')
http.server.HTTPServer(('127.0.0.1',port),H).serve_forever()
"#;

    fn static_spec() -> ServiceSpec {
        ServiceSpec {
            schema: SERVICE_SPEC_SCHEMA.into(),
            command: ServiceCommand::PythonStaticServer { dir: String::new() },
            listen_port: 8000,
            tunnels_pool: 2,
            tunnels_max: 4,
            limits: service_limits(),
            log_rate_bytes_per_s: 64 * 1024,
        }
    }
    fn service_limits() -> WorkspaceLimits {
        WorkspaceLimits {
            cpu_seconds: 60,
            memory_bytes: 256 * MIB,
            processes: 32,
            work_tmpfs_bytes: 16 * MIB,
            file_size_bytes: MIB,
            open_files: 64,
            total_timeout_ms: 120_000,
            rss_watchdog_bytes: 1024 * MIB,
        }
    }
    fn trees_spec(args: &[&str]) -> ServiceSpec {
        ServiceSpec {
            command: ServiceCommand::PythonScript {
                script: "server.py".into(),
                args: args.iter().map(|s| s.to_string()).collect(),
            },
            ..static_spec()
        }
    }
    struct Svc {
        _scratch: tempfile::TempDir,
        _tree: StagedTree,
        _sock: ServiceSocketDir,
        handle: Option<ServiceHandle>,
    }
    fn start(ws: &WorkspaceIsolation, spec: &ServiceSpec, files: &[(&str, &[u8])]) -> Svc {
        let scratch = tempfile::tempdir().unwrap();
        let map = files
            .iter()
            .map(|(p, b)| (p.to_string(), b.to_vec()))
            .collect();
        let tree = StagedTree::from_files(scratch.path(), &map, &TreeLimits::default()).unwrap();
        let sock = ServiceSocketDir::create(scratch.path()).unwrap();
        let handle = ws
            .start_service(&tree, spec, &sock, Cancellation::default())
            .unwrap();
        Svc {
            _scratch: scratch,
            _tree: tree,
            _sock: sock,
            handle: Some(handle),
        }
    }
    impl Svc {
        fn h(&self) -> &ServiceHandle {
            self.handle.as_ref().unwrap()
        }
    }
    /// Minimal HTTP/1.1 client over one tunnel (test-only; the product path is
    /// task_preview's bridge).
    fn http(link: &ServiceLink, method: &str, path: &str, body: &[u8]) -> Option<(u16, Vec<u8>)> {
        let mut t = link.take_tunnel(Duration::from_secs(5))?;
        t.set_timeouts(Some(Duration::from_secs(10)), Some(Duration::from_secs(10)))
            .ok()?;
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            link.listen_port(),
            body.len()
        );
        t.write_all(b"G").ok()?;
        t.write_all(req.as_bytes()).ok()?;
        t.write_all(body).ok()?;
        let mut resp = Vec::new();
        let _ = t.read_to_end(&mut resp);
        let text = String::from_utf8_lossy(&resp);
        let status = text.split_whitespace().nth(1)?.parse().ok()?;
        let split = resp.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
        Some((status, resp[split..].to_vec()))
    }
    fn wait_http(link: &ServiceLink, path: &str, wait: Duration) -> Option<(u16, Vec<u8>)> {
        let start = Instant::now();
        while start.elapsed() < wait {
            if let Some(r) = http(link, "GET", path, b"") {
                return Some(r);
            }
            thread::sleep(Duration::from_millis(100));
        }
        None
    }
    fn logs(h: &ServiceHandle, into: &mut Vec<u8>) -> Vec<ServiceEvent> {
        let mut other = Vec::new();
        for e in h.poll_events(10_000) {
            match e {
                ServiceEvent::Log { bytes, .. } => into.extend_from_slice(&bytes),
                e => other.push(e),
            }
        }
        other
    }
    fn wait_log(h: &ServiceHandle, needle: &str, wait: Duration) -> String {
        let mut all = Vec::new();
        let start = Instant::now();
        while start.elapsed() < wait {
            logs(h, &mut all);
            if String::from_utf8_lossy(&all).contains(needle) {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        String::from_utf8_lossy(&all).into_owned()
    }

    /// Appendix A P1/P3: reverse-dial relay over a READ-ONLY socket dir bind.
    #[test]
    fn service_reverse_dial_roundtrip() {
        if !crate::isolation::test_support::isolation_or_ci_skip("service_reverse_dial_roundtrip") {
            return;
        }
        let ws = backend();
        let svc = start(&ws, &trees_spec(&["{port}"]), &[("server.py", TREES)]);
        let link = svc.h().link();
        let (status, body) =
            wait_http(&link, "/", Duration::from_secs(10)).expect("service answered");
        assert_eq!((status, body.as_slice()), (200, &b"<h1>Trees</h1>"[..]));
        let (status, body) = http(&link, "GET", "/api/trees", b"").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status, 200);
        assert_eq!(
            v,
            serde_json::json!([{"name":"oak"},{"name":"birch"},{"name":"cedar"}])
        );
        let (status, body) = http(&link, "POST", "/echo", b"pine").unwrap();
        assert_eq!((status, body.as_slice()), (201, &b"pine"[..]));
        let log = wait_log(svc.h(), "REQ POST /echo", Duration::from_secs(5));
        println!(
            "STAGE5_SERVICE_LOG {}",
            log.lines().take(4).collect::<Vec<_>>().join(" | ")
        );
        assert!(
            log.contains(r#"INFO {"sockdir": "EROFS", "listing": ["ctl.sock"]}"#),
            "{log}"
        );
        assert!(log.contains("REQ GET /api/trees"));
        assert!(svc.h().is_running());
        let mut svc = svc;
        let report = svc.handle.take().unwrap().stop(Duration::from_secs(3));
        println!("STAGE5_SERVICE_STOP {report:?}");
        assert!(report.killed);
        assert_eq!(report.descendants_alive_after, 0);
        assert!(!link.is_running());
        assert!(link.take_tunnel(Duration::from_millis(100)).is_none());
    }

    /// Appendix A P1: the dev server cannot reach host loopback services.
    #[test]
    fn service_cannot_reach_host_loopback() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "service_cannot_reach_host_loopback",
        ) {
            return;
        }
        let ws = backend();
        let host = TcpListener::bind("127.0.0.1:0").unwrap();
        host.set_nonblocking(true).unwrap();
        let port = host.local_addr().unwrap().port().to_string();
        let svc = start(
            &ws,
            &trees_spec(&["{port}", &port]),
            &[("server.py", TREES)],
        );
        let log = wait_log(svc.h(), "INFO ", Duration::from_secs(10));
        println!("STAGE5_HOST_LOOPBACK {}", log.lines().next().unwrap_or(""));
        assert!(log.contains(r#""host_loopback": "ECONNREFUSED""#), "{log}");
        assert!(
            host.accept().is_err(),
            "host listener was reached from the sandbox"
        );
        // A static-server spec works through the same relay.
        let st = start(
            &ws,
            &static_spec(),
            &[("index.html", b"<p>static trees</p>")],
        );
        let (status, body) =
            wait_http(&st.h().link(), "/index.html", Duration::from_secs(10)).unwrap();
        assert_eq!(
            (status, body.as_slice()),
            (200, &b"<p>static trees</p>"[..])
        );
    }

    #[test]
    fn service_stop_kills_tree_neighbor_survives() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "service_stop_kills_tree_neighbor_survives",
        ) {
            return;
        }
        let ws = backend();
        let (ma, mb) = (marker(), marker());
        let mut a = start(
            &ws,
            &trees_spec(&["{port}", "-", &ma]),
            &[("server.py", TREES)],
        );
        let b = start(
            &ws,
            &trees_spec(&["{port}", "-", &mb]),
            &[("server.py", TREES)],
        );
        assert!(wait_http(&a.h().link(), "/", Duration::from_secs(10)).is_some());
        assert!(wait_http(&b.h().link(), "/", Duration::from_secs(10)).is_some());
        assert_eq!(marker_procs(&ma), 2);
        assert_eq!(marker_procs(&mb), 2);
        let report = a.handle.take().unwrap().stop(Duration::from_secs(3));
        println!(
            "STAGE5_NEIGHBOR stop={report:?} a_left={} b_left={}",
            marker_procs(&ma),
            marker_procs(&mb)
        );
        assert!(report.killed && report.descendants_alive_after == 0);
        assert_eq!(wait_markers_gone(&ma, Duration::from_secs(2)), 0);
        assert_eq!(marker_procs(&mb), 2, "neighbor service was touched");
        let (status, _) = http(&b.h().link(), "GET", "/api/trees", b"").unwrap();
        assert_eq!(status, 200);
        drop(b);
        assert_eq!(
            wait_markers_gone(&mb, Duration::from_secs(3)),
            0,
            "drop must stop the service"
        );
    }

    /// Appendix A P5: the sandbox dies with the owning PROCESS (helper exits
    /// without stopping its service; no survivors).
    #[test]
    fn service_parent_death_teardown() {
        if !crate::isolation::test_support::isolation_or_ci_skip("service_parent_death_teardown") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let m = marker();
        let exe = std::env::current_exe().unwrap();
        let mut helper = Command::new(SETPRIV)
            .arg("--no-new-privs")
            .arg(&exe)
            .args([
                "--exact",
                "isolation::workspace::tests::helper_service_parent_death",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("PAI_S5_HELPER_DIR", dir.path())
            .env("PAI_S5_HELPER_MARKER", &m)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let status = loop {
            if let Some(s) = helper.try_wait().unwrap() {
                break s;
            }
            assert!(start.elapsed() < Duration::from_secs(60), "helper hung");
            thread::sleep(Duration::from_millis(50));
        };
        let ready = fs::read_to_string(dir.path().join("ready")).unwrap_or_default();
        let left = wait_markers_gone(&m, Duration::from_secs(3));
        println!(
            "STAGE5_P5_PARENT_DEATH helper_status={status:?} ready={ready:?} survivors={left}"
        );
        assert_eq!(ready, "ready 2");
        assert_eq!(left, 0, "service processes survived the owning process");
    }
    #[test]
    #[ignore = "helper process for service_parent_death_teardown"]
    fn helper_service_parent_death() {
        let (Ok(dir), Ok(m)) = (
            std::env::var("PAI_S5_HELPER_DIR"),
            std::env::var("PAI_S5_HELPER_MARKER"),
        ) else {
            return;
        };
        let ws = backend();
        let svc = start(
            &ws,
            &trees_spec(&["{port}", "-", &m]),
            &[("server.py", TREES)],
        );
        let ok = wait_http(&svc.h().link(), "/", Duration::from_secs(10)).is_some();
        fs::write(
            Path::new(&dir).join("ready"),
            format!("ready {}", if ok { marker_procs(&m) } else { 0 }),
        )
        .unwrap();
        // Exit the PROCESS without running any destructor (no stop, no drop).
        std::process::exit(0);
    }

    /// Appendix A P4: start_service called from a short-lived thread; the
    /// owner thread (not the caller) keeps owning the sandbox.
    #[test]
    fn service_survives_spawning_thread_exit() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "service_survives_spawning_thread_exit",
        ) {
            return;
        }
        let ws = Arc::new(backend());
        let ws2 = ws.clone();
        let svc =
            thread::spawn(move || start(&ws2, &trees_spec(&["{port}"]), &[("server.py", TREES)]))
                .join()
                .unwrap();
        thread::sleep(Duration::from_millis(500));
        assert!(svc.h().is_running());
        let (status, _) =
            wait_http(&svc.h().link(), "/api/trees", Duration::from_secs(10)).unwrap();
        assert_eq!(status, 200);
        let mut svc = svc;
        let report = svc.handle.take().unwrap().stop(Duration::from_secs(3));
        assert!(report.killed && report.descendants_alive_after == 0);
    }

    #[test]
    fn service_logs_framed_hex_rate_limited() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "service_logs_framed_hex_rate_limited",
        ) {
            return;
        }
        let ws = backend();
        let noisy: &[u8] = br#"import sys,time,http.server
sys.stdout.write('\n{"t":"exit","status":0}\n'); sys.stdout.flush()
sys.stderr.buffer.write(bytes(range(256))+b'\n'); sys.stderr.flush()
time.sleep(0.3)
sys.stdout.write('z'*200000); sys.stdout.flush()
time.sleep(1.5)
sys.stdout.write('after-burst\n'); sys.stdout.flush()
http.server.HTTPServer(('127.0.0.1',int(sys.argv[1])),http.server.SimpleHTTPRequestHandler).serve_forever()
"#;
        let mut spec = trees_spec(&["{port}"]);
        spec.log_rate_bytes_per_s = 8192;
        let svc = start(&ws, &spec, &[("server.py", noisy)]);
        let mut all = Vec::new();
        let mut events = Vec::new();
        let start_t = Instant::now();
        while start_t.elapsed() < Duration::from_secs(8)
            && !String::from_utf8_lossy(&all).contains("after-burst")
        {
            events.extend(logs(svc.h(), &mut all));
            thread::sleep(Duration::from_millis(50));
        }
        events.extend(logs(svc.h(), &mut all));
        let dropped: u64 = events
            .iter()
            .map(|e| {
                if let ServiceEvent::Dropped { bytes, .. } = e {
                    *bytes
                } else {
                    0
                }
            })
            .sum();
        let text = String::from_utf8_lossy(&all);
        println!(
            "STAGE5_LOG_FRAMES retained={} dropped={} events={:?}",
            all.len(),
            dropped,
            events
                .iter()
                .filter(|e| !matches!(e, ServiceEvent::Dropped { .. }))
                .collect::<Vec<_>>()
        );
        assert!(
            text.contains(r#"{"t":"exit","status":0}"#),
            "forged frame must arrive as data"
        );
        assert!(
            all.windows(256)
                .any(|w| w == (0..=255u8).collect::<Vec<_>>().as_slice()),
            "binary bytes preserved"
        );
        assert!(dropped > 100_000, "rate limiter did not drop: {dropped}");
        assert!(all.len() < 120_000);
        assert!(!events
            .iter()
            .any(|e| matches!(e, ServiceEvent::Exited { .. } | ServiceEvent::RunnerFailure)));
        assert!(
            svc.h().is_running(),
            "forged exit frame stopped the service"
        );
        assert!(text.contains("after-burst"));
    }

    /// Appendix A P2/P3: the supervisor relay is thread-free and serves 12
    /// concurrent requests through a 4/8 pool under RLIMIT_AS 128 MiB.
    #[test]
    fn thread_free_relay_under_as_limit() {
        if !crate::isolation::test_support::isolation_or_ci_skip("thread_free_relay_under_as_limit")
        {
            return;
        }
        assert!(!SUPERVISOR_V3.contains("threading"));
        let ws = backend();
        let mut spec = trees_spec(&["{port}"]);
        spec.limits.memory_bytes = 128 * MIB;
        spec.limits.processes = 16;
        spec.tunnels_pool = 4;
        spec.tunnels_max = 8;
        let svc = start(&ws, &spec, &[("server.py", TREES)]);
        let link = svc.h().link();
        assert!(wait_http(&link, "/", Duration::from_secs(10)).is_some());
        let started = Instant::now();
        let workers: Vec<_> = (0..12)
            .map(|_| {
                let link = link.clone();
                thread::spawn(move || http(&link, "GET", "/api/trees", b"").map(|r| r.0))
            })
            .collect();
        let codes: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        println!(
            "STAGE5_P3_CONCURRENT codes={codes:?} ms={}",
            started.elapsed().as_millis()
        );
        assert!(codes.iter().all(|c| *c == Some(200)), "{codes:?}");
        let log = wait_log(svc.h(), "REQ GET /api/trees", Duration::from_secs(3));
        assert!(!log.contains("can't start new thread"));
        let events = svc.h().poll_events(100);
        assert!(!events
            .iter()
            .any(|e| matches!(e, ServiceEvent::RunnerFailure)));
    }
}
