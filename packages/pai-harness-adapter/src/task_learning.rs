//! Stage 6 owner K2: the coding-task <-> knowledge learning loop
//! (`stage6-design.md` §0, §2, §5).
//!
//! * [`TaskLearning::relevant_patterns`]: one Stage 3 CURRENT-mode lexical query
//!   per working-set file, fenced by the EXACT file identity (repository label
//!   source_id, `SELECTION_SCHEMA`, manifest sha of the captured base selection,
//!   sha256 of the file bytes, Stage 4 platform string). Only active,
//!   non-contradictory VerifiedPattern / ApprovedProcedure heads are returned,
//!   labelled "lexical match on exact file identity". A stale or missing index
//!   is reported with no hits; the index is NEVER rebuilt here.
//! * [`TaskLearning::propose_candidate`]: explicit UI event only, and only when
//!   the task's own ledger shows build/test passed on the CURRENT manifest and
//!   every changed file is reviewed Accepted. Writes one Observation Evidence
//!   (gate/command ids, exit codes, tree sha; never file contents) and one
//!   Candidate. Nothing is promoted.
//! * [`TaskLearning::verification_preview`] / [`TaskLearning::verify_candidate`]:
//!   a Stage 4 [`VerificationRecipe`] only when representable; the user confirms
//!   the exact recipe sha before anything runs; `KnowledgeVerifier` decides
//!   (baseline must fail, oracle bytes identical, dependency fence). Non-Linux
//!   and non-representable tasks are `unsupported` BEFORE any materialization
//!   or spawn.
//! * [`TaskLearning::approve_pattern`] / [`TaskLearning::revoke_pattern`]:
//!   UI-only wrappers over `approve_from_ui` / `revoke_from_ui` (never model
//!   tools; the Tauri glue checks the main window).
//!
//! Recipe mapping (Stage 4 `VerificationRecipe::validate` is stricter than the
//! design sketch; each rule below is listed as a deviation in the K2 handoff):
//! * every Oracle-role gate command must be `PythonScript`; 3-4 of them, one case
//!   each (a script is never repeated to pad the count);
//! * case kind comes from the command id prefix: `negative*` -> Negative,
//!   `regression*` -> Regression, anything else -> Positive. Stage 4 needs at
//!   least one of each kind and the first case Positive (the first Positive
//!   command is moved to the front, the rest keep plan order);
//! * expected status / stdout / stderr are the exact bytes the LAST passing gate
//!   on the current manifest recorded for that command (read back from its
//!   encrypted gate log; truncated, non-UTF-8 or >1024-byte output is
//!   unsupported). Every K2 Stage 4 run is capped at 1024 bytes per stream
//!   (the Stage 4 expected-output limit). Stage 3 skips over-long corpus terms,
//!   so hex-encoded receipt output can no longer block index rebuilds;
//! * output-file contract: every argument of the form `/tmp/<name>` is a
//!   declared output whose expected sha256 is sha256(the command's stdout); the
//!   first Positive case must declare one (Stage 4 requires a file
//!   postcondition on case 0);
//! * oracle_files = protected ∩ selected, implementation_files = the rest; the
//!   base and current file sets must be equal and at least one implementation
//!   file must change; fixed Stage 4 run limits; 2 repetitions.
//!
//! Every string that originated in task files or gate output (objective, case
//! stdout, snippets) is untrusted DATA for the UI. Errors carry no payload.

use crate::coding_task::ports::{
    manifest_of, manifest_sha256, FileDecision, Files, GateCommand, GateRole, SELECTION_SCHEMA,
};
use crate::coding_task::{
    CodingTaskService, IsolationCapability, TaskError, TaskView, NON_LINUX_REASON,
};
use crate::isolation::{
    ActualRun, Cancellation, IsolationError, ProjectSnapshot, RunLimits, SnapshotBinding,
    SnapshotPolicy, Termination,
};
use crate::knowledge::{digest, EvidenceVault, KnowledgeStoreError};
use crate::knowledge_retrieval::{
    ApplicabilityFence, KnowledgeRetriever, QueryKind, RecallMode, RetrievalError, RetrievalQuery,
    TrustedSource, DECLARED_ALIASES, FENCE_SCHEMA,
};
use crate::knowledge_verification::{
    ActualCheckedState, CaseKind, KnowledgeVerifier, UiApprovalEvent, VerificationCase,
    VerificationError, VerificationPolicy, VerificationRecipe, RECIPE_SCHEMA,
};
use crate::task_ledger::{
    now_ms, random_hex16, CheckStatus, GateRecord, ReviewStatus, TaskId, TaskSpec,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use unoone_capability_contracts::knowledge::{
    Applicability, Audit, Candidate, Evidence, EvidenceKind, KnowledgeKind, KnowledgeMetadata,
    KnowledgePrivacy, KnowledgeRecord, RecordHeader, RecordRef, KNOWLEDGE_SCHEMA,
};
use unoone_vault_core::Vault;

// K1 types (Stage 6 merge: stand-ins replaced by the real K1 definitions).
pub use crate::knowledge_service::{CheckSummary, IndexState, KnowledgeHitView, SourceBadge};

// ===========================================================================
// K2 public types (stage6-design.md §2, frozen)
// ===========================================================================

/// Fixed classification, no payload (Display is a lowercase snake_case word).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearningError {
    Locked,
    NotFound,
    Conflict,
    Invalid,
    Unsupported,
    NotReady,
    Persistence,
}
impl std::fmt::Display for LearningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Locked => "locked",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Invalid => "invalid",
            Self::Unsupported => "unsupported",
            Self::NotReady => "not_ready",
            Self::Persistence => "persistence",
        })
    }
}
impl std::error::Error for LearningError {}

#[derive(Serialize)]
pub struct RelevantPatternsView {
    pub index: IndexState,
    pub hits: Vec<KnowledgeHitView>,
    pub note: String,
}

