//! Stage 2 bounded knowledge metadata. No record is execution authority.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const KNOWLEDGE_SCHEMA: &str = "inbharat.pai.knowledge.v1";
pub const MAX_RECORD_BYTES: usize = 256 * 1024;
pub const MAX_CONTENT_BYTES: usize = 64 * 1024;
pub const MAX_REFERENCES: usize = 64;
pub const MAX_REVISION: u32 = 4096;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeKind {
    Evidence,
    Candidate,
    VerifiedPattern,
    ApprovedProcedure,
    Invalidation,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgePrivacy {
    Private,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Observation,
    Artifact,
    CheckResult,
    ProcedureRun,
    UiApproval,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Supporting,
    Contradicting,
    Derived,
    Superseding,
    InvalidatedBy,
}

/// Digest is SHA-256 of the canonical serialized KnowledgeRecord, not a model assertion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct RecordRef {
    pub logical_id: String,
    pub revision: u32,
    pub kind: KnowledgeKind,
    pub content_digest: String,
}

impl RecordRef {
    pub fn validate(&self) -> Result<(), String> {
        valid_id(&self.logical_id)?;
        if self.revision == 0 || self.revision > MAX_REVISION || !valid_digest(&self.content_digest)
        {
            return Err("invalid reference revision or digest".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GraphEdge {
    pub relation: EdgeKind,
    pub target: RecordRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Audit {
    pub actor: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Applicability {
    pub topics: Vec<String>,
    pub platforms: Vec<String>,
    pub constraints: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeMetadata {
    pub source_id: String,
    pub source_version: String,
    /// Lowercase SHA-1 or SHA-256 commit identity; no claim of repository trust.
    pub source_commit: String,
    pub license: String,
    pub privacy: KnowledgePrivacy,
    pub applicability: Applicability,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecordHeader {
    #[serde(deserialize_with = "strict_schema")]
    pub schema: String,
    pub logical_id: String,
    pub revision: u32,
    pub previous: Option<RecordRef>,
    pub timestamp_ms: u64,
    pub audit: Audit,
    pub metadata: KnowledgeMetadata,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub header: RecordHeader,
    pub evidence_kind: EvidenceKind,
    pub content: String,
    /// SHA-256 of UTF-8 content; recomputed by the vault adapter.
    pub content_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub header: RecordHeader,
    pub statement: String,
    pub evidence: Vec<RecordRef>,
}

/// Recorded verification metadata only. Stage 4 must run actual checks again.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerifiedPattern {
    pub header: RecordHeader,
    pub candidate: RecordRef,
    pub checks: Vec<RecordRef>,
    pub statement: String,
}

/// Retains the existing ProcedureOutcome vocabulary, but its JSON booleans and
/// the name "approved" cannot grant execution authority. Stage 4 must recompute
/// promotion requirements from actual runs AND obtain explicit UI approval.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ApprovedProcedure {
    pub header: RecordHeader,
    pub pattern: RecordRef,
    #[serde(deserialize_with = "strict_outcome")]
    pub outcome: crate::ProcedureOutcome,
    pub outcome_evidence: Vec<RecordRef>,
    pub approval_evidence: RecordRef,
}

/// Append-only revocation: target bytes remain intact for audit reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Invalidation {
    pub header: RecordHeader,
    pub target: RecordRef,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "kind",
    content = "record",
    rename_all = "snake_case",
    deny_unknown_fields
)]
/// The large procedure payload is boxed in Rust only; serde preserves the v1
/// tagged JSON envelope and canonical field order (no wire/schema migration).
/// Construct it with `KnowledgeRecord::ApprovedProcedure(Box::new(procedure))`.
pub enum KnowledgeRecord {
    Evidence(Evidence),
    Candidate(Candidate),
    VerifiedPattern(VerifiedPattern),
    ApprovedProcedure(Box<ApprovedProcedure>),
    Invalidation(Invalidation),
}

impl KnowledgeRecord {
    /// Strict ingress: old/future schemas are rejected, never migrated implicitly.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err("record exceeds byte bound".into());
        }
        let record: Self =
            serde_json::from_slice(bytes).map_err(|_| "malformed knowledge record")?;
        record.validate()?;
        Ok(record)
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| "cannot encode record")?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err("record exceeds byte bound".into());
        }
        Ok(bytes)
    }
    pub fn header(&self) -> &RecordHeader {
        match self {
            Self::Evidence(x) => &x.header,
            Self::Candidate(x) => &x.header,
            Self::VerifiedPattern(x) => &x.header,
            Self::ApprovedProcedure(x) => &x.header,
            Self::Invalidation(x) => &x.header,
        }
    }
    pub fn kind(&self) -> KnowledgeKind {
        match self {
            Self::Evidence(_) => KnowledgeKind::Evidence,
            Self::Candidate(_) => KnowledgeKind::Candidate,
            Self::VerifiedPattern(_) => KnowledgeKind::VerifiedPattern,
            Self::ApprovedProcedure(_) => KnowledgeKind::ApprovedProcedure,
            Self::Invalidation(_) => KnowledgeKind::Invalidation,
        }
    }
    /// Deliberately unconditional. This metadata API is not the Stage 4 runner.
    pub fn authorizes_execution(&self) -> bool {
        false
    }
    /// All typed and graph references, including the revision predecessor.
    pub fn references(&self) -> Vec<&RecordRef> {
        let h = self.header();
        let mut refs: Vec<_> = h.previous.iter().collect();
        refs.extend(h.edges.iter().map(|e| &e.target));
        match self {
            Self::Evidence(_) => {}
            Self::Candidate(x) => refs.extend(x.evidence.iter()),
            Self::VerifiedPattern(x) => {
                refs.push(&x.candidate);
                refs.extend(x.checks.iter());
            }
            Self::ApprovedProcedure(x) => {
                refs.push(&x.pattern);
                refs.extend(x.outcome_evidence.iter());
                refs.push(&x.approval_evidence);
            }
            Self::Invalidation(x) => refs.push(&x.target),
        }
        refs
    }
    pub fn validate(&self) -> Result<(), String> {
        let h = self.header();
        if h.schema != KNOWLEDGE_SCHEMA {
            return Err("unsupported knowledge schema".into());
        }
        valid_id(&h.logical_id)?;
        if h.revision == 0 || h.revision > MAX_REVISION || h.timestamp_ms == 0 {
            return Err("invalid revision or timestamp".into());
        }
        // An immutable invalidation ID cannot become any other record kind.
        // Store/catalog lineage validation also enforces this for existing IDs.
        if h.previous
            .as_ref()
            .is_some_and(|p| p.kind == KnowledgeKind::Invalidation)
        {
            return Err("invalidation IDs are immutable audit facts".into());
        }
        match (&h.previous, h.revision) {
            (None, 1) => {}
            (Some(p), revision)
                if p.logical_id == h.logical_id && p.revision.checked_add(1) == Some(revision) =>
            {
                p.validate()?;
            }
            _ => return Err("revision requires exact predecessor".into()),
        }
        if matches!(self, Self::Evidence(_) | Self::Invalidation(_))
            && (h.revision != 1 || h.previous.is_some())
        {
            return Err("evidence and invalidation IDs are create-only".into());
        }
        text(&h.audit.actor, 128)?;
        text(&h.audit.reason, 4096)?;
        let m = &h.metadata;
        text(&m.source_id, 512)?;
        text(&m.source_version, 256)?;
        text(&m.license, 1024)?;
        if !valid_hex(&m.source_commit, 40) && !valid_hex(&m.source_commit, 64) {
            return Err("invalid source commit".into());
        }
        strings(&m.applicability.topics, 32, 256)?;
        strings(&m.applicability.platforms, 16, 128)?;
        text(&m.applicability.constraints, 4096)?;
        if h.edges.len() > MAX_REFERENCES {
            return Err("too many graph edges".into());
        }
        let mut unique = BTreeSet::new();
        for edge in &h.edges {
            edge.target.validate()?;
            if !unique.insert((
                serde_json::to_string(&edge.relation).unwrap(),
                edge.target.clone(),
            )) {
                return Err("duplicate graph edge".into());
            }
            if edge.relation == EdgeKind::Superseding
                && edge.target.kind == KnowledgeKind::Invalidation
            {
                return Err("invalidations cannot be superseded".into());
            }
            if edge.relation == EdgeKind::InvalidatedBy
                && edge.target.kind != KnowledgeKind::Invalidation
            {
                return Err("invalidated_by requires invalidation".into());
            }
        }
        for reference in self.references() {
            reference.validate()?;
            if reference.logical_id == h.logical_id && reference.revision >= h.revision {
                return Err("self or future revision reference".into());
            }
        }
        match self {
            Self::Evidence(x) => {
                text(&x.content, MAX_CONTENT_BYTES)?;
                if !valid_digest(&x.content_sha256) {
                    return Err("invalid content SHA-256".into());
                }
            }
            Self::Candidate(x) => {
                text(&x.statement, 16 * 1024)?;
                refs(&x.evidence, KnowledgeKind::Evidence)?;
            }
            Self::VerifiedPattern(x) => {
                expected_kind(&x.candidate, KnowledgeKind::Candidate)?;
                refs(&x.checks, KnowledgeKind::Evidence)?;
                text(&x.statement, 16 * 1024)?;
            }
            Self::ApprovedProcedure(x) => {
                expected_kind(&x.pattern, KnowledgeKind::VerifiedPattern)?;
                refs(&x.outcome_evidence, KnowledgeKind::Evidence)?;
                expected_kind(&x.approval_evidence, KnowledgeKind::Evidence)?;
                validate_outcome(&x.outcome)?;
            }
            Self::Invalidation(x) => {
                text(&x.reason, 4096)?;
                if x.target.kind == KnowledgeKind::Invalidation {
                    return Err("revocations cannot revoke revocations".into());
                }
            }
        }
        Ok(())
    }
}

