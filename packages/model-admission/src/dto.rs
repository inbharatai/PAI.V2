use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// UNKNOWN carries no value; zero is never a substitute for a failed probe.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "provenance",
    content = "value",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
pub enum Observation<T> {
    Detected(T),
    Estimated(T),
    Tested(T),
    Unknown,
}
impl<T> Observation<T> {
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Detected(v) | Self::Estimated(v) | Self::Tested(v) => Some(v),
            Self::Unknown => None,
        }
    }
    pub fn measured(&self) -> Option<&T> {
        match self {
            Self::Detected(v) | Self::Tested(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ThermalState {
    Nominal,
    Warm,
    Throttled,
    Critical,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BackendHealth {
    DetectedOnly,
    LoadValidated,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackendProbe {
    pub runtime: String,
    pub runtime_version: String,
    pub backend: String,
    pub driver_version: String,
    pub health: Observation<BackendHealth>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeviceProbe {
    pub schema_version: u32,
    pub probe_id: String,
    pub captured_at_ms: u64,
    /// Local classification only. Never upload a fingerprint.
    pub device_class: Observation<String>,
    pub os: Observation<String>,
    pub os_version: Observation<String>,
    pub os_api_level: Observation<u32>,
    pub abi: Observation<String>,
    pub cpu_features: Observation<Vec<String>>,
    pub total_ram_bytes: Observation<u64>,
    pub gpu_name: Observation<String>,
    pub total_vram_bytes: Observation<u64>,
    pub available_ram_bytes: Observation<u64>,
    pub available_vram_bytes: Observation<u64>,
    pub unified_memory: Observation<bool>,
    pub low_memory_threshold_bytes: Observation<u64>,
    /// Native/process budget (on desktop, adapter may report measured OS/process limit).
    pub native_budget_bytes: Observation<u64>,
    pub heap_budget_bytes: Observation<u64>,
    pub usable_storage_bytes: Observation<u64>,
    pub disk_bytes_per_second: Observation<u64>,
    pub battery_percent: Observation<u8>,
    pub thermal: Observation<ThermalState>,
    pub backends: Vec<BackendProbe>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Capability {
    Chat,
    Voice,
    Vision,
    DeviceAction,
    Coding,
    Tools,
    Search,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ModelRole {
    General,
    Specialist,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArtifactKind {
    Weights,
    Projector,
    Speech,
    Tokenizer,
    Other,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub id: String,
    pub kind: ArtifactKind,
    pub format: String,
    pub sha256: String,
    pub download_bytes: u64,
    pub installed_bytes: u64,
    pub source_revision: String,
    /// Relative signed-catalog path. Origin enforcement belongs to downloader.
    pub catalog_path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRequirement {
    pub runtime: String,
    pub version: String,
    pub backend: String,
    pub os: String,
    pub os_version: String,
    pub required_os_api_level: Option<u32>,
    pub abi: String,
    pub cpu_features: Vec<String>,
    pub driver_version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Licence {
    pub id: String,
    pub revision: String,
    pub notice_sha256: String,
    pub distribution: DistributionPermission,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DistributionPermission {
    Approved,
    NotApproved,
    Unknown,
}
/// Memory components are explicit peak upper bounds for THIS configuration.
/// No extrapolation from parameter count, file size, or KV datatype is allowed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryPlan {
    pub context_tokens: u32,
    pub kv_format: String,
    pub provenance: Observation<String>,
    pub weights_ram_bytes: u64,
    pub projector_ram_bytes: u64,
    pub vision_ram_bytes: u64,
    pub kv_ram_bytes: u64,
    pub speech_ram_bytes: u64,
    pub runtime_ram_bytes: u64,
    pub per_agent_ram_bytes: u64,
    pub peak_vram_bytes: u64,
    pub heap_bytes: u64,
    pub os_reserve_bytes: u64,
    pub available_reserve_bytes: u64,
    pub comfortable_headroom_bytes: u64,
    pub disk_headroom_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogCandidate {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub role: ModelRole,
    pub artifacts: Vec<Artifact>,
    pub runtime: RuntimeRequirement,
    pub licences: Vec<Licence>,
    pub capabilities: Vec<Capability>,
    pub languages: Vec<String>,
    pub modalities: Vec<String>,
    pub memory: MemoryPlan,
    pub max_parallel_agents: u32,
    pub qualification_record_ids: Vec<String>,
    pub expires_at_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdmissionRequest {
    pub schema_version: u32,
    pub capabilities: Vec<Capability>,
    pub languages: Vec<String>,
    pub context_tokens: u32,
    pub kv_format: String,
    pub parallel_agents: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QualificationKind {
    PhysicalDevice,
    ValidatedGenericProfile,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Responsiveness {
    pub cold_load_ms: u64,
    pub first_token_p95_ms: u64,
    /// Measured count/time, not guessed device-specific TPS.
    pub generated_tokens: u64,
    pub generation_ms: u64,
    pub thermal: ThermalState,
    pub evidence_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QualificationScope {
    /// Entire signed candidate pins hashes, projector, runtime and memory profile.
    pub candidate: CatalogCandidate,
    pub device_class: String,
    pub parallel_agents: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QualificationCheckKind {
    Integrity,
    Load,
    MemoryPeak,
    ToolFormat,
    CapabilityTasks,
    ThermalSoak,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckOutcome {
    Pass,
    Fail,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QualificationCheck {
    pub kind: QualificationCheckKind,
    pub outcome: CheckOutcome,
    pub evidence_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QualificationRecord {
    pub schema_version: u32,
    pub id: String,
    pub kind: QualificationKind,
    pub scope: QualificationScope,
    pub tested_capabilities: Vec<Capability>,
    pub tested_languages: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub checks: Vec<QualificationCheck>,
    pub responsiveness: Option<Responsiveness>,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DecisionStatus {
    Recommended,
    SupportedWithLimits,
    Unsupported,
    NotYetQualified,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Reason {
    Admitted,
    InvalidInput,
    UnknownProbe,
    StaleProbe,
    BackendNotValidated,
    RuntimeMismatch,
    CapabilityMismatch,
    ContextMismatch,
    PermanentMemoryMisfit,
    MemoryPressure,
    InsufficientStorage,
    ThermalPressure,
    ArithmeticOverflow,
    Unqualified,
    PreflightRequired,
    EvidenceExpired,
    LicenceNotApproved,
    LimitedHeadroom,
    ResponsivenessUnmeasured,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub schema_version: u32,
    pub candidate_id: String,
    pub candidate_version: String,
    pub probe_id: String,
    pub status: DecisionStatus,
    pub reasons: Vec<Reason>,
    /// Status is compatibility, not permission. Pressure/unknown can require recheck.
    pub eligible_now: bool,
    pub context_tokens: u32,
    pub backend: String,
    pub peak_ram_bytes: Option<u64>,
    pub peak_vram_bytes: Option<u64>,
    pub storage_reservation_bytes: Option<u64>,
    pub evidence_ids: Vec<String>,
    pub responsiveness: Option<Responsiveness>,
}
