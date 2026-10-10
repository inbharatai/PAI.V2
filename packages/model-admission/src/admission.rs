use crate::{dto::*, trust::*};

/// Inject time; do not read system clock in pure policy.
pub const MAX_PROBE_AGE_MS: u64 = 60_000;
fn sum(xs: impl IntoIterator<Item = u64>) -> Option<u64> {
    xs.into_iter().try_fold(0u64, u64::checked_add)
}
pub fn download_bytes(c: &CatalogCandidate) -> Option<u64> {
    sum(c.artifacts.iter().map(|a| a.download_bytes))
}
pub fn storage_reservation(c: &CatalogCandidate) -> Option<u64> {
    // Retain compressed/partial source AND installed replacement. Existing
    // working model is not credited as reclaimable storage.
    sum(c
        .artifacts
        .iter()
        .flat_map(|a| [a.download_bytes, a.installed_bytes])
        .chain([c.memory.disk_headroom_bytes]))
}
pub fn peak_ram(c: &CatalogCandidate, agents: u32, unified: bool) -> Option<u64> {
    let m = &c.memory;
    sum([
        m.weights_ram_bytes,
        m.projector_ram_bytes,
        m.vision_ram_bytes,
        m.kv_ram_bytes,
        m.speech_ram_bytes,
        m.runtime_ram_bytes,
        m.per_agent_ram_bytes.checked_mul(agents as u64)?,
        if unified { m.peak_vram_bytes } else { 0 },
    ])
}
fn finish(mut d: Decision, status: DecisionStatus, reason: Reason) -> Decision {
    d.status = status;
    d.reasons.push(reason);
    d
}

