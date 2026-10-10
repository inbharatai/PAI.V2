use serde_json::{json, Value};
use unoone_model_admission::*;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/admission-v1.json")).unwrap()
}
/// Synthetic verifier binds exact test payload + domain; it is NOT cryptography
/// and is never exported by the production crate. No key material is generated.
struct FixtureVerifier {
    bytes: Vec<u8>,
    domain: SignedDomain,
}
impl SignatureVerifier for FixtureVerifier {
    fn verify(&self, d: SignedDomain, b: &[u8], a: &Attestation) -> Result<(), ContractError> {
        if b == self.bytes && d == self.domain && a.signature == "SYNTHETIC-TEST-NOT-A-SIGNATURE" {
            Ok(())
        } else {
            Err(ContractError::VerificationFailed)
        }
    }
}
fn attestation() -> Attestation {
    Attestation {
        key_id: "synthetic-test".into(),
        algorithm: "Ed25519".into(),
        signature: "SYNTHETIC-TEST-NOT-A-SIGNATURE".into(),
    }
}
fn verified(v: &Value) -> VerifiedCandidate {
    let bytes = serde_json::to_vec(v).unwrap();
    verify_candidate(
        &bytes,
        &attestation(),
        &FixtureVerifier {
            bytes: bytes.clone(),
            domain: SignedDomain::CatalogCandidateV1,
        },
    )
    .unwrap()
}
fn qualified(v: &Value, c: &VerifiedCandidate) -> Result<ValidatedQualification, ContractError> {
    let bytes = serde_json::to_vec(v).unwrap();
    validate_qualification(
        &bytes,
        &attestation(),
        &FixtureVerifier {
            bytes: bytes.clone(),
            domain: SignedDomain::QualificationRecordV1,
        },
        c,
    )
}
struct FixtureAuthority(StandingDownloadPolicy);
impl StandingPolicyAuthority for FixtureAuthority {
    fn validate_local_approval(&self, p: &StandingDownloadPolicy) -> Result<(), ContractError> {
        if p == &self.0 {
            Ok(())
        } else {
            Err(ContractError::VerificationFailed)
        }
    }
}
struct Inputs {
    c: VerifiedCandidate,
    p: DeviceProbe,
    r: AdmissionRequest,
    q: Vec<ValidatedQualification>,
    grant: NativePolicyGrant,
    network: NetworkState,
    perf: Responsiveness,
    now: u64,
}
impl Inputs {
    fn from(mut v: Value) -> Self {
        v["qualification"]["scope"]["candidate"] = v["candidate"].clone();
        let c = verified(&v["candidate"]);
        let q = vec![qualified(&v["qualification"], &c).unwrap()];
        let p: StandingDownloadPolicy = serde_json::from_value(v["policy"].clone()).unwrap();
        Self {
            c,
            p: serde_json::from_value(v["probe"].clone()).unwrap(),
            r: serde_json::from_value(v["request"].clone()).unwrap(),
            q,
            grant: NativePolicyGrant::from_local_store(p.clone(), &FixtureAuthority(p)).unwrap(),
            network: NetworkState {
                kind: NetworkKind::Wifi,
                metered: Observation::Detected(false),
            },
            perf: serde_json::from_value(v["responsiveness"].clone()).unwrap(),
            now: v["now_ms"].as_u64().unwrap(),
        }
    }
    fn admission(&self) -> AdmissionContext<'_> {
        AdmissionContext {
            probe: &self.p,
            request: &self.r,
            qualifications: &self.q,
            preflight: None,
            now_ms: self.now,
        }
    }
    fn download(&self, purpose: DownloadPurpose) -> DownloadContext<'_> {
        DownloadContext {
            admission: self.admission(),
            grant: Some(&self.grant),
            purpose,
            network: &self.network,
            current_model_store_bytes: 0,
        }
    }
    fn report(&self, smoke: NativeSmokeResult) -> NativePreflightReport {
        NativePreflightReport {
            candidate: self.c.candidate().clone(),
            request: self.r.clone(),
            probe_id: self.p.probe_id.clone(),
            captured_at_ms: self.now,
            expires_at_ms: self.now + 10000,
            evidence_id: "synthetic-native-smoke".into(),
            smoke,
        }
    }
    fn passed(&self) -> NativeSmokeResult {
        NativeSmokeResult::Passed {
            responsiveness: self.perf.clone(),
        }
    }
    fn staged(&self, m: &mut Provisioner, purpose: DownloadPurpose) -> u64 {
        assert!(matches!(
            m.begin(self.c.clone(), &self.download(purpose.clone()))
                .unwrap(),
            ExternalAction::DownloadOrResume { .. }
        ));
        let id = m.snapshot().attempt;
        assert_eq!(m.snapshot().phase, ProvisioningPhase::PendingExternalIo);
        let total = download_bytes(self.c.candidate()).unwrap();
        assert!(matches!(
            m.download_progress(id, total, &self.download(purpose))
                .unwrap(),
            ExternalAction::VerifyStagedArtifacts { .. }
        ));
        let measurements = self
            .c
            .candidate()
            .artifacts
            .iter()
            .map(|a| NativeArtifactMeasurement {
                artifact_id: a.id.clone(),
                sha256: a.sha256.clone(),
                installed_bytes: a.installed_bytes,
            })
            .collect::<Vec<_>>();
        m.staged_artifacts(id, &measurements).unwrap();
        id
    }
    fn activate(&self, m: &mut Provisioner) {
        let id = self.staged(m, DownloadPurpose::InitialGeneral);
        m.start_load(id, &self.admission()).unwrap();
        m.finish_load(id, self.report(self.passed()), &self.admission())
            .unwrap();
        assert_eq!(m.snapshot().phase, ProvisioningPhase::Ready);
    }
}
fn input() -> Inputs {
    Inputs::from(fixture())
}
fn replace(root: &mut Value, path: &str, value: Value) {
    let pointer = format!("/{}", path.replace('.', "/"));
    *root.pointer_mut(&pointer).unwrap() = value;
}

