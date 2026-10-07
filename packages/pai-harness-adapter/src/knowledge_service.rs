//! Stage 6 knowledge service facade (owner K1) over the ONE canonical encrypted
//! Stage 2 `EvidenceVault`, Stage 3 lexical retrieval and the bounded local
//! distiller. UI-facing, server-computed views only.
//!
//! Rules (stage6-design.md §0): nothing is promoted here — distillation writes
//! Evidence + Candidate only; VerifiedPattern only via the Stage 4 verifier and
//! ApprovedProcedure only via `KnowledgeVerifier::approve_from_ui`. Reject and
//! revoke are append-only Stage 2 invalidations. No network, model, training
//! export, plaintext corpus or file write outside the vault: export bundles are
//! returned to the caller. Errors are fixed payload-free classifications.
//! Strings that came from source documents are untrusted data: titles,
//! snippets and statements are display-sanitized; raw evidence content is
//! returned only as data for text rendering.

use crate::isolation::{hex, ActualRun, Termination};
use crate::knowledge::{self, EvidenceVault, KnowledgeStoreError, Loaded, RecordMapping};
use crate::knowledge::{StoredKnowledge, MAX_CATALOG_ENTRIES};
use crate::knowledge_distiller::{self as distiller, display_text, lexical_terms, random_hex};
use crate::knowledge_retrieval::{
    ApplicabilityFence, KnowledgeRetriever, NormalizationAudit, QueryKind, RecallMode,
    RetrievalError, RetrievalQuery, TrustedSource, DECLARED_ALIASES, FENCE_SCHEMA,
};
use crate::knowledge_verification::{VerificationCase, RECEIPT_SCHEMA};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use unoone_capability_contracts::knowledge::*;
use unoone_vault_core::Vault;

pub use crate::knowledge_distiller::{
    distill_request_sha256, Citation, ContradictionView, DistillBudget, DistillReport,
    DistillRequest, DistillRunSummary, DistillSource, DistilledCandidate, ExcludedSource,
    UiDistillEvent, DISTILL_METHOD,
};

pub const EXPORT_SCHEMA: &str = "inbharat.pai.knowledge-export.v1";
pub const TRAINING_EXPORT: &str = "disabled: separate consent and licence review required";
const MAX_QUERY_CHARS: usize = 256;
const MAX_SEARCH_LIMIT: usize = 16;
const MAX_LIST_LIMIT: usize = 100;
const MAX_EXPORT_REFERENCES: usize = 64;
const MAX_EXPORT_BYTES: usize = 8 * 1024 * 1024;
const MAX_REASON_CHARS: usize = 1024;
const MAX_TITLE_CHARS: usize = 120;
const MAX_SNIPPET_CHARS: usize = 280;
const MAX_DETAIL_CONTENT_CHARS: usize = 16 * 1024;
const MAX_INCOMING_EDGES: usize = 256;
const MAX_GRAPH_DEPTH: usize = 128;
const MAX_EVENT_IDS: usize = 4096;
const NORMALIZATION_ALGORITHM: &str =
    "unicode-scalar-lowercase/identifier-colon-underscore/and/v1; no embeddings";
const PROBE_TEXT: &str = "pai:index:status:probe";
const PROBE_PLATFORM: &str = "pai-index-status-probe";
const LIST_WHY: &str =
    "listed from the authenticated catalog, newest first; not a relevance ranking";
const VERIFICATION_RESIDUAL: &str = "verification summary is display-only: parsed from vault-authenticated Stage 4 receipts; receipt MACs, fences and promotion gates are re-checked only by the Stage 4 verifier";

// ------------------------------------------------------------------- types

