//! PAI capability contracts — the shared record vocabulary for the universal
//! capability layer (see `docs/UNIVERSAL_CAPABILITY_PLAN_2026-10-01.md`).
//!
//! `capability.v1.json` (embedded below) is the single source of truth for five
//! versioned record kinds:
//!
//! - [`PerceptionObservation`] — detector labels vs OCR vs model description,
//!   with truthful distance semantics (`apparent_size_cue` is NEVER metres).
//! - [`EnvironmentObservation`] — bounded MK-style environment learning with a
//!   mandatory epistemic status (`observation | hypothesis | correction |
//!   verified_fact`) and provenance. A hypothesis is never trusted for control.
//! - [`ProcedureOutcome`] — bounded procedure records with preconditions,
//!   postconditions, verification evidence, risk class, and an explicit
//!   promotion gate (`suggested` never auto-executes; `approved` requires all
//!   six requirements including explicit approval).
//! - [`AudioBackendStatus`] — honest backend identity per capability
//!   (`reference`/scaffold engines and system fallbacks are labeled as such).
//! - [`DeviceCapability`] — adapter manifests: control requires a verified
//!   adapter; unverified devices are read-only identity inspection.
//!
//! Every type round-trips the checked-in example fixtures (see tests), so the
//! JSON contract and the Rust types cannot drift silently.

use serde::{Deserialize, Serialize};

/// The checked-in contract document, embedded at compile time.
pub const CAPABILITY_JSON: &str = include_str!("../capability.v1.json");

pub const SCHEMA_ID: &str = "inbharat.pai.capability.v1";

// ---------------------------------------------------------------------------
// Perception observation
// ---------------------------------------------------------------------------

