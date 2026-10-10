//! Trusted native integration seam. These values deliberately implement neither
//! Serialize nor Deserialize. Calling these constructors is a HOST responsibility:
//! evidence must come from a native verifier and approval from a local human/policy,
//! never from model output or a hydrated record. Structural validation is not proof.
use crate::*;

#[derive(Debug, Clone)]
pub struct LocalGrant {
    grant: CapabilityGrant,
    approval_ref: String,
}
impl LocalGrant {
    pub fn grant(&self) -> &CapabilityGrant {
        &self.grant
    }
    pub fn approval_ref(&self) -> &str {
        &self.approval_ref
    }
    pub fn live(&self, replica: &str, now: u64, stop_generation: u64) -> bool {
        !self.grant.revoked
            && self.grant.replica_id == replica
            && now >= self.grant.issued_at_ms
            && now < self.grant.expires_at_ms
            && stop_generation == self.grant.stop_generation
    }
}
/// Only native local consent storage may call this; no wire approval boolean exists.
pub fn approve_locally(
    grant: CapabilityGrant,
    local_replica: &str,
    approval_ref: &str,
    now: u64,
    stop_generation: u64,
) -> Result<LocalGrant> {
    Document::new(Record::CapabilityGrant(grant.clone()))?;
    ensure(
        !approval_ref.is_empty() && approval_ref.len() <= 128,
        "local approval reference required",
    )?;
    let local = LocalGrant {
        grant,
        approval_ref: approval_ref.into(),
    };
    ensure(
        local.live(local_replica, now, stop_generation),
        "grant expired/revoked/wrong host/generation",
    )?;
    Ok(local)
}
/// Requested child ∩ live parent ∩ native host manifest. Empty scopes stay empty.
/// This does not reserve a budget or spawn a worker; host must atomically account
/// sibling consumption and propagate stop/revocation before every dispatch.
pub fn attenuate_child(
    request: &AgentSpec,
    parent: &LocalGrant,
    host: &Scopes,
    local_replica: &str,
    now: u64,
    stop_generation: u64,
) -> Result<AgentSpec> {
    Document::new(Record::AgentSpec(request.clone()))?;
    host.validate()?;
    ensure(
        parent.live(local_replica, now, stop_generation),
        "parent grant not live",
    )?;
    ensure(
        request.stop_generation == stop_generation && request.expires_at_ms > now,
        "stopped/expired child",
    )?;
    let mut child = request.clone();
    child.scopes = request
        .scopes
        .intersect(&parent.grant.scopes)
        .intersect(host);
    ensure(
        child.scopes.hosts.iter().any(|h| h == local_replica),
        "host not granted",
    )?;
    child.budget = request.budget.intersect(&parent.grant.budget);
    if child.scopes.network == NetworkPolicy::OfflineOnly {
        child.budget.max_network_calls = 0;
    }
    child.expires_at_ms = request.expires_at_ms.min(parent.grant.expires_at_ms);
    child.budget.max_duration_ms = child.budget.max_duration_ms.min(child.expires_at_ms - now);
    ensure(
        child.depth <= child.budget.max_depth && parent.grant.budget.max_children > 0,
        "child depth/count",
    )?;
    Document::new(Record::AgentSpec(child.clone()))?;
    Ok(child)
}

/// Host-generated observation, NOT a serializable model claim. Do not populate it
/// from JSON. Provider mutation verification requires reconciled provider object ID.
#[derive(Debug)]
pub struct NativeObservation {
    pub task_id: String,
    pub operation_id: String,
    pub replica_id: String,
    pub evidence_ref: String,
    pub postcondition_matched: bool,
    pub external_object_id: Option<String>,
}
#[derive(Debug, Clone)]
pub struct VerifiedReceipt {
    receipt: TaskReceipt,
}
impl VerifiedReceipt {
    pub fn receipt(&self) -> &TaskReceipt {
        &self.receipt
    }
}
/// No automatic `From<TaskReceipt>` or deserialize path can confer verification.
pub fn verify_native(
    claim: &TaskReceipt,
    observation: NativeObservation,
    local_replica: &str,
) -> Result<VerifiedReceipt> {
    Document::new(Record::TaskReceipt(claim.clone()))?;
    ensure(
        claim.source == ProvenanceSource::Native
            && claim.replica_id == local_replica
            && observation.replica_id == local_replica
            && observation.task_id == claim.task_id
            && observation.operation_id == claim.operation_id
            && observation.postcondition_matched,
        "native postcondition mismatch",
    )?;
    ensure(
        !observation.evidence_ref.is_empty()
            && claim
                .after_evidence_refs
                .contains(&observation.evidence_ref),
        "native evidence mismatch",
    )?;
    ensure(
        !matches!(
            claim.dispatch_intent,
            DispatchIntent::OpenComposer | DispatchIntent::OpenEventForm
        ),
        "action-only observation",
    )?;
    if claim.dispatch_intent == DispatchIntent::ProviderMutation {
        ensure(
            observation
                .external_object_id
                .as_ref()
                .is_some_and(|id| !id.is_empty())
                && observation.external_object_id == claim.external_object_id,
            "provider object not reconciled",
        )?;
    }
    let mut receipt = claim.clone();
    receipt.outcome = ReceiptOutcome::Verified;
    Ok(VerifiedReceipt { receipt })
}