/// Controller-supplied configuration (not caller JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeServiceConfig {
    /// Controller-private, for fd-safe capture.
    pub scratch: PathBuf,
    /// Held-out / evaluation content hashes (lowercase hex).
    pub excluded_sha256: BTreeSet<String>,
    /// Case-insensitive substrings of the relative path, e.g. "heldout".
    pub excluded_name_markers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeError {
    Locked,
    Uninitialized,
    NotFound,
    Conflict,
    Invalid,
    Limit,
    Unsupported,
    Corrupt,
    Persistence,
}
impl std::fmt::Display for KnowledgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Locked => "locked",
            Self::Uninitialized => "uninitialized",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Invalid => "invalid",
            Self::Limit => "limit",
            Self::Unsupported => "unsupported",
            Self::Corrupt => "corrupt",
            Self::Persistence => "persistence",
        })
    }
}
impl std::error::Error for KnowledgeError {}
type Result<T> = std::result::Result<T, KnowledgeError>;
type ExportPlan = (Vec<RecordRef>, Vec<(RecordRef, String)>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnowledgeStatus {
    pub initialized: bool,
    pub index: IndexState,
    /// Raw catalog revisions.
    pub catalog_entries: usize,
    /// Logical records (head revision) per kind:
    /// evidence|candidate|verified_pattern|approved_procedure|invalidation.
    pub counts: BTreeMap<String, usize>,
    /// Active logical records (head revision) per kind.
    pub active_counts: BTreeMap<String, usize>,
    /// Always `DISTILL_METHOD`.
    pub method: String,
    pub residuals: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexState {
    Fresh,
    Stale,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeQuery {
    /// <= 256 scalars.
    pub text: String,
    pub mode: RecallMode,
    pub platform: String,
    /// REQUIRED for current mode (Stage 3 rule); UI may omit -> historical only.
    pub trusted_source: Option<TrustedSource>,
    /// Empty = all.
    pub kinds: Vec<KnowledgeKind>,
    /// 1..=16.
    pub limit: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnowledgeSearchView {
    pub hits: Vec<KnowledgeHitView>,
    pub normalization: NormalizationAudit,
    pub index: IndexState,
    pub note: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnowledgeHitView {
    pub reference: RecordRef,
    pub kind: KnowledgeKind,
    /// First line of statement/content, <= 120 scalars, display-sanitized.
    pub title: String,
    pub snippet: String,
    pub source: SourceBadge,
    pub active: bool,
    pub contradictory: bool,
    pub invalidated: bool,
    pub why_recalled: String,
    pub mode: RecallMode,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceBadge {
    pub source_id: String,
    pub source_version: String,
    pub source_commit: String,
    pub file_digest: Option<String>,
    pub license: String,
    /// "private".
    pub privacy: String,
    pub platforms: Vec<String>,
    pub topics: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListState {
    All,
    Active,
    Invalidated,
    Contradictory,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeListFilter {
    pub kinds: Vec<KnowledgeKind>,
    pub state: ListState,
    pub offset: usize,
    /// 1..=100.
    pub limit: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnowledgeListView {
    pub items: Vec<KnowledgeHitView>,
    pub total: usize,
    pub offset: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnowledgeDetailView {
    pub reference: RecordRef,
    pub kind: KnowledgeKind,
    pub body: KnowledgeBody,
    pub source: SourceBadge,
    pub audit: Audit,
    pub timestamp_ms: u64,
    /// All revisions of the logical id, oldest first.
    pub history: Vec<RecordRef>,
    /// Outgoing + incoming (including derived inverse InvalidatedBy).
    pub edges: Vec<EdgeView>,
    pub active: bool,
    pub invalidated: bool,
    pub contradictory: bool,
    /// VerifiedPattern / ApprovedProcedure only.
    pub verification: Option<VerificationSummary>,
    /// Subset of ["reject","revoke_approval","export"]; server-computed.
    pub allowed_actions: Vec<String>,
    pub residuals: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KnowledgeBody {
    Evidence {
        evidence_kind: EvidenceKind,
        /// Bounded to <= 16 Ki scalars; `truncated` says so.
        content: String,
        truncated: bool,
        content_sha256: String,
    },
    Candidate {
        statement: String,
        evidence: Vec<RecordRef>,
    },
    VerifiedPattern {
        statement: String,
        candidate: RecordRef,
        checks: Vec<RecordRef>,
    },
    ApprovedProcedure {
        pattern: RecordRef,
        outcome_evidence: Vec<RecordRef>,
        approval_evidence: RecordRef,
    },
    Invalidation {
        target: RecordRef,
        reason: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EdgeView {
    pub relation: EdgeKind,
    pub target: RecordRef,
    /// "outgoing" | "incoming".
    pub direction: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerificationSummary {
    pub checks: Vec<CheckSummary>,
    pub repetitions: u32,
    pub approved: bool,
    pub approval: Option<RecordRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckSummary {
    pub reference: RecordRef,
    pub case: String,
    /// baseline | fixed.
    pub role: String,
    pub status: Option<i32>,
    pub termination: String,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiRejectEvent {
    pub target: RecordRef,
    /// <= 1024 scalars.
    pub reason: String,
    /// 32 lowercase hex.
    pub ui_event_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiRevokeEvent {
    pub approved: RecordRef,
    pub ui_event_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiInitEvent {
    pub ui_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    /// 1..=64.
    pub references: Vec<RecordRef>,
    pub include_evidence_content: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportPreview {
    pub request_sha256: String,
    pub items: Vec<ExportItemPreview>,
    /// (reference, reason).
    pub refused: Vec<(RecordRef, String)>,
    pub contains_private_content: bool,
    /// Always `TRAINING_EXPORT`.
    pub training_export: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportItemPreview {
    pub reference: RecordRef,
    pub kind: KnowledgeKind,
    pub title: String,
    pub licence: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiExportConsentEvent {
    pub request: ExportRequest,
    /// Must equal the preview's request_sha256.
    pub request_sha256: String,
    pub ui_event_id: String,
    pub acknowledged_private: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportBundle {
    /// `EXPORT_SCHEMA`.
    pub schema: String,
    pub json: String,
    /// SHA-256 of `json` bytes (deterministic for the same request and vault state).
    pub sha256: String,
    pub items: usize,
}

/// `sha256(canonical serde_json of ExportRequest)`, the preview/consent binding.
pub fn export_request_sha256(request: &ExportRequest) -> Result<String> {
    let bytes = serde_json::to_vec(request).map_err(|_| KnowledgeError::Invalid)?;
    Ok(knowledge::digest(&bytes))
}

// --------------------------------------------------------- internal helpers

pub(crate) fn store_error(e: KnowledgeStoreError) -> KnowledgeError {
    match e {
        KnowledgeStoreError::Locked => KnowledgeError::Locked,
        KnowledgeStoreError::Uninitialized => KnowledgeError::Uninitialized,
        KnowledgeStoreError::Conflict | KnowledgeStoreError::Invalidated => {
            KnowledgeError::Conflict
        }
        KnowledgeStoreError::NotFound => KnowledgeError::NotFound,
        KnowledgeStoreError::InvalidRecord => KnowledgeError::Invalid,
        KnowledgeStoreError::Corrupt | KnowledgeStoreError::UnsupportedSchema => {
            KnowledgeError::Corrupt
        }
        KnowledgeStoreError::Limit => KnowledgeError::Limit,
        KnowledgeStoreError::Persistence => KnowledgeError::Persistence,
    }
}
fn retrieval_error(e: RetrievalError) -> KnowledgeError {
    match e {
        RetrievalError::Store(e) => store_error(e),
        RetrievalError::UninitializedIndex | RetrievalError::StaleIndex => KnowledgeError::Conflict,
        RetrievalError::UnsupportedIndexSchema => KnowledgeError::Corrupt,
        RetrievalError::InvalidQuery | RetrievalError::MissingTrustedVersion => {
            KnowledgeError::Invalid
        }
        RetrievalError::IndexLimit => KnowledgeError::Limit,
    }
}
fn kind_key(kind: KnowledgeKind) -> &'static str {
    match kind {
        KnowledgeKind::Evidence => "evidence",
        KnowledgeKind::Candidate => "candidate",
        KnowledgeKind::VerifiedPattern => "verified_pattern",
        KnowledgeKind::ApprovedProcedure => "approved_procedure",
        KnowledgeKind::Invalidation => "invalidation",
    }
}
const ALL_KINDS: [KnowledgeKind; 5] = [
    KnowledgeKind::Evidence,
    KnowledgeKind::Candidate,
    KnowledgeKind::VerifiedPattern,
    KnowledgeKind::ApprovedProcedure,
    KnowledgeKind::Invalidation,
];
fn valid_event_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn status_residuals() -> Vec<String> {
    [
        "extraction is deterministic and shallow (headings, docstrings, first lines), not semantic understanding",
        "contradiction detection is a lexical heuristic; possible contradictions are reported only and never persisted as Contradicting edges (a persisted edge would mark both records contradictory permanently under Stage 2/3 semantics)",
        "current-mode retrieval needs exact trusted file identity, so the Explorer is mostly historical/audit",
        "search is Stage 3 lexical postings, not semantic; the index is rebuilt only explicitly",
        "no training export; export bundles are returned to the caller, never written by the service",
        "trusted-owner threat model and Stage 2/3 rollback residuals unchanged",
        "Stage 4 verification shapes limited to PythonScript oracles; verification unavailable on Windows",
        VERIFICATION_RESIDUAL,
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Flags {
    pub(crate) active: bool,
    pub(crate) invalidated: bool,
    pub(crate) contradictory: bool,
}

/// One authenticated full read (Stage 2 `load_catalog` + `load_all` under the
/// shared vault mutex) plus derived per-revision flags. No cache is retained.
pub(crate) struct Snapshot {
    pub(crate) entries: Vec<RecordMapping>,
    pub(crate) loaded: Loaded,
    flags: BTreeMap<RecordRef, Flags>,
    heads: BTreeMap<String, RecordRef>,
}
impl Snapshot {
    pub(crate) fn get(&self, reference: &RecordRef) -> Option<&StoredKnowledge> {
        self.loaded
            .get(&(reference.logical_id.clone(), reference.revision))
            .filter(|item| item.mapping.reference == *reference)
    }
    pub(crate) fn flags_of(&self, reference: &RecordRef) -> Result<Flags> {
        self.flags
            .get(reference)
            .copied()
            .ok_or(KnowledgeError::Corrupt)
    }
    pub(crate) fn is_head(&self, reference: &RecordRef) -> bool {
        self.heads.get(&reference.logical_id) == Some(reference)
    }
    fn record(&self, reference: &RecordRef) -> Result<&KnowledgeRecord> {
        self.get(reference)
            .map(|item| &item.record)
            .ok_or(KnowledgeError::Corrupt)
    }
}

/// Stage 2/3-equivalent flags for every revision, computed in one pass:
/// `active` mirrors `knowledge::require_active` (superseded root, or explicit
/// invalidation anywhere in the dependency closure, invalidations themselves
/// not expanded); `contradictory` mirrors `selected_from_loaded` (any
/// Contradicting edge touching the full dependency closure); `invalidated` is
/// the explicit-revocation half of inactivity (superseding alone is not it).
/// Parity with `EvidenceVault::read_targeted` is asserted by tests.
fn compute_flags(loaded: &Loaded) -> Result<BTreeMap<RecordRef, Flags>> {
    let mut revoked = BTreeSet::new();
    let mut superseded = BTreeSet::new();
    let mut touched = BTreeSet::new();
    for item in loaded.values() {
        if let KnowledgeRecord::Invalidation(x) = &item.record {
            revoked.insert(x.target.clone());
        }
        for edge in &item.record.header().edges {
            match edge.relation {
                EdgeKind::Superseding => {
                    superseded.insert(edge.target.clone());
                }
                EdgeKind::Contradicting => {
                    touched.insert(edge.target.clone());
                    touched.insert(item.mapping.reference.clone());
                }
                _ => {}
            }
        }
    }
    let lookup = |reference: &RecordRef| -> Result<&StoredKnowledge> {
        loaded
            .get(&(reference.logical_id.clone(), reference.revision))
            .filter(|item| item.mapping.reference == *reference)
            .ok_or(KnowledgeError::Corrupt)
    };
    let mut out = BTreeMap::new();
    for item in loaded.values() {
        let root = item.mapping.reference.clone();
        // Explicit invalidation (require_active traversal order and bounds).
        let mut invalidated = false;
        let mut seen = BTreeSet::new();
        let mut pending = vec![(root.clone(), 0usize)];
        while let Some((reference, depth)) = pending.pop() {
            if depth >= MAX_GRAPH_DEPTH || seen.len() >= MAX_CATALOG_ENTRIES {
                return Err(KnowledgeError::Limit);
            }
            if !seen.insert(reference.clone()) {
                continue;
            }
            let current = lookup(&reference)?;
            if revoked.contains(&reference)
                || current
                    .record
                    .header()
                    .edges
                    .iter()
                    .any(|e| e.relation == EdgeKind::InvalidatedBy)
            {
                invalidated = true;
                break;
            }
            if !matches!(current.record, KnowledgeRecord::Invalidation(_)) {
                pending.extend(
                    current
                        .record
                        .references()
                        .into_iter()
                        .map(|r| (r.clone(), depth + 1)),
                );
            }
        }
        // Contradiction over the full dependency closure (selected_from_loaded).
        let mut closure = BTreeSet::new();
        let mut pending = vec![(root.clone(), 0usize)];
        while let Some((reference, depth)) = pending.pop() {
            if depth >= MAX_GRAPH_DEPTH {
                return Err(KnowledgeError::Limit);
            }
            if closure.insert(reference.clone()) {
                let current = lookup(&reference)?;
                pending.extend(
                    current
                        .record
                        .references()
                        .into_iter()
                        .map(|r| (r.clone(), depth + 1)),
                );
            }
        }
        let contradictory = closure.iter().any(|r| touched.contains(r));
        let active = !superseded.contains(&root) && !invalidated;
        out.insert(
            root,
            Flags {
                active,
                invalidated,
                contradictory,
            },
        );
    }
    Ok(out)
}

fn take_snapshot(store: &EvidenceVault) -> Result<Snapshot> {
    let (entries, loaded) = store
        .with_vault(|vault| {
            let (catalog, _) = knowledge::load_catalog(vault)?;
            let loaded = knowledge::load_all(vault, &catalog)?;
            Ok((catalog.entries, loaded))
        })
        .map_err(store_error)?;
    let flags = compute_flags(&loaded)?;
    let heads = entries
        .iter()
        .map(|m| (m.reference.logical_id.clone(), m.reference.clone()))
        .collect();
    Ok(Snapshot {
        entries,
        loaded,
        flags,
        heads,
    })
}

fn badge(metadata: &KnowledgeMetadata) -> SourceBadge {
    let file_digest =
        serde_json::from_str::<ApplicabilityFence>(&metadata.applicability.constraints)
            .ok()
            .filter(|f| f.schema == FENCE_SCHEMA && valid_digest(&f.file_digest))
            .map(|f| f.file_digest);
    SourceBadge {
        source_id: metadata.source_id.clone(),
        source_version: metadata.source_version.clone(),
        source_commit: metadata.source_commit.clone(),
        file_digest,
        license: metadata.license.clone(),
        privacy: "private".into(),
        platforms: metadata.applicability.platforms.clone(),
        topics: metadata.applicability.topics.clone(),
    }
}
fn pattern_statement<'a>(snapshot: &'a Snapshot, pattern: &RecordRef) -> Option<&'a str> {
    match snapshot.get(pattern).map(|item| &item.record) {
        Some(KnowledgeRecord::VerifiedPattern(x)) => Some(&x.statement),
        _ => None,
    }
}
/// The record's primary human text (untrusted for source-derived records).
fn primary_text(snapshot: &Snapshot, record: &KnowledgeRecord) -> String {
    match record {
        KnowledgeRecord::Evidence(x) => x.content.clone(),
        KnowledgeRecord::Candidate(x) => x.statement.clone(),
        KnowledgeRecord::VerifiedPattern(x) => x.statement.clone(),
        KnowledgeRecord::ApprovedProcedure(x) => match pattern_statement(snapshot, &x.pattern) {
            Some(statement) => format!("approved procedure: {statement}"),
            None => format!(
                "approved procedure of {} r{}",
                x.pattern.logical_id, x.pattern.revision
            ),
        },
        KnowledgeRecord::Invalidation(x) => format!(
            "invalidation of {} {} r{}: {}",
            kind_key(x.target.kind),
            x.target.logical_id,
            x.target.revision,
            x.reason
        ),
    }
}
fn title_of(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    display_text(line, MAX_TITLE_CHARS)
}
fn hit_view(
    snapshot: &Snapshot,
    reference: &RecordRef,
    why_recalled: String,
    mode: RecallMode,
) -> Result<KnowledgeHitView> {
    let record = snapshot.record(reference)?;
    let flags = snapshot.flags_of(reference)?;
    let text = primary_text(snapshot, record);
    Ok(KnowledgeHitView {
        reference: reference.clone(),
        kind: reference.kind,
        title: title_of(&text),
        snippet: display_text(&text, MAX_SNIPPET_CHARS),
        source: badge(&record.header().metadata),
        active: flags.active,
        contradictory: flags.contradictory,
        invalidated: flags.invalidated,
        why_recalled,
        mode,
    })
}
fn normalization_for(text: &str) -> NormalizationAudit {
    let mut terms = BTreeSet::new();
    let mut aliases = BTreeSet::new();
    for mut term in lexical_terms(text) {
        if let Some((from, to)) = DECLARED_ALIASES.iter().find(|(from, _)| *from == term) {
            aliases.insert(format!("{from}->{to}"));
            term = (*to).into();
        }
        terms.insert(term);
    }
    NormalizationAudit {
        algorithm: NORMALIZATION_ALGORITHM.into(),
        terms: terms.into_iter().collect(),
        declared_aliases_used: aliases.into_iter().collect(),
    }
}

/// Mirror of the Stage 4 receipt envelope/body fields needed for display.
#[derive(Deserialize)]
struct CheckEnvelope {
    schema: String,
    logical_id: String,
    evidence_kind: EvidenceKind,
    body: CheckBody,
}
#[derive(Deserialize)]
struct CheckBody {
    baseline: bool,
    repetition: u32,
    case: VerificationCase,
    actual: ActualRun,
}
fn parse_check(snapshot: &Snapshot, reference: &RecordRef) -> Option<(CheckSummary, u32)> {
    let KnowledgeRecord::Evidence(e) = &snapshot.get(reference)?.record else {
        return None;
    };
    if e.evidence_kind != EvidenceKind::CheckResult {
        return None;
    }
    let envelope: CheckEnvelope = serde_json::from_str(&e.content).ok()?;
    if envelope.schema != RECEIPT_SCHEMA
        || envelope.logical_id != reference.logical_id
        || envelope.evidence_kind != EvidenceKind::CheckResult
    {
        return None;
    }
    let CheckBody {
        baseline,
        repetition,
        case,
        actual,
    } = envelope.body;
    // Same predicate as Stage 4 `case_passed`.
    let passed = actual.termination == Termination::Completed
        && actual.status == Some(case.expected_status)
        && actual.stdout_hex == hex(case.expected_stdout.as_bytes())
        && actual.stderr_hex == hex(case.expected_stderr.as_bytes())
        && actual.output_files == case.expected_files;
    let termination = serde_json::to_value(actual.termination)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    Some((
        CheckSummary {
            reference: reference.clone(),
            case: display_text(&case.name, 128),
            role: if baseline { "baseline" } else { "fixed" }.into(),
            status: actual.status,
            termination,
            passed,
        },
        repetition,
    ))
}
fn verification_summary(
    snapshot: &Snapshot,
    checks: &[RecordRef],
    approved: bool,
    approval: Option<RecordRef>,
) -> VerificationSummary {
    let mut summaries = Vec::new();
    let mut repetitions = 0;
    for check in checks {
        match parse_check(snapshot, check) {
            Some((summary, repetition)) => {
                repetitions = repetitions.max(repetition);
                summaries.push(summary);
            }
            None => summaries.push(CheckSummary {
                reference: check.clone(),
                case: "unparsed".into(),
                role: "unknown".into(),
                status: None,
                termination: "unparsed".into(),
                passed: false,
            }),
        }
    }
    VerificationSummary {
        checks: summaries,
        repetitions,
        approved,
        approval,
    }
}
/// Candidate-cited evidence behind an exportable record (pattern -> candidate).
fn cited_evidence(snapshot: &Snapshot, reference: &RecordRef) -> Result<Vec<RecordRef>> {
    let pattern = match snapshot.record(reference)? {
        KnowledgeRecord::VerifiedPattern(x) => x.clone(),
        KnowledgeRecord::ApprovedProcedure(x) => match snapshot.record(&x.pattern)? {
            KnowledgeRecord::VerifiedPattern(p) => p.clone(),
            _ => return Err(KnowledgeError::Corrupt),
        },
        _ => return Ok(vec![]),
    };
    match snapshot.record(&pattern.candidate)? {
        KnowledgeRecord::Candidate(c) => Ok(c.evidence.clone()),
        _ => Err(KnowledgeError::Corrupt),
    }
}

#[derive(Serialize)]
struct BundleDocument {
    schema: &'static str,
    training_export: &'static str,
    note: &'static str,
    include_evidence_content: bool,
    items: Vec<BundleItem>,
}
#[derive(Serialize)]
struct BundleItem {
    reference: RecordRef,
    kind: KnowledgeKind,
    title: String,
    statement: String,
    source: SourceBadge,
    audit: Audit,
    timestamp_ms: u64,
    verification: Option<VerificationSummary>,
    evidence: Vec<BundleEvidence>,
}
#[derive(Serialize)]
struct BundleEvidence {
    reference: RecordRef,
    evidence_kind: EvidenceKind,
    content_sha256: String,
    content: Option<String>,
}

#[derive(Default)]
struct EventLedger {
    used: BTreeSet<String>,
    order: VecDeque<String>,
}
/// Reserves a UI event id (double-submit/replay protection); released unless
/// committed after the state change succeeded.
struct EventClaim<'a> {
    ledger: &'a Mutex<EventLedger>,
    id: String,
    committed: bool,
}
impl EventClaim<'_> {
    fn commit(mut self) {
        self.committed = true;
    }
}
impl Drop for EventClaim<'_> {
    fn drop(&mut self) {
        let Ok(mut ledger) = self.ledger.lock() else {
            return;
        };
        if self.committed {
            ledger.order.push_back(self.id.clone());
            while ledger.order.len() > MAX_EVENT_IDS {
                if let Some(old) = ledger.order.pop_front() {
                    ledger.used.remove(&old);
                }
            }
        } else {
            ledger.used.remove(&self.id);
        }
    }
}

// ------------------------------------------------------------------ service

/// UI facade. Shares the product vault `Arc` (no second store). The only
/// in-memory state is the used-UI-event-id ledger and a lock epoch.
pub struct KnowledgeService {
    store: EvidenceVault,
    retriever: KnowledgeRetriever,
    config: KnowledgeServiceConfig,
    epoch: AtomicU64,
    events: Mutex<EventLedger>,
}

impl KnowledgeService {
    /// No I/O.
    pub fn new(vault: Arc<Mutex<Option<Vault>>>, config: KnowledgeServiceConfig) -> Self {
        let config = KnowledgeServiceConfig {
            excluded_sha256: config
                .excluded_sha256
                .iter()
                .map(|h| h.trim().to_ascii_lowercase())
                .collect(),
            ..config
        };
        Self {
            store: EvidenceVault::new(vault.clone()),
            retriever: KnowledgeRetriever::new(vault),
            config,
            epoch: AtomicU64::new(0),
            events: Mutex::new(EventLedger::default()),
        }
    }

    /// Call after the vault's emergency lock: aborts in-flight distillation
    /// before its next write and forgets used UI event ids.
    pub fn on_lock(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut ledger) = self.events.lock() {
            *ledger = EventLedger::default();
        }
    }

    fn claim(&self, id: &str) -> Result<EventClaim<'_>> {
        if !valid_event_id(id) {
            return Err(KnowledgeError::Invalid);
        }
        let mut ledger = self
            .events
            .lock()
            .map_err(|_| KnowledgeError::Persistence)?;
        if !ledger.used.insert(id.to_string()) {
            return Err(KnowledgeError::Conflict);
        }
        Ok(EventClaim {
            ledger: &self.events,
            id: id.to_string(),
            committed: false,
        })
    }

    fn snapshot(&self) -> Result<Snapshot> {
        take_snapshot(&self.store)
    }

    /// Read-only probe through Stage 3 search (same lock and authentication
    /// as a real search; a term/platform that matches no document).
    fn index_state(&self) -> Result<IndexState> {
        let probe = RetrievalQuery {
            text: PROBE_TEXT.into(),
            kind: QueryKind::Symbol,
            mode: RecallMode::Historical,
            platform: PROBE_PLATFORM.into(),
            trusted_source: None,
            topic: None,
            candidate_limit: 1,
            result_limit: 1,
            context_chars: 1024,
            snippet_chars: 1,
        };
        match self.retriever.search(&probe) {
            Ok(_) => Ok(IndexState::Fresh),
            Err(RetrievalError::StaleIndex) => Ok(IndexState::Stale),
            Err(RetrievalError::UninitializedIndex) => Ok(IndexState::Missing),
            Err(e) => Err(retrieval_error(e)),
        }
    }

    pub fn status(&self) -> Result<KnowledgeStatus> {
        let zero: BTreeMap<String, usize> = ALL_KINDS
            .iter()
            .map(|k| (kind_key(*k).to_string(), 0))
            .collect();
        let snapshot = match self.snapshot() {
            Ok(snapshot) => snapshot,
            Err(KnowledgeError::Uninitialized) => {
                return Ok(KnowledgeStatus {
                    initialized: false,
                    index: IndexState::Missing,
                    catalog_entries: 0,
                    counts: zero.clone(),
                    active_counts: zero,
                    method: DISTILL_METHOD.into(),
                    residuals: status_residuals(),
                })
            }
            Err(e) => return Err(e),
        };
        let index = self.index_state()?;
        let mut counts = zero.clone();
        let mut active_counts = zero;
        for reference in snapshot.heads.values() {
            *counts.entry(kind_key(reference.kind).into()).or_default() += 1;
            if snapshot.flags_of(reference)?.active {
                *active_counts
                    .entry(kind_key(reference.kind).into())
                    .or_default() += 1;
            }
        }
        Ok(KnowledgeStatus {
            initialized: true,
            index,
            catalog_entries: snapshot.entries.len(),
            counts,
            active_counts,
            method: DISTILL_METHOD.into(),
            residuals: status_residuals(),
        })
    }

    /// Explicit UI initialization (Stage 2 create-only, idempotent).
    pub fn initialize(&self, event: UiInitEvent) -> Result<KnowledgeStatus> {
        if !valid_event_id(&event.ui_event_id) {
            return Err(KnowledgeError::Invalid);
        }
        self.store.initialize().map_err(store_error)?;
        self.status()
    }

    /// Explicit Stage 3 rebuild (UI button); never implicit in search.
    pub fn rebuild_index(&self) -> Result<KnowledgeStatus> {
        self.retriever.rebuild().map_err(retrieval_error)?;
        self.status()
    }

    pub fn search(&self, query: KnowledgeQuery) -> Result<KnowledgeSearchView> {
        if query.text.chars().count() > MAX_QUERY_CHARS
            || !(1..=MAX_SEARCH_LIMIT).contains(&query.limit)
        {
            return Err(KnowledgeError::Invalid);
        }
        let kinds: BTreeSet<KnowledgeKind> = query.kinds.iter().copied().collect();
        let request = RetrievalQuery {
            text: query.text.clone(),
            kind: QueryKind::Terms,
            mode: query.mode,
            platform: query.platform.clone(),
            trusted_source: query.trusted_source.clone(),
            topic: None,
            candidate_limit: knowledge::MAX_TARGETED_CANDIDATES,
            result_limit: MAX_SEARCH_LIMIT,
            context_chars: 16384,
            snippet_chars: 32,
        };
        let mode_note = match query.mode {
            RecallMode::Current => "current mode: exact trusted source/file identity, logical heads, active and non-contradictory only",
            RecallMode::Historical => "historical results are audit-only and may be stale, revoked or contradictory",
        };
        let result = match self.retriever.search(&request) {
            Ok(result) => result,
            Err(e @ (RetrievalError::StaleIndex | RetrievalError::UninitializedIndex)) => {
                let (index, note) = if e == RetrievalError::StaleIndex {
                    (
                        IndexState::Stale,
                        "index stale: rebuild the index explicitly; no results until then",
                    )
                } else {
                    (
                        IndexState::Missing,
                        "index missing: rebuild the index explicitly; no results until then",
                    )
                };
                return Ok(KnowledgeSearchView {
                    hits: vec![],
                    normalization: normalization_for(&query.text),
                    index,
                    note: note.into(),
                });
            }
            Err(e) => return Err(retrieval_error(e)),
        };
        let snapshot = self.snapshot()?;
        let mut hits = Vec::new();
        for hit in &result.hits {
            if !kinds.is_empty() && !kinds.contains(&hit.reference.kind) {
                continue;
            }
            let flags = snapshot.flags_of(&hit.reference)?;
            // Defense in depth against a mutation between search and render.
            if query.mode == RecallMode::Current && (!flags.active || flags.contradictory) {
                continue;
            }
            hits.push(hit_view(
                &snapshot,
                &hit.reference,
                hit.why_recalled.clone(),
                query.mode,
            )?);
            if hits.len() >= query.limit {
                break;
            }
        }
        Ok(KnowledgeSearchView {
            hits,
            normalization: result.normalization,
            index: IndexState::Fresh,
            note: format!("lexical postings, not semantic; {mode_note}"),
        })
    }

    pub fn list(&self, filter: KnowledgeListFilter) -> Result<KnowledgeListView> {
        if !(1..=MAX_LIST_LIMIT).contains(&filter.limit) {
            return Err(KnowledgeError::Invalid);
        }
        let kinds: BTreeSet<KnowledgeKind> = filter.kinds.iter().copied().collect();
        let snapshot = self.snapshot()?;
        let mut selected = Vec::new();
        for mapping in snapshot.entries.iter().rev() {
            let reference = &mapping.reference;
            if !snapshot.is_head(reference)
                || (!kinds.is_empty() && !kinds.contains(&reference.kind))
            {
                continue;
            }
            let flags = snapshot.flags_of(reference)?;
            let keep = match filter.state {
                ListState::All => true,
                ListState::Active => flags.active,
                ListState::Invalidated => flags.invalidated,
                ListState::Contradictory => flags.contradictory,
            };
            if keep {
                selected.push(reference);
            }
        }
        let total = selected.len();
        let items = selected
            .into_iter()
            .skip(filter.offset)
            .take(filter.limit)
            .map(|r| hit_view(&snapshot, r, LIST_WHY.into(), RecallMode::Historical))
            .collect::<Result<Vec<_>>>()?;
        Ok(KnowledgeListView {
            items,
            total,
            offset: filter.offset,
        })
    }

    pub fn detail(&self, logical_id: &str, revision: Option<u32>) -> Result<KnowledgeDetailView> {
        valid_id(logical_id).map_err(|_| KnowledgeError::Invalid)?;
        if revision == Some(0) {
            return Err(KnowledgeError::Invalid);
        }
        let snapshot = self.snapshot()?;
        detail_from(&snapshot, logical_id, revision)
    }

    /// Append-only rejection (Stage 2 invalidation) of the exact displayed head
    /// revision of an active Evidence/Candidate/VerifiedPattern.
    pub fn reject(&self, event: UiRejectEvent) -> Result<KnowledgeDetailView> {
        let claim = self.claim(&event.ui_event_id)?;
        let reason = event.reason.trim();
        if reason.is_empty()
            || reason.chars().count() > MAX_REASON_CHARS
            || reason
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
            || event.target.validate().is_err()
            || !matches!(
                event.target.kind,
                KnowledgeKind::Evidence | KnowledgeKind::Candidate | KnowledgeKind::VerifiedPattern
            )
        {
            return Err(KnowledgeError::Invalid);
        }
        let snapshot = self.snapshot()?;
        let view = self.invalidate(
            &snapshot,
            &event.target,
            "reject",
            reason,
            format!("explicit UI rejection; ui_event {}", event.ui_event_id),
        )?;
        claim.commit();
        Ok(view)
    }

    /// Append-only revocation of the exact displayed active ApprovedProcedure
    /// head (same record shape as `KnowledgeVerifier::revoke_from_ui`, through
    /// Stage 2 directly so it also works where Stage 4 isolation is unavailable).
    pub fn revoke_approval(&self, event: UiRevokeEvent) -> Result<KnowledgeDetailView> {
        let claim = self.claim(&event.ui_event_id)?;
        if event.approved.validate().is_err()
            || event.approved.kind != KnowledgeKind::ApprovedProcedure
        {
            return Err(KnowledgeError::Invalid);
        }
        let snapshot = self.snapshot()?;
        let view = self.invalidate(
            &snapshot,
            &event.approved,
            "revoke",
            "explicit UI revocation",
            format!(
                "explicit procedure revocation; ui_event {}",
                event.ui_event_id
            ),
        )?;
        claim.commit();
        Ok(view)
    }

    fn invalidate(
        &self,
        snapshot: &Snapshot,
        target: &RecordRef,
        prefix: &str,
        reason: &str,
        audit_reason: String,
    ) -> Result<KnowledgeDetailView> {
        let Some(item) = snapshot.get(target) else {
            let known = snapshot.entries.iter().any(|m| {
                m.reference.logical_id == target.logical_id
                    && m.reference.revision == target.revision
            });
            return Err(if known {
                KnowledgeError::Conflict
            } else {
                KnowledgeError::NotFound
            });
        };
        if !snapshot.is_head(target) || !snapshot.flags_of(target)?.active {
            return Err(KnowledgeError::Conflict);
        }
        let mut header = item.record.header().clone();
        header.logical_id = format!("{prefix}-{}", random_hex()?);
        header.revision = 1;
        header.previous = None;
        header.edges.clear();
        header.timestamp_ms = distiller::now_ms()?;
        header.audit = Audit {
            actor: "knowledge-ui-controller".into(),
            reason: audit_reason,
        };
        self.store
            .invalidate(KnowledgeRecord::Invalidation(Invalidation {
                header,
                target: target.clone(),
                reason: reason.into(),
            }))
            .map_err(store_error)?;
        let snapshot = self.snapshot()?;
        detail_from(&snapshot, &target.logical_id, Some(target.revision))
    }

    /// (exportable references, refused (reference, reason)).
    fn export_plan(&self, snapshot: &Snapshot, request: &ExportRequest) -> Result<ExportPlan> {
        if request.references.is_empty()
            || request.references.len() > MAX_EXPORT_REFERENCES
            || request.references.iter().collect::<BTreeSet<_>>().len() != request.references.len()
            || request.references.iter().any(|r| r.validate().is_err())
        {
            return Err(KnowledgeError::Invalid);
        }
        let mut items = Vec::new();
        let mut refused = Vec::new();
        for reference in &request.references {
            let reason = if snapshot.get(reference).is_none() {
                Some("not_found")
            } else if !matches!(
                reference.kind,
                KnowledgeKind::VerifiedPattern | KnowledgeKind::ApprovedProcedure
            ) {
                Some("kind_not_exportable")
            } else {
                let flags = snapshot.flags_of(reference)?;
                if flags.invalidated {
                    Some("invalidated")
                } else if !flags.active {
                    Some("inactive")
                } else if flags.contradictory {
                    Some("contradictory")
                } else {
                    None
                }
            };
            match reason {
                Some(reason) => refused.push((reference.clone(), reason.to_string())),
                None => items.push(reference.clone()),
            }
        }
        Ok((items, refused))
    }

    pub fn export_preview(&self, request: ExportRequest) -> Result<ExportPreview> {
        let request_sha256 = export_request_sha256(&request)?;
        let snapshot = self.snapshot()?;
        let (items, refused) = self.export_plan(&snapshot, &request)?;
        let mut previews = Vec::new();
        let mut cites_evidence = false;
        for reference in &items {
            let record = snapshot.record(reference)?;
            cites_evidence |= !cited_evidence(&snapshot, reference)?.is_empty();
            previews.push(ExportItemPreview {
                reference: reference.clone(),
                kind: reference.kind,
                title: title_of(&primary_text(&snapshot, record)),
                licence: record.header().metadata.license.clone(),
            });
        }
        Ok(ExportPreview {
            request_sha256,
            items: previews,
            refused,
            contains_private_content: request.include_evidence_content && cites_evidence,
            training_export: TRAINING_EXPORT.into(),
        })
    }

    /// Consent-bound export; the bundle is returned (UI saves it via a user
    /// file dialog). Evidence content only with include + acknowledgement.
    pub fn export(&self, event: UiExportConsentEvent) -> Result<ExportBundle> {
        let claim = self.claim(&event.ui_event_id)?;
        if event.request_sha256 != export_request_sha256(&event.request)? {
            return Err(KnowledgeError::Conflict);
        }
        if event.request.include_evidence_content && !event.acknowledged_private {
            return Err(KnowledgeError::Invalid);
        }
        let snapshot = self.snapshot()?;
        let (items, _) = self.export_plan(&snapshot, &event.request)?;
        if items.is_empty() {
            return Err(KnowledgeError::Invalid);
        }
        let include = event.request.include_evidence_content && event.acknowledged_private;
        let mut bundle_items = Vec::new();
        for reference in &items {
            let record = snapshot.record(reference)?;
            let header = record.header();
            let text = primary_text(&snapshot, record);
            let statement = match record {
                KnowledgeRecord::VerifiedPattern(x) => x.statement.clone(),
                KnowledgeRecord::ApprovedProcedure(x) => pattern_statement(&snapshot, &x.pattern)
                    .ok_or(KnowledgeError::Corrupt)?
                    .to_string(),
                _ => return Err(KnowledgeError::Corrupt),
            };
            let mut evidence = Vec::new();
            for cited in cited_evidence(&snapshot, reference)? {
                let KnowledgeRecord::Evidence(e) = snapshot.record(&cited)? else {
                    return Err(KnowledgeError::Corrupt);
                };
                evidence.push(BundleEvidence {
                    reference: cited.clone(),
                    evidence_kind: e.evidence_kind,
                    content_sha256: e.content_sha256.clone(),
                    content: include.then(|| e.content.clone()),
                });
            }
            bundle_items.push(BundleItem {
                reference: reference.clone(),
                kind: reference.kind,
                title: title_of(&text),
                statement,
                source: badge(&header.metadata),
                audit: header.audit.clone(),
                timestamp_ms: header.timestamp_ms,
                verification: verification_for(&snapshot, record, reference)?,
                evidence,
            });
        }
        let document = BundleDocument {
            schema: EXPORT_SCHEMA,
            training_export: TRAINING_EXPORT,
            note: "private knowledge export: metadata, never execution authority; no training use",
            include_evidence_content: include,
            items: bundle_items,
        };
        let bytes = serde_json::to_vec(&document).map_err(|_| KnowledgeError::Corrupt)?;
        if bytes.len() > MAX_EXPORT_BYTES {
            return Err(KnowledgeError::Limit);
        }
        let json = String::from_utf8(bytes).map_err(|_| KnowledgeError::Corrupt)?;
        claim.commit();
        Ok(ExportBundle {
            schema: EXPORT_SCHEMA.into(),
            sha256: knowledge::digest(json.as_bytes()),
            json,
            items: items.len(),
        })
    }

    /// Runs a confirmed distillation (the UI showed the plan and its
    /// `distill_request_sha256`). Writes Evidence + Candidate only; the Stage 3
    /// index becomes stale until an explicit `rebuild_index`.
    pub fn distill(&self, request: DistillRequest, event: UiDistillEvent) -> Result<DistillReport> {
        let started = Instant::now();
        let claim = self.claim(&event.ui_event_id)?;
        let request_sha256 = distill_request_sha256(&request)?;
        if event.request_sha256 != request_sha256 {
            return Err(KnowledgeError::Conflict);
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        let snapshot = self.snapshot()?;
        let mut elapsed = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let alive = || self.epoch.load(Ordering::SeqCst) == epoch;
        let mut context = distiller::RunContext {
            store: &self.store,
            config: &self.config,
            request_sha256: &request_sha256,
            ui_event_id: &event.ui_event_id,
            elapsed_ms: &mut elapsed,
            alive: &alive,
        };
        let report = distiller::run(&mut context, &request, &snapshot)?;
        claim.commit();
        Ok(report)
    }

    pub fn distill_runs(&self) -> Result<Vec<DistillRunSummary>> {
        let snapshot = self.snapshot()?;
        Ok(distiller::run_summaries(&snapshot))
    }
}

fn verification_for(
    snapshot: &Snapshot,
    record: &KnowledgeRecord,
    reference: &RecordRef,
) -> Result<Option<VerificationSummary>> {
    Ok(match record {
        KnowledgeRecord::VerifiedPattern(x) => {
            // Approved iff an ACTIVE ApprovedProcedure revision approves this pattern.
            let approval = snapshot
                .loaded
                .values()
                .find_map(|item| match &item.record {
                    KnowledgeRecord::ApprovedProcedure(p)
                        if p.pattern == *reference
                            && snapshot
                                .flags_of(&item.mapping.reference)
                                .is_ok_and(|f| f.active) =>
                    {
                        Some(p.approval_evidence.clone())
                    }
                    _ => None,
                });
            Some(verification_summary(
                snapshot,
                &x.checks,
                approval.is_some(),
                approval,
            ))
        }
        KnowledgeRecord::ApprovedProcedure(x) => {
            let checks = match snapshot.record(&x.pattern)? {
                KnowledgeRecord::VerifiedPattern(p) => p.checks.clone(),
                _ => return Err(KnowledgeError::Corrupt),
            };
            let active = snapshot.flags_of(reference)?.active;
            Some(verification_summary(
                snapshot,
                &checks,
                active,
                Some(x.approval_evidence.clone()),
            ))
        }
        _ => None,
    })
}

fn detail_from(
    snapshot: &Snapshot,
    logical_id: &str,
    revision: Option<u32>,
) -> Result<KnowledgeDetailView> {
    let history: Vec<RecordRef> = snapshot
        .entries
        .iter()
        .filter(|m| m.reference.logical_id == logical_id)
        .map(|m| m.reference.clone())
        .collect();
    let reference = match revision {
        None => history.last().cloned(),
        Some(revision) => history.iter().find(|r| r.revision == revision).cloned(),
    }
    .ok_or(KnowledgeError::NotFound)?;
    let record = snapshot.record(&reference)?;
    let flags = snapshot.flags_of(&reference)?;
    let header = record.header();
    let mut residuals = vec![
        "metadata only: no record authorizes execution".to_string(),
        "source-derived text is untrusted data; render as plain text".to_string(),
    ];
    let body = match record {
        KnowledgeRecord::Evidence(x) => {
            let truncated = x.content.chars().count() > MAX_DETAIL_CONTENT_CHARS;
            if truncated {
                residuals.push("evidence content truncated to 16384 scalars".into());
            }
            KnowledgeBody::Evidence {
                evidence_kind: x.evidence_kind,
                content: x.content.chars().take(MAX_DETAIL_CONTENT_CHARS).collect(),
                truncated,
                content_sha256: x.content_sha256.clone(),
            }
        }
        KnowledgeRecord::Candidate(x) => KnowledgeBody::Candidate {
            statement: x.statement.clone(),
            evidence: x.evidence.clone(),
        },
        KnowledgeRecord::VerifiedPattern(x) => KnowledgeBody::VerifiedPattern {
            statement: x.statement.clone(),
            candidate: x.candidate.clone(),
            checks: x.checks.clone(),
        },
        KnowledgeRecord::ApprovedProcedure(x) => KnowledgeBody::ApprovedProcedure {
            pattern: x.pattern.clone(),
            outcome_evidence: x.outcome_evidence.clone(),
            approval_evidence: x.approval_evidence.clone(),
        },
        KnowledgeRecord::Invalidation(x) => KnowledgeBody::Invalidation {
            target: x.target.clone(),
            reason: x.reason.clone(),
        },
    };
    let mut edges: Vec<EdgeView> = header
        .edges
        .iter()
        .map(|e| EdgeView {
            relation: e.relation,
            target: e.target.clone(),
            direction: "outgoing".into(),
        })
        .collect();
    let mut incoming = 0;
    'scan: for mapping in &snapshot.entries {
        let other = snapshot.record(&mapping.reference)?;
        let mut found = Vec::new();
        if let KnowledgeRecord::Invalidation(x) = other {
            if x.target == reference {
                found.push(EdgeKind::InvalidatedBy);
            }
        }
        found.extend(
            other
                .header()
                .edges
                .iter()
                .filter(|e| e.target == reference)
                .map(|e| e.relation),
        );
        for relation in found {
            if incoming >= MAX_INCOMING_EDGES {
                residuals.push("incoming edges truncated at 256".into());
                break 'scan;
            }
            incoming += 1;
            edges.push(EdgeView {
                relation,
                target: mapping.reference.clone(),
                direction: "incoming".into(),
            });
        }
    }
    let verification = verification_for(snapshot, record, &reference)?;
    if verification.is_some() {
        residuals.push(VERIFICATION_RESIDUAL.into());
    }
    let head = snapshot.is_head(&reference);
    let mut allowed_actions = Vec::new();
    if head
        && flags.active
        && matches!(
            reference.kind,
            KnowledgeKind::Evidence | KnowledgeKind::Candidate | KnowledgeKind::VerifiedPattern
        )
    {
        allowed_actions.push("reject".to_string());
    }
    if head && flags.active && reference.kind == KnowledgeKind::ApprovedProcedure {
        allowed_actions.push("revoke_approval".to_string());
    }
    if flags.active
        && !flags.contradictory
        && matches!(
            reference.kind,
            KnowledgeKind::VerifiedPattern | KnowledgeKind::ApprovedProcedure
        )
    {
        allowed_actions.push("export".to_string());
    }
    Ok(KnowledgeDetailView {
        kind: reference.kind,
        body,
        source: badge(&header.metadata),
        audit: header.audit.clone(),
        timestamp_ms: header.timestamp_ms,
        history,
        edges,
        active: flags.active,
        invalidated: flags.invalidated,
        contradictory: flags.contradictory,
        verification,
        allowed_actions,
        residuals,
        reference,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge_distiller::{record_index_terms, RunContext, RULE_NEGATION};
    use std::path::Path;

    const PASSWORD: &[u8] = b"stage6-k1-synthetic-fixture-password";
    const PLATFORM: &str = "linux-x86_64";

    fn fixture() -> (tempfile::TempDir, Arc<Mutex<Option<Vault>>>) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UNOONE");
        Vault::create(&root, PASSWORD).unwrap();
        let mut vault = Vault::open(&root).unwrap();
        vault.unlock(PASSWORD).unwrap();
        (temp, Arc::new(Mutex::new(Some(vault))))
    }
    fn event(n: u8) -> String {
        format!("{n:032x}")
    }
    fn budget(max_candidates: usize) -> DistillBudget {
        DistillBudget {
            max_total_bytes: 1024 * 1024,
            max_candidates,
            deadline_ms: 30_000,
        }
    }
    fn pasted(label: &str, text: &str) -> DistillSource {
        DistillSource::PastedText {
            label: label.into(),
            text: text.into(),
        }
    }
    fn query(
        text: &str,
        mode: RecallMode,
        trusted_source: Option<TrustedSource>,
        kinds: Vec<KnowledgeKind>,
    ) -> KnowledgeQuery {
        KnowledgeQuery {
            text: text.into(),
            mode,
            platform: PLATFORM.into(),
            trusted_source,
            kinds,
            limit: 16,
        }
    }
    fn all(
        limit: usize,
        offset: usize,
        state: ListState,
        kinds: Vec<KnowledgeKind>,
    ) -> KnowledgeListFilter {
        KnowledgeListFilter {
            kinds,
            state,
            offset,
            limit,
        }
    }
    fn assert_no_plaintext(path: &Path, markers: &[&str]) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                assert_no_plaintext(&path, markers);
                continue;
            }
            let raw = std::fs::read(&path).unwrap();
            for marker in markers {
                assert!(
                    !raw.windows(marker.len()).any(|w| w == marker.as_bytes()),
                    "plaintext {marker} in {}",
                    path.display()
                );
                assert!(!path.to_string_lossy().contains(marker));
            }
        }
    }
    fn assert_citation(store: &EvidenceVault, candidate: &DistilledCandidate) {
        let KnowledgeRecord::Evidence(e) = store.read(&candidate.citation.evidence).unwrap().record
        else {
            panic!("citation must reference evidence");
        };
        assert_eq!(e.evidence_kind, EvidenceKind::Artifact);
        let cited = &e.content[candidate.citation.start..candidate.citation.end];
        assert_eq!(candidate.citation.excerpt, display_text(cited, 280));
        let KnowledgeRecord::Candidate(c) = store.read(&candidate.reference).unwrap().record else {
            panic!("candidate kind");
        };
        assert_eq!(c.evidence, vec![candidate.citation.evidence.clone()]);
        assert_eq!(c.statement, candidate.statement);
        assert_eq!(c.header.metadata, e.header.metadata, "same source scope");
        let fence: ApplicabilityFence =
            serde_json::from_str(&c.header.metadata.applicability.constraints).unwrap();
        assert_eq!(fence.file_digest, c.header.metadata.source_version);
        assert_eq!(fence.file_digest, c.header.metadata.source_commit);
        assert!(c.header.metadata.source_id.starts_with("local:"));
        assert_eq!(c.header.metadata.source_id.len(), 22);
        assert_eq!(c.header.metadata.applicability.platforms, [PLATFORM]);
    }

    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "LocalFile capture needs the Linux fd-safe snapshot backend (non-Linux reports it as excluded: unreadable)"
    )]
    fn knowledge_service_distill_pipeline_real_vault() {
        let (temp, vault) = fixture();
        let project = temp.path().join("project");
        let scratch = temp.path().join("scratch");
        for dir in ["src", "data", "notes"] {
            std::fs::create_dir_all(project.join(dir)).unwrap();
        }
        let python = "\"\"\"Calculator module.\"\"\"\n\n\ndef add(a, b):\n    \"\"\"Return the sum of a and b PRIVATEMARKER-py.\"\"\"\n    return a + b\n";
        std::fs::write(project.join("src/calc.py"), python).unwrap();
        std::fs::write(
            project.join("data/blob.bin"),
            [0xffu8, 0xfe, 0x41, 0x00, 0x80],
        )
        .unwrap();
        std::fs::write(
            project.join("notes/heldout-plan.md"),
            "# Plan\n\nSecret plan.\n",
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(project.join("src/calc.py"), project.join("link.md")).unwrap();
        let eval = "# Evaluation\n\nThe expected answer is 42.\n";
        let svc = KnowledgeService::new(
            vault.clone(),
            KnowledgeServiceConfig {
                scratch: scratch.clone(),
                // Upper-case on purpose: normalized to lowercase by `new`.
                excluded_sha256: [knowledge::digest(eval.as_bytes()).to_ascii_uppercase()].into(),
                excluded_name_markers: vec![
                    "heldout".into(),
                    "held-out".into(),
                    "acceptance-suite".into(),
                ],
            },
        );
        let store = EvidenceVault::new(vault.clone());
        let guide = "# Vault locking\n\nThe vault locks after five minutes of inactivity. More text here.\n\n## Recovery codes\n\n- Store recovery codes offline PRIVATEMARKER-7f3a.\n\n```\n# not a heading\n```\n";
        let local = |path: &str| DistillSource::LocalFile {
            root: project.clone(),
            path: path.into(),
        };
        let request = DistillRequest {
            sources: vec![
                pasted("guide.md", guide),
                local("src/calc.py"),
                pasted("Heldout answers.md", "# Answers\n\nAll of them.\n"),
                pasted("eval.md", eval),
                pasted("copy-of-guide.md", guide),
                local("data/blob.bin"),
                local("link.md"),
                local("notes/heldout-plan.md"),
            ],
            budget: budget(64),
            platform: PLATFORM.into(),
            license: "CC-BY-4.0".into(),
            topics: vec!["security".into()],
        };
        let sha = distill_request_sha256(&request).unwrap();
        let confirm = |sha: &str, id: u8| UiDistillEvent {
            request_sha256: sha.into(),
            ui_event_id: event(id),
        };

        // Uninitialized store: status is honest, mutations refuse.
        let status = svc.status().unwrap();
        assert!(!status.initialized);
        assert_eq!(status.index, IndexState::Missing);
        assert_eq!(status.method, DISTILL_METHOD);
        assert_eq!(status.counts.len(), 5);
        assert_eq!(
            svc.distill(request.clone(), confirm(&sha, 1)).unwrap_err(),
            KnowledgeError::Uninitialized
        );
        assert_eq!(
            svc.list(all(10, 0, ListState::All, vec![])).unwrap_err(),
            KnowledgeError::Uninitialized
        );
        assert_eq!(
            svc.initialize(UiInitEvent {
                ui_event_id: "not-hex".into()
            })
            .unwrap_err(),
            KnowledgeError::Invalid
        );
        let status = svc
            .initialize(UiInitEvent {
                ui_event_id: event(2),
            })
            .unwrap();
        assert!(status.initialized);
        assert_eq!(status.catalog_entries, 0);
        assert_eq!(status.index, IndexState::Missing);

        // The confirmed hash binds the displayed plan.
        assert_eq!(
            svc.distill(request.clone(), confirm(&"0".repeat(64), 3))
                .unwrap_err(),
            KnowledgeError::Conflict
        );
        let mut bad_event = confirm(&sha, 3);
        bad_event.ui_event_id = "ABC".into();
        assert_eq!(
            svc.distill(request.clone(), bad_event).unwrap_err(),
            KnowledgeError::Invalid
        );
        assert!(
            store.catalog().unwrap().is_empty(),
            "refused runs write nothing"
        );

        // Event 1 was released by the failed (uninitialized) attempt.
        let report = svc.distill(request.clone(), confirm(&sha, 1)).unwrap();
        assert_eq!(report.method, DISTILL_METHOD);
        assert!(!report.budget_exhausted);
        let excluded: BTreeMap<&str, &str> = report
            .excluded
            .iter()
            .map(|x| (x.label.as_str(), x.reason.as_str()))
            .collect();
        assert_eq!(excluded["Heldout answers.md"], "held_out_name");
        assert_eq!(excluded["notes/heldout-plan.md"], "held_out_name");
        assert_eq!(excluded["eval.md"], "held_out_hash");
        assert_eq!(excluded["copy-of-guide.md"], "duplicate");
        assert_eq!(
            excluded["link.md"], "unreadable",
            "symlink refused by fd-safe capture"
        );
        #[cfg(target_os = "linux")]
        {
            assert_eq!(excluded["data/blob.bin"], "not_utf8");
            assert_eq!(report.excluded.len(), 6);
            assert_eq!(report.evidence.len(), 2);
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert_eq!(excluded["src/calc.py"], "unreadable");
            assert_eq!(excluded["data/blob.bin"], "unreadable");
        }
        let statements: Vec<&str> = report
            .candidates
            .iter()
            .map(|c| c.statement.as_str())
            .collect();
        assert!(statements
            .contains(&"Vault locking: The vault locks after five minutes of inactivity."));
        assert!(statements
            .contains(&"Recovery codes: Store recovery codes offline PRIVATEMARKER-7f3a."));
        #[cfg(target_os = "linux")]
        assert!(statements.contains(&"def add(a, b): Return the sum of a and b PRIVATEMARKER-py."));
        for candidate in &report.candidates {
            assert!(report.evidence.contains(&candidate.citation.evidence));
            assert_citation(&store, candidate);
        }
        let first = report
            .candidates
            .iter()
            .find(|c| c.statement.starts_with("Vault locking"))
            .unwrap()
            .clone();
        assert_eq!(
            first.citation.excerpt,
            "# Vault locking The vault locks after five minutes of inactivity."
        );
        assert_eq!(first.citation.start, 0);
        // Nothing promoted: Evidence + Candidate only.
        assert!(store.catalog().unwrap().iter().all(|m| matches!(
            m.reference.kind,
            KnowledgeKind::Evidence | KnowledgeKind::Candidate
        )));
        let KnowledgeRecord::Evidence(summary) = store.read(&report.summary).unwrap().record else {
            panic!("summary is evidence");
        };
        assert_eq!(summary.evidence_kind, EvidenceKind::Observation);
        assert!(summary.content.contains(DISTILL_METHOD));
        assert!(summary.content.contains("held_out_hash"));
        assert!(
            !summary.content.contains("PRIVATEMARKER") && !summary.content.contains("guide.md")
        );
        assert_eq!(
            svc.distill(request.clone(), confirm(&sha, 1)).unwrap_err(),
            KnowledgeError::Conflict,
            "a confirmed UI event cannot be replayed"
        );

        // Index: missing until an explicit rebuild; never rebuilt by search.
        let view = svc
            .search(query("vault locks", RecallMode::Historical, None, vec![]))
            .unwrap();
        assert_eq!(view.index, IndexState::Missing);
        assert!(view.hits.is_empty());
        assert_eq!(view.normalization.terms, ["locks", "vault"]);
        assert_eq!(svc.status().unwrap().index, IndexState::Missing);
        assert_eq!(svc.rebuild_index().unwrap().index, IndexState::Fresh);
        let view = svc
            .search(query("vault locks", RecallMode::Historical, None, vec![]))
            .unwrap();
        assert_eq!(view.index, IndexState::Fresh);
        assert!(view.note.contains("not semantic"));
        assert!(view.hits.iter().any(|h| h.reference == first.reference
            && h.mode == RecallMode::Historical
            && h.active
            && !h.invalidated
            && h.title == first.statement));
        let view = svc
            .search(query(
                "vault locks",
                RecallMode::Historical,
                None,
                vec![KnowledgeKind::Evidence],
            ))
            .unwrap();
        assert!(!view.hits.is_empty());
        assert!(view.hits.iter().all(|h| h.kind == KnowledgeKind::Evidence));
        assert_eq!(
            svc.search(query("vault locks", RecallMode::Current, None, vec![]))
                .unwrap_err(),
            KnowledgeError::Invalid,
            "current mode requires a trusted source"
        );
        let guide_sha = knowledge::digest(guide.as_bytes());
        let trusted = TrustedSource {
            source_id: format!("local:{}", &knowledge::digest(b"guide.md")[..16]),
            source_version: guide_sha.clone(),
            source_commit: guide_sha.clone(),
            file_digest: guide_sha.clone(),
        };
        let view = svc
            .search(query(
                "vault locks",
                RecallMode::Current,
                Some(trusted.clone()),
                vec![KnowledgeKind::Candidate],
            ))
            .unwrap();
        assert_eq!(view.hits.len(), 1);
        assert_eq!(view.hits[0].reference, first.reference);
        assert_eq!(
            view.hits[0].source.file_digest.as_deref(),
            Some(guide_sha.as_str())
        );
        assert_eq!(view.hits[0].source.privacy, "private");
        assert_eq!(view.hits[0].source.license, "CC-BY-4.0");
        let mut wrong = trusted.clone();
        wrong.file_digest = "a".repeat(64);
        assert!(svc
            .search(query(
                "vault locks",
                RecallMode::Current,
                Some(wrong),
                vec![]
            ))
            .unwrap()
            .hits
            .is_empty());
        for bad in [
            KnowledgeQuery {
                limit: 0,
                ..query("vault", RecallMode::Historical, None, vec![])
            },
            KnowledgeQuery {
                limit: 17,
                ..query("vault", RecallMode::Historical, None, vec![])
            },
            query(&"v".repeat(257), RecallMode::Historical, None, vec![]),
        ] {
            assert_eq!(svc.search(bad).unwrap_err(), KnowledgeError::Invalid);
        }

        // Second run: contradiction heuristic + candidate budget.
        let second = DistillRequest {
            sources: vec![
                pasted(
                    "policy.md",
                    "# Vault locking\n\nThe vault never locks after five minutes of inactivity.\n",
                ),
                pasted(
                    "rules.md",
                    "# Alpha\n\nAlpha rule one.\n\n# Beta\n\nBeta rule two.\n\n# Gamma\n\nGamma rule three.\n",
                ),
            ],
            budget: budget(2),
            ..request.clone()
        };
        let sha2 = distill_request_sha256(&second).unwrap();
        let report2 = svc.distill(second, confirm(&sha2, 4)).unwrap();
        assert!(report2.budget_exhausted);
        assert_eq!(report2.candidates.len(), 2);
        assert_eq!(report2.possible_contradictions.len(), 1);
        let contradiction = &report2.possible_contradictions[0];
        assert_eq!(contradiction.existing, first.reference);
        assert_eq!(contradiction.rule, RULE_NEGATION);
        assert_eq!(contradiction.candidate, report2.candidates[0].reference);
        // Merge decision: heuristic contradictions are reported only; no
        // Contradicting edge is persisted, so neither record is poisoned.
        let existing = svc.detail(&first.reference.logical_id, None).unwrap();
        assert!(existing.active && !existing.invalidated && !existing.contradictory);
        assert!(!existing
            .edges
            .iter()
            .any(|e| e.relation == EdgeKind::Contradicting));
        let newer = svc
            .detail(&contradiction.candidate.logical_id, None)
            .unwrap();
        assert!(newer.active && !newer.contradictory);
        assert!(!newer
            .edges
            .iter()
            .any(|e| e.relation == EdgeKind::Contradicting));
        assert_eq!(newer.allowed_actions, ["reject"]);
        assert!(
            !store.is_invalidated(&first.reference).unwrap(),
            "never auto-invalidated"
        );
        // Writes made the index stale; search reports it and returns nothing.
        assert_eq!(svc.status().unwrap().index, IndexState::Stale);
        let view = svc
            .search(query("vault locks", RecallMode::Historical, None, vec![]))
            .unwrap();
        assert_eq!(view.index, IndexState::Stale);
        assert!(view.hits.is_empty());
        svc.rebuild_index().unwrap();
        // Not poisoned by a reported (unpersisted) heuristic contradiction.
        let current = svc
            .search(query(
                "vault locks",
                RecallMode::Current,
                Some(trusted),
                vec![KnowledgeKind::Candidate],
            ))
            .unwrap();
        assert!(current.hits.iter().all(|h| !h.contradictory));

        // Third run: a document over the Stage 3 per-document term cap is
        // chunked so the index stays buildable.
        let mut big = String::new();
        for k in 0..8 {
            big.push_str(&format!("# Part {k}\n\nPart {k} explains group {k}.\n\n"));
            for line in 0..20 {
                let words: Vec<String> = (0..10)
                    .map(|w| format!("w{}x", k * 1000 + line * 10 + w))
                    .collect();
                big.push_str(&words.join(" "));
                big.push('\n');
            }
        }
        let third = DistillRequest {
            sources: vec![pasted("big.md", &big)],
            budget: budget(64),
            ..request.clone()
        };
        let sha3 = distill_request_sha256(&third).unwrap();
        let report3 = svc.distill(third, confirm(&sha3, 5)).unwrap();
        assert!(report3.evidence.len() >= 3, "{}", report3.evidence.len());
        assert_eq!(report3.candidates.len(), 8);
        assert!(!report3.budget_exhausted && report3.excluded.is_empty());
        for reference in &report3.evidence {
            let terms = record_index_terms(&store.read(reference).unwrap().record);
            assert!(terms.len() <= 512, "{}", terms.len());
        }
        for candidate in &report3.candidates {
            assert_citation(&store, candidate);
        }
        assert_eq!(svc.rebuild_index().unwrap().index, IndexState::Fresh);

        // Deadline (injected clock) and lock-epoch abort, through the same run.
        let fourth = DistillRequest {
            sources: vec![
                pasted("late-a.md", "# Late\n\nLate one.\n"),
                pasted("late-b.md", "# Later\n\nLate two.\n"),
            ],
            budget: budget(64),
            ..request.clone()
        };
        let sha4 = distill_request_sha256(&fourth).unwrap();
        let before = store.catalog().unwrap().len();
        let snapshot = take_snapshot(&svc.store).unwrap();
        let ui = event(6);
        let dead = || false;
        let mut zero = || 0u64;
        let mut context = RunContext {
            store: &svc.store,
            config: &svc.config,
            request_sha256: &sha4,
            ui_event_id: &ui,
            elapsed_ms: &mut zero,
            alive: &dead,
        };
        assert_eq!(
            distiller::run(&mut context, &fourth, &snapshot).unwrap_err(),
            KnowledgeError::Locked
        );
        assert_eq!(
            store.catalog().unwrap().len(),
            before,
            "no write after lock"
        );
        let mut calls = 0u64;
        let mut clock = || {
            calls += 1;
            if calls >= 2 {
                1 << 40
            } else {
                0
            }
        };
        let alive = || true;
        let mut context = RunContext {
            store: &svc.store,
            config: &svc.config,
            request_sha256: &sha4,
            ui_event_id: &ui,
            elapsed_ms: &mut clock,
            alive: &alive,
        };
        let late = distiller::run(&mut context, &fourth, &snapshot).unwrap();
        assert!(late.budget_exhausted);
        assert!(late.evidence.is_empty() && late.candidates.is_empty());
        let reasons: Vec<(&str, &str)> = late
            .excluded
            .iter()
            .map(|x| (x.label.as_str(), x.reason.as_str()))
            .collect();
        assert_eq!(reasons, [("late-a.md", "budget"), ("late-b.md", "budget")]);
        assert_eq!(
            store.catalog().unwrap().len(),
            before + 1,
            "only the run summary"
        );

        let runs = svc.distill_runs().unwrap();
        assert_eq!(runs.len(), 4);
        assert_eq!(runs[0].run_id, late.run_id);
        assert_eq!(
            (runs[0].evidence, runs[0].candidates, runs[0].excluded),
            (0, 0, 2)
        );
        assert_eq!(runs[3].run_id, report.run_id);
        assert_eq!(runs[3].summary, report.summary);
        assert_eq!(runs[3].candidates, report.candidates.len());
        assert_eq!(runs[3].excluded, report.excluded.len());

        assert_no_plaintext(
            &temp.path().join("UNOONE"),
            &[
                "PRIVATEMARKER-7f3a",
                "PRIVATEMARKER-py",
                "Vault locking",
                "inactivity",
                "guide.md",
                "CC-BY-4.0",
            ],
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            std::fs::read_dir(&scratch).unwrap().count(),
            0,
            "staged fd-safe captures are removed"
        );

        svc.on_lock();
        vault.lock().unwrap().as_mut().unwrap().lock().unwrap();
        assert_eq!(svc.status().unwrap_err(), KnowledgeError::Locked);
        assert_eq!(
            svc.distill(fourth, confirm(&sha4, 8)).unwrap_err(),
            KnowledgeError::Locked
        );
        assert_eq!(svc.distill_runs().unwrap_err(), KnowledgeError::Locked);
    }

    fn metadata(topic: &str) -> KnowledgeMetadata {
        KnowledgeMetadata {
            source_id: "repo:fixture".into(),
            source_version: "v1".into(),
            source_commit: "b".repeat(40),
            license: "MIT".into(),
            privacy: KnowledgePrivacy::Private,
            applicability: Applicability {
                topics: vec![topic.into()],
                platforms: vec![PLATFORM.into()],
                constraints: ApplicabilityFence {
                    schema: FENCE_SCHEMA.into(),
                    file_digest: "c".repeat(64),
                }
                .encode()
                .unwrap(),
            },
        }
    }
    fn header(id: &str, previous: Option<&RecordRef>, edges: Vec<GraphEdge>) -> RecordHeader {
        RecordHeader {
            schema: KNOWLEDGE_SCHEMA.into(),
            logical_id: id.into(),
            revision: previous.map_or(1, |p| p.revision + 1),
            previous: previous.cloned(),
            timestamp_ms: 1,
            audit: Audit {
                actor: "fixture".into(),
                reason: "fixture".into(),
            },
            metadata: metadata("routing"),
            edges,
        }
    }
    fn evidence(id: &str, kind: EvidenceKind, content: &str) -> KnowledgeRecord {
        KnowledgeRecord::Evidence(Evidence {
            header: header(id, None, vec![]),
            evidence_kind: kind,
            content: content.into(),
            content_sha256: knowledge::digest(content.as_bytes()),
        })
    }
    fn candidate(
        id: &str,
        statement: &str,
        evidence: &RecordRef,
        edges: Vec<GraphEdge>,
    ) -> KnowledgeRecord {
        KnowledgeRecord::Candidate(Candidate {
            header: header(id, None, edges),
            statement: statement.into(),
            evidence: vec![evidence.clone()],
        })
    }
    fn receipt(id: &str, baseline: bool, repetition: u32, status: i32, stdout: &str) -> String {
        let argv = ["/usr/bin/python3", "-I", "/work/check.py"];
        serde_json::to_string(&serde_json::json!({
            "schema": RECEIPT_SCHEMA, "logical_id": id, "evidence_kind": "check_result",
            "body": {"run_id": "run-fixture", "baseline": baseline, "repetition": repetition,
                "case": {"name": "positive", "kind": "positive", "argv": argv, "expected_status": 0,
                    "expected_stdout": "ok\n", "expected_stderr": "", "expected_files": {}},
                "actual": {"argv": argv, "status": status, "termination": "completed",
                    "stdout_hex": hex(stdout.as_bytes()), "stderr_hex": "", "log_sha256": "d".repeat(64),
                    "output_files": {}, "elapsed_ms": 3}},
            "tag": "e".repeat(64)
        }))
        .unwrap()
    }
    fn outcome() -> unoone_capability_contracts::ProcedureOutcome {
        serde_json::from_value(serde_json::json!({
            "schema": unoone_capability_contracts::schemas::PROCEDURE,
            "procedure_id": "procedure-one", "bounded_arguments": "fixed",
            "preconditions": "checked", "postconditions": "checked", "result": "success",
            "verification": {"verified": true, "evidence": "recorded-claim-not-authority"}, "risk_class": "LOW",
            "promotion": {"status": "approved", "policy_version": "v1", "requirements": {
                "bounded_arguments": true, "repeatable_success": true, "verified_postconditions": true,
                "low_risk_class": true, "no_contradictory_evidence": true, "explicit_approval": true}},
            "timestamp_ms": 1, "provenance": {"platform": "fixture", "device_id": "fixture", "source": "recorded-claim"}
        }))
        .unwrap()
    }
    fn ghost(kind: KnowledgeKind) -> RecordRef {
        RecordRef {
            logical_id: "ghost-record".into(),
            revision: 1,
            kind,
            content_digest: "f".repeat(64),
        }
    }

    #[test]
    fn knowledge_service_review_export_lock_real_vault() {
        let (temp, vault) = fixture();
        let store = EvidenceVault::new(vault.clone());
        store.initialize().unwrap();
        let create = |record| store.create(record).unwrap().reference;
        let ev = create(evidence(
            "ev-route",
            EvidenceKind::Artifact,
            "PRIVATE-EVIDENCE: route requests through the proxy.",
        ));
        let cand = create(candidate(
            "pattern-one",
            "Route requests through the proxy",
            &ev,
            vec![],
        ));
        let base = create(evidence(
            "check-base",
            EvidenceKind::CheckResult,
            &receipt("check-base", true, 0, 1, ""),
        ));
        let fix1 = create(evidence(
            "check-fix-1",
            EvidenceKind::CheckResult,
            &receipt("check-fix-1", false, 1, 0, "ok\n"),
        ));
        let fix2 = create(evidence(
            "check-fix-2",
            EvidenceKind::CheckResult,
            &receipt("check-fix-2", false, 2, 0, "ok\n"),
        ));
        let vp = store
            .append_revision(
                KnowledgeRecord::VerifiedPattern(VerifiedPattern {
                    header: header("pattern-one", Some(&cand), vec![]),
                    candidate: cand.clone(),
                    checks: vec![base.clone(), fix1.clone(), fix2.clone()],
                    statement: "Route requests through the proxy".into(),
                }),
                &cand,
            )
            .unwrap()
            .reference;
        let run = create(evidence(
            "run-one",
            EvidenceKind::ProcedureRun,
            "{\"run\":1}",
        ));
        let ui = create(evidence("ui-one", EvidenceKind::UiApproval, "{\"ui\":1}"));
        let ap = store
            .append_revision(
                KnowledgeRecord::ApprovedProcedure(Box::new(ApprovedProcedure {
                    header: header("pattern-one", Some(&vp), vec![]),
                    pattern: vp.clone(),
                    outcome: outcome(),
                    outcome_evidence: vec![run.clone()],
                    approval_evidence: ui.clone(),
                })),
                &vp,
            )
            .unwrap()
            .reference;
        let cand2 = create(candidate("cand-two", "Use the second route", &ev, vec![]));
        let ev2 = create(evidence(
            "ev-two",
            EvidenceKind::Observation,
            "Observed second route.",
        ));
        let cand3 = create(candidate(
            "cand-three",
            "Second route depends on observation",
            &ev2,
            vec![],
        ));
        let candb = create(candidate("pattern-b", "Prefer direct routes", &ev, vec![]));
        let checkb = create(evidence(
            "check-b",
            EvidenceKind::CheckResult,
            "not a stage4 receipt",
        ));
        let vpb = store
            .append_revision(
                KnowledgeRecord::VerifiedPattern(VerifiedPattern {
                    header: header("pattern-b", Some(&candb), vec![]),
                    candidate: candb.clone(),
                    checks: vec![checkb.clone()],
                    statement: "Prefer direct routes".into(),
                }),
                &candb,
            )
            .unwrap()
            .reference;
        let candc = create(candidate(
            "cand-c",
            "Never prefer direct routes",
            &ev,
            vec![GraphEdge {
                relation: EdgeKind::Contradicting,
                target: vpb.clone(),
            }],
        ));
        let svc = KnowledgeService::new(
            vault.clone(),
            KnowledgeServiceConfig {
                scratch: temp.path().join("scratch"),
                excluded_sha256: BTreeSet::new(),
                excluded_name_markers: vec![],
            },
        );

        // Parity of the one-pass flags with Stage 2/3 targeted reads.
        let snapshot = take_snapshot(&svc.store).unwrap();
        for mapping in store.catalog().unwrap() {
            let read = store
                .read_targeted(std::slice::from_ref(&mapping.reference))
                .unwrap();
            let flags = snapshot.flags_of(&mapping.reference).unwrap();
            assert_eq!(
                flags.active, read.items[0].active,
                "{:?}",
                mapping.reference
            );
            assert_eq!(flags.contradictory, read.items[0].contradictory);
        }

        let status = svc.status().unwrap();
        assert_eq!(status.catalog_entries, 16);
        let expected: BTreeMap<String, usize> = [
            ("approved_procedure", 1),
            ("candidate", 3),
            ("evidence", 8),
            ("invalidation", 0),
            ("verified_pattern", 1),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_string(), *v))
        .collect();
        assert_eq!(status.counts, expected);
        assert_eq!(status.active_counts, expected);
        assert_eq!(status.index, IndexState::Missing);
        assert!(!status.residuals.is_empty());

        // List: heads only, newest first, filters, pagination, bounds.
        let page = svc.list(all(5, 0, ListState::All, vec![])).unwrap();
        assert_eq!(page.total, 13);
        assert_eq!(page.items.len(), 5);
        assert_eq!(page.items[0].reference, candc);
        assert_eq!(
            svc.list(all(5, 10, ListState::All, vec![]))
                .unwrap()
                .items
                .len(),
            3
        );
        for limit in [0, 101] {
            assert_eq!(
                svc.list(all(limit, 0, ListState::All, vec![])).unwrap_err(),
                KnowledgeError::Invalid
            );
        }
        let approved = svc
            .list(all(
                10,
                0,
                ListState::All,
                vec![KnowledgeKind::ApprovedProcedure],
            ))
            .unwrap();
        assert_eq!(approved.items.len(), 1);
        assert_eq!(
            approved.items[0].title,
            "approved procedure: Route requests through the proxy"
        );
        let contradictory: BTreeSet<RecordRef> = svc
            .list(all(100, 0, ListState::Contradictory, vec![]))
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.reference)
            .collect();
        assert_eq!(contradictory, [vpb.clone(), candc.clone()].into());

        // Detail: history, verification summary with real exit codes, actions.
        let detail = svc.detail("pattern-one", None).unwrap();
        assert_eq!(detail.reference, ap);
        assert_eq!(detail.history, [cand.clone(), vp.clone(), ap.clone()]);
        assert!(matches!(
            detail.body,
            KnowledgeBody::ApprovedProcedure { .. }
        ));
        assert_eq!(detail.allowed_actions, ["revoke_approval", "export"]);
        let verification = detail.verification.clone().unwrap();
        assert!(verification.approved);
        assert_eq!(verification.approval, Some(ui.clone()));
        assert_eq!(verification.repetitions, 2);
        let roles: Vec<(&str, Option<i32>, bool)> = verification
            .checks
            .iter()
            .map(|c| (c.role.as_str(), c.status, c.passed))
            .collect();
        assert_eq!(
            roles,
            [
                ("baseline", Some(1), false),
                ("fixed", Some(0), true),
                ("fixed", Some(0), true)
            ]
        );
        assert!(verification
            .checks
            .iter()
            .all(|c| c.termination == "completed" && c.case == "positive"));
        assert!(detail.residuals.iter().any(|r| r == VERIFICATION_RESIDUAL));
        let pattern = svc.detail("pattern-one", Some(2)).unwrap();
        assert_eq!(
            pattern.allowed_actions,
            ["export"],
            "not the head: no reject"
        );
        assert!(pattern.verification.unwrap().approved);
        assert!(svc
            .detail("pattern-one", Some(1))
            .unwrap()
            .allowed_actions
            .is_empty());
        let b = svc.detail("pattern-b", None).unwrap();
        assert_eq!(
            b.allowed_actions,
            ["reject"],
            "contradictory: not exportable"
        );
        assert_eq!(b.verification.unwrap().checks[0].case, "unparsed");
        assert!(b.edges.iter().any(|e| e.direction == "incoming"
            && e.relation == EdgeKind::Contradicting
            && e.target == candc));
        let e = svc.detail("ev-route", None).unwrap();
        assert_eq!(
            e.body,
            KnowledgeBody::Evidence {
                evidence_kind: EvidenceKind::Artifact,
                content: "PRIVATE-EVIDENCE: route requests through the proxy.".into(),
                truncated: false,
                content_sha256: knowledge::digest(
                    b"PRIVATE-EVIDENCE: route requests through the proxy."
                ),
            }
        );
        assert_eq!(e.allowed_actions, ["reject"]);
        assert_eq!(
            svc.detail("ghost-record", None).unwrap_err(),
            KnowledgeError::NotFound
        );
        assert_eq!(
            svc.detail("bad id!", None).unwrap_err(),
            KnowledgeError::Invalid
        );
        assert_eq!(
            svc.detail("pattern-one", Some(0)).unwrap_err(),
            KnowledgeError::Invalid
        );
        assert_eq!(
            svc.detail("pattern-one", Some(9)).unwrap_err(),
            KnowledgeError::NotFound
        );

        // Export preview: refusals with reasons, consent hash.
        let request = ExportRequest {
            references: vec![
                ap.clone(),
                vp.clone(),
                vpb.clone(),
                cand2.clone(),
                ev.clone(),
                ghost(KnowledgeKind::VerifiedPattern),
            ],
            include_evidence_content: false,
        };
        let preview = svc.export_preview(request.clone()).unwrap();
        assert_eq!(
            preview.request_sha256,
            export_request_sha256(&request).unwrap()
        );
        assert_eq!(
            preview
                .items
                .iter()
                .map(|i| i.reference.clone())
                .collect::<Vec<_>>(),
            [ap.clone(), vp.clone()]
        );
        assert_eq!(preview.items[0].licence, "MIT");
        let refused: Vec<(RecordRef, &str)> = preview
            .refused
            .iter()
            .map(|(r, why)| (r.clone(), why.as_str()))
            .collect();
        assert_eq!(
            refused,
            [
                (vpb.clone(), "contradictory"),
                (cand2.clone(), "kind_not_exportable"),
                (ev.clone(), "kind_not_exportable"),
                (ghost(KnowledgeKind::VerifiedPattern), "not_found"),
            ]
        );
        assert!(!preview.contains_private_content);
        assert_eq!(preview.training_export, TRAINING_EXPORT);
        let with_content = ExportRequest {
            references: vec![ap.clone(), vp.clone()],
            include_evidence_content: true,
        };
        let preview = svc.export_preview(with_content.clone()).unwrap();
        assert!(preview.contains_private_content);
        for bad in [
            ExportRequest {
                references: vec![],
                include_evidence_content: false,
            },
            ExportRequest {
                references: vec![ap.clone(), ap.clone()],
                include_evidence_content: false,
            },
            ExportRequest {
                references: (0..65).map(|_| ap.clone()).collect(),
                include_evidence_content: false,
            },
        ] {
            assert_eq!(
                svc.export_preview(bad).unwrap_err(),
                KnowledgeError::Invalid
            );
        }

        // Export consent: hash binding, private acknowledgement, determinism.
        let consent =
            |request: &ExportRequest, sha: String, id: u8, ack: bool| UiExportConsentEvent {
                request: request.clone(),
                request_sha256: sha,
                ui_event_id: event(id),
                acknowledged_private: ack,
            };
        let sha = preview.request_sha256.clone();
        assert_eq!(
            svc.export(consent(&with_content, "0".repeat(64), 20, true))
                .unwrap_err(),
            KnowledgeError::Conflict
        );
        assert_eq!(
            svc.export(consent(&with_content, sha.clone(), 20, false))
                .unwrap_err(),
            KnowledgeError::Invalid,
            "private evidence content needs acknowledgement"
        );
        let bundle = svc
            .export(consent(&with_content, sha.clone(), 20, true))
            .unwrap();
        assert_eq!(bundle.schema, EXPORT_SCHEMA);
        assert_eq!(bundle.items, 2);
        assert_eq!(bundle.sha256, knowledge::digest(bundle.json.as_bytes()));
        assert!(bundle.json.contains("PRIVATE-EVIDENCE"));
        let parsed: serde_json::Value = serde_json::from_str(&bundle.json).unwrap();
        assert_eq!(parsed["training_export"], TRAINING_EXPORT);
        assert_eq!(
            parsed["items"][0]["statement"],
            "Route requests through the proxy"
        );
        assert_eq!(parsed["items"][0]["verification"]["checks"][0]["status"], 1);
        assert_eq!(
            svc.export(consent(&with_content, sha.clone(), 20, true))
                .unwrap_err(),
            KnowledgeError::Conflict,
            "event replay"
        );
        let again = svc
            .export(consent(&with_content, sha.clone(), 21, true))
            .unwrap();
        assert_eq!(
            (again.json.clone(), again.sha256.clone()),
            (bundle.json.clone(), bundle.sha256.clone())
        );
        let metadata_only = ExportRequest {
            references: vec![ap.clone(), vp.clone()],
            include_evidence_content: false,
        };
        let plain = svc
            .export(consent(
                &metadata_only,
                export_request_sha256(&metadata_only).unwrap(),
                22,
                false,
            ))
            .unwrap();
        assert!(!plain.json.contains("PRIVATE-EVIDENCE"));
        assert!(plain.json.contains(&knowledge::digest(
            b"PRIVATE-EVIDENCE: route requests through the proxy."
        )));
        let refused_only = ExportRequest {
            references: vec![cand2.clone()],
            include_evidence_content: false,
        };
        assert_eq!(
            svc.export(consent(
                &refused_only,
                export_request_sha256(&refused_only).unwrap(),
                23,
                false
            ))
            .unwrap_err(),
            KnowledgeError::Invalid
        );
        // on_lock forgets used event ids (ledger is unlocked-session state).
        svc.on_lock();
        assert!(svc
            .export(consent(&with_content, sha.clone(), 20, true))
            .is_ok());

        // Reject: append-only invalidation of the exact active head.
        let reject = |target: &RecordRef, reason: &str, id: u8| UiRejectEvent {
            target: target.clone(),
            reason: reason.into(),
            ui_event_id: event(id),
        };
        let rejected = svc.reject(reject(&cand2, "wrong route", 30)).unwrap();
        assert_eq!(rejected.reference, cand2);
        assert!(rejected.invalidated && !rejected.active);
        assert!(rejected.allowed_actions.is_empty());
        assert!(rejected.edges.iter().any(|e| e.direction == "incoming"
            && e.relation == EdgeKind::InvalidatedBy
            && e.target.kind == KnowledgeKind::Invalidation));
        assert_eq!(
            svc.reject(reject(&cand2, "again", 31)).unwrap_err(),
            KnowledgeError::Conflict
        );
        assert_eq!(
            svc.reject(reject(&cand3, " ", 32)).unwrap_err(),
            KnowledgeError::Invalid
        );
        assert_eq!(
            svc.reject(reject(&cand3, &"r".repeat(1025), 33))
                .unwrap_err(),
            KnowledgeError::Invalid
        );
        let mut bad_id = reject(&cand3, "x", 34);
        bad_id.ui_event_id = "123".into();
        assert_eq!(svc.reject(bad_id).unwrap_err(), KnowledgeError::Invalid);
        assert_eq!(
            svc.reject(reject(&ap, "no", 35)).unwrap_err(),
            KnowledgeError::Invalid
        );
        assert_eq!(
            svc.reject(reject(&vp, "stale", 36)).unwrap_err(),
            KnowledgeError::Conflict
        );
        assert_eq!(
            svc.reject(reject(&ghost(KnowledgeKind::Candidate), "x", 37))
                .unwrap_err(),
            KnowledgeError::NotFound
        );
        let mut forged = ev.clone();
        forged.content_digest = "0".repeat(64);
        assert_eq!(
            svc.reject(reject(&forged, "x", 38)).unwrap_err(),
            KnowledgeError::Conflict
        );
        svc.reject(reject(&ev2, "bad observation", 39)).unwrap();
        let dependent = svc.detail("cand-three", None).unwrap();
        assert!(
            dependent.invalidated && !dependent.active,
            "transitive revocation"
        );
        let invalidated: BTreeSet<RecordRef> = svc
            .list(all(100, 0, ListState::Invalidated, vec![]))
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.reference)
            .collect();
        assert_eq!(
            invalidated,
            [cand2.clone(), ev2.clone(), cand3.clone()].into()
        );

        // Revoke approval.
        let revoke = |approved: &RecordRef, id: u8| UiRevokeEvent {
            approved: approved.clone(),
            ui_event_id: event(id),
        };
        assert_eq!(
            svc.revoke_approval(revoke(&vp, 40)).unwrap_err(),
            KnowledgeError::Invalid
        );
        let revoked = svc.revoke_approval(revoke(&ap, 41)).unwrap();
        assert!(revoked.invalidated && !revoked.active);
        assert!(revoked.allowed_actions.is_empty());
        assert!(!revoked.verification.unwrap().approved);
        let pattern = svc.detail("pattern-one", Some(2)).unwrap();
        let summary = pattern.verification.unwrap();
        assert!(!summary.approved && summary.approval.is_none());
        assert_eq!(
            svc.revoke_approval(revoke(&ap, 42)).unwrap_err(),
            KnowledgeError::Conflict
        );
        let refused = svc
            .export_preview(ExportRequest {
                references: vec![ap.clone()],
                include_evidence_content: false,
            })
            .unwrap()
            .refused;
        assert_eq!(refused, [(ap.clone(), "invalidated".to_string())]);
        let status = svc.status().unwrap();
        assert_eq!(status.counts["invalidation"], 3);
        assert_eq!(
            status.active_counts["candidate"], 1,
            "only cand-c stays active"
        );

        // Explicit rebuild then lexical search.
        assert_eq!(svc.rebuild_index().unwrap().index, IndexState::Fresh);
        let view = svc
            .search(query("second route", RecallMode::Historical, None, vec![]))
            .unwrap();
        assert!(view
            .hits
            .iter()
            .any(|h| h.reference == cand2 && h.invalidated && !h.active));
        assert!(view.note.contains("audit-only"));

        // Locked vault: every method fails closed with Locked.
        vault.lock().unwrap().as_mut().unwrap().lock().unwrap();
        let locked = KnowledgeError::Locked;
        assert_eq!(svc.status().unwrap_err(), locked);
        assert_eq!(
            svc.initialize(UiInitEvent {
                ui_event_id: event(50)
            })
            .unwrap_err(),
            locked
        );
        assert_eq!(svc.rebuild_index().unwrap_err(), locked);
        assert_eq!(
            svc.search(query("route", RecallMode::Historical, None, vec![]))
                .unwrap_err(),
            locked
        );
        assert_eq!(
            svc.list(all(10, 0, ListState::All, vec![])).unwrap_err(),
            locked
        );
        assert_eq!(svc.detail("pattern-one", None).unwrap_err(), locked);
        assert_eq!(svc.reject(reject(&candc, "x", 51)).unwrap_err(), locked);
        assert_eq!(svc.revoke_approval(revoke(&ap, 52)).unwrap_err(), locked);
        assert_eq!(
            svc.export_preview(metadata_only.clone()).unwrap_err(),
            locked
        );
        assert_eq!(
            svc.export(consent(
                &metadata_only,
                export_request_sha256(&metadata_only).unwrap(),
                53,
                false
            ))
            .unwrap_err(),
            locked
        );
        let request = DistillRequest {
            sources: vec![pasted("a.md", "# A\n\nB.\n")],
            budget: budget(4),
            platform: PLATFORM.into(),
            license: "unknown".into(),
            topics: vec![],
        };
        let sha = distill_request_sha256(&request).unwrap();
        assert_eq!(
            svc.distill(
                request,
                UiDistillEvent {
                    request_sha256: sha,
                    ui_event_id: event(54)
                }
            )
            .unwrap_err(),
            locked
        );
        assert_eq!(svc.distill_runs().unwrap_err(), locked);
        *vault.lock().unwrap() = None;
        assert_eq!(svc.status().unwrap_err(), locked);
    }

    #[test]
    fn knowledge_service_error_words_and_wire_shapes() {
        let words: Vec<String> = [
            KnowledgeError::Locked,
            KnowledgeError::Uninitialized,
            KnowledgeError::NotFound,
            KnowledgeError::Conflict,
            KnowledgeError::Invalid,
            KnowledgeError::Limit,
            KnowledgeError::Unsupported,
            KnowledgeError::Corrupt,
            KnowledgeError::Persistence,
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            words,
            [
                "locked",
                "uninitialized",
                "not_found",
                "conflict",
                "invalid",
                "limit",
                "unsupported",
                "corrupt",
                "persistence"
            ]
        );
        assert_eq!(serde_json::to_value(IndexState::Stale).unwrap(), "stale");
        assert_eq!(
            serde_json::to_value(ListState::Contradictory).unwrap(),
            "contradictory"
        );
        let reference = ghost(KnowledgeKind::Candidate);
        let body = serde_json::to_value(KnowledgeBody::Candidate {
            statement: "s".into(),
            evidence: vec![reference.clone()],
        })
        .unwrap();
        assert_eq!(body["kind"], "candidate");
        assert_eq!(body["statement"], "s");
        let body = serde_json::to_value(KnowledgeBody::Evidence {
            evidence_kind: EvidenceKind::CheckResult,
            content: "c".into(),
            truncated: false,
            content_sha256: "a".repeat(64),
        })
        .unwrap();
        assert_eq!(body["kind"], "evidence");
        assert_eq!(body["evidence_kind"], "check_result");
        let query: KnowledgeQuery = serde_json::from_value(serde_json::json!({
            "text": "vault", "mode": "historical", "platform": PLATFORM, "kinds": ["verified_pattern"], "limit": 4
        }))
        .unwrap();
        assert_eq!(query.trusted_source, None);
        assert_eq!(query.kinds, [KnowledgeKind::VerifiedPattern]);
        for hostile in [
            serde_json::json!({"text": "v", "mode": "historical", "platform": "p", "kinds": [], "limit": 1, "semantic": true}),
            serde_json::json!({"text": "v", "mode": "semantic", "platform": "p", "kinds": [], "limit": 1}),
        ] {
            assert!(serde_json::from_value::<KnowledgeQuery>(hostile).is_err());
        }
        assert!(
            serde_json::from_value::<KnowledgeListFilter>(serde_json::json!({
                "kinds": [], "state": "all", "offset": 0, "limit": 1, "sort": "x"
            }))
            .is_err()
        );
        assert!(serde_json::from_value::<UiRejectEvent>(serde_json::json!({
            "target": reference, "reason": "r", "ui_event_id": event(1), "force": true
        }))
        .is_err());
        assert!(
            serde_json::from_value::<UiExportConsentEvent>(serde_json::json!({
                "request": {"references": [], "include_evidence_content": false, "training": true},
                "request_sha256": "a", "ui_event_id": event(1), "acknowledged_private": false
            }))
            .is_err()
        );
        let preview = ExportPreview {
            request_sha256: "a".repeat(64),
            items: vec![],
            refused: vec![(reference.clone(), "kind_not_exportable".into())],
            contains_private_content: false,
            training_export: TRAINING_EXPORT.into(),
        };
        let json = serde_json::to_value(preview).unwrap();
        assert_eq!(json["refused"][0][1], "kind_not_exportable");
        assert_eq!(json["refused"][0][0]["logical_id"], "ghost-record");
        let edge = serde_json::to_value(EdgeView {
            relation: EdgeKind::InvalidatedBy,
            target: reference,
            direction: "incoming".into(),
        })
        .unwrap();
        assert_eq!(edge["relation"], "invalidated_by");
        assert!(
            valid_event_id(&event(255))
                && !valid_event_id(&event(1).to_uppercase().replace('0', "A"))
        );
        // The Tauri glue keeps the service in an Arc shared across threads.
        fn send_sync<T: Send + Sync>() {}
        send_sync::<KnowledgeService>();
    }
}