/// SYNTHETIC in tests; in the product only the main window sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiProposeCandidateEvent {
    pub view_seq: u64,
    pub change_set_sha256: String,
    pub ui_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateProposalView {
    pub candidate: RecordRef,
    pub evidence: Vec<RecordRef>,
    pub verification: VerificationSupport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VerificationSupport {
    Supported { cases: usize },
    Unsupported { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerificationPreview {
    pub candidate: RecordRef,
    pub recipe: VerificationRecipe,
    pub recipe_sha256: String,
    pub support: VerificationSupport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiVerifyCandidateEvent {
    pub candidate: RecordRef,
    pub recipe_sha256: String,
    pub ui_event_id: String,
}

/// `state`: verified | failed | cancelled | unsupported (verify), approved
/// (approve), revoked (revoke).
#[derive(Serialize)]
pub struct LearningVerificationView {
    pub state: String,
    pub pattern: Option<RecordRef>,
    pub procedure_run: Option<RecordRef>,
    pub run_sha256: Option<String>,
    pub policy_sha256: Option<String>,
    pub checks: Vec<CheckSummary>,
    pub approved: Option<RecordRef>,
    pub residuals: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiApprovePatternEvent {
    pub pattern: RecordRef,
    pub procedure_run: RecordRef,
    pub displayed_run_sha256: String,
    pub displayed_policy_sha256: String,
    pub ui_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiRevokePatternEvent {
    pub approved: RecordRef,
    pub ui_event_id: String,
}

// ===========================================================================
// Constants
// ===========================================================================

/// Label of every relevant-pattern hit (design §2).
pub const RELEVANT_LABEL: &str = "lexical match on exact file identity";
const RELEVANT_NOTE: &str =
    "lexical match on exact file identity: Stage 3 current mode, fenced by \
     repository label, Stage 5 selection schema, base selection manifest, file SHA-256 and \
     platform; active, non-contradictory verified/approved patterns only; lexical AND postings, \
     not semantic; the index is never rebuilt implicitly";
const STALE_NOTE: &str = "knowledge index is stale or missing: no hits are shown until the index \
     is rebuilt explicitly (it is never rebuilt implicitly)";
const OBSERVATION_SCHEMA: &str = "inbharat.pai.stage6.task-learning.gate-summary.v1";
const METHOD: &str = "stage6-task-learning-v1; deterministic; no model; no network";
const LICENSE: &str = "private-local: derived from the user's own coding task";
const TOPIC_MARKER: &str = "coding-task-learning";
const PYTHON: &str = "/usr/bin/python3";
const MAX_OBJECTIVE_BYTES: usize = 1024;
const MAX_QUERY_TERMS: usize = 16;
const MAX_QUERY_CHARS: usize = 256;
const MAX_TERM_CHARS: usize = 64;
/// Per-stream byte cap of every K2 Stage 4 run AND of every expected output:
/// Stage 4 recipe validation caps each expected stdout/stderr at 1 KiB, so the
/// run cap matches it. (Was 128 while Stage 3 failed rebuild on corpus terms
/// >256 scalars; the Stage 6 merge made Stage 3 skip such terms.)
const MAX_CASE_OUTPUT: usize = 1024;
/// Longest run of term characters kept in learned text (lowercase expansion is
/// <= 3 scalars per char, so a run stays < Stage 3's 256-scalar term limit).
const MAX_TERM_RUN: usize = 64;
const MAX_TITLE_CHARS: usize = 120;
const SNIPPET_CHARS: usize = 280;
const STAGE4_MAX_FILES: usize = 16;
const STAGE4_MAX_BYTES: usize = 1024 * 1024;
/// Receipt age the approval gate admits (Stage 4 policy maximum, 24 h).
const RECEIPT_AGE_MS: u64 = 24 * 60 * 60 * 1000;

const RESIDUALS: &[&str] = &[
    "Stage 4 verification shapes are limited to PythonScript oracles (argv /usr/bin/python3 -I /work/<script>)",
    "expected status/stdout/stderr are copied from the last passing Stage 5 gate; a /tmp/<name> argument is an output whose bytes must equal the command's stdout",
    "Stage 4 output is capped at 1024 bytes per stream (the Stage 4 expected-output limit); an oracle whose expected output exceeds 1 KiB is Unsupported, and a baseline failing with more output ends output_limit and the run is failed",
    "Windows / non-Linux: verification, approval and revocation are unavailable (Unsupported before any spawn)",
    "current-mode retrieval needs the exact file identity (base selection manifest + file digest)",
    "trusted-owner threat model and Stage 2/3 rollback residuals unchanged",
];

fn run_limits() -> RunLimits {
    RunLimits {
        cpu_seconds: 5,
        memory_bytes: 256 * 1024 * 1024,
        processes: 32,
        timeout_ms: 5000,
        output_bytes: MAX_CASE_OUTPUT,
    }
}

// ===========================================================================
// Error mapping (fixed classifications, no payload)
// ===========================================================================

fn task_err(e: TaskError) -> LearningError {
    match e {
        TaskError::Locked => LearningError::Locked,
        TaskError::NotFound => LearningError::NotFound,
        TaskError::Conflict | TaskError::Busy => LearningError::Conflict,
        TaskError::IsolationUnavailable | TaskError::WorktreeUnavailable => {
            LearningError::Unsupported
        }
        TaskError::Paused | TaskError::AdmissionDenied(_) => LearningError::NotReady,
        TaskError::Invalid(_) | TaskError::Edit(_) => LearningError::Invalid,
        TaskError::LedgerFull | TaskError::Internal => LearningError::Persistence,
    }
}
fn store_err(e: KnowledgeStoreError) -> LearningError {
    match e {
        KnowledgeStoreError::Locked => LearningError::Locked,
        KnowledgeStoreError::Uninitialized => LearningError::NotReady,
        KnowledgeStoreError::Conflict | KnowledgeStoreError::Invalidated => LearningError::Conflict,
        KnowledgeStoreError::NotFound => LearningError::NotFound,
        KnowledgeStoreError::InvalidRecord => LearningError::Invalid,
        KnowledgeStoreError::Corrupt
        | KnowledgeStoreError::UnsupportedSchema
        | KnowledgeStoreError::Limit
        | KnowledgeStoreError::Persistence => LearningError::Persistence,
    }
}
fn iso_err(e: IsolationError) -> LearningError {
    match e {
        IsolationError::IsolationUnavailable => LearningError::Unsupported,
        IsolationError::InvalidInput => LearningError::Invalid,
        IsolationError::DeniedRoot | IsolationError::HashMismatch => LearningError::Conflict,
        IsolationError::Io => LearningError::Persistence,
    }
}
fn verify_err(e: VerificationError) -> LearningError {
    match e {
        VerificationError::Store(s) => store_err(s),
        VerificationError::Isolation(i) => iso_err(i),
        VerificationError::InvalidRecipe => LearningError::Invalid,
        VerificationError::Identity | VerificationError::GateDenied => LearningError::Conflict,
        VerificationError::ForgedProof => LearningError::Persistence,
    }
}
fn io_err<T>(r: std::io::Result<T>) -> Result<T, LearningError> {
    r.map_err(|_| LearningError::Persistence)
}

// ===========================================================================
// Small pure helpers
// ===========================================================================

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn check_event_id(id: &str) -> Result<(), LearningError> {
    if is_lower_hex(id, 32) {
        Ok(())
    } else {
        Err(LearningError::Invalid)
    }
}
fn check_sha(s: &str) -> Result<(), LearningError> {
    if is_lower_hex(s, 64) {
        Ok(())
    } else {
        Err(LearningError::Invalid)
    }
}
fn check_ref(r: &RecordRef, kind: KnowledgeKind) -> Result<(), LearningError> {
    if r.kind != kind || r.validate().is_err() {
        return Err(LearningError::Invalid);
    }
    Ok(())
}
/// The Stage 4 platform string (`ProjectSnapshot` binding platform).
pub fn stage4_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}
fn truncate_utf8(text: &str, cap: usize) -> &str {
    if text.len() <= cap {
        return text;
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
fn recipe_sha256(recipe: &VerificationRecipe) -> Result<String, LearningError> {
    // Same formula as the Stage 4 run receipt's recipe_sha256.
    serde_json::to_vec(recipe)
        .map(|b| digest(&b))
        .map_err(|_| LearningError::Persistence)
}
fn changed_files(base: &Files, current: &Files) -> BTreeSet<String> {
    base.keys()
        .chain(current.keys())
        .filter(|p| base.get(*p) != current.get(*p))
        .cloned()
        .collect()
}
fn termination_name(t: Termination) -> String {
    match t {
        Termination::Completed => "completed",
        Termination::Timeout => "timeout",
        Termination::OutputLimit => "output_limit",
        Termination::Cancelled => "cancelled",
        Termination::RunnerFailure => "runner_failure",
    }
    .to_owned()
}

/// Exactly Stage 3's tokenizer (`knowledge_retrieval::lexical_terms`):
/// Unicode-scalar lowercase, alphanumeric / '_' / ':' runs.
fn lexical_terms(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut term = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || c == '_' || c == ':' {
            term.push(c);
        } else if !term.is_empty() {
            result.push(std::mem::take(&mut term));
        }
    }
    if !term.is_empty() {
        result.push(term);
    }
    result
}
/// Bounded query terms for one file: file-stem terms, then objective terms;
/// deduplicated, <= 16 terms, <= 256 scalars joined, terms <= 64 scalars;
/// Stage 3 alias SOURCES are dropped (the query side would rewrite them and
/// never match the corpus spelling).
fn query_terms(path: &str, objective: &str) -> Vec<String> {
    let stem = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    let mut chars = 0usize;
    for term in lexical_terms(&stem)
        .into_iter()
        .chain(lexical_terms(objective))
    {
        if out.len() >= MAX_QUERY_TERMS {
            break;
        }
        let n = term.chars().count();
        if n > MAX_TERM_CHARS
            || out.contains(&term)
            || DECLARED_ALIASES.iter().any(|(from, _)| *from == term)
        {
            continue;
        }
        let add = n + usize::from(!out.is_empty());
        if chars + add > MAX_QUERY_CHARS {
            break;
        }
        chars += add;
        out.push(term);
    }
    out
}
/// Candidate topics: a marker plus the primary file's query terms, so the
/// current-mode AND query also matches the later ApprovedProcedure head (whose
/// lexical body is a run digest) through its inherited topics.
fn topics(primary: &str, objective: &str) -> Vec<String> {
    let mut topics = vec![TOPIC_MARKER.to_owned()];
    topics.extend(query_terms(primary, objective));
    topics
}
/// Break runs of Stage 3 term characters longer than `MAX_TERM_RUN` with a
/// space, so a learned record never carries a term Stage 3 `rebuild()` rejects.
fn index_safe(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = 0usize;
    for c in text.chars() {
        if c.to_lowercase()
            .any(|l| l.is_alphanumeric() || l == '_' || l == ':')
        {
            if run == MAX_TERM_RUN {
                out.push(' ');
                run = 0;
            }
            run += 1;
        } else {
            run = 0;
        }
        out.push(c);
    }
    out
}
fn first_line(text: &str) -> String {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect()
}

/// Parse a Stage 5 gate log (`== <id> <stdout|stderr> <n> bytes\n<n bytes>\n`
/// per stream). Length-prefixed, so embedded separators are harmless; parsing
/// stops at the first incomplete entry (the run cap may cut the tail) and only
/// complete entries are returned.
fn parse_gate_log(log: &[u8]) -> BTreeMap<(String, String), Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut i = 0usize;
    while i < log.len() {
        let Some(nl) = log[i..].iter().position(|b| *b == b'\n').map(|p| p + i) else {
            break;
        };
        let Ok(header) = std::str::from_utf8(&log[i..nl]) else {
            break;
        };
        let parts: Vec<&str> = header.split(' ').collect();
        let [marker, id, stream, n, unit] = parts.as_slice() else {
            break;
        };
        let Ok(n) = n.parse::<usize>() else {
            break;
        };
        if *marker != "==" || *unit != "bytes" || !matches!(*stream, "stdout" | "stderr") {
            break;
        }
        let start = nl + 1;
        let Some(end) = start.checked_add(n) else {
            break;
        };
        if end >= log.len() || log[end] != b'\n' {
            break;
        }
        let key = ((*id).to_owned(), (*stream).to_owned());
        if out.contains_key(&key) {
            break;
        }
        out.insert(key, log[start..end].to_vec());
        i = end + 1;
    }
    out
}

fn case_kind(id: &str) -> CaseKind {
    let id = id.to_ascii_lowercase();
    if id.starts_with("negative") {
        CaseKind::Negative
    } else if id.starts_with("regression") {
        CaseKind::Regression
    } else {
        CaseKind::Positive
    }
}
/// Exactly the Stage 4 expected-file path rules (validate + runner).
fn is_tmp_output(arg: &str) -> bool {
    let Some(rest) = arg.strip_prefix("/tmp/") else {
        return false;
    };
    !rest.is_empty()
        && arg.len() <= 256
        && rest != "."
        && !rest.contains('/')
        && !rest.contains('\\')
        && !arg.contains("..")
        && !arg.contains('\0')
}
fn command_kind_name(c: &GateCommand) -> &'static str {
    match c {
        GateCommand::PythonScript { .. } => "python_script",
        GateCommand::PythonUnittest { .. } => "python_unittest",
        GateCommand::PythonCompile { .. } => "python_compile",
    }
}
fn preview_binding(files: &Files, primary: &str) -> SnapshotBinding {
    SnapshotBinding {
        source: TrustedSource {
            source_id: "preview".into(),
            source_version: SELECTION_SCHEMA.into(),
            source_commit: "0".repeat(64),
            file_digest: files.get(primary).map(|b| digest(b)).unwrap_or_default(),
        },
        platform: stage4_platform(),
        primary: primary.to_owned(),
        files: manifest_of(files),
        snapshot_sha256: String::new(),
    }
}

/// The recipe that would run plus the first reason it cannot (if any).
struct RecipePlan {
    recipe: VerificationRecipe,
    unsupported: Option<String>,
}

/// Deterministic recipe from the task spec, its base/current bytes and the
/// last passing gate on the current manifest. Gate logs are read only once the
/// command shape is representable.
fn build_recipe(
    spec: &TaskSpec,
    base: &Files,
    current: &Files,
    gate: &GateRecord,
    mut read_log: impl FnMut(&str) -> Result<Vec<u8>, LearningError>,
) -> Result<RecipePlan, LearningError> {
    let oracle_files: BTreeSet<String> = spec
        .oracle_files
        .iter()
        .filter(|p| current.contains_key(*p))
        .cloned()
        .collect();
    let implementation_files: BTreeSet<String> = current
        .keys()
        .filter(|p| !oracle_files.contains(*p))
        .cloned()
        .collect();
    let mut recipe = VerificationRecipe {
        schema: RECIPE_SCHEMA.into(),
        repetitions: 2,
        limits: run_limits(),
        cases: vec![],
        oracle_files,
        implementation_files,
    };
    let oracle_commands: Vec<&GateCommand> = spec
        .gate_plan
        .commands
        .iter()
        .filter(|c| c.role() == GateRole::Oracle)
        .collect();
    let shape = if oracle_commands.is_empty() {
        Some("the task has no oracle-role gate command".to_owned())
    } else if let Some(c) = oracle_commands
        .iter()
        .find(|c| !matches!(c, GateCommand::PythonScript { .. }))
    {
        Some(format!(
            "oracle-role gate command `{}` is {}; Stage 4 recipes accept only PythonScript \
             oracles (argv /usr/bin/python3 -I /work/<script>)",
            c.id(),
            command_kind_name(c)
        ))
    } else if !(3..=4).contains(&oracle_commands.len()) {
        Some(format!(
            "Stage 4 needs 3-4 cases; the task has {} PythonScript oracle command(s) and a \
             script is never repeated to pad the count",
            oracle_commands.len()
        ))
    } else {
        None
    };
    if shape.is_some() {
        return Ok(RecipePlan {
            recipe,
            unsupported: shape,
        });
    }
    let mut log = Vec::new();
    for blob in &gate.logs {
        log.extend(read_log(&blob.sha256)?);
    }
    let outputs = parse_gate_log(&log);
    let mut reason: Option<String> = None;
    let mut cases = Vec::new();
    for c in oracle_commands {
        let GateCommand::PythonScript {
            id, script, args, ..
        } = c
        else {
            continue;
        };
        let mut fail = |r: String| {
            reason.get_or_insert(r);
        };
        let Some(summary) = gate.commands.iter().find(|x| &x.id == id) else {
            fail(format!("the passing gate has no result for `{id}`"));
            continue;
        };
        let (Some(stdout), Some(stderr)) = (
            outputs.get(&(id.clone(), "stdout".to_owned())),
            outputs.get(&(id.clone(), "stderr".to_owned())),
        ) else {
            fail(format!("the gate log of `{id}` was not retained"));
            continue;
        };
        let Some(status) = summary.status else {
            fail(format!("`{id}` has no exit status"));
            continue;
        };
        if summary.termination != Termination::Completed
            || summary.truncated
            || stdout.len() as u64 != summary.stdout_total_bytes
            || stdout.len() as u64 != summary.stdout_retained_bytes
            || stderr.len() as u64 != summary.stderr_total_bytes
            || stderr.len() as u64 != summary.stderr_retained_bytes
            || stdout.len() > MAX_CASE_OUTPUT
            || stderr.len() > MAX_CASE_OUTPUT
        {
            fail(format!(
                "`{id}` output is truncated or larger than {MAX_CASE_OUTPUT} bytes per stream"
            ));
            continue;
        }
        let (Ok(expected_stdout), Ok(expected_stderr)) = (
            String::from_utf8(stdout.clone()),
            String::from_utf8(stderr.clone()),
        ) else {
            fail(format!("`{id}` output is not UTF-8"));
            continue;
        };
        let kind = case_kind(id);
        if kind != CaseKind::Negative && status != 0 {
            fail(format!(
                "`{id}` is a {} case but passed with exit {status}; only negative* cases may \
                 expect a non-zero exit",
                if kind == CaseKind::Positive {
                    "positive"
                } else {
                    "regression"
                }
            ));
            continue;
        }
        let expected_files: BTreeMap<String, String> = args
            .iter()
            .filter(|a| is_tmp_output(a))
            .map(|a| (a.clone(), digest(stdout)))
            .collect();
        let mut argv = vec![
            PYTHON.to_owned(),
            "-I".to_owned(),
            format!("/work/{script}"),
        ];
        argv.extend(args.iter().cloned());
        cases.push(VerificationCase {
            name: id.clone(),
            kind,
            argv,
            expected_status: status,
            expected_stdout,
            expected_stderr,
            expected_files,
        });
    }
    if let Some(i) = cases.iter().position(|c| c.kind == CaseKind::Positive) {
        let first = cases.remove(i);
        cases.insert(0, first);
    }
    recipe.cases = cases;
    if reason.is_none() {
        reason = representability(&recipe, spec, base, current);
    }
    Ok(RecipePlan {
        recipe,
        unsupported: reason,
    })
}
fn representability(
    recipe: &VerificationRecipe,
    spec: &TaskSpec,
    base: &Files,
    current: &Files,
) -> Option<String> {
    for kind in [CaseKind::Positive, CaseKind::Negative, CaseKind::Regression] {
        if !recipe.cases.iter().any(|c| c.kind == kind) {
            return Some(
                "Stage 4 requires positive, negative and regression cases: name oracle command \
                 ids with the prefixes `negative` / `regression` (others count as positive)"
                    .into(),
            );
        }
    }
    if recipe.cases[0].expected_files.is_empty() {
        return Some(
            "the first positive oracle must pass a /tmp/<name> output argument and write its \
             stdout bytes there (Stage 4 requires a file postcondition on case 0)"
                .into(),
        );
    }
    if !base.keys().eq(current.keys()) {
        return Some("files were created or deleted; Stage 4 compares the same file set".into());
    }
    if recipe
        .oracle_files
        .iter()
        .any(|p| base.get(p) != current.get(p))
    {
        return Some("an oracle file differs between base and current".into());
    }
    if !recipe
        .implementation_files
        .iter()
        .any(|p| base.get(p) != current.get(p))
    {
        return Some("no implementation file changed".into());
    }
    for files in [base, current] {
        if files.len() > STAGE4_MAX_FILES
            || files.values().map(Vec::len).sum::<usize>() > STAGE4_MAX_BYTES
        {
            return Some("the selection exceeds Stage 4 snapshot bounds (16 files, 1 MiB)".into());
        }
    }
    if recipe
        .validate(&preview_binding(current, &spec.primary))
        .is_err()
        || recipe
            .validate(&preview_binding(base, &spec.primary))
            .is_err()
    {
        return Some(
            "the recipe is outside Stage 4 bounds (argv <= 16 entries of <= 256 bytes, <= 4 \
             outputs per case, recipe <= 8 KiB)"
                .into(),
        );
    }
    None
}

fn support_of(view: &TaskView, plan: &RecipePlan) -> VerificationSupport {
    if !cfg!(target_os = "linux") {
        return VerificationSupport::Unsupported {
            reason: format!("Stage 4 verification unavailable: {NON_LINUX_REASON}"),
        };
    }
    match &view.capability {
        IsolationCapability::RuntimeVerified { .. } => {}
        IsolationCapability::Unsupported { reason } => {
            return VerificationSupport::Unsupported {
                reason: format!("Stage 4 verification unavailable: {reason}"),
            }
        }
        IsolationCapability::SupportedUnverified => {
            return VerificationSupport::Unsupported {
                reason: "Stage 4 verification unavailable: isolation is not runtime-verified"
                    .into(),
            }
        }
    }
    match &plan.unsupported {
        Some(reason) => VerificationSupport::Unsupported {
            reason: reason.clone(),
        },
        None => VerificationSupport::Supported {
            cases: plan.recipe.cases.len(),
        },
    }
}
/// Execution (verify/approve/revoke all need `KnowledgeVerifier`) is
/// available only on Linux with a runtime-verified backend.
fn execution_unsupported(view: &TaskView) -> Option<String> {
    let probe = RecipePlan {
        recipe: VerificationRecipe {
            schema: RECIPE_SCHEMA.into(),
            repetitions: 2,
            limits: run_limits(),
            cases: vec![],
            oracle_files: BTreeSet::new(),
            implementation_files: BTreeSet::new(),
        },
        unsupported: None,
    };
    match support_of(view, &probe) {
        VerificationSupport::Unsupported { reason } => Some(reason),
        VerificationSupport::Supported { .. } => None,
    }
}
fn residuals() -> Vec<String> {
    RESIDUALS.iter().map(|s| (*s).to_owned()).collect()
}
fn unsupported_view(reason: String) -> LearningVerificationView {
    let mut residuals = vec![reason];
    residuals.extend(self::residuals());
    LearningVerificationView {
        state: "unsupported".into(),
        pattern: None,
        procedure_run: None,
        run_sha256: None,
        policy_sha256: None,
        checks: vec![],
        approved: None,
        residuals,
    }
}

// ===========================================================================
// Private scratch materialization (0700 dirs, 0600 files, create_new)
// ===========================================================================

fn make_private_dir(dir: &Path) -> Result<(), LearningError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        io_err(std::fs::DirBuilder::new().mode(0o700).create(dir))
    }
    #[cfg(not(unix))]
    {
        io_err(std::fs::create_dir(dir))
    }
}
fn make_private_dirs(dir: &Path) -> Result<(), LearningError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        io_err(
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir),
        )
    }
    #[cfg(not(unix))]
    {
        io_err(std::fs::create_dir_all(dir))
    }
}
fn valid_rel(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 256
        && !path.contains('\\')
        && !path.contains('\0')
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != "..")
}
fn materialize(dir: &Path, files: &Files) -> Result<(), LearningError> {
    use std::io::Write;
    make_private_dir(dir)?;
    for (path, bytes) in files {
        if !valid_rel(path) {
            return Err(LearningError::Invalid);
        }
        let target = dir.join(path);
        let parent = target.parent().ok_or(LearningError::Invalid)?;
        make_private_dirs(parent)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        io_err(io_err(options.open(&target))?.write_all(bytes))?;
    }
    Ok(())
}
/// One verification's private directory; removed (best effort) on drop. Every
/// `ProjectSnapshot` taken from it must be dropped first (declared later).
struct Workdir {
    dir: PathBuf,
}
impl Drop for Workdir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Baseline + fixed snapshots of one verification. Field order IS drop order:
/// both snapshots (their own scratch copies) go before the private workdir.
struct SnapshotPair {
    baseline: ProjectSnapshot,
    fixed: ProjectSnapshot,
    _work: Workdir,
}

