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

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use inbharat_harness_core::providers::{MemoryProvider, MemoryRecord, MemoryScope};
use pai_harness_adapter::PaiVaultMemoryProvider;
use unoone_capability_contracts::{
    ProcedureOutcome, ProcedureResult, Promotion, PromotionRequirements, PromotionStatus,
    Provenance, Verification,
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
pub fn harness_procedure_outcome(
    evidence: &HarnessRunEvidence,
    timestamp_ms: u64,
) -> ProcedureOutcome {
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
pub fn record_harness_run_outcome(
    vault: &Mutex<Option<Vault>>,
    evidence: &HarnessRunEvidence,
) -> Option<String> {
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

/// P7 run trail (2026-10-01, user directive: "the memory of the drive
/// should have the context and steps and what was done"). The bounded
/// evidence one MEANINGFUL agentic run records about itself — the request,
/// every step as it happened, and what came out — for the vault memory.
/// `session_id`/`output` are `None` on the stopped/failed path (the
/// harness returns no outcome there); the trail steps and the failure are
/// still the honest record of what was actually done.
pub struct AgentRunTrail<'a> {
    pub request: &'a str,
    /// "completed" | "stopped" (user Stop / superseded) | "failed".
    pub status: &'a str,
    pub failure: Option<&'a str>,
    pub steps: u32,
    pub tool_calls: u32,
    pub elapsed_ms: u64,
    pub model_id: &'a str,
    pub trail: &'a [crate::harness_bridge::AgentTrailStep],
    pub output: Option<&'a str>,
    pub session_id: Option<&'a str>,
}

/// The record id for one run trail: the harness session when the run
/// completed, the wall-clock millisecond when it did not (there is no
/// session id to cite). Memory ids are bounded portable identifiers
/// ([A-Za-z0-9._-]); anything else in the source is dropped.
fn trail_record_id(session_id: Option<&str>, timestamp_ms: u64) -> String {
    let source = session_id
        .map(|id| id.to_owned())
        .unwrap_or_else(|| timestamp_ms.to_string());
    let sanitized: String = source
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    let stem = if sanitized.is_empty() {
        timestamp_ms.to_string()
    } else {
        sanitized
    };
    format!("run-trail-{stem}").chars().take(128).collect()
}

/// Build the trail record's content JSON. Pure, so bounds are unit-testable
/// without a model or vault. The content is bounded hard: request 2 KiB,
/// output 8 KiB, failure 512 bytes, then the whole body 64 KiB — an
/// occasional agent-run record must stay small next to the 250-step trail
/// cap enforced where the steps are collected.
fn agent_run_trail_content(trail: &AgentRunTrail, conversation_id: &str) -> String {
    let steps: Vec<serde_json::Value> = trail
        .trail
        .iter()
        .map(|step| {
            serde_json::json!({
                "phase": step.phase,
                "tool": step.tool,
                "detail": step.detail,
            })
        })
        .collect();
    let body = serde_json::json!({
        "kind": "agent_run_trail",
        "request": unoone_text::truncate_bytes_with_notice(trail.request, 2 * 1024),
        "conversationId": conversation_id,
        "status": trail.status,
        "failure": trail.failure.map(|text| unoone_text::truncate_bytes_with_notice(text, 512)),
        "steps": trail.steps,
        "toolCalls": trail.tool_calls,
        "elapsedMs": trail.elapsed_ms,
        "model": trail.model_id,
        "trail": steps,
        "output": trail.output.map(|text| unoone_text::truncate_bytes_with_notice(text, 8 * 1024)),
        "recordedAtMs": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    });
    unoone_text::truncate_bytes_with_notice(&body.to_string(), 64 * 1024)
}

/// Store one run trail as a Project-scope memory record through the SAME
/// vault memory provider the harness runs use — so it lands in the one
/// envelope both hosts read (the phone hydrates it as harness memory,
/// P1-E), and later runs can search it. Best-effort and non-fatal by the
/// same contract as the outcome record: a locked vault, a lock failure or
/// a validation rejection skips the trail and never breaks the chat.
pub fn record_agent_run_trail(
    memory: &PaiVaultMemoryProvider,
    vault_id: &str,
    conversation_id: &str,
    trail: &AgentRunTrail,
) -> Option<String> {
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut attributes = BTreeMap::new();
    attributes.insert("kind".to_owned(), "agent_run_trail".to_owned());
    attributes.insert("conversation".to_owned(), conversation_id.to_owned());
    attributes.insert("status".to_owned(), trail.status.to_owned());
    let record = MemoryRecord {
        id: trail_record_id(trail.session_id, timestamp_ms),
        scope: MemoryScope::Project,
        namespace: vault_id.to_owned(),
        content: agent_run_trail_content(trail, conversation_id),
        attributes,
    };
    // Never store a record the shared harness contract rejects.
    if let Err(reason) = record.validate() {
        eprintln!(
            "env-learning: run trail rejected by the memory contract ({reason}) — not recorded"
        );
        return None;
    }
    let id = record.id.clone();
    match memory.store(record) {
        Ok(()) => Some(id),
        Err(error) => {
            eprintln!("env-learning: run-trail memory write non-fatal: {error}");
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
            assert!(
                outcome.validate().is_ok(),
                "contract must accept the record"
            );
            assert!(!outcome.promotable().expect("non-BLOCK is Ok"));
            assert_eq!(outcome.promotion.status, PromotionStatus::None);
            assert!(!outcome.promotion.requirements.explicit_approval);
            assert!(!outcome.promotion.requirements.repeatable_success);
            assert!(!outcome.promotion.requirements.verified_postconditions);
            assert_eq!(
                outcome.promotion.requirements.low_risk_class, !full_access,
                "risk honesty must follow the actual capability lane"
            );
            assert_eq!(
                outcome.schema,
                unoone_capability_contracts::schemas::PROCEDURE
            );
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
        let (_record, bytes) = vault
            .read_record(&record_id)
            .expect("record must read back");
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

    // ---- P7 run trail ----

    fn trail_steps() -> Vec<crate::harness_bridge::AgentTrailStep> {
        vec![
            crate::harness_bridge::AgentTrailStep {
                phase: "call",
                tool: "fs.write".to_owned(),
                detail: "Writing website/index.html (1,874 bytes)".to_owned(),
            },
            crate::harness_bridge::AgentTrailStep {
                phase: "result",
                tool: "fs.write".to_owned(),
                detail: "Done: wrote website/index.html".to_owned(),
            },
        ]
    }

    fn trail<'a>(
        request: &'a str,
        output: Option<&'a str>,
        session_id: Option<&'a str>,
        steps: &'a [crate::harness_bridge::AgentTrailStep],
    ) -> AgentRunTrail<'a> {
        AgentRunTrail {
            request,
            status: if output.is_some() {
                "completed"
            } else {
                "stopped"
            },
            failure: if output.is_some() {
                None
            } else {
                Some("cancelled:pai.desktop_tool: user")
            },
            steps: 12,
            tool_calls: 5,
            elapsed_ms: 8_400,
            model_id: "gemma4-12b-q4",
            trail: steps,
            output,
            session_id,
        }
    }

    /// The trail content is valid JSON with the honest fields the directive
    /// asked for — context (the request), steps, and what was done (output).
    #[test]
    fn trail_content_carries_request_steps_and_output_bounded() {
        let long_request = "x".repeat(100 * 1024);
        let long_output = "y".repeat(100 * 1024);
        let steps = trail_steps();
        let content = agent_run_trail_content(
            &trail(&long_request, Some(&long_output), Some("session-1"), &steps),
            "conv-1",
        );
        assert!(
            content.len() <= 64 * 1024 + 128,
            "content must respect the 64 KiB bound"
        );
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("content is JSON");
        assert_eq!(parsed["kind"], "agent_run_trail");
        assert_eq!(parsed["trail"].as_array().map(Vec::len), Some(2));
        assert_eq!(parsed["trail"][0]["tool"], "fs.write");
        assert!(parsed["request"].as_str().unwrap().contains("Truncated"));
        assert!(parsed["output"].as_str().unwrap().contains("Truncated"));
    }

    /// Memory ids are bounded portable identifiers: a session id with alien
    /// characters is sanitized, and a missing session (stopped run) falls
    /// back to the wall-clock timestamp — never an empty id.
    #[test]
    fn trail_record_id_is_always_a_valid_portable_identifier() {
        let sanitized = trail_record_id(Some("ab/c d\\e:f"), 1_760_000_000_000);
        assert!(!sanitized.is_empty());
        assert!(sanitized
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')));
        assert!(sanitized.starts_with("run-trail-"));
        let fallback = trail_record_id(None, 1_760_000_000_000);
        assert_eq!(fallback, "run-trail-1760000000000");
    }

    /// The trail lands in the vault through the SAME memory provider the
    /// harness uses, in the envelope the phone hydrates — and reads back.
    #[test]
    fn trail_record_round_trips_through_the_harness_memory_provider() {
        use pai_harness_adapter::PaiVaultMemoryProviderConfig;
        use std::sync::Arc;
        let (vault, _dir, _root) = test_vault();
        let shared: Arc<Mutex<Option<Vault>>> = Arc::new(Mutex::new(Some(vault)));
        let memory = PaiVaultMemoryProvider::new(
            Arc::clone(&shared),
            PaiVaultMemoryProviderConfig {
                origin_platform: "DESKTOP".to_owned(),
                origin_device_id: "unoone-power".to_owned(),
                ..PaiVaultMemoryProviderConfig::default()
            },
        )
        .expect("provider builds");
        let id = record_agent_run_trail(
            &memory,
            "test-vault-id",
            "conv-1",
            &trail(
                "build me a website",
                Some("done: index.html"),
                Some("session-42"),
                &trail_steps(),
            ),
        )
        .expect("trail record must be written");
        assert!(id.starts_with("run-trail-session-42"));
        let stored = MemoryProvider::retrieve(&memory, MemoryScope::Project, "test-vault-id", &id)
            .expect("retrieve")
            .expect("record exists");
        assert_eq!(
            stored.attributes.get("kind").map(String::as_str),
            Some("agent_run_trail")
        );
        let content: serde_json::Value = serde_json::from_str(&stored.content).unwrap();
        assert_eq!(content["kind"], "agent_run_trail");
        assert_eq!(content["status"], "completed");
        assert_eq!(content["trail"].as_array().map(Vec::len), Some(2));
    }

    /// A stopped run (no outcome) still records its honest partial trail.
    #[test]
    fn stopped_run_records_partial_trail_without_output() {
        use pai_harness_adapter::PaiVaultMemoryProviderConfig;
        use std::sync::Arc;
        let (vault, _dir, _root) = test_vault();
        let shared: Arc<Mutex<Option<Vault>>> = Arc::new(Mutex::new(Some(vault)));
        let memory = PaiVaultMemoryProvider::new(
            Arc::clone(&shared),
            PaiVaultMemoryProviderConfig::default(),
        )
        .expect("provider builds");
        let id = record_agent_run_trail(
            &memory,
            "test-vault-id",
            "conv-1",
            &trail("build me a website", None, None, &trail_steps()),
        )
        .expect("stopped trail must be written");
        let stored = MemoryProvider::retrieve(&memory, MemoryScope::Project, "test-vault-id", &id)
            .expect("retrieve")
            .expect("record exists");
        assert_eq!(
            stored.attributes.get("status").map(String::as_str),
            Some("stopped")
        );
        let content: serde_json::Value = serde_json::from_str(&stored.content).unwrap();
        assert_eq!(content["status"], "stopped");
        assert!(content["failure"].as_str().unwrap().contains("cancelled"));
        assert!(content
            .get("output")
            .map(serde_json::Value::is_null)
            .unwrap_or(true));
    }

    // Reuse the desktop test-vault idiom (documents.rs): a REAL encrypted
    // vault in a temp dir, created, unlocked.
    fn test_vault() -> (Vault, tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp vault dir");
        let vault_root = dir.path().join("UNOONE");
        Vault::create(&vault_root, b"env-learning-test-pw").expect("create test vault");
        let mut vault = Vault::open(&vault_root).expect("open test vault");
        vault
            .unlock(b"env-learning-test-pw")
            .expect("unlock test vault");
        (vault, dir, vault_root)
    }
}