/// This exact function gates recommendation, acquisition and load. Its result
/// is a serializable EXPLANATION, not an authorization token. Re-evaluate at each
/// boundary with a freshly trusted probe/catalog/qualification/policy.
pub fn evaluate(
    c: &VerifiedCandidate,
    p: &DeviceProbe,
    r: &AdmissionRequest,
    qs: &[ValidatedQualification],
    preflight: Option<&NativePreflight>,
    now: u64,
) -> Decision {
    let c = c.candidate();
    let mut d = Decision {
        schema_version: SCHEMA_VERSION,
        candidate_id: c.id.clone(),
        candidate_version: c.version.clone(),
        probe_id: p.probe_id.clone(),
        status: DecisionStatus::NotYetQualified,
        reasons: vec![],
        eligible_now: false,
        context_tokens: r.context_tokens,
        backend: c.runtime.backend.clone(),
        peak_ram_bytes: None,
        peak_vram_bytes: None,
        storage_reservation_bytes: None,
        evidence_ids: vec![],
        responsiveness: None,
    };
    if p.schema_version != SCHEMA_VERSION
        || r.schema_version != SCHEMA_VERSION
        || !identifier(&p.probe_id)
        || r.parallel_agents == 0
        || r.parallel_agents > c.max_parallel_agents
        || r.capabilities.is_empty()
        || r.languages.is_empty()
    {
        return finish(d, DecisionStatus::Unsupported, Reason::InvalidInput);
    }
    if now >= c.expires_at_ms {
        return finish(d, DecisionStatus::NotYetQualified, Reason::EvidenceExpired);
    }
    if p.captured_at_ms > now || now - p.captured_at_ms > MAX_PROBE_AGE_MS {
        return finish(d, DecisionStatus::NotYetQualified, Reason::StaleProbe);
    }
    if c.licences
        .iter()
        .any(|l| l.distribution != DistributionPermission::Approved)
    {
        return finish(d, DecisionStatus::Unsupported, Reason::LicenceNotApproved);
    }
    if r.context_tokens != c.memory.context_tokens || r.kv_format != c.memory.kv_format {
        return finish(d, DecisionStatus::Unsupported, Reason::ContextMismatch);
    }
    if r.capabilities.iter().any(|x| !c.capabilities.contains(x))
        || r.languages.iter().any(|x| !c.languages.contains(x))
    {
        return finish(d, DecisionStatus::Unsupported, Reason::CapabilityMismatch);
    }
    let (Some(os), Some(os_version), Some(abi), Some(features), Some(class)) = (
        p.os.measured(),
        p.os_version.measured(),
        p.abi.measured(),
        p.cpu_features.measured(),
        p.device_class.measured(),
    ) else {
        return finish(d, DecisionStatus::NotYetQualified, Reason::UnknownProbe);
    };
    let runtime = &c.runtime;
    if os != &runtime.os
        || os_version != &runtime.os_version
        || abi != &runtime.abi
        || runtime.cpu_features.iter().any(|f| !features.contains(f))
    {
        return finish(d, DecisionStatus::Unsupported, Reason::RuntimeMismatch);
    }
    if let Some(api) = runtime.required_os_api_level {
        match p.os_api_level.measured() {
            Some(found) if *found == api => {}
            Some(_) => return finish(d, DecisionStatus::Unsupported, Reason::RuntimeMismatch),
            None => return finish(d, DecisionStatus::NotYetQualified, Reason::UnknownProbe),
        }
    }
    if !p.backends.iter().any(|b| {
        b.runtime == runtime.runtime
            && b.runtime_version == runtime.version
            && b.backend == runtime.backend
            && b.driver_version == runtime.driver_version
            && b.health == Observation::Tested(BackendHealth::LoadValidated)
    }) {
        return finish(
            d,
            DecisionStatus::NotYetQualified,
            Reason::BackendNotValidated,
        );
    }
    if c.memory.provenance.value().is_none() {
        return finish(d, DecisionStatus::NotYetQualified, Reason::Unqualified);
    }
    let (
        Some(total),
        Some(avail),
        Some(unified),
        Some(low),
        Some(native),
        Some(heap),
        Some(storage),
        Some(thermal),
    ) = (
        p.total_ram_bytes.measured(),
        p.available_ram_bytes.measured(),
        p.unified_memory.measured(),
        p.low_memory_threshold_bytes.measured(),
        p.native_budget_bytes.measured(),
        p.heap_budget_bytes.measured(),
        p.usable_storage_bytes.measured(),
        p.thermal.measured(),
    )
    else {
        return finish(d, DecisionStatus::NotYetQualified, Reason::UnknownProbe);
    };
    if *total == 0 || *avail > *total || *native == 0 || *heap == 0 || *low > *total {
        return finish(d, DecisionStatus::Unsupported, Reason::InvalidInput);
    }
    let (Some(peak), Some(disk)) = (
        peak_ram(c, r.parallel_agents, *unified),
        storage_reservation(c),
    ) else {
        return finish(d, DecisionStatus::Unsupported, Reason::ArithmeticOverflow);
    };
    d.peak_ram_bytes = Some(peak);
    d.peak_vram_bytes = Some(c.memory.peak_vram_bytes);
    d.storage_reservation_bytes = Some(disk);
    let (Some(total_need), Some(avail_need), Some(comfortable_need)) = (
        peak.checked_add(c.memory.os_reserve_bytes),
        peak.checked_add(c.memory.available_reserve_bytes.max(*low)),
        peak.checked_add(c.memory.available_reserve_bytes.max(*low))
            .and_then(|v| v.checked_add(c.memory.comfortable_headroom_bytes)),
    ) else {
        return finish(d, DecisionStatus::Unsupported, Reason::ArithmeticOverflow);
    };
    if total_need > *total || peak > *native || c.memory.heap_bytes > *heap {
        return finish(
            d,
            DecisionStatus::Unsupported,
            Reason::PermanentMemoryMisfit,
        );
    }
    let native = preflight.filter(|v| v.matches(c, p, r, now));
    let mut matching: Vec<_> = qs
        .iter()
        .map(|q| q.record())
        .filter(|q| {
            q.scope.candidate == *c
                && q.scope.device_class == *class
                && q.scope.parallel_agents == r.parallel_agents
                && q.issued_at_ms <= now
                && now < q.expires_at_ms
                && r.capabilities
                    .iter()
                    .all(|x| q.tested_capabilities.contains(x))
                && r.languages.iter().all(|x| q.tested_languages.contains(x))
        })
        .collect();
    matching.sort_by(|a, b| a.id.cmp(&b.id));
    let Some(q) = matching
        .iter()
        .copied()
        .find(|q| q.kind == QualificationKind::PhysicalDevice || native.is_some())
    else {
        return finish(
            d,
            DecisionStatus::NotYetQualified,
            if matching.is_empty() {
                Reason::Unqualified
            } else {
                Reason::PreflightRequired
            },
        );
    };
    d.evidence_ids = q.evidence_ids.clone();
    d.evidence_ids.push(q.id.clone());
    // Prefer this device's fresh tested responsiveness, otherwise retain the
    // qualification record's scoped measurement (never call it this-device TPS).
    d.responsiveness = native
        .map(|n| n.responsiveness().clone())
        .or_else(|| q.responsiveness.clone());
    if let Some(n) = native {
        d.evidence_ids.push(n.evidence_id().to_owned());
    }
    if !*unified && c.memory.peak_vram_bytes > 0 {
        let (Some(vram), Some(total_vram)) = (
            p.available_vram_bytes.measured(),
            p.total_vram_bytes.measured(),
        ) else {
            return finish(d, DecisionStatus::NotYetQualified, Reason::UnknownProbe);
        };
        if vram > total_vram {
            return finish(d, DecisionStatus::Unsupported, Reason::InvalidInput);
        }
        if c.memory.peak_vram_bytes > *total_vram {
            return finish(
                d,
                DecisionStatus::Unsupported,
                Reason::PermanentMemoryMisfit,
            );
        }
        if c.memory.peak_vram_bytes > *vram {
            return finish(
                d,
                DecisionStatus::SupportedWithLimits,
                Reason::MemoryPressure,
            );
        }
    }
    if avail_need > *avail {
        return finish(
            d,
            DecisionStatus::SupportedWithLimits,
            Reason::MemoryPressure,
        );
    }
    if disk > *storage {
        return finish(
            d,
            DecisionStatus::SupportedWithLimits,
            Reason::InsufficientStorage,
        );
    }
    if matches!(thermal, ThermalState::Throttled | ThermalState::Critical) {
        return finish(
            d,
            DecisionStatus::SupportedWithLimits,
            Reason::ThermalPressure,
        );
    }
    d.eligible_now = true;
    if *avail < comfortable_need || *thermal == ThermalState::Warm {
        return finish(
            d,
            DecisionStatus::SupportedWithLimits,
            Reason::LimitedHeadroom,
        );
    }
    if d.responsiveness.is_none() {
        return finish(
            d,
            DecisionStatus::SupportedWithLimits,
            Reason::ResponsivenessUnmeasured,
        );
    }
    finish(d, DecisionStatus::Recommended, Reason::Admitted)
}