#[test]
fn shared_golden_vectors() {
    let base = fixture();
    for case in base["cases"].as_array().unwrap() {
        let mut v = base.clone();
        for (path, value) in case["changes"].as_object().unwrap() {
            replace(&mut v, path, value.clone());
        }
        let i = Inputs::from(v);
        let d = i.admission().evaluate(&i.c);
        assert_eq!(
            serde_json::to_value(&d.status).unwrap(),
            case["status"],
            "{}",
            case["name"]
        );
        assert_eq!(
            d.eligible_now,
            case["eligible"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        assert_eq!(
            serde_json::to_value(d.reasons.last().unwrap()).unwrap(),
            case["reason"],
            "{}",
            case["name"]
        );
    }
}
#[test]
fn exact_reported_kv_allocation_and_projector_are_budgeted_for_text() {
    let i = input();
    let m = &i.c.candidate().memory;
    assert_eq!(m.kv_ram_bytes, 1_879_048_192);
    let peak = peak_ram(i.c.candidate(), 1, false).unwrap();
    assert_eq!(
        peak,
        m.weights_ram_bytes
            + m.projector_ram_bytes
            + m.vision_ram_bytes
            + 1_879_048_192
            + m.speech_ram_bytes
            + m.runtime_ram_bytes
            + m.per_agent_ram_bytes
    );
    assert!(!i.r.capabilities.contains(&Capability::Vision));
    assert!(m.projector_ram_bytes > 0);
}
#[test]
fn tampered_id_or_hash_cannot_reuse_signed_payload() {
    let v = fixture();
    let bytes = serde_json::to_vec(&v["candidate"]).unwrap();
    let verifier = FixtureVerifier {
        bytes: bytes.clone(),
        domain: SignedDomain::CatalogCandidateV1,
    };
    for key in ["id", "version"] {
        let mut tampered = v["candidate"].clone();
        tampered[key] = json!("tampered");
        assert!(verify_candidate(
            &serde_json::to_vec(&tampered).unwrap(),
            &attestation(),
            &verifier
        )
        .is_err());
    }
    let mut tampered = v["candidate"].clone();
    tampered["artifacts"][0]["sha256"] = json!("e".repeat(64));
    assert!(verify_candidate(
        &serde_json::to_vec(&tampered).unwrap(),
        &attestation(),
        &verifier
    )
    .is_err());
}
#[test]
fn qualification_is_exact_scope_not_generic_verified_bool() {
    let mut v = fixture();
    let original = verified(&v["candidate"]);
    v["candidate"]["memory"]["context_tokens"] = json!(4096);
    let changed = verified(&v["candidate"]);
    assert!(qualified(&v["qualification"], &changed).is_err());
    v["qualification"]["workflowVerified"] = json!(true);
    assert!(qualified(&v["qualification"], &original).is_err());
    v["probe"]["workflow_verified"] = json!(true);
    assert!(decode::<DeviceProbe>(&serde_json::to_vec(&v["probe"]).unwrap()).is_err());
}
#[test]
fn native_preflight_never_substitutes_for_qualification() {
    let i = input();
    let native = NativePreflight::from_native_report(i.report(i.passed())).unwrap();
    let d = evaluate(&i.c, &i.p, &i.r, &[], Some(&native), i.now);
    assert!(!d.eligible_now);
    assert_eq!(d.status, DecisionStatus::NotYetQualified);
}
#[test]
fn validated_generic_needs_matching_native_preflight() {
    let mut v = fixture();
    v["qualification"]["kind"] = json!("VALIDATED_GENERIC_PROFILE");
    let i = Inputs::from(v);
    assert!(!i.admission().evaluate(&i.c).eligible_now);
    let native = NativePreflight::from_native_report(i.report(i.passed())).unwrap();
    assert!(evaluate(&i.c, &i.p, &i.r, &i.q, Some(&native), i.now).eligible_now);
    let mut report = i.report(i.passed());
    report.probe_id = "other-probe".into();
    let wrong = NativePreflight::from_native_report(report).unwrap();
    assert!(!evaluate(&i.c, &i.p, &i.r, &i.q, Some(&wrong), i.now).eligible_now);
}
#[test]
fn stale_or_future_probe_blocks_and_expired_evidence_does_not_admit() {
    let mut i = input();
    i.p.captured_at_ms = 1;
    assert_eq!(i.admission().evaluate(&i.c).reasons, [Reason::StaleProbe]);
    i.p.captured_at_ms = i.now + 1;
    assert!(!i.admission().evaluate(&i.c).eligible_now);
    i.p.captured_at_ms = 1000000;
    i.now = 1000000;
    assert_eq!(
        i.admission().evaluate(&i.c).reasons,
        [Reason::EvidenceExpired]
    );
}
#[test]
fn unknown_and_estimated_vram_are_not_credited() {
    let mut v = fixture();
    v["candidate"]["memory"]["peak_vram_bytes"] = json!(1000);
    let mut i = Inputs::from(v);
    assert_eq!(i.admission().evaluate(&i.c).reasons, [Reason::UnknownProbe]);
    i.p.available_vram_bytes = Observation::Estimated(8 * 1024 * 1024 * 1024);
    assert!(!i.admission().evaluate(&i.c).eligible_now);
    i.p.available_vram_bytes = Observation::Detected(1000);
    i.p.total_vram_bytes = Observation::Detected(2000);
    assert!(i.admission().evaluate(&i.c).eligible_now);
}
#[test]
fn unified_memory_never_adds_vram_to_ram_capacity() {
    let mut v = fixture();
    v["candidate"]["memory"]["peak_vram_bytes"] = json!(12u64 * 1024 * 1024 * 1024);
    let mut i = Inputs::from(v);
    i.p.unified_memory = Observation::Detected(true);
    i.p.available_vram_bytes = Observation::Detected(u64::MAX);
    assert_eq!(
        i.admission().evaluate(&i.c).reasons,
        [Reason::PermanentMemoryMisfit]
    );
}
#[test]
fn standing_policy_reuses_grant_without_prompt_and_pauses_outside_scope() {
    let mut i = input();
    let check = |i: &Inputs| {
        check_download_policy(
            &i.c,
            Some(&i.grant),
            &DownloadPurpose::InitialGeneral,
            &i.network,
            0,
            i.now,
        )
    };
    assert_eq!(check(&i), PolicyDecision::AllowedWithinGrant);
    assert_eq!(check(&i), PolicyDecision::AllowedWithinGrant);
    i.network.metered = Observation::Detected(true);
    assert_eq!(check(&i), PolicyDecision::Pause(PolicyPause::Metered));
    i.network.metered = Observation::Unknown;
    assert_eq!(check(&i), PolicyDecision::Pause(PolicyPause::Metered));
    i.network.kind = NetworkKind::Cellular;
    assert_eq!(check(&i), PolicyDecision::Pause(PolicyPause::Network));
}
#[test]
fn expired_revoked_or_missing_grants_do_not_download() {
    let i = input();
    assert_eq!(
        check_download_policy(&i.c, None, &DownloadPurpose::Recovery, &i.network, 0, i.now),
        PolicyDecision::Pause(PolicyPause::MissingGrant)
    );
    assert_eq!(
        check_download_policy(
            &i.c,
            Some(&i.grant),
            &DownloadPurpose::Recovery,
            &i.network,
            0,
            i.grant.policy().expires_at_ms
        ),
        PolicyDecision::Pause(PolicyPause::Expired)
    );
    let mut changed = i.grant.policy().clone();
    changed.allow_metered = true;
    assert!(NativePolicyGrant::from_local_store(
        changed,
        &FixtureAuthority(i.grant.policy().clone())
    )
    .is_err());
}
#[test]
fn policy_storage_overflow_model_and_licence_rules_fail_closed() {
    let i = input();
    assert_eq!(
        check_download_policy(
            &i.c,
            Some(&i.grant),
            &DownloadPurpose::Recovery,
            &i.network,
            u64::MAX,
            i.now
        ),
        PolicyDecision::Pause(PolicyPause::Overflow)
    );
    for (field, value, reason) in [
        ("accepted_licences", json!([]), PolicyPause::Licence),
        ("max_model_store_bytes", json!(1), PolicyPause::StorageCap),
        ("allow_recovery", json!(false), PolicyPause::Purpose),
    ] {
        let mut v = fixture();
        v["policy"][field] = value;
        let i = Inputs::from(v);
        assert_eq!(
            check_download_policy(
                &i.c,
                Some(&i.grant),
                &DownloadPurpose::Recovery,
                &i.network,
                0,
                i.now
            ),
            PolicyDecision::Pause(reason)
        );
    }
    let mut v = fixture();
    v["policy"]["model_rules"][0]["model_id"] = json!("different");
    let i = Inputs::from(v);
    assert_eq!(
        check_download_policy(
            &i.c,
            Some(&i.grant),
            &DownloadPurpose::Recovery,
            &i.network,
            0,
            i.now
        ),
        PolicyDecision::Pause(PolicyPause::ModelRule)
    );
}
#[test]
fn interrupted_download_never_active_and_resume_retains_offset() {
    let i = input();
    let mut m = Provisioner::default();
    let ctx = i.download(DownloadPurpose::InitialGeneral);
    m.begin(i.c.clone(), &ctx).unwrap();
    let id = m.snapshot().attempt;
    m.download_progress(id, 123, &ctx).unwrap();
    m.interrupted(id).unwrap();
    assert!(m.active().is_none());
    assert_eq!(m.snapshot().phase, ProvisioningPhase::Paused);
    assert_eq!(
        m.resume(id, &ctx).unwrap(),
        ExternalAction::DownloadOrResume {
            attempt: id,
            offset: 123
        }
    );
}
#[test]
fn update_oom_preserves_working_model_and_is_failed_not_ready() {
    let i = input();
    let mut m = Provisioner::default();
    i.activate(&mut m);
    let old = m.active().unwrap().candidate.candidate().clone();
    let id = i.staged(&mut m, DownloadPurpose::Update);
    m.start_load(id, &i.admission()).unwrap();
    m.finish_load(
        id,
        i.report(NativeSmokeResult::Oom {
            allocation_bytes: Some(1_879_048_192),
        }),
        &i.admission(),
    )
    .unwrap();
    assert_eq!(m.snapshot().phase, ProvisioningPhase::Failed);
    assert_eq!(
        m.snapshot().failure.unwrap().allocation_bytes,
        Some(1_879_048_192)
    );
    assert_eq!(m.active().unwrap().candidate.candidate(), &old);
}
#[test]
fn cancellation_retry_ignores_late_callbacks_and_preserves_active() {
    let i = input();
    let mut m = Provisioner::default();
    i.activate(&mut m);
    let ctx = i.download(DownloadPurpose::Update);
    m.begin(i.c.clone(), &ctx).unwrap();
    let first = m.snapshot().attempt;
    m.cancel(first).unwrap();
    assert!(m.active().is_some());
    m.begin(i.c.clone(), &ctx).unwrap();
    let second = m.snapshot().attempt;
    assert!(second > first);
    assert_eq!(
        m.download_progress(first, 0, &ctx),
        Err(ContractError::StaleAttempt)
    );
    assert!(m.active().is_some());
    assert_eq!(m.snapshot().phase, ProvisioningPhase::PendingExternalIo);
}
#[test]
fn corrupt_or_incomplete_artifacts_never_load() {
    let i = input();
    let mut m = Provisioner::default();
    let ctx = i.download(DownloadPurpose::InitialGeneral);
    m.begin(i.c.clone(), &ctx).unwrap();
    let id = m.snapshot().attempt;
    assert!(m.start_load(id, &i.admission()).is_err());
    m.download_progress(id, download_bytes(i.c.candidate()).unwrap(), &ctx)
        .unwrap();
    assert!(m.staged_artifacts(id, &[]).is_err());
    assert!(m.active().is_none());
    assert_eq!(m.snapshot().phase, ProvisioningPhase::Failed);
}
#[test]
fn predownload_and_startload_share_pressure_decision() {
    let mut i = input();
    let mut m = Provisioner::default();
    let id = i.staged(&mut m, DownloadPurpose::InitialGeneral);
    i.p.available_ram_bytes = Observation::Detected(1);
    assert!(!i.admission().evaluate(&i.c).eligible_now);
    assert_eq!(
        m.start_load(id, &i.admission()).unwrap(),
        ExternalAction::None
    );
    assert_eq!(m.snapshot().phase, ProvisioningPhase::Failed);
    assert!(m.active().is_none());
}
#[test]
fn ready_requires_successful_native_smoke_not_download_verification() {
    let i = input();
    let mut m = Provisioner::default();
    let id = i.staged(&mut m, DownloadPurpose::InitialGeneral);
    assert!(m.active().is_none());
    assert_eq!(m.snapshot().phase, ProvisioningPhase::Staged);
    assert!(m
        .finish_load(id, i.report(i.passed()), &i.admission())
        .is_err());
    m.start_load(id, &i.admission()).unwrap();
    m.finish_load(
        id,
        i.report(NativeSmokeResult::ToolFormatFailure),
        &i.admission(),
    )
    .unwrap();
    assert!(m.active().is_none());
    assert_eq!(m.snapshot().phase, ProvisioningPhase::Failed);
}
#[test]
fn offline_after_acquisition_load_does_not_require_network() {
    let mut i = input();
    let mut m = Provisioner::default();
    let id = i.staged(&mut m, DownloadPurpose::InitialGeneral);
    i.network.kind = NetworkKind::Offline;
    m.start_load(id, &i.admission()).unwrap();
    m.finish_load(id, i.report(i.passed()), &i.admission())
        .unwrap();
    assert_eq!(m.snapshot().phase, ProvisioningPhase::Ready);
}
#[test]
fn recovery_obeys_same_policy_no_cloud_or_unqualified_fallback() {
    let i = input();
    let candidates = vec![i.c.clone()];
    assert!(recovery_candidate(&candidates, &i.download(DownloadPurpose::Recovery)).is_some());
    assert!(oom_recovery_candidate(
        &candidates,
        i.c.candidate(),
        &i.download(DownloadPurpose::Recovery)
    )
    .is_none());
    let mut prior = i.c.candidate().clone();
    prior.memory.kv_ram_bytes += 1;
    assert!(
        oom_recovery_candidate(&candidates, &prior, &i.download(DownloadPurpose::Recovery))
            .is_some()
    );
    let mut ctx = i.download(DownloadPurpose::Recovery);
    ctx.grant = None;
    assert!(recovery_candidate(&candidates, &ctx).is_none());
    ctx.grant = Some(&i.grant);
    ctx.admission.qualifications = &[];
    assert!(recovery_candidate(&candidates, &ctx).is_none());
}
#[test]
fn ranking_uses_measured_responsiveness_not_size_or_label() {
    let first = input();
    let mut v = fixture();
    v["candidate"]["id"] = json!("12b-marketing-label");
    v["qualification"]["responsiveness"]["first_token_p95_ms"] = json!(9000);
    let second = Inputs::from(v);
    let ds = vec![
        first.admission().evaluate(&first.c),
        second.admission().evaluate(&second.c),
    ];
    let choices = rank_choices(&[second.c, first.c], &ds, ModelRole::General);
    assert_eq!(choices.primary.as_deref(), Some("synthetic-general"));
    assert_eq!(choices.alternative.as_deref(), Some("12b-marketing-label"));
}
#[test]
fn no_fabricated_tps_when_responsiveness_unmeasured() {
    let mut v = fixture();
    v["qualification"]["responsiveness"] = Value::Null;
    let i = Inputs::from(v);
    let d = i.admission().evaluate(&i.c);
    assert!(d.responsiveness.is_none());
    assert!(d.eligible_now);
    assert_eq!(d.status, DecisionStatus::SupportedWithLimits);
}
#[test]
fn zero_negative_unknown_fields_and_bad_versions_rejected() {
    let mut v = fixture();
    v["probe"]["available_ram_bytes"]["value"] = json!(-1);
    assert!(decode::<DeviceProbe>(&serde_json::to_vec(&v["probe"]).unwrap()).is_err());
    let mut i = input();
    i.p.schema_version = 2;
    assert!(!i.admission().evaluate(&i.c).eligible_now);
    i.p.schema_version = 1;
    i.p.total_ram_bytes = Observation::Detected(0);
    assert!(!i.admission().evaluate(&i.c).eligible_now);
    assert!(decode::<DeviceProbe>(&vec![b' '; MAX_WIRE_BYTES + 1]).is_err());
}
#[test]
fn policy_expiry_or_revocation_during_download_pauses_not_ready() {
    let i = input();
    let mut m = Provisioner::default();
    let mut ctx = i.download(DownloadPurpose::InitialGeneral);
    m.begin(i.c.clone(), &ctx).unwrap();
    let id = m.snapshot().attempt;
    ctx.grant = None;
    assert_eq!(
        m.download_progress(id, 5, &ctx).unwrap(),
        ExternalAction::None
    );
    assert_eq!(m.snapshot().pause, Some(PolicyPause::MissingGrant));
    assert!(m.active().is_none());
}
#[test]
fn failed_native_preflight_cannot_be_minted_as_success() {
    let i = input();
    assert!(
        NativePreflight::from_native_report(i.report(NativeSmokeResult::Oom {
            allocation_bytes: Some(1_879_048_192)
        }))
        .is_err()
    );
}
#[test]
fn signed_failed_check_is_not_qualification() {
    let mut v = fixture();
    let c = verified(&v["candidate"]);
    for outcome in ["FAIL", "UNKNOWN"] {
        v["qualification"]["checks"][1]["outcome"] = json!(outcome);
        assert!(qualified(&v["qualification"], &c).is_err());
    }
}
#[test]
fn pressure_without_qualification_never_claims_supported() {
    let mut i = input();
    i.p.available_ram_bytes = Observation::Detected(1);
    i.q.clear();
    assert_eq!(
        i.admission().evaluate(&i.c).status,
        DecisionStatus::NotYetQualified
    );
}
#[test]
fn update_cannot_bypass_initial_opt_out() {
    let i = input();
    let mut m = Provisioner::default();
    assert!(m
        .begin(i.c.clone(), &i.download(DownloadPurpose::Update))
        .is_err());
}
#[test]
fn same_model_update_retires_previous_instance_not_new_instance() {
    let i = input();
    let mut m = Provisioner::default();
    i.activate(&mut m);
    let previous = m.active().unwrap().activation_attempt;
    let id = i.staged(&mut m, DownloadPurpose::Update);
    m.start_load(id, &i.admission()).unwrap();
    assert_eq!(
        m.finish_load(id, i.report(i.passed()), &i.admission())
            .unwrap(),
        ExternalAction::RetirePrevious {
            model_id: i.c.candidate().id.clone(),
            activation_attempt: previous
        }
    );
    assert_eq!(m.active().unwrap().activation_attempt, id);
    assert_ne!(id, previous);
}
#[test]
fn router_uses_verified_installed_offline_and_serializes_leases() {
    let mut i = input();
    let c = i.c.candidate();
    let measurements: Vec<_> = c
        .artifacts
        .iter()
        .map(|a| NativeArtifactMeasurement {
            artifact_id: a.id.clone(),
            sha256: a.sha256.clone(),
            installed_bytes: a.installed_bytes,
        })
        .collect();
    let installed =
        vec![NativeInstalledModel::from_native_inventory(i.c.clone(), &measurements).unwrap()];
    i.network.kind = NetworkKind::Offline;
    let ctx = i.download(DownloadPurpose::InitialGeneral);
    assert!(matches!(
        route_local(&installed, &[], &ctx, false),
        RoutePlan::LeaseInstalled { .. }
    ));
    assert_eq!(
        route_local(&installed, &[], &ctx, true),
        RoutePlan::QueueUntilLeaseReleased
    );
    assert!(NativeInstalledModel::from_native_inventory(i.c.clone(), &[]).is_err());
}
#[test]
fn router_provisions_only_in_scope_and_no_cloud_fallback() {
    let mut i = input();
    let catalog = vec![i.c.clone()];
    assert!(matches!(
        route_local(
            &[],
            &catalog,
            &i.download(DownloadPurpose::InitialGeneral),
            false
        ),
        RoutePlan::ProvisionWithinGrant { .. }
    ));
    let mut ctx = i.download(DownloadPurpose::InitialGeneral);
    ctx.grant = None;
    assert_eq!(
        route_local(&[], &catalog, &ctx, false),
        RoutePlan::PauseOutsideGrant(PolicyPause::MissingGrant)
    );
    i.q.clear();
    assert_eq!(
        route_local(
            &[],
            &catalog,
            &i.download(DownloadPurpose::InitialGeneral),
            false
        ),
        RoutePlan::NoQualifiedLocalModel
    );
}
#[test]
fn vram_permanent_misfit_and_android_api_scope() {
    let mut v = fixture();
    v["candidate"]["memory"]["peak_vram_bytes"] = json!(1000);
    let mut i = Inputs::from(v);
    i.p.total_vram_bytes = Observation::Detected(500);
    i.p.available_vram_bytes = Observation::Detected(500);
    assert_eq!(
        i.admission().evaluate(&i.c).reasons,
        [Reason::PermanentMemoryMisfit]
    );
    let mut v = fixture();
    v["candidate"]["runtime"]["required_os_api_level"] = json!(35);
    let mut i = Inputs::from(v);
    assert_eq!(i.admission().evaluate(&i.c).reasons, [Reason::UnknownProbe]);
    i.p.os_api_level = Observation::Detected(34);
    assert_eq!(
        i.admission().evaluate(&i.c).reasons,
        [Reason::RuntimeMismatch]
    );
    i.p.os_api_level = Observation::Detected(35);
    assert!(i.admission().evaluate(&i.c).eligible_now);
}

#[test]
fn different_version_is_not_active_until_native_success() {
    let original = input();
    let mut m = Provisioner::default();
    original.activate(&mut m);
    let mut v = fixture();
    v["candidate"]["version"] = json!("2");
    v["candidate"]["artifacts"][0]["sha256"] = json!("f".repeat(64));
    let replacement = Inputs::from(v);
    let id = replacement.staged(&mut m, DownloadPurpose::Update);
    assert_eq!(m.active().unwrap().candidate.candidate().version, "1");
    m.start_load(id, &replacement.admission()).unwrap();
    assert_eq!(m.active().unwrap().candidate.candidate().version, "1");
    m.finish_load(
        id,
        replacement.report(replacement.passed()),
        &replacement.admission(),
    )
    .unwrap();
    assert_eq!(m.active().unwrap().candidate.candidate().version, "2");
}