/// Exact identity of the task's current working set (design §2).
struct Identity {
    source_id: String,
    base_manifest_sha256: String,
    base_primary_sha256: String,
    current_primary_sha256: String,
    platform: String,
}
impl Identity {
    fn source(&self, file_digest: &str) -> TrustedSource {
        TrustedSource {
            source_id: self.source_id.clone(),
            source_version: SELECTION_SCHEMA.into(),
            source_commit: self.base_manifest_sha256.clone(),
            file_digest: file_digest.to_owned(),
        }
    }
    fn metadata(&self, topics: Vec<String>) -> Result<KnowledgeMetadata, LearningError> {
        Ok(KnowledgeMetadata {
            source_id: self.source_id.clone(),
            source_version: SELECTION_SCHEMA.into(),
            source_commit: self.base_manifest_sha256.clone(),
            license: LICENSE.into(),
            privacy: KnowledgePrivacy::Private,
            applicability: Applicability {
                topics,
                platforms: vec![self.platform.clone()],
                constraints: ApplicabilityFence {
                    schema: FENCE_SCHEMA.into(),
                    file_digest: self.current_primary_sha256.clone(),
                }
                .encode()
                .map_err(|_| LearningError::Invalid)?,
            },
        })
    }
    /// The record carries exactly this task's current identity.
    fn admits(&self, m: &KnowledgeMetadata) -> bool {
        let fence = serde_json::from_str::<ApplicabilityFence>(&m.applicability.constraints);
        matches!(fence, Ok(f) if f.schema == FENCE_SCHEMA && f.file_digest == self.current_primary_sha256)
            && m.source_id == self.source_id
            && m.source_version == SELECTION_SCHEMA
            && m.source_commit == self.base_manifest_sha256
            && m.applicability.platforms == [self.platform.clone()]
    }
    /// Same repository + base selection (any working-set revision).
    fn same_base(&self, m: &KnowledgeMetadata) -> bool {
        m.source_id == self.source_id
            && m.source_version == SELECTION_SCHEMA
            && m.source_commit == self.base_manifest_sha256
    }
}

/// One consistent read of the task: the public view plus the K2 read-only
/// accessor's bytes, at the same ledger head.
struct Material {
    view: TaskView,
    spec: TaskSpec,
    base: Files,
    current: Files,
    gates: Vec<GateRecord>,
}
impl Material {
    fn identity(&self) -> Result<Identity, LearningError> {
        let primary = &self.spec.primary;
        let current = self.current.get(primary).ok_or(LearningError::Conflict)?;
        let base = self.base.get(primary).ok_or(LearningError::Persistence)?;
        Ok(Identity {
            source_id: self.view.repository.source_id.clone(),
            base_manifest_sha256: manifest_sha256(&manifest_of(&self.base)),
            base_primary_sha256: digest(base),
            current_primary_sha256: digest(current),
            platform: stage4_platform(),
        })
    }
    /// The last passing gate on the CURRENT manifest, with every changed file
    /// reviewed Accepted; otherwise `NotReady`. Read from the task ledger only.
    fn ready_gate(&self) -> Result<GateRecord, LearningError> {
        let outcome = &self.view.outcome;
        let CheckStatus::Passed { gate } = &outcome.test_status else {
            return Err(LearningError::NotReady);
        };
        let has_build = self
            .spec
            .gate_plan
            .commands
            .iter()
            .any(|c| c.role() == GateRole::Build);
        match &outcome.build_status {
            CheckStatus::Passed { gate: g } if g == gate => {}
            CheckStatus::NotRun if !has_build => {}
            _ => return Err(LearningError::NotReady),
        }
        let current_sha = manifest_sha256(&manifest_of(&self.current));
        let record = self
            .gates
            .iter()
            .rev()
            .find(|g| &g.gate_run_id == gate)
            .ok_or(LearningError::NotReady)?;
        if record.working_set_sha256 != current_sha
            || record.termination == Termination::RunnerFailure
        {
            return Err(LearningError::NotReady);
        }
        let changed = changed_files(&self.base, &self.current);
        let shown: BTreeSet<String> = self.view.diff.iter().map(|d| d.path.clone()).collect();
        if changed.is_empty()
            || shown != changed
            || self
                .view
                .diff
                .iter()
                .any(|d| d.stale || !matches!(d.decision, FileDecision::Accepted))
        {
            return Err(LearningError::NotReady);
        }
        match outcome.review_status {
            ReviewStatus::Decided {
                accepted,
                rejected: 0,
            } if accepted as usize == changed.len() => {}
            _ => return Err(LearningError::NotReady),
        }
        Ok(record.clone())
    }
}

/// A verification preview plus the exact material it was built from.
struct Prepared {
    preview: VerificationPreview,
    material: Material,
    identity: Identity,
}

#[derive(Deserialize)]
struct SealedCheck {
    body: CheckBody,
}
#[derive(Deserialize)]
struct CheckBody {
    baseline: bool,
    case: VerificationCase,
    actual: ActualRun,
}
fn case_passed(case: &VerificationCase, actual: &ActualRun) -> bool {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    actual.termination == Termination::Completed
        && actual.status == Some(case.expected_status)
        && actual.stdout_hex == hex(case.expected_stdout.as_bytes())
        && actual.stderr_hex == hex(case.expected_stderr.as_bytes())
        && actual.output_files == case.expected_files
}

// ===========================================================================
// Service
// ===========================================================================

/// The coding-task <-> knowledge learning loop. `Send + Sync`; construction
/// does no I/O. All records go through the ONE encrypted `EvidenceVault`.
pub struct TaskLearning {
    vault: Arc<Mutex<Option<Vault>>>,
    tasks: Arc<CodingTaskService>,
    store: EvidenceVault,
    retriever: KnowledgeRetriever,
    scratch: PathBuf,
    /// Linux only, created lazily on the first execution-bearing call.
    verifier: Mutex<Option<Arc<KnowledgeVerifier>>>,
    active: Mutex<BTreeMap<u64, Cancellation>>,
    next_run: AtomicU64,
    /// UI event ids already consumed by a state-changing call (in-memory,
    /// per process): a replayed event is refused with Conflict.
    used_events: Mutex<BTreeSet<String>>,
}

impl TaskLearning {
    /// No I/O: the verifier, the scratch directory and every read are lazy.
    pub fn new(
        vault: Arc<Mutex<Option<Vault>>>,
        tasks: Arc<CodingTaskService>,
        scratch: PathBuf,
    ) -> Self {
        Self {
            store: EvidenceVault::new(vault.clone()),
            retriever: KnowledgeRetriever::new(vault.clone()),
            vault,
            tasks,
            scratch,
            verifier: Mutex::new(None),
            active: Mutex::new(BTreeMap::new()),
            next_run: AtomicU64::new(1),
            used_events: Mutex::new(BTreeSet::new()),
        }
    }

    /// Validate a UI event id and consume it (replay -> Conflict). Consumed
    /// even if the call later fails: the UI mints a fresh id per click.
    fn claim_event(&self, id: &str) -> Result<(), LearningError> {
        check_event_id(id)?;
        let mut used = self
            .used_events
            .lock()
            .map_err(|_| LearningError::Persistence)?;
        if used.len() >= 65_536 {
            used.clear(); // bounded memory; ids are 128-bit random per click
        }
        if !used.insert(id.to_owned()) {
            return Err(LearningError::Conflict);
        }
        Ok(())
    }

    /// Lock hook (call after the vault is locked, like `CodingTaskService::on_lock`):
    /// cancels in-flight verifications and drops the cached verifier.
    pub fn on_lock(&self) {
        if let Ok(active) = self.active.lock() {
            for cancel in active.values() {
                cancel.cancel();
            }
        }
        if let Ok(mut verifier) = self.verifier.lock() {
            *verifier = None;
        }
    }

    fn material(&self, task: &TaskId) -> Result<Material, LearningError> {
        let view = self.tasks.task_view(task).map_err(task_err)?;
        let (seq, spec, base, current, gates) =
            self.tasks.stage6_task_material(task).map_err(task_err)?;
        if seq != view.view_seq {
            return Err(LearningError::Conflict);
        }
        Ok(Material {
            view,
            spec,
            base,
            current,
            gates,
        })
    }

    fn plan(
        &self,
        task: &TaskId,
        m: &Material,
        gate: &GateRecord,
    ) -> Result<RecipePlan, LearningError> {
        build_recipe(&m.spec, &m.base, &m.current, gate, |sha| {
            self.tasks.stage6_gate_log(task, sha).map_err(task_err)
        })
    }

    fn verifier(&self) -> Result<Arc<KnowledgeVerifier>, LearningError> {
        let mut slot = self
            .verifier
            .lock()
            .map_err(|_| LearningError::Persistence)?;
        if let Some(v) = slot.as_ref() {
            return Ok(v.clone());
        }
        let policy = VerificationPolicy::readonly_replay(RECEIPT_AGE_MS).map_err(verify_err)?;
        let verifier =
            Arc::new(KnowledgeVerifier::new(self.vault.clone(), policy).map_err(verify_err)?);
        *slot = Some(verifier.clone());
        Ok(verifier)
    }

    fn workdir(&self) -> Result<Workdir, LearningError> {
        make_private_dirs(&self.scratch)?;
        let scratch = io_err(std::fs::canonicalize(&self.scratch))?;
        let dir = scratch.join(format!("learn-{}", random_hex16()));
        make_private_dir(&dir)?;
        Ok(Workdir { dir })
    }

    fn capture(
        policy: &SnapshotPolicy,
        root: &Path,
        source: TrustedSource,
        primary: &str,
        files: &Files,
    ) -> Result<ProjectSnapshot, LearningError> {
        let list: Vec<String> = files.keys().cloned().collect();
        let snapshot = policy
            .capture(root, source, primary, &list)
            .map_err(iso_err)?;
        if snapshot.binding().files != manifest_of(files) {
            return Err(LearningError::Conflict);
        }
        Ok(snapshot)
    }

