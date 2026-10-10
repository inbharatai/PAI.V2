use crate::{admission::*, dto::*, trust::*};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NetworkKind {
    Wifi,
    Ethernet,
    Cellular,
    Offline,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NetworkState {
    pub kind: NetworkKind,
    pub metered: Observation<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DownloadPurpose {
    InitialGeneral,
    NeededSpecialist,
    Update,
    Recovery,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "rule",
    content = "versions",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
pub enum VersionRule {
    Exact(Vec<String>),
    AnySignedCompatible,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelRule {
    pub model_id: String,
    pub versions: VersionRule,
    pub runtime: String,
    pub allowed_capabilities: Vec<Capability>,
    pub max_download_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StandingDownloadPolicy {
    pub schema_version: u32,
    pub id: String,
    pub revision: u64,
    pub not_before_ms: u64,
    pub expires_at_ms: u64,
    pub allowed_networks: Vec<NetworkKind>,
    pub allow_metered: bool,
    pub max_model_store_bytes: u64,
    pub max_download_bytes: u64,
    pub accepted_licences: Vec<Licence>,
    pub model_rules: Vec<ModelRule>,
    pub allow_initial_general: bool,
    pub allow_needed_specialist: bool,
    pub allow_updates: bool,
    pub allow_recovery: bool,
}
/// Implement in the local consent/grant store, not IPC or imported sync JSON.
/// A deserialized policy alone has no authority. Revocations must be consulted
/// again on every boundary; do not retain a stale approved wrapper across IO.
pub trait StandingPolicyAuthority {
    fn validate_local_approval(&self, policy: &StandingDownloadPolicy)
        -> Result<(), ContractError>;
}
#[derive(Clone, Debug)]
pub struct NativePolicyGrant {
    policy: StandingDownloadPolicy,
}
impl NativePolicyGrant {
    pub fn from_local_store<A: StandingPolicyAuthority>(
        policy: StandingDownloadPolicy,
        authority: &A,
    ) -> Result<Self, ContractError> {
        if policy.schema_version != SCHEMA_VERSION || !identifier(&policy.id) || policy.revision == 0 || policy.not_before_ms >= policy.expires_at_ms || policy.max_download_bytes == 0 || policy.max_model_store_bytes == 0 || policy.model_rules.is_empty() || policy.model_rules.len() > 128 || policy.model_rules.iter().any(|r| !identifier(&r.model_id) || r.runtime.is_empty() || r.allowed_capabilities.is_empty() || r.max_download_bytes == 0 || matches!(&r.versions, VersionRule::Exact(v) if v.is_empty() || v.iter().any(|s|!identifier(s)))) { return Err(ContractError::Invalid("policy")); }
        authority.validate_local_approval(&policy)?;
        Ok(Self { policy })
    }
    pub fn policy(&self) -> &StandingDownloadPolicy {
        &self.policy
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PolicyPause {
    MissingGrant,
    Expired,
    Network,
    Metered,
    StorageCap,
    Licence,
    ModelRule,
    Purpose,
    Admission,
    Overflow,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "status",
    content = "reason",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
pub enum PolicyDecision {
    AllowedWithinGrant,
    Pause(PolicyPause),
}

pub fn check_download_policy(
    c: &VerifiedCandidate,
    grant: Option<&NativePolicyGrant>,
    purpose: &DownloadPurpose,
    network: &NetworkState,
    current_model_store_bytes: u64,
    now: u64,
) -> PolicyDecision {
    let Some(grant) = grant else {
        return PolicyDecision::Pause(PolicyPause::MissingGrant);
    };
    let p = grant.policy();
    let c = c.candidate();
    let pause = PolicyDecision::Pause;
    if now < p.not_before_ms || now >= p.expires_at_ms {
        return pause(PolicyPause::Expired);
    }
    if matches!(network.kind, NetworkKind::Offline | NetworkKind::Unknown)
        || !p.allowed_networks.contains(&network.kind)
    {
        return pause(PolicyPause::Network);
    }
    match network.metered.measured() {
        Some(false) => {}
        Some(true) if p.allow_metered => {}
        _ => return pause(PolicyPause::Metered),
    }
    let allowed_purpose = match purpose {
        DownloadPurpose::InitialGeneral => p.allow_initial_general && c.role == ModelRole::General,
        DownloadPurpose::NeededSpecialist => {
            p.allow_needed_specialist && c.role == ModelRole::Specialist
        }
        DownloadPurpose::Update => p.allow_updates,
        DownloadPurpose::Recovery => p.allow_recovery,
    };
    if !allowed_purpose {
        return pause(PolicyPause::Purpose);
    }
    if c.licences.iter().any(|l| {
        l.distribution != DistributionPermission::Approved || !p.accepted_licences.contains(l)
    }) {
        return pause(PolicyPause::Licence);
    }
    let (Some(bytes), Some(reserve)) = (download_bytes(c), storage_reservation(c)) else {
        return pause(PolicyPause::Overflow);
    };
    let Some(total) = current_model_store_bytes.checked_add(reserve) else {
        return pause(PolicyPause::Overflow);
    };
    if bytes > p.max_download_bytes || total > p.max_model_store_bytes {
        return pause(PolicyPause::StorageCap);
    }
    if !p.model_rules.iter().any(|r| {
        r.model_id == c.id
            && r.runtime == c.runtime.runtime
            && bytes <= r.max_download_bytes
            && c.capabilities
                .iter()
                .all(|cap| r.allowed_capabilities.contains(cap))
            && match &r.versions {
                VersionRule::Exact(v) => v.contains(&c.version),
                VersionRule::AnySignedCompatible => true,
            }
    }) {
        return pause(PolicyPause::ModelRule);
    }
    PolicyDecision::AllowedWithinGrant
}

pub struct AdmissionContext<'a> {
    pub probe: &'a DeviceProbe,
    pub request: &'a AdmissionRequest,
    pub qualifications: &'a [ValidatedQualification],
    pub preflight: Option<&'a NativePreflight>,
    pub now_ms: u64,
}
impl AdmissionContext<'_> {
    pub fn evaluate(&self, c: &VerifiedCandidate) -> Decision {
        evaluate(
            c,
            self.probe,
            self.request,
            self.qualifications,
            self.preflight,
            self.now_ms,
        )
    }
}
pub struct DownloadContext<'a> {
    pub admission: AdmissionContext<'a>,
    pub grant: Option<&'a NativePolicyGrant>,
    pub purpose: DownloadPurpose,
    pub network: &'a NetworkState,
    pub current_model_store_bytes: u64,
}
impl DownloadContext<'_> {
    pub fn check(&self, c: &VerifiedCandidate) -> Result<Decision, PolicyPause> {
        let d = self.admission.evaluate(c);
        if !d.eligible_now {
            return Err(PolicyPause::Admission);
        }
        match check_download_policy(
            c,
            self.grant,
            &self.purpose,
            self.network,
            self.current_model_store_bytes,
            self.admission.now_ms,
        ) {
            PolicyDecision::AllowedWithinGrant => Ok(d),
            PolicyDecision::Pause(p) => Err(p),
        }
    }
}
/// OOM retry must change the exact configuration and reduce the validated peak
/// budget, not just have a smaller marketing label. Lower context requires a
/// separately signed/qualified candidate profile and matching AdmissionRequest.
pub fn oom_recovery_candidate<'a>(
    candidates: &'a [VerifiedCandidate],
    failed: &CatalogCandidate,
    context: &DownloadContext<'_>,
) -> Option<&'a VerifiedCandidate> {
    if context.purpose != DownloadPurpose::Recovery {
        return None;
    }
    let unified = *context.admission.probe.unified_memory.measured()?;
    let prior_ram = peak_ram(failed, context.admission.request.parallel_agents, unified)?;
    let mut eligible: Vec<_> = candidates
        .iter()
        .filter(|c| c.candidate() != failed)
        .filter_map(|c| {
            let d = context.check(c).ok()?;
            let ram = d.peak_ram_bytes?;
            ((ram < prior_ram
                && c.candidate().memory.peak_vram_bytes <= failed.memory.peak_vram_bytes)
                || (ram <= prior_ram
                    && c.candidate().memory.peak_vram_bytes < failed.memory.peak_vram_bytes))
                .then_some((c, d))
        })
        .collect();
    eligible.sort_by_key(|(c, d)| {
        (
            d.responsiveness
                .as_ref()
                .map(|p| p.first_token_p95_ms)
                .unwrap_or(u64::MAX),
            c.candidate().id.clone(),
        )
    });
    eligible.first().map(|(c, _)| *c)
}
/// Same checks for non-OOM recovery; no special bypass, cloud path or guessed tier.
/// Caller must exclude known failing configurations for driver/thermal failures.
pub fn recovery_candidate<'a>(
    candidates: &'a [VerifiedCandidate],
    context: &DownloadContext<'_>,
) -> Option<&'a VerifiedCandidate> {
    if context.purpose != DownloadPurpose::Recovery {
        return None;
    }
    let mut eligible: Vec<_> = candidates
        .iter()
        .filter_map(|c| context.check(c).ok().map(|d| (c, d)))
        .collect();
    eligible.sort_by_key(|(c, d)| {
        (
            d.responsiveness
                .as_ref()
                .map(|p| p.first_token_p95_ms)
                .unwrap_or(u64::MAX),
            c.candidate().id.clone(),
        )
    });
    eligible.first().map(|(c, _)| *c)
}
