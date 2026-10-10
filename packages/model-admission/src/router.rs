use crate::{dto::*, lifecycle::NativeArtifactMeasurement, policy::*, trust::*};

/// Native model-store inventory, not a catalog's self-asserted installed flag.
/// Rehash/version-check through the host adapter when files or store change.
#[derive(Clone, Debug)]
pub struct NativeInstalledModel {
    candidate: VerifiedCandidate,
}
impl NativeInstalledModel {
    pub fn from_native_inventory(
        candidate: VerifiedCandidate,
        measurements: &[NativeArtifactMeasurement],
    ) -> Result<Self, ContractError> {
        let artifacts = &candidate.candidate().artifacts;
        if artifacts.len() != measurements.len()
            || !artifacts.iter().all(|a| {
                measurements
                    .iter()
                    .filter(|m| {
                        m.artifact_id == a.id
                            && m.sha256 == a.sha256
                            && m.installed_bytes == a.installed_bytes
                    })
                    .count()
                    == 1
            })
        {
            return Err(ContractError::VerificationFailed);
        }
        Ok(Self { candidate })
    }
    pub fn candidate(&self) -> &VerifiedCandidate {
        &self.candidate
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoutePlan {
    /// This is a request for a native lease/load, not a completed acquisition.
    LeaseInstalled {
        model_id: String,
        version: String,
    },
    ProvisionWithinGrant {
        model_id: String,
        version: String,
    },
    QueueUntilLeaseReleased,
    PauseOutsideGrant(PolicyPause),
    NoQualifiedLocalModel,
}
/// Conservative v1: exactly one model lease at a time. It serializes concurrent
/// tasks instead of speculatively promising multiple resident model capacity.
/// Caller owns cancellation, actual mutex/lease, task retention and loading.
/// Specialization is requested only for the current request's verified needs.
/// Initial setup MUST pass InitialGeneral; later missing specialist requests
/// MUST pass NeededSpecialist. Installed models do not need network permission.
pub fn route_local(
    installed: &[NativeInstalledModel],
    catalog: &[VerifiedCandidate],
    context: &DownloadContext<'_>,
    native_lease_occupied: bool,
) -> RoutePlan {
    if native_lease_occupied {
        return RoutePlan::QueueUntilLeaseReleased;
    }
    let mut local: Vec<_> = installed
        .iter()
        .filter_map(|m| {
            let d = context.admission.evaluate(&m.candidate);
            d.eligible_now.then_some((m, d))
        })
        .collect();
    local.sort_by_key(|(m, d)| {
        (
            d.responsiveness
                .as_ref()
                .map(|r| r.first_token_p95_ms)
                .unwrap_or(u64::MAX),
            m.candidate.candidate().id.clone(),
            m.candidate.candidate().version.clone(),
        )
    });
    if let Some((m, _)) = local.first() {
        let c = m.candidate.candidate();
        return RoutePlan::LeaseInstalled {
            model_id: c.id.clone(),
            version: c.version.clone(),
        };
    }
    let mut missing: Vec<_> = catalog
        .iter()
        .filter(|c| {
            !installed
                .iter()
                .any(|m| m.candidate.candidate() == c.candidate())
        })
        .filter(|c| match context.purpose {
            DownloadPurpose::InitialGeneral => c.candidate().role == ModelRole::General,
            DownloadPurpose::NeededSpecialist => c.candidate().role == ModelRole::Specialist,
            _ => true,
        })
        .filter_map(|c| {
            let d = context.admission.evaluate(c);
            d.eligible_now.then_some((c, d))
        })
        .collect();
    missing.sort_by_key(|(c, d)| {
        (
            d.responsiveness
                .as_ref()
                .map(|r| r.first_token_p95_ms)
                .unwrap_or(u64::MAX),
            c.candidate().id.clone(),
            c.candidate().version.clone(),
        )
    });
    let mut pause = None;
    for (c, _) in missing {
        match context.check(c) {
            Ok(_) => {
                return RoutePlan::ProvisionWithinGrant {
                    model_id: c.candidate().id.clone(),
                    version: c.candidate().version.clone(),
                }
            }
            Err(reason) => {
                if pause.is_none() {
                    pause = Some(reason);
                }
            }
        }
    }
    pause
        .map(RoutePlan::PauseOutsideGrant)
        .unwrap_or(RoutePlan::NoQualifiedLocalModel)
}