    /// Materialize base and current bytes into two fresh private directories
    /// (0700) and capture them under a `SnapshotPolicy` over exactly those two
    /// roots, both bound to the same trusted source (base selection manifest).
    fn snapshot_pair(&self, m: &Material, id: &Identity) -> Result<SnapshotPair, LearningError> {
        let work = self.workdir()?;
        let base_root = work.dir.join("base");
        let fixed_root = work.dir.join("fixed");
        let snap = work.dir.join("snap");
        materialize(&base_root, &m.base)?;
        materialize(&fixed_root, &m.current)?;
        make_private_dir(&snap)?;
        let policy = SnapshotPolicy::new(vec![base_root.clone(), fixed_root.clone()], snap)
            .map_err(iso_err)?;
        let baseline = Self::capture(
            &policy,
            &base_root,
            id.source(&id.base_primary_sha256),
            &m.spec.primary,
            &m.base,
        )?;
        let fixed = Self::capture(
            &policy,
            &fixed_root,
            id.source(&id.current_primary_sha256),
            &m.spec.primary,
            &m.current,
        )?;
        Ok(SnapshotPair {
            baseline,
            fixed,
            _work: work,
        })
    }

    fn register(&self) -> Result<(u64, Cancellation), LearningError> {
        let key = self.next_run.fetch_add(1, Ordering::SeqCst);
        let cancel = Cancellation::default();
        self.active
            .lock()
            .map_err(|_| LearningError::Persistence)?
            .insert(key, cancel.clone());
        Ok((key, cancel))
    }
    fn unregister(&self, key: u64) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&key);
        }
    }

    /// Display-only decode of check receipts the verifier just produced or
    /// authenticated (never an admission gate).
    fn check_summaries(&self, refs: &[RecordRef]) -> Result<Vec<CheckSummary>, LearningError> {
        if refs.is_empty() {
            return Ok(vec![]);
        }
        let read = self.store.read_targeted(refs).map_err(store_err)?;
        let mut out = Vec::new();
        for (reference, item) in refs.iter().zip(read.items) {
            let KnowledgeRecord::Evidence(evidence) = &item.stored.record else {
                return Err(LearningError::Persistence);
            };
            let sealed: SealedCheck =
                serde_json::from_str(&evidence.content).map_err(|_| LearningError::Persistence)?;
            out.push(CheckSummary {
                reference: reference.clone(),
                case: sealed.body.case.name.clone(),
                role: if sealed.body.baseline {
                    "baseline"
                } else {
                    "fixed"
                }
                .into(),
                status: sealed.body.actual.status,
                termination: termination_name(sealed.body.actual.termination),
                passed: case_passed(&sealed.body.case, &sealed.body.actual),
            });
        }
        Ok(out)
    }

    // ------------------------------------------------------------- API

    /// Stage 3 current-mode recall per working-set file (exact file identity).
    /// Stale/missing index -> that state and no hits; never rebuilds.
    pub fn relevant_patterns(
        &self,
        task: &TaskId,
        limit: usize,
    ) -> Result<RelevantPatternsView, LearningError> {
        if !(1..=8).contains(&limit) {
            return Err(LearningError::Invalid);
        }
        let m = self.material(task)?;
        let id = m.identity()?;
        let objective = m.view.objective.clone();
        let mut recalled: Vec<(String, crate::knowledge_retrieval::RecallHit)> = Vec::new();
        let mut searched = false;
        for (path, bytes) in &m.current {
            let terms = query_terms(path, &objective);
            if terms.is_empty() {
                continue;
            }
            let query = RetrievalQuery {
                text: terms.join(" "),
                kind: QueryKind::Terms,
                mode: RecallMode::Current,
                platform: id.platform.clone(),
                trusted_source: Some(id.source(&digest(bytes))),
                topic: None,
                candidate_limit: crate::knowledge::MAX_TARGETED_CANDIDATES,
                result_limit: 16,
                context_chars: 16384,
                snippet_chars: SNIPPET_CHARS,
            };
            let result = match self.retriever.search(&query) {
                Ok(r) => r,
                Err(RetrievalError::StaleIndex) => return Ok(Self::empty(IndexState::Stale)),
                Err(
                    RetrievalError::UninitializedIndex
                    | RetrievalError::Store(KnowledgeStoreError::Uninitialized),
                ) => return Ok(Self::empty(IndexState::Missing)),
                Err(RetrievalError::Store(e)) => return Err(store_err(e)),
                Err(RetrievalError::InvalidQuery | RetrievalError::MissingTrustedVersion) => {
                    continue
                }
                Err(RetrievalError::UnsupportedIndexSchema | RetrievalError::IndexLimit) => {
                    return Err(LearningError::Persistence)
                }
            };
            searched = true;
            for hit in result.hits {
                if hit.active
                    && !hit.contradictory
                    && hit.mode == RecallMode::Current
                    && matches!(
                        hit.reference.kind,
                        KnowledgeKind::VerifiedPattern | KnowledgeKind::ApprovedProcedure
                    )
                    && !recalled.iter().any(|(_, h)| h.reference == hit.reference)
                {
                    recalled.push((path.clone(), hit));
                }
            }
        }
        if !searched {
            return Ok(Self::empty(IndexState::Missing));
        }
        recalled.truncate(limit);
        let hits = self.hit_views(recalled)?;
        Ok(RelevantPatternsView {
            index: IndexState::Fresh,
            hits,
            note: RELEVANT_NOTE.into(),
        })
    }
    fn empty(index: IndexState) -> RelevantPatternsView {
        RelevantPatternsView {
            index,
            hits: vec![],
            note: STALE_NOTE.into(),
        }
    }
    fn hit_views(
        &self,
        recalled: Vec<(String, crate::knowledge_retrieval::RecallHit)>,
    ) -> Result<Vec<KnowledgeHitView>, LearningError> {
        if recalled.is_empty() {
            return Ok(vec![]);
        }
        let refs: Vec<RecordRef> = recalled.iter().map(|(_, h)| h.reference.clone()).collect();
        let read = self.store.read_targeted(&refs).map_err(store_err)?;
        let mut out = Vec::new();
        for ((path, hit), item) in recalled.into_iter().zip(read.items) {
            let record = &item.stored.record;
            let statement = match record {
                KnowledgeRecord::VerifiedPattern(x) => x.statement.clone(),
                KnowledgeRecord::ApprovedProcedure(x) => match self.store.read(&x.pattern) {
                    Ok(p) => match p.record {
                        KnowledgeRecord::VerifiedPattern(v) => v.statement,
                        _ => String::new(),
                    },
                    Err(e) => return Err(store_err(e)),
                },
                _ => continue,
            };
            let m = &record.header().metadata;
            out.push(KnowledgeHitView {
                reference: hit.reference.clone(),
                kind: hit.reference.kind,
                title: first_line(&statement),
                snippet: statement.chars().take(SNIPPET_CHARS).collect(),
                source: SourceBadge {
                    source_id: hit.source.source_id.clone(),
                    source_version: hit.source.source_version.clone(),
                    source_commit: hit.source.source_commit.clone(),
                    file_digest: Some(hit.source.file_digest.clone()),
                    license: m.license.clone(),
                    privacy: "private".into(),
                    platforms: hit.platforms.clone(),
                    topics: hit.topics.clone(),
                },
                active: item.active,
                contradictory: item.contradictory,
                invalidated: !item.active,
                why_recalled: format!(
                    "{RELEVANT_LABEL} (`{path}`); {}",
                    truncate_utf8(&hit.why_recalled, 1024)
                ),
                mode: RecallMode::Current,
            });
        }
        Ok(out)
    }

    /// Explicit UI proposal: one Observation Evidence + one Candidate, only
    /// after build/test passed on the CURRENT manifest and full Accepted review.
    pub fn propose_candidate(
        &self,
        task: &TaskId,
        ev: UiProposeCandidateEvent,
    ) -> Result<CandidateProposalView, LearningError> {
        self.claim_event(&ev.ui_event_id)?;
        check_sha(&ev.change_set_sha256)?;
        let m = self.material(task)?;
        if ev.view_seq != m.view.view_seq || ev.change_set_sha256 != m.view.change_set_sha256 {
            return Err(LearningError::Conflict);
        }
        let gate = m.ready_gate()?;
        let id = m.identity()?;
        let changed = changed_files(&m.base, &m.current);
        let metadata = id.metadata(topics(&m.spec.primary, &m.view.objective))?;
        let now = now_ms().max(1);
        let header = |logical_id: String, reason: &str| RecordHeader {
            schema: KNOWLEDGE_SCHEMA.into(),
            logical_id,
            revision: 1,
            previous: None,
            timestamp_ms: now,
            audit: Audit {
                actor: "stage6-task-learning-ui".into(),
                reason: reason.into(),
            },
            metadata: metadata.clone(),
            edges: vec![],
        };
        let content = serde_json::to_string(&serde_json::json!({
            "schema": OBSERVATION_SCHEMA,
            "method": METHOD,
            "task_id": task.as_str(),
            "ui_event_id": ev.ui_event_id,
            "gate_run_id": gate.gate_run_id,
            "tree_sha256": gate.working_set_sha256,
            "plan_sha256": gate.plan_sha256,
            "workspace_profile_sha256": gate.workspace_profile_sha256,
            "gate_at_ms": gate.at_ms,
            "commands": gate.commands.iter().map(|c| serde_json::json!({
                "id": c.id,
                "role": c.role,
                "status": c.status,
                "termination": c.termination,
            })).collect::<Vec<_>>(),
            "base_manifest_sha256": id.base_manifest_sha256,
            "changed_files": changed,
            "file_contents_included": false,
        }))
        .map_err(|_| LearningError::Persistence)?;
        let evidence = self
            .store
            .create(KnowledgeRecord::Evidence(Evidence {
                header: header(
                    format!("task-learning-gate-{}", random_hex16()),
                    "passing coding-task gate summary (ids, exit codes, tree sha; no file contents)",
                ),
                evidence_kind: EvidenceKind::Observation,
                content_sha256: digest(content.as_bytes()),
                content,
            }))
            .map_err(store_err)?
            .reference;
        let statement = format!(
            "{}\nChanged files: {}",
            index_safe(truncate_utf8(m.view.objective.trim(), MAX_OBJECTIVE_BYTES)),
            changed.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        let candidate = self
            .store
            .create(KnowledgeRecord::Candidate(Candidate {
                header: header(
                    format!("task-learning-{}", random_hex16()),
                    "explicit UI proposal from a passing, fully reviewed coding task",
                ),
                statement,
                evidence: vec![evidence.clone()],
            }))
            .map_err(store_err)?
            .reference;
        let plan = self.plan(task, &m, &gate)?;
        Ok(CandidateProposalView {
            candidate,
            evidence: vec![evidence],
            verification: support_of(&m.view, &plan),
        })
    }

    fn prepare_verification(
        &self,
        task: &TaskId,
        candidate: &RecordRef,
    ) -> Result<Prepared, LearningError> {
        check_ref(candidate, KnowledgeKind::Candidate)?;
        let m = self.material(task)?;
        let gate = m.ready_gate()?;
        let id = m.identity()?;
        let latest = self
            .store
            .read_latest(&candidate.logical_id)
            .map_err(store_err)?;
        if latest.mapping.reference != *candidate {
            return Err(LearningError::Conflict);
        }
        let read = self
            .store
            .read_targeted(std::slice::from_ref(candidate))
            .map_err(store_err)?;
        let item = read
            .items
            .into_iter()
            .next()
            .ok_or(LearningError::NotFound)?;
        let KnowledgeRecord::Candidate(c) = &item.stored.record else {
            return Err(LearningError::Invalid);
        };
        if !item.active || item.contradictory || !id.admits(&c.header.metadata) {
            return Err(LearningError::Conflict);
        }
        let plan = self.plan(task, &m, &gate)?;
        let support = support_of(&m.view, &plan);
        let preview = VerificationPreview {
            candidate: candidate.clone(),
            recipe_sha256: recipe_sha256(&plan.recipe)?,
            recipe: plan.recipe,
            support,
        };
        Ok(Prepared {
            preview,
            material: m,
            identity: id,
        })
    }

    /// The exact recipe (and its sha) `verify_candidate` would run, so the user
    /// confirms what will execute. Read-only.
    pub fn verification_preview(
        &self,
        task: &TaskId,
        candidate: &RecordRef,
    ) -> Result<VerificationPreview, LearningError> {
        Ok(self.prepare_verification(task, candidate)?.preview)
    }

    /// Stage 4 verification of the confirmed recipe over freshly materialized
    /// base/current snapshots. Unsupported (non-Linux / non-representable)
    /// returns `state: "unsupported"` before anything is written or spawned.
    pub fn verify_candidate(
        &self,
        task: &TaskId,
        ev: UiVerifyCandidateEvent,
    ) -> Result<LearningVerificationView, LearningError> {
        self.claim_event(&ev.ui_event_id)?;
        check_sha(&ev.recipe_sha256)?;
        let prepared = self.prepare_verification(task, &ev.candidate)?;
        if let VerificationSupport::Unsupported { reason } = &prepared.preview.support {
            return Ok(unsupported_view(reason.clone()));
        }
        if ev.recipe_sha256 != prepared.preview.recipe_sha256 {
            return Err(LearningError::Conflict);
        }
        let verifier = self.verifier()?;
        let pair = self.snapshot_pair(&prepared.material, &prepared.identity)?;
        let (key, cancel) = self.register()?;
        let result = verifier.verify_and_promote(
            &ev.candidate,
            &pair.baseline,
            &pair.fixed,
            &prepared.preview.recipe,
            &cancel,
        );
        drop(pair);
        self.unregister(key);
        let report = result.map_err(verify_err)?;
        let checks = self.check_summaries(&report.checks)?;
        let mut residuals = residuals();
        if checks
            .first()
            .is_some_and(|c| c.role == "baseline" && c.termination != "completed")
        {
            residuals.insert(
                0,
                format!(
                    "the baseline run ended `{}` (not completed), so it does not count as a \
                     failing baseline: oracles must fail with <= {MAX_CASE_OUTPUT} bytes per \
                     stream within the run limits",
                    checks[0].termination
                ),
            );
        }
        Ok(LearningVerificationView {
            state: match report.actual_checked_state {
                ActualCheckedState::Verified => "verified",
                ActualCheckedState::Failed => "failed",
                ActualCheckedState::Cancelled => "cancelled",
            }
            .into(),
            pattern: report.pattern,
            procedure_run: Some(report.procedure_run),
            run_sha256: Some(report.run_sha256),
            policy_sha256: Some(report.policy_sha256),
            checks,
            approved: None,
            residuals,
        })
    }

    /// UI-only approval of the exact displayed pattern/run/policy tuple over a
    /// fresh capture of the CURRENT working set (`approve_from_ui`).
    pub fn approve_pattern(
        &self,
        task: &TaskId,
        ev: UiApprovePatternEvent,
    ) -> Result<LearningVerificationView, LearningError> {
        self.claim_event(&ev.ui_event_id)?;
        check_ref(&ev.pattern, KnowledgeKind::VerifiedPattern)?;
        check_ref(&ev.procedure_run, KnowledgeKind::Evidence)?;
        check_sha(&ev.displayed_run_sha256)?;
        check_sha(&ev.displayed_policy_sha256)?;
        let m = self.material(task)?;
        if let Some(reason) = execution_unsupported(&m.view) {
            return Ok(unsupported_view(reason));
        }
        let id = m.identity()?;
        let pattern = self.store.read(&ev.pattern).map_err(store_err)?;
        let KnowledgeRecord::VerifiedPattern(x) = &pattern.record else {
            return Err(LearningError::Invalid);
        };
        if !id.admits(&x.header.metadata) {
            return Err(LearningError::Conflict);
        }
        let checks = x.checks.clone();
        let verifier = self.verifier()?;
        let work = self.workdir()?;
        let root = work.dir.join("current");
        let snap = work.dir.join("snap");
        materialize(&root, &m.current)?;
        make_private_dir(&snap)?;
        let policy = SnapshotPolicy::new(vec![root.clone()], snap).map_err(iso_err)?;
        let current = Self::capture(
            &policy,
            &root,
            id.source(&id.current_primary_sha256),
            &m.spec.primary,
            &m.current,
        )?;
        let approved = verifier
            .approve_from_ui(
                UiApprovalEvent {
                    pattern: ev.pattern.clone(),
                    procedure_run: ev.procedure_run.clone(),
                    displayed_run_sha256: ev.displayed_run_sha256.clone(),
                    displayed_policy_sha256: ev.displayed_policy_sha256.clone(),
                },
                &current,
            )
            .map_err(verify_err)?;
        Ok(LearningVerificationView {
            state: "approved".into(),
            pattern: Some(ev.pattern),
            procedure_run: Some(ev.procedure_run),
            run_sha256: Some(ev.displayed_run_sha256),
            policy_sha256: Some(ev.displayed_policy_sha256),
            checks: self.check_summaries(&checks)?,
            approved: Some(approved),
            residuals: residuals(),
        })
    }

    /// UI-only append-only revocation of this task's approved procedure
    /// (`revoke_from_ui`).
    pub fn revoke_pattern(
        &self,
        task: &TaskId,
        ev: UiRevokePatternEvent,
    ) -> Result<LearningVerificationView, LearningError> {
        self.claim_event(&ev.ui_event_id)?;
        check_ref(&ev.approved, KnowledgeKind::ApprovedProcedure)?;
        let m = self.material(task)?;
        if let Some(reason) = execution_unsupported(&m.view) {
            return Ok(unsupported_view(reason));
        }
        let id = m.identity()?;
        let approved = self.store.read(&ev.approved).map_err(store_err)?;
        let KnowledgeRecord::ApprovedProcedure(x) = &approved.record else {
            return Err(LearningError::Invalid);
        };
        if !id.same_base(&x.header.metadata) {
            return Err(LearningError::Conflict);
        }
        let verifier = self.verifier()?;
        let invalidation = verifier.revoke_from_ui(&ev.approved).map_err(verify_err)?;
        let mut residuals = vec![format!(
            "revoked by invalidation record {} revision {}",
            invalidation.logical_id, invalidation.revision
        )];
        residuals.extend(self::residuals());
        Ok(LearningVerificationView {
            state: "revoked".into(),
            pattern: Some(x.pattern.clone()),
            procedure_run: x.outcome_evidence.first().cloned(),
            run_sha256: None,
            policy_sha256: None,
            checks: vec![],
            approved: Some(ev.approved),
            residuals,
        })
    }
}

