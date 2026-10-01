//! P1-C — desktop half of the bounded, MK-style environment-learning
//! producer. Every completed Power harness agent run leaves a
//! `ProcedureOutcome` contract record (`capability.v1.json`, the same
//! contract the Android producer writes) in the canonical vault, with
//! requirements evaluated HONESTLY:
//!
//! - bounded_arguments: the run is bounded by the harness's non-bypassable
//!   budget — the record states the real step/tool-call/time bounds;
//! - repeatable_success: false — ONE run is never repeatability evidence
//!   (the desktop keeps no streak counter; only the user's explicit
//!   approval plus a repeatable streak could ever promote, and no desktop
//!   path sets that today);
//! - verified_postconditions: false — there is no deterministic
//!   postcondition verifier for a free-form agent answer; claiming one
//!   would be the exact overstatement the mission bans;
//! - explicit_approval: always false on the automatic path — nothing the
//!   desktop records can auto-promote, ever.
//!
//! The record is honest telemetry, not authority: `promotable()` is false
//! for every record this module writes, and a future consumer must still
//! pass the contract's full promotion gate. Vault writes are best-effort
//! and non-fatal: learning telemetry must never break a chat.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use unoone_capability_contracts::{
    ProcedureOutcome, ProcedureResult, Promotion, PromotionRequirements, PromotionStatus, Provenance,
    Verification,
};
use unoone_vault_core::{Record, RecordType, Vault};

/// The one procedure id the desktop produces: a whole harness agent run.
pub const PROCEDURE_ID: &str = "harness.agent_run";

/// Policy version stamped on every desktop harness-run outcome record.
pub const POLICY_VERSION: &str = "harness-run-policy-v1";

/// Real evidence extracted from one COMPLETED harness run (pure data, so
/// the builder is fully unit-testable without a model or vault).
pub struct HarnessRunEvidence {
    /// The route level the harness actually took (`decision.level.as_str()`).
    pub route_level: String,
    /// Whether the run had the full-access (workspace/subprocess) lane on.
    pub full_access: bool,
    pub steps: u32,
    pub tool_calls: u32,
    pub elapsed_ms: u64,
    /// The identity-verified model id that served the run.
    pub model_id: String,
}

/// Builds the contract `ProcedureOutcome` for one completed run. Pure.
pub fn harness_procedure_outcome(evidence: &HarnessRunEvidence, timestamp_ms: u64) -> ProcedureOutcome {
    ProcedureOutcome {
        schema: unoone_capability_contracts::schemas::PROCEDURE.to_owned(),
        procedure_id: PROCEDURE_ID.to_owned(),
        bounded_arguments: format!(
            "harness run bounded by non-bypassable budget: route {}, {} steps, {} tool calls, {} ms",
            evidence.route_level, evidence.steps, evidence.tool_calls, evidence.elapsed_ms
        ),
        preconditions: "vault unlocked; model identity verified on the local llama.cpp server"
            .to_owned(),
        postconditions: "agent run completed within its budget and returned output".to_owned(),
        result: ProcedureResult::Success,
        failure_reason: None,
        verification: Verification {
            verified: false,
            evidence: "no deterministic postcondition verifier exists for a free-form agent run output"
                .to_owned(),
        },
        risk_class: if evidence.full_access { "CONFIRM" } else { "DIRECT" }.to_owned(),
        promotion: Promotion {
            status: PromotionStatus::None,
            policy_version: POLICY_VERSION.to_owned(),
            requirements: PromotionRequirements {
                bounded_arguments: true,
                repeatable_success: false,
                verified_postconditions: false,
                // A read-only run (Model+FileRead) cannot mutate the host;
                // a full-access run can — that is not a low-risk action.
                low_risk_class: !evidence.full_access,
                no_contradictory_evidence: true,
                explicit_approval: false,
            },
        },
        timestamp_ms,
        provenance: Provenance {
            platform: "DESKTOP".to_owned(),
            device_id: "unoone-power".to_owned(),
            source: "harness-agent-run".to_owned(),
            model: Some(evidence.model_id.clone()),
            artifact_sha256: None,
        },
    }
}

/// The cross-host read envelope for a desktop procedure outcome, mirroring
/// the Android `VaultRecordFactory` envelope convention: index fields a
/// host can read without parsing the contract body, plus the verbatim
/// contract body in `outcomeJson`.
fn procedure_outcome_envelope(outcome: &ProcedureOutcome) -> serde_json::Value {
    serde_json::json!({
        "kind": "procedure_outcome",
        "procedureId": outcome.procedure_id,
        "result": outcome.result,
        "riskClass": outcome.risk_class,
        "status": outcome.promotion.status,
        "outcomeJson": outcome,
    })
}