fn strict_schema<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let schema = String::deserialize(deserializer)?;
    if schema != KNOWLEDGE_SCHEMA {
        return Err(serde::de::Error::custom("unsupported knowledge schema"));
    }
    Ok(schema)
}

pub fn valid_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err("invalid logical identity".into());
    }
    Ok(())
}
pub fn valid_digest(value: &str) -> bool {
    valid_hex(value, 64)
}
fn valid_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn text(value: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        return Err("invalid text bound".into());
    }
    Ok(())
}
fn strings(values: &[String], max: usize, size: usize) -> Result<(), String> {
    if values.is_empty() || values.len() > max {
        return Err("invalid list bound".into());
    }
    let mut seen = BTreeSet::new();
    for v in values {
        text(v, size)?;
        if !seen.insert(v) {
            return Err("duplicate list item".into());
        }
    }
    Ok(())
}
fn expected_kind(reference: &RecordRef, kind: KnowledgeKind) -> Result<(), String> {
    reference.validate()?;
    if reference.kind != kind {
        return Err("incorrect reference kind".into());
    }
    Ok(())
}
fn refs(values: &[RecordRef], kind: KnowledgeKind) -> Result<(), String> {
    if values.is_empty() || values.len() > MAX_REFERENCES {
        return Err("invalid evidence list bound".into());
    }
    let mut seen = BTreeSet::new();
    for v in values {
        expected_kind(v, kind)?;
        if !seen.insert(v) {
            return Err("duplicate evidence reference".into());
        }
    }
    Ok(())
}
fn validate_outcome(x: &crate::ProcedureOutcome) -> Result<(), String> {
    x.validate()?;
    valid_id(&x.procedure_id)?;
    for value in [
        &x.bounded_arguments,
        &x.preconditions,
        &x.postconditions,
        &x.verification.evidence,
    ] {
        text(value, 16 * 1024)?;
    }
    text(&x.risk_class, 64)?;
    text(&x.promotion.policy_version, 256)?;
    if x.timestamp_ms == 0 {
        return Err("invalid outcome timestamp".into());
    }
    if let Some(v) = &x.failure_reason {
        text(v, 4096)?;
    }
    for v in [
        &x.provenance.platform,
        &x.provenance.device_id,
        &x.provenance.source,
    ] {
        text(v, 512)?;
    }
    if let Some(v) = &x.provenance.model {
        text(v, 512)?;
    }
    if let Some(v) = &x.provenance.artifact_sha256 {
        if !valid_digest(v) {
            return Err("invalid artifact hash".into());
        }
    }
    Ok(())
}