// ===========================================================================
// Pure unit tests (all targets; no process, no vault)
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_task::ports::{CopyOutMode, CopyOutRequest, GatePlan, WorkspaceLimits};
    use crate::coding_task::RepairBudget;
    use crate::task_ledger::{
        BlobKind, BlobRef, CommandSummary, LabelSource, OracleVisibility, RepositoryLabel,
    };

    fn entry(id: &str, stream: &str, bytes: &[u8]) -> Vec<u8> {
        let mut out = format!("== {id} {stream} {} bytes\n", bytes.len()).into_bytes();
        out.extend_from_slice(bytes);
        out.push(b'\n');
        out
    }

    #[test]
    fn gate_log_parse_is_length_prefixed_and_stops_at_a_cut_tail() {
        let mut log = entry("a", "stdout", b"one\n== b stdout 3 bytes\nxyz");
        log.extend(entry("a", "stderr", b""));
        log.extend(entry("b", "stdout", b"two"));
        let parsed = parse_gate_log(&log);
        assert_eq!(
            parsed[&("a".to_owned(), "stdout".to_owned())],
            b"one\n== b stdout 3 bytes\nxyz".to_vec(),
            "embedded separators stay data"
        );
        assert_eq!(parsed[&("a".to_owned(), "stderr".to_owned())], b"".to_vec());
        assert_eq!(
            parsed[&("b".to_owned(), "stdout".to_owned())],
            b"two".to_vec()
        );
        // The run cap cut the tail: the header claims 10 bytes, 4 follow.
        let mut cut = entry("a", "stdout", b"ok");
        cut.extend_from_slice(b"== b stdout 10 bytes\nabcd");
        let parsed = parse_gate_log(&cut);
        assert_eq!(parsed.len(), 1, "only complete entries");
        assert!(parse_gate_log(b"garbage\n").is_empty());
    }

    #[test]
    fn query_terms_are_bounded_deduplicated_alias_free_and_covered_by_topics() {
        let terms = query_terms(
            "pkg/trip.py",
            "Fix the trip: miles->km, Locking factor 1.6!",
        );
        assert_eq!(
            terms,
            ["trip", "fix", "the", "trip:", "miles", "km", "factor", "1", "6"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            "Stage 3 tokenizer; alias source `locking` dropped"
        );
        let long = (0..40)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let bounded = query_terms("a.py", &long);
        assert_eq!(bounded.len(), MAX_QUERY_TERMS);
        assert!(bounded.join(" ").chars().count() <= MAX_QUERY_CHARS);
        let huge = "x".repeat(65);
        assert_eq!(query_terms("a.py", &huge), vec!["a".to_owned()]);
        let t = topics("pkg/trip.py", "Fix the trip");
        assert_eq!(t[0], TOPIC_MARKER);
        for term in query_terms("pkg/trip.py", "Fix the trip") {
            assert!(t.contains(&term), "every primary query term is a topic");
        }
        let unique: BTreeSet<&String> = t.iter().collect();
        assert_eq!(unique.len(), t.len(), "topics are unique (Stage 2 rule)");
    }

    fn limits() -> WorkspaceLimits {
        WorkspaceLimits {
            cpu_seconds: 30,
            memory_bytes: 256 * 1024 * 1024,
            processes: 16,
            work_tmpfs_bytes: 32 * 1024 * 1024,
            file_size_bytes: 1024 * 1024,
            open_files: 64,
            total_timeout_ms: 120_000,
            rss_watchdog_bytes: 1024 * 1024 * 1024,
        }
    }
    fn script(id: &str, path: &str, args: &[&str]) -> GateCommand {
        GateCommand::PythonScript {
            id: id.into(),
            role: GateRole::Oracle,
            script: path.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            timeout_ms: 30_000,
            output_bytes: 16 * 1024,
        }
    }
    fn files(pairs: &[(&str, &str)]) -> Files {
        pairs
            .iter()
            .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
            .collect()
    }
    fn spec(commands: Vec<GateCommand>, oracle: &[&str], selected: &Files) -> TaskSpec {
        TaskSpec {
            objective: "fix the trip report".into(),
            repository: RepositoryLabel {
                display_root: "/synthetic".into(),
                source_id: "repo:synthetic".into(),
                branch: None,
                head_commit: None,
                label_source: LabelSource::Unknown,
            },
            selected_files: selected.keys().cloned().collect(),
            primary: "trip.py".into(),
            oracle_files: oracle.iter().map(|s| s.to_string()).collect(),
            oracle_visibility: OracleVisibility::Hidden,
            acceptance: vec![],
            gate_plan: GatePlan {
                schema: crate::coding_task::ports::GATE_PLAN_SCHEMA.into(),
                commands,
                stop_on_failure: false,
                copy_out: CopyOutRequest {
                    mode: CopyOutMode::Off,
                    ignore_dir_names: CopyOutRequest::default_ignores(),
                    max_files: 16,
                    max_total_bytes: 1024 * 1024,
                },
                limits: limits(),
            },
            preview: None,
            repair: RepairBudget::default(),
        }
    }
    fn summary(id: &str, status: i32, out: &str, err: &str) -> CommandSummary {
        CommandSummary {
            id: id.into(),
            role: GateRole::Oracle,
            argv: vec![],
            status: Some(status),
            termination: Termination::Completed,
            stdout_total_bytes: out.len() as u64,
            stderr_total_bytes: err.len() as u64,
            stdout_retained_bytes: out.len() as u64,
            stderr_retained_bytes: err.len() as u64,
            truncated: false,
            log_sha256: "0".repeat(64),
            excerpt: String::new(),
        }
    }
    /// (gate record, blob store) for (id, exit, stdout, stderr) results.
    fn gate(results: &[(&str, i32, &str, &str)]) -> (GateRecord, BTreeMap<String, Vec<u8>>) {
        let mut log = Vec::new();
        for (id, _, out, err) in results {
            log.extend(entry(id, "stdout", out.as_bytes()));
            log.extend(entry(id, "stderr", err.as_bytes()));
        }
        let sha = digest(&log);
        let record = GateRecord {
            gate_run_id: "gate-1".into(),
            working_set_sha256: "1".repeat(64),
            plan_sha256: "2".repeat(64),
            workspace_profile_sha256: "3".repeat(64),
            commands: results
                .iter()
                .map(|(id, s, o, e)| summary(id, *s, o, e))
                .collect(),
            logs: vec![BlobRef {
                kind: BlobKind::GateLog,
                sha256: sha.clone(),
                size: log.len() as u64,
            }],
            termination: Termination::Completed,
            elapsed_ms: 1,
            at_ms: 1,
        };
        (record, [(sha, log)].into_iter().collect())
    }
    const ORACLES: [&str; 3] = [
        "checks/a_trip.py",
        "checks/negative_b.py",
        "checks/regression_c.py",
    ];
    fn fixture(fixed_distance: &str) -> (Files, Files) {
        let mut base = vec![
            ("trip.py", "from distance import f\n"),
            ("distance.py", "def f(x):\n    return x * 1.6\n"),
        ];
        for o in ORACLES {
            base.push((o, "print('synthetic')\n"));
        }
        let mut current = base.clone();
        current[1] = ("distance.py", fixed_distance);
        (files(&base), files(&current))
    }

    #[test]
    fn recipe_maps_python_script_oracles_to_exact_stage4_cases() {
        let (base, current) = fixture("def f(x):\n    return x * 1.609344\n");
        let commands = vec![
            script("negative-b", ORACLES[1], &[]),
            script("a-trip", ORACLES[0], &["/tmp/trip.out", "50"]),
            script("regression-c", ORACLES[2], &[]),
        ];
        let s = spec(commands, &ORACLES, &current);
        let (record, blobs) = gate(&[
            ("negative-b", 3, "rejected\n", ""),
            ("a-trip", 0, "trip km=80.467\n", ""),
            ("regression-c", 0, "zero ok\n", "note\n"),
        ]);
        let plan = build_recipe(&s, &base, &current, &record, |sha| {
            blobs.get(sha).cloned().ok_or(LearningError::NotFound)
        })
        .unwrap();
        assert_eq!(plan.unsupported, None);
        let r = &plan.recipe;
        assert_eq!(r.schema, RECIPE_SCHEMA);
        assert_eq!(r.repetitions, 2);
        let names: Vec<&str> = r.cases.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["a-trip", "negative-b", "regression-c"],
            "first positive moved first"
        );
        assert_eq!(r.cases[0].kind, CaseKind::Positive);
        assert_eq!(r.cases[1].kind, CaseKind::Negative);
        assert_eq!(r.cases[2].kind, CaseKind::Regression);
        assert_eq!(
            r.cases[0].argv,
            [
                "/usr/bin/python3",
                "-I",
                "/work/checks/a_trip.py",
                "/tmp/trip.out",
                "50"
            ]
        );
        assert_eq!(r.cases[0].expected_stdout, "trip km=80.467\n");
        assert_eq!(
            r.cases[0].expected_files,
            [("/tmp/trip.out".to_owned(), digest(b"trip km=80.467\n"))]
                .into_iter()
                .collect()
        );
        assert_eq!(r.cases[1].expected_status, 3);
        assert_eq!(r.cases[2].expected_stderr, "note\n");
        assert_eq!(
            r.oracle_files,
            ORACLES
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            r.implementation_files,
            ["distance.py", "trip.py"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>()
        );
        assert!(r.validate(&preview_binding(&current, "trip.py")).is_ok());
        let again = build_recipe(&s, &base, &current, &record, |sha| {
            blobs.get(sha).cloned().ok_or(LearningError::NotFound)
        })
        .unwrap();
        assert_eq!(
            recipe_sha256(&again.recipe).unwrap(),
            recipe_sha256(r).unwrap(),
            "deterministic recipe hash"
        );
    }

    #[test]
    fn recipe_shapes_stage4_cannot_represent_are_unsupported_with_reasons() {
        let (base, current) = fixture("def f(x):\n    return x * 1.609344\n");
        let ok = [
            ("a-trip", 0, "t\n", ""),
            ("negative-b", 0, "n\n", ""),
            ("regression-c", 0, "r\n", ""),
        ];
        let run = |commands: Vec<GateCommand>,
                   results: &[(&str, i32, &str, &str)],
                   base: &Files,
                   current: &Files| {
            let s = spec(commands, &ORACLES, current);
            let (record, blobs) = gate(results);
            let mut reads = 0;
            let plan = build_recipe(&s, base, current, &record, |sha| {
                reads += 1;
                blobs.get(sha).cloned().ok_or(LearningError::NotFound)
            })
            .unwrap();
            (plan.unsupported.unwrap_or_default(), reads)
        };
        let three = || {
            vec![
                script("a-trip", ORACLES[0], &["/tmp/trip.out"]),
                script("negative-b", ORACLES[1], &[]),
                script("regression-c", ORACLES[2], &[]),
            ]
        };
        // unittest-only oracle: refused on shape, before any gate log is read.
        let unittest = vec![GateCommand::PythonUnittest {
            id: "oracle".into(),
            role: GateRole::Oracle,
            start_dir: "checks".into(),
            pattern: "*.py".into(),
            timeout_ms: 30_000,
            output_bytes: 1024,
        }];
        let (reason, reads) = run(unittest, &[], &base, &current);
        assert!(reason.contains("python_unittest") && reason.contains("PythonScript"));
        assert_eq!(reads, 0);
        let (reason, _) = run(three()[..2].to_vec(), &ok[..2], &base, &current);
        assert!(reason.contains("3-4 cases"), "{reason}");
        let mut no_negative = three();
        no_negative[1] = script("other-b", ORACLES[1], &[]);
        let (reason, _) = run(
            no_negative,
            &[ok[0], ("other-b", 0, "n\n", ""), ok[2]],
            &base,
            &current,
        );
        assert!(reason.contains("negative"), "{reason}");
        let mut no_output = three();
        no_output[0] = script("a-trip", ORACLES[0], &["/tmp/../etc/x"]);
        let (reason, _) = run(no_output, &ok, &base, &current);
        assert!(reason.contains("/tmp/<name>"), "{reason}");
        let (reason, _) = run(
            three(),
            &[("a-trip", 1, "t\n", ""), ok[1], ok[2]],
            &base,
            &current,
        );
        assert!(reason.contains("non-zero exit"), "{reason}");
        let big = "x".repeat(MAX_CASE_OUTPUT + 1);
        let (reason, _) = run(
            three(),
            &[("a-trip", 0, big.as_str(), ""), ok[1], ok[2]],
            &base,
            &current,
        );
        assert!(reason.contains("truncated or larger"), "{reason}");
        let (reason, _) = run(three(), &ok, &base, &base);
        assert!(
            reason.contains("no implementation file changed"),
            "{reason}"
        );
        let mut grown = current.clone();
        grown.insert("extra.py".into(), b"x = 1\n".to_vec());
        let (reason, _) = run(three(), &ok, &base, &grown);
        assert!(reason.contains("same file set"), "{reason}");
        // A gate log cut before the command's entry is not guessed.
        let s = spec(three(), &ORACLES, &current);
        let (mut record, blobs) = gate(&ok);
        record.logs.clear();
        let plan = build_recipe(&s, &base, &current, &record, |sha| {
            blobs.get(sha).cloned().ok_or(LearningError::NotFound)
        })
        .unwrap();
        assert!(plan.unsupported.unwrap().contains("not retained"));
    }

    #[test]
    fn errors_are_fixed_words_and_events_deny_unknown_fields() {
        let words: Vec<String> = [
            LearningError::Locked,
            LearningError::NotFound,
            LearningError::Conflict,
            LearningError::Invalid,
            LearningError::Unsupported,
            LearningError::NotReady,
            LearningError::Persistence,
        ]
        .iter()
        .map(|e| e.to_string())
        .collect();
        assert_eq!(
            words,
            [
                "locked",
                "not_found",
                "conflict",
                "invalid",
                "unsupported",
                "not_ready",
                "persistence"
            ]
        );
        assert!(serde_json::from_str::<UiProposeCandidateEvent>(
            r#"{"view_seq":1,"change_set_sha256":"a","ui_event_id":"b","extra":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<UiRevokePatternEvent>(
            r#"{"approved":{"logical_id":"x","revision":1,"kind":"approved_procedure","content_digest":"a"},"ui_event_id":"b","checked":true}"#
        )
        .is_err());
        let support = serde_json::to_value(VerificationSupport::Supported { cases: 3 }).unwrap();
        assert_eq!(
            support,
            serde_json::json!({"kind": "supported", "cases": 3})
        );
        assert_eq!(
            serde_json::to_value(IndexState::Stale).unwrap(),
            serde_json::json!("stale")
        );
        assert!(check_event_id(&"a".repeat(32)).is_ok());
        assert!(check_event_id(&"A".repeat(32)).is_err());
        assert!(is_tmp_output("/tmp/trip.out") && !is_tmp_output("/tmp/a/b"));
        assert!(!is_tmp_output("/tmp/") && !is_tmp_output("/work/x") && !is_tmp_output("/tmp/."));
        assert_eq!(run_limits().output_bytes, MAX_CASE_OUTPUT);
        assert!(run_limits().validate().is_ok());
    }

    #[test]
    fn index_safe_breaks_only_overlong_term_runs() {
        let sha = "a".repeat(64);
        assert_eq!(
            index_safe(&format!("keep {sha} ok")),
            format!("keep {sha} ok")
        );
        let long = "b".repeat(150);
        let safe = index_safe(&format!("x {long}"));
        assert!(lexical_terms(&safe)
            .iter()
            .all(|t| t.chars().count() <= MAX_TERM_RUN));
        assert_eq!(
            safe.replace(' ', ""),
            format!("x{long}"),
            "only spaces inserted"
        );
    }
}