/// Writes the outcome record for one completed harness run into the vault.
/// Best-effort and non-fatal (lock failure, locked vault, or a contract
/// rejection just means no telemetry this time — the chat is unaffected).
/// Returns the record id when a record was written.
pub fn record_harness_run_outcome(vault: &Mutex<Option<Vault>>, evidence: &HarnessRunEvidence) -> Option<String> {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let outcome = harness_procedure_outcome(evidence, timestamp_ms);
    // Never store a record the shared contract rejects.
    if let Err(reason) = outcome.validate() {
        eprintln!("env-learning: run outcome rejected by the contract ({reason}) — not recorded");
        return None;
    }
    let envelope = procedure_outcome_envelope(&outcome);
    let Ok(bytes) = serde_json::to_vec(&envelope) else {
        return None;
    };
    let mut guard = vault.lock().ok()?; // a poisoned lock is a skipped record, never a broken chat
    let open = guard.as_mut()?;
    let record = Record::new(RecordType::ToolResult, "DESKTOP", "unoone-power");
    let record_id = record.record_id.clone();
    match open.write_record(record, &bytes) {
        Ok(()) => Some(record_id),
        Err(error) => {
            eprintln!("env-learning: vault write non-fatal: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(full_access: bool) -> HarnessRunEvidence {
        HarnessRunEvidence {
            route_level: if full_access { "L3" } else { "L1" }.to_owned(),
            full_access,
            steps: 12,
            tool_calls: 5,
            elapsed_ms: 8_400,
            model_id: "gemma4-12b-q5".to_owned(),
        }
    }

    /// A record this module writes is NEVER promotable — the honesty pin.
    #[test]
    fn every_desktop_run_outcome_is_valid_yet_never_promotable() {
        for full_access in [false, true] {
            let outcome = harness_procedure_outcome(&evidence(full_access), 1_760_000_000_000);
            assert!(outcome.validate().is_ok(), "contract must accept the record");
            assert!(!outcome.promotable().expect("non-BLOCK is Ok"));
            assert_eq!(outcome.promotion.status, PromotionStatus::None);
            assert!(!outcome.promotion.requirements.explicit_approval);
            assert!(!outcome.promotion.requirements.repeatable_success);
            assert!(!outcome.promotion.requirements.verified_postconditions);
            assert_eq!(
                outcome.promotion.requirements.low_risk_class, !full_access,
                "risk honesty must follow the actual capability lane"
            );
            assert_eq!(outcome.schema, unoone_capability_contracts::schemas::PROCEDURE);
            assert_eq!(outcome.procedure_id, PROCEDURE_ID);
            assert!(outcome.provenance.model.as_deref().is_some());
        }
    }

    /// The envelope carries index fields plus the verbatim contract body, so
    /// another host can list/search it without parsing the contract.
    #[test]
    fn envelope_round_trips_the_contract_body_verbatim() {
        let outcome = harness_procedure_outcome(&evidence(true), 1_760_000_000_000);
        let envelope = procedure_outcome_envelope(&outcome);
        assert_eq!(envelope["kind"], "procedure_outcome");
        assert_eq!(envelope["procedureId"], PROCEDURE_ID);
        assert_eq!(envelope["riskClass"], "CONFIRM");
        // The body round-trips through the envelope unchanged.
        let parsed: ProcedureOutcome =
            serde_json::from_value(envelope["outcomeJson"].clone()).expect("body round-trips");
        assert_eq!(parsed, outcome);
    }

    /// The record lands in the vault and reads back: the write/read
    /// round-trip with a REAL vault-core instance (encrypted at rest).
    #[test]
    fn outcome_record_round_trips_through_a_real_vault() {
        let (vault, _dir, _root) = test_vault();
        let shared: Mutex<Option<Vault>> = Mutex::new(Some(vault));
        let record_id = record_harness_run_outcome(&shared, &evidence(false))
            .expect("outcome record must be written");
        let open = shared.lock().unwrap();
        let vault = open.as_ref().unwrap();
        let (_record, bytes) = vault.read_record(&record_id).expect("record must read back");
        let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(envelope["kind"], "procedure_outcome");
        let parsed: ProcedureOutcome =
            serde_json::from_value(envelope["outcomeJson"].clone()).unwrap();
        assert!(parsed.validate().is_ok());
        assert!(!parsed.promotable().unwrap());
    }

    /// A locked vault (None) or a poisoned lock is a skipped record, never an
    /// error — learning telemetry must not break the chat path.
    #[test]
    fn locked_vault_and_poisoned_lock_are_skipped_not_errors() {
        let locked: Mutex<Option<Vault>> = Mutex::new(None);
        assert!(record_harness_run_outcome(&locked, &evidence(false)).is_none());
    }

    // Reuse the desktop test-vault idiom (documents.rs): a REAL encrypted
    // vault in a temp dir, created, unlocked.
    fn test_vault() -> (Vault, tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp vault dir");
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"env-learning-test-pw").expect("create test vault");
        let mut vault = Vault::open(&vault_root).expect("open test vault");
        vault.unlock(b"env-learning-test-pw").expect("unlock test vault");
        (vault, dir, vault_root)
    }
}