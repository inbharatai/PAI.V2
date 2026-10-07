//! Stage4 actual bounded verification. KnowledgeRecord remains metadata ONLY.
//! Receipts are strict server-generated, domain-MACed immutable encrypted Evidence.
//! UI approval is a controller method, never a registered Harness tool/model API.
//! Trusted owner/controller/keyholder boundary; not whole-vault anti-rollback.
//!
//! Dependency applicability policy (`DEPENDENCY_POLICY`, enforced at verify start,
//! before promotion, at approval, at permit issue and again at replay): every
//! record in the authenticated transitive dependency closure returned by Stage3
//! `read_targeted` (the record itself, previous revisions, candidate evidence,
//! receipts, edges) must carry a structured Stage3 `ApplicabilityFence`
//! (`FENCE_SCHEMA`) whose source_id, source_version and source_commit equal the
//! trusted source shared by the baseline and fixed snapshots, whose platforms are
//! exactly `[snapshot platform]`, and whose file_digest is the baseline primary
//! digest (evidence describing the bug) or the fixed primary digest. Missing or
//! free-text fences and any other value fail closed. The fence is part of the
//! signed ProcedureRun body and is re-derived from the signed snapshot bindings
//! whenever the run is read.
//!
//! Oracle policy: the recipe partitions the selected files into declared oracle
//! (test) files and implementation files, bound into the recipe hash. Oracle bytes
//! must be identical in baseline and fixed snapshots, every case executes a
//! declared oracle file and at least one implementation file must change.

use crate::isolation::{
    self, hash, hex, ActualRun, Cancellation, IsolationError, LinuxIsolation, ProjectSnapshot,
    RunLimits, RuntimeProfile, SnapshotBinding, Termination,
};
use crate::knowledge::{EvidenceVault, KnowledgeStoreError, StoredKnowledge};
use crate::knowledge_retrieval::{ApplicabilityFence, FENCE_SCHEMA};
use hmac::{Hmac, Mac};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use unoone_capability_contracts::{
    knowledge::*, ProcedureOutcome, ProcedureResult, Promotion, PromotionRequirements,
    PromotionStatus, Provenance, Verification,
};
use unoone_vault_core::Vault;