// ===========================================================================
// End-to-end (Linux): real disposable Vault, production CodingTaskService,
// real bwrap gates and real Stage 4 verification. Every proposer is SCRIPTED
// (labelled: pre-written patches, not a model score) and every UI event is
// SYNTHETIC (built by the test from what the view displayed, not a human).
// The fixture (trip distance/speed conversion) differs from every held-out
// fixture (quantity/invoice) and from the Stage 5 temperature/duration one.
// ===========================================================================
#[cfg(all(test, target_os = "linux"))]
mod e2e_tests {
    use super::*;
    use crate::coding_task::ports::{
        CopyOutMode, CopyOutRequest, GatePlan, ProposedEdit, UiReviewEvent, WorkspaceLimits,
        GATE_PLAN_SCHEMA,
    };
    use crate::coding_task::{
        GateTarget, OpenTaskRequest, PlatformClass, ProposerError, RepairBudget, RepairContext,
        RepairProposer, ServiceConfig,
    };
    use crate::task_ledger::test_support::vault_fixture;
    use crate::task_ledger::{
        AcceptanceCriterion, CommandSummary, CriterionCheck, EditOrigin, OracleVisibility,
    };
    use std::collections::VecDeque;

    type Shared = Arc<Mutex<Option<Vault>>>;

    const TRIP: &str = "\"\"\"Synthetic Stage 6 fixture: trip distance/speed report (not a held-out case).\"\"\"\nfrom distance import miles_to_km\nfrom speed import kmh_to_mps\n\n\ndef trip_report(miles, hours):\n    km = miles_to_km(miles)\n    return km, kmh_to_mps(km / hours)\n";
    const DISTANCE_BUG: &str = "def miles_to_km(miles):\n    return miles * 1.6\n";
    const DISTANCE_FIX: &str = "def miles_to_km(miles):\n    return miles * 1.609344\n";
    const SPEED_BUG: &str = "def kmh_to_mps(kmh):\n    if kmh < 0:\n        raise ValueError(\"negative speed\")\n    return kmh / 3.0\n";
    const SPEED_FIX: &str = "def kmh_to_mps(kmh):\n    if kmh < 0:\n        raise ValueError(\"negative speed\")\n    return kmh / 3.6\n";
    /// Positive oracle: writes its stdout bytes to the /tmp output argument and
    /// fails tersely (Stage 4 output cap, see `MAX_CASE_OUTPUT`).
    const POSITIVE: &str = "import sys\n\nsys.path.insert(0, \"/work\")\nfrom trip import trip_report  # noqa: E402\n\nkm, mps = trip_report(50, 2)\nif abs(km - 80.4672) > 1e-6 or abs(mps - 11.176) > 1e-6:\n    print(\"trip mismatch\")\n    sys.exit(1)\ntext = \"trip km=%.3f mps=%.3f\\n\" % (km, mps)\nwith open(sys.argv[1], \"w\") as out:\n    out.write(text)\nsys.stdout.write(text)\n";
    const NEGATIVE: &str = "import sys\n\nsys.path.insert(0, \"/work\")\nfrom speed import kmh_to_mps  # noqa: E402\n\ntry:\n    kmh_to_mps(-1)\nexcept ValueError:\n    print(\"rejected negative speed\")\n    sys.exit(3)\nraise AssertionError(\"negative speed accepted\")\n";
    const REGRESSION: &str = "import sys\n\nsys.path.insert(0, \"/work\")\nfrom trip import trip_report  # noqa: E402\n\nkm, mps = trip_report(0, 1)\nif km != 0 or mps != 0:\n    print(\"zero trip mismatch\")\n    sys.exit(1)\nprint(\"zero trip ok\")\n";
    const UNITTEST: &str = "import unittest\n\nfrom trip import trip_report\n\n\nclass Trip(unittest.TestCase):\n    def test_trip(self):\n        km, mps = trip_report(50, 2)\n        self.assertAlmostEqual(km, 80.4672)\n        self.assertAlmostEqual(mps, 11.176)\n";
    const OBJECTIVE: &str =
        "fix the miles to km factor and the km/h to m/s divisor in the trip report";
    const SCRIPT_FILES: [&str; 6] = [
        "trip.py",
        "distance.py",
        "speed.py",
        "checks/positive_trip.py",
        "checks/negative_speed.py",
        "checks/regression_zero.py",
    ];

