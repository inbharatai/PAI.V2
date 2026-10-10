use crate::dto::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_WIRE_BYTES: usize = 262_144;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractError {
    Invalid(&'static str),
    VerificationFailed,
    WrongScope,
    Expired,
    InvalidTransition,
    StaleAttempt,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Attestation {
    pub key_id: String,
    pub algorithm: String,
    pub signature: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignedDomain {
    CatalogCandidateV1,
    QualificationRecordV1,
}
/// Implement ONLY in a trusted native adapter. Must verify exact bytes, trusted
/// key set, domain, revocation and production origin/catalog lineage. Never use
/// a JSON flag or an always-Ok verifier. This crate deliberately holds no keys.
pub trait SignatureVerifier {
    fn verify(
        &self,
        domain: SignedDomain,
        payload: &[u8],
        attestation: &Attestation,
    ) -> Result<(), ContractError>;
}
#[derive(Clone, Debug)]
pub struct VerifiedCandidate {
    candidate: CatalogCandidate,
}
impl VerifiedCandidate {
    pub fn candidate(&self) -> &CatalogCandidate {
        &self.candidate
    }
}
#[derive(Clone, Debug)]
pub struct ValidatedQualification {
    record: QualificationRecord,
}
impl ValidatedQualification {
    pub fn record(&self) -> &QualificationRecord {
        &self.record
    }
}

pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ContractError> {
    if bytes.len() > MAX_WIRE_BYTES {
        return Err(ContractError::Invalid("wire size"));
    }
    serde_json::from_slice(bytes).map_err(|_| ContractError::Invalid("JSON or unknown field"))
}
fn attest<V: SignatureVerifier>(
    bytes: &[u8],
    a: &Attestation,
    domain: SignedDomain,
    v: &V,
) -> Result<(), ContractError> {
    if bytes.len() > MAX_WIRE_BYTES
        || !identifier(&a.key_id)
        || a.algorithm != "Ed25519"
        || a.signature.is_empty()
    {
        return Err(ContractError::VerificationFailed);
    }
    v.verify(domain, bytes, a)
}
pub fn verify_candidate<V: SignatureVerifier>(
    bytes: &[u8],
    a: &Attestation,
    v: &V,
) -> Result<VerifiedCandidate, ContractError> {
    attest(bytes, a, SignedDomain::CatalogCandidateV1, v)?;
    let candidate: CatalogCandidate = decode(bytes)?;
    validate_candidate(&candidate)?;
    Ok(VerifiedCandidate { candidate })
}
pub fn validate_qualification<V: SignatureVerifier>(
    bytes: &[u8],
    a: &Attestation,
    v: &V,
    c: &VerifiedCandidate,
) -> Result<ValidatedQualification, ContractError> {
    attest(bytes, a, SignedDomain::QualificationRecordV1, v)?;
    let r: QualificationRecord = decode(bytes)?;
    if r.schema_version != SCHEMA_VERSION
        || !identifier(&r.id)
        || r.scope.candidate != c.candidate
        || !c.candidate.qualification_record_ids.contains(&r.id)
        || !identifier(&r.scope.device_class)
        || r.scope.parallel_agents == 0
        || r.scope.parallel_agents > c.candidate.max_parallel_agents
    {
        return Err(ContractError::WrongScope);
    }
    if r.issued_at_ms >= r.expires_at_ms
        || r.evidence_ids.is_empty()
        || !unique_strings(&r.evidence_ids)
        || r.tested_capabilities.is_empty()
        || r.tested_capabilities
            .iter()
            .any(|x| !c.candidate.capabilities.contains(x))
        || r.tested_languages
            .iter()
            .any(|x| !c.candidate.languages.contains(x))
    {
        return Err(ContractError::Invalid("qualification evidence"));
    }
    let required = [
        QualificationCheckKind::Integrity,
        QualificationCheckKind::Load,
        QualificationCheckKind::MemoryPeak,
        QualificationCheckKind::ToolFormat,
        QualificationCheckKind::CapabilityTasks,
        QualificationCheckKind::ThermalSoak,
    ];
    if r.checks.len() != required.len()
        || required.iter().any(|kind| {
            r.checks
                .iter()
                .filter(|check| {
                    &check.kind == kind
                        && check.outcome == CheckOutcome::Pass
                        && identifier(&check.evidence_id)
                        && r.evidence_ids.contains(&check.evidence_id)
                })
                .count()
                != 1
        })
    {
        return Err(ContractError::Invalid(
            "qualification checks not all passed",
        ));
    }
    if r.responsiveness
        .as_ref()
        .is_some_and(|p| !valid_responsiveness(p))
    {
        return Err(ContractError::Invalid("responsiveness"));
    }
    Ok(ValidatedQualification { record: r })
}
pub(crate) fn identifier(s: &str) -> bool {
    s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && s.len() <= 160
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
pub(crate) fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn text(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
fn unique_strings(s: &[String]) -> bool {
    s.len() <= 128
        && s.iter().all(|x| text(x))
        && s.iter().collect::<BTreeSet<_>>().len() == s.len()
}
pub fn validate_candidate(c: &CatalogCandidate) -> Result<(), ContractError> {
    let r = &c.runtime;
    if c.schema_version != SCHEMA_VERSION
        || !identifier(&c.id)
        || !identifier(&c.version)
        || c.expires_at_ms == 0
        || c.artifacts.is_empty()
        || c.artifacts.len() > 64
        || c.max_parallel_agents == 0
        || c.max_parallel_agents > 64
    {
        return Err(ContractError::Invalid("candidate identity"));
    }
    if [
        &r.runtime,
        &r.version,
        &r.backend,
        &r.os,
        &r.os_version,
        &r.abi,
        &r.driver_version,
    ]
    .iter()
    .any(|s| !text(s))
        || !unique_strings(&r.cpu_features)
    {
        return Err(ContractError::Invalid("runtime"));
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for a in &c.artifacts {
        if !identifier(&a.id)
            || !ids.insert(&a.id)
            || !paths.insert(&a.catalog_path)
            || !digest(&a.sha256)
            || a.download_bytes == 0
            || a.installed_bytes == 0
            || !text(&a.format)
            || !text(&a.source_revision)
            || a.catalog_path.len() > 512
            || a.catalog_path
                .split('/')
                .any(|p| !identifier(p) || p == "." || p == "..")
        {
            return Err(ContractError::Invalid("artifact"));
        }
    }
    if !c.artifacts.iter().any(|a| a.kind == ArtifactKind::Weights)
        || c.licences.is_empty()
        || c.licences.len() > 32
        || c.licences
            .iter()
            .any(|l| !identifier(&l.id) || !text(&l.revision) || !digest(&l.notice_sha256))
    {
        return Err(ContractError::Invalid("weights or licence"));
    }
    let m = &c.memory;
    if m.context_tokens == 0
        || !text(&m.kv_format)
        || m.weights_ram_bytes == 0
        || m.runtime_ram_bytes == 0
        || m.os_reserve_bytes == 0
        || m.available_reserve_bytes == 0
        || m.disk_headroom_bytes == 0
        || c.capabilities.is_empty()
        || c.capabilities.iter().collect::<BTreeSet<_>>().len() != c.capabilities.len()
        || !unique_strings(&c.languages)
        || !unique_strings(&c.modalities)
        || c.languages.is_empty()
        || c.modalities.is_empty()
        || !unique_strings(&c.qualification_record_ids)
    {
        return Err(ContractError::Invalid("memory or capabilities"));
    }
    if c.artifacts
        .iter()
        .any(|a| a.kind == ArtifactKind::Projector)
        && (m.projector_ram_bytes == 0 || m.vision_ram_bytes == 0)
    {
        return Err(ContractError::Invalid("projector budget"));
    }
    if c.capabilities.contains(&Capability::Vision)
        && !c
            .artifacts
            .iter()
            .any(|a| a.kind == ArtifactKind::Projector)
    {
        return Err(ContractError::Invalid("vision projector"));
    }
    if c.capabilities.contains(&Capability::Voice) && m.speech_ram_bytes == 0 {
        return Err(ContractError::Invalid("speech budget"));
    }
    Ok(())
}
pub(crate) fn valid_responsiveness(r: &Responsiveness) -> bool {
    r.cold_load_ms > 0
        && r.first_token_p95_ms > 0
        && r.generated_tokens > 0
        && r.generation_ms > 0
        && identifier(&r.evidence_id)
        && matches!(r.thermal, ThermalState::Nominal | ThermalState::Warm)
}

/// Trusted native results are intentionally NOT Deserialize. The host must
/// produce this only from actual scoped native smoke checks, never bridge JSON.
#[derive(Clone, Debug)]
pub struct NativePreflightReport {
    pub candidate: CatalogCandidate,
    pub request: AdmissionRequest,
    pub probe_id: String,
    pub captured_at_ms: u64,
    pub expires_at_ms: u64,
    pub evidence_id: String,
    pub smoke: NativeSmokeResult,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeSmokeResult {
    Passed { responsiveness: Responsiveness },
    Oom { allocation_bytes: Option<u64> },
    DriverFailure,
    ToolFormatFailure,
    ThermalFailure,
    Cancelled,
}
#[derive(Clone, Debug)]
pub struct NativePreflight {
    report: NativePreflightReport,
}
impl NativePreflight {
    pub fn from_native_report(report: NativePreflightReport) -> Result<Self, ContractError> {
        if report.captured_at_ms >= report.expires_at_ms
            || !identifier(&report.probe_id)
            || !identifier(&report.evidence_id)
        {
            return Err(ContractError::Invalid("native report"));
        }
        match &report.smoke {
            NativeSmokeResult::Passed { responsiveness }
                if valid_responsiveness(responsiveness) =>
            {
                Ok(Self { report })
            }
            _ => Err(ContractError::Invalid("native smoke failed")),
        }
    }
    pub(crate) fn matches(
        &self,
        c: &CatalogCandidate,
        p: &DeviceProbe,
        r: &AdmissionRequest,
        now: u64,
    ) -> bool {
        self.report.candidate == *c
            && self.report.request == *r
            && self.report.probe_id == p.probe_id
            && self.report.captured_at_ms <= now
            && now < self.report.expires_at_ms
    }
    pub(crate) fn responsiveness(&self) -> &Responsiveness {
        match &self.report.smoke {
            NativeSmokeResult::Passed { responsiveness } => responsiveness,
            _ => unreachable!("constructor rejects failed reports"),
        }
    }
    pub(crate) fn evidence_id(&self) -> &str {
        &self.report.evidence_id
    }
}