pub const RECEIPT_SCHEMA: &str = "inbharat.pai.runtime-receipt.v1";
pub const RECIPE_SCHEMA: &str = "inbharat.pai.verification-recipe.v2";
pub const POLICY_VERSION: &str = "stage4-readonly-synthetic-replay-v2";
pub const DEPENDENCY_POLICY: &str = "inbharat.pai.stage4.dependency-applicability.v1";
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationError {
    Store(KnowledgeStoreError),
    Isolation(IsolationError),
    InvalidRecipe,
    Identity,
    ForgedProof,
    GateDenied,
}
impl From<KnowledgeStoreError> for VerificationError {
    fn from(e: KnowledgeStoreError) -> Self {
        Self::Store(e)
    }
}
impl From<IsolationError> for VerificationError {
    fn from(e: IsolationError) -> Self {
        Self::Isolation(e)
    }
}
impl std::fmt::Display for VerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "verification: {self:?}")
    }
}
impl std::error::Error for VerificationError {}
type Result<T> = std::result::Result<T, VerificationError>;
fn bytes<T: Serialize>(x: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(x).map_err(|_| VerificationError::ForgedProof)
}
fn digest<T: Serialize>(x: &T) -> Result<String> {
    Ok(hash(&bytes(x)?))
}
fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| VerificationError::GateDenied)?
        .as_millis()
        .try_into()
        .map_err(|_| VerificationError::GateDenied)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaseKind {
    Positive,
    Negative,
    Regression,
}
/// This is an owner-approved test oracle, not a model-supplied checked flag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerificationCase {
    pub name: String,
    pub kind: CaseKind,
    pub argv: Vec<String>,
    pub expected_status: i32,
    pub expected_stdout: String,
    pub expected_stderr: String,
    /// Absolute single-component /tmp paths -> SHA-256 of bounded expected bytes.
    pub expected_files: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerificationRecipe {
    pub schema: String,
    pub repetitions: u32,
    pub limits: RunLimits,
    pub cases: Vec<VerificationCase>,
    /// Owner-declared test/oracle files (every case's `/work/<script>` is one).
    /// Must be byte-identical in baseline and fixed snapshots.
    pub oracle_files: BTreeSet<String>,
    /// Owner-declared code under test; includes the attested primary. At least one
    /// must change. Oracle and implementation sets partition the selected files.
    pub implementation_files: BTreeSet<String>,
}
impl VerificationRecipe {
    pub fn validate(&self, snapshot: &SnapshotBinding) -> Result<()> {
        self.limits.validate()?;
        if self.schema != RECIPE_SCHEMA
            || !(2..=3).contains(&self.repetitions)
            || !(3..=4).contains(&self.cases.len())
            || bytes(self)?.len() > 8192
        {
            return Err(VerificationError::InvalidRecipe);
        }
        if self.oracle_files.is_empty()
            || self.implementation_files.is_empty()
            || !self.oracle_files.is_disjoint(&self.implementation_files)
            || self.oracle_files.len() + self.implementation_files.len() != snapshot.files.len()
            || !self
                .oracle_files
                .iter()
                .chain(&self.implementation_files)
                .all(|p| snapshot.files.contains_key(p))
            || !self.implementation_files.contains(&snapshot.primary)
        {
            return Err(VerificationError::InvalidRecipe);
        }
        let mut names = BTreeSet::new();
        for c in &self.cases {
            if valid_id(&c.name).is_err()
                || !names.insert(&c.name)
                || !(0..=255).contains(&c.expected_status)
                || c.expected_stdout.len() > 1024
                || c.expected_stderr.len() > 1024
                || c.expected_files.len() > 4
                || c.expected_files.iter().any(|(p, h)| {
                    !valid_digest(h)
                        || !p.starts_with("/tmp/")
                        || p[5..].is_empty()
                        || p[5..].contains('/')
                        || p.contains("..")
                        || p.contains('\0')
                })
                || c.argv.len() < 3
                || c.argv.len() > 16
                || c.argv[0] != "/usr/bin/python3"
                || c.argv[1] != "-I"
                || !c.argv[2].starts_with("/work/")
                || !snapshot.files.contains_key(&c.argv[2][6..])
                || !self.oracle_files.contains(&c.argv[2][6..])
                || c.argv.iter().any(|a| a.len() > 256 || a.contains('\0'))
                || (c.kind != CaseKind::Negative && c.expected_status != 0)
            {
                return Err(VerificationError::InvalidRecipe);
            }
        }
        for kind in [CaseKind::Positive, CaseKind::Negative, CaseKind::Regression] {
            if !self.cases.iter().any(|c| c.kind == kind) {
                return Err(VerificationError::InvalidRecipe);
            }
        }
        if self.cases[0].kind != CaseKind::Positive || self.cases[0].expected_files.is_empty() {
            return Err(VerificationError::InvalidRecipe);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerificationPolicy {
    pub schema: String,
    pub version: String,
    pub max_receipt_age_ms: u64,
}
impl VerificationPolicy {
    pub fn readonly_replay(max_receipt_age_ms: u64) -> Result<Self> {
        if !(1000..=24 * 60 * 60 * 1000).contains(&max_receipt_age_ms) {
            return Err(VerificationError::GateDenied);
        }
        Ok(Self {
            schema: "inbharat.pai.verification-policy.v1".into(),
            version: POLICY_VERSION.into(),
            max_receipt_age_ms,
        })
    }
}
/// Signed applicability fence (see module docs). Derived ONLY from the trusted
/// baseline/fixed snapshot bindings, never from candidate or model metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DependencyFence {
    policy: String,
    source_id: String,
    source_version: String,
    source_commit: String,
    platform: String,
    /// Sorted, deduplicated {baseline primary digest, fixed primary digest}.
    file_digests: Vec<String>,
}
impl DependencyFence {
    fn for_run(baseline: &SnapshotBinding, fixed: &SnapshotBinding) -> Result<Self> {
        let (b, f) = (&baseline.source, &fixed.source);
        if b.source_id != f.source_id
            || b.source_version != f.source_version
            || b.source_commit != f.source_commit
            || baseline.platform != fixed.platform
            || !valid_digest(&b.file_digest)
            || !valid_digest(&f.file_digest)
        {
            return Err(VerificationError::Identity);
        }
        Ok(Self {
            policy: DEPENDENCY_POLICY.into(),
            source_id: f.source_id.clone(),
            source_version: f.source_version.clone(),
            source_commit: f.source_commit.clone(),
            platform: fixed.platform.clone(),
            file_digests: [b.file_digest.clone(), f.file_digest.clone()]
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        })
    }
    fn admits(&self, m: &KnowledgeMetadata) -> bool {
        let Ok(fence) = serde_json::from_str::<ApplicabilityFence>(&m.applicability.constraints)
        else {
            return false;
        };
        self.policy == DEPENDENCY_POLICY
            && fence.schema == FENCE_SCHEMA
            && valid_digest(&fence.file_digest)
            && self.file_digests.contains(&fence.file_digest)
            && m.source_id == self.source_id
            && m.source_version == self.source_version
            && m.source_commit == self.source_commit
            && m.applicability.platforms.len() == 1
            && m.applicability.platforms[0] == self.platform
    }
    /// Every authenticated transitive dependency (self included) must be admitted.
    fn require(&self, dependencies: &[KnowledgeMetadata]) -> Result<()> {
        if dependencies.is_empty() || !dependencies.iter().all(|m| self.admits(m)) {
            return Err(VerificationError::GateDenied);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActualCheckedState {
    Verified,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Serialize)]
pub struct VerificationReport {
    pub actual_checked_state: ActualCheckedState,
    pub pattern: Option<RecordRef>,
    pub procedure_run: RecordRef,
    pub run_sha256: String,
    pub policy_sha256: String,
    pub checks: Vec<RecordRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CheckReceipt {
    run_id: String,
    candidate: RecordRef,
    snapshot: SnapshotBinding,
    recipe_sha256: String,
    policy_sha256: String,
    patch_sha256: String,
    baseline: bool,
    repetition: u32,
    case: VerificationCase,
    actual: ActualRun,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RunReceipt {
    run_id: String,
    candidate: RecordRef,
    baseline: SnapshotBinding,
    fixed: SnapshotBinding,
    recipe: VerificationRecipe,
    recipe_sha256: String,
    policy_sha256: String,
    patch_sha256: String,
    dependency_fence: DependencyFence,
    profile: RuntimeProfile,
    checks: Vec<RecordRef>,
    timestamp_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ApprovalReceipt {
    pattern: RecordRef,
    procedure_run: RecordRef,
    run_sha256: String,
    policy_sha256: String,
    timestamp_ms: u64,
    event_id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sealed<T> {
    schema: String,
    logical_id: String,
    evidence_kind: EvidenceKind,
    body: T,
    tag: String,
}
/// UI must show exact pattern revision, run digest and policy digest, then send
/// an explicit affirmative event through this method. Roadmap consent is NOT it.
#[derive(Debug, Clone)]
pub struct UiApprovalEvent {
    pub pattern: RecordRef,
    pub procedure_run: RecordRef,
    pub displayed_run_sha256: String,
    pub displayed_policy_sha256: String,
}
/// Unforgeable in safe Rust through this API: no constructor, Clone, serde or fields.
/// Consuming replay performs another fresh gate; possession alone is not authority.
pub struct AutonomousReusePermit {
    approved: RecordRef,
    run_sha256: String,
    snapshot_sha256: String,
    policy_sha256: String,
}

pub struct KnowledgeVerifier {
    store: EvidenceVault,
    isolation: LinuxIsolation,
    policy: VerificationPolicy,
}
impl KnowledgeVerifier {
    pub fn new(vault: Arc<Mutex<Option<Vault>>>, policy: VerificationPolicy) -> Result<Self> {
        if policy.schema != "inbharat.pai.verification-policy.v1"
            || policy.version != POLICY_VERSION
            || VerificationPolicy::readonly_replay(policy.max_receipt_age_ms).is_err()
        {
            return Err(VerificationError::GateDenied);
        }
        Ok(Self {
            store: EvidenceVault::new(vault),
            isolation: LinuxIsolation::new()?,
            policy,
        })
    }
    pub fn runtime_profile(&self) -> &RuntimeProfile {
        self.isolation.profile()
    }
    pub fn policy_sha256(&self) -> Result<String> {
        digest(&self.policy)
    }
    /// Stage3 authenticated targeted read: active, noncontradictory, plus the
    /// authenticated transitive dependency metadata (the record itself included).
    fn targeted(&self, reference: &RecordRef) -> Result<(StoredKnowledge, Vec<KnowledgeMetadata>)> {
        let read = self.store.read_targeted(std::slice::from_ref(reference))?;
        let item = read
            .items
            .into_iter()
            .next()
            .ok_or(VerificationError::GateDenied)?;
        if !item.active || item.contradictory {
            return Err(VerificationError::GateDenied);
        }
        Ok((item.stored, item.dependency_metadata))
    }
    /// Unfenced liveness only. Used solely by append-only revocation, never as an
    /// admission/promotion/reuse gate.
    fn active(&self, reference: &RecordRef) -> Result<StoredKnowledge> {
        Ok(self.targeted(reference)?.0)
    }
    /// Admission gate: active, noncontradictory AND every transitive dependency
    /// satisfies the signed dependency fence.
    fn fenced(&self, reference: &RecordRef, fence: &DependencyFence) -> Result<StoredKnowledge> {
        let (stored, dependencies) = self.targeted(reference)?;
        fence.require(&dependencies)?;
        Ok(stored)
    }
    fn latest(&self, reference: &RecordRef, fence: &DependencyFence) -> Result<StoredKnowledge> {
        let stored = self.fenced(reference, fence)?;
        if self
            .store
            .read_latest(&reference.logical_id)?
            .mapping
            .reference
            != *reference
        {
            return Err(VerificationError::GateDenied);
        }
        Ok(stored)
    }
    /// Actual baseline failure then ALL positive/negative/regression cases in >=2
    /// distinct fresh processes/namespaces. Failed runs are immutable audit evidence.
    /// No externally supplied CheckResult/verified booleans are admitted. The
    /// dependency fence and oracle rules are enforced BEFORE anything is spawned.
    pub fn verify_and_promote(
        &self,
        candidate: &RecordRef,
        baseline: &ProjectSnapshot,
        fixed: &ProjectSnapshot,
        recipe: &VerificationRecipe,
        cancel: &Cancellation,
    ) -> Result<VerificationReport> {
        let fence = DependencyFence::for_run(baseline.binding(), fixed.binding())?;
        let item = self.latest(candidate, &fence)?;
        let x = match item.record {
            KnowledgeRecord::Candidate(x) => x,
            _ => return Err(VerificationError::GateDenied),
        };
        identity(&x.header.metadata, fixed.binding())?;
        recipe.validate(fixed.binding())?;
        recipe.validate(baseline.binding())?;
        oracle_preserving_patch(recipe, baseline.binding(), fixed.binding())?;
        baseline.recheck_source()?;
        fixed.recheck_source()?;
        if baseline.binding().snapshot_sha256 == fixed.binding().snapshot_sha256 {
            return Err(VerificationError::Identity);
        }
        let (run, procedure_run) = self.execute_run(
            candidate,
            &x.header.metadata,
            baseline,
            fixed,
            recipe,
            cancel,
        )?;
        let run_sha256 = digest(&run)?;
        let state = self.checked_state(&run)?;
        let pattern = if state == ActualCheckedState::Verified {
            fixed.recheck_source()?;
            self.latest(candidate, &run.dependency_fence)?;
            let mut header = x.header.clone();
            header.revision += 1;
            header.previous = Some(candidate.clone());
            header.timestamp_ms = now()?;
            header.audit = Audit {
                actor: "stage4-runner".into(),
                reason: "actual repeated bounded verification".into(),
            };
            Some(
                self.store
                    .append_revision(
                        KnowledgeRecord::VerifiedPattern(VerifiedPattern {
                            header,
                            candidate: candidate.clone(),
                            checks: run.checks.clone(),
                            statement: x.statement,
                        }),
                        candidate,
                    )?
                    .reference,
            )
        } else {
            None
        };
        Ok(VerificationReport {
            actual_checked_state: state,
            pattern,
            procedure_run,
            run_sha256,
            policy_sha256: run.policy_sha256.clone(),
            checks: run.checks,
        })
    }
    /// Execute and persist the immutable signed check/run receipts (no promotion).
    /// The run's dependency fence is derived here from the two snapshot bindings.
    fn execute_run(
        &self,
        candidate: &RecordRef,
        metadata: &KnowledgeMetadata,
        baseline: &ProjectSnapshot,
        fixed: &ProjectSnapshot,
        recipe: &VerificationRecipe,
        cancel: &Cancellation,
    ) -> Result<(RunReceipt, RecordRef)> {
        let dependency_fence = DependencyFence::for_run(baseline.binding(), fixed.binding())?;
        let run_id = format!("run-{}", isolation::nonce()?);
        let recipe_sha256 = digest(recipe)?;
        let policy_sha256 = self.policy_sha256()?;
        let patch_sha256 = patch_digest(baseline.binding(), fixed.binding())?;
        let mut checks = Vec::new();
        for (snapshot, is_baseline, repetition, case) in
            std::iter::once((baseline, true, 0, &recipe.cases[0])).chain(
                (1..=recipe.repetitions).flat_map(|repetition| {
                    recipe
                        .cases
                        .iter()
                        .map(move |case| (fixed, false, repetition, case))
                }),
            )
        {
            let outputs = case.expected_files.keys().cloned().collect::<Vec<_>>();
            let actual =
                self.isolation
                    .run(snapshot, &case.argv, &outputs, &recipe.limits, cancel)?;
            let check = CheckReceipt {
                run_id: run_id.clone(),
                candidate: candidate.clone(),
                snapshot: snapshot.binding().clone(),
                recipe_sha256: recipe_sha256.clone(),
                policy_sha256: policy_sha256.clone(),
                patch_sha256: patch_sha256.clone(),
                baseline: is_baseline,
                repetition,
                case: case.clone(),
                actual,
            };
            checks.push(
                self.save_receipt("check", EvidenceKind::CheckResult, &check, metadata, &[])?
                    .reference,
            );
        }
        let run = RunReceipt {
            run_id,
            candidate: candidate.clone(),
            baseline: baseline.binding().clone(),
            fixed: fixed.binding().clone(),
            recipe: recipe.clone(),
            recipe_sha256,
            policy_sha256,
            patch_sha256,
            dependency_fence,
            profile: self.isolation.profile().clone(),
            checks: checks.clone(),
            timestamp_ms: now()?,
        };
        let procedure_run = self
            .save_receipt("run", EvidenceKind::ProcedureRun, &run, metadata, &checks)?
            .reference;
        Ok((run, procedure_run))
    }
    fn checked_state(&self, run: &RunReceipt) -> Result<ActualCheckedState> {
        run.recipe.validate(&run.fixed)?;
        run.recipe.validate(&run.baseline)?;
        oracle_preserving_patch(&run.recipe, &run.baseline, &run.fixed)
            .map_err(|_| VerificationError::ForgedProof)?;
        if run.dependency_fence != DependencyFence::for_run(&run.baseline, &run.fixed)?
            || run.fixed.platform != run.profile.platform
            || run.baseline.platform != run.profile.platform
            || run.policy_sha256 != self.policy_sha256()?
            || run.profile != *self.isolation.profile()
            || run.recipe_sha256 != digest(&run.recipe)?
            || run.patch_sha256 != patch_digest(&run.baseline, &run.fixed)?
            || run.checks.len() != 1 + run.recipe.cases.len() * run.recipe.repetitions as usize
        {
            return Err(VerificationError::ForgedProof);
        }
        let mut all_pass = true;
        let mut cancelled = false;
        let mut normalized: BTreeMap<usize, String> = BTreeMap::new();
        for (i, reference) in run.checks.iter().enumerate() {
            let check: CheckReceipt = self.fenced_receipt(
                reference,
                "check",
                EvidenceKind::CheckResult,
                &run.dependency_fence,
            )?;
            let baseline = i == 0;
            let index = if baseline {
                0
            } else {
                (i - 1) % run.recipe.cases.len()
            };
            let repetition = if baseline {
                0
            } else {
                ((i - 1) / run.recipe.cases.len() + 1) as u32
            };
            if check.run_id != run.run_id
                || check.candidate != run.candidate
                || check.recipe_sha256 != run.recipe_sha256
                || check.policy_sha256 != run.policy_sha256
                || check.patch_sha256 != run.patch_sha256
                || check.baseline != baseline
                || check.repetition != repetition
                || check.case != run.recipe.cases[index]
                || check.snapshot
                    != if baseline {
                        run.baseline.clone()
                    } else {
                        run.fixed.clone()
                    }
                || check.actual.argv != check.case.argv
                || check.actual.log_sha256 != isolation::log_hash(&check.actual)?
            {
                return Err(VerificationError::ForgedProof);
            }
            let passed = case_passed(&check.case, &check.actual);
            if baseline {
                all_pass &= !passed
                    && check.actual.termination == Termination::Completed
                    && check.actual.status.is_some();
            } else {
                all_pass &= passed;
                // Repeatability compares observed exact status, output and files,
                // not durations, claimed labels or two identical stored receipts.
                let observed = check.actual.log_sha256.clone();
                if let Some(first) = normalized.get(&index) {
                    all_pass &= *first == observed;
                } else {
                    normalized.insert(index, observed);
                }
            }
            cancelled |= check.actual.termination == Termination::Cancelled;
        }
        Ok(if cancelled {
            ActualCheckedState::Cancelled
        } else if all_pass {
            ActualCheckedState::Verified
        } else {
            ActualCheckedState::Failed
        })
    }
    fn valid_pattern_run(
        &self,
        pattern: &RecordRef,
        run_ref: &RecordRef,
    ) -> Result<(VerifiedPattern, RunReceipt, String)> {
        let run = self.read_run(run_ref)?;
        let stored = self.fenced(pattern, &run.dependency_fence)?;
        let x = match stored.record {
            KnowledgeRecord::VerifiedPattern(x) => x,
            _ => return Err(VerificationError::GateDenied),
        };
        if x.candidate != run.candidate
            || x.checks != run.checks
            || identity(&x.header.metadata, &run.fixed).is_err()
            || self.checked_state(&run)? != ActualCheckedState::Verified
        {
            return Err(VerificationError::GateDenied);
        }
        let age = now()?
            .checked_sub(run.timestamp_ms)
            .ok_or(VerificationError::GateDenied)?;
        if age > self.policy.max_receipt_age_ms {
            return Err(VerificationError::GateDenied);
        }
        let candidate = self.fenced(&run.candidate, &run.dependency_fence)?;
        let candidate = match candidate.record {
            KnowledgeRecord::Candidate(c) => c,
            _ => return Err(VerificationError::ForgedProof),
        };
        if x.header.logical_id != run.candidate.logical_id
            || x.header.revision != run.candidate.revision + 1
            || x.header.previous.as_ref() != Some(&run.candidate)
            || x.header.metadata != candidate.header.metadata
            || x.statement != candidate.statement
        {
            return Err(VerificationError::ForgedProof);
        }
        let run_hash = digest(&run)?;
        Ok((x, run, run_hash))
    }
    /// UI-only trusted controller surface. No Harness tool definition/LLM metadata.
    /// Call only for the user's affirmative approval of the exact displayed tuple.
    /// Every gate runs BEFORE the UI receipt is persisted (no orphan on denial).
    pub fn approve_from_ui(
        &self,
        event: UiApprovalEvent,
        current: &ProjectSnapshot,
    ) -> Result<RecordRef> {
        let (pattern, run, run_hash) =
            self.valid_pattern_run(&event.pattern, &event.procedure_run)?;
        self.latest(&event.pattern, &run.dependency_fence)?;
        current.recheck_source()?;
        if current.binding() != &run.fixed
            || event.displayed_run_sha256 != run_hash
            || event.displayed_policy_sha256 != self.policy_sha256()?
        {
            return Err(VerificationError::GateDenied);
        }
        let outcome = self.outcome(&run, &run_hash, true)?;
        if !outcome
            .promotable()
            .map_err(|_| VerificationError::GateDenied)?
        {
            return Err(VerificationError::GateDenied);
        }
        // Final fresh checks immediately before the first write.
        current.recheck_source()?;
        self.latest(&event.pattern, &run.dependency_fence)?;
        let approval = ApprovalReceipt {
            pattern: event.pattern.clone(),
            procedure_run: event.procedure_run.clone(),
            run_sha256: run_hash.clone(),
            policy_sha256: event.displayed_policy_sha256,
            timestamp_ms: now()?,
            event_id: format!("ui-{}", isolation::nonce()?),
        };
        let approval_ref = self
            .save_receipt(
                "ui",
                EvidenceKind::UiApproval,
                &approval,
                &pattern.header.metadata,
                &[event.pattern.clone(), event.procedure_run.clone()],
            )?
            .reference;
        let mut header = pattern.header;
        header.revision += 1;
        header.previous = Some(event.pattern.clone());
        header.timestamp_ms = approval.timestamp_ms;
        header.audit = Audit {
            actor: "stage4-ui-controller".into(),
            reason: "explicit exact revision/run/policy approval".into(),
        };
        Ok(self
            .store
            .append_revision(
                KnowledgeRecord::ApprovedProcedure(Box::new(ApprovedProcedure {
                    header,
                    pattern: event.pattern.clone(),
                    outcome,
                    outcome_evidence: vec![event.procedure_run],
                    approval_evidence: approval_ref,
                })),
                &event.pattern,
            )?
            .reference)
    }
    fn outcome(
        &self,
        run: &RunReceipt,
        run_hash: &str,
        explicit: bool,
    ) -> Result<ProcedureOutcome> {
        let verified = self.checked_state(run)? == ActualCheckedState::Verified;
        let requirements = PromotionRequirements {
            bounded_arguments: run.recipe.validate(&run.fixed).is_ok(),
            repeatable_success: verified && run.recipe.repetitions >= 2,
            verified_postconditions: verified,
            low_risk_class: run.profile.backend == isolation::ISOLATION_PROFILE,
            no_contradictory_evidence: self.fenced(&run.candidate, &run.dependency_fence).is_ok(),
            explicit_approval: explicit,
        };
        Ok(ProcedureOutcome {
            schema: unoone_capability_contracts::schemas::PROCEDURE.into(),
            procedure_id: run.run_id.clone(),
            bounded_arguments: String::from_utf8(bytes(&run.recipe)?)
                .map_err(|_| VerificationError::ForgedProof)?,
            preconditions: run.fixed.snapshot_sha256.clone(),
            postconditions: run_hash.into(),
            result: if verified {
                ProcedureResult::Success
            } else {
                ProcedureResult::Failure
            },
            failure_reason: None,
            verification: Verification {
                verified,
                evidence: run_hash.into(),
            },
            risk_class: "LOW".into(),
            promotion: Promotion {
                status: if explicit {
                    PromotionStatus::Approved
                } else {
                    PromotionStatus::Suggested
                },
                policy_version: self.policy.version.clone(),
                requirements,
            },
            timestamp_ms: run.timestamp_ms,
            provenance: Provenance {
                platform: run.fixed.platform.clone(),
                device_id: "isolated-local-runtime".into(),
                source: "stage4-actual-runner".into(),
                model: None,
                artifact_sha256: Some(run.patch_sha256.clone()),
            },
        })
    }
    /// Fresh encrypted active reads, latest identity, incoming/transitive
    /// contradiction checks, receipt MACs and independent recomputed outcome.
    pub fn autonomous_reuse_permit(
        &self,
        approved: &RecordRef,
        current: &ProjectSnapshot,
    ) -> Result<AutonomousReusePermit> {
        current.recheck_source()?;
        let procedure = match self.targeted(approved)?.0.record {
            KnowledgeRecord::ApprovedProcedure(x) => x,
            _ => return Err(VerificationError::GateDenied),
        };
        if procedure.outcome_evidence.len() != 1 {
            return Err(VerificationError::GateDenied);
        }
        let (_, run, run_hash) =
            self.valid_pattern_run(&procedure.pattern, &procedure.outcome_evidence[0])?;
        // The approved record's whole transitive closure (pattern, run, UI receipt,
        // candidate evidence, any edge) must satisfy the run's signed fence.
        self.latest(approved, &run.dependency_fence)?;
        let approval: ApprovalReceipt = self.fenced_receipt(
            &procedure.approval_evidence,
            "ui",
            EvidenceKind::UiApproval,
            &run.dependency_fence,
        )?;
        if procedure.header.logical_id != procedure.pattern.logical_id
            || procedure.header.revision != procedure.pattern.revision + 1
            || procedure.header.previous.as_ref() != Some(&procedure.pattern)
            || procedure.header.timestamp_ms != approval.timestamp_ms
            || identity(&procedure.header.metadata, &run.fixed).is_err()
            || approval.pattern != procedure.pattern
            || approval.procedure_run != procedure.outcome_evidence[0]
            || approval.run_sha256 != run_hash
            || approval.policy_sha256 != self.policy_sha256()?
            || current.binding() != &run.fixed
            || procedure.outcome != self.outcome(&run, &run_hash, true)?
            || !procedure
                .outcome
                .promotable()
                .map_err(|_| VerificationError::GateDenied)?
        {
            return Err(VerificationError::GateDenied);
        }
        Ok(AutonomousReusePermit {
            approved: approved.clone(),
            run_sha256: run_hash,
            snapshot_sha256: run.fixed.snapshot_sha256,
            policy_sha256: self.policy_sha256()?,
        })
    }
    /// Stage4 precondition replay only, not arbitrary autonomous execution/jobs.
    /// The consuming call rechecks revocation/lock/current identity BEFORE spawn.
    pub fn replay_with_permit(
        &self,
        permit: AutonomousReusePermit,
        current: &ProjectSnapshot,
        cancel: &Cancellation,
    ) -> Result<Vec<ActualRun>> {
        let fresh = self.autonomous_reuse_permit(&permit.approved, current)?;
        if fresh.run_sha256 != permit.run_sha256
            || fresh.snapshot_sha256 != permit.snapshot_sha256
            || fresh.policy_sha256 != permit.policy_sha256
        {
            return Err(VerificationError::GateDenied);
        }
        let x = match self.targeted(&permit.approved)?.0.record {
            KnowledgeRecord::ApprovedProcedure(x) => x,
            _ => return Err(VerificationError::GateDenied),
        };
        let run = self.read_run(&x.outcome_evidence[0])?;
        let mut actual = Vec::new();
        for c in &run.recipe.cases {
            self.latest(&permit.approved, &run.dependency_fence)?;
            current.recheck_source()?;
            let outputs = c.expected_files.keys().cloned().collect::<Vec<_>>();
            let result =
                self.isolation
                    .run(current, &c.argv, &outputs, &run.recipe.limits, cancel)?;
            let check = CheckReceipt {
                run_id: format!("replay-{}", isolation::nonce()?),
                candidate: run.candidate.clone(),
                snapshot: current.binding().clone(),
                recipe_sha256: run.recipe_sha256.clone(),
                policy_sha256: run.policy_sha256.clone(),
                patch_sha256: run.patch_sha256.clone(),
                baseline: false,
                repetition: 1,
                case: c.clone(),
                actual: result.clone(),
            };
            self.save_receipt(
                "check",
                EvidenceKind::CheckResult,
                &check,
                &x.header.metadata,
                std::slice::from_ref(&permit.approved),
            )?;
            if !case_passed(c, &result) {
                return Err(VerificationError::GateDenied);
            }
            actual.push(result);
        }
        Ok(actual)
    }
    /// Append-only revocation of this exact approval; no deletion/overwrite.
    pub fn revoke_from_ui(&self, approved: &RecordRef) -> Result<RecordRef> {
        let item = self.active(approved)?;
        if !matches!(item.record, KnowledgeRecord::ApprovedProcedure(_)) {
            return Err(VerificationError::GateDenied);
        }
        let mut header = item.record.header().clone();
        header.logical_id = format!("revoke-{}", isolation::nonce()?);
        header.revision = 1;
        header.previous = None;
        header.edges.clear();
        header.timestamp_ms = now()?;
        header.audit = Audit {
            actor: "stage4-ui-controller".into(),
            reason: "explicit procedure revocation".into(),
        };
        Ok(self
            .store
            .invalidate(KnowledgeRecord::Invalidation(Invalidation {
                header,
                target: approved.clone(),
                reason: "explicit UI revocation".into(),
            }))?
            .reference)
    }
    fn save_receipt<T: Serialize>(
        &self,
        domain: &str,
        kind: EvidenceKind,
        body: &T,
        metadata: &KnowledgeMetadata,
        references: &[RecordRef],
    ) -> Result<crate::knowledge::RecordMapping> {
        let logical_id = format!("{domain}-{}", isolation::nonce()?);
        let mut sealed = Sealed {
            schema: RECEIPT_SCHEMA.into(),
            logical_id: logical_id.clone(),
            evidence_kind: kind,
            body,
            tag: String::new(),
        };
        let unsigned = bytes(&sealed)?;
        sealed.tag = self.store.with_vault(|vault| {
            Ok(mac(vault, domain, &unsigned)?
                .finalize()
                .into_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect())
        })?;
        let content =
            String::from_utf8(bytes(&sealed)?).map_err(|_| VerificationError::ForgedProof)?;
        let header = RecordHeader {
            schema: KNOWLEDGE_SCHEMA.into(),
            logical_id,
            revision: 1,
            previous: None,
            timestamp_ms: now()?,
            audit: Audit {
                actor: "stage4-runner".into(),
                reason: format!("immutable actual {domain} receipt"),
            },
            metadata: metadata.clone(),
            edges: references
                .iter()
                .map(|r| GraphEdge {
                    relation: EdgeKind::Derived,
                    target: r.clone(),
                })
                .collect(),
        };
        Ok(self.store.create(KnowledgeRecord::Evidence(Evidence {
            header,
            evidence_kind: kind,
            content_sha256: hash(content.as_bytes()),
            content,
        }))?)
    }
    /// Unfenced authenticated audit read (tests/diagnostics only, never a gate).
    #[cfg(test)]
    fn read_receipt<T: Serialize + DeserializeOwned>(
        &self,
        reference: &RecordRef,
        domain: &str,
        kind: EvidenceKind,
    ) -> Result<T> {
        Ok(self.authenticated(reference, domain, kind)?.0)
    }
    fn fenced_receipt<T: Serialize + DeserializeOwned>(
        &self,
        reference: &RecordRef,
        domain: &str,
        kind: EvidenceKind,
        fence: &DependencyFence,
    ) -> Result<T> {
        let (body, dependencies) = self.authenticated(reference, domain, kind)?;
        fence.require(&dependencies)?;
        Ok(body)
    }
    /// The signed run carries its own fence; it must equal the fence re-derived
    /// from its signed snapshot bindings, and the run's closure must satisfy it.
    fn read_run(&self, reference: &RecordRef) -> Result<RunReceipt> {
        let (run, dependencies): (RunReceipt, _) =
            self.authenticated(reference, "run", EvidenceKind::ProcedureRun)?;
        if run.dependency_fence != DependencyFence::for_run(&run.baseline, &run.fixed)? {
            return Err(VerificationError::ForgedProof);
        }
        run.dependency_fence.require(&dependencies)?;
        Ok(run)
    }
    fn authenticated<T: Serialize + DeserializeOwned>(
        &self,
        reference: &RecordRef,
        domain: &str,
        kind: EvidenceKind,
    ) -> Result<(T, Vec<KnowledgeMetadata>)> {
        let (item, dependencies) = self.targeted(reference)?;
        let evidence = match item.record {
            KnowledgeRecord::Evidence(e) if e.evidence_kind == kind => e,
            _ => return Err(VerificationError::ForgedProof),
        };
        let mut sealed: Sealed<T> =
            serde_json::from_str(&evidence.content).map_err(|_| VerificationError::ForgedProof)?;
        if sealed.schema != RECEIPT_SCHEMA
            || sealed.logical_id != reference.logical_id
            || sealed.evidence_kind != kind
            || bytes(&sealed)? != evidence.content.as_bytes()
            || !valid_digest(&sealed.tag)
        {
            return Err(VerificationError::ForgedProof);
        }
        let tag = sealed.tag.clone();
        sealed.tag.clear();
        let unsigned = bytes(&sealed)?;
        let mut decoded = [0u8; 32];
        for (i, b) in decoded.iter_mut().enumerate() {
            *b = u8::from_str_radix(&tag[2 * i..2 * i + 2], 16)
                .map_err(|_| VerificationError::ForgedProof)?;
        }
        self.store
            .with_vault(|vault| {
                mac(vault, domain, &unsigned)?
                    .verify_slice(&decoded)
                    .map_err(|_| KnowledgeStoreError::Corrupt)
            })
            .map_err(|_| VerificationError::ForgedProof)?;
        Ok((sealed.body, dependencies))
    }
}
fn mac(
    vault: &Vault,
    domain: &str,
    bytes: &[u8],
) -> std::result::Result<Hmac<Sha256>, KnowledgeStoreError> {
    if !["check", "run", "ui"].contains(&domain) {
        return Err(KnowledgeStoreError::Corrupt);
    }
    let root = vault.master_key().ok_or(KnowledgeStoreError::Locked)?;
    let mut key = unoone_vault_core::crypto::derive_domain_key(
        root,
        &format!("inbharat.pai.stage4.{domain}.hmac-sha256.v1"),
    );
    let result = Hmac::<Sha256>::new_from_slice(&key);
    unoone_vault_core::crypto::secure_zero(&mut key);
    let mut mac = result.map_err(|_| KnowledgeStoreError::Corrupt)?;
    mac.update(b"inbharat.pai.actual-runtime-receipt\0v1\0");
    mac.update(domain.as_bytes());
    mac.update(b"\0");
    mac.update(bytes);
    Ok(mac)
}
fn identity(metadata: &KnowledgeMetadata, binding: &SnapshotBinding) -> Result<()> {
    let fence: ApplicabilityFence = serde_json::from_str(&metadata.applicability.constraints)
        .map_err(|_| VerificationError::Identity)?;
    if fence.schema != FENCE_SCHEMA
        || fence.file_digest != binding.source.file_digest
        || metadata.source_id != binding.source.source_id
        || metadata.source_version != binding.source.source_version
        || metadata.source_commit != binding.source.source_commit
        || metadata.applicability.platforms != [binding.platform.clone()]
    {
        return Err(VerificationError::Identity);
    }
    Ok(())
}
fn patch_digest(baseline: &SnapshotBinding, fixed: &SnapshotBinding) -> Result<String> {
    let paths = baseline
        .files
        .keys()
        .chain(fixed.files.keys())
        .collect::<BTreeSet<_>>();
    let delta = paths
        .into_iter()
        .filter_map(|p| {
            let before = baseline.files.get(p);
            let after = fixed.files.get(p);
            if before == after {
                None
            } else {
                Some((p, before, after))
            }
        })
        .collect::<Vec<_>>();
    if delta.is_empty() {
        return Err(VerificationError::Identity);
    }
    digest(&delta)
}
/// B3: the declared oracle/test files are byte-identical between baseline and
/// fixed (so the baseline failed against the SAME test bytes), the selected file
/// sets match, and at least one declared implementation file actually changed.
fn oracle_preserving_patch(
    recipe: &VerificationRecipe,
    baseline: &SnapshotBinding,
    fixed: &SnapshotBinding,
) -> Result<()> {
    if baseline.primary != fixed.primary
        || !baseline.files.keys().eq(fixed.files.keys())
        || recipe
            .oracle_files
            .iter()
            .any(|p| !baseline.files.contains_key(p) || baseline.files.get(p) != fixed.files.get(p))
        || !recipe
            .implementation_files
            .iter()
            .any(|p| baseline.files.get(p) != fixed.files.get(p))
    {
        return Err(VerificationError::InvalidRecipe);
    }
    Ok(())
}
fn case_passed(case: &VerificationCase, actual: &ActualRun) -> bool {
    actual.termination == Termination::Completed
        && actual.status == Some(case.expected_status)
        && actual.stdout_hex == hex(case.expected_stdout.as_bytes())
        && actual.stderr_hex == hex(case.expected_stderr.as_bytes())
        && actual.output_files == case.expected_files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isolation::SnapshotPolicy;
    use crate::knowledge_retrieval::TrustedSource;
    use std::fs;
    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "needs the Linux isolation backend (non-Linux fails closed before any spawn)"
    )]
    fn verification_real_reproducer_promotion_receipts_and_live_approval_gates() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "verification_real_reproducer_promotion_receipts_and_live_approval_gates",
        ) {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("bug");
        let fixed_root = temp.path().join("fixed");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&fixed_root).unwrap();
        // Synthetic division bug. Negative input MUST reject; regression zero MUST work.
        // -I excludes current directory: explicitly admit ONLY readonly /work.
        let mut test_code=b"import sys; sys.path.insert(0,'/work')\nif sys.argv[1]=='slow':\n import time; time.sleep(2)\nif sys.argv[1]=='loud':\n print('x'*20000); sys.exit(0)\n".to_vec();
        test_code.extend_from_slice(b"import sys\nfrom calc import divide\nmode=sys.argv[1]\nif mode=='negative':\n try: divide(8,0)\n except ValueError: print('rejected');sys.exit(2)\n raise AssertionError('zero accepted')\nx=divide(8,2) if mode=='positive' else divide(0,2)\nassert x==(4 if mode=='positive' else 0),x\nopen('/tmp/result','w').write(str(x))\nprint('ok:'+str(x))\n");
        let buggy = b"def divide(a,b):\n if b==0: raise ValueError('zero')\n return a//b+1\n";
        let corrected = b"def divide(a,b):\n if b==0: raise ValueError('zero')\n return a//b\n";
        for root in [&old, &fixed_root] {
            fs::write(root.join("check.py"), &test_code).unwrap();
        }
        fs::write(old.join("calc.py"), buggy).unwrap();
        fs::write(fixed_root.join("calc.py"), corrected).unwrap();
        let snapshots = SnapshotPolicy::new(
            vec![old.clone(), fixed_root.clone()],
            temp.path().to_owned(),
        )
        .unwrap();
        let source = |code: &[u8]| TrustedSource {
            source_id: "stage4-synthetic-private".into(),
            source_version: "1".into(),
            source_commit: "b".repeat(40),
            file_digest: hash(code),
        };
        let files = vec!["calc.py".into(), "check.py".into()];
        let baseline = snapshots
            .capture(&old, source(buggy), "calc.py", &files)
            .unwrap();
        let fixed = snapshots
            .capture(&fixed_root, source(corrected), "calc.py", &files)
            .unwrap();
        let vault_root = temp.path().join("vault");
        let password = b"stage4-only-synthetic-vault";
        Vault::create(&vault_root, password).unwrap();
        let mut vault = Vault::open(&vault_root).unwrap();
        vault.unlock(password).unwrap();
        let handle = Arc::new(Mutex::new(Some(vault)));
        let store = EvidenceVault::new(handle.clone());
        store.initialize().unwrap();
        let metadata = KnowledgeMetadata {
            source_id: source(corrected).source_id,
            source_version: "1".into(),
            source_commit: "b".repeat(40),
            license: "synthetic-only".into(),
            privacy: KnowledgePrivacy::Private,
            applicability: Applicability {
                topics: vec!["synthetic division".into()],
                platforms: vec![fixed.binding().platform.clone()],
                constraints: ApplicabilityFence {
                    schema: FENCE_SCHEMA.into(),
                    file_digest: hash(corrected),
                }
                .encode()
                .unwrap(),
            },
        };
        let header = |id: &str| RecordHeader {
            schema: KNOWLEDGE_SCHEMA.into(),
            logical_id: id.into(),
            revision: 1,
            previous: None,
            timestamp_ms: 1,
            audit: Audit {
                actor: "test-owner".into(),
                reason: "synthetic Stage4".into(),
            },
            metadata: metadata.clone(),
            edges: vec![],
        };
        let observation = store
            .create(KnowledgeRecord::Evidence(Evidence {
                header: header("observation"),
                evidence_kind: EvidenceKind::Observation,
                content: "synthetic bug report".into(),
                content_sha256: hash(b"synthetic bug report"),
            }))
            .unwrap()
            .reference;
        let candidate = store
            .create(KnowledgeRecord::Candidate(Candidate {
                header: header("division-pattern"),
                statement: "bounded division correct for tested cases".into(),
                evidence: vec![observation.clone()],
            }))
            .unwrap()
            .reference;
        let recipe = VerificationRecipe {
            schema: RECIPE_SCHEMA.into(),
            repetitions: 2,
            limits: RunLimits {
                cpu_seconds: 2,
                memory_bytes: 128 * 1024 * 1024,
                processes: 32,
                timeout_ms: 1000,
                output_bytes: 1024,
            },
            cases: [
                ("positive", CaseKind::Positive, 0, "ok:4\n", Some("4")),
                ("negative", CaseKind::Negative, 2, "rejected\n", None),
                ("regression", CaseKind::Regression, 0, "ok:0\n", Some("0")),
            ]
            .into_iter()
            .map(|(name, kind, status, out, file)| VerificationCase {
                name: name.into(),
                kind,
                argv: vec![
                    "/usr/bin/python3".into(),
                    "-I".into(),
                    "/work/check.py".into(),
                    name.into(),
                ],
                expected_status: status,
                expected_stdout: out.into(),
                expected_stderr: String::new(),
                expected_files: file
                    .into_iter()
                    .map(|f| ("/tmp/result".into(), hash(f.as_bytes())))
                    .collect(),
            })
            .collect(),
            oracle_files: ["check.py".to_string()].into(),
            implementation_files: ["calc.py".to_string()].into(),
        };
        let verifier = KnowledgeVerifier::new(
            handle.clone(),
            VerificationPolicy::readonly_replay(600_000).unwrap(),
        )
        .unwrap();
        let mut once = recipe.clone();
        once.repetitions = 1;
        assert!(verifier
            .verify_and_promote(
                &candidate,
                &baseline,
                &fixed,
                &once,
                &Cancellation::default()
            )
            .is_err());
        let mut wrong = recipe.clone();
        wrong.cases[0]
            .expected_files
            .insert("/tmp/result".into(), hash(b"false"));
        let failed = verifier
            .verify_and_promote(
                &candidate,
                &baseline,
                &fixed,
                &wrong,
                &Cancellation::default(),
            )
            .unwrap();
        assert_eq!(failed.actual_checked_state, ActualCheckedState::Failed);
        assert!(failed.pattern.is_none());
        assert_eq!(
            store
                .read_latest(&candidate.logical_id)
                .unwrap()
                .mapping
                .reference,
            candidate
        );
        for (mode, termination) in [
            ("slow", Termination::Timeout),
            ("loud", Termination::OutputLimit),
        ] {
            let mut bounded = recipe.clone();
            bounded.cases[0].argv[3] = mode.into();
            bounded.limits.timeout_ms = 100;
            let rejected = verifier
                .verify_and_promote(
                    &candidate,
                    &baseline,
                    &fixed,
                    &bounded,
                    &Cancellation::default(),
                )
                .unwrap();
            assert_eq!(rejected.actual_checked_state, ActualCheckedState::Failed);
            assert!(rejected.pattern.is_none());
            let receipt: CheckReceipt = verifier
                .read_receipt(&rejected.checks[1], "check", EvidenceKind::CheckResult)
                .unwrap();
            assert_eq!(receipt.actual.termination, termination);
        }
        let cancelled = Cancellation::default();
        cancelled.cancel();
        let rejected = verifier
            .verify_and_promote(&candidate, &baseline, &fixed, &recipe, &cancelled)
            .unwrap();
        assert_eq!(rejected.actual_checked_state, ActualCheckedState::Cancelled);
        assert!(rejected.pattern.is_none());
        let report = verifier
            .verify_and_promote(
                &candidate,
                &baseline,
                &fixed,
                &recipe,
                &Cancellation::default(),
            )
            .unwrap();
        assert_eq!(report.actual_checked_state, ActualCheckedState::Verified);
        assert_eq!(report.checks.len(), 7);
        let pattern = report.pattern.clone().unwrap();
        assert!(!store.read(&pattern).unwrap().record.authorizes_execution());
        let first: CheckReceipt = verifier
            .read_receipt(&report.checks[0], "check", EvidenceKind::CheckResult)
            .unwrap();
        assert_eq!(first.actual.status, Some(1));
        assert!(!case_passed(&first.case, &first.actual));
        let run: RunReceipt = verifier
            .read_receipt(&report.procedure_run, "run", EvidenceKind::ProcedureRun)
            .unwrap();
        assert_eq!(run.recipe.repetitions, 2);
        assert!(!run.profile.proc_mounted);
        assert_eq!(
            run.patch_sha256,
            patch_digest(baseline.binding(), fixed.binding()).unwrap()
        );
        // Forge canonical plausible approved proof via real EvidenceVault writer.
        let actual_evidence = match store.read(&report.checks[1]).unwrap().record {
            KnowledgeRecord::Evidence(e) => e,
            _ => unreachable!(),
        };
        let mut forged: Sealed<CheckReceipt> =
            serde_json::from_str(&actual_evidence.content).unwrap();
        forged.logical_id = "forged-check".into();
        forged.body.actual.status = Some(0);
        forged.tag = "0".repeat(64);
        let content = String::from_utf8(bytes(&forged).unwrap()).unwrap();
        let forged_ref = store
            .create(KnowledgeRecord::Evidence(Evidence {
                header: header("forged-check"),
                evidence_kind: EvidenceKind::CheckResult,
                content_sha256: hash(content.as_bytes()),
                content,
            }))
            .unwrap()
            .reference;
        assert!(matches!(
            verifier.read_receipt::<CheckReceipt>(&forged_ref, "check", EvidenceKind::CheckResult),
            Err(VerificationError::ForgedProof)
        ));
        let mut swapped = run.clone();
        swapped.checks[1] = forged_ref;
        assert!(verifier.checked_state(&swapped).is_err());
        assert!(verifier
            .read_receipt::<RunReceipt>(&report.procedure_run, "ui", EvidenceKind::ProcedureRun)
            .is_err());
        let event = UiApprovalEvent {
            pattern: pattern.clone(),
            procedure_run: report.procedure_run.clone(),
            displayed_run_sha256: report.run_sha256.clone(),
            displayed_policy_sha256: report.policy_sha256.clone(),
        };
        let mut mismatch = event.clone();
        mismatch.displayed_run_sha256 = "0".repeat(64);
        assert!(verifier.approve_from_ui(mismatch, &fixed).is_err());
        let approved = verifier.approve_from_ui(event, &fixed).unwrap();
        let mut fabricated = store.read(&approved).unwrap().record;
        if let KnowledgeRecord::ApprovedProcedure(x) = &mut fabricated {
            x.header.logical_id = "fabricated-approved".into();
            x.header.revision = 1;
            x.header.previous = None;
        }
        let fabricated = store.create(fabricated).unwrap().reference;
        assert!(verifier
            .autonomous_reuse_permit(&fabricated, &fixed)
            .is_err());
        let mut stale_run = run.clone();
        stale_run.fixed.platform = "windows-x86_64".into();
        assert!(verifier.checked_state(&stale_run).is_err());
        let mut wrong_policy = VerificationPolicy::readonly_replay(500_000).unwrap();
        wrong_policy.version = POLICY_VERSION.into();
        let policy_verifier = KnowledgeVerifier::new(handle.clone(), wrong_policy).unwrap();
        assert!(policy_verifier
            .autonomous_reuse_permit(&approved, &fixed)
            .is_err());
        let permit = verifier.autonomous_reuse_permit(&approved, &fixed).unwrap();
        let replay = verifier
            .replay_with_permit(permit, &fixed, &Cancellation::default())
            .unwrap();
        assert_eq!(replay.len(), 3);
        let permit = verifier.autonomous_reuse_permit(&approved, &fixed).unwrap();
        // Version/commit/platform/hash stale roots fail closed, before any runner.
        fs::write(fixed_root.join("calc.py"), buggy).unwrap();
        assert!(verifier.autonomous_reuse_permit(&approved, &fixed).is_err());
        fs::write(fixed_root.join("calc.py"), corrected).unwrap();
        for (version, commit) in [("2", "b".repeat(40)), ("1", "c".repeat(40))] {
            let mut s = source(corrected);
            s.source_version = version.into();
            s.source_commit = commit;
            let stale = snapshots
                .capture(&fixed_root, s, "calc.py", &files)
                .unwrap();
            assert!(verifier.autonomous_reuse_permit(&approved, &stale).is_err());
        }
        handle.lock().unwrap().as_mut().unwrap().lock().unwrap();
        assert!(verifier.autonomous_reuse_permit(&approved, &fixed).is_err());
        handle
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .unlock(password)
            .unwrap();
        // Incoming contradiction to provenance denies an already-minted permit.
        let mut contradiction_header = header("contradiction");
        contradiction_header.edges.push(GraphEdge {
            relation: EdgeKind::Contradicting,
            target: observation,
        });
        let contradiction = store
            .create(KnowledgeRecord::Evidence(Evidence {
                header: contradiction_header,
                evidence_kind: EvidenceKind::Observation,
                content: "contradictory test fact".into(),
                content_sha256: hash(b"contradictory test fact"),
            }))
            .unwrap()
            .reference;
        assert!(verifier.autonomous_reuse_permit(&approved, &fixed).is_err());
        assert!(verifier
            .replay_with_permit(permit, &fixed, &Cancellation::default())
            .is_err());
        // Use a second isolated candidate to demonstrate actual revocation liveness,
        // keeping one expensive real Vault fixture for the compound suite.
        let clean = store
            .create(KnowledgeRecord::Candidate(Candidate {
                header: header("clean-pattern"),
                statement: "independent synthetic pattern".into(),
                evidence: vec![report.checks[1].clone()],
            }))
            .unwrap()
            .reference;
        let clean_report = verifier
            .verify_and_promote(&clean, &baseline, &fixed, &recipe, &Cancellation::default())
            .unwrap();
        let clean_approved = verifier
            .approve_from_ui(
                UiApprovalEvent {
                    pattern: clean_report.pattern.unwrap(),
                    procedure_run: clean_report.procedure_run,
                    displayed_run_sha256: clean_report.run_sha256,
                    displayed_policy_sha256: clean_report.policy_sha256,
                },
                &fixed,
            )
            .unwrap();
        let live = verifier
            .autonomous_reuse_permit(&clean_approved, &fixed)
            .unwrap();
        verifier.revoke_from_ui(&clean_approved).unwrap();
        assert!(verifier
            .autonomous_reuse_permit(&clean_approved, &fixed)
            .is_err());
        assert!(verifier
            .replay_with_permit(live, &fixed, &Cancellation::default())
            .is_err());
        let _ = contradiction;
        // Persisted vault bytes contain no private receipt payload/source text.
        fn no_plaintext(root: &std::path::Path) {
            for e in fs::read_dir(root).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    no_plaintext(&p);
                } else {
                    let b = fs::read(p).unwrap();
                    for marker in [
                        "stage4-synthetic-private",
                        "runtime-receipt.v1",
                        "ok:4",
                        "synthetic division",
                    ] {
                        assert!(!b.windows(marker.len()).any(|w| w == marker.as_bytes()));
                    }
                }
            }
        }
        no_plaintext(&vault_root);
        println!(
            "STAGE4_RUNTIME_PROFILE {}",
            serde_json::to_string(verifier.runtime_profile()).unwrap()
        );
        println!(
            "STAGE4_ACTUAL_REPORT {}",
            serde_json::to_string(&report).unwrap()
        );
        println!(
            "STAGE4_BASELINE_RECEIPT {}",
            String::from_utf8(bytes(&first).unwrap()).unwrap()
        );
        println!(
            "STAGE4_RUN_RECEIPT {}",
            String::from_utf8(bytes(&run).unwrap()).unwrap()
        );
        for r in &report.checks {
            let c: CheckReceipt = verifier
                .read_receipt(r, "check", EvidenceKind::CheckResult)
                .unwrap_or_else(|_| {
                    // The first pattern is contradicted later. Exact immutable audit read
                    // still permits synthetic evidence export for this test, not gating.
                    let item = store.read(r).unwrap();
                    match item.record {
                        KnowledgeRecord::Evidence(e) => {
                            serde_json::from_str::<Sealed<CheckReceipt>>(&e.content)
                                .unwrap()
                                .body
                        }
                        _ => unreachable!(),
                    }
                });
            println!(
                "STAGE4_CHECK_RECEIPT {}",
                String::from_utf8(bytes(&c).unwrap()).unwrap()
            );
        }
    }

    // ---- Stage4 repair regressions (B1 dependency fence, B3 oracle immutability) ----
    const REPAIR_CHECK: &[u8] = b"import sys; sys.path.insert(0,'/work')\nfrom calc import divide\nmode=sys.argv[1]\nif mode=='negative':\n try: divide(8,0)\n except ValueError: print('rejected');sys.exit(2)\n raise AssertionError('zero accepted')\nx=divide(8,2) if mode=='positive' else divide(0,2)\nassert x==(4 if mode=='positive' else 0),x\nopen('/tmp/result','w').write(str(x))\nprint('ok:'+str(x))\n";
    const REPAIR_BUGGY: &[u8] =
        b"def divide(a,b):\n if b==0: raise ValueError('zero')\n return a//b+1\n";
    const REPAIR_CORRECT: &[u8] =
        b"def divide(a,b):\n if b==0: raise ValueError('zero')\n return a//b\n";
    /// Reviewer B3 reproduction: rewrites the oracle, never imports calc.
    const REPAIR_HOLLOW: &[u8] = b"import sys\nm=sys.argv[1]\nif m=='negative': print('rejected'); sys.exit(2)\nv='4' if m=='positive' else '0'\nopen('/tmp/result','w').write(v)\nprint('ok:'+v)\n";
    struct RepairFx {
        temp: tempfile::TempDir,
        snapshots: SnapshotPolicy,
        store: EvidenceVault,
        verifier: KnowledgeVerifier,
    }
    fn repair_fx(projects: &[(&str, &[u8], &[u8])]) -> RepairFx {
        let temp = tempfile::tempdir().unwrap();
        let mut roots = Vec::new();
        for (name, calc, check) in projects {
            let root = temp.path().join(name);
            fs::create_dir(&root).unwrap();
            fs::write(root.join("calc.py"), calc).unwrap();
            fs::write(root.join("check.py"), check).unwrap();
            roots.push(root);
        }
        let scratch = temp.path().join("scratch");
        fs::create_dir(&scratch).unwrap();
        let snapshots = SnapshotPolicy::new(roots, scratch).unwrap();
        let vault_root = temp.path().join("vault");
        let password = b"stage4-repair-synthetic-vault";
        Vault::create(&vault_root, password).unwrap();
        let mut vault = Vault::open(&vault_root).unwrap();
        vault.unlock(password).unwrap();
        let handle = Arc::new(Mutex::new(Some(vault)));
        let store = EvidenceVault::new(handle.clone());
        store.initialize().unwrap();
        let verifier = KnowledgeVerifier::new(
            handle,
            VerificationPolicy::readonly_replay(600_000).unwrap(),
        )
        .unwrap();
        RepairFx {
            temp,
            snapshots,
            store,
            verifier,
        }
    }
    fn repair_source(primary: &[u8]) -> TrustedSource {
        TrustedSource {
            source_id: "stage4-repair-synthetic".into(),
            source_version: "1".into(),
            source_commit: "b".repeat(40),
            file_digest: hash(primary),
        }
    }
    fn repair_snapshot(fx: &RepairFx, project: &str) -> ProjectSnapshot {
        let root = fx.temp.path().join(project);
        let calc = fs::read(root.join("calc.py")).unwrap();
        fx.snapshots
            .capture(
                &root,
                repair_source(&calc),
                "calc.py",
                &["calc.py".into(), "check.py".into()],
            )
            .unwrap()
    }
    fn repair_metadata(platform: &str, file_digest: &str) -> KnowledgeMetadata {
        KnowledgeMetadata {
            source_id: "stage4-repair-synthetic".into(),
            source_version: "1".into(),
            source_commit: "b".repeat(40),
            license: "synthetic-only".into(),
            privacy: KnowledgePrivacy::Private,
            applicability: Applicability {
                topics: vec!["synthetic division".into()],
                platforms: vec![platform.into()],
                constraints: ApplicabilityFence {
                    schema: FENCE_SCHEMA.into(),
                    file_digest: file_digest.into(),
                }
                .encode()
                .unwrap(),
            },
        }
    }
    fn repair_header(id: &str, md: &KnowledgeMetadata, edges: Vec<GraphEdge>) -> RecordHeader {
        RecordHeader {
            schema: KNOWLEDGE_SCHEMA.into(),
            logical_id: id.into(),
            revision: 1,
            previous: None,
            timestamp_ms: 1,
            audit: Audit {
                actor: "test-owner".into(),
                reason: "synthetic Stage4 repair".into(),
            },
            metadata: md.clone(),
            edges,
        }
    }
    fn repair_evidence(
        store: &EvidenceVault,
        id: &str,
        md: &KnowledgeMetadata,
        edges: Vec<GraphEdge>,
    ) -> RecordRef {
        store
            .create(KnowledgeRecord::Evidence(Evidence {
                header: repair_header(id, md, edges),
                evidence_kind: EvidenceKind::Observation,
                content: format!("synthetic observation {id}"),
                content_sha256: hash(format!("synthetic observation {id}").as_bytes()),
            }))
            .unwrap()
            .reference
    }
    fn repair_candidate(
        store: &EvidenceVault,
        id: &str,
        md: &KnowledgeMetadata,
        evidence: Vec<RecordRef>,
    ) -> RecordRef {
        store
            .create(KnowledgeRecord::Candidate(Candidate {
                header: repair_header(id, md, vec![]),
                statement: format!("synthetic candidate {id}"),
                evidence,
            }))
            .unwrap()
            .reference
    }
    fn derived(target: &RecordRef) -> GraphEdge {
        GraphEdge {
            relation: EdgeKind::Derived,
            target: target.clone(),
        }
    }
    fn repair_recipe() -> VerificationRecipe {
        VerificationRecipe {
            schema: RECIPE_SCHEMA.into(),
            repetitions: 2,
            limits: RunLimits {
                cpu_seconds: 2,
                memory_bytes: 128 * 1024 * 1024,
                processes: 32,
                timeout_ms: 1000,
                output_bytes: 1024,
            },
            cases: [
                ("positive", CaseKind::Positive, 0, "ok:4\n", Some("4")),
                ("negative", CaseKind::Negative, 2, "rejected\n", None),
                ("regression", CaseKind::Regression, 0, "ok:0\n", Some("0")),
            ]
            .into_iter()
            .map(|(name, kind, status, out, file)| VerificationCase {
                name: name.into(),
                kind,
                argv: vec![
                    "/usr/bin/python3".into(),
                    "-I".into(),
                    "/work/check.py".into(),
                    name.into(),
                ],
                expected_status: status,
                expected_stdout: out.into(),
                expected_stderr: String::new(),
                expected_files: file
                    .into_iter()
                    .map(|f| ("/tmp/result".into(), hash(f.as_bytes())))
                    .collect(),
            })
            .collect(),
            oracle_files: ["check.py".to_string()].into(),
            implementation_files: ["calc.py".to_string()].into(),
        }
    }
    fn repair_ui(report: &VerificationReport) -> UiApprovalEvent {
        UiApprovalEvent {
            pattern: report.pattern.clone().unwrap(),
            procedure_run: report.procedure_run.clone(),
            displayed_run_sha256: report.run_sha256.clone(),
            displayed_policy_sha256: report.policy_sha256.clone(),
        }
    }
    /// A genuine signed run whose promotion append lost a race to a general writer
    /// (the production execute_run, no promotion). Proves approval-time fencing.
    fn unpromoted_run(
        fx: &RepairFx,
        candidate: &RecordRef,
        md: &KnowledgeMetadata,
        baseline: &ProjectSnapshot,
        fixed: &ProjectSnapshot,
        recipe: &VerificationRecipe,
    ) -> (RecordRef, String, Vec<RecordRef>) {
        let (run, run_ref) = fx
            .verifier
            .execute_run(
                candidate,
                md,
                baseline,
                fixed,
                recipe,
                &Cancellation::default(),
            )
            .unwrap();
        (run_ref, digest(&run).unwrap(), run.checks)
    }
    /// Writer front-runs approve_from_ui's append with a byte-identical procedure
    /// (genuine UI receipt + recomputed outcome) plus `extra` header edges.
    fn front_run_approved(
        fx: &RepairFx,
        report: &VerificationReport,
        extra: Vec<GraphEdge>,
    ) -> RecordRef {
        let v = &fx.verifier;
        let pattern_ref = report.pattern.clone().unwrap();
        let pattern = match fx.store.read(&pattern_ref).unwrap().record {
            KnowledgeRecord::VerifiedPattern(x) => x,
            _ => unreachable!(),
        };
        let run: RunReceipt = v
            .read_receipt(&report.procedure_run, "run", EvidenceKind::ProcedureRun)
            .unwrap();
        let approval = ApprovalReceipt {
            pattern: pattern_ref.clone(),
            procedure_run: report.procedure_run.clone(),
            run_sha256: report.run_sha256.clone(),
            policy_sha256: v.policy_sha256().unwrap(),
            timestamp_ms: now().unwrap(),
            event_id: format!("ui-{}", isolation::nonce().unwrap()),
        };
        let ui = v
            .save_receipt(
                "ui",
                EvidenceKind::UiApproval,
                &approval,
                &pattern.header.metadata,
                &[pattern_ref.clone(), report.procedure_run.clone()],
            )
            .unwrap()
            .reference;
        let outcome = v.outcome(&run, &report.run_sha256, true).unwrap();
        let mut header = pattern.header.clone();
        header.revision += 1;
        header.previous = Some(pattern_ref.clone());
        header.timestamp_ms = approval.timestamp_ms;
        header.edges.extend(extra);
        fx.store
            .append_revision(
                KnowledgeRecord::ApprovedProcedure(Box::new(ApprovedProcedure {
                    header,
                    pattern: pattern_ref.clone(),
                    outcome,
                    outcome_evidence: vec![report.procedure_run.clone()],
                    approval_evidence: ui,
                })),
                &pattern_ref,
            )
            .unwrap()
            .reference
    }
    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "needs the Linux isolation backend (non-Linux fails closed before any spawn)"
    )]
    fn repair_b1_dependency_fence_enforced_at_verify_approve_permit_and_replay() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "repair_b1_dependency_fence_enforced_at_verify_approve_permit_and_replay",
        ) {
            return;
        }
        let fx = repair_fx(&[
            ("bug", REPAIR_BUGGY, REPAIR_CHECK),
            ("fixed", REPAIR_CORRECT, REPAIR_CHECK),
        ]);
        let baseline = repair_snapshot(&fx, "bug");
        let fixed = repair_snapshot(&fx, "fixed");
        let platform = fixed.binding().platform.clone();
        let good = repair_metadata(&platform, &hash(REPAIR_CORRECT));
        let recipe = repair_recipe();
        let store = &fx.store;
        let v = &fx.verifier;
        // Legitimate bug report describing the BASELINE bytes is admitted end-to-end.
        let bug_report = repair_evidence(
            store,
            "repair-bug-report",
            &repair_metadata(&platform, &hash(REPAIR_BUGGY)),
            vec![],
        );
        let legit = repair_candidate(store, "repair-legit", &good, vec![bug_report.clone()]);
        let report = v
            .verify_and_promote(&legit, &baseline, &fixed, &recipe, &Cancellation::default())
            .unwrap();
        assert_eq!(report.actual_checked_state, ActualCheckedState::Verified);
        // Denied approvals persist nothing (gates run before the UI receipt write).
        let before = store.catalog().unwrap().len();
        let mut wrong_policy = repair_ui(&report);
        wrong_policy.displayed_policy_sha256 = "0".repeat(64);
        assert!(v.approve_from_ui(wrong_policy, &fixed).is_err());
        assert!(v.approve_from_ui(repair_ui(&report), &baseline).is_err());
        assert_eq!(
            store.catalog().unwrap().len(),
            before,
            "orphan approval receipt"
        );
        let approved = v.approve_from_ui(repair_ui(&report), &fixed).unwrap();
        let permit = v.autonomous_reuse_permit(&approved, &fixed).unwrap();
        assert_eq!(
            v.replay_with_permit(permit, &fixed, &Cancellation::default())
                .unwrap()
                .len(),
            3
        );
        // The fence is part of the signed run body and re-derivable from it.
        let run: RunReceipt = v
            .read_receipt(&report.procedure_run, "run", EvidenceKind::ProcedureRun)
            .unwrap();
        let mut digests = vec![hash(REPAIR_BUGGY), hash(REPAIR_CORRECT)];
        digests.sort();
        assert_eq!(run.dependency_fence.policy, DEPENDENCY_POLICY);
        assert_eq!(run.dependency_fence.file_digests, digests);
        assert_eq!(run.dependency_fence.platform, platform);
        assert_eq!(
            (
                run.dependency_fence.source_id.as_str(),
                run.dependency_fence.source_version.as_str(),
                run.dependency_fence.source_commit.as_str()
            ),
            ("stage4-repair-synthetic", "1", "b".repeat(40).as_str())
        );
        println!(
            "STAGE4_REPAIR_B1 legit_baseline_digest_bug_report verified=true approved=true permit=true replay_cases=3 signed_fence={}",
            serde_json::to_string(&run.dependency_fence).unwrap()
        );
        // A run whose recorded fence differs from its snapshots is not a proof.
        let mut tampered = run.clone();
        tampered.dependency_fence.file_digests = vec![hash(b"anything")];
        assert!(matches!(
            v.checked_state(&tampered),
            Err(VerificationError::ForgedProof)
        ));
        // Every single-field applicability mismatch fails closed BEFORE any run.
        let mut stale = Vec::new();
        stale.push((
            "platform",
            repair_metadata("windows-x86_64", &hash(REPAIR_CORRECT)),
        ));
        stale.push((
            "file_digest",
            repair_metadata(&platform, &hash(b"neither-baseline-nor-fixed")),
        ));
        let mut m = good.clone();
        m.source_version = "2".into();
        stale.push(("source_version", m));
        let mut m = good.clone();
        m.source_commit = "c".repeat(40);
        stale.push(("source_commit", m));
        let mut m = good.clone();
        m.source_id = "other-synthetic-source".into();
        stale.push(("source_id", m));
        let mut m = good.clone();
        m.applicability.constraints = "free-text historical constraint".into();
        stale.push(("missing_fence", m));
        let mut m = good.clone();
        m.applicability.constraints = format!(
            "{{\"schema\":\"other\",\"file_digest\":\"{}\"}}",
            hash(REPAIR_CORRECT)
        );
        stale.push(("wrong_fence_schema", m));
        let mut m = good.clone();
        m.applicability.platforms.push("windows-x86_64".into());
        stale.push(("extra_platform", m));
        let mut results = Vec::new();
        for (label, md) in stale {
            let id = label.replace('_', "-");
            let direct = repair_evidence(store, &format!("repair-stale-{id}"), &md, vec![]);
            // Transitive: a fresh-looking parent derived from the stale record.
            let parent = repair_evidence(
                store,
                &format!("repair-parent-{id}"),
                &good,
                vec![derived(&direct)],
            );
            for (shape, dep) in [("direct", direct), ("transitive", parent)] {
                let cand =
                    repair_candidate(store, &format!("repair-{shape}-{id}"), &good, vec![dep]);
                let before = store.catalog().unwrap().len();
                let result = v.verify_and_promote(
                    &cand,
                    &baseline,
                    &fixed,
                    &recipe,
                    &Cancellation::default(),
                );
                let denied = matches!(result, Err(VerificationError::GateDenied))
                    && store.catalog().unwrap().len() == before
                    && store
                        .read_latest(&cand.logical_id)
                        .unwrap()
                        .mapping
                        .reference
                        == cand;
                println!(
                    "STAGE4_REPAIR_B1 verify mismatch={label} shape={shape} result={:?} denied_before_any_run={denied}",
                    result.as_ref().map(|r| r.actual_checked_state)
                );
                results.push((format!("verify-{shape}-{label}"), denied));
            }
        }
        // Approval-time: a writer wins the promotion race with an otherwise exact
        // VerifiedPattern that adds an edge to a stale record (version 2).
        let mut v2 = good.clone();
        v2.source_version = "2".into();
        let stale_edge = repair_evidence(store, "repair-stale-edge", &v2, vec![]);
        for (label, extra) in [
            ("control", vec![]),
            ("stale_edge", vec![derived(&stale_edge)]),
        ] {
            let cand = repair_candidate(
                store,
                &format!("repair-frontrun-pattern-{}", label.replace('_', "-")),
                &good,
                vec![bug_report.clone()],
            );
            let (run_ref, run_hash, checks) =
                unpromoted_run(&fx, &cand, &good, &baseline, &fixed, &recipe);
            let candidate = match store.read(&cand).unwrap().record {
                KnowledgeRecord::Candidate(c) => c,
                _ => unreachable!(),
            };
            let mut header = candidate.header.clone();
            header.revision = 2;
            header.previous = Some(cand.clone());
            header.edges.extend(extra);
            let pattern = store
                .append_revision(
                    KnowledgeRecord::VerifiedPattern(VerifiedPattern {
                        header,
                        candidate: cand.clone(),
                        checks,
                        statement: candidate.statement,
                    }),
                    &cand,
                )
                .unwrap()
                .reference;
            let before = store.catalog().unwrap().len();
            let approved = v.approve_from_ui(
                UiApprovalEvent {
                    pattern,
                    procedure_run: run_ref,
                    displayed_run_sha256: run_hash,
                    displayed_policy_sha256: v.policy_sha256().unwrap(),
                },
                &fixed,
            );
            println!(
                "STAGE4_REPAIR_B1 approve front_run={label} approved={}",
                approved.is_ok()
            );
            if label == "control" {
                assert!(
                    approved.is_ok(),
                    "control approval must succeed: {approved:?}"
                );
            } else {
                // Denied approval leaves no orphan UI receipt.
                results.push((
                    "approve-stale-edge".into(),
                    matches!(approved, Err(VerificationError::GateDenied))
                        && store.catalog().unwrap().len() == before,
                ));
            }
        }
        // Permit/replay-time: a writer front-runs approve_from_ui's append with an
        // exact ApprovedProcedure (genuine UI receipt, recomputed outcome) + stale edge.
        for (label, extra) in [
            ("control", vec![]),
            ("stale_edge", vec![derived(&stale_edge)]),
        ] {
            let cand = repair_candidate(
                store,
                &format!("repair-frontrun-approved-{}", label.replace('_', "-")),
                &good,
                vec![bug_report.clone()],
            );
            let report = v
                .verify_and_promote(&cand, &baseline, &fixed, &recipe, &Cancellation::default())
                .unwrap();
            assert_eq!(report.actual_checked_state, ActualCheckedState::Verified);
            let forged = front_run_approved(&fx, &report, extra);
            let permit = v.autonomous_reuse_permit(&forged, &fixed);
            println!(
                "STAGE4_REPAIR_B1 permit front_run={label} permit={}",
                permit.is_ok()
            );
            if label == "control" {
                let permit = permit.expect("control permit must succeed");
                assert_eq!(
                    v.replay_with_permit(permit, &fixed, &Cancellation::default())
                        .unwrap()
                        .len(),
                    3
                );
            } else {
                results.push((
                    "permit-stale-edge".into(),
                    matches!(permit, Err(VerificationError::GateDenied)),
                ));
            }
        }
        println!("STAGE4_REPAIR_B1_SUMMARY {results:?}");
        assert!(results.iter().all(|(_, denied)| *denied), "{results:?}");
    }
    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "needs the Linux isolation backend (non-Linux fails closed before any spawn)"
    )]
    fn repair_b3_test_only_or_oracle_altering_patch_rejected_before_promotion() {
        if !crate::isolation::test_support::isolation_or_ci_skip(
            "repair_b3_test_only_or_oracle_altering_patch_rejected_before_promotion",
        ) {
            return;
        }
        let fx = repair_fx(&[
            ("bug", REPAIR_BUGGY, REPAIR_CHECK),
            ("hollow", REPAIR_BUGGY, REPAIR_HOLLOW),
            ("both", REPAIR_CORRECT, REPAIR_HOLLOW),
        ]);
        let baseline = repair_snapshot(&fx, "bug");
        let platform = baseline.binding().platform.clone();
        let mut results = Vec::new();
        for (project, primary) in [("hollow", REPAIR_BUGGY), ("both", REPAIR_CORRECT)] {
            let fixed = repair_snapshot(&fx, project);
            let md = repair_metadata(&platform, &hash(primary));
            let obs = repair_evidence(&fx.store, &format!("repair-b3-obs-{project}"), &md, vec![]);
            let cand = repair_candidate(&fx.store, &format!("repair-b3-{project}"), &md, vec![obs]);
            let before = fx.store.catalog().unwrap().len();
            let result = fx.verifier.verify_and_promote(
                &cand,
                &baseline,
                &fixed,
                &repair_recipe(),
                &Cancellation::default(),
            );
            let denied = matches!(result, Err(VerificationError::InvalidRecipe))
                && fx.store.catalog().unwrap().len() == before
                && fx
                    .store
                    .read_latest(&cand.logical_id)
                    .unwrap()
                    .mapping
                    .reference
                    == cand;
            println!(
                "STAGE4_REPAIR_B3 fixed={project} result={:?} denied_before_any_run={denied}",
                result
                    .as_ref()
                    .map(|r| (r.actual_checked_state, r.pattern.is_some()))
            );
            results.push((project, denied));
        }
        // Declarations are validated against BOTH snapshots and bound into hashes.
        let fixed = repair_snapshot(&fx, "both");
        let good = repair_recipe();
        let mut swapped = good.clone();
        std::mem::swap(&mut swapped.oracle_files, &mut swapped.implementation_files);
        let mut undeclared = good.clone();
        undeclared.oracle_files.clear();
        let mut overlap = good.clone();
        overlap.oracle_files.insert("calc.py".into());
        let mut extra = good.clone();
        extra.implementation_files.insert("other.py".into());
        let mut primary_oracle = good.clone();
        primary_oracle.oracle_files.insert("calc.py".into());
        primary_oracle.implementation_files.clear();
        for (label, recipe) in [
            ("roles_swapped_script_is_implementation", swapped.clone()),
            ("oracle_undeclared", undeclared),
            ("overlapping_roles", overlap),
            ("undeclared_extra_file", extra),
            ("primary_declared_oracle", primary_oracle),
        ] {
            let invalid = matches!(
                recipe.validate(baseline.binding()),
                Err(VerificationError::InvalidRecipe)
            ) && matches!(
                recipe.validate(fixed.binding()),
                Err(VerificationError::InvalidRecipe)
            );
            println!("STAGE4_REPAIR_B3 declaration={label} invalid={invalid}");
            results.push((label, invalid));
        }
        assert!(good.validate(baseline.binding()).is_ok());
        assert_ne!(digest(&good).unwrap(), digest(&swapped).unwrap());
        // Pure oracle-preservation rule: legit implementation-only patch passes.
        let mut legit = baseline.binding().clone();
        legit.files.insert("calc.py".into(), hash(REPAIR_CORRECT));
        assert!(oracle_preserving_patch(&good, baseline.binding(), &legit).is_ok());
        assert!(oracle_preserving_patch(&good, baseline.binding(), baseline.binding()).is_err());
        assert!(oracle_preserving_patch(&good, baseline.binding(), fixed.binding()).is_err());
        assert!(results.iter().all(|(_, denied)| *denied), "{results:?}");
    }
}