    struct Env {
        _vault_temp: tempfile::TempDir,
        _base: tempfile::TempDir,
        base: PathBuf,
        vault: Shared,
        vault_root: PathBuf,
        root: PathBuf,
        scratch: PathBuf,
        learn: PathBuf,
        worktrees: PathBuf,
    }
    fn env(files: &[(&str, &str)]) -> Env {
        let (vault_temp, vault, vault_root) = vault_fixture();
        let temp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(temp.path()).unwrap();
        let root = base.join("repo");
        let worktrees = base.join("wt");
        std::fs::create_dir_all(&worktrees).unwrap();
        for (path, text) in files {
            let target = root.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, text).unwrap();
        }
        Env {
            _vault_temp: vault_temp,
            _base: temp,
            vault,
            vault_root,
            root,
            scratch: base.join("s"),
            learn: base.join("k"),
            worktrees,
            base,
        }
    }
    fn service(e: &Env, platform: PlatformClass) -> Arc<CodingTaskService> {
        Arc::new(CodingTaskService::new(
            e.vault.clone(),
            ServiceConfig {
                scratch: e.scratch.clone(),
                worktree_base: e.worktrees.clone(),
                worktree_deny_within: vec![e.vault_root.clone()],
                platform,
                host_commands_enabled: false,
            },
        ))
    }
    fn limits() -> WorkspaceLimits {
        WorkspaceLimits {
            cpu_seconds: 30,
            memory_bytes: 256 * 1024 * 1024,
            processes: 16,
            work_tmpfs_bytes: 32 * 1024 * 1024,
            file_size_bytes: 1024 * 1024,
            open_files: 64,
            total_timeout_ms: 120_000,
            rss_watchdog_bytes: 1024 * 1024 * 1024,
        }
    }
    fn plan(oracles: Vec<GateCommand>) -> GatePlan {
        let mut commands = vec![GateCommand::PythonCompile {
            id: "build".into(),
            role: GateRole::Build,
            files: vec!["trip.py".into(), "distance.py".into(), "speed.py".into()],
            timeout_ms: 30_000,
            output_bytes: 16 * 1024,
        }];
        commands.extend(oracles);
        GatePlan {
            schema: GATE_PLAN_SCHEMA.into(),
            commands,
            stop_on_failure: false,
            copy_out: CopyOutRequest {
                mode: CopyOutMode::Off,
                ignore_dir_names: CopyOutRequest::default_ignores(),
                max_files: 16,
                max_total_bytes: 1024 * 1024,
            },
            limits: limits(),
        }
    }
    fn script(id: &str, path: &str, args: &[&str]) -> GateCommand {
        GateCommand::PythonScript {
            id: id.into(),
            role: GateRole::Oracle,
            script: path.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            timeout_ms: 30_000,
            output_bytes: 16 * 1024,
        }
    }
    fn criterion(command: &str, exit: i32) -> AcceptanceCriterion {
        AcceptanceCriterion {
            id: format!("{command}-ok"),
            text: format!("synthetic criterion: {command} exits {exit}"),
            check: CriterionCheck::GateCommand {
                command_id: command.into(),
                expected_exit: exit,
            },
            confirmed_by_user: true,
        }
    }
    fn request(
        e: &Env,
        files: &[&str],
        plan: GatePlan,
        acceptance: Vec<AcceptanceCriterion>,
    ) -> OpenTaskRequest {
        OpenTaskRequest {
            root: e.root.clone(),
            files: files.iter().map(|s| s.to_string()).collect(),
            primary: "trip.py".into(),
            oracle_files: vec![],
            oracle_visibility: OracleVisibility::Hidden,
            objective: OBJECTIVE.into(),
            acceptance,
            gate_plan: plan,
            preview: None,
            repair: RepairBudget {
                max_attempts: 2,
                max_total_gate_ms: 10 * 60 * 1000,
            },
            allowed_new_prefixes: vec![],
        }
    }
    fn replace(path: &str, content: &str) -> ProposedEdit {
        ProposedEdit::Replace {
            path: path.into(),
            content: content.into(),
        }
    }
    fn set(xs: &[&str]) -> BTreeSet<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }
    fn cmd<'a>(v: &'a TaskView, id: &str) -> &'a CommandSummary {
        v.gates
            .last()
            .unwrap()
            .commands
            .iter()
            .find(|c| c.id == id)
            .unwrap()
    }
    /// SYNTHETIC UI review (Accepted) echoing exactly what the view displayed.
    fn accept(v: &TaskView, path: &str) -> UiReviewEvent {
        let d = v.diff.iter().find(|d| d.path == path).unwrap();
        UiReviewEvent {
            task_id: v.task_id.clone(),
            view_seq: v.view_seq,
            path: path.into(),
            decision: FileDecision::Accepted,
            displayed_base_sha256: d.base_sha256.clone(),
            displayed_new_sha256: d.new_sha256.clone(),
        }
    }
    /// SYNTHETIC UI "Save as candidate" event (view_seq + change-set hash shown).
    fn propose(v: &TaskView) -> UiProposeCandidateEvent {
        UiProposeCandidateEvent {
            view_seq: v.view_seq,
            change_set_sha256: v.change_set_sha256.clone(),
            ui_event_id: random_hex16(),
        }
    }
    /// SYNTHETIC UI "Approve for reuse" event with the displayed run/policy hashes.
    fn approve(r: &LearningVerificationView) -> UiApprovePatternEvent {
        UiApprovePatternEvent {
            pattern: r.pattern.clone().unwrap(),
            procedure_run: r.procedure_run.clone().unwrap(),
            displayed_run_sha256: r.run_sha256.clone().unwrap(),
            displayed_policy_sha256: r.policy_sha256.clone().unwrap(),
            ui_event_id: random_hex16(),
        }
    }
    /// SCRIPTED proposer (labelled): pre-written patches in order. Not a model;
    /// it proves nothing about one.
    struct ScriptedPatches(Mutex<VecDeque<Vec<ProposedEdit>>>);
    impl RepairProposer for ScriptedPatches {
        fn propose(&self, _ctx: &RepairContext) -> Result<Vec<ProposedEdit>, ProposerError> {
            self.0
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(ProposerError::Unavailable)
        }
    }
    fn kinds(store: &EvidenceVault) -> Vec<KnowledgeKind> {
        store
            .catalog()
            .unwrap()
            .iter()
            .map(|m| m.reference.kind)
            .collect()
    }
    fn fake(kind: KnowledgeKind) -> RecordRef {
        RecordRef {
            logical_id: "synthetic-ref".into(),
            revision: 2,
            kind,
            content_digest: "0".repeat(64),
        }
    }
    fn dir_entries(dir: &Path) -> usize {
        std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
    }

    #[test]
    fn k2_e2e_two_module_fix_propose_verify_approve_recall_revoke() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "k2_e2e_two_module_fix_propose_verify_approve_recall_revoke",
        ) {
            return;
        }
        let e = env(&[
            ("trip.py", TRIP),
            ("distance.py", DISTANCE_BUG),
            ("speed.py", SPEED_BUG),
            ("checks/positive_trip.py", POSITIVE),
            ("checks/negative_speed.py", NEGATIVE),
            ("checks/regression_zero.py", REGRESSION),
        ]);
        let tasks = service(&e, PlatformClass::Linux);
        assert!(
            matches!(
                tasks.capability(),
                IsolationCapability::RuntimeVerified { .. }
            ),
            "real readiness probe must pass on this host"
        );
        let store = EvidenceVault::new(e.vault.clone());
        store.initialize().unwrap(); // explicit knowledge initialization (K1 UI action)
        let retriever = KnowledgeRetriever::new(e.vault.clone());
        let learning = TaskLearning::new(e.vault.clone(), tasks.clone(), e.learn.clone());

        let v = tasks
            .open_task(request(
                &e,
                &SCRIPT_FILES,
                plan(vec![
                    script(
                        "positive-trip",
                        "checks/positive_trip.py",
                        &["/tmp/trip.out"],
                    ),
                    script("negative-speed", "checks/negative_speed.py", &[]),
                    script("regression-zero", "checks/regression_zero.py", &[]),
                ]),
                vec![
                    criterion("build", 0),
                    criterion("positive-trip", 0),
                    criterion("negative-speed", 3),
                    criterion("regression-zero", 0),
                ],
            ))
            .unwrap();
        let task = TaskId::parse(&v.task_id).unwrap();
        assert_eq!(
            v.oracle.protected,
            set(&SCRIPT_FILES[3..]),
            "oracle scripts are protected"
        );

        // ---- propose refused before checks pass / before review
        assert!(matches!(
            learning.propose_candidate(&task, propose(&v)),
            Err(LearningError::NotReady)
        ));
        let v = tasks.run_gate(&task, GateTarget::Current).unwrap();
        assert_eq!(
            cmd(&v, "positive-trip").status,
            Some(1),
            "real failing oracle"
        );
        assert_eq!(cmd(&v, "negative-speed").status, Some(3));
        assert_eq!(cmd(&v, "regression-zero").status, Some(0));
        assert!(matches!(v.outcome.test_status, CheckStatus::Failed { .. }));
        assert!(matches!(
            learning.propose_candidate(&task, propose(&v)),
            Err(LearningError::NotReady)
        ));
        let scripted = ScriptedPatches(Mutex::new(VecDeque::from([vec![
            replace("distance.py", DISTANCE_FIX),
            replace("speed.py", SPEED_FIX),
        ]])));
        let v = tasks.run_repair_loop(&task, &scripted).unwrap();
        assert!(matches!(v.outcome.test_status, CheckStatus::Passed { .. }));
        assert!(matches!(v.outcome.build_status, CheckStatus::Passed { .. }));
        assert_eq!(cmd(&v, "positive-trip").status, Some(0));
        assert!(
            matches!(
                learning.propose_candidate(&task, propose(&v)),
                Err(LearningError::NotReady)
            ),
            "checks passed but review pending"
        );
        let v = tasks.review_file(&task, accept(&v, "distance.py")).unwrap();
        let v = tasks.review_file(&task, accept(&v, "speed.py")).unwrap();
        let mut stale = propose(&v);
        stale.view_seq -= 1;
        assert!(matches!(
            learning.propose_candidate(&task, stale),
            Err(LearningError::Conflict)
        ));
        let mut wrong = propose(&v);
        wrong.change_set_sha256 = "0".repeat(64);
        assert!(matches!(
            learning.propose_candidate(&task, wrong),
            Err(LearningError::Conflict)
        ));
        let mut bad = propose(&v);
        bad.ui_event_id = "not-an-event-id".into();
        assert!(matches!(
            learning.propose_candidate(&task, bad),
            Err(LearningError::Invalid)
        ));
        assert!(kinds(&store).is_empty(), "refusals write nothing");

        // ---- propose: Observation evidence + Candidate, nothing promoted
        let proposal = learning.propose_candidate(&task, propose(&v)).unwrap();
        assert_eq!(
            proposal.verification,
            VerificationSupport::Supported { cases: 3 }
        );
        assert_eq!(
            kinds(&store),
            vec![KnowledgeKind::Evidence, KnowledgeKind::Candidate]
        );
        let gate_id = v.gates.last().unwrap().gate_run_id.clone();
        let observation = match store.read(&proposal.evidence[0]).unwrap().record {
            KnowledgeRecord::Evidence(x) => x,
            _ => panic!("observation evidence expected"),
        };
        assert_eq!(observation.evidence_kind, EvidenceKind::Observation);
        assert!(observation.content.contains(&gate_id));
        assert!(observation.content.contains("negative-speed"));
        for content in [DISTANCE_FIX, SPEED_FIX, POSITIVE] {
            assert!(!observation
                .content
                .contains(content.lines().last().unwrap().trim()));
        }
        let candidate_record = match store.read(&proposal.candidate).unwrap().record {
            KnowledgeRecord::Candidate(x) => x,
            _ => panic!("candidate expected"),
        };
        assert!(candidate_record.statement.starts_with(OBJECTIVE));
        assert!(candidate_record
            .statement
            .ends_with("Changed files: distance.py, speed.py"));
        assert_eq!(
            candidate_record.header.metadata.source_id,
            v.repository.source_id
        );
        let base_manifest: BTreeMap<String, String> = [
            ("trip.py", TRIP),
            ("distance.py", DISTANCE_BUG),
            ("speed.py", SPEED_BUG),
            ("checks/positive_trip.py", POSITIVE),
            ("checks/negative_speed.py", NEGATIVE),
            ("checks/regression_zero.py", REGRESSION),
        ]
        .iter()
        .map(|(p, c)| (p.to_string(), digest(c.as_bytes())))
        .collect();
        assert_eq!(
            candidate_record.header.metadata.source_commit,
            manifest_sha256(&base_manifest),
            "source_commit = manifest sha of the captured base selection"
        );

        // ---- simulated non-Linux (Windows) over the SAME ledger and vault:
        // search works; verify/approve/revoke are unsupported before any spawn.
        let win_scratch = e.base.join("k-win");
        let win = TaskLearning::new(
            e.vault.clone(),
            service(&e, PlatformClass::NonLinux),
            win_scratch.clone(),
        );
        let before = store.catalog().unwrap().len();
        let preview = win
            .verification_preview(&task, &proposal.candidate)
            .unwrap();
        assert!(
            matches!(&preview.support, VerificationSupport::Unsupported { reason } if reason.contains(NON_LINUX_REASON))
        );
        let r = win
            .verify_candidate(
                &task,
                UiVerifyCandidateEvent {
                    candidate: proposal.candidate.clone(),
                    recipe_sha256: preview.recipe_sha256.clone(),
                    ui_event_id: random_hex16(),
                },
            )
            .unwrap();
        assert_eq!(r.state, "unsupported");
        assert!(r.pattern.is_none() && r.procedure_run.is_none() && r.checks.is_empty());
        let r = win
            .approve_pattern(
                &task,
                UiApprovePatternEvent {
                    pattern: fake(KnowledgeKind::VerifiedPattern),
                    procedure_run: fake(KnowledgeKind::Evidence),
                    displayed_run_sha256: "0".repeat(64),
                    displayed_policy_sha256: "0".repeat(64),
                    ui_event_id: random_hex16(),
                },
            )
            .unwrap();
        assert_eq!(r.state, "unsupported");
        let r = win
            .revoke_pattern(
                &task,
                UiRevokePatternEvent {
                    approved: fake(KnowledgeKind::ApprovedProcedure),
                    ui_event_id: random_hex16(),
                },
            )
            .unwrap();
        assert_eq!(r.state, "unsupported");
        assert!(matches!(
            win.relevant_patterns(&task, 4).unwrap().index,
            IndexState::Missing
        ));
        assert_eq!(store.catalog().unwrap().len(), before, "nothing written");
        assert!(!win_scratch.exists(), "nothing materialized or spawned");

        // ---- relevant patterns: missing index, then explicit rebuild only
        let rel = learning.relevant_patterns(&task, 4).unwrap();
        assert!(matches!(rel.index, IndexState::Missing) && rel.hits.is_empty());
        retriever.rebuild().unwrap(); // EXPLICIT rebuild (the K1 UI button)
        let rel = learning.relevant_patterns(&task, 4).unwrap();
        assert!(matches!(rel.index, IndexState::Fresh));
        assert!(rel.hits.is_empty(), "a Candidate is not a pattern");
        assert!(matches!(
            learning.relevant_patterns(&task, 9),
            Err(LearningError::Invalid)
        ));

        // ---- preview: the exact recipe that will run
        let preview = learning
            .verification_preview(&task, &proposal.candidate)
            .unwrap();
        assert_eq!(preview.support, VerificationSupport::Supported { cases: 3 });
        let recipe = &preview.recipe;
        let names: Vec<&str> = recipe.cases.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["positive-trip", "negative-speed", "regression-zero"]
        );
        assert_eq!(
            recipe.cases[0].argv,
            [
                "/usr/bin/python3",
                "-I",
                "/work/checks/positive_trip.py",
                "/tmp/trip.out"
            ]
        );
        let stdout = "trip km=80.467 mps=11.176\n";
        assert_eq!(recipe.cases[0].expected_stdout, stdout);
        assert_eq!(
            recipe.cases[0].expected_files,
            [("/tmp/trip.out".to_owned(), digest(stdout.as_bytes()))]
                .into_iter()
                .collect()
        );
        assert_eq!(
            recipe
                .cases
                .iter()
                .map(|c| c.expected_status)
                .collect::<Vec<_>>(),
            [0, 3, 0]
        );
        assert_eq!(recipe.oracle_files, set(&SCRIPT_FILES[3..]));
        assert_eq!(recipe.implementation_files, set(&SCRIPT_FILES[..3]));
        assert_eq!(
            preview.recipe_sha256,
            digest(&serde_json::to_vec(recipe).unwrap())
        );

        // ---- recipe hash mismatch refused before anything runs or is written
        let before = store.catalog().unwrap().len();
        assert!(matches!(
            learning.verify_candidate(
                &task,
                UiVerifyCandidateEvent {
                    candidate: proposal.candidate.clone(),
                    recipe_sha256: "0".repeat(64),
                    ui_event_id: random_hex16(),
                },
            ),
            Err(LearningError::Conflict)
        ));
        assert_eq!(store.catalog().unwrap().len(), before);
        assert_eq!(dir_entries(&e.learn), 0, "nothing materialized");

        // ---- real Stage 4 verification: baseline fails, fixed passes twice
        let verified = learning
            .verify_candidate(
                &task,
                UiVerifyCandidateEvent {
                    candidate: proposal.candidate.clone(),
                    recipe_sha256: preview.recipe_sha256.clone(),
                    ui_event_id: random_hex16(),
                },
            )
            .unwrap();
        assert_eq!(verified.state, "verified");
        let pattern = verified.pattern.clone().unwrap();
        assert_eq!(pattern.kind, KnowledgeKind::VerifiedPattern);
        assert_eq!(pattern.logical_id, proposal.candidate.logical_id);
        assert_eq!(pattern.revision, 2);
        assert_eq!(
            verified.checks.len(),
            7,
            "1 baseline + 3 cases x 2 repetitions"
        );
        let baseline = &verified.checks[0];
        assert_eq!(
            (baseline.role.as_str(), baseline.case.as_str()),
            ("baseline", "positive-trip")
        );
        assert_eq!(baseline.status, Some(1), "real failing baseline exit");
        assert_eq!(baseline.termination, "completed");
        assert!(!baseline.passed);
        for c in &verified.checks[1..] {
            assert_eq!(c.role, "fixed");
            assert!(c.passed, "{} must pass", c.case);
            let expected = if c.case == "negative-speed" { 3 } else { 0 };
            assert_eq!(c.status, Some(expected));
        }
        assert_eq!(dir_entries(&e.learn), 0, "private scratch removed");
        eprintln!(
            "STAGE6_K2_VERIFY state={} checks={:?}",
            verified.state,
            verified
                .checks
                .iter()
                .map(|c| (c.role.clone(), c.case.clone(), c.status, c.passed))
                .collect::<Vec<_>>()
        );

        // ---- recall: stale until an explicit rebuild, then exact identity only
        let rel = learning.relevant_patterns(&task, 4).unwrap();
        assert!(matches!(rel.index, IndexState::Stale) && rel.hits.is_empty());
        retriever.rebuild().unwrap();
        let rel = learning.relevant_patterns(&task, 4).unwrap();
        assert!(matches!(rel.index, IndexState::Fresh));
        assert_eq!(rel.hits.len(), 1);
        let hit = &rel.hits[0];
        assert_eq!(hit.reference, pattern);
        assert_eq!(hit.kind, KnowledgeKind::VerifiedPattern);
        assert!(hit.why_recalled.starts_with(RELEVANT_LABEL));
        assert!(hit.why_recalled.contains("`trip.py`"));
        assert_eq!(
            hit.source.file_digest.as_deref(),
            Some(digest(TRIP.as_bytes()).as_str())
        );
        assert!(hit.active && !hit.contradictory && !hit.invalidated);
        assert!(OBJECTIVE.starts_with(&hit.title));

        // ---- approve: mismatching displayed hash refused; matching -> approved
        let mut mismatch = approve(&verified);
        mismatch.displayed_run_sha256 = "0".repeat(64);
        assert!(matches!(
            learning.approve_pattern(&task, mismatch),
            Err(LearningError::Conflict)
        ));
        let approved = learning.approve_pattern(&task, approve(&verified)).unwrap();
        assert_eq!(approved.state, "approved");
        let approved_ref = approved.approved.clone().unwrap();
        assert_eq!(approved_ref.kind, KnowledgeKind::ApprovedProcedure);
        assert_eq!(approved_ref.revision, 3);
        assert_eq!(approved.checks.len(), 7);
        assert!(matches!(
            store.read(&approved_ref).unwrap().record,
            KnowledgeRecord::ApprovedProcedure(_)
        ));
        retriever.rebuild().unwrap();
        let rel = learning.relevant_patterns(&task, 4).unwrap();
        assert_eq!(rel.hits.len(), 1);
        assert_eq!(
            rel.hits[0].reference, approved_ref,
            "approved head recalled"
        );

        // ---- changed file (SCRIPTED edit, labelled): no hit for a new identity
        tasks
            .propose_edit(
                &task,
                replace("trip.py", &format!("{TRIP}# scripted edit\n")),
                EditOrigin::User,
            )
            .unwrap();
        let rel = learning.relevant_patterns(&task, 4).unwrap();
        assert!(matches!(rel.index, IndexState::Fresh));
        assert!(rel.hits.is_empty(), "changed file -> no stale pattern");
        assert!(matches!(
            learning.verification_preview(&task, &proposal.candidate),
            Err(LearningError::NotReady)
        ));

        // ---- revoke -> invalidated; the exact identity no longer recalls it
        let revoked = learning
            .revoke_pattern(
                &task,
                UiRevokePatternEvent {
                    approved: approved_ref.clone(),
                    ui_event_id: random_hex16(),
                },
            )
            .unwrap();
        assert_eq!(revoked.state, "revoked");
        assert!(store.is_invalidated(&approved_ref).unwrap());
        tasks
            .propose_edit(&task, replace("trip.py", TRIP), EditOrigin::User)
            .unwrap();
        retriever.rebuild().unwrap();
        let rel = learning.relevant_patterns(&task, 4).unwrap();
        assert!(matches!(rel.index, IndexState::Fresh));
        assert!(rel.hits.is_empty(), "revoked pattern is never recalled");
    }

    #[test]
    fn k2_e2e_unittest_only_oracle_is_unsupported_before_spawn() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "k2_e2e_unittest_only_oracle_is_unsupported_before_spawn",
        ) {
            return;
        }
        let e = env(&[
            ("trip.py", TRIP),
            ("distance.py", DISTANCE_BUG),
            ("speed.py", SPEED_BUG),
            ("tests/__init__.py", "# synthetic oracle package\n"),
            ("tests/test_trip.py", UNITTEST),
        ]);
        let tasks = service(&e, PlatformClass::Linux);
        assert!(matches!(
            tasks.capability(),
            IsolationCapability::RuntimeVerified { .. }
        ));
        let store = EvidenceVault::new(e.vault.clone());
        store.initialize().unwrap();
        let learning = TaskLearning::new(e.vault.clone(), tasks.clone(), e.learn.clone());
        let v = tasks
            .open_task(request(
                &e,
                &[
                    "trip.py",
                    "distance.py",
                    "speed.py",
                    "tests/__init__.py",
                    "tests/test_trip.py",
                ],
                plan(vec![GateCommand::PythonUnittest {
                    id: "oracle".into(),
                    role: GateRole::Oracle,
                    start_dir: "tests".into(),
                    pattern: "test_*.py".into(),
                    timeout_ms: 30_000,
                    output_bytes: 16 * 1024,
                }]),
                vec![criterion("build", 0), criterion("oracle", 0)],
            ))
            .unwrap();
        let task = TaskId::parse(&v.task_id).unwrap();
        // SCRIPTED patches (labelled), applied as synthetic user edits.
        tasks
            .propose_edit(
                &task,
                replace("distance.py", DISTANCE_FIX),
                EditOrigin::User,
            )
            .unwrap();
        tasks
            .propose_edit(&task, replace("speed.py", SPEED_FIX), EditOrigin::User)
            .unwrap();
        let v = tasks.run_gate(&task, GateTarget::Current).unwrap();
        assert!(matches!(v.outcome.test_status, CheckStatus::Passed { .. }));
        let v = tasks.review_file(&task, accept(&v, "distance.py")).unwrap();
        let v = tasks.review_file(&task, accept(&v, "speed.py")).unwrap();
        let proposal = learning.propose_candidate(&task, propose(&v)).unwrap();
        let VerificationSupport::Unsupported { reason } = &proposal.verification else {
            panic!("unittest-only oracle must be unsupported");
        };
        assert!(reason.contains("python_unittest") && reason.contains("PythonScript"));
        let preview = learning
            .verification_preview(&task, &proposal.candidate)
            .unwrap();
        assert_eq!(preview.support, proposal.verification);
        assert!(preview.recipe.cases.is_empty());
        let r = learning
            .verify_candidate(
                &task,
                UiVerifyCandidateEvent {
                    candidate: proposal.candidate.clone(),
                    recipe_sha256: preview.recipe_sha256.clone(),
                    ui_event_id: random_hex16(),
                },
            )
            .unwrap();
        assert_eq!(r.state, "unsupported");
        assert_eq!(&r.residuals[0], reason);
        assert!(r.pattern.is_none() && r.checks.is_empty());
        assert_eq!(
            kinds(&store),
            vec![KnowledgeKind::Evidence, KnowledgeKind::Candidate],
            "no receipt, no promotion"
        );
        assert!(!e.learn.exists(), "nothing materialized, nothing spawned");
    }

    /// Positive oracle whose FAILURE is a long Python traceback (> 128 bytes).
    const POSITIVE_ASSERT: &str = "import sys\n\nsys.path.insert(0, \"/work\")\nfrom trip import trip_report  # noqa: E402\n\nkm, mps = trip_report(50, 2)\nassert abs(km - 80.4672) < 1e-6, \"km mismatch: %r (expected 80.4672 km for 50 miles)\" % km\nassert abs(mps - 11.176) < 1e-6, mps\ntext = \"trip km=%.3f mps=%.3f\\n\" % (km, mps)\nwith open(sys.argv[1], \"w\") as out:\n    out.write(text)\nsys.stdout.write(text)\n";

    #[test]
    fn k2_e2e_long_traceback_baseline_is_capped_and_index_stays_buildable() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "k2_e2e_long_traceback_baseline_is_capped_and_index_stays_buildable",
        ) {
            return;
        }
        let e = env(&[
            ("trip.py", TRIP),
            ("distance.py", DISTANCE_BUG),
            ("speed.py", SPEED_BUG),
            ("checks/positive_trip.py", POSITIVE_ASSERT),
            ("checks/negative_speed.py", NEGATIVE),
            ("checks/regression_zero.py", REGRESSION),
        ]);
        let tasks = service(&e, PlatformClass::Linux);
        assert!(matches!(
            tasks.capability(),
            IsolationCapability::RuntimeVerified { .. }
        ));
        let store = EvidenceVault::new(e.vault.clone());
        store.initialize().unwrap();
        let retriever = KnowledgeRetriever::new(e.vault.clone());
        let learning = TaskLearning::new(e.vault.clone(), tasks.clone(), e.learn.clone());
        let v = tasks
            .open_task(request(
                &e,
                &SCRIPT_FILES,
                plan(vec![
                    script(
                        "positive-trip",
                        "checks/positive_trip.py",
                        &["/tmp/trip.out"],
                    ),
                    script("negative-speed", "checks/negative_speed.py", &[]),
                    script("regression-zero", "checks/regression_zero.py", &[]),
                ]),
                vec![criterion("negative-speed", 3)],
            ))
            .unwrap();
        let task = TaskId::parse(&v.task_id).unwrap();
        // SCRIPTED patches (labelled), applied as synthetic user edits.
        tasks
            .propose_edit(
                &task,
                replace("distance.py", DISTANCE_FIX),
                EditOrigin::User,
            )
            .unwrap();
        tasks
            .propose_edit(&task, replace("speed.py", SPEED_FIX), EditOrigin::User)
            .unwrap();
        let v = tasks.run_gate(&task, GateTarget::Current).unwrap();
        assert!(matches!(v.outcome.test_status, CheckStatus::Passed { .. }));
        let v = tasks.review_file(&task, accept(&v, "distance.py")).unwrap();
        let v = tasks.review_file(&task, accept(&v, "speed.py")).unwrap();
        let proposal = learning.propose_candidate(&task, propose(&v)).unwrap();
        let preview = learning
            .verification_preview(&task, &proposal.candidate)
            .unwrap();
        assert_eq!(preview.support, VerificationSupport::Supported { cases: 3 });
        assert_eq!(preview.recipe.limits.output_bytes, MAX_CASE_OUTPUT);

        // Stage 6 merge: Stage 3 now skips over-long corpus terms, so the
        // former 128-byte cap is gone. The long-traceback baseline completes
        // with exit 1 (a clean failing baseline) and the fix verifies.
        let r = learning
            .verify_candidate(
                &task,
                UiVerifyCandidateEvent {
                    candidate: proposal.candidate.clone(),
                    recipe_sha256: preview.recipe_sha256.clone(),
                    ui_event_id: random_hex16(),
                },
            )
            .unwrap();
        assert_eq!(r.state, "verified", "{:?}", r.residuals);
        assert!(r.pattern.is_some());
        assert_eq!(r.checks[0].role, "baseline");
        assert_eq!(
            (r.checks[0].status, r.checks[0].termination.as_str()),
            (Some(1), "completed")
        );
        assert!(
            r.checks[1..].iter().all(|c| c.passed),
            "fixed cases still pass"
        );
        // REGRESSION (formerly a defect probe): a receipt holding a full
        // traceback hex term (>256 scalars) no longer blocks the index.
        retriever
            .rebuild()
            .expect("long receipt terms are skipped, not fatal");
        assert!(matches!(
            learning.relevant_patterns(&task, 4).unwrap().index,
            IndexState::Fresh
        ));
        eprintln!("STAGE6_K2_REGRESSION long_receipt_rebuild=ok");
    }
}
