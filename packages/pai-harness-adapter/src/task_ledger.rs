//! Stage 5 durable encrypted task ledger (design §3).
//!
//! * Storage: the SAME `Arc<Mutex<Option<Vault>>>` as the product vault. No new
//!   database and no plaintext file. Three record roles, all `RecordType::Memory`,
//!   `PrivacyLevel::Private`, origin `PAI`/`LOCAL`, canonical AAD v2:
//!   one sealed task index (namespaced UUID, overwritten atomically), one sealed
//!   head per task (random UUID, overwritten atomically per commit) and
//!   create-only content blobs (UUID derived from the ledger key).
//! * Every record is an HMAC-sealed typed envelope. The domain key is derived
//!   from the vault master key, so a generic vault writer (generic record
//!   commands, the memory provider writing a colliding UUID) cannot forge a head,
//!   index or blob. A master-key holder can; a disk-level rollback to an older
//!   sealed head is NOT detected (see [`LEDGER_RESIDUALS`]).
//! * Each commit is a single atomic head write (`Vault::write_record`: journal,
//!   temp, fsync, rename). The journal is validated by deterministic replay at
//!   commit time AND at every load, so the §3.4 state machine is a property of
//!   every head the ledger will ever return. Commit boundaries are persisted
//!   (`TaskHead::commit_starts`), so the "same commit" rules hold exactly at
//!   load too.
//! * The ledger persists no process identifier of any kind.

use crate::coding_task::ports::{
    manifest_sha256, EditDenial, FileDecision, GateCommand, GatePlan, GateRole, HttpCheckRecord,
    OracleReason, PreviewSpec, ReadyState, ReviewState, ServiceDescriptor, StopRecord,
    WorkingSetManifest, WorktreeBinding, GATE_PLAN_SCHEMA,
};
use crate::coding_task::RepairBudget;
use crate::isolation::Termination;
use crate::knowledge::{self as kv, KnowledgeStoreError};
use hmac::{Hmac, Mac};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use unoone_vault_core::{PrivacyLevel, Record, Vault};

pub const LEDGER_SCHEMA: &str = "inbharat.pai.task-ledger.v1";
pub const LEDGER_HMAC_DOMAIN: &str = "inbharat.pai.stage5.task-ledger.hmac-sha256.v1";
const MAC_PREFIX: &[u8] = b"inbharat.pai.task-ledger\0v1\0";
const INDEX_NAMESPACE: &[u8] = b"inbharat.pai.stage5.task-index.v1\0";

/// Hard cap on ledger records (index + heads + blobs); counts toward the memory
/// provider's 8,192-record repair scan budget.
pub const MAX_LEDGER_RECORDS: u32 = 2048;
pub const MAX_TASKS: usize = 64;
pub const MAX_BLOBS_PER_TASK: usize = 128;
/// Sealed head / index / blob envelope cap.
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;
/// Raw blob payload cap (hex-encoded inside the sealed envelope).
pub const MAX_BLOB_RAW_BYTES: usize = 448 * 1024;
pub const MAX_JOURNAL_ENTRIES: usize = 4096;
pub const MAX_EVENTS_PER_COMMIT: usize = 64;
pub const MAX_BLOBS_PER_COMMIT: usize = 32;
pub const MAX_OBJECTIVE_BYTES: usize = 4096;
pub const MAX_SELECTED_FILES: usize = 16;
pub const MAX_WORKING_SET_FILES: usize = 32;
pub const MAX_FILE_BYTES: usize = 256 * 1024;
pub const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

/// Known, accepted gaps. Surfaced by the ledger so they are reported, not hidden.
pub const LEDGER_RESIDUALS: &[&str] = &[
    "no-anti-rollback: an older sealed task head restored from disk is accepted (no monotonic root)",
    "keyholder-forgery: a holder of the unlocked vault master key can derive the ledger key and forge state",
    "orphan-blob: a crash between a blob write and the head write leaves at most the unreferenced encrypted blobs of that commit",
    "observation-truth: a worktree observation must equal the intent's pre/post images, but whether the worktree holds them is the controller's read-only probe; an in-process caller of the pub commit API can assert a matching one",
];

// ---------------------------------------------------------------------------
// Errors (messages carry no record payload or file content)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    Locked,
    NotFound,
    Conflict,
    Corrupt,
    IllegalTransition(&'static str),
    LedgerFull,
    Invalid(&'static str),
    Persistence,
}
impl std::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "task ledger: {self:?}")
    }
}
impl std::error::Error for LedgerError {}
impl From<KnowledgeStoreError> for LedgerError {
    fn from(e: KnowledgeStoreError) -> Self {
        match e {
            KnowledgeStoreError::Locked => Self::Locked,
            KnowledgeStoreError::Conflict => Self::Conflict,
            KnowledgeStoreError::NotFound => Self::NotFound,
            KnowledgeStoreError::Limit => Self::LedgerFull,
            KnowledgeStoreError::Persistence => Self::Persistence,
            _ => Self::Corrupt,
        }
    }
}
pub type Result<T> = std::result::Result<T, LedgerError>;

// ---------------------------------------------------------------------------
// Identity, epoch and boot
// ---------------------------------------------------------------------------