// Strict ingress mirrors only: the stored/public outcome remains the existing
// ProcedureOutcome. Its older lenient serde parser cannot discard unknown
// nested keys (including purported authority flags) at this boundary.
fn strict_outcome<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<crate::ProcedureOutcome, D::Error> {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Requirements {
        bounded_arguments: bool,
        repeatable_success: bool,
        verified_postconditions: bool,
        low_risk_class: bool,
        no_contradictory_evidence: bool,
        explicit_approval: bool,
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Promotion {
        status: crate::PromotionStatus,
        policy_version: String,
        requirements: Requirements,
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Verification {
        verified: bool,
        evidence: String,
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Provenance {
        platform: String,
        device_id: String,
        source: String,
        model: Option<String>,
        artifact_sha256: Option<String>,
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Outcome {
        schema: String,
        procedure_id: String,
        bounded_arguments: String,
        preconditions: String,
        postconditions: String,
        result: crate::ProcedureResult,
        failure_reason: Option<String>,
        verification: Verification,
        risk_class: String,
        promotion: Promotion,
        timestamp_ms: u64,
        provenance: Provenance,
    }
    let value = Outcome::deserialize(deserializer)?;
    let json = serde_json::to_value(value).map_err(serde::de::Error::custom)?;
    serde_json::from_value(json).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> KnowledgeRecord {
        KnowledgeRecord::Evidence(Evidence {
            header: RecordHeader {
                schema: KNOWLEDGE_SCHEMA.into(),
                logical_id: "evidence-one".into(),
                revision: 1,
                previous: None,
                timestamp_ms: 1,
                audit: Audit {
                    actor: "fixture-user".into(),
                    reason: "fixture-import".into(),
                },
                metadata: KnowledgeMetadata {
                    source_id: "private-source-marker".into(),
                    source_version: "v1".into(),
                    source_commit: "aef047b7b0c0cfb57f1f56ed2be33affcf60247e".into(),
                    license: "private-license-marker".into(),
                    privacy: KnowledgePrivacy::Private,
                    applicability: Applicability {
                        topics: vec!["private-topic-marker".into()],
                        platforms: vec!["fixture-host".into()],
                        constraints: "private-path-marker".into(),
                    },
                },
                edges: vec![],
            },
            evidence_kind: EvidenceKind::Observation,
            content: "private-payload-marker".into(),
            content_sha256: "a".repeat(64),
        })
    }

    fn outcome() -> crate::ProcedureOutcome {
        let json = serde_json::json!({
            "schema": crate::schemas::PROCEDURE, "procedure_id": "procedure-one",
            "bounded_arguments": "fixed", "preconditions": "checked", "postconditions": "checked",
            "result": "success", "verification": {"verified": true, "evidence": "claim"},
            "risk_class": "LOW", "promotion": {"status": "approved", "policy_version": "v1",
                "requirements": {"bounded_arguments": true, "repeatable_success": true,
                    "verified_postconditions": true, "low_risk_class": true,
                    "no_contradictory_evidence": true, "explicit_approval": true}},
            "timestamp_ms": 1, "provenance": {"platform": "fixture", "device_id": "fixture",
                "source": "self-attestation"}
        });
        let outcome: crate::ProcedureOutcome = serde_json::from_value(json).unwrap();
        outcome
    }

    fn immutable_reference() -> RecordRef {
        RecordRef {
            logical_id: "immutable-revocation".into(),
            revision: 1,
            kind: KnowledgeKind::Invalidation,
            content_digest: "b".repeat(64),
        }
    }

    #[test]
    fn knowledge_immutable_invalidation_cannot_be_superseded() {
        let mut hostile = evidence();
        if let KnowledgeRecord::Evidence(ref mut item) = hostile {
            item.header.edges.push(GraphEdge {
                relation: EdgeKind::Superseding,
                target: immutable_reference(),
            });
        }
        assert!(
            hostile.validate().is_err(),
            "invalidation cannot be superseded"
        );
        assert!(hostile.encode().is_err());
        assert!(KnowledgeRecord::decode(&serde_json::to_vec(&hostile).unwrap()).is_err());
    }

    #[test]
    fn knowledge_immutable_invalidation_cannot_transition_to_any_kind() {
        let mut header = evidence().header().clone();
        header.logical_id = immutable_reference().logical_id.clone();
        header.revision = 2;
        header.previous = Some(immutable_reference());
        let evidence_ref = RecordRef {
            logical_id: "evidence-one".into(),
            kind: KnowledgeKind::Evidence,
            ..immutable_reference()
        };
        let records = [
            KnowledgeRecord::Evidence(Evidence {
                header: header.clone(),
                evidence_kind: EvidenceKind::Observation,
                content: "fixture".into(),
                content_sha256: "a".repeat(64),
            }),
            KnowledgeRecord::Candidate(Candidate {
                header: header.clone(),
                statement: "fixture".into(),
                evidence: vec![evidence_ref.clone()],
            }),
            KnowledgeRecord::VerifiedPattern(VerifiedPattern {
                header: header.clone(),
                candidate: RecordRef {
                    logical_id: "candidate-one".into(),
                    kind: KnowledgeKind::Candidate,
                    ..evidence_ref.clone()
                },
                checks: vec![evidence_ref.clone()],
                statement: "fixture".into(),
            }),
            KnowledgeRecord::ApprovedProcedure(Box::new(ApprovedProcedure {
                header: header.clone(),
                pattern: RecordRef {
                    logical_id: "pattern-one".into(),
                    kind: KnowledgeKind::VerifiedPattern,
                    ..evidence_ref.clone()
                },
                outcome: outcome(),
                outcome_evidence: vec![evidence_ref.clone()],
                approval_evidence: evidence_ref.clone(),
            })),
            KnowledgeRecord::Invalidation(Invalidation {
                header,
                target: evidence_ref,
                reason: "fixture".into(),
            }),
        ];
        for record in records {
            assert!(
                record.validate().is_err(),
                "{:?} cannot replace invalidation",
                record.kind()
            );
            assert!(record.encode().is_err());
            assert!(KnowledgeRecord::decode(&serde_json::to_vec(&record).unwrap()).is_err());
        }
    }

    #[test]
    fn knowledge_record_layout_bounds_large_payload() {
        assert!(
            std::mem::size_of::<KnowledgeRecord>() <= 512,
            "large procedure payload must not be stored inline in every record"
        );
    }

    #[test]
    fn knowledge_strict_schema_bounds_and_edges() {
        let record = evidence();
        assert!(record.validate().is_ok());
        let bytes = serde_json::to_vec(&record).unwrap();
        assert_eq!(KnowledgeRecord::decode(&bytes).unwrap(), record);
        let mut json = serde_json::to_value(&record).unwrap();
        json["record"]["header"]["unexpected"] = true.into();
        assert!(KnowledgeRecord::decode(&serde_json::to_vec(&json).unwrap()).is_err());
        let mut json = serde_json::to_value(&record).unwrap();
        for schema in ["inbharat.pai.knowledge.v0", "inbharat.pai.knowledge.v99"] {
            json["record"]["header"]["schema"] = schema.into();
            assert!(KnowledgeRecord::decode(&serde_json::to_vec(&json).unwrap()).is_err());
        }
        let mut bad = record.clone();
        if let KnowledgeRecord::Evidence(ref mut item) = bad {
            item.content = "x".repeat(MAX_CONTENT_BYTES + 1);
        }
        assert!(bad.validate().is_err());
        let mut bad = record;
        if let KnowledgeRecord::Evidence(ref mut item) = bad {
            item.content_sha256 = "not-a-hash".into();
        }
        assert!(bad.validate().is_err());
        for edge in [
            EdgeKind::Supporting,
            EdgeKind::Contradicting,
            EdgeKind::Derived,
            EdgeKind::Superseding,
            EdgeKind::InvalidatedBy,
        ] {
            assert_eq!(
                serde_json::from_slice::<EdgeKind>(&serde_json::to_vec(&edge).unwrap()).unwrap(),
                edge
            );
        }
    }

    #[test]
    fn knowledge_metadata_never_authorizes_even_with_all_procedure_flags() {
        let outcome = outcome();
        let reference = RecordRef {
            logical_id: "evidence-one".into(),
            revision: 1,
            kind: KnowledgeKind::Evidence,
            content_digest: "b".repeat(64),
        };
        let mut header = evidence().header().clone();
        header.logical_id = "approved-one".into();
        let record = KnowledgeRecord::ApprovedProcedure(Box::new(ApprovedProcedure {
            header,
            pattern: RecordRef {
                kind: KnowledgeKind::VerifiedPattern,
                logical_id: "pattern-one".into(),
                ..reference.clone()
            },
            outcome,
            outcome_evidence: vec![reference.clone()],
            approval_evidence: reference,
        }));
        assert!(record.validate().is_ok());
        assert!(!record.authorizes_execution());
        #[derive(Serialize)]
        #[serde(tag = "kind", content = "record", rename_all = "snake_case")]
        enum InlineV1Record {
            ApprovedProcedure(ApprovedProcedure),
        }
        let KnowledgeRecord::ApprovedProcedure(payload) = &record else {
            unreachable!();
        };
        let legacy = InlineV1Record::ApprovedProcedure(payload.as_ref().clone());
        let legacy_bytes = serde_json::to_vec(&legacy).unwrap();
        assert_eq!(record.encode().unwrap(), legacy_bytes);
        assert_eq!(KnowledgeRecord::decode(&legacy_bytes).unwrap(), record);
        let mut encoded = serde_json::to_value(record).unwrap();
        encoded["record"]["outcome"]["promotion"]["requirements"]["model_authority"] = true.into();
        assert!(KnowledgeRecord::decode(&serde_json::to_vec(&encoded).unwrap()).is_err());
    }
}