/// The five record kinds' schema constants.
pub mod schemas {
    pub const PERCEPTION: &str = "inbharat.pai.perception.v1";
    pub const ENV_OBSERVATION: &str = "inbharat.pai.envobs.v1";
    pub const PROCEDURE: &str = "inbharat.pai.procedure.v1";
    pub const AUDIO: &str = "inbharat.pai.audio.v1";
    pub const DEVICE: &str = "inbharat.pai.device.v1";
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Provenance {
    pub platform: String,
    pub device_id: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NormalizedBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PerceptionLabel {
    pub label: String,
    pub confidence: f32,
    /// `box` is a Rust keyword; the JSON field stays `box`.
    #[serde(rename = "box")]
    pub box_normalized: NormalizedBox,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    #[serde(default)]
    pub track_uncertain: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DistanceKind {
    /// Bounding-box fill ratio — an apparent proximity cue, NEVER metres.
    ApparentSizeCue,
    /// Only when a supported, calibrated sensor supplies it (evidence names the sensor).
    CalibratedMeters,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DistanceEvidence {
    pub kind: DistanceKind,
    pub value: f32,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PerceptionSource {
    Camera,
    Screen,
    Ocr,
    /// A model's free-text image description — a hypothesis with provenance,
    /// never a verified detector class.
    ModelDescription,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PerceptionObservation {
    pub schema: String,
    pub source: PerceptionSource,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_identity: Option<String>,
    #[serde(default)]
    pub labels: Vec<PerceptionLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<DistanceEvidence>,
    pub provenance: Provenance,
}

impl PerceptionObservation {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != schemas::PERCEPTION {
            return Err(format!("perception schema must be {}", schemas::PERCEPTION));
        }
        for l in &self.labels {
            for v in [
                l.box_normalized.x,
                l.box_normalized.y,
                l.box_normalized.width,
                l.box_normalized.height,
            ] {
                if !(0.0..=1.0).contains(&v) {
                    return Err(format!("label '{}' box must be normalized 0..1", l.label));
                }
            }
            if !(0.0..=1.0).contains(&l.confidence) {
                return Err(format!("label '{}' confidence must be 0..1", l.label));
            }
        }
        if let Some(d) = &self.distance {
            if matches!(d.kind, DistanceKind::CalibratedMeters) && d.evidence.trim().is_empty() {
                return Err(
                    "calibrated_meters distance requires evidence naming the sensor".to_string(),
                );
            }
        }
        if self.description.is_some() && self.provenance.source.trim().is_empty() {
            return Err("a model description requires provenance".to_string());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Environment observation (bounded MK-style environment learning)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicStatus {
    Observation,
    /// Never trusted for device control.
    Hypothesis,
    Correction,
    VerifiedFact,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnvScope {
    Device,
    User,
    Project,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnvironmentObservation {
    pub schema: String,
    pub subject: String,
    pub observed_capability: String,
    pub evidence: String,
    pub confidence: Confidence,
    pub scope: EnvScope,
    pub epistemic_status: EpistemicStatus,
    pub provenance: Provenance,
    pub timestamp_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl EnvironmentObservation {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != schemas::ENV_OBSERVATION {
            return Err(format!(
                "environment schema must be {}",
                schemas::ENV_OBSERVATION
            ));
        }
        if matches!(self.epistemic_status, EpistemicStatus::VerifiedFact)
            && self
                .verification_ref
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
        {
            return Err("verified_fact requires a verification_ref".to_string());
        }
        if self.evidence.trim().is_empty() {
            return Err(
                "evidence must be stated — appearance-only inference is not allowed".to_string(),
            );
        }
        Ok(())
    }

    /// A hypothesis can never become device-control authority.
    pub fn may_authorize_device_control(&self) -> bool {
        matches!(self.epistemic_status, EpistemicStatus::VerifiedFact)
    }
}

// ---------------------------------------------------------------------------
// Procedure outcome (MK-style procedure learning)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureResult {
    Success,
    Failure,
    Partial,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Verification {
    pub verified: bool,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromotionStatus {
    None,
    /// Learning suggests; a human/policy approves. Never auto-executes.
    Suggested,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromotionRequirements {
    pub bounded_arguments: bool,
    pub repeatable_success: bool,
    pub verified_postconditions: bool,
    pub low_risk_class: bool,
    pub no_contradictory_evidence: bool,
    pub explicit_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Promotion {
    pub status: PromotionStatus,
    pub policy_version: String,
    pub requirements: PromotionRequirements,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcedureOutcome {
    pub schema: String,
    pub procedure_id: String,
    pub bounded_arguments: String,
    pub preconditions: String,
    pub postconditions: String,
    pub result: ProcedureResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    pub verification: Verification,
    pub risk_class: String,
    pub promotion: Promotion,
    pub timestamp_ms: u64,
    pub provenance: Provenance,
}

impl ProcedureOutcome {
    /// The promotion gate: every requirement must hold, including explicit
    /// approval. This is the safeguard that prevents a model guess or an
    /// unverified lucky run from becoming a trusted procedure.
    pub fn promotable(&self) -> Result<bool, String> {
        if self.risk_class == "BLOCK" {
            return Err("BLOCK-tier procedures are never promotable".to_string());
        }
        let r = &self.promotion.requirements;
        if !(r.bounded_arguments
            && r.repeatable_success
            && r.verified_postconditions
            && r.low_risk_class
            && r.no_contradictory_evidence
            && r.explicit_approval)
        {
            return Ok(false);
        }
        // Promotion evidence must itself be verified success.
        if !matches!(self.result, ProcedureResult::Success) || !self.verification.verified {
            return Ok(false);
        }
        Ok(true)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != schemas::PROCEDURE {
            return Err(format!("procedure schema must be {}", schemas::PROCEDURE));
        }
        if matches!(self.promotion.status, PromotionStatus::Approved) && !self.promotable()? {
            return Err("approved promotion requires all gates + verified success".to_string());
        }
        if matches!(self.promotion.status, PromotionStatus::Suggested)
            && self.promotion.requirements.explicit_approval
        {
            return Err("suggested must not carry explicit_approval yet".to_string());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Audio backend status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AudioCapability {
    Asr,
    Tts,
    Vad,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AudioBackend {
    SherpaOnnx,
    InbharatAudio,
    WhisperCpp,
    Piper,
    /// Online-ish emergency fallback — offline=false, surfaced to the user.
    AndroidSystemTts,
    /// Scaffold/reference engine output — must NEVER be reported as production.
    Reference,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum StreamingClass {
    StatefulStreaming,
    SegmentChunked,
    BufferedFinal,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioBackendStatus {
    pub schema: String,
    pub capability: AudioCapability,
    pub backend: AudioBackend,
    pub ready: bool,
    pub streaming_class: StreamingClass,
    pub offline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AudioBackendStatus {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != schemas::AUDIO {
            return Err(format!("audio schema must be {}", schemas::AUDIO));
        }
        if matches!(self.backend, AudioBackend::Reference) && self.ready {
            return Err("reference backend must never report ready as production".to_string());
        }
        if matches!(self.backend, AudioBackend::AndroidSystemTts) && self.offline {
            return Err("android system fallback is online-ish: offline must be false".to_string());
        }
        if !self.ready
            && self
                .reason
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
        {
            return Err(
                "not-ready backends must state a reason (fail-closed contract)".to_string(),
            );
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Device capability (host-side adapter manifest)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceVerification {
    Verified,
    Observed,
    Unverified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    NotRequested,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceCapability {
    pub schema: String,
    pub device_identity: String,
    pub host: String,
    #[serde(rename = "connection")]
    pub connection_kind: String,
    pub supported_operations: Vec<String>,
    pub permission_state: PermissionState,
    pub verification: DeviceVerification,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_evidence: Option<String>,
    pub discovery_evidence: String,
    pub timestamp_ms: u64,
}

impl DeviceCapability {
    /// Control operations are only available through a VERIFIED adapter.
    /// Unverified/observed devices get truthful read-only identity inspection.
    pub fn allows_control(&self) -> bool {
        matches!(self.verification, DeviceVerification::Verified)
            && self
                .verification_evidence
                .as_deref()
                .map(|e| !e.trim().is_empty())
                .unwrap_or(false)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != schemas::DEVICE {
            return Err(format!("device schema must be {}", schemas::DEVICE));
        }
        if matches!(self.verification, DeviceVerification::Verified)
            && self
                .verification_evidence
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
        {
            return Err("verified devices require verification evidence".to_string());
        }
        if !self.allows_control() && !self.supported_operations.is_empty() {
            // Read-only devices may still expose inspection operations, but any
            // operation implying control must be dropped; callers enforce this.
            Ok(())
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Contract document access
// ---------------------------------------------------------------------------

/// Parse the embedded contract document (all five record examples round-trip
/// through their types — enforced by tests).
pub fn contract_document() -> serde_json::Value {
    serde_json::from_str(CAPABILITY_JSON).expect("capability.v1.json must parse")
}

pub fn example(kind: &str) -> serde_json::Value {
    let ex = contract_document()["records"][kind]["example"].clone();
    if !ex.is_object() {
        panic!("no example for record kind {kind}");
    }
    ex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_document_parses_and_covers_all_five_kinds() {
        let doc = contract_document();
        assert_eq!(doc["schema"], SCHEMA_ID);
        for kind in [
            "perception_observation",
            "environment_observation",
            "procedure_outcome",
            "audio_backend_status",
            "device_capability",
        ] {
            assert!(
                doc["records"][kind]["example"].is_object(),
                "{kind} needs an example"
            );
            assert!(
                !doc["records"][kind]["invariants"]
                    .as_array()
                    .unwrap()
                    .is_empty(),
                "{kind} needs invariants"
            );
        }
    }

    #[test]
    fn perception_example_round_trips_and_validates() {
        let raw = example("perception_observation");
        let o: PerceptionObservation = serde_json::from_value(raw).expect("example round-trips");
        assert!(o.validate().is_ok());
        assert_eq!(o.source, PerceptionSource::Camera);
        assert_eq!(
            o.distance.as_ref().unwrap().kind,
            DistanceKind::ApparentSizeCue
        );
    }

    #[test]
    fn calibrated_meters_requires_sensor_evidence() {
        let mut o: PerceptionObservation =
            serde_json::from_value(example("perception_observation")).unwrap();
        o.distance = Some(DistanceEvidence {
            kind: DistanceKind::CalibratedMeters,
            value: 1.2,
            evidence: "   ".into(),
        });
        assert!(
            o.validate().is_err(),
            "empty sensor evidence must be rejected"
        );
        o.distance.as_mut().unwrap().evidence = "ToF sensor, factory-calibrated".into();
        assert!(o.validate().is_ok());
    }

    #[test]
    fn unnormalized_boxes_are_rejected() {
        let mut o: PerceptionObservation =
            serde_json::from_value(example("perception_observation")).unwrap();
        o.labels[0].box_normalized.x = 1.5;
        assert!(o.validate().is_err());
    }

    #[test]
    fn environment_example_round_trips_and_gates_control() {
        let o: EnvironmentObservation = serde_json::from_value(example("environment_observation"))
            .expect("example round-trips");
        assert!(o.validate().is_ok());
        assert!(
            o.may_authorize_device_control(),
            "verified_fact may authorize"
        );

        let mut h = o.clone();
        h.epistemic_status = EpistemicStatus::Hypothesis;
        assert!(
            !h.may_authorize_device_control(),
            "hypothesis never authorizes control"
        );

        // verified_fact without verification evidence is invalid
        let mut v = o.clone();
        v.verification_ref = Some("  ".into());
        assert!(v.validate().is_err());
    }

    #[test]
    fn procedure_example_round_trips_and_promotion_gates_hold() {
        let o: ProcedureOutcome =
            serde_json::from_value(example("procedure_outcome")).expect("example round-trips");
        assert!(o.validate().is_ok());
        // Suggested without explicit approval: promotable() is false until approval.
        assert!(!o.promotable().unwrap());

        let mut approved = o.clone();
        approved.promotion.status = PromotionStatus::Approved;
        approved.promotion.requirements.explicit_approval = true;
        assert!(approved.promotable().unwrap());
        assert!(approved.validate().is_ok());

        // Unverified success is NOT promotion evidence.
        let mut unverified = approved.clone();
        unverified.verification.verified = false;
        assert!(!unverified.promotable().unwrap());
        assert!(unverified.validate().is_err());

        // BLOCK procedures are never promotable.
        let mut blocked = approved;
        blocked.risk_class = "BLOCK".into();
        assert!(blocked.promotable().is_err());
        // APPROVED + BLOCK: the promotable() Err propagates out of validate().
        assert!(blocked.validate().is_err());

        // Parity pin (live-caught Kotlin divergence): for status NONE the
        // `matches!(Approved) && !self.promotable()?` short-circuit means
        // validate() NEVER consults promotable() — a BLOCK-tier record with
        // status NONE is valid honest telemetry (the attempt was recorded,
        // it can never promote). Producers rely on this to store blocked
        // attempts; a mirror that eagerly calls promotable() silently drops
        // every BLOCK record.
        let mut blocked_telemetry = o.clone();
        blocked_telemetry.risk_class = "BLOCK".into();
        assert!(blocked_telemetry.validate().is_ok());
        assert!(blocked_telemetry.promotable().is_err());
    }

    #[test]
    fn audio_example_round_trips_and_honesty_rules_hold() {
        let s: AudioBackendStatus =
            serde_json::from_value(example("audio_backend_status")).expect("example round-trips");
        assert!(s.validate().is_ok());
        assert_eq!(s.backend, AudioBackend::SherpaOnnx);
        assert!(s.offline);

        // Reference engines can never claim production readiness.
        let mut r = s.clone();
        r.backend = AudioBackend::Reference;
        assert!(r.validate().is_err());

        // System fallback is online-ish.
        let mut f = s.clone();
        f.backend = AudioBackend::AndroidSystemTts;
        f.offline = true;
        assert!(f.validate().is_err());

        // Not-ready requires a reason (fail-closed).
        let mut nr = s;
        nr.ready = false;
        nr.reason = None;
        assert!(nr.validate().is_err());
    }

    #[test]
    fn device_example_round_trips_and_control_requires_verified_adapter() {
        let d: DeviceCapability =
            serde_json::from_value(example("device_capability")).expect("example round-trips");
        assert!(d.validate().is_ok());
        assert!(
            d.allows_control(),
            "verified + evidenced adapter may control"
        );

        let mut u = d.clone();
        u.verification = DeviceVerification::Unverified;
        assert!(!u.allows_control(), "unverified devices never control");

        let mut noev = d;
        noev.verification_evidence = Some("".into());
        assert!(!noev.allows_control());
    }
}