/// 128-bit OS-random task identifier, lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TaskId(String);
impl TaskId {
    pub fn random() -> Self {
        Self(random_hex16())
    }
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() == 32 && is_lower_hex(value) {
            Ok(Self(value.to_owned()))
        } else {
            Err(LedgerError::Invalid("task id"))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for TaskId {
    type Error = LedgerError;
    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}
impl From<TaskId> for String {
    fn from(value: TaskId) -> Self {
        value.0
    }
}
impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identity of the current process (`boot_id`, random per process) plus the
/// shared lock-epoch counter owned by `CodingTaskService`.
#[derive(Debug, Clone)]
pub struct BootInfo {
    pub boot_id: String,
    counter: Arc<AtomicU64>,
}
impl BootInfo {
    pub fn new(counter: Arc<AtomicU64>) -> Self {
        Self {
            boot_id: random_hex16(),
            counter,
        }
    }
    /// Capture the CURRENT epoch. Long operations capture at admission.
    pub fn guard(&self) -> EpochGuard {
        EpochGuard {
            boot_id: self.boot_id.clone(),
            epoch: self.counter.load(Ordering::SeqCst),
            counter: self.counter.clone(),
        }
    }
    pub fn current_epoch(&self) -> u64 {
        self.counter.load(Ordering::SeqCst)
    }
}

/// Lock epoch captured at admission and re-checked under the vault mutex at
/// every commit: a late result after a lock/unlock cycle is never persisted.
#[derive(Debug, Clone)]
pub struct EpochGuard {
    boot_id: String,
    epoch: u64,
    counter: Arc<AtomicU64>,
}
impl EpochGuard {
    pub fn check(&self) -> Result<()> {
        if self.counter.load(Ordering::SeqCst) == self.epoch {
            Ok(())
        } else {
            Err(LedgerError::Locked)
        }
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn boot_id(&self) -> &str {
        &self.boot_id
    }
}

// ---------------------------------------------------------------------------
// Persisted schema (§3.2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub objective: String,
    pub repository: RepositoryLabel,
    pub selected_files: BTreeSet<String>,
    pub primary: String,
    pub oracle_files: BTreeSet<String>,
    pub oracle_visibility: OracleVisibility,
    pub acceptance: Vec<AcceptanceCriterion>,
    pub gate_plan: GatePlan,
    pub preview: Option<PreviewSpec>,
    pub repair: RepairBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryLabel {
    pub display_root: String,
    pub source_id: String,
    pub branch: Option<String>,
    pub head_commit: Option<String>,
    pub label_source: LabelSource,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelSource {
    GitFilesUnverified,
    UserLabel,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleVisibility {
    Visible,
    Hidden,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub text: String,
    pub check: CriterionCheck,
    pub confirmed_by_user: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum CriterionCheck {
    GateCommand {
        command_id: String,
        expected_exit: i32,
    },
    Http {
        check_id: String,
    },
    Manual,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub revision: u32,
    pub author: PlanAuthor,
    pub steps: Vec<PlannedStep>,
    pub confirmed: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanAuthor {
    User,
    Model,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedStep {
    pub step_id: String,
    pub kind: StepKind,
    pub summary: String,
    pub effect: EffectClass,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Capture,
    Edit,
    Gate,
    Repair,
    PreviewStart,
    HttpCheck,
    PreviewStop,
    Review,
    Apply,
    RevertApplied,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    LedgerOnly,
    /// Sandbox-only, no host effect (gate, HTTP check, model call).
    Pure,
    ProcessLifecycle,
    HostWrite,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Planned,
    Started,
    Completed,
    Failed,
    Interrupted,
    CompletedByObservation,
    Abandoned,
    Skipped,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalEntry {
    pub seq: u64,
    pub prev_sha256: String,
    pub at_ms: u64,
    pub boot_id: String,
    pub epoch: u64,
    pub event: LedgerEvent,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum LedgerEvent {
    TaskOpened {
        spec_sha256: String,
        base_manifest_sha256: String,
        snapshot_sha256: String,
    },
    PlanRecorded {
        plan: Plan,
    },
    PlanConfirmed {
        revision: u32,
        ui_event_id: String,
    },
    StepStarted {
        step_id: String,
        attempt: u32,
        intent: StepIntent,
    },
    StepCompleted {
        step_id: String,
        attempt: u32,
        result: StepResultRef,
    },
    StepFailed {
        step_id: String,
        attempt: u32,
        reason: FailureReason,
        evidence: Vec<BlobRef>,
    },
    StepInterrupted {
        step_id: String,
        attempt: u32,
        observation: ReconcileObservation,
    },
    /// A recorded fact; no mutation performed.
    StepReconciled {
        step_id: String,
        attempt: u32,
        observation: ReconcileObservation,
    },
    EditApplied {
        origin: EditOrigin,
        manifest_sha256: String,
        paths: Vec<String>,
    },
    EditDenied {
        origin: EditOrigin,
        path: String,
        reason: EditDenial,
    },
    GateRecorded {
        record: GateRecord,
    },
    HttpChecksRecorded {
        record: HttpCheckRecord,
    },
    PreviewStarted {
        descriptor: ServiceDescriptor,
    },
    PreviewStopped {
        report: StopRecord,
    },
    FileReviewed {
        path: String,
        decision: FileDecision,
        reviewed_new_sha256: Option<String>,
        ui_event_id: String,
    },
    ReconcileResolved {
        step_id: String,
        resolution: ReviewResolution,
        ui_event_id: String,
    },
    AdmissionChecked {
        trace: AdmissionTrace,
    },
    /// Untrusted model text; never read by [`assess`].
    Narrative {
        role: String,
        text_sha256: String,
        blob: BlobRef,
    },
    Checkpoint {
        checkpoint: Checkpoint,
    },
    TaskClosed {
        status: TaskStatus,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepIntent {
    pub idempotency_key: String,
    pub effect: EffectClass,
    pub working_set_sha256: String,
    pub worktree: Option<WorktreeBinding>,
    /// path -> sha256, or None (= absent)
    pub pre_image: BTreeMap<String, Option<String>>,
    pub post_image: BTreeMap<String, Option<String>>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum StepResultRef {
    Nothing,
    Gate {
        gate_run_id: String,
    },
    HttpChecks {
        service_id: String,
        tree_sha256: String,
    },
    PreviewStopped {
        logs: Vec<BlobRef>,
    },
    RepairAttempt {
        record: RepairAttemptRecord,
    },
    RepairLoop {
        attempts: Vec<RepairAttemptRecord>,
        stop: RepairStop,
        last_gates: Vec<String>,
    },
    WorktreeCreated {
        binding: WorktreeBinding,
    },
    Applied {
        files: Vec<String>,
        worktree: WorktreeBinding,
        post_image: BTreeMap<String, Option<String>>,
    },
    Reverted {
        files: Vec<String>,
        worktree: WorktreeBinding,
    },
    Restored {
        files: Vec<String>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairAttemptRecord {
    pub n: u8,
    pub edits_sha256: String,
    pub gate_ref: Option<String>,
    pub applied: u32,
    pub denied: u32,
    pub proposer_failed: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairStop {
    Passed,
    BudgetExhausted,
    NoProgress,
    Infrastructure,
    OracleDenied,
    Cancelled,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum FailureReason {
    Infrastructure,
    Cancelled,
    ProposerFailed,
    PreviewStartupFailed,
    Worktree {
        error: String,
    },
    PreImageMismatch {
        path: String,
    },
    /// A worktree write failed in a way that may already have changed the
    /// target (B's `Io`, or any error after a write, or a post-image check
    /// mismatch). Never retried: the worktree is OBSERVED (reads only, the same
    /// classification as restart recovery) and the step becomes Interrupted /
    /// CompletedByObservation with the §3.5 UI options; the task pauses.
    WorktreeStateUnknown {
        error: String,
        observation: ReconcileObservation,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum ReconcileObservation {
    /// Pure step: results are never recycled.
    NotApplicable,
    /// Process lifecycle: the process died with the app; never re-owned by PID.
    ProcessNotOwned,
    MatchesPostImage {
        hashes: BTreeMap<String, Option<String>>,
    },
    MatchesPreImage,
    Unknown {
        per_path: BTreeMap<String, Option<String>>,
        identity_ok: bool,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum EditOrigin {
    Model,
    User,
    SandboxCopyOut { gate: String },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewResolution {
    Abandon,
    RetryAsNewAttempt,
    ConfirmObservation,
    MarkManuallyResolved,
    RestorePreImage,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionPurpose {
    Gate,
    Repair,
    Preview,
    HttpCheck,
    Resume,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Unsupported,
    SupportedUnverified,
    RuntimeVerified,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionTrace {
    pub purpose: AdmissionPurpose,
    pub vault_unlocked: bool,
    pub epoch: u64,
    pub capability: CapabilityState,
    pub workspace_profile_sha256: Option<String>,
    pub preflight_ok: Option<bool>,
    pub roots_ok: Option<bool>,
    pub source_unchanged: Option<bool>,
    pub worktree_identity_ok: Option<bool>,
    pub admitted: bool,
    pub at_ms: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobKind {
    FileContent,
    GateLog,
    CopyOut,
    PreviewLog,
    RequestLog,
    PatchExport,
    Narrative,
}
impl BlobKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::FileContent => "file_content",
            Self::GateLog => "gate_log",
            Self::CopyOut => "copy_out",
            Self::PreviewLog => "preview_log",
            Self::RequestLog => "request_log",
            Self::PatchExport => "patch_export",
            Self::Narrative => "narrative",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobRef {
    pub kind: BlobKind,
    pub sha256: String,
    pub size: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBlob {
    pub kind: BlobKind,
    pub bytes: Vec<u8>,
}
impl NewBlob {
    pub fn new(kind: BlobKind, bytes: Vec<u8>) -> Self {
        Self { kind, bytes }
    }
    pub fn blob_ref(&self) -> BlobRef {
        BlobRef {
            kind: self.kind,
            sha256: kv::digest(&self.bytes),
            size: self.bytes.len() as u64,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateRecord {
    pub gate_run_id: String,
    /// == staged tree_sha256
    pub working_set_sha256: String,
    pub plan_sha256: String,
    pub workspace_profile_sha256: String,
    pub commands: Vec<CommandSummary>,
    pub logs: Vec<BlobRef>,
    pub termination: Termination,
    pub elapsed_ms: u64,
    pub at_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSummary {
    pub id: String,
    pub role: GateRole,
    pub argv: Vec<String>,
    pub status: Option<i32>,
    pub termination: Termination,
    pub stdout_total_bytes: u64,
    pub stderr_total_bytes: u64,
    pub stdout_retained_bytes: u64,
    pub stderr_retained_bytes: u64,
    pub truncated: bool,
    pub log_sha256: String,
    /// <= 4 KiB, untrusted generated-process output
    pub excerpt: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub seq: u64,
    pub working_set: WorkingSetManifest,
    pub review: ReviewState,
    pub last_gate: Option<String>,
    pub repair_attempts_used: u8,
    pub preview: PreviewLedgerState,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum PreviewLedgerState {
    #[default]
    NotStarted,
    Running {
        descriptor: ServiceDescriptor,
    },
    StartupFailed {
        descriptor: ServiceDescriptor,
    },
    Stopped {
        record: StopRecord,
    },
    NotOwnedAfterRestart {
        service_id: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum TaskStatus {
    Open,
    Running,
    AwaitingReview,
    Stopped { reason: StopReason },
    Paused { reason: PauseReason },
    Applied,
    Rejected,
    Cancelled,
    Closed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    BudgetExhausted,
    NoProgress,
    Infrastructure,
    OracleDenied,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    ReviewRequired,
}
/// Controller-derived protected (oracle) set. Model input never contributes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct OracleDerivation {
    pub declared: BTreeSet<String>,
    pub derived: BTreeSet<String>,
    pub implementation_closure: BTreeSet<String>,
    pub protected: BTreeSet<String>,
    /// Owner B's rule per protected path where B's derivation (directory,
    /// conftest, imported-helper, referenced-data) contributed. Paths without
    /// an entry came from A's rules (Oracle-role command files and their
    /// import/mention closure). Keys are always a subset of `protected`.
    pub reasons: BTreeMap<String, OracleReason>,
}

/// The single mutable per-task record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskHead {
    pub task_id: TaskId,
    pub created_at_ms: u64,
    pub spec: TaskSpec,
    pub spec_sha256: String,
    pub oracle: OracleDerivation,
    pub allowed_new_prefixes: Vec<String>,
    pub base_manifest: BTreeMap<String, String>,
    pub journal: Vec<JournalEntry>,
    pub blobs: Vec<BlobRef>,
    /// Persisted commit boundaries: the journal seq of the first entry of
    /// every commit, strictly increasing. Load-time replay enforces the "same
    /// commit" rules (admission/restore before a start, edit/review with their
    /// checkpoint) exactly, one guard (boot, epoch) per commit. Empty only in
    /// heads written before boundaries were persisted: entries before the first
    /// recorded boundary replay with the adjacency rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commit_starts: Vec<u64>,
}
impl TaskHead {
    pub fn seq(&self) -> u64 {
        self.journal.len() as u64
    }
    pub fn has_blob(&self, kind: BlobKind, sha256: &str) -> bool {
        self.blobs
            .iter()
            .any(|b| b.kind == kind && b.sha256 == sha256)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskIndexEntry {
    pub task_id: TaskId,
    pub head_uuid: String,
    pub created_at_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct IndexBody {
    tasks: Vec<TaskIndexEntry>,
    /// Conservative count of every head/blob record ever created by the ledger
    /// (including orphans of interrupted commits). Written BEFORE the records.
    reserved_records: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlobBody {
    kind: BlobKind,
    sha256: String,
    hex: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    schema: String,
    vault_id: String,
    task_id: String,
    kind: String,
    seq: u64,
    body: T,
    tag: String,
}

// ---------------------------------------------------------------------------
// Ledger
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerLimits {
    pub max_records: u32,
    pub max_tasks: usize,
    pub max_blobs_per_task: usize,
    pub max_record_bytes: usize,
    pub max_journal_entries: usize,
}
impl LedgerLimits {
    pub const PRODUCTION: Self = Self {
        max_records: MAX_LEDGER_RECORDS,
        max_tasks: MAX_TASKS,
        max_blobs_per_task: MAX_BLOBS_PER_TASK,
        max_record_bytes: MAX_RECORD_BYTES,
        max_journal_entries: MAX_JOURNAL_ENTRIES,
    };
}

/// Read-only, fd-safe observation of a pinned task worktree used by
/// [`TaskLedger::recover`]. Implementations MUST NOT mutate anything.
pub trait WorktreeProbe {
    /// sha256 of each path (None = absent). `Err` = identity changed, missing
    /// or unreadable (reported as `Unknown`).
    fn observe(
        &self,
        binding: &WorktreeBinding,
        paths: &[String],
    ) -> std::result::Result<BTreeMap<String, Option<String>>, ProbeFailure>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailure {
    IdentityChanged,
    Unavailable,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reconciliation {
    pub recorded: Vec<(String, u32, ReconcileObservation)>,
    pub paused: bool,
    pub seq: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    Public,
    Recover,
}

pub struct TaskLedger {
    vault: Arc<Mutex<Option<Vault>>>,
    limits: LedgerLimits,
}

impl TaskLedger {
    pub fn new(vault: Arc<Mutex<Option<Vault>>>) -> Self {
        Self {
            vault,
            limits: LedgerLimits::PRODUCTION,
        }
    }
    #[cfg(test)]
    pub(crate) fn with_limits(vault: Arc<Mutex<Option<Vault>>>, limits: LedgerLimits) -> Self {
        Self { vault, limits }
    }
    pub fn index_uuid() -> String {
        kv::namespaced_uuid(INDEX_NAMESPACE)
    }
    pub fn residuals() -> &'static [&'static str] {
        LEDGER_RESIDUALS
    }

    fn with_vault<T>(&self, op: impl FnOnce(&mut Vault) -> Result<T>) -> Result<T> {
        let mut guard = self.vault.lock().map_err(|_| LedgerError::Persistence)?;
        let vault = guard.as_mut().ok_or(LedgerError::Locked)?;
        if !vault.is_unlocked() {
            return Err(LedgerError::Locked);
        }
        op(vault)
    }

    /// True when the shared vault is present and unlocked (admission input).
    pub fn vault_unlocked(&self) -> bool {
        self.with_vault(|_| Ok(())).is_ok()
    }

    pub fn list(&self) -> Result<Vec<TaskIndexEntry>> {
        self.with_vault(|vault| {
            Ok(load_index(vault, &self.limits)?
                .map(|(_, body, _)| body.tasks)
                .unwrap_or_default())
        })
    }

    /// Verified head (MAC, schema, vault_id, task_id, hash chain, full replay).
    pub fn load(&self, task: &TaskId) -> Result<TaskHead> {
        self.with_vault(|vault| Ok(load_head(vault, &self.limits, task)?.1))
    }

    /// Verified head plus its deterministic replay.
    pub fn load_derived(&self, task: &TaskId) -> Result<(TaskHead, Derived)> {
        let head = self.load(task)?;
        let derived = replay(&head)?;
        Ok((head, derived))
    }

    pub fn read_blob(&self, task: &TaskId, blob: &BlobRef) -> Result<Vec<u8>> {
        self.with_vault(|vault| read_blob_record(vault, &self.limits, task, blob))
    }

    pub fn read_blob_by_sha(&self, task: &TaskId, kind: BlobKind, sha256: &str) -> Result<Vec<u8>> {
        let blob = BlobRef {
            kind,
            sha256: sha256.to_owned(),
            size: 0,
        };
        self.with_vault(|vault| read_blob_record(vault, &self.limits, task, &blob))
    }

    /// Create a task: TaskOpened + initial Checkpoint, base file blobs.
    #[allow(clippy::too_many_arguments)]
    pub fn create_task(
        &self,
        epoch: &EpochGuard,
        spec: TaskSpec,
        oracle: OracleDerivation,
        allowed_new_prefixes: Vec<String>,
        base_files: &BTreeMap<String, Vec<u8>>,
        snapshot_sha256: &str,
    ) -> Result<TaskHead> {
        validate_spec(&spec)?;
        if base_files.keys().cloned().collect::<BTreeSet<_>>() != spec.selected_files {
            return Err(LedgerError::Invalid("base files must equal the selection"));
        }
        let mut total = 0usize;
        for bytes in base_files.values() {
            if bytes.len() > MAX_FILE_BYTES {
                return Err(LedgerError::Invalid("file too large"));
            }
            total += bytes.len();
        }
        if total > MAX_CAPTURE_BYTES {
            return Err(LedgerError::Invalid("selection too large"));
        }
        if !is_sha256(snapshot_sha256) {
            return Err(LedgerError::Invalid("snapshot sha"));
        }
        if !oracle.declared.is_subset(&spec.oracle_files)
            || oracle.protected != spec.oracle_files
            || !oracle.protected.is_subset(&spec.selected_files)
            || oracle.reasons.keys().any(|p| !oracle.protected.contains(p))
            || oracle
                .protected
                .iter()
                .any(|p| oracle.implementation_closure.contains(p) || *p == spec.primary)
        {
            return Err(LedgerError::Invalid("oracle derivation"));
        }
        if allowed_new_prefixes.len() > 8 || allowed_new_prefixes.iter().any(|p| !valid_prefix(p)) {
            return Err(LedgerError::Invalid("allowed new prefixes"));
        }
        let manifest: BTreeMap<String, String> = base_files
            .iter()
            .map(|(p, b)| (p.clone(), kv::digest(b)))
            .collect();
        let task_id = TaskId::random();
        let now = now_ms();
        let spec_sha256 = sha_json(&spec)?;
        let blobs: Vec<NewBlob> = base_files
            .values()
            .map(|b| NewBlob::new(BlobKind::FileContent, b.clone()))
            .collect();
        let ws = WorkingSetManifest {
            base_sha256: manifest_sha256(&manifest),
            current_sha256: manifest_sha256(&manifest),
            base: manifest.clone(),
            current: manifest.clone(),
        };
        let events = vec![
            LedgerEvent::TaskOpened {
                spec_sha256: spec_sha256.clone(),
                base_manifest_sha256: manifest_sha256(&manifest),
                snapshot_sha256: snapshot_sha256.to_owned(),
            },
            LedgerEvent::Checkpoint {
                checkpoint: Checkpoint {
                    seq: 2,
                    working_set: ws,
                    review: ReviewState::default(),
                    last_gate: None,
                    repair_attempts_used: 0,
                    preview: PreviewLedgerState::NotStarted,
                },
            },
        ];
        let mut head = TaskHead {
            task_id: task_id.clone(),
            created_at_ms: now,
            spec,
            spec_sha256,
            oracle,
            allowed_new_prefixes,
            base_manifest: manifest,
            journal: vec![],
            blobs: vec![],
            commit_starts: vec![],
        };
        self.with_vault(|vault| {
            epoch.check()?;
            let index = load_index(vault, &self.limits)?;
            let (index_meta, mut body, generation) = match index {
                Some((meta, body, generation)) => (Some(meta), body, generation),
                None => (None, IndexBody::default(), 0),
            };
            if body.tasks.len() >= self.limits.max_tasks {
                return Err(LedgerError::LedgerFull);
            }
            let plan = prepare_blobs(vault, &self.limits, &head, &blobs)?;
            let mut derived = Derived::default();
            append_entries(
                &mut head,
                &mut derived,
                epoch,
                events,
                &plan.refs,
                Origin::Public,
            )?;
            head.blobs.extend(plan.refs.iter().cloned());
            if head.blobs.len() > self.limits.max_blobs_per_task {
                return Err(LedgerError::LedgerFull);
            }
            let new_records = plan.new_records.len() as u32 + 1;
            reserve(&self.limits, &body, new_records)?;
            let head_uuid = loop {
                let candidate = kv::private_metadata().record_id;
                if candidate != Self::index_uuid()
                    && kv::optional_record(vault, &candidate)?.is_none()
                {
                    break candidate;
                }
            };
            let head_bytes = seal_envelope(vault, "head", task_id.as_str(), head.seq(), &head)?;
            if head_bytes.len() > self.limits.max_record_bytes {
                return Err(LedgerError::LedgerFull);
            }
            // Reservation first (conservative count survives any crash), then
            // blobs, then the head, then the index entry that publishes it.
            body.reserved_records += new_records;
            let index_meta = write_index(vault, &self.limits, index_meta, &body, generation + 1)?;
            write_blob_records(vault, task_id.as_str(), &plan.new_records)?;
            kv::write_new(vault, &head_uuid, &head_bytes)?;
            body.tasks.push(TaskIndexEntry {
                task_id: task_id.clone(),
                head_uuid,
                created_at_ms: now,
            });
            write_index(vault, &self.limits, Some(index_meta), &body, generation + 2)?;
            let (_, verified) = load_head(vault, &self.limits, &task_id)?;
            if verified != head {
                return Err(LedgerError::Corrupt);
            }
            Ok(verified)
        })
    }

    /// §3.3 commit protocol under the vault mutex. Returns the new seq.
    pub fn commit(
        &self,
        task: &TaskId,
        expected_seq: u64,
        epoch: &EpochGuard,
        events: Vec<LedgerEvent>,
        blobs: Vec<NewBlob>,
    ) -> Result<u64> {
        self.commit_inner(task, expected_seq, epoch, events, blobs, Origin::Public)
    }

    fn commit_inner(
        &self,
        task: &TaskId,
        expected_seq: u64,
        epoch: &EpochGuard,
        events: Vec<LedgerEvent>,
        blobs: Vec<NewBlob>,
        origin: Origin,
    ) -> Result<u64> {
        if events.is_empty() || events.len() > MAX_EVENTS_PER_COMMIT {
            return Err(LedgerError::Invalid("event count"));
        }
        if blobs.len() > MAX_BLOBS_PER_COMMIT {
            return Err(LedgerError::LedgerFull);
        }
        self.with_vault(|vault| {
            // 1. epoch
            epoch.check()?;
            // 2. load + verify
            let (mut meta, mut head) = load_head(vault, &self.limits, task)?;
            // 3. optimistic seq
            if head.seq() != expected_seq {
                return Err(LedgerError::Conflict);
            }
            if head.journal.len() + events.len() > self.limits.max_journal_entries {
                return Err(LedgerError::LedgerFull);
            }
            // 4. transitions (replay existing, then the new entries)
            let mut derived = replay(&head)?;
            let plan = prepare_blobs(vault, &self.limits, &head, &blobs)?;
            append_entries(&mut head, &mut derived, epoch, events, &plan.refs, origin)?;
            for blob in &plan.refs {
                if !head.blobs.contains(blob) {
                    head.blobs.push(blob.clone());
                }
            }
            if head.blobs.len() > self.limits.max_blobs_per_task {
                return Err(LedgerError::LedgerFull);
            }
            let head_bytes = seal_envelope(vault, "head", task.as_str(), head.seq(), &head)?;
            if head_bytes.len() > self.limits.max_record_bytes {
                return Err(LedgerError::LedgerFull);
            }
            // 5. blobs: create-only, reservation first
            if !plan.new_records.is_empty() {
                let (index_meta, mut body, generation) =
                    load_index(vault, &self.limits)?.ok_or(LedgerError::Corrupt)?;
                reserve(&self.limits, &body, plan.new_records.len() as u32)?;
                body.reserved_records += plan.new_records.len() as u32;
                write_index(vault, &self.limits, Some(index_meta), &body, generation + 1)?;
                write_blob_records(vault, task.as_str(), &plan.new_records)?;
            }
            #[cfg(test)]
            if fault_hit(FaultPoint::AfterBlobsBeforeHead) {
                return Err(LedgerError::Persistence);
            }
            // 6. head: atomic overwrite (commit point = rename), then verify
            meta.privacy_level = PrivacyLevel::Private;
            vault
                .write_record(meta, &head_bytes)
                .map_err(kv::map_error)?;
            let (_, verified) = load_head(vault, &self.limits, task)?;
            if verified != head {
                return Err(LedgerError::Corrupt);
            }
            Ok(verified.seq())
        })
    }

    /// §3.5 restart reconciliation. Reads only (the probe has no mutating
    /// method); records Interrupted/Reconciled facts; never replays a step.
    pub fn recover(
        &self,
        task: &TaskId,
        boot: &BootInfo,
        probe: &dyn WorktreeProbe,
    ) -> Result<Reconciliation> {
        let guard = boot.guard();
        for _ in 0..3 {
            let (head, derived) = self.load_derived(task)?;
            let mut events = Vec::new();
            let mut recorded = Vec::new();
            for id in &derived.step_order {
                let step = &derived.steps[id];
                if step.state != StepState::Started
                    || (step.started_boot == guard.boot_id && step.started_epoch == guard.epoch)
                {
                    continue;
                }
                let (observation, reconciled) = match step.effect {
                    EffectClass::LedgerOnly | EffectClass::Pure => {
                        (ReconcileObservation::NotApplicable, false)
                    }
                    EffectClass::ProcessLifecycle => (ReconcileObservation::ProcessNotOwned, false),
                    EffectClass::HostWrite => observe_host_write(step.intent.as_ref(), probe),
                };
                recorded.push((step.step_id.clone(), step.attempt, observation.clone()));
                events.push(if reconciled {
                    LedgerEvent::StepReconciled {
                        step_id: step.step_id.clone(),
                        attempt: step.attempt,
                        observation,
                    }
                } else {
                    LedgerEvent::StepInterrupted {
                        step_id: step.step_id.clone(),
                        attempt: step.attempt,
                        observation,
                    }
                });
            }
            if events.is_empty() {
                return Ok(Reconciliation {
                    recorded,
                    paused: derived.needs_resume,
                    seq: head.seq(),
                });
            }
            match self.commit_inner(task, head.seq(), &guard, events, vec![], Origin::Recover) {
                Ok(seq) => {
                    return Ok(Reconciliation {
                        recorded,
                        paused: true,
                        seq,
                    })
                }
                Err(LedgerError::Conflict) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(LedgerError::Conflict)
    }
}

pub(crate) fn observe_host_write(
    intent: Option<&StepIntent>,
    probe: &dyn WorktreeProbe,
) -> (ReconcileObservation, bool) {
    let unknown = |per_path, identity_ok| ReconcileObservation::Unknown {
        per_path,
        identity_ok,
    };
    let Some(intent) = intent else {
        return (unknown(BTreeMap::new(), false), false);
    };
    let Some(binding) = intent.worktree.as_ref() else {
        return (unknown(BTreeMap::new(), false), false);
    };
    let paths: Vec<String> = intent
        .pre_image
        .keys()
        .chain(intent.post_image.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    match probe.observe(binding, &paths) {
        Err(_) => (unknown(BTreeMap::new(), false), false),
        Ok(observed) => {
            let image = |img: &BTreeMap<String, Option<String>>| {
                paths
                    .iter()
                    .all(|p| observed.get(p).cloned().flatten() == img.get(p).cloned().flatten())
                    && paths.iter().all(|p| observed.contains_key(p))
            };
            if !paths.is_empty() && image(&intent.post_image) {
                (
                    ReconcileObservation::MatchesPostImage { hashes: observed },
                    true,
                )
            } else if !paths.is_empty() && image(&intent.pre_image) {
                (ReconcileObservation::MatchesPreImage, false)
            } else {
                (unknown(observed, true), false)
            }
        }
    }
}

/// True when `observation` is one [`observe_host_write`] can produce for
/// `intent` with a probe that answers exactly the requested paths. Checked on
/// every observation-bearing entry (StepFailed{WorktreeStateUnknown},
/// StepInterrupted/StepReconciled of a HostWrite step) at commit and load:
/// * MatchesPostImage carries EXACTLY the intent's post-image over every
///   intent path (a path only in the pre-image must be absent);
/// * MatchesPreImage needs a recorded pre-image that differs from the
///   post-image (it carries no hashes);
/// * Unknown after an identity/probe failure carries nothing; otherwise it
///   carries an observation of exactly the intent paths.
///
/// Whether the worktree really holds the observed bytes is a property of the
/// controller's read-only probe; the ledger cannot see the worktree.
fn observation_matches_intent(
    intent: Option<&StepIntent>,
    observation: &ReconcileObservation,
) -> bool {
    let unbound = |per_path: &BTreeMap<String, Option<String>>, identity_ok: bool| {
        !identity_ok && per_path.is_empty()
    };
    let Some(intent) = intent.filter(|i| i.worktree.is_some()) else {
        return matches!(observation, ReconcileObservation::Unknown { per_path, identity_ok }
            if unbound(per_path, *identity_ok));
    };
    let paths: BTreeSet<&String> = intent
        .pre_image
        .keys()
        .chain(intent.post_image.keys())
        .collect();
    let image = |img: &BTreeMap<String, Option<String>>| -> BTreeMap<String, Option<String>> {
        paths
            .iter()
            .map(|p| ((*p).clone(), img.get(*p).cloned().flatten()))
            .collect()
    };
    match observation {
        ReconcileObservation::MatchesPostImage { hashes } => {
            !paths.is_empty() && *hashes == image(&intent.post_image)
        }
        ReconcileObservation::MatchesPreImage => {
            !paths.is_empty() && image(&intent.pre_image) != image(&intent.post_image)
        }
        ReconcileObservation::Unknown {
            per_path,
            identity_ok,
        } => {
            unbound(per_path, *identity_ok)
                || (*identity_ok && per_path.keys().eq(paths.iter().copied()))
        }
        ReconcileObservation::NotApplicable | ReconcileObservation::ProcessNotOwned => false,
    }
}

// ---------------------------------------------------------------------------
// Replay / state machine (§3.4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepRecord {
    pub step_id: String,
    pub attempt: u32,
    pub state: StepState,
    pub effect: EffectClass,
    pub intent: Option<StepIntent>,
    pub started_boot: String,
    pub started_epoch: u64,
    pub started_seq: u64,
    pub observation: Option<ReconcileObservation>,
    pub resolution: Option<ReviewResolution>,
    pub result: Option<StepResultRef>,
    pub failure: Option<FailureReason>,
}
impl StepRecord {
    pub fn unresolved(&self) -> bool {
        matches!(
            self.state,
            StepState::Interrupted | StepState::CompletedByObservation
        ) && self.resolution.is_none()
    }
    /// Options the UI may offer for an interrupted/reconciled step (§3.5 table).
    pub fn resolution_options(&self) -> Vec<ReviewResolution> {
        use ReviewResolution::*;
        if !self.unresolved() {
            return vec![];
        }
        match (&self.effect, &self.observation) {
            (_, Some(ReconcileObservation::MatchesPostImage { .. })) => {
                vec![ConfirmObservation, Abandon]
            }
            (_, Some(ReconcileObservation::MatchesPreImage)) => vec![RetryAsNewAttempt, Abandon],
            (_, Some(ReconcileObservation::Unknown { .. })) => {
                let restorable = self
                    .intent
                    .as_ref()
                    .is_some_and(|i| i.worktree.is_some() && !i.pre_image.is_empty());
                let mut v = vec![];
                if restorable {
                    v.push(RestorePreImage);
                }
                v.extend([MarkManuallyResolved, Abandon]);
                v
            }
            _ => vec![RetryAsNewAttempt, Abandon],
        }
    }
}

/// Deterministic replay of a verified journal. Never contains narrative text.
#[derive(Debug, Clone, Default)]
pub struct Derived {
    pub steps: BTreeMap<String, StepRecord>,
    pub step_order: Vec<String>,
    pub plan: Option<Plan>,
    pub checkpoint: Option<Checkpoint>,
    pub gates: Vec<GateRecord>,
    pub http_checks: Vec<HttpCheckRecord>,
    pub preview: PreviewLedgerState,
    pub closed: Option<TaskStatus>,
    pub edits_applied: u32,
    pub edits_denied: u32,
    pub oracle_denied: u32,
    pub copy_out_paths: BTreeSet<String>,
    pub needs_resume: bool,
    pub last_admission: Option<AdmissionTrace>,
    pub last_repair: Option<RepairStop>,
    pub last_repair_seq: u64,
    pub last_change_seq: u64,
    pub repair_attempts_used: u8,
    pub worktree: Option<WorktreeBinding>,
    pub pre_image_mismatch: bool,
    pub narratives: u32,
    /// Manifest of the last EditApplied not yet consumed by its checkpoint.
    pending_edit: Option<String>,
    /// Union of the paths of those EditApplied events.
    pending_edit_paths: BTreeSet<String>,
    pending_reviews: Vec<PendingReview>,
}
/// A FileReviewed event awaiting its checkpoint (same commit).
#[derive(Debug, Clone)]
struct PendingReview {
    path: String,
    decision: FileDecision,
    reviewed_new_sha256: Option<String>,
    ui_event_id: String,
}
impl Derived {
    pub fn working_set(&self) -> Option<&WorkingSetManifest> {
        self.checkpoint.as_ref().map(|c| &c.working_set)
    }
    pub fn review(&self) -> ReviewState {
        self.checkpoint
            .as_ref()
            .map(|c| c.review.clone())
            .unwrap_or_default()
    }
    pub fn unresolved_steps(&self) -> Vec<&StepRecord> {
        self.step_order
            .iter()
            .map(|id| &self.steps[id])
            .filter(|s| s.unresolved())
            .collect()
    }
    pub fn running_steps(&self) -> Vec<&StepRecord> {
        self.step_order
            .iter()
            .map(|id| &self.steps[id])
            .filter(|s| s.state == StepState::Started && s.effect != EffectClass::ProcessLifecycle)
            .collect()
    }
    pub fn paused(&self) -> bool {
        self.needs_resume
    }
    /// Task-level status derived ONLY from ledger facts.
    pub fn status(&self) -> TaskStatus {
        if let Some(closed) = &self.closed {
            return closed.clone();
        }
        if self.needs_resume {
            return TaskStatus::Paused {
                reason: PauseReason::ReviewRequired,
            };
        }
        if !self.running_steps().is_empty() {
            return TaskStatus::Running;
        }
        if matches!(self.apply_status(), ApplyStatus::Applied { .. }) {
            return TaskStatus::Applied;
        }
        if let Some(stop) = self.last_repair {
            if self.last_repair_seq >= self.last_change_seq {
                let reason = match stop {
                    RepairStop::Passed => return TaskStatus::AwaitingReview,
                    RepairStop::BudgetExhausted => StopReason::BudgetExhausted,
                    RepairStop::NoProgress => StopReason::NoProgress,
                    RepairStop::Infrastructure => StopReason::Infrastructure,
                    RepairStop::OracleDenied => StopReason::OracleDenied,
                    RepairStop::Cancelled => return TaskStatus::Open,
                };
                return TaskStatus::Stopped { reason };
            }
        }
        match self.working_set() {
            Some(ws) if ws.base != ws.current => TaskStatus::AwaitingReview,
            _ => TaskStatus::Open,
        }
    }
    /// Apply state from HostWrite step facts (step-id prefixes are the
    /// controller convention: `apply-`, `revert-applied-`).
    pub fn apply_status(&self) -> ApplyStatus {
        let mut status = ApplyStatus::NotApplied;
        for id in &self.step_order {
            let s = &self.steps[id];
            let apply = id.starts_with("apply-");
            let revert = id.starts_with("revert-applied-");
            if !(apply || revert) {
                continue;
            }
            match (&s.state, &s.result) {
                (
                    StepState::Completed,
                    Some(StepResultRef::Applied {
                        files, worktree, ..
                    }),
                ) => {
                    status = ApplyStatus::Applied {
                        files: files.clone(),
                        worktree: worktree.path.clone(),
                    }
                }
                (StepState::Completed, Some(StepResultRef::Reverted { .. })) => {
                    status = ApplyStatus::Reverted
                }
                (StepState::CompletedByObservation, _)
                    if s.resolution == Some(ReviewResolution::ConfirmObservation) =>
                {
                    let intent = s.intent.as_ref();
                    status = if apply {
                        ApplyStatus::Applied {
                            files: intent
                                .map(|i| i.post_image.keys().cloned().collect())
                                .unwrap_or_default(),
                            worktree: intent
                                .and_then(|i| i.worktree.as_ref().map(|w| w.path.clone()))
                                .unwrap_or_default(),
                        }
                    } else {
                        ApplyStatus::Reverted
                    }
                }
                (StepState::Started, _) | (StepState::Interrupted, _) => {
                    status = ApplyStatus::Interrupted
                }
                (StepState::CompletedByObservation, _) if s.resolution.is_none() => {
                    status = ApplyStatus::Interrupted
                }
                _ => {}
            }
        }
        status
    }
}

/// The commit an entry belongs to. `exact` = the boundary is known (always at
/// commit time; at load from [`TaskHead::commit_starts`]); otherwise (entries
/// of a head written before boundaries were persisted) "same commit" rules
/// degrade to adjacency.
#[derive(Default)]
struct CommitScope {
    start_index: usize,
    exact: bool,
}
impl CommitScope {
    /// True when the entry right before the one being applied (`previous`
    /// = all earlier entries) belongs to the same commit.
    fn previous_in_same_commit(&self, previous: &[JournalEntry]) -> bool {
        !self.exact || previous.len() > self.start_index
    }
}

/// An edit or review event still waits for its checkpoint.
fn pending_open(d: &Derived) -> bool {
    d.pending_edit.is_some() || !d.pending_reviews.is_empty()
}

fn replay(head: &TaskHead) -> Result<Derived> {
    let mut derived = Derived::default();
    let available: BTreeSet<(BlobKind, String)> = head
        .blobs
        .iter()
        .map(|b| (b.kind, b.sha256.clone()))
        .collect();
    let starts = &head.commit_starts;
    if starts.first() == Some(&0)
        || starts.windows(2).any(|w| w[0] >= w[1])
        || starts
            .last()
            .is_some_and(|s| *s > head.journal.len() as u64)
    {
        return Err(LedgerError::Corrupt);
    }
    let mut next_start = starts.iter().peekable();
    let mut scope = CommitScope::default();
    let ctx = HeadCtx {
        task_id: &head.task_id,
        spec_sha256: &head.spec_sha256,
        base_manifest: &head.base_manifest,
    };
    for (i, entry) in head.journal.iter().enumerate() {
        if next_start.next_if(|s| **s == i as u64 + 1).is_some() {
            // Commit boundary: the previous commit must be complete (every
            // edit/review consumed by its checkpoint), exactly as at commit.
            if pending_open(&derived) {
                return Err(LedgerError::Corrupt);
            }
            scope = CommitScope {
                start_index: i,
                exact: true,
            };
        } else if scope.exact {
            // One commit = one guard.
            let first = &head.journal[scope.start_index];
            if entry.boot_id != first.boot_id || entry.epoch != first.epoch {
                return Err(LedgerError::Corrupt);
            }
        }
        apply_entry(
            &ctx,
            &head.journal[..i],
            &mut derived,
            entry,
            &available,
            &scope,
            Origin::Recover,
            false,
        )?;
    }
    if pending_open(&derived) {
        return Err(LedgerError::Corrupt);
    }
    Ok(derived)
}

/// Append and validate new entries (sets seq/prev/boot/epoch from the guard).
fn append_entries(
    head: &mut TaskHead,
    derived: &mut Derived,
    epoch: &EpochGuard,
    events: Vec<LedgerEvent>,
    new_blobs: &[BlobRef],
    origin: Origin,
) -> Result<()> {
    let mut available: BTreeSet<(BlobKind, String)> = head
        .blobs
        .iter()
        .map(|b| (b.kind, b.sha256.clone()))
        .collect();
    available.extend(new_blobs.iter().map(|b| (b.kind, b.sha256.clone())));
    let scope = CommitScope {
        start_index: head.journal.len(),
        exact: true,
    };
    if !events.is_empty() {
        // Persist the boundary: load-time replay re-derives this scope.
        head.commit_starts.push(head.seq() + 1);
    }
    let now = now_ms();
    for mut event in events {
        // Checkpoint bookkeeping fields are derived from the journal itself,
        // never supplied by a caller (and re-validated at every replay).
        if let LedgerEvent::Checkpoint { checkpoint } = &mut event {
            checkpoint.seq = head.seq() + 1;
            checkpoint.last_gate = derived.gates.last().map(|g| g.gate_run_id.clone());
            checkpoint.repair_attempts_used = derived.repair_attempts_used;
            checkpoint.preview = derived.preview.clone();
        }
        let prev = match head.journal.last() {
            Some(e) => sha_json(e)?,
            None => genesis(&head.task_id),
        };
        let entry = JournalEntry {
            seq: head.seq() + 1,
            prev_sha256: prev,
            at_ms: now,
            boot_id: epoch.boot_id.clone(),
            epoch: epoch.epoch,
            event,
        };
        let ctx = HeadCtx {
            task_id: &head.task_id,
            spec_sha256: &head.spec_sha256,
            base_manifest: &head.base_manifest,
        };
        apply_entry(
            &ctx,
            &head.journal,
            derived,
            &entry,
            &available,
            &scope,
            origin,
            true,
        )?;
        head.journal.push(entry);
    }
    if pending_open(derived) {
        return Err(LedgerError::IllegalTransition(
            "edit/review without matching checkpoint in the same commit",
        ));
    }
    Ok(())
}

fn ill(reason: &'static str) -> LedgerError {
    LedgerError::IllegalTransition(reason)
}

/// Immutable per-task context needed by the replay rules.
struct HeadCtx<'a> {
    task_id: &'a TaskId,
    spec_sha256: &'a str,
    base_manifest: &'a BTreeMap<String, String>,
}

#[allow(clippy::too_many_arguments)]
fn apply_entry(
    head: &HeadCtx<'_>,
    previous: &[JournalEntry],
    d: &mut Derived,
    entry: &JournalEntry,
    available: &BTreeSet<(BlobKind, String)>,
    scope: &CommitScope,
    origin: Origin,
    committing: bool,
) -> Result<()> {
    // Chain and identity
    let expected_prev = match previous.last() {
        Some(e) => sha_json(e)?,
        None => genesis(head.task_id),
    };
    if entry.seq != previous.len() as u64 + 1
        || entry.prev_sha256 != expected_prev
        || entry.boot_id.len() != 32
        || !is_lower_hex(&entry.boot_id)
    {
        return Err(LedgerError::Corrupt);
    }
    let blob_ok = |b: &BlobRef, kind: BlobKind| {
        b.kind == kind && is_sha256(&b.sha256) && available.contains(&(b.kind, b.sha256.clone()))
    };
    if entry.seq == 1 && !matches!(entry.event, LedgerEvent::TaskOpened { .. }) {
        return Err(ill("first entry must be TaskOpened"));
    }
    // An edit or review is followed only by edit/review events and then its
    // checkpoint (same commit; at load also when boundaries are unknown).
    if pending_open(d)
        && !matches!(
            entry.event,
            LedgerEvent::EditApplied { .. }
                | LedgerEvent::EditDenied { .. }
                | LedgerEvent::FileReviewed { .. }
                | LedgerEvent::Checkpoint { .. }
        )
    {
        return Err(ill("edit/review must be followed by its checkpoint"));
    }
    match &entry.event {
        LedgerEvent::TaskOpened {
            spec_sha256,
            base_manifest_sha256,
            snapshot_sha256,
        } => {
            if entry.seq != 1
                || spec_sha256 != head.spec_sha256
                || *base_manifest_sha256 != manifest_sha256(head.base_manifest)
                || !is_sha256(snapshot_sha256)
            {
                return Err(ill("TaskOpened"));
            }
        }
        LedgerEvent::PlanRecorded { plan } => {
            let expected = d.plan.as_ref().map_or(1, |p| p.revision + 1);
            if plan.revision != expected
                || plan.steps.len() > 64
                || (plan.author == PlanAuthor::Model && plan.confirmed)
                || plan
                    .steps
                    .iter()
                    .any(|s| !valid_id(&s.step_id) || s.summary.len() > 512)
                || plan
                    .steps
                    .iter()
                    .map(|s| &s.step_id)
                    .collect::<BTreeSet<_>>()
                    .len()
                    != plan.steps.len()
            {
                return Err(ill("PlanRecorded"));
            }
            d.plan = Some(plan.clone());
        }
        LedgerEvent::PlanConfirmed {
            revision,
            ui_event_id,
        } => {
            let plan = d.plan.as_mut().ok_or(ill("no plan"))?;
            if plan.revision != *revision || plan.confirmed || !is_event_id(ui_event_id) {
                return Err(ill("PlanConfirmed"));
            }
            plan.confirmed = true;
        }
        LedgerEvent::StepStarted {
            step_id,
            attempt,
            intent,
        } => {
            if d.closed.is_some() {
                return Err(ill("task closed"));
            }
            if d.needs_resume {
                // Sole exception: "Restore pre-image" (UI click) restores EXACTLY the
                // recorded pre-image of the resolved step, in the same commit.
                let restore = intent.effect == EffectClass::HostWrite
                    && previous.last().is_some_and(|p| {
                        p.boot_id == entry.boot_id
                            && p.epoch == entry.epoch
                            && matches!(&p.event, LedgerEvent::ReconcileResolved {
                                step_id: target,
                                resolution: ReviewResolution::RestorePreImage,
                                ..
                            } if d.steps.get(target).and_then(|s| s.intent.as_ref()).is_some_and(|t| {
                                t.worktree.is_some()
                                    && t.worktree == intent.worktree
                                    && t.pre_image == intent.post_image
                            }))
                    });
                if !(restore && scope.previous_in_same_commit(previous)) {
                    return Err(ill("task paused: review required"));
                }
            }
            if !valid_id(step_id)
                || *attempt == 0
                || !valid_id(&intent.idempotency_key)
                || !is_sha256(&intent.working_set_sha256)
                || intent.pre_image.len() > 64
                || intent.post_image.len() > 64
                || intent
                    .pre_image
                    .iter()
                    .chain(intent.post_image.iter())
                    .any(|(p, h)| !valid_rel_path(p) || h.as_deref().is_some_and(|h| !is_sha256(h)))
            {
                return Err(ill("StepStarted fields"));
            }
            if let Some(plan) = &d.plan {
                if let Some(planned) = plan.steps.iter().find(|s| &s.step_id == step_id) {
                    if plan.author == PlanAuthor::Model && !plan.confirmed {
                        return Err(ill("model plan step not confirmed"));
                    }
                    if planned.effect != intent.effect {
                        return Err(ill("effect differs from plan"));
                    }
                }
            }
            match d.steps.get(step_id) {
                None if *attempt == 1 => {}
                Some(prev)
                    if matches!(prev.state, StepState::Failed | StepState::Planned)
                        && prev.attempt.checked_add(1) == Some(*attempt) => {}
                _ => return Err(ill("StepStarted from illegal state")),
            }
            if matches!(
                intent.effect,
                EffectClass::Pure | EffectClass::ProcessLifecycle
            ) {
                let admitted = previous.last().is_some_and(|p| {
                    p.boot_id == entry.boot_id
                        && p.epoch == entry.epoch
                        && matches!(&p.event, LedgerEvent::AdmissionChecked { trace }
                            if trace.admitted && trace.purpose != AdmissionPurpose::Resume)
                });
                if !admitted || !scope.previous_in_same_commit(previous) {
                    return Err(ill("admission trace must precede start in the same commit"));
                }
            }
            if intent.effect == EffectClass::HostWrite
                && intent.worktree.is_none()
                && (!intent.pre_image.is_empty() || !intent.post_image.is_empty())
            {
                return Err(ill("host write without worktree binding"));
            }
            if !d.steps.contains_key(step_id) {
                d.step_order.push(step_id.clone());
            }
            d.steps.insert(
                step_id.clone(),
                StepRecord {
                    step_id: step_id.clone(),
                    attempt: *attempt,
                    state: StepState::Started,
                    effect: intent.effect,
                    intent: Some(intent.clone()),
                    started_boot: entry.boot_id.clone(),
                    started_epoch: entry.epoch,
                    started_seq: entry.seq,
                    observation: None,
                    resolution: None,
                    result: None,
                    failure: None,
                },
            );
        }
        LedgerEvent::StepCompleted {
            step_id,
            attempt,
            result,
        } => {
            let step = started_same_boot(d, step_id, *attempt, entry)?;
            validate_result(result, available)?;
            step.state = StepState::Completed;
            step.result = Some(result.clone());
            match result {
                StepResultRef::RepairLoop { stop, .. } => {
                    d.last_repair = Some(*stop);
                    d.last_repair_seq = entry.seq;
                }
                StepResultRef::RepairAttempt { .. } => {
                    d.repair_attempts_used = d.repair_attempts_used.saturating_add(1);
                }
                StepResultRef::WorktreeCreated { binding } => d.worktree = Some(binding.clone()),
                _ => {}
            }
        }
        LedgerEvent::StepFailed {
            step_id,
            attempt,
            reason,
            evidence,
        } => {
            if evidence.len() > 8
                || evidence
                    .iter()
                    .any(|b| !available.contains(&(b.kind, b.sha256.clone())))
            {
                return Err(ill("StepFailed evidence"));
            }
            if matches!(reason, FailureReason::PreImageMismatch { .. }) {
                d.pre_image_mismatch = true;
            }
            let unknown = matches!(reason, FailureReason::WorktreeStateUnknown { .. });
            let step = started_same_boot(d, step_id, *attempt, entry)?;
            step.state = StepState::Failed;
            step.failure = Some(reason.clone());
            if let FailureReason::WorktreeStateUnknown { error, observation } = reason {
                // In-process reconciliation by observation: same consistency rule
                // as recover() for HostWrite steps; the task pauses for the UI.
                if step.effect != EffectClass::HostWrite
                    || step
                        .intent
                        .as_ref()
                        .and_then(|i| i.worktree.as_ref())
                        .is_none()
                    || error.is_empty()
                    || error.len() > 64
                {
                    return Err(ill("worktree state unknown requires a HostWrite step"));
                }
                step.state = match observation {
                    ReconcileObservation::MatchesPostImage { .. } => {
                        StepState::CompletedByObservation
                    }
                    ReconcileObservation::MatchesPreImage
                    | ReconcileObservation::Unknown { .. } => StepState::Interrupted,
                    _ => return Err(ill("observation inconsistent with effect class")),
                };
                // The observation must be one a read-only probe of THIS intent
                // can yield (MatchesPostImage = exactly the post-image).
                if !observation_matches_intent(step.intent.as_ref(), observation) {
                    return Err(ill("observation does not match the intent images"));
                }
                step.observation = Some(observation.clone());
            }
            if unknown {
                d.needs_resume = true;
            }
        }
        LedgerEvent::StepInterrupted {
            step_id,
            attempt,
            observation,
        }
        | LedgerEvent::StepReconciled {
            step_id,
            attempt,
            observation,
        } => {
            if committing && origin != Origin::Recover {
                return Err(ill("interruption is recorded only by recover()"));
            }
            let reconciled = matches!(entry.event, LedgerEvent::StepReconciled { .. });
            let step = d.steps.get_mut(step_id).ok_or(ill("unknown step"))?;
            if step.state != StepState::Started
                || step.attempt != *attempt
                || (step.started_boot == entry.boot_id && step.started_epoch == entry.epoch)
            {
                return Err(ill("interruption requires an older boot or epoch"));
            }
            let consistent = match (step.effect, observation) {
                (
                    EffectClass::LedgerOnly | EffectClass::Pure,
                    ReconcileObservation::NotApplicable,
                ) => !reconciled,
                (EffectClass::ProcessLifecycle, ReconcileObservation::ProcessNotOwned) => {
                    !reconciled
                }
                (EffectClass::HostWrite, ReconcileObservation::MatchesPostImage { .. }) => {
                    reconciled
                }
                (
                    EffectClass::HostWrite,
                    ReconcileObservation::MatchesPreImage | ReconcileObservation::Unknown { .. },
                ) => !reconciled,
                _ => false,
            };
            if !consistent {
                return Err(ill("observation inconsistent with effect class"));
            }
            if step.effect == EffectClass::HostWrite
                && !observation_matches_intent(step.intent.as_ref(), observation)
            {
                return Err(ill("observation does not match the intent images"));
            }
            step.state = if reconciled {
                StepState::CompletedByObservation
            } else {
                StepState::Interrupted
            };
            step.observation = Some(observation.clone());
            d.needs_resume = true;
            if step.effect == EffectClass::ProcessLifecycle {
                if let PreviewLedgerState::Running { descriptor } = &d.preview {
                    d.preview = PreviewLedgerState::NotOwnedAfterRestart {
                        service_id: descriptor.service_id.clone(),
                    };
                }
            }
        }
        LedgerEvent::ReconcileResolved {
            step_id,
            resolution,
            ui_event_id,
        } => {
            if !is_event_id(ui_event_id) {
                return Err(ill("ui event id"));
            }
            let step = d.steps.get_mut(step_id).ok_or(ill("unknown step"))?;
            if !step.resolution_options().contains(resolution) {
                return Err(ill("resolution not offered for this observation"));
            }
            step.resolution = Some(*resolution);
            step.state = match resolution {
                ReviewResolution::ConfirmObservation => StepState::CompletedByObservation,
                ReviewResolution::RetryAsNewAttempt => StepState::Planned,
                ReviewResolution::Abandon
                | ReviewResolution::MarkManuallyResolved
                | ReviewResolution::RestorePreImage => StepState::Abandoned,
            };
        }
        LedgerEvent::EditApplied {
            origin: edit_origin,
            manifest_sha256: sha,
            paths,
        } => {
            if paths.is_empty()
                || paths.len() > MAX_WORKING_SET_FILES
                || paths.iter().any(|p| !valid_rel_path(p))
                || !is_sha256(sha)
            {
                return Err(ill("EditApplied"));
            }
            validate_origin(edit_origin)?;
            d.edits_applied = d.edits_applied.saturating_add(paths.len() as u32);
            if matches!(edit_origin, EditOrigin::SandboxCopyOut { .. }) {
                d.copy_out_paths.extend(paths.iter().cloned());
            }
            d.pending_edit = Some(sha.clone());
            d.pending_edit_paths.extend(paths.iter().cloned());
            d.last_change_seq = entry.seq;
        }
        LedgerEvent::EditDenied {
            origin: edit_origin,
            path,
            reason,
        } => {
            if path.len() > 256 || path.contains('\0') {
                return Err(ill("EditDenied path"));
            }
            validate_origin(edit_origin)?;
            d.edits_denied = d.edits_denied.saturating_add(1);
            if *reason == EditDenial::OracleProtected {
                d.oracle_denied = d.oracle_denied.saturating_add(1);
            }
        }
        LedgerEvent::GateRecorded { record } => {
            if !valid_id(&record.gate_run_id)
                || d.gates.iter().any(|g| g.gate_run_id == record.gate_run_id)
                || !is_sha256(&record.working_set_sha256)
                || !is_sha256(&record.plan_sha256)
                || !is_sha256(&record.workspace_profile_sha256)
                || record.commands.len() > 8
                || record.logs.len() > 4
                || record.logs.iter().any(|b| !blob_ok(b, BlobKind::GateLog))
                || record.commands.iter().any(|c| {
                    c.excerpt.len() > 4096 || !is_sha256(&c.log_sha256) || c.argv.len() > 64
                })
            {
                return Err(ill("GateRecorded"));
            }
            d.gates.push(record.clone());
            d.last_change_seq = entry.seq;
        }
        LedgerEvent::HttpChecksRecorded { record } => {
            if record.results.len() > 16 || !is_sha256(&record.tree_sha256) {
                return Err(ill("HttpChecksRecorded"));
            }
            d.http_checks.push(record.clone());
        }
        LedgerEvent::PreviewStarted { descriptor } => {
            if descriptor.task_id != head.task_id.as_str() || !is_sha256(&descriptor.tree_sha256) {
                return Err(ill("PreviewStarted"));
            }
            d.preview = match descriptor.ready {
                ReadyState::Ready { .. } => PreviewLedgerState::Running {
                    descriptor: descriptor.clone(),
                },
                ReadyState::StartupFailed { .. } => PreviewLedgerState::StartupFailed {
                    descriptor: descriptor.clone(),
                },
            };
        }
        LedgerEvent::PreviewStopped { report } => {
            d.preview = PreviewLedgerState::Stopped {
                record: report.clone(),
            };
        }
        LedgerEvent::FileReviewed {
            path,
            decision,
            reviewed_new_sha256,
            ui_event_id,
        } => {
            if !valid_rel_path(path)
                || !is_event_id(ui_event_id)
                || reviewed_new_sha256
                    .as_deref()
                    .is_some_and(|h| !is_sha256(h))
            {
                return Err(ill("FileReviewed"));
            }
            d.pending_reviews.push(PendingReview {
                path: path.clone(),
                decision: decision.clone(),
                reviewed_new_sha256: reviewed_new_sha256.clone(),
                ui_event_id: ui_event_id.clone(),
            });
        }
        LedgerEvent::AdmissionChecked { trace } => {
            if trace.admitted && trace.epoch != entry.epoch {
                return Err(ill("admission epoch"));
            }
            if trace.purpose == AdmissionPurpose::Resume && trace.admitted {
                if !d.unresolved_steps().is_empty() {
                    return Err(ill("resume with unresolved interrupted steps"));
                }
                d.needs_resume = false;
            } else if trace.admitted && d.needs_resume {
                return Err(ill("admission while paused"));
            }
            d.last_admission = Some(trace.clone());
        }
        LedgerEvent::Narrative {
            role,
            text_sha256,
            blob,
        } => {
            if role.is_empty()
                || role.len() > 64
                || !role
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !blob_ok(blob, BlobKind::Narrative)
                || blob.sha256 != *text_sha256
            {
                return Err(ill("Narrative"));
            }
            d.narratives = d.narratives.saturating_add(1);
        }
        LedgerEvent::Checkpoint { checkpoint } => {
            validate_checkpoint(head, d, entry, checkpoint, available)?;
            d.checkpoint = Some(checkpoint.clone());
        }
        LedgerEvent::TaskClosed { status } => {
            if d.closed.is_some()
                || !matches!(
                    status,
                    TaskStatus::Rejected | TaskStatus::Cancelled | TaskStatus::Closed
                )
            {
                return Err(ill("TaskClosed"));
            }
            d.closed = Some(status.clone());
        }
    }
    Ok(())
}

fn started_same_boot<'a>(
    d: &'a mut Derived,
    step_id: &str,
    attempt: u32,
    entry: &JournalEntry,
) -> Result<&'a mut StepRecord> {
    let step = d.steps.get_mut(step_id).ok_or(ill("unknown step"))?;
    if step.state != StepState::Started || step.attempt != attempt {
        return Err(ill("completion requires a Started step"));
    }
    if step.started_boot != entry.boot_id || step.started_epoch != entry.epoch {
        return Err(ill("completion across boot or epoch"));
    }
    Ok(step)
}

fn validate_origin(origin: &EditOrigin) -> Result<()> {
    match origin {
        EditOrigin::SandboxCopyOut { gate } if !valid_id(gate) => Err(ill("edit origin")),
        _ => Ok(()),
    }
}

fn validate_result(result: &StepResultRef, available: &BTreeSet<(BlobKind, String)>) -> Result<()> {
    let ok = match result {
        StepResultRef::PreviewStopped { logs } => {
            logs.len() <= 4
                && logs
                    .iter()
                    .all(|b| available.contains(&(b.kind, b.sha256.clone())))
        }
        StepResultRef::RepairLoop {
            attempts,
            last_gates,
            ..
        } => attempts.len() <= 8 && last_gates.len() <= 3,
        StepResultRef::Applied {
            files, post_image, ..
        } => files.len() <= MAX_WORKING_SET_FILES && post_image.len() <= 64,
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(ill("step result bounds"))
    }
}

fn validate_checkpoint(
    head: &HeadCtx<'_>,
    d: &mut Derived,
    entry: &JournalEntry,
    cp: &Checkpoint,
    available: &BTreeSet<(BlobKind, String)>,
) -> Result<()> {
    let ws = &cp.working_set;
    if cp.seq != entry.seq
        || &ws.base != head.base_manifest
        || ws.base_sha256 != manifest_sha256(&ws.base)
        || ws.current_sha256 != manifest_sha256(&ws.current)
        || ws.current.len() > MAX_WORKING_SET_FILES
        || ws.current.keys().any(|p| !valid_rel_path(p))
        || ws
            .current
            .values()
            .any(|h| !available.contains(&(BlobKind::FileContent, h.clone())))
    {
        return Err(ill("checkpoint working set"));
    }
    if cp.last_gate != d.gates.last().map(|g| g.gate_run_id.clone())
        || cp.repair_attempts_used != d.repair_attempts_used
        || cp.preview != d.preview
    {
        return Err(ill("checkpoint disagrees with journal"));
    }
    // Content changes only together with their content-changing event in the
    // same commit (the journal adjacency rule keeps them adjacent at load).
    // The only working-set writer is EditApplied: model/user edits, sandbox
    // copy-out and the user's revert-to-base. A worktree "restore pre-image"
    // is a HostWrite step and never changes the working set. Reference: the
    // previous checkpoint (the base before the first one).
    let before: BTreeMap<String, String> = d
        .checkpoint
        .as_ref()
        .map_or(head.base_manifest, |c| &c.working_set.current)
        .clone();
    let content_changed: BTreeSet<String> = before
        .keys()
        .chain(ws.current.keys())
        .filter(|p| before.get(*p) != ws.current.get(*p))
        .cloned()
        .collect();
    let edited = std::mem::take(&mut d.pending_edit_paths);
    match d.pending_edit.take() {
        Some(pending) if pending != ws.current_sha256 => {
            return Err(ill("edit manifest differs from checkpoint"));
        }
        None if !content_changed.is_empty() => {
            return Err(ill(
                "content change without an edit event in the same commit",
            ));
        }
        _ => {}
    }
    if !content_changed.is_subset(&edited) {
        return Err(ill("content change not covered by an edit event"));
    }
    let previous = d.review();
    let mut reviewed: BTreeMap<String, PendingReview> = BTreeMap::new();
    for r in d.pending_reviews.drain(..) {
        // Every FileReviewed binds exactly the checkpoint entry it decided:
        // decision, reviewed new hash and UI event id.
        match cp.review.files.get(&r.path) {
            Some(e)
                if e.decision == r.decision
                    && e.reviewed_new_sha256 == r.reviewed_new_sha256
                    && e.ui_event_id == r.ui_event_id => {}
            _ => return Err(ill("review event differs from checkpoint")),
        }
        reviewed.insert(r.path.clone(), r);
    }
    for (path, review) in &cp.review.files {
        if !valid_rel_path(path) || !is_event_id_or_empty(&review.ui_event_id) {
            return Err(ill("checkpoint review"));
        }
        let prior = previous.files.get(path);
        let changed = prior.map(|r| &r.decision) != Some(&review.decision);
        let decided = !matches!(review.decision, FileDecision::Pending);
        let ui = reviewed.get(path);
        // Never auto-accept: any change TO a decided state needs a UI review event.
        if changed && decided && ui.is_none() {
            return Err(ill("decision changed without a UI review event"));
        }
        // Without a UI review event a decided entry is carried over unchanged
        // (decision AND binding); the only other change is a reset to Pending.
        if decided && ui.is_none() && prior != Some(review) {
            return Err(ill(
                "decided review entry re-bound without a UI review event",
            ));
        }
        // A decision on a file whose content changed in this checkpoint resets to
        // Pending, unless a FileReviewed for EXACTLY the new hash is in this
        // commit, or it is a Rejected decision the controller keeps
        // (`reset_reviews`/revert): the user's rejection of the content shown
        // before the change, content back at base, or exactly the rejected bytes.
        if decided && content_changed.contains(path) {
            let current = ws.current.get(path);
            let fresh = ui.is_some_and(|r| r.reviewed_new_sha256.as_ref() == current);
            let kept_rejection = review.decision == FileDecision::Rejected
                && (ui.is_some_and(|r| r.reviewed_new_sha256.as_ref() == before.get(path))
                    || current == ws.base.get(path)
                    || review.reviewed_new_sha256.as_ref() == current);
            if !(fresh || kept_rejection) {
                return Err(ill("decision on changed content must reset to Pending"));
            }
        }
        // An accepting decision always binds the CURRENT content hash.
        if matches!(
            review.decision,
            FileDecision::Accepted | FileDecision::PartiallyAccepted { .. }
        ) && (review.reviewed_new_sha256 != ws.current.get(path).cloned()
            || review.reviewed_base_sha256 != ws.base.get(path).cloned())
        {
            return Err(ill("accepted decision does not bind current content"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Spec validation
// ---------------------------------------------------------------------------

pub fn validate_spec(spec: &TaskSpec) -> Result<()> {
    let inv = LedgerError::Invalid;
    if spec.objective.len() > MAX_OBJECTIVE_BYTES {
        return Err(inv("objective too long"));
    }
    if spec.selected_files.is_empty()
        || spec.selected_files.len() > MAX_SELECTED_FILES
        || spec.selected_files.iter().any(|p| !valid_rel_path(p))
    {
        return Err(inv("selected files"));
    }
    if !spec.selected_files.contains(&spec.primary) {
        return Err(inv("primary must be selected"));
    }
    if !spec.oracle_files.is_subset(&spec.selected_files)
        || spec.oracle_files.contains(&spec.primary)
    {
        return Err(inv("oracle files"));
    }
    if spec.repository.display_root.len() > 4096
        || spec.repository.source_id.len() > 128
        || spec
            .repository
            .branch
            .as_ref()
            .is_some_and(|b| b.len() > 256)
        || spec
            .repository
            .head_commit
            .as_ref()
            .is_some_and(|b| b.len() > 128)
    {
        return Err(inv("repository label"));
    }
    let plan = &spec.gate_plan;
    let command_ids: BTreeSet<&str> = plan.commands.iter().map(GateCommand::id).collect();
    if plan.schema != GATE_PLAN_SCHEMA
        || plan.commands.is_empty()
        || plan.commands.len() > 8
        || command_ids.len() != plan.commands.len()
        || command_ids.iter().any(|id| !valid_id(id))
    {
        return Err(inv("gate plan"));
    }
    if spec.repair.max_attempts > 5 || spec.repair.max_total_gate_ms > 15 * 60 * 1000 {
        return Err(inv("repair budget"));
    }
    let check_ids: BTreeSet<&str> = spec
        .preview
        .as_ref()
        .map(|p| p.http_checks.iter().map(|c| c.id.as_str()).collect())
        .unwrap_or_default();
    if spec
        .preview
        .as_ref()
        .is_some_and(|p| p.http_checks.len() > 16)
    {
        return Err(inv("http checks"));
    }
    if spec.acceptance.len() > 16
        || spec
            .acceptance
            .iter()
            .map(|c| &c.id)
            .collect::<BTreeSet<_>>()
            .len()
            != spec.acceptance.len()
    {
        return Err(inv("acceptance"));
    }
    for c in &spec.acceptance {
        if !valid_id(&c.id) || c.text.len() > 1024 {
            return Err(inv("criterion"));
        }
        match &c.check {
            CriterionCheck::GateCommand { command_id, .. }
                if !command_ids.contains(command_id.as_str()) =>
            {
                return Err(inv("criterion references unknown command"))
            }
            CriterionCheck::Http { check_id } if !check_ids.contains(check_id.as_str()) => {
                return Err(inv("criterion references unknown http check"))
            }
            _ => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Outcome model (§3.6) — distinct fields, never from model output
// ---------------------------------------------------------------------------

/// Trusted controller facts that are not in the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutcomeEnvironment {
    pub execution_available: bool,
    pub source_changed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolCounters {
    pub applied: u32,
    pub denied: u32,
    pub oracle_denied: u32,
}

/// The ONLY input of [`assess`]. Narrative events are not reachable from it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskHeadView {
    pub spec: TaskSpec,
    pub oracle: OracleDerivation,
    pub working_set: WorkingSetManifest,
    pub review: ReviewState,
    pub gates: Vec<GateRecord>,
    pub http_checks: Vec<HttpCheckRecord>,
    pub preview: PreviewLedgerState,
    pub apply: ApplyStatus,
    pub tool: ToolCounters,
    pub copy_out_paths: BTreeSet<String>,
    pub interrupted_steps: u32,
    pub unresolved_steps: u32,
    pub pre_image_mismatch: bool,
    pub closed: Option<TaskStatus>,
    pub environment: OutcomeEnvironment,
}
impl TaskHeadView {
    pub fn build(head: &TaskHead, d: &Derived, environment: OutcomeEnvironment) -> Self {
        let working_set = d
            .working_set()
            .cloned()
            .unwrap_or_else(|| WorkingSetManifest {
                base: head.base_manifest.clone(),
                current: head.base_manifest.clone(),
                base_sha256: manifest_sha256(&head.base_manifest),
                current_sha256: manifest_sha256(&head.base_manifest),
            });
        Self {
            spec: head.spec.clone(),
            oracle: head.oracle.clone(),
            working_set,
            review: d.review(),
            gates: d.gates.clone(),
            http_checks: d.http_checks.clone(),
            preview: d.preview.clone(),
            apply: d.apply_status(),
            tool: ToolCounters {
                applied: d.edits_applied,
                denied: d.edits_denied,
                oracle_denied: d.oracle_denied,
            },
            copy_out_paths: d.copy_out_paths.clone(),
            interrupted_steps: d.steps.values().filter(|s| s.observation.is_some()).count() as u32,
            unresolved_steps: d.unresolved_steps().len() as u32,
            pre_image_mismatch: d.pre_image_mismatch,
            closed: d.closed.clone(),
            environment,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Ok,
    Partial { failed: u32 },
    NotRun,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckErrorKind {
    RunnerFailure,
    Timeout,
    OutputLimit,
    Cancelled,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    NotRun,
    Passed {
        gate: String,
    },
    Failed {
        gate: String,
        command: String,
        exit: Option<i32>,
    },
    Error {
        gate: String,
        kind: CheckErrorKind,
    },
    Stale {
        gate: String,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpStatus {
    Passed,
    Failed,
    NotRun,
    Stale,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewStatus {
    NotApplicable,
    NotStarted,
    StartupFailed,
    Ready { http: HttpStatus },
    Stopped,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserStatus {
    NotVerifiedByProduct,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Unverified,
    Unmet { criteria: Vec<String> },
    ChecksPassedPendingReview,
    Accepted,
    Rejected,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Pending { n: u32 },
    Decided { accepted: u32, rejected: u32 },
    Stale { n: u32 },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    NotApplied,
    Applied {
        files: Vec<String>,
        worktree: String,
    },
    Interrupted,
    Reverted,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Risk {
    pub id: String,
    pub summary: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskOutcome {
    pub tool_status: ToolStatus,
    pub build_status: CheckStatus,
    pub test_status: CheckStatus,
    pub preview_status: PreviewStatus,
    pub browser_status: BrowserStatus,
    pub goal_status: GoalStatus,
    pub review_status: ReviewStatus,
    pub apply_status: ApplyStatus,
    pub unresolved_risks: Vec<Risk>,
}

/// Paths whose content differs between base and current.
pub fn changed_paths(ws: &WorkingSetManifest) -> BTreeSet<String> {
    ws.base
        .keys()
        .chain(ws.current.keys())
        .filter(|p| ws.base.get(*p) != ws.current.get(*p))
        .cloned()
        .collect()
}

/// Manifest of the accepted composition: Accepted -> current, Partial ->
/// composed, otherwise base (created-but-unaccepted files are absent).
pub fn accepted_manifest(
    ws: &WorkingSetManifest,
    review: &ReviewState,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for path in ws.base.keys().chain(ws.current.keys()) {
        let entry = if ws.base.get(path) == ws.current.get(path) {
            ws.base.get(path)
        } else {
            match review.files.get(path).map(|r| &r.decision) {
                Some(FileDecision::Accepted) => ws.current.get(path),
                Some(FileDecision::PartiallyAccepted {
                    composed_sha256, ..
                }) => Some(composed_sha256),
                _ => ws.base.get(path),
            }
        };
        if let Some(sha) = entry {
            out.insert(path.clone(), sha.clone());
        }
    }
    out
}

/// Content identity the checks must be bound to: the accepted composition
/// whenever review deviates from "take everything", else the current set.
pub fn evaluated_sha256(ws: &WorkingSetManifest, review: &ReviewState) -> String {
    let deviates = changed_paths(ws).iter().any(|p| {
        matches!(
            review.files.get(p).map(|r| &r.decision),
            Some(FileDecision::PartiallyAccepted { .. } | FileDecision::Rejected)
        )
    });
    if deviates {
        manifest_sha256(&accepted_manifest(ws, review))
    } else {
        ws.current_sha256.clone()
    }
}

fn expected_exit(spec: &TaskSpec, command_id: &str) -> i32 {
    spec.acceptance
        .iter()
        .filter(|c| c.confirmed_by_user)
        .find_map(|c| match &c.check {
            CriterionCheck::GateCommand {
                command_id: id,
                expected_exit,
            } if id == command_id => Some(*expected_exit),
            _ => None,
        })
        .unwrap_or(0)
}

fn error_kind(t: Termination) -> CheckErrorKind {
    match t {
        Termination::Timeout => CheckErrorKind::Timeout,
        Termination::OutputLimit => CheckErrorKind::OutputLimit,
        Termination::Cancelled => CheckErrorKind::Cancelled,
        _ => CheckErrorKind::RunnerFailure,
    }
}

fn role_status(view: &TaskHeadView, roles: &[GateRole], evaluated: &str) -> CheckStatus {
    let planned: Vec<&GateCommand> = view
        .spec
        .gate_plan
        .commands
        .iter()
        .filter(|c| roles.contains(&c.role()))
        .collect();
    if planned.is_empty() {
        return CheckStatus::NotRun;
    }
    let Some(latest) = view.gates.last() else {
        return CheckStatus::NotRun;
    };
    let Some(gate) = view
        .gates
        .iter()
        .rev()
        .find(|g| g.working_set_sha256 == evaluated)
    else {
        return CheckStatus::Stale {
            gate: latest.gate_run_id.clone(),
        };
    };
    let id = gate.gate_run_id.clone();
    if gate.termination == Termination::RunnerFailure {
        return CheckStatus::Error {
            gate: id,
            kind: CheckErrorKind::RunnerFailure,
        };
    }
    let mut failed = None;
    let mut missing = false;
    for command in planned {
        match gate.commands.iter().find(|c| c.id == command.id()) {
            Some(c) if c.termination != Termination::Completed => {
                return CheckStatus::Error {
                    gate: id,
                    kind: error_kind(c.termination),
                }
            }
            Some(c) if c.status != Some(expected_exit(&view.spec, &c.id)) => {
                failed.get_or_insert((c.id.clone(), c.status));
            }
            Some(_) => {}
            None => missing = true,
        }
    }
    if let Some((command, exit)) = failed {
        return CheckStatus::Failed {
            gate: id,
            command,
            exit,
        };
    }
    if missing {
        if gate.termination != Termination::Completed {
            return CheckStatus::Error {
                gate: id,
                kind: error_kind(gate.termination),
            };
        }
        return CheckStatus::NotRun;
    }
    CheckStatus::Passed { gate: id }
}

/// Pure outcome function. Its input cannot reach Narrative events or tool JSON.
pub fn assess(view: &TaskHeadView) -> TaskOutcome {
    let ws = &view.working_set;
    let evaluated = evaluated_sha256(ws, &view.review);
    let tool_status = if view.tool.denied > 0 {
        ToolStatus::Partial {
            failed: view.tool.denied,
        }
    } else if view.tool.applied > 0 {
        ToolStatus::Ok
    } else {
        ToolStatus::NotRun
    };
    let build_status = role_status(view, &[GateRole::Build], &evaluated);
    let test_status = role_status(view, &[GateRole::Test, GateRole::Oracle], &evaluated);

    // HTTP checks bound to the live descriptor's tree and service ids.
    let http_for = |descriptor: &ServiceDescriptor| -> HttpStatus {
        match view.http_checks.iter().rev().find(|r| {
            r.service_id == descriptor.service_id && r.tree_sha256 == descriptor.tree_sha256
        }) {
            None => HttpStatus::NotRun,
            Some(_) if descriptor.tree_sha256 != evaluated => HttpStatus::Stale,
            Some(r) if !r.results.is_empty() && r.results.iter().all(|x| x.passed) => {
                HttpStatus::Passed
            }
            Some(_) => HttpStatus::Failed,
        }
    };
    let preview_status = match (&view.spec.preview, &view.preview) {
        (None, _) => PreviewStatus::NotApplicable,
        (_, PreviewLedgerState::NotStarted) => PreviewStatus::NotStarted,
        (_, PreviewLedgerState::StartupFailed { .. }) => PreviewStatus::StartupFailed,
        (_, PreviewLedgerState::Running { descriptor }) => PreviewStatus::Ready {
            http: http_for(descriptor),
        },
        (
            _,
            PreviewLedgerState::Stopped { .. } | PreviewLedgerState::NotOwnedAfterRestart { .. },
        ) => PreviewStatus::Stopped,
    };

    // Review
    let changed = changed_paths(ws);
    let (mut pending, mut stale, mut accepted, mut rejected) = (0u32, 0u32, 0u32, 0u32);
    let mut partial = false;
    for path in &changed {
        match view.review.files.get(path) {
            None => pending += 1,
            Some(r) => match &r.decision {
                FileDecision::Pending => {
                    if r.reviewed_new_sha256.is_some()
                        && r.reviewed_new_sha256.as_ref() != ws.current.get(path)
                    {
                        stale += 1
                    } else {
                        pending += 1
                    }
                }
                FileDecision::Accepted => accepted += 1,
                FileDecision::PartiallyAccepted { .. } => {
                    accepted += 1;
                    partial = true
                }
                FileDecision::Rejected => rejected += 1,
            },
        }
    }
    let review_status = if stale > 0 {
        ReviewStatus::Stale { n: stale }
    } else if pending > 0 || changed.is_empty() {
        ReviewStatus::Pending { n: pending }
    } else {
        ReviewStatus::Decided { accepted, rejected }
    };

    // Goal: only user-confirmed machine criteria on the evaluated content.
    let bound_gate = view
        .gates
        .iter()
        .rev()
        .find(|g| g.working_set_sha256 == evaluated);
    let live_descriptor = match &view.preview {
        PreviewLedgerState::Running { descriptor } => Some(descriptor),
        _ => None,
    };
    let (mut unmet, mut unknown, mut machine, mut manual) = (vec![], false, 0, 0);
    for c in view.spec.acceptance.iter().filter(|c| c.confirmed_by_user) {
        match &c.check {
            CriterionCheck::Manual => manual += 1,
            CriterionCheck::GateCommand {
                command_id,
                expected_exit,
            } => {
                machine += 1;
                let result = bound_gate
                    .filter(|g| g.termination != Termination::RunnerFailure)
                    .and_then(|g| g.commands.iter().find(|x| &x.id == command_id));
                match result {
                    Some(x)
                        if x.termination == Termination::Completed
                            && x.status == Some(*expected_exit) => {}
                    Some(_) => unmet.push(c.id.clone()),
                    None => unknown = true,
                }
            }
            CriterionCheck::Http { check_id } => {
                machine += 1;
                let result = live_descriptor
                    .filter(|d| d.tree_sha256 == evaluated)
                    .and_then(|d| {
                        view.http_checks.iter().rev().find(|r| {
                            r.service_id == d.service_id && r.tree_sha256 == d.tree_sha256
                        })
                    })
                    .and_then(|r| r.results.iter().find(|x| &x.id == check_id));
                match result {
                    Some(x) if x.passed => {}
                    Some(_) => unmet.push(c.id.clone()),
                    None => unknown = true,
                }
            }
        }
    }
    let goal_status = if view.closed == Some(TaskStatus::Rejected) {
        GoalStatus::Rejected
    } else if !unmet.is_empty() {
        GoalStatus::Unmet { criteria: unmet }
    } else if machine == 0 || unknown {
        GoalStatus::Unverified
    } else if manual == 0
        && matches!(view.apply, ApplyStatus::Applied { .. })
        && matches!(review_status, ReviewStatus::Decided { accepted, .. } if accepted > 0)
    {
        GoalStatus::Accepted
    } else {
        GoalStatus::ChecksPassedPendingReview
    };

    // Risks (§8.4), stable ids, server-side only.
    let mut risks = BTreeMap::new();
    let mut add = |id: &str, summary: &str| {
        risks.insert(id.to_owned(), summary.to_owned());
    };
    for status in [&build_status, &test_status] {
        match status {
            CheckStatus::Failed { .. } | CheckStatus::Error { .. } => {
                add("checks.failing", "checks have failing or errored commands")
            }
            CheckStatus::NotRun => add(
                "checks.not_run",
                "checks have not run on the evaluated content",
            ),
            CheckStatus::Stale { .. } => {
                add("checks.stale", "latest checks ran on different content")
            }
            CheckStatus::Passed { .. } => {}
        }
    }
    if partial && !matches!(test_status, CheckStatus::Passed { .. }) {
        add(
            "review.partial_untested",
            "partial acceptance has not been tested",
        );
    }
    if matches!(review_status, ReviewStatus::Stale { .. }) {
        add("review.stale", "a reviewed file changed after review");
    }
    if view.tool.oracle_denied > 0 {
        add(
            "oracle.edit_attempted",
            "an edit to a protected oracle file was attempted and denied",
        );
    }
    if ws
        .current
        .keys()
        .any(|p| !view.spec.selected_files.contains(p))
    {
        add(
            "files.new_outside_selection",
            "new files outside the original selection",
        );
    }
    if !view.copy_out_paths.is_empty() {
        add(
            "files.sandbox_copy_out",
            "changes came from sandbox copy-out",
        );
    }
    if view.environment.source_changed == Some(true) {
        add(
            "source.changed_since_capture",
            "source files changed since capture",
        );
    }
    if view.interrupted_steps > 0 {
        add("steps.interrupted", "interrupted or reconciled steps exist");
    }
    if view.pre_image_mismatch {
        add(
            "worktree.pre_image_mismatch",
            "a worktree pre-image did not match",
        );
    }
    if view.spec.preview.is_some() {
        add(
            "preview.http_only",
            "HTTP-level preview only; browser rendering not verified by UnoOne",
        );
    }
    if matches!(view.preview, PreviewLedgerState::StartupFailed { .. }) {
        add("preview.startup_failed", "preview startup failed");
    }
    add(
        "isolation.residuals",
        "isolation residuals: no /proc, no cgroup quota, soft RSS watchdog only",
    );
    if view.spec.repository.label_source != LabelSource::UserLabel {
        add(
            "repository.label_unverified",
            "branch/HEAD label not verified",
        );
    }
    if !view.environment.execution_available {
        add(
            "platform.execution_unavailable",
            "execution unavailable on this platform",
        );
    }
    TaskOutcome {
        tool_status,
        build_status,
        test_status,
        preview_status,
        browser_status: BrowserStatus::NotVerifiedByProduct,
        goal_status,
        review_status,
        apply_status: view.apply.clone(),
        unresolved_risks: risks
            .into_iter()
            .map(|(id, summary)| Risk { id, summary })
            .collect(),
    }
}

/// Hash of the sorted risk-id set echoed by `UiApplyEvent`.
pub fn risks_sha256(risks: &[Risk]) -> String {
    let ids: BTreeSet<&str> = risks.iter().map(|r| r.id.as_str()).collect();
    kv::digest(&serde_json::to_vec(&ids).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Sealing, records, blobs
// ---------------------------------------------------------------------------

fn ledger_mac(vault: &Vault) -> Result<Hmac<Sha256>> {
    let master = vault.master_key().ok_or(LedgerError::Locked)?;
    let mut key = unoone_vault_core::crypto::derive_domain_key(master, LEDGER_HMAC_DOMAIN);
    let mac = Hmac::<Sha256>::new_from_slice(&key);
    unoone_vault_core::crypto::secure_zero(&mut key);
    mac.map_err(|_| LedgerError::Corrupt)
}

fn envelope_mac(vault: &Vault, kind: &str, unsigned: &[u8]) -> Result<Hmac<Sha256>> {
    let mut mac = ledger_mac(vault)?;
    mac.update(MAC_PREFIX);
    mac.update(kind.as_bytes());
    mac.update(b"\0");
    mac.update(unsigned);
    Ok(mac)
}

fn to_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| LedgerError::Corrupt)
}

fn seal_envelope<T: Serialize + Clone>(
    vault: &Vault,
    kind: &str,
    task_id: &str,
    seq: u64,
    body: &T,
) -> Result<Vec<u8>> {
    let mut envelope = Envelope {
        schema: LEDGER_SCHEMA.into(),
        vault_id: vault.vault_id().ok_or(LedgerError::Corrupt)?.to_owned(),
        task_id: task_id.to_owned(),
        kind: kind.to_owned(),
        seq,
        body: body.clone(),
        tag: String::new(),
    };
    let unsigned = to_json(&envelope)?;
    envelope.tag = hex(&envelope_mac(vault, kind, &unsigned)?
        .finalize()
        .into_bytes());
    to_json(&envelope)
}

fn open_envelope<T: Serialize + DeserializeOwned>(
    vault: &Vault,
    bytes: &[u8],
    kind: &str,
    task_id: &str,
) -> Result<(u64, T)> {
    let mut envelope: Envelope<T> =
        serde_json::from_slice(bytes).map_err(|_| LedgerError::Corrupt)?;
    // The stored bytes must equal the typed re-serialization (Stage 4 pattern).
    if to_json(&envelope)? != bytes
        || envelope.schema != LEDGER_SCHEMA
        || envelope.kind != kind
        || envelope.task_id != task_id
        || Some(envelope.vault_id.as_str()) != vault.vault_id()
        || envelope.tag.len() != 64
        || !is_lower_hex(&envelope.tag)
    {
        return Err(LedgerError::Corrupt);
    }
    let mut decoded = [0u8; 32];
    for (i, byte) in decoded.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&envelope.tag[2 * i..2 * i + 2], 16)
            .map_err(|_| LedgerError::Corrupt)?;
    }
    envelope.tag.clear();
    let unsigned = to_json(&envelope)?;
    envelope_mac(vault, kind, &unsigned)?
        .verify_slice(&decoded)
        .map_err(|_| LedgerError::Corrupt)?;
    Ok((envelope.seq, envelope.body))
}

fn read_sealed(
    vault: &Vault,
    limits: &LedgerLimits,
    uuid: &str,
) -> Result<Option<(Record, Vec<u8>)>> {
    match kv::optional_record(vault, uuid)? {
        None => Ok(None),
        Some((meta, bytes)) => {
            if bytes.len() > limits.max_record_bytes {
                return Err(LedgerError::LedgerFull);
            }
            kv::check_metadata(&meta, uuid, &bytes)?;
            Ok(Some((meta, bytes)))
        }
    }
}

fn load_index(vault: &Vault, limits: &LedgerLimits) -> Result<Option<(Record, IndexBody, u64)>> {
    let Some((meta, bytes)) = read_sealed(vault, limits, &TaskLedger::index_uuid())? else {
        return Ok(None);
    };
    let (generation, body): (u64, IndexBody) = open_envelope(vault, &bytes, "index", "")?;
    let ids: BTreeSet<&TaskId> = body.tasks.iter().map(|t| &t.task_id).collect();
    if body.tasks.len() > MAX_TASKS
        || ids.len() != body.tasks.len()
        || body.reserved_records > MAX_LEDGER_RECORDS
        || body
            .tasks
            .iter()
            .any(|t| unoone_vault_core::vault::validate_record_id(&t.head_uuid).is_err())
    {
        return Err(LedgerError::Corrupt);
    }
    Ok(Some((meta, body, generation)))
}

fn write_index(
    vault: &mut Vault,
    limits: &LedgerLimits,
    previous: Option<Record>,
    body: &IndexBody,
    generation: u64,
) -> Result<Record> {
    let uuid = TaskLedger::index_uuid();
    let bytes = seal_envelope(vault, "index", "", generation, body)?;
    if bytes.len() > limits.max_record_bytes {
        return Err(LedgerError::LedgerFull);
    }
    match previous {
        None => kv::write_new(vault, &uuid, &bytes)?,
        Some(mut meta) => {
            meta.privacy_level = PrivacyLevel::Private;
            vault.write_record(meta, &bytes).map_err(kv::map_error)?;
        }
    }
    let (meta, readback) = read_sealed(vault, limits, &uuid)?.ok_or(LedgerError::Corrupt)?;
    if readback != bytes {
        return Err(LedgerError::Corrupt);
    }
    Ok(meta)
}

fn reserve(limits: &LedgerLimits, body: &IndexBody, new_records: u32) -> Result<()> {
    // +1 for the index record itself.
    match body
        .reserved_records
        .checked_add(new_records)
        .and_then(|n| n.checked_add(1))
    {
        Some(total) if total <= limits.max_records => Ok(()),
        _ => Err(LedgerError::LedgerFull),
    }
}

fn load_head(vault: &Vault, limits: &LedgerLimits, task: &TaskId) -> Result<(Record, TaskHead)> {
    let (_, index, _) = load_index(vault, limits)?.ok_or(LedgerError::NotFound)?;
    let entry = index
        .tasks
        .iter()
        .find(|t| &t.task_id == task)
        .ok_or(LedgerError::NotFound)?;
    let (meta, bytes) =
        read_sealed(vault, limits, &entry.head_uuid)?.ok_or(LedgerError::Corrupt)?;
    let (seq, head): (u64, TaskHead) = open_envelope(vault, &bytes, "head", task.as_str())?;
    if &head.task_id != task
        || seq != head.seq()
        || head.journal.is_empty()
        || head.journal.len() > limits.max_journal_entries
        || head.blobs.len() > limits.max_blobs_per_task
        || head.spec_sha256 != sha_json(&head.spec)?
        || head.created_at_ms != entry.created_at_ms
        || head.blobs.iter().collect::<BTreeSet<_>>().len() != head.blobs.len()
    {
        return Err(LedgerError::Corrupt);
    }
    // Full deterministic replay: chain + state machine.
    replay(&head).map_err(|_| LedgerError::Corrupt)?;
    Ok((meta, head))
}

struct BlobPlan {
    refs: Vec<BlobRef>,
    new_records: Vec<(String, Vec<u8>)>,
}

fn blob_uuid(vault: &Vault, task_id: &str, kind: BlobKind, sha256: &str) -> Result<String> {
    let mut mac = ledger_mac(vault)?;
    mac.update(b"blob\0");
    mac.update(task_id.as_bytes());
    mac.update(b"\0");
    mac.update(kind.name().as_bytes());
    mac.update(b"\0");
    mac.update(sha256.as_bytes());
    let out = mac.finalize().into_bytes();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&out[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h = hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}

fn prepare_blobs(
    vault: &Vault,
    limits: &LedgerLimits,
    head: &TaskHead,
    blobs: &[NewBlob],
) -> Result<BlobPlan> {
    let mut plan = BlobPlan {
        refs: vec![],
        new_records: vec![],
    };
    let mut seen = BTreeSet::new();
    for blob in blobs {
        if blob.bytes.len() > MAX_BLOB_RAW_BYTES {
            return Err(LedgerError::LedgerFull);
        }
        let r = blob.blob_ref();
        if !seen.insert((r.kind, r.sha256.clone())) {
            continue;
        }
        plan.refs.push(r.clone());
        if head.has_blob(r.kind, &r.sha256) {
            continue;
        }
        let uuid = blob_uuid(vault, head.task_id.as_str(), r.kind, &r.sha256)?;
        if read_sealed(vault, limits, &uuid)?.is_some() {
            // Create-only + idempotent: an existing record must verify to the same content.
            if read_blob_record(vault, limits, &head.task_id, &r)? != blob.bytes {
                return Err(LedgerError::Corrupt);
            }
            continue;
        }
        let body = BlobBody {
            kind: r.kind,
            sha256: r.sha256.clone(),
            hex: hex(&blob.bytes),
        };
        let bytes = seal_envelope(vault, "blob", head.task_id.as_str(), 0, &body)?;
        if bytes.len() > limits.max_record_bytes {
            return Err(LedgerError::LedgerFull);
        }
        plan.new_records.push((uuid, bytes));
    }
    Ok(plan)
}

fn write_blob_records(vault: &mut Vault, _task: &str, records: &[(String, Vec<u8>)]) -> Result<()> {
    for (uuid, bytes) in records {
        kv::write_new(vault, uuid, bytes)?;
    }
    Ok(())
}

fn read_blob_record(
    vault: &Vault,
    limits: &LedgerLimits,
    task: &TaskId,
    blob: &BlobRef,
) -> Result<Vec<u8>> {
    if !is_sha256(&blob.sha256) {
        return Err(LedgerError::Invalid("blob sha"));
    }
    let uuid = blob_uuid(vault, task.as_str(), blob.kind, &blob.sha256)?;
    let (_, bytes) = read_sealed(vault, limits, &uuid)?.ok_or(LedgerError::NotFound)?;
    let (seq, body): (u64, BlobBody) = open_envelope(vault, &bytes, "blob", task.as_str())?;
    let content = unhex(&body.hex).ok_or(LedgerError::Corrupt)?;
    if seq != 0
        || body.kind != blob.kind
        || body.sha256 != blob.sha256
        || kv::digest(&content) != blob.sha256
    {
        return Err(LedgerError::Corrupt);
    }
    Ok(content)
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !is_lower_hex(s) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}
pub(crate) fn is_lower_hex(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn is_sha256(s: &str) -> bool {
    s.len() == 64 && is_lower_hex(s)
}
fn is_event_id(s: &str) -> bool {
    s.len() == 32 && is_lower_hex(s)
}
fn is_event_id_or_empty(s: &str) -> bool {
    s.is_empty() || is_event_id(s)
}
pub(crate) fn random_hex16() -> String {
    hex(&unoone_vault_core::crypto::generate_nonce()[..16])
}
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
pub(crate) fn sha_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(kv::digest(&to_json(value)?))
}
fn genesis(task: &TaskId) -> String {
    kv::digest(format!("inbharat.pai.task-ledger.genesis\0{}", task.as_str()).as_bytes())
}
/// Identifier syntax for step/criterion/command/gate ids.
pub(crate) fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}
/// Canonical relative path: non-empty, <= 256 bytes, normal components only,
/// no backslash, no NUL, depth <= 8 (same rules as `isolation::relative`).
pub(crate) fn valid_rel_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 256
        && !p.contains('\\')
        && !p.contains('\0')
        && !p.starts_with('/')
        && p.split('/').count() <= 8
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}
fn valid_prefix(p: &str) -> bool {
    p.ends_with('/') && valid_rel_path(p.trim_end_matches('/'))
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FaultPoint {
    AfterBlobsBeforeHead,
}
#[cfg(test)]
thread_local! {
    static FAULT: std::cell::Cell<Option<FaultPoint>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
pub(crate) fn set_fault(point: Option<FaultPoint>) {
    FAULT.with(|f| f.set(point));
}
#[cfg(test)]
fn fault_hit(point: FaultPoint) -> bool {
    FAULT.with(|f| {
        if f.get() == Some(point) {
            f.set(None);
            true
        } else {
            false
        }
    })
}

// ---------------------------------------------------------------------------
// Test support shared with coding_task tests (disposable REAL vaults)
// ---------------------------------------------------------------------------
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::coding_task::ports::{
        CopyOutMode, CopyOutRequest, GateCommand, GatePlan, GateRole, WorkspaceLimits,
        GATE_PLAN_SCHEMA,
    };
    use std::path::{Path, PathBuf};

    pub(crate) const PASSWORD: &[u8] = b"stage5-ledger-synthetic-vault";

    pub(crate) fn vault_fixture() -> (tempfile::TempDir, Arc<Mutex<Option<Vault>>>, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("vault");
        Vault::create(&root, PASSWORD).unwrap();
        let handle = reopen(&root);
        (temp, handle, root)
    }
    pub(crate) fn reopen(root: &Path) -> Arc<Mutex<Option<Vault>>> {
        let mut vault = Vault::open(root).unwrap();
        vault.unlock(PASSWORD).unwrap();
        Arc::new(Mutex::new(Some(vault)))
    }
    pub(crate) fn limits() -> WorkspaceLimits {
        WorkspaceLimits {
            cpu_seconds: 10,
            memory_bytes: 256 * 1024 * 1024,
            processes: 16,
            work_tmpfs_bytes: 32 * 1024 * 1024,
            file_size_bytes: 1024 * 1024,
            open_files: 64,
            total_timeout_ms: 30_000,
            rss_watchdog_bytes: 1024 * 1024 * 1024,
        }
    }
    /// Temperature/duration fixture (differs from the held-out quantity/invoice).
    pub(crate) fn sample_plan() -> GatePlan {
        GatePlan {
            schema: GATE_PLAN_SCHEMA.into(),
            commands: vec![
                GateCommand::PythonCompile {
                    id: "build".into(),
                    role: GateRole::Build,
                    files: vec!["temperature.py".into(), "duration.py".into()],
                    timeout_ms: 5_000,
                    output_bytes: 4096,
                },
                GateCommand::PythonUnittest {
                    id: "oracle".into(),
                    role: GateRole::Oracle,
                    start_dir: "tests".into(),
                    pattern: "test_*.py".into(),
                    timeout_ms: 5_000,
                    output_bytes: 4096,
                },
            ],
            stop_on_failure: false,
            copy_out: CopyOutRequest {
                mode: CopyOutMode::Off,
                ignore_dir_names: ["__pycache__".to_owned(), ".pytest_cache".to_owned()].into(),
                max_files: 16,
                max_total_bytes: 1024 * 1024,
            },
            limits: limits(),
        }
    }
    pub(crate) fn sample_files(marker: &str) -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            (
                "temperature.py".to_owned(),
                format!("# {marker}\ndef to_kelvin(c):\n    return c + 273\n").into_bytes(),
            ),
            (
                "duration.py".to_owned(),
                b"import temperature\n\ndef window(seconds, step):\n    return [seconds[i:i + step] for i in range(0, len(seconds), step)]\n".to_vec(),
            ),
            (
                "tests/test_temperature.py".to_owned(),
                b"import unittest\nimport temperature\nfrom tests import helpers\n\nclass T(unittest.TestCase):\n    def test_k(self):\n        self.assertEqual(temperature.to_kelvin(0), helpers.ZERO)\n# ORACLE-SECRET-VECTOR\n".to_vec(),
            ),
            ("tests/helpers.py".to_owned(), b"ZERO = 273.15\n".to_vec()),
        ])
    }
    pub(crate) fn sample_spec(objective: &str) -> TaskSpec {
        TaskSpec {
            objective: objective.into(),
            repository: RepositoryLabel {
                display_root: "/synthetic/projects/stage5/repo".into(),
                source_id: format!("repo:{}", "a".repeat(64)),
                branch: Some("main".into()),
                head_commit: None,
                label_source: LabelSource::GitFilesUnverified,
            },
            selected_files: sample_files("x").keys().cloned().collect(),
            primary: "duration.py".into(),
            oracle_files: [
                "tests/test_temperature.py".to_owned(),
                "tests/helpers.py".to_owned(),
            ]
            .into(),
            oracle_visibility: OracleVisibility::Hidden,
            acceptance: vec![
                AcceptanceCriterion {
                    id: "build-ok".into(),
                    text: "compiles".into(),
                    check: CriterionCheck::GateCommand {
                        command_id: "build".into(),
                        expected_exit: 0,
                    },
                    confirmed_by_user: true,
                },
                AcceptanceCriterion {
                    id: "oracle-ok".into(),
                    text: "held tests pass".into(),
                    check: CriterionCheck::GateCommand {
                        command_id: "oracle".into(),
                        expected_exit: 0,
                    },
                    confirmed_by_user: true,
                },
            ],
            gate_plan: sample_plan(),
            preview: None,
            repair: RepairBudget::default(),
        }
    }
    pub(crate) fn oracle_for(spec: &TaskSpec) -> OracleDerivation {
        OracleDerivation {
            declared: spec.oracle_files.clone(),
            derived: BTreeSet::new(),
            implementation_closure: BTreeSet::new(),
            protected: spec.oracle_files.clone(),
            reasons: BTreeMap::new(),
        }
    }
    pub(crate) fn snapshot() -> String {
        "b".repeat(64)
    }
    pub(crate) fn create(ledger: &TaskLedger, boot: &BootInfo, marker: &str) -> TaskHead {
        let spec = sample_spec(marker);
        let oracle = oracle_for(&spec);
        ledger
            .create_task(
                &boot.guard(),
                spec,
                oracle,
                vec!["tests/".into()],
                &sample_files(marker),
                &snapshot(),
            )
            .unwrap()
    }
    pub(crate) fn admission(epoch: u64, purpose: AdmissionPurpose) -> LedgerEvent {
        LedgerEvent::AdmissionChecked {
            trace: AdmissionTrace {
                purpose,
                vault_unlocked: true,
                epoch,
                capability: CapabilityState::RuntimeVerified,
                workspace_profile_sha256: Some("c".repeat(64)),
                preflight_ok: Some(true),
                roots_ok: None,
                source_unchanged: None,
                worktree_identity_ok: None,
                admitted: true,
                at_ms: 1,
            },
        }
    }
    pub(crate) fn intent(effect: EffectClass) -> StepIntent {
        StepIntent {
            idempotency_key: "k".into(),
            effect,
            working_set_sha256: "d".repeat(64),
            worktree: None,
            pre_image: BTreeMap::new(),
            post_image: BTreeMap::new(),
        }
    }
    pub(crate) fn started(step: &str, effect: EffectClass) -> LedgerEvent {
        LedgerEvent::StepStarted {
            step_id: step.into(),
            attempt: 1,
            intent: intent(effect),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::coding_task::ports::{FileReview, ReadyState, ServiceDescriptor};
    use std::cell::Cell;
    use std::path::Path;

    fn boot() -> BootInfo {
        BootInfo::new(Arc::new(AtomicU64::new(0)))
    }
    fn walk(dir: &Path, out: &mut Vec<Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(std::fs::read(&path).unwrap());
            }
        }
    }
    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }
    fn head_uuid(ledger: &TaskLedger, task: &TaskId) -> String {
        ledger
            .list()
            .unwrap()
            .into_iter()
            .find(|e| &e.task_id == task)
            .unwrap()
            .head_uuid
    }
    fn narrative(text: &str) -> (LedgerEvent, NewBlob) {
        let blob = NewBlob::new(BlobKind::Narrative, text.as_bytes().to_vec());
        let r = blob.blob_ref();
        (
            LedgerEvent::Narrative {
                role: "implementer".into(),
                text_sha256: r.sha256.clone(),
                blob: r,
            },
            blob,
        )
    }
    fn binding() -> WorktreeBinding {
        WorktreeBinding {
            path: "/synthetic/worktrees/pai-task-0001".into(),
            dev: 7,
            ino: 99,
            created_at_ms: 1,
        }
    }
    fn host_write(pre: &[(&str, Option<&str>)], post: &[(&str, Option<&str>)]) -> LedgerEvent {
        let map = |v: &[(&str, Option<&str>)]| {
            v.iter()
                .map(|(p, h)| (p.to_string(), h.map(str::to_owned)))
                .collect::<BTreeMap<_, _>>()
        };
        LedgerEvent::StepStarted {
            step_id: "apply-9".into(),
            attempt: 1,
            intent: StepIntent {
                idempotency_key: "apply:9".into(),
                effect: EffectClass::HostWrite,
                working_set_sha256: "d".repeat(64),
                worktree: Some(binding()),
                pre_image: map(pre),
                post_image: map(post),
            },
        }
    }

    /// FAKE worktree probe with a mutation counter (it has no mutating path).
    struct FakeProbe {
        files: BTreeMap<String, Option<String>>,
        identity_ok: bool,
        mutations: Cell<u32>,
        observations: Cell<u32>,
    }
    impl WorktreeProbe for FakeProbe {
        fn observe(
            &self,
            b: &WorktreeBinding,
            paths: &[String],
        ) -> std::result::Result<BTreeMap<String, Option<String>>, ProbeFailure> {
            self.observations.set(self.observations.get() + 1);
            if !self.identity_ok || b != &binding() {
                return Err(ProbeFailure::IdentityChanged);
            }
            Ok(paths
                .iter()
                .map(|p| (p.clone(), self.files.get(p).cloned().flatten()))
                .collect())
        }
    }
    fn probe(files: &[(&str, Option<&str>)], identity_ok: bool) -> FakeProbe {
        FakeProbe {
            files: files
                .iter()
                .map(|(p, h)| (p.to_string(), h.map(str::to_owned)))
                .collect(),
            identity_ok,
            mutations: Cell::new(0),
            observations: Cell::new(0),
        }
    }

    #[test]
    fn commit_roundtrip_is_encrypted_and_maced() {
        let (_t, vault, root) = vault_fixture();
        let ledger = TaskLedger::new(vault.clone());
        let b = boot();
        let marker = "STAGE5-PLAINTEXT-MARKER-7f3a";
        let head = create(&ledger, &b, marker);
        let (event, blob) = narrative(&format!("model says {marker}"));
        let seq = ledger
            .commit(
                &head.task_id,
                head.seq(),
                &b.guard(),
                vec![event],
                vec![blob.clone()],
            )
            .unwrap();
        assert_eq!(seq, 3);
        // Encrypted at rest: no plaintext (or hex-encoded plaintext) anywhere in the vault dir.
        let mut files = vec![];
        walk(&root, &mut files);
        assert!(files.len() >= 6);
        let hexed = hex(marker.as_bytes());
        for bytes in &files {
            assert!(!contains(bytes, marker.as_bytes()));
            assert!(!contains(bytes, hexed.as_bytes()));
        }
        // Round trip through the verified (MAC + chain + replay) path.
        let loaded = ledger.load(&head.task_id).unwrap();
        assert_eq!(loaded.seq(), 3);
        assert!(loaded.spec.objective.contains(marker));
        assert_eq!(
            ledger.read_blob(&head.task_id, &blob.blob_ref()).unwrap(),
            blob.bytes
        );
        // Records are Memory/Private canonical-AAD records of the SAME vault.
        let uuid = head_uuid(&ledger, &head.task_id);
        let guard = vault.lock().unwrap();
        let v = guard.as_ref().unwrap();
        let (meta, bytes) = kv::strict_read(v, &uuid).unwrap();
        kv::check_metadata(&meta, &meta.record_id, &bytes).unwrap();
        assert_eq!(meta.privacy_level, PrivacyLevel::Private);
        let (imeta, ibytes) = kv::strict_read(v, &TaskLedger::index_uuid()).unwrap();
        kv::check_metadata(&imeta, &TaskLedger::index_uuid(), &ibytes).unwrap();
    }

    #[test]
    fn generic_vault_writer_forgery_rejected() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault.clone());
        let b = boot();
        let a = create(&ledger, &b, "task-a");
        let other = create(&ledger, &b, "task-b");
        let a_uuid = head_uuid(&ledger, &a.task_id);
        let b_uuid = head_uuid(&ledger, &other.task_id);
        let original = {
            let g = vault.lock().unwrap();
            g.as_ref().unwrap().read_record(&a_uuid).unwrap()
        };
        let generic_write = |uuid: &str, bytes: &[u8]| {
            let mut g = vault.lock().unwrap();
            let v = g.as_mut().unwrap();
            let (meta, _) = v.read_record(uuid).unwrap();
            v.write_record(meta, bytes).unwrap();
        };
        let canonical = |mutate: &dyn Fn(&mut Envelope<TaskHead>, &Vault)| -> Vec<u8> {
            let g = vault.lock().unwrap();
            let v = g.as_ref().unwrap();
            let mut env: Envelope<TaskHead> = serde_json::from_slice(&original.1).unwrap();
            mutate(&mut env, v);
            to_json(&env).unwrap()
        };
        // (a) zero tag
        generic_write(&a_uuid, &canonical(&|e, _| e.tag = "0".repeat(64)));
        assert_eq!(ledger.load(&a.task_id), Err(LedgerError::Corrupt));
        // (b) cross-domain tag: a valid HMAC under the Stage 4 receipt domain key
        generic_write(
            &a_uuid,
            &canonical(&|e, v| {
                e.tag.clear();
                let unsigned = to_json(&*e).unwrap();
                let mut key = unoone_vault_core::crypto::derive_domain_key(
                    v.master_key().unwrap(),
                    "inbharat.pai.stage4.check.hmac-sha256.v1",
                );
                let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
                unoone_vault_core::crypto::secure_zero(&mut key);
                mac.update(MAC_PREFIX);
                mac.update(b"head\0");
                mac.update(&unsigned);
                e.tag = hex(&mac.finalize().into_bytes());
            }),
        );
        assert_eq!(ledger.load(&a.task_id), Err(LedgerError::Corrupt));
        // (c) body edit keeping the genuine tag
        generic_write(
            &a_uuid,
            &canonical(&|e, _| e.body.spec.objective = "forged".into()),
        );
        assert_eq!(ledger.load(&a.task_id), Err(LedgerError::Corrupt));
        // (d) non-canonical bytes with the genuine tag (whitespace)
        let mut spaced = original.1.clone();
        spaced.insert(1, b' ');
        generic_write(&a_uuid, &spaced);
        assert_eq!(ledger.load(&a.task_id), Err(LedgerError::Corrupt));
        // (e) genuine head copied from ANOTHER task into this task's UUID
        let b_bytes = {
            let g = vault.lock().unwrap();
            g.as_ref().unwrap().read_record(&b_uuid).unwrap().1
        };
        generic_write(&a_uuid, &b_bytes);
        assert_eq!(ledger.load(&a.task_id), Err(LedgerError::Corrupt));
        // Restoring the genuine bytes verifies again (rejections were the MAC/binding).
        generic_write(&a_uuid, &original.1);
        assert_eq!(ledger.load(&a.task_id).unwrap().seq(), 2);
        // (f) index forged by a generic writer -> every list/load fails closed
        let index_bytes = {
            let g = vault.lock().unwrap();
            g.as_ref()
                .unwrap()
                .read_record(&TaskLedger::index_uuid())
                .unwrap()
                .1
        };
        let mut env: Envelope<IndexBody> = serde_json::from_slice(&index_bytes).unwrap();
        env.body.tasks[0].head_uuid = b_uuid.clone();
        env.tag = "0".repeat(64);
        generic_write(&TaskLedger::index_uuid(), &to_json(&env).unwrap());
        assert_eq!(ledger.list(), Err(LedgerError::Corrupt));
        generic_write(&TaskLedger::index_uuid(), &index_bytes);
        // (g) blob overwritten by a generic writer
        let file_sha = a.base_manifest["temperature.py"].clone();
        let blob_uuid_a = {
            let g = vault.lock().unwrap();
            blob_uuid(
                g.as_ref().unwrap(),
                a.task_id.as_str(),
                BlobKind::FileContent,
                &file_sha,
            )
            .unwrap()
        };
        generic_write(&blob_uuid_a, b"{}");
        assert_eq!(
            ledger.read_blob_by_sha(&a.task_id, BlobKind::FileContent, &file_sha),
            Err(LedgerError::Corrupt)
        );
        // Keyholder residual: a re-seal with the DERIVED ledger key is accepted (documented).
        let resealed = {
            let g = vault.lock().unwrap();
            let v = g.as_ref().unwrap();
            let mut head: TaskHead = serde_json::from_slice::<Envelope<TaskHead>>(&original.1)
                .unwrap()
                .body;
            head.created_at_ms = a.created_at_ms;
            seal_envelope(v, "head", a.task_id.as_str(), head.seq(), &head).unwrap()
        };
        generic_write(&a_uuid, &resealed);
        assert!(ledger.load(&a.task_id).is_ok());
        assert!(LEDGER_RESIDUALS
            .iter()
            .any(|r| r.starts_with("keyholder-forgery")));
    }

    #[test]
    fn stale_seq_conflict() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault);
        let b = boot();
        let head = create(&ledger, &b, "m");
        let (e1, b1) = narrative("one");
        ledger
            .commit(&head.task_id, 2, &b.guard(), vec![e1], vec![b1])
            .unwrap();
        let (e2, b2) = narrative("two");
        assert_eq!(
            ledger.commit(&head.task_id, 2, &b.guard(), vec![e2], vec![b2]),
            Err(LedgerError::Conflict)
        );
        assert_eq!(ledger.load(&head.task_id).unwrap().seq(), 3);
    }

    #[test]
    fn illegal_transition_rejected() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault);
        let boot_a = boot();
        let head = create(&ledger, &boot_a, "m");
        let t = &head.task_id;
        let completed = |step: &str| LedgerEvent::StepCompleted {
            step_id: step.into(),
            attempt: 1,
            result: StepResultRef::Nothing,
        };
        // Pure start without an admission trace in the same commit
        assert!(matches!(
            ledger.commit(
                t,
                2,
                &boot_a.guard(),
                vec![started("gate-1", EffectClass::Pure)],
                vec![]
            ),
            Err(LedgerError::IllegalTransition(_))
        ));
        // Completion of an unknown step
        assert!(matches!(
            ledger.commit(t, 2, &boot_a.guard(), vec![completed("gate-1")], vec![]),
            Err(LedgerError::IllegalTransition(_))
        ));
        let seq = ledger
            .commit(
                t,
                2,
                &boot_a.guard(),
                vec![
                    admission(0, AdmissionPurpose::Gate),
                    started("gate-1", EffectClass::Pure),
                ],
                vec![],
            )
            .unwrap();
        // Started -> Completed across boots is illegal
        let boot_b = BootInfo::new(Arc::new(AtomicU64::new(0)));
        assert_eq!(
            ledger.commit(t, seq, &boot_b.guard(), vec![completed("gate-1")], vec![]),
            Err(LedgerError::IllegalTransition(
                "completion across boot or epoch"
            ))
        );
        // Interruption cannot be written through the public commit path
        assert!(matches!(
            ledger.commit(
                t,
                seq,
                &boot_b.guard(),
                vec![LedgerEvent::StepInterrupted {
                    step_id: "gate-1".into(),
                    attempt: 1,
                    observation: ReconcileObservation::NotApplicable,
                }],
                vec![]
            ),
            Err(LedgerError::IllegalTransition(_))
        ));
        // Restarting a Started step is illegal
        assert!(matches!(
            ledger.commit(
                t,
                seq,
                &boot_a.guard(),
                vec![
                    admission(0, AdmissionPurpose::Gate),
                    started("gate-1", EffectClass::Pure)
                ],
                vec![]
            ),
            Err(LedgerError::IllegalTransition(_))
        ));
        // Same boot + epoch completes
        let seq = ledger
            .commit(t, seq, &boot_a.guard(), vec![completed("gate-1")], vec![])
            .unwrap();
        // Unconfirmed model plan steps cannot start; confirmation unlocks them.
        let plan = Plan {
            revision: 1,
            author: PlanAuthor::Model,
            steps: vec![PlannedStep {
                step_id: "s-edit".into(),
                kind: StepKind::Edit,
                summary: "edit".into(),
                effect: EffectClass::LedgerOnly,
            }],
            confirmed: false,
        };
        let seq = ledger
            .commit(
                t,
                seq,
                &boot_a.guard(),
                vec![LedgerEvent::PlanRecorded { plan }],
                vec![],
            )
            .unwrap();
        assert_eq!(
            ledger.commit(
                t,
                seq,
                &boot_a.guard(),
                vec![started("s-edit", EffectClass::LedgerOnly)],
                vec![]
            ),
            Err(LedgerError::IllegalTransition(
                "model plan step not confirmed"
            ))
        );
        let seq = ledger
            .commit(
                t,
                seq,
                &boot_a.guard(),
                vec![LedgerEvent::PlanConfirmed {
                    revision: 1,
                    ui_event_id: "e".repeat(32),
                }],
                vec![],
            )
            .unwrap();
        let seq = ledger
            .commit(
                t,
                seq,
                &boot_a.guard(),
                vec![
                    started("s-edit", EffectClass::LedgerOnly),
                    completed("s-edit"),
                ],
                vec![],
            )
            .unwrap();
        // Model-authored plan cannot claim to be confirmed
        let forged = Plan {
            revision: 2,
            author: PlanAuthor::Model,
            steps: vec![],
            confirmed: true,
        };
        assert!(ledger
            .commit(
                t,
                seq,
                &boot_a.guard(),
                vec![LedgerEvent::PlanRecorded { plan: forged }],
                vec![]
            )
            .is_err());
        // Closed tasks accept no new steps
        let seq = ledger
            .commit(
                t,
                seq,
                &boot_a.guard(),
                vec![LedgerEvent::TaskClosed {
                    status: TaskStatus::Cancelled,
                }],
                vec![],
            )
            .unwrap();
        assert_eq!(
            ledger.commit(
                t,
                seq,
                &boot_a.guard(),
                vec![
                    admission(0, AdmissionPurpose::Gate),
                    started("gate-2", EffectClass::Pure)
                ],
                vec![]
            ),
            Err(LedgerError::IllegalTransition("task closed"))
        );
        // Nothing illegal was ever persisted: the stored journal replays cleanly.
        let (_, d) = ledger.load_derived(t).unwrap();
        assert_eq!(d.status(), TaskStatus::Cancelled);
    }

    #[test]
    fn checkpoint_decision_change_requires_ui_review_event() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault);
        let b = boot();
        let head = create(&ledger, &b, "m");
        let (_, d) = ledger.load_derived(&head.task_id).unwrap();
        let mut ws = d.working_set().unwrap().clone();
        let new = b"def to_kelvin(c):\n    return c + 273.15\n".to_vec();
        let sha = kv::digest(&new);
        ws.current.insert("temperature.py".into(), sha.clone());
        ws.current_sha256 = manifest_sha256(&ws.current);
        let cp = |ws: &WorkingSetManifest, review: ReviewState| LedgerEvent::Checkpoint {
            checkpoint: Checkpoint {
                seq: 0,
                working_set: ws.clone(),
                review,
                last_gate: None,
                repair_attempts_used: 0,
                preview: PreviewLedgerState::NotStarted,
            },
        };
        let edit = LedgerEvent::EditApplied {
            origin: EditOrigin::Model,
            manifest_sha256: ws.current_sha256.clone(),
            paths: vec!["temperature.py".into()],
        };
        let accepted = ReviewState {
            files: BTreeMap::from([(
                "temperature.py".to_owned(),
                FileReview {
                    decision: FileDecision::Accepted,
                    reviewed_base_sha256: ws.base.get("temperature.py").cloned(),
                    reviewed_new_sha256: Some(sha.clone()),
                    ui_event_id: "f".repeat(32),
                },
            )]),
        };
        let blob = NewBlob::new(BlobKind::FileContent, new);
        // A model edit that smuggles an Accepted decision into the checkpoint is rejected.
        assert_eq!(
            ledger.commit(
                &head.task_id,
                2,
                &b.guard(),
                vec![edit.clone(), cp(&ws, accepted.clone())],
                vec![blob.clone()]
            ),
            Err(LedgerError::IllegalTransition(
                "decision changed without a UI review event"
            ))
        );
        // An edit without its checkpoint is rejected; a checkpoint without the blob too.
        assert!(ledger
            .commit(
                &head.task_id,
                2,
                &b.guard(),
                vec![edit.clone()],
                vec![blob.clone()]
            )
            .is_err());
        assert!(ledger
            .commit(
                &head.task_id,
                2,
                &b.guard(),
                vec![edit.clone(), cp(&ws, ReviewState::default())],
                vec![]
            )
            .is_err());
        let seq = ledger
            .commit(
                &head.task_id,
                2,
                &b.guard(),
                vec![edit, cp(&ws, ReviewState::default())],
                vec![blob],
            )
            .unwrap();
        // With the UI review event the same decision is accepted.
        ledger
            .commit(
                &head.task_id,
                seq,
                &b.guard(),
                vec![
                    LedgerEvent::FileReviewed {
                        path: "temperature.py".into(),
                        decision: FileDecision::Accepted,
                        reviewed_new_sha256: Some(sha),
                        ui_event_id: "f".repeat(32),
                    },
                    cp(&ws, accepted),
                ],
                vec![],
            )
            .unwrap();
    }

    #[test]
    fn crash_between_blob_and_head_consistent() {
        let (_t, vault, root) = vault_fixture();
        let ledger = TaskLedger::new(vault.clone());
        let b = boot();
        let head = create(&ledger, &b, "m");
        let (event, blob) = narrative("crash candidate");
        set_fault(Some(FaultPoint::AfterBlobsBeforeHead));
        assert_eq!(
            ledger.commit(
                &head.task_id,
                2,
                &b.guard(),
                vec![event.clone()],
                vec![blob.clone()]
            ),
            Err(LedgerError::Persistence)
        );
        set_fault(None);
        // "Process dies": drop the vault, reopen + unlock a FRESH Vault instance.
        drop(ledger);
        *vault.lock().unwrap() = None;
        drop(vault);
        let fresh = reopen(&root);
        let ledger = TaskLedger::new(fresh);
        let loaded = ledger.load(&head.task_id).unwrap();
        assert_eq!(loaded.seq(), 2, "the head commit point was never reached");
        assert!(!loaded.has_blob(BlobKind::Narrative, &blob.blob_ref().sha256));
        // At most the unreferenced encrypted blob of that commit exists (harmless).
        assert!(ledger.read_blob(&head.task_id, &blob.blob_ref()).is_ok());
        // Re-commit is idempotent on the existing create-only blob.
        let seq = ledger
            .commit(
                &head.task_id,
                2,
                &b.guard(),
                vec![event],
                vec![blob.clone()],
            )
            .unwrap();
        assert_eq!(seq, 3);
        assert!(ledger
            .load(&head.task_id)
            .unwrap()
            .has_blob(BlobKind::Narrative, &blob.blob_ref().sha256));
        assert!(LEDGER_RESIDUALS
            .iter()
            .any(|r| r.starts_with("orphan-blob")));
    }

    #[test]
    fn head_rollback_residual_documented() {
        let (_t, vault, root) = vault_fixture();
        let ledger = TaskLedger::new(vault);
        let b = boot();
        let head = create(&ledger, &b, "m");
        let path = root
            .join("VAULT/records")
            .join(format!("{}.enc.json", head_uuid(&ledger, &head.task_id)));
        let old = std::fs::read(&path).unwrap();
        let (event, blob) = narrative("later");
        ledger
            .commit(&head.task_id, 2, &b.guard(), vec![event], vec![blob])
            .unwrap();
        assert_eq!(ledger.load(&head.task_id).unwrap().seq(), 3);
        // Disk-level rollback to the older sealed head: ACCEPTED (known gap) ...
        std::fs::write(&path, old).unwrap();
        assert_eq!(ledger.load(&head.task_id).unwrap().seq(), 2);
        // ... and reported, not hidden.
        assert!(TaskLedger::residuals()
            .iter()
            .any(|r| r.starts_with("no-anti-rollback")));
    }

    #[test]
    fn recover_post_image_reconciles_without_write() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault);
        let boot_a = boot();
        let head = create(&ledger, &boot_a, "m");
        let (h0, h1) = ("0".repeat(64), "1".repeat(64));
        let seq = ledger
            .commit(
                &head.task_id,
                2,
                &boot_a.guard(),
                vec![host_write(
                    &[("temperature.py", Some(&h0))],
                    &[("temperature.py", Some(&h1))],
                )],
                vec![],
            )
            .unwrap();
        // Crash: no completion. New process, new boot id.
        let boot_b = boot();
        let p = probe(&[("temperature.py", Some(&h1))], true);
        let rec = ledger.recover(&head.task_id, &boot_b, &p).unwrap();
        assert_eq!(p.mutations.get(), 0, "recover performs reads only");
        assert_eq!(p.observations.get(), 1);
        assert!(rec.paused);
        assert_eq!(rec.seq, seq + 1);
        assert!(matches!(
            &rec.recorded[..],
            [(id, 1, ReconcileObservation::MatchesPostImage { .. })] if id == "apply-9"
        ));
        let (h, d) = ledger.load_derived(&head.task_id).unwrap();
        assert!(matches!(
            h.journal.last().unwrap().event,
            LedgerEvent::StepReconciled { .. }
        ));
        assert!(!h
            .journal
            .iter()
            .any(|e| matches!(e.event, LedgerEvent::StepCompleted { .. })));
        assert_eq!(
            d.status(),
            TaskStatus::Paused {
                reason: PauseReason::ReviewRequired
            }
        );
        let step = &d.steps["apply-9"];
        assert_eq!(step.state, StepState::CompletedByObservation);
        assert_eq!(
            step.resolution_options(),
            vec![
                ReviewResolution::ConfirmObservation,
                ReviewResolution::Abandon
            ]
        );
        assert_eq!(d.apply_status(), ApplyStatus::Interrupted);
        // Idempotent: a second recover in the same boot records nothing new.
        let again = ledger.recover(&head.task_id, &boot_b, &p).unwrap();
        assert!(again.recorded.is_empty());
        assert_eq!(again.seq, seq + 1);
        // Resume is refused while unresolved; confirm (UI) then resume unpauses.
        let s = again.seq;
        assert!(ledger
            .commit(
                &head.task_id,
                s,
                &boot_b.guard(),
                vec![admission(0, AdmissionPurpose::Resume)],
                vec![]
            )
            .is_err());
        let s = ledger
            .commit(
                &head.task_id,
                s,
                &boot_b.guard(),
                vec![
                    LedgerEvent::ReconcileResolved {
                        step_id: "apply-9".into(),
                        resolution: ReviewResolution::ConfirmObservation,
                        ui_event_id: "a".repeat(32),
                    },
                    admission(0, AdmissionPurpose::Resume),
                ],
                vec![],
            )
            .unwrap();
        let (_, d) = ledger.load_derived(&head.task_id).unwrap();
        assert!(!d.paused());
        assert!(matches!(d.apply_status(), ApplyStatus::Applied { .. }));
        assert_eq!(s, seq + 3);
        assert_eq!(p.mutations.get(), 0);
    }

    #[test]
    fn recover_unknown_or_mixed_pauses() {
        let (h0, h1, hx) = ("0".repeat(64), "1".repeat(64), "9".repeat(64));
        let cases: Vec<(FakeProbe, &str)> = vec![
            (
                probe(&[("a.py", Some(&h1)), ("b.py", Some(&h0))], true),
                "mixed",
            ),
            (
                probe(&[("a.py", Some(&hx)), ("b.py", None)], true),
                "unknown",
            ),
            (
                probe(&[("a.py", Some(&h1)), ("b.py", Some(&h1))], false),
                "identity",
            ),
            (
                probe(&[("a.py", Some(&h0)), ("b.py", Some(&h0))], true),
                "pre",
            ),
        ];
        for (p, label) in cases {
            let (_t, vault, _root) = vault_fixture();
            let ledger = TaskLedger::new(vault);
            let boot_a = boot();
            let head = create(&ledger, &boot_a, "m");
            ledger
                .commit(
                    &head.task_id,
                    2,
                    &boot_a.guard(),
                    vec![host_write(
                        &[("a.py", Some(&h0)), ("b.py", Some(&h0))],
                        &[("a.py", Some(&h1)), ("b.py", Some(&h1))],
                    )],
                    vec![],
                )
                .unwrap();
            let boot_b = boot();
            let rec = ledger.recover(&head.task_id, &boot_b, &p).unwrap();
            assert_eq!(p.mutations.get(), 0);
            assert!(rec.paused, "{label}");
            let (_, d) = ledger.load_derived(&head.task_id).unwrap();
            let step = &d.steps["apply-9"];
            assert_eq!(step.state, StepState::Interrupted, "{label}");
            match (label, step.observation.as_ref().unwrap()) {
                ("pre", ReconcileObservation::MatchesPreImage) => assert_eq!(
                    step.resolution_options(),
                    vec![
                        ReviewResolution::RetryAsNewAttempt,
                        ReviewResolution::Abandon
                    ]
                ),
                (
                    "identity",
                    ReconcileObservation::Unknown {
                        identity_ok,
                        per_path,
                    },
                ) => {
                    assert!(!identity_ok && per_path.is_empty())
                }
                (_, ReconcileObservation::Unknown { identity_ok, .. }) => {
                    assert!(identity_ok);
                    assert_eq!(
                        step.resolution_options(),
                        vec![
                            ReviewResolution::RestorePreImage,
                            ReviewResolution::MarkManuallyResolved,
                            ReviewResolution::Abandon
                        ]
                    );
                }
                other => panic!("{label}: {other:?}"),
            }
            assert_eq!(
                d.status(),
                TaskStatus::Paused {
                    reason: PauseReason::ReviewRequired
                }
            );
            // Nothing continues while paused.
            assert_eq!(
                ledger.commit(
                    &head.task_id,
                    rec.seq,
                    &boot_b.guard(),
                    vec![
                        admission(0, AdmissionPurpose::Gate),
                        started("gate-x", EffectClass::Pure)
                    ],
                    vec![]
                ),
                Err(LedgerError::IllegalTransition("admission while paused"))
            );
        }
    }

    #[test]
    fn recover_pure_step_interrupted_never_rerun() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault);
        let boot_a = boot();
        let head = create(&ledger, &boot_a, "m");
        let t = &head.task_id;
        let descriptor = ServiceDescriptor {
            service_id: "svc-1".into(),
            task_id: t.to_string(),
            tree_sha256: "e".repeat(64),
            spec_sha256: "e".repeat(64),
            workspace_profile_sha256: "e".repeat(64),
            bridge_port: 41000,
            started_at_ms: 1,
            ready: ReadyState::Ready { after_ms: 3 },
        };
        let seq = ledger
            .commit(
                t,
                2,
                &boot_a.guard(),
                vec![
                    admission(0, AdmissionPurpose::Gate),
                    started("gate-3", EffectClass::Pure),
                    admission(0, AdmissionPurpose::Preview),
                    started("preview-5", EffectClass::ProcessLifecycle),
                    LedgerEvent::PreviewStarted { descriptor },
                ],
                vec![],
            )
            .unwrap();
        let boot_b = boot();
        let p = probe(&[], true);
        let rec = ledger.recover(t, &boot_b, &p).unwrap();
        assert_eq!(
            p.observations.get(),
            0,
            "no worktree probe for pure/process steps"
        );
        assert_eq!(rec.recorded.len(), 2);
        let (h, d) = ledger.load_derived(t).unwrap();
        // Only interruption FACTS were appended: no result recycled, nothing re-run.
        let appended: Vec<_> = h.journal[seq as usize..].iter().map(|e| &e.event).collect();
        assert!(matches!(
            appended[..],
            [
                LedgerEvent::StepInterrupted {
                    observation: ReconcileObservation::NotApplicable,
                    ..
                },
                LedgerEvent::StepInterrupted {
                    observation: ReconcileObservation::ProcessNotOwned,
                    ..
                }
            ]
        ));
        assert_eq!(d.steps["gate-3"].state, StepState::Interrupted);
        assert!(matches!(
            d.preview,
            PreviewLedgerState::NotOwnedAfterRestart { .. }
        ));
        // The old run's completion can never land, from either boot.
        let late = LedgerEvent::StepCompleted {
            step_id: "gate-3".into(),
            attempt: 1,
            result: StepResultRef::Gate {
                gate_run_id: "gate-3".into(),
            },
        };
        assert!(ledger
            .commit(t, rec.seq, &boot_a.guard(), vec![late.clone()], vec![])
            .is_err());
        assert!(ledger
            .commit(t, rec.seq, &boot_b.guard(), vec![late], vec![])
            .is_err());
        // Retry is a NEW attempt after UI resolution + resume.
        let ui = |step: &str, r| LedgerEvent::ReconcileResolved {
            step_id: step.into(),
            resolution: r,
            ui_event_id: "b".repeat(32),
        };
        let s = ledger
            .commit(
                t,
                rec.seq,
                &boot_b.guard(),
                vec![
                    ui("gate-3", ReviewResolution::RetryAsNewAttempt),
                    ui("preview-5", ReviewResolution::Abandon),
                    admission(0, AdmissionPurpose::Resume),
                ],
                vec![],
            )
            .unwrap();
        let retry = |attempt| LedgerEvent::StepStarted {
            step_id: "gate-3".into(),
            attempt,
            intent: intent(EffectClass::Pure),
        };
        assert!(ledger
            .commit(
                t,
                s,
                &boot_b.guard(),
                vec![admission(0, AdmissionPurpose::Gate), retry(1)],
                vec![]
            )
            .is_err());
        ledger
            .commit(
                t,
                s,
                &boot_b.guard(),
                vec![admission(0, AdmissionPurpose::Gate), retry(2)],
                vec![],
            )
            .unwrap();
    }

    #[test]
    fn lock_epoch_discards_late_commit() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault);
        let counter = Arc::new(AtomicU64::new(0));
        let b = BootInfo::new(counter.clone());
        let head = create(&ledger, &b, "m");
        let admitted = b.guard();
        let seq = ledger
            .commit(
                &head.task_id,
                2,
                &admitted,
                vec![
                    admission(0, AdmissionPurpose::Gate),
                    started("gate-3", EffectClass::Pure),
                ],
                vec![],
            )
            .unwrap();
        counter.fetch_add(1, Ordering::SeqCst); // on_lock
        let (event, blob) = narrative("late gate result");
        let late = vec![
            event,
            LedgerEvent::StepCompleted {
                step_id: "gate-3".into(),
                attempt: 1,
                result: StepResultRef::Nothing,
            },
        ];
        assert_eq!(
            ledger.commit(
                &head.task_id,
                seq,
                &admitted,
                late.clone(),
                vec![blob.clone()]
            ),
            Err(LedgerError::Locked)
        );
        let h = ledger.load(&head.task_id).unwrap();
        assert_eq!(h.seq(), seq, "nothing persisted");
        assert!(!h.has_blob(BlobKind::Narrative, &blob.blob_ref().sha256));
        // With a fresh guard the step belongs to an older epoch: not completable.
        assert!(matches!(
            ledger.commit(&head.task_id, seq, &b.guard(), late, vec![blob]),
            Err(LedgerError::IllegalTransition(_))
        ));
        // It surfaces as Interrupted on recovery.
        let rec = ledger
            .recover(&head.task_id, &b, &probe(&[], true))
            .unwrap();
        assert!(matches!(
            rec.recorded[..],
            [(_, 1, ReconcileObservation::NotApplicable)]
        ));
    }

    #[test]
    fn ledger_record_cap_and_head_size_limits() {
        assert_eq!(MAX_LEDGER_RECORDS, 2048);
        assert_eq!(MAX_BLOBS_PER_TASK, 128);
        assert_eq!(MAX_TASKS, 64);
        assert_eq!(MAX_RECORD_BYTES, 1024 * 1024);
        assert_eq!(LedgerLimits::PRODUCTION.max_records, MAX_LEDGER_RECORDS);
        let b = boot();
        // Per-task blob cap.
        {
            let (_t, vault, _root) = vault_fixture();
            let ledger = TaskLedger::with_limits(
                vault,
                LedgerLimits {
                    max_blobs_per_task: 6,
                    ..LedgerLimits::PRODUCTION
                },
            );
            let head = create(&ledger, &b, "m"); // 4 file blobs
            let mut seq = head.seq();
            for i in 0..2 {
                let (e, bl) = narrative(&format!("n{i}"));
                seq = ledger
                    .commit(&head.task_id, seq, &b.guard(), vec![e], vec![bl])
                    .unwrap();
            }
            let (e, bl) = narrative("overflow");
            assert_eq!(
                ledger.commit(&head.task_id, seq, &b.guard(), vec![e], vec![bl]),
                Err(LedgerError::LedgerFull)
            );
            assert_eq!(ledger.load(&head.task_id).unwrap().seq(), seq);
            // Oversized blob payload
            let big = NewBlob::new(BlobKind::GateLog, vec![b'x'; MAX_BLOB_RAW_BYTES + 1]);
            let (e, _) = narrative("x");
            assert_eq!(
                ledger.commit(&head.task_id, seq, &b.guard(), vec![e], vec![big]),
                Err(LedgerError::LedgerFull)
            );
        }
        // Global record cap (conservative reservation counter) and task cap.
        {
            let (_t, vault, _root) = vault_fixture();
            let ledger = TaskLedger::with_limits(
                vault,
                LedgerLimits {
                    max_records: 12,
                    max_tasks: 64,
                    ..LedgerLimits::PRODUCTION
                },
            );
            create(&ledger, &b, "one"); // index + head + 4 blobs = 6
            create(&ledger, &b, "two"); // 11
            let spec = sample_spec("three");
            assert_eq!(
                ledger.create_task(
                    &b.guard(),
                    spec.clone(),
                    oracle_for(&spec),
                    vec![],
                    &sample_files("three"),
                    &snapshot()
                ),
                Err(LedgerError::LedgerFull)
            );
            assert_eq!(ledger.list().unwrap().len(), 2);
            let (_t2, vault2, _root2) = vault_fixture();
            let ledger2 = TaskLedger::with_limits(
                vault2,
                LedgerLimits {
                    max_tasks: 1,
                    ..LedgerLimits::PRODUCTION
                },
            );
            create(&ledger2, &b, "one");
            assert_eq!(
                ledger2.create_task(
                    &b.guard(),
                    spec.clone(),
                    oracle_for(&spec),
                    vec![],
                    &sample_files("x"),
                    &snapshot()
                ),
                Err(LedgerError::LedgerFull)
            );
        }
        // Head size and journal length caps.
        {
            let (_t, vault, _root) = vault_fixture();
            let ledger = TaskLedger::with_limits(
                vault,
                LedgerLimits {
                    max_record_bytes: 16 * 1024,
                    max_journal_entries: 6,
                    ..LedgerLimits::PRODUCTION
                },
            );
            let head = create(&ledger, &b, "m");
            let plan = Plan {
                revision: 1,
                author: PlanAuthor::User,
                steps: (0..40)
                    .map(|i| PlannedStep {
                        step_id: format!("s{i}"),
                        kind: StepKind::Edit,
                        summary: "y".repeat(500),
                        effect: EffectClass::LedgerOnly,
                    })
                    .collect(),
                confirmed: true,
            };
            assert_eq!(
                ledger.commit(
                    &head.task_id,
                    2,
                    &b.guard(),
                    vec![LedgerEvent::PlanRecorded { plan }],
                    vec![]
                ),
                Err(LedgerError::LedgerFull)
            );
            let mut seq = 2;
            let mut full = false;
            for i in 0..8 {
                let (e, bl) = narrative(&format!("j{i}"));
                match ledger.commit(&head.task_id, seq, &b.guard(), vec![e], vec![bl]) {
                    Ok(s) => seq = s,
                    Err(LedgerError::LedgerFull) => {
                        full = true;
                        break;
                    }
                    Err(e) => panic!("{e:?}"),
                }
            }
            assert!(full);
            assert_eq!(seq, 6);
        }
        // Spec bounds
        let mut spec = sample_spec(&"o".repeat(MAX_OBJECTIVE_BYTES + 1));
        assert_eq!(
            validate_spec(&spec),
            Err(LedgerError::Invalid("objective too long"))
        );
        spec.objective = "ok".into();
        spec.repair.max_attempts = 6;
        assert_eq!(
            validate_spec(&spec),
            Err(LedgerError::Invalid("repair budget"))
        );
    }

    fn keys(value: &serde_json::Value, out: &mut BTreeSet<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    out.insert(k.to_ascii_lowercase());
                    keys(v, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| keys(v, out)),
            _ => {}
        }
    }

    #[test]
    fn no_pid_fields_in_schema() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault);
        let b = boot();
        let head = create(&ledger, &b, "m");
        let descriptor = ServiceDescriptor {
            service_id: "svc".into(),
            task_id: head.task_id.to_string(),
            tree_sha256: "e".repeat(64),
            spec_sha256: "e".repeat(64),
            workspace_profile_sha256: "e".repeat(64),
            bridge_port: 1,
            started_at_ms: 1,
            ready: ReadyState::Ready { after_ms: 1 },
        };
        let gate = GateRecord {
            gate_run_id: "gate-3".into(),
            working_set_sha256: "e".repeat(64),
            plan_sha256: "e".repeat(64),
            workspace_profile_sha256: "e".repeat(64),
            commands: vec![CommandSummary {
                id: "build".into(),
                role: GateRole::Build,
                argv: vec!["/usr/bin/python3".into()],
                status: Some(0),
                termination: Termination::Completed,
                stdout_total_bytes: 0,
                stderr_total_bytes: 0,
                stdout_retained_bytes: 0,
                stderr_retained_bytes: 0,
                truncated: false,
                log_sha256: "e".repeat(64),
                excerpt: String::new(),
            }],
            logs: vec![],
            termination: Termination::Completed,
            elapsed_ms: 1,
            at_ms: 1,
        };
        let seq = ledger
            .commit(
                &head.task_id,
                2,
                &b.guard(),
                vec![
                    admission(0, AdmissionPurpose::Gate),
                    started("gate-3", EffectClass::Pure),
                    LedgerEvent::GateRecorded { record: gate },
                    LedgerEvent::StepCompleted {
                        step_id: "gate-3".into(),
                        attempt: 1,
                        result: StepResultRef::Gate {
                            gate_run_id: "gate-3".into(),
                        },
                    },
                    admission(0, AdmissionPurpose::Preview),
                    started("preview-7", EffectClass::ProcessLifecycle),
                    LedgerEvent::PreviewStarted { descriptor },
                    host_write(&[("a.py", None)], &[("a.py", Some(&"1".repeat(64)))]),
                ],
                vec![],
            )
            .unwrap();
        ledger
            .recover(&head.task_id, &boot(), &probe(&[("a.py", None)], true))
            .unwrap();
        let h = ledger.load(&head.task_id).unwrap();
        assert!(h.seq() > seq);
        let mut all = BTreeSet::new();
        keys(&serde_json::to_value(&h).unwrap(), &mut all);
        keys(
            &serde_json::to_value(ledger.list().unwrap()).unwrap(),
            &mut all,
        );
        assert!(all.contains("boot_id") && all.contains("epoch") && all.contains("service_id"));
        for k in &all {
            assert!(
                k != "pid"
                    && !k.ends_with("_pid")
                    && !k.starts_with("pid_")
                    && !k.contains("process_id"),
                "process identifier field persisted: {k}"
            );
        }
        // Source-level guard over every persisted schema definition: this
        // module's non-test part, plus (Pass 2) the exact definitions of every
        // owner-B/C type the ledger serializes, extracted from their owner
        // files (those files legitimately own processes elsewhere, e.g. the
        // service owner thread in isolation::workspace, so only the persisted
        // item bodies are scanned). A process handle or identifier type must
        // not appear in any of them.
        let source = include_str!("task_ledger.rs");
        let schema = &source[..source.find("// Test support shared").unwrap()];
        let item = |file: &'static str, text: &'static str, name: &str| -> String {
            let start = ["pub struct ", "pub enum "]
                .iter()
                .find_map(|kw| {
                    let needle = format!("{kw}{name} ");
                    text.find(&needle)
                        .or_else(|| text.find(&format!("{kw}{name}(")))
                })
                .unwrap_or_else(|| panic!("{file}: persisted type {name} not found"));
            let open = start + text[start..].find(['{', ';']).unwrap();
            if text.as_bytes()[open] == b';' {
                return text[start..=open].to_owned();
            }
            let mut depth = 0usize;
            for (i, c) in text[open..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            return text[start..=open + i].to_owned();
                        }
                    }
                    _ => {}
                }
            }
            panic!("{file}: unbalanced {name}")
        };
        let workspace = include_str!("isolation/workspace.rs");
        let preview = include_str!("task_preview.rs");
        let coding = include_str!("coding_task.rs");
        let persisted: Vec<(&str, &'static str, &[&str])> = vec![
            (
                "isolation::workspace",
                workspace,
                &[
                    "GatePlan",
                    "GateCommand",
                    "GateRole",
                    "CopyOutRequest",
                    "CopyOutMode",
                    "WorkspaceLimits",
                    "ServiceSpec",
                    "ServiceCommand",
                    "StopReport",
                    "Stream",
                ],
            ),
            (
                "task_preview",
                preview,
                &[
                    "PreviewSpec",
                    "HttpCheck",
                    "HttpMethod",
                    "ServiceDescriptor",
                    "ReadyState",
                    "StartupFailure",
                    "StopReasonKind",
                    "HttpCheckRecord",
                    "HttpCheckResult",
                    "EvidenceLevel",
                ],
            ),
            (
                "task_workspace",
                include_str!("task_workspace.rs"),
                &["WorkingSetManifest", "EditDenial", "OracleReason"],
            ),
            (
                "task_diff",
                include_str!("task_diff.rs"),
                &["ReviewState", "FileReview", "FileDecision"],
            ),
            (
                "task_worktree",
                include_str!("task_worktree.rs"),
                &["WorktreeBinding"],
            ),
            (
                "coding_task::ports",
                coding,
                &["StopRecord", "RepairBudget"],
            ),
        ];
        let mut texts: Vec<(String, String)> = vec![("task_ledger".into(), schema.to_owned())];
        for (file, text, names) in persisted {
            for name in names {
                texts.push((format!("{file}::{name}"), item(file, text, name)));
            }
        }
        for (name, text) in texts.iter().map(|(n, t)| (n.as_str(), t.as_str())) {
            for needle in [
                "pub pid",
                " pid:",
                "pid_t",
                "process_id",
                "getpid",
                "process::id",
                concat!("std::", "process"),
                "Child",
            ] {
                assert!(!text.contains(needle), "{name}: {needle}");
            }
        }
    }

    // -----------------------------------------------------------------
    // Stage 5 review R3 (C6) defence-in-depth regressions: G1 (L5), G2
    // (L6), G3 (L7), ported from probes/q2_ledger.rs. Each shape is refused
    // at COMMIT and, re-sealed with the derived ledger key (keyholder; the
    // MAC is valid), at LOAD, so the load refusal can only be the replay.
    // -----------------------------------------------------------------

    fn ws_with(ws: &WorkingSetManifest, path: &str, sha: &str) -> WorkingSetManifest {
        let mut next = ws.clone();
        next.current.insert(path.into(), sha.into());
        next.current_sha256 = manifest_sha256(&next.current);
        next
    }
    fn cp_event(ws: &WorkingSetManifest, review: ReviewState) -> LedgerEvent {
        LedgerEvent::Checkpoint {
            checkpoint: Checkpoint {
                seq: 0,
                working_set: ws.clone(),
                review,
                last_gate: None,
                repair_attempts_used: 0,
                preview: PreviewLedgerState::NotStarted,
            },
        }
    }
    fn edit_event(ws: &WorkingSetManifest, origin: EditOrigin, paths: &[&str]) -> LedgerEvent {
        LedgerEvent::EditApplied {
            origin,
            manifest_sha256: ws.current_sha256.clone(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
        }
    }
    fn reviewed_event(
        path: &str,
        decision: FileDecision,
        new: Option<&str>,
        id: char,
    ) -> LedgerEvent {
        LedgerEvent::FileReviewed {
            path: path.into(),
            decision,
            reviewed_new_sha256: new.map(str::to_owned),
            ui_event_id: id.to_string().repeat(32),
        }
    }
    fn review_one(
        path: &str,
        decision: FileDecision,
        base: Option<&String>,
        new: Option<&str>,
        id: char,
    ) -> ReviewState {
        ReviewState {
            files: BTreeMap::from([(
                path.to_owned(),
                FileReview {
                    decision,
                    reviewed_base_sha256: base.cloned(),
                    reviewed_new_sha256: new.map(str::to_owned),
                    ui_event_id: id.to_string().repeat(32),
                },
            )]),
        }
    }
    fn is_ill(r: &Result<u64>) -> bool {
        matches!(r, Err(LedgerError::IllegalTransition(_)))
    }
    /// Keyholder forgery: appends `events` to a copy of `head` stamped exactly
    /// like `append_entries` stamps a commit (chained seq/prev, one boot/epoch/
    /// at_ms, checkpoint bookkeeping from the replay) WITHOUT validating them.
    /// The entries continue the head's last commit (same guard).
    fn forge(
        head: &TaskHead,
        guard: &EpochGuard,
        events: Vec<LedgerEvent>,
        blobs: &[NewBlob],
    ) -> TaskHead {
        let mut h = head.clone();
        let d = replay(&h).unwrap();
        for blob in blobs {
            let r = blob.blob_ref();
            if !h.blobs.contains(&r) {
                h.blobs.push(r);
            }
        }
        let at_ms = h.journal.last().unwrap().at_ms;
        for mut event in events {
            if let LedgerEvent::Checkpoint { checkpoint } = &mut event {
                checkpoint.seq = h.seq() + 1;
                checkpoint.last_gate = d.gates.last().map(|g| g.gate_run_id.clone());
                checkpoint.repair_attempts_used = d.repair_attempts_used;
                checkpoint.preview = d.preview.clone();
            }
            let prev_sha256 = sha_json(h.journal.last().unwrap()).unwrap();
            h.journal.push(JournalEntry {
                seq: h.seq() + 1,
                prev_sha256,
                at_ms,
                boot_id: guard.boot_id().to_owned(),
                epoch: guard.epoch(),
                event,
            });
        }
        h
    }
    /// Re-seal `head` with the DERIVED ledger key over the task's head record
    /// and load it through the verified path (MAC + chain + replay).
    fn reseal_and_load(
        ledger: &TaskLedger,
        vault: &Arc<Mutex<Option<Vault>>>,
        head: &TaskHead,
    ) -> Result<u64> {
        let uuid = head_uuid(ledger, &head.task_id);
        {
            let mut g = vault.lock().unwrap();
            let v = g.as_mut().unwrap();
            let bytes = seal_envelope(v, "head", head.task_id.as_str(), head.seq(), head).unwrap();
            let (meta, _) = v.read_record(&uuid).unwrap();
            v.write_record(meta, &bytes).unwrap();
        }
        ledger.load(&head.task_id).map(|h| h.seq())
    }

    /// Commits every case on top of `genuine`; returns the cases NOT refused
    /// with IllegalTransition. A case that got persisted (pre-fix behaviour) is
    /// undone by restoring the genuine head so the next case starts clean.
    fn accepted_at_commit(
        ledger: &TaskLedger,
        vault: &Arc<Mutex<Option<Vault>>>,
        genuine: &TaskHead,
        guard: &EpochGuard,
        cases: &[(&str, Vec<LedgerEvent>)],
        blobs: &[NewBlob],
    ) -> Vec<String> {
        let mut out = vec![];
        for (label, events) in cases {
            let t = &genuine.task_id;
            let r = ledger.commit(t, genuine.seq(), guard, events.clone(), blobs.to_vec());
            if !is_ill(&r) {
                out.push(format!("{label}: {r:?}"));
            }
            if ledger.load(t).map(|h| h.seq()) != Ok(genuine.seq()) {
                out.push(format!("{label}: persisted"));
                assert_eq!(reseal_and_load(ledger, vault, genuine), Ok(genuine.seq()));
            }
        }
        out
    }
    /// Appends every case to `genuine` (keyholder re-seal) and loads it; returns
    /// the cases NOT refused as Corrupt. Restores the genuine head afterwards.
    fn accepted_at_load(
        ledger: &TaskLedger,
        vault: &Arc<Mutex<Option<Vault>>>,
        genuine: &TaskHead,
        guard: &EpochGuard,
        cases: &[(&str, Vec<LedgerEvent>)],
        blobs: &[NewBlob],
    ) -> Vec<String> {
        let mut out = vec![];
        for (label, events) in cases {
            let forged = forge(genuine, guard, events.clone(), blobs);
            let r = reseal_and_load(ledger, vault, &forged);
            if r != Err(LedgerError::Corrupt) {
                out.push(format!("{label}: {r:?}"));
            }
        }
        assert_eq!(reseal_and_load(ledger, vault, genuine), Ok(genuine.seq()));
        out
    }
    fn none() -> Vec<String> {
        Vec::new()
    }

    /// R3 L5 / G1: a checkpoint that changes content needs its EditApplied
    /// (covering the changed path) in the same commit, and a decision on the
    /// changed file resets to Pending unless a FileReviewed for exactly the new
    /// hash is in the same commit.
    #[test]
    fn r3_l5_content_change_needs_edit_event_and_fresh_review() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault.clone());
        let b = boot();
        let head = create(&ledger, &b, "m");
        let t = &head.task_id;
        let path = "temperature.py";
        let ws0 = ledger
            .load_derived(t)
            .unwrap()
            .1
            .working_set()
            .unwrap()
            .clone();
        let base = ws0.base.get(path);
        let x = b"def to_kelvin(c):\n    return c + 273.15\n".to_vec();
        let xs = kv::digest(&x);
        let ws_x = ws_with(&ws0, path, &xs);
        let seq = ledger
            .commit(
                t,
                2,
                &b.guard(),
                vec![
                    edit_event(&ws_x, EditOrigin::Model, &[path]),
                    cp_event(&ws_x, ReviewState::default()),
                ],
                vec![NewBlob::new(BlobKind::FileContent, x)],
            )
            .unwrap();
        let accepted_x = review_one(path, FileDecision::Accepted, base, Some(&xs), 'f');
        let seq = ledger
            .commit(
                t,
                seq,
                &b.guard(),
                vec![
                    reviewed_event(path, FileDecision::Accepted, Some(&xs), 'f'),
                    cp_event(&ws_x, accepted_x.clone()),
                ],
                vec![],
            )
            .unwrap();
        // New content Y; the Accepted entry re-bound to Y (the R3 L5 shape) and
        // the controller's reset shape (decision Pending, old hash = stale marker).
        let y = b"def to_kelvin(c):\n    import os; os._exit(0)\n".to_vec();
        let ys = kv::digest(&y);
        let ws_y = ws_with(&ws0, path, &ys);
        let y_blob = NewBlob::new(BlobKind::FileContent, y);
        let accepted_y = review_one(path, FileDecision::Accepted, base, Some(&ys), 'f');
        let pending_y = review_one(path, FileDecision::Pending, base, Some(&xs), 'f');
        let edit_y = edit_event(&ws_y, EditOrigin::Model, &[path]);
        let illegal: Vec<(&str, Vec<LedgerEvent>)> = vec![
            (
                "lone checkpoint, Accepted re-bound to the new hash (R3 L5)",
                vec![cp_event(&ws_y, accepted_y.clone())],
            ),
            (
                "lone checkpoint, decision reset: still no content-changing event",
                vec![cp_event(&ws_y, pending_y.clone())],
            ),
            (
                "EditApplied present, Accepted re-bound without FileReviewed",
                vec![edit_y.clone(), cp_event(&ws_y, accepted_y.clone())],
            ),
            (
                "EditApplied names another path than the changed one",
                vec![
                    edit_event(&ws_y, EditOrigin::Model, &["duration.py"]),
                    cp_event(&ws_y, pending_y.clone()),
                ],
            ),
            (
                "FileReviewed for the OLD hash carried to the new content",
                vec![
                    edit_y.clone(),
                    reviewed_event(path, FileDecision::Accepted, Some(&xs), 'f'),
                    cp_event(&ws_y, accepted_y.clone()),
                ],
            ),
        ];
        let genuine = ledger.load(t).unwrap();
        let blobs = [y_blob.clone()];
        let at_commit = accepted_at_commit(&ledger, &vault, &genuine, &b.guard(), &illegal, &blobs);
        let at_load = accepted_at_load(&ledger, &vault, &genuine, &b.guard(), &illegal, &blobs);
        assert_eq!((at_commit, at_load), (none(), none()));
        // Control: the controller's shape (EditApplied + reset to Pending) re-sealed loads.
        let forged = forge(
            &genuine,
            &b.guard(),
            vec![edit_y.clone(), cp_event(&ws_y, pending_y.clone())],
            std::slice::from_ref(&y_blob),
        );
        assert_eq!(reseal_and_load(&ledger, &vault, &forged), Ok(seq + 2));
        assert_eq!(reseal_and_load(&ledger, &vault, &genuine), Ok(seq));
        // Legal at commit: the reset shape, then a FileReviewed for EXACTLY the new
        // hash in the same commit as a further content change.
        let s = ledger
            .commit(
                t,
                seq,
                &b.guard(),
                vec![edit_y, cp_event(&ws_y, pending_y)],
                vec![y_blob],
            )
            .unwrap();
        let z = b"def to_kelvin(c):\n    return c + 273.15  # z\n".to_vec();
        let zs = kv::digest(&z);
        let ws_z = ws_with(&ws0, path, &zs);
        let s = ledger
            .commit(
                t,
                s,
                &b.guard(),
                vec![
                    edit_event(&ws_z, EditOrigin::User, &[path]),
                    reviewed_event(path, FileDecision::Accepted, Some(&zs), 'a'),
                    cp_event(
                        &ws_z,
                        review_one(path, FileDecision::Accepted, base, Some(&zs), 'a'),
                    ),
                ],
                vec![NewBlob::new(BlobKind::FileContent, z)],
            )
            .unwrap();
        let (h, d) = ledger.load_derived(t).unwrap();
        assert_eq!(h.seq(), s);
        assert_eq!(d.review().files[path].decision, FileDecision::Accepted);
        assert_eq!(d.working_set().unwrap().current[path], zs);
    }

    /// R3 L5 / G1 (Rejected): the controller keeps a Rejected decision only
    /// where `reset_reviews` keeps it (content back at base, or exactly the
    /// rejected bytes again); any other content change resets it to Pending.
    #[test]
    fn r3_l5_rejected_decision_on_changed_content_resets() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault.clone());
        let b = boot();
        let head = create(&ledger, &b, "m");
        let t = &head.task_id;
        let path = "temperature.py";
        let ws0 = ledger
            .load_derived(t)
            .unwrap()
            .1
            .working_set()
            .unwrap()
            .clone();
        let base = ws0.base.get(path);
        let blob = |s: &str| NewBlob::new(BlobKind::FileContent, s.as_bytes().to_vec());
        let (x, z, dur) = (
            blob("def to_kelvin(c):\n    return c + 273.15\n"),
            blob("def to_kelvin(c):\n    return c + 274\n"),
            blob("def window(s, n):\n    return []\n"),
        );
        let (xs, zs, ds) = (
            x.blob_ref().sha256,
            z.blob_ref().sha256,
            dur.blob_ref().sha256,
        );
        let ws_x = ws_with(&ws0, path, &xs);
        let seq = ledger
            .commit(
                t,
                2,
                &b.guard(),
                vec![
                    edit_event(&ws_x, EditOrigin::Model, &[path]),
                    cp_event(&ws_x, ReviewState::default()),
                ],
                vec![x.clone()],
            )
            .unwrap();
        // revert_file shape: EditApplied{User} + FileReviewed{Rejected, displayed
        // hash} + checkpoint with the content back at base.
        let rejected = review_one(path, FileDecision::Rejected, base, Some(&xs), 'c');
        let seq = ledger
            .commit(
                t,
                seq,
                &b.guard(),
                vec![
                    edit_event(&ws0, EditOrigin::User, &[path]),
                    reviewed_event(path, FileDecision::Rejected, Some(&xs), 'c'),
                    cp_event(&ws0, rejected.clone()),
                ],
                vec![],
            )
            .unwrap();
        // Model edit of ANOTHER file: Rejected kept (content == base).
        let ws_d = ws_with(&ws0, "duration.py", &ds);
        let seq = ledger
            .commit(
                t,
                seq,
                &b.guard(),
                vec![
                    edit_event(&ws_d, EditOrigin::Model, &["duration.py"]),
                    cp_event(&ws_d, rejected.clone()),
                ],
                vec![dur],
            )
            .unwrap();
        // ABA: the model writes exactly the rejected bytes again; reset_reviews
        // keeps Rejected (reviewed hash == current) -> legal.
        let ws_dx = ws_with(&ws_d, path, &xs);
        let seq = ledger
            .commit(
                t,
                seq,
                &b.guard(),
                vec![
                    edit_event(&ws_dx, EditOrigin::Model, &[path]),
                    cp_event(&ws_dx, rejected.clone()),
                ],
                vec![],
            )
            .unwrap();
        // New content Z with the Rejected decision kept: refused (commit + load).
        let ws_dz = ws_with(&ws_d, path, &zs);
        let keep = vec![
            edit_event(&ws_dz, EditOrigin::Model, &[path]),
            cp_event(&ws_dz, rejected.clone()),
        ];
        let cases = [("Rejected kept on new content", keep)];
        let genuine = ledger.load(t).unwrap();
        let blobs = [z.clone()];
        let at_commit = accepted_at_commit(&ledger, &vault, &genuine, &b.guard(), &cases, &blobs);
        let at_load = accepted_at_load(&ledger, &vault, &genuine, &b.guard(), &cases, &blobs);
        assert_eq!((at_commit, at_load), (none(), none()));
        // ... and legal once reset to Pending (the controller's shape).
        let pending = review_one(path, FileDecision::Pending, base, Some(&xs), 'c');
        let s = ledger
            .commit(
                t,
                seq,
                &b.guard(),
                vec![
                    edit_event(&ws_dz, EditOrigin::Model, &[path]),
                    cp_event(&ws_dz, pending),
                ],
                vec![z],
            )
            .unwrap();
        assert_eq!(ledger.load(t).unwrap().seq(), s);
    }

    /// R3 L6 / G2: a WorktreeStateUnknown observation must be the one a probe
    /// of the intent can produce: MatchesPostImage carries EXACTLY the intent's
    /// post-image, MatchesPreImage needs a pre-image, an identity failure
    /// carries no hashes.
    #[test]
    fn r3_l6_worktree_unknown_observation_must_match_intent_images() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault.clone());
        let b = boot();
        let head = create(&ledger, &b, "m");
        let t = &head.task_id;
        let (h0, h1, h9) = ("0".repeat(64), "1".repeat(64), "9".repeat(64));
        let p = "temperature.py";
        let start = host_write(&[(p, Some(&h0))], &[(p, Some(&h1))]);
        let failed = |observation| LedgerEvent::StepFailed {
            step_id: "apply-9".into(),
            attempt: 1,
            reason: FailureReason::WorktreeStateUnknown {
                error: "io".into(),
                observation,
            },
            evidence: vec![],
        };
        let hashes = |pairs: &[(&str, Option<&str>)]| -> BTreeMap<String, Option<String>> {
            pairs
                .iter()
                .map(|(p, h)| (p.to_string(), h.map(str::to_owned)))
                .collect()
        };
        let post = |pairs: &[(&str, Option<&str>)]| ReconcileObservation::MatchesPostImage {
            hashes: hashes(pairs),
        };
        let bare = LedgerEvent::StepStarted {
            step_id: "apply-9".into(),
            attempt: 1,
            intent: StepIntent {
                worktree: Some(binding()),
                ..intent(EffectClass::HostWrite)
            },
        };
        let illegal: Vec<(&str, Vec<LedgerEvent>)> = vec![
            (
                "MatchesPostImage{} (R3 L6)",
                vec![start.clone(), failed(post(&[]))],
            ),
            (
                "MatchesPostImage with the pre-image hash",
                vec![start.clone(), failed(post(&[(p, Some(&h0))]))],
            ),
            (
                "MatchesPostImage with another hash",
                vec![start.clone(), failed(post(&[(p, Some(&h9))]))],
            ),
            (
                "MatchesPostImage with an extra path",
                vec![
                    start.clone(),
                    failed(post(&[(p, Some(&h1)), ("duration.py", None)])),
                ],
            ),
            (
                "MatchesPostImage claiming absent",
                vec![start.clone(), failed(post(&[(p, None)]))],
            ),
            (
                "MatchesPreImage without any recorded pre-image",
                vec![bare.clone(), failed(ReconcileObservation::MatchesPreImage)],
            ),
            (
                "identity failure carrying observed hashes",
                vec![
                    start.clone(),
                    failed(ReconcileObservation::Unknown {
                        per_path: hashes(&[(p, Some(&h1))]),
                        identity_ok: false,
                    }),
                ],
            ),
        ];
        let genuine = ledger.load(t).unwrap();
        let at_commit = accepted_at_commit(&ledger, &vault, &genuine, &b.guard(), &illegal, &[]);
        let at_load = accepted_at_load(&ledger, &vault, &genuine, &b.guard(), &illegal, &[]);
        assert_eq!((at_commit, at_load), (none(), none()));
        // Control: the observation the controller records (exact post-image).
        let exact = vec![start.clone(), failed(post(&[(p, Some(&h1))]))];
        let forged = forge(&genuine, &b.guard(), exact.clone(), &[]);
        assert_eq!(reseal_and_load(&ledger, &vault, &forged), Ok(4));
        assert_eq!(reseal_and_load(&ledger, &vault, &genuine), Ok(2));
        // Legal at commit: exact post-image -> CompletedByObservation -> (UI) Applied.
        let s = ledger.commit(t, 2, &b.guard(), exact, vec![]).unwrap();
        let (_, d) = ledger.load_derived(t).unwrap();
        assert_eq!(d.steps["apply-9"].state, StepState::CompletedByObservation);
        assert_eq!(d.apply_status(), ApplyStatus::Interrupted);
        ledger
            .commit(
                t,
                s,
                &b.guard(),
                vec![LedgerEvent::ReconcileResolved {
                    step_id: "apply-9".into(),
                    resolution: ReviewResolution::ConfirmObservation,
                    ui_event_id: "d".repeat(32),
                }],
                vec![],
            )
            .unwrap();
        let (_, d) = ledger.load_derived(t).unwrap();
        assert!(matches!(d.apply_status(), ApplyStatus::Applied { .. }));
        // The other observations a real probe yields stay legal.
        for observation in [
            ReconcileObservation::MatchesPreImage,
            ReconcileObservation::Unknown {
                per_path: hashes(&[(p, Some(&h9))]),
                identity_ok: true,
            },
            ReconcileObservation::Unknown {
                per_path: BTreeMap::new(),
                identity_ok: false,
            },
        ] {
            let other = create(&ledger, &b, "o");
            let s = ledger
                .commit(
                    &other.task_id,
                    2,
                    &b.guard(),
                    vec![start.clone(), failed(observation)],
                    vec![],
                )
                .unwrap();
            let (_, d) = ledger.load_derived(&other.task_id).unwrap();
            assert_eq!(s, 4);
            assert_eq!(d.steps["apply-9"].state, StepState::Interrupted);
        }
    }

    /// R3 L7 / G3: every FileReviewed binds exactly the checkpoint's entry for
    /// that path (decision, reviewed new hash, UI event id).
    #[test]
    fn r3_l7_file_reviewed_binds_checkpoint_entry() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault.clone());
        let b = boot();
        let head = create(&ledger, &b, "m");
        let t = &head.task_id;
        let path = "temperature.py";
        let ws0 = ledger
            .load_derived(t)
            .unwrap()
            .1
            .working_set()
            .unwrap()
            .clone();
        let x = b"def to_kelvin(c):\n    return c + 273.15\n".to_vec();
        let xs = kv::digest(&x);
        let ws_x = ws_with(&ws0, path, &xs);
        let seq = ledger
            .commit(
                t,
                2,
                &b.guard(),
                vec![
                    edit_event(&ws_x, EditOrigin::Model, &[path]),
                    cp_event(&ws_x, ReviewState::default()),
                ],
                vec![NewBlob::new(BlobKind::FileContent, x)],
            )
            .unwrap();
        let acc = review_one(
            path,
            FileDecision::Accepted,
            ws0.base.get(path),
            Some(&xs),
            'f',
        );
        let zero = "0".repeat(64);
        let illegal: Vec<(&str, Vec<LedgerEvent>)> = vec![
            (
                "reviewed hash differs from the binding (R3 L7)",
                vec![
                    reviewed_event(path, FileDecision::Accepted, Some(&zero), 'f'),
                    cp_event(&ws_x, acc.clone()),
                ],
            ),
            (
                "reviewed hash missing",
                vec![
                    reviewed_event(path, FileDecision::Accepted, None, 'f'),
                    cp_event(&ws_x, acc.clone()),
                ],
            ),
            (
                "UI event id differs from the checkpoint entry",
                vec![
                    reviewed_event(path, FileDecision::Accepted, Some(&xs), 'e'),
                    cp_event(&ws_x, acc.clone()),
                ],
            ),
            (
                "second FileReviewed for the path with another hash",
                vec![
                    reviewed_event(path, FileDecision::Accepted, Some(&xs), 'f'),
                    reviewed_event(path, FileDecision::Accepted, Some(&zero), 'f'),
                    cp_event(&ws_x, acc.clone()),
                ],
            ),
        ];
        let genuine = ledger.load(t).unwrap();
        let at_commit = accepted_at_commit(&ledger, &vault, &genuine, &b.guard(), &illegal, &[]);
        let at_load = accepted_at_load(&ledger, &vault, &genuine, &b.guard(), &illegal, &[]);
        assert_eq!((at_commit, at_load), (none(), none()));
        let legal = vec![
            reviewed_event(path, FileDecision::Accepted, Some(&xs), 'f'),
            cp_event(&ws_x, acc),
        ];
        let forged = forge(&genuine, &b.guard(), legal.clone(), &[]);
        assert_eq!(reseal_and_load(&ledger, &vault, &forged), Ok(seq + 2));
        assert_eq!(reseal_and_load(&ledger, &vault, &genuine), Ok(seq));
        let s = ledger.commit(t, seq, &b.guard(), legal, vec![]).unwrap();
        let (h, d) = ledger.load_derived(t).unwrap();
        assert_eq!(h.seq(), s);
        assert_eq!(d.review().files[path].decision, FileDecision::Accepted);
    }

    /// Like `forge`, but every group is its OWN commit with its own guard, the
    /// boundary recorded exactly as `append_entries` records it. Checkpoint
    /// bookkeeping comes from the replay of `head` (the groups add no gate,
    /// repair or preview fact).
    fn forge_commits(
        head: &TaskHead,
        commits: Vec<(&EpochGuard, Vec<LedgerEvent>)>,
        blobs: &[NewBlob],
    ) -> TaskHead {
        let mut h = head.clone();
        let d = replay(head).unwrap();
        for blob in blobs {
            let r = blob.blob_ref();
            if !h.blobs.contains(&r) {
                h.blobs.push(r);
            }
        }
        for (n, (guard, events)) in commits.into_iter().enumerate() {
            h.commit_starts.push(h.seq() + 1);
            let at_ms = h.journal.last().unwrap().at_ms + 1 + n as u64;
            for mut event in events {
                if let LedgerEvent::Checkpoint { checkpoint } = &mut event {
                    checkpoint.seq = h.seq() + 1;
                    checkpoint.last_gate = d.gates.last().map(|g| g.gate_run_id.clone());
                    checkpoint.repair_attempts_used = d.repair_attempts_used;
                    checkpoint.preview = d.preview.clone();
                }
                let prev_sha256 = sha_json(h.journal.last().unwrap()).unwrap();
                h.journal.push(JournalEntry {
                    seq: h.seq() + 1,
                    prev_sha256,
                    at_ms,
                    boot_id: guard.boot_id().to_owned(),
                    epoch: guard.epoch(),
                    event,
                });
            }
        }
        h
    }

    /// Commit boundaries are persisted and the "same commit" rules hold
    /// EXACTLY at load (R3 C6: they used to degrade to adjacency).
    #[test]
    fn commit_boundaries_persisted_and_enforced_at_load() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault.clone());
        let b = boot();
        let other = boot();
        let head = create(&ledger, &b, "m");
        let t = &head.task_id;
        assert_eq!(head.commit_starts, vec![1]);
        let path = "temperature.py";
        let ws0 = ledger
            .load_derived(t)
            .unwrap()
            .1
            .working_set()
            .unwrap()
            .clone();
        let x = NewBlob::new(
            BlobKind::FileContent,
            b"def to_kelvin(c):\n    return 1\n".to_vec(),
        );
        let xs = x.blob_ref().sha256;
        let ws_x = ws_with(&ws0, path, &xs);
        let seq = ledger
            .commit(
                t,
                2,
                &b.guard(),
                vec![
                    edit_event(&ws_x, EditOrigin::Model, &[path]),
                    cp_event(&ws_x, ReviewState::default()),
                ],
                vec![x.clone()],
            )
            .unwrap();
        let seq = ledger
            .commit(
                t,
                seq,
                &b.guard(),
                vec![
                    admission(0, AdmissionPurpose::Gate),
                    started("gate-1", EffectClass::Pure),
                ],
                vec![],
            )
            .unwrap();
        let genuine = ledger.load(t).unwrap();
        assert_eq!(genuine.commit_starts, vec![1, 3, 5]);
        assert_eq!(seq, 6);
        let (n1, n1_blob) = narrative("one");
        let (n2, n2_blob) = narrative("two");
        let acc = review_one(
            path,
            FileDecision::Accepted,
            ws0.base.get(path),
            Some(&xs),
            'f',
        );
        let review = vec![
            reviewed_event(path, FileDecision::Accepted, Some(&xs), 'f'),
            cp_event(&ws_x, acc.clone()),
        ];
        let completed = LedgerEvent::StepCompleted {
            step_id: "gate-1".into(),
            attempt: 1,
            result: StepResultRef::Nothing,
        };
        let g = b.guard();
        let og = other.guard();
        let split: Vec<(&str, TaskHead)> = vec![
            (
                "admission and start in two commits",
                forge_commits(
                    &genuine,
                    vec![
                        (
                            &g,
                            vec![completed.clone(), admission(0, AdmissionPurpose::Gate)],
                        ),
                        (&g, vec![started("gate-2", EffectClass::Pure)]),
                    ],
                    &[],
                ),
            ),
            (
                "FileReviewed and its checkpoint in two commits",
                forge_commits(
                    &genuine,
                    vec![(&g, review[..1].to_vec()), (&g, review[1..].to_vec())],
                    &[],
                ),
            ),
            (
                "EditApplied and its checkpoint in two commits",
                forge_commits(
                    &genuine,
                    vec![
                        (&g, vec![edit_event(&ws0, EditOrigin::User, &[path])]),
                        (&g, vec![cp_event(&ws0, ReviewState::default())]),
                    ],
                    &[],
                ),
            ),
            (
                "two guards in one commit",
                forge_commits(
                    &genuine,
                    vec![(&g, vec![n1.clone()])],
                    std::slice::from_ref(&n1_blob),
                ),
            ),
        ];
        // The last case: append an entry of ANOTHER guard to that one commit.
        let mut split = split;
        {
            let h = &mut split.last_mut().unwrap().1;
            let prev_sha256 = sha_json(h.journal.last().unwrap()).unwrap();
            let at_ms = h.journal.last().unwrap().at_ms;
            h.blobs.push(n2_blob.blob_ref());
            h.journal.push(JournalEntry {
                seq: h.seq() + 1,
                prev_sha256,
                at_ms,
                boot_id: og.boot_id().to_owned(),
                epoch: og.epoch(),
                event: n2.clone(),
            });
        }
        let mut malformed = Vec::new();
        for starts in [
            vec![0, 1, 3, 5],
            vec![1, 3, 3, 5],
            vec![1, 5, 3],
            vec![1, 3, 5, 7],
        ] {
            let mut h = genuine.clone();
            h.commit_starts = starts.clone();
            malformed.push(h);
        }
        for (label, h) in &split {
            assert_eq!(
                reseal_and_load(&ledger, &vault, h),
                Err(LedgerError::Corrupt),
                "load accepted: {label}"
            );
        }
        for h in &malformed {
            assert_eq!(
                reseal_and_load(&ledger, &vault, h),
                Err(LedgerError::Corrupt),
                "load accepted: {:?}",
                h.commit_starts
            );
        }
        // Controls: the same entries as ONE commit each (and separate guards in
        // separate commits) load.
        let ok = [
            forge_commits(
                &genuine,
                vec![(
                    &g,
                    vec![
                        completed.clone(),
                        admission(0, AdmissionPurpose::Gate),
                        started("gate-2", EffectClass::Pure),
                    ],
                )],
                &[],
            ),
            forge_commits(&genuine, vec![(&g, review.clone())], &[]),
            forge_commits(
                &genuine,
                vec![(&g, vec![n1.clone()]), (&og, vec![n2.clone()])],
                &[n1_blob.clone(), n2_blob.clone()],
            ),
        ];
        for h in &ok {
            assert_eq!(reseal_and_load(&ledger, &vault, h), Ok(h.seq()));
        }
        // Legacy heads (no recorded boundaries) still load with the adjacency
        // rules, and record boundaries from their next commit on.
        let mut legacy = split[1].1.clone();
        legacy.commit_starts.clear();
        assert_eq!(reseal_and_load(&ledger, &vault, &legacy), Ok(legacy.seq()));
        let mut legacy = genuine.clone();
        legacy.commit_starts.clear();
        assert_eq!(reseal_and_load(&ledger, &vault, &legacy), Ok(seq));
        let s = ledger
            .commit(t, seq, &b.guard(), vec![completed.clone()], vec![])
            .unwrap();
        let upgraded = ledger.load(t).unwrap();
        assert_eq!(upgraded.commit_starts, vec![seq + 1]);
        assert_eq!(upgraded.seq(), s);
        // ... and their recorded part is exact again.
        let forged = forge_commits(
            &upgraded,
            vec![
                (&g, vec![admission(0, AdmissionPurpose::Gate)]),
                (&g, vec![started("gate-2", EffectClass::Pure)]),
            ],
            &[],
        );
        assert_eq!(
            reseal_and_load(&ledger, &vault, &forged),
            Err(LedgerError::Corrupt)
        );
        assert_eq!(reseal_and_load(&ledger, &vault, &upgraded), Ok(s));
    }

    /// R3 L6 / G2 for the recover() records: a StepReconciled/StepInterrupted
    /// of a HostWrite step must carry an observation consistent with the
    /// intent, also in a re-sealed head (keyholder).
    #[test]
    fn recover_observation_must_match_intent_at_load() {
        let (_t, vault, _root) = vault_fixture();
        let ledger = TaskLedger::new(vault.clone());
        let boot_a = boot();
        let boot_b = boot();
        let head = create(&ledger, &boot_a, "m");
        let t = &head.task_id;
        let (h0, h1, h9) = ("0".repeat(64), "1".repeat(64), "9".repeat(64));
        let p = "temperature.py";
        ledger
            .commit(
                t,
                2,
                &boot_a.guard(),
                vec![host_write(&[(p, Some(&h0))], &[(p, Some(&h1))])],
                vec![],
            )
            .unwrap();
        let genuine = ledger.load(t).unwrap();
        let rec = |reconciled: bool, observation| {
            if reconciled {
                LedgerEvent::StepReconciled {
                    step_id: "apply-9".into(),
                    attempt: 1,
                    observation,
                }
            } else {
                LedgerEvent::StepInterrupted {
                    step_id: "apply-9".into(),
                    attempt: 1,
                    observation,
                }
            }
        };
        let post = |h: Option<&str>| ReconcileObservation::MatchesPostImage {
            hashes: BTreeMap::from([(p.to_owned(), h.map(str::to_owned))]),
        };
        let g = boot_b.guard();
        for (label, event) in [
            (
                "reconciled, empty hashes",
                rec(
                    true,
                    ReconcileObservation::MatchesPostImage {
                        hashes: BTreeMap::new(),
                    },
                ),
            ),
            ("reconciled, pre-image hash", rec(true, post(Some(&h0)))),
            ("reconciled, other hash", rec(true, post(Some(&h9)))),
            (
                "interrupted, identity failure with hashes",
                rec(
                    false,
                    ReconcileObservation::Unknown {
                        per_path: BTreeMap::from([(p.to_owned(), Some(h9.clone()))]),
                        identity_ok: false,
                    },
                ),
            ),
            (
                "interrupted, observation of other paths",
                rec(
                    false,
                    ReconcileObservation::Unknown {
                        per_path: BTreeMap::from([("duration.py".to_owned(), Some(h9.clone()))]),
                        identity_ok: true,
                    },
                ),
            ),
        ] {
            let forged = forge_commits(&genuine, vec![(&g, vec![event])], &[]);
            assert_eq!(
                reseal_and_load(&ledger, &vault, &forged),
                Err(LedgerError::Corrupt),
                "load accepted: {label}"
            );
        }
        for event in [
            rec(true, post(Some(&h1))),
            rec(false, ReconcileObservation::MatchesPreImage),
            rec(
                false,
                ReconcileObservation::Unknown {
                    per_path: BTreeMap::from([(p.to_owned(), Some(h9.clone()))]),
                    identity_ok: true,
                },
            ),
        ] {
            let forged = forge_commits(&genuine, vec![(&g, vec![event])], &[]);
            assert_eq!(reseal_and_load(&ledger, &vault, &forged), Ok(4));
        }
        assert_eq!(reseal_and_load(&ledger, &vault, &genuine), Ok(3));
        // The genuine recover() path records the exact post-image and loads.
        let probe = probe(&[(p, Some(&h1))], true);
        let r = ledger.recover(t, &boot_b, &probe).unwrap();
        assert!(matches!(
            &r.recorded[..],
            [(_, 1, ReconcileObservation::MatchesPostImage { hashes })]
                if hashes == &BTreeMap::from([(p.to_owned(), Some(h1.clone()))])
        ));
        assert_eq!(ledger.load(t).unwrap().commit_starts, vec![1, 3, 4]);
    }
}