/// One first-use general model and at most one alternative. Specialists are
/// selected later for an actual unmet capability, never pre-downloaded en masse.
#[derive(Clone, Debug)]
pub struct RankedChoices {
    pub primary: Option<String>,
    pub alternative: Option<String>,
}
pub fn rank_choices(
    candidates: &[VerifiedCandidate],
    decisions: &[Decision],
    role: ModelRole,
) -> RankedChoices {
    let mut eligible: Vec<_> = candidates
        .iter()
        .filter(|c| c.candidate().role == role)
        .filter_map(|c| {
            decisions
                .iter()
                .find(|d| {
                    d.candidate_id == c.candidate().id
                        && d.candidate_version == c.candidate().version
                        && d.eligible_now
                })
                .map(|d| (c.candidate(), d))
        })
        .collect();
    eligible.sort_by(|(a, da), (b, db)| {
        let key = |d: &Decision| {
            (
                d.status != DecisionStatus::Recommended,
                d.responsiveness
                    .as_ref()
                    .map(|r| r.first_token_p95_ms)
                    .unwrap_or(u64::MAX),
                d.responsiveness
                    .as_ref()
                    .map(|r| r.cold_load_ms)
                    .unwrap_or(u64::MAX),
            )
        };
        key(da)
            .cmp(&key(db))
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.version.cmp(&b.version))
    });
    RankedChoices {
        primary: eligible.first().map(|(c, _)| c.id.clone()),
        alternative: eligible.get(1).map(|(c, _)| c.id.clone()),
    }
}
