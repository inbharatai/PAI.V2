use crate::{admission::download_bytes, dto::*, policy::*, trust::*};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProvisioningPhase {
    Idle,
    PendingExternalIo,
    Downloading,
    Paused,
    AwaitingVerification,
    Staged,
    Loading,
    Ready,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalFailure {
    pub code: String,
    pub allocation_bytes: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProvisioningSnapshot {
    pub schema_version: u32,
    pub attempt: u64,
    pub phase: ProvisioningPhase,
    pub candidate_id: Option<String>,
    pub active_model_id: Option<String>,
    pub downloaded_bytes: u64,
    pub expected_download_bytes: u64,
    pub pause: Option<PolicyPause>,
    pub failure: Option<LocalFailure>,
}
/// This is an IO REQUEST, not proof a download/load occurred.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalAction {
    DownloadOrResume {
        attempt: u64,
        offset: u64,
    },
    VerifyStagedArtifacts {
        attempt: u64,
    },
    LoadAndSmoke {
        attempt: u64,
    },
    CancelAndRetainWorking {
        attempt: u64,
    },
    RetirePrevious {
        model_id: String,
        activation_attempt: u64,
    },
    None,
}
/// Native hashing adapter output; deliberately not deserializable. Hashes must
/// be read from staged bytes, not copied from the manifest. Signature validation
/// is a separate VerifiedCandidate gate. The host must atomically publish files.
#[derive(Clone, Debug)]
pub struct NativeArtifactMeasurement {
    pub artifact_id: String,
    pub sha256: String,
    pub installed_bytes: u64,
}
#[derive(Clone, Debug)]
struct Attempt {
    candidate: VerifiedCandidate,
    request: AdmissionRequest,
    purpose: DownloadPurpose,
}
#[derive(Clone, Debug)]
pub struct ActiveModel {
    pub candidate: VerifiedCandidate,
    pub request: AdmissionRequest,
    pub evidence_id: String,
    pub activation_attempt: u64,
}
#[derive(Clone, Debug)]
pub struct Provisioner {
    state: ProvisioningSnapshot,
    attempt: Option<Attempt>,
    active: Option<ActiveModel>,
}
impl Default for Provisioner {
    fn default() -> Self {
        Self {
            state: ProvisioningSnapshot {
                schema_version: SCHEMA_VERSION,
                attempt: 0,
                phase: ProvisioningPhase::Idle,
                candidate_id: None,
                active_model_id: None,
                downloaded_bytes: 0,
                expected_download_bytes: 0,
                pause: None,
                failure: None,
            },
            attempt: None,
            active: None,
        }
    }
}
impl Provisioner {
    pub fn snapshot(&self) -> ProvisioningSnapshot {
        self.state.clone()
    }
    pub fn active(&self) -> Option<&ActiveModel> {
        self.active.as_ref()
    }
    fn check_attempt(&self, id: u64) -> Result<&Attempt, ContractError> {
        if id != self.state.attempt {
            return Err(ContractError::StaleAttempt);
        }
        self.attempt
            .as_ref()
            .ok_or(ContractError::InvalidTransition)
    }
    fn pause(&mut self, reason: PolicyPause) {
        self.state.phase = ProvisioningPhase::Paused;
        self.state.pause = Some(reason);
    }
    pub fn begin(
        &mut self,
        c: VerifiedCandidate,
        context: &DownloadContext<'_>,
    ) -> Result<ExternalAction, ContractError> {
        if matches!(
            self.state.phase,
            ProvisioningPhase::PendingExternalIo
                | ProvisioningPhase::Downloading
                | ProvisioningPhase::AwaitingVerification
                | ProvisioningPhase::Loading
        ) {
            return Err(ContractError::InvalidTransition);
        }
        if context.purpose == DownloadPurpose::InitialGeneral && self.active.is_some() {
            return Err(ContractError::Invalid("initial already provisioned"));
        }
        if context.purpose == DownloadPurpose::Update
            && !self
                .active
                .as_ref()
                .is_some_and(|a| a.candidate.candidate().id == c.candidate().id)
        {
            return Err(ContractError::Invalid(
                "update requires matching installed active model",
            ));
        }
        if context.purpose == DownloadPurpose::NeededSpecialist
            && (self.active.is_none()
                || self.active.as_ref().is_some_and(|a| {
                    context
                        .admission
                        .request
                        .capabilities
                        .iter()
                        .all(|cap| a.candidate.candidate().capabilities.contains(cap))
                }))
        {
            return Err(ContractError::Invalid("specialist not needed"));
        }
        self.state.attempt = self
            .state
            .attempt
            .checked_add(1)
            .ok_or(ContractError::Invalid("attempt overflow"))?;
        self.state.candidate_id = Some(c.candidate().id.clone());
        self.state.downloaded_bytes = 0;
        self.state.expected_download_bytes =
            download_bytes(c.candidate()).ok_or(ContractError::Invalid("download overflow"))?;
        self.state.pause = None;
        self.state.failure = None;
        self.attempt = Some(Attempt {
            candidate: c.clone(),
            request: context.admission.request.clone(),
            purpose: context.purpose.clone(),
        });
        match context.check(&c) {
            Ok(_) => {
                self.state.phase = ProvisioningPhase::PendingExternalIo;
                Ok(ExternalAction::DownloadOrResume {
                    attempt: self.state.attempt,
                    offset: 0,
                })
            }
            Err(p) => {
                self.pause(p);
                Ok(ExternalAction::None)
            }
        }
    }
    /// Call again at resume and every transfer chunk with a refreshed local
    /// policy grant. Out-of-grant network/policy changes pause without prompting
    /// repeatedly; UI offers one explicit grant expansion/recheck action.
    pub fn resume(
        &mut self,
        id: u64,
        context: &DownloadContext<'_>,
    ) -> Result<ExternalAction, ContractError> {
        let a = self.check_attempt(id)?;
        if !matches!(
            self.state.phase,
            ProvisioningPhase::Paused
                | ProvisioningPhase::PendingExternalIo
                | ProvisioningPhase::Downloading
        ) || context.admission.request != &a.request
            || context.purpose != a.purpose
        {
            return Err(ContractError::InvalidTransition);
        }
        match context.check(&a.candidate) {
            Ok(_) => {
                self.state.pause = None;
                self.state.phase = ProvisioningPhase::PendingExternalIo;
                Ok(ExternalAction::DownloadOrResume {
                    attempt: id,
                    offset: self.state.downloaded_bytes,
                })
            }
            Err(p) => {
                self.pause(p);
                Ok(ExternalAction::None)
            }
        }
    }
    pub fn download_progress(
        &mut self,
        id: u64,
        bytes: u64,
        context: &DownloadContext<'_>,
    ) -> Result<ExternalAction, ContractError> {
        let a = self.check_attempt(id)?;
        if !matches!(
            self.state.phase,
            ProvisioningPhase::PendingExternalIo | ProvisioningPhase::Downloading
        ) || context.admission.request != &a.request
            || context.purpose != a.purpose
        {
            return Err(ContractError::InvalidTransition);
        }
        if let Err(p) = context.check(&a.candidate) {
            self.pause(p);
            return Ok(ExternalAction::None);
        }
        if bytes < self.state.downloaded_bytes || bytes > self.state.expected_download_bytes {
            return Err(ContractError::Invalid("progress"));
        }
        self.state.downloaded_bytes = bytes;
        if bytes == self.state.expected_download_bytes {
            self.state.phase = ProvisioningPhase::AwaitingVerification;
            Ok(ExternalAction::VerifyStagedArtifacts { attempt: id })
        } else {
            self.state.phase = ProvisioningPhase::Downloading;
            Ok(ExternalAction::None)
        }
    }
    pub fn interrupted(&mut self, id: u64) -> Result<(), ContractError> {
        self.check_attempt(id)?;
        if !matches!(
            self.state.phase,
            ProvisioningPhase::Downloading | ProvisioningPhase::PendingExternalIo
        ) {
            return Err(ContractError::InvalidTransition);
        }
        self.pause(PolicyPause::Network);
        Ok(())
    }
    pub fn staged_artifacts(
        &mut self,
        id: u64,
        measurements: &[NativeArtifactMeasurement],
    ) -> Result<(), ContractError> {
        let a = self.check_attempt(id)?;
        if self.state.phase != ProvisioningPhase::AwaitingVerification {
            return Err(ContractError::InvalidTransition);
        }
        let artifacts = &a.candidate.candidate().artifacts;
        let matches = measurements.len() == artifacts.len()
            && artifacts.iter().all(|expected| {
                measurements
                    .iter()
                    .filter(|m| {
                        m.artifact_id == expected.id
                            && m.sha256 == expected.sha256
                            && m.installed_bytes == expected.installed_bytes
                    })
                    .count()
                    == 1
            });
        if !matches {
            self.fail("ARTIFACT_INTEGRITY", None);
            return Err(ContractError::VerificationFailed);
        }
        self.state.phase = ProvisioningPhase::Staged;
        Ok(())
    }
    /// Offline startup needs no network/download permission. Qualification and
    /// admission remain mandatory. The native probe must measure remaining
    /// budget while previous model is still resident; do NOT count its RAM as
    /// reclaimable until replacement passes. Staged bytes remain inactive.
    pub fn start_load(
        &mut self,
        id: u64,
        context: &AdmissionContext<'_>,
    ) -> Result<ExternalAction, ContractError> {
        let a = self.check_attempt(id)?;
        if self.state.phase != ProvisioningPhase::Staged || context.request != &a.request {
            return Err(ContractError::InvalidTransition);
        }
        if !context.evaluate(&a.candidate).eligible_now {
            self.fail("LOAD_ADMISSION_RECHECK", None);
            return Ok(ExternalAction::None);
        }
        self.state.phase = ProvisioningPhase::Loading;
        Ok(ExternalAction::LoadAndSmoke { attempt: id })
    }
    pub fn finish_load(
        &mut self,
        id: u64,
        report: NativePreflightReport,
        context: &AdmissionContext<'_>,
    ) -> Result<ExternalAction, ContractError> {
        let a = self.check_attempt(id)?;
        if self.state.phase != ProvisioningPhase::Loading {
            return Err(ContractError::InvalidTransition);
        }
        if report.candidate != *a.candidate.candidate()
            || report.request != a.request
            || context.request != &a.request
            || report.probe_id != context.probe.probe_id
        {
            return Err(ContractError::WrongScope);
        }
        match &report.smoke {
            NativeSmokeResult::Oom { allocation_bytes } => {
                self.fail("LOAD_OOM", *allocation_bytes);
                return Ok(ExternalAction::None);
            }
            NativeSmokeResult::DriverFailure => {
                self.fail("DRIVER_FAILURE", None);
                return Ok(ExternalAction::None);
            }
            NativeSmokeResult::ToolFormatFailure => {
                self.fail("TOOL_FORMAT_FAILURE", None);
                return Ok(ExternalAction::None);
            }
            NativeSmokeResult::ThermalFailure => {
                self.fail("THERMAL_FAILURE", None);
                return Ok(ExternalAction::None);
            }
            NativeSmokeResult::Cancelled => {
                return self.cancel(id);
            }
            NativeSmokeResult::Passed { .. } => {}
        }
        let native = NativePreflight::from_native_report(report)?;
        if !native.matches(
            a.candidate.candidate(),
            context.probe,
            context.request,
            context.now_ms,
        ) || !context.evaluate(&a.candidate).eligible_now
        {
            self.fail("FINAL_ADMISSION_RECHECK", None);
            return Ok(ExternalAction::None);
        }
        let new_active = ActiveModel {
            candidate: a.candidate.clone(),
            request: a.request.clone(),
            evidence_id: native.evidence_id().to_owned(),
            activation_attempt: id,
        };
        let previous = self.active.replace(new_active);
        self.state.active_model_id = self.state.candidate_id.clone();
        self.state.phase = ProvisioningPhase::Ready;
        Ok(previous
            .map(|a| ExternalAction::RetirePrevious {
                model_id: a.candidate.candidate().id.clone(),
                activation_attempt: a.activation_attempt,
            })
            .unwrap_or(ExternalAction::None))
    }
    fn fail(&mut self, code: &str, allocation_bytes: Option<u64>) {
        self.state.phase = ProvisioningPhase::Failed;
        self.state.failure = Some(LocalFailure {
            code: code.to_owned(),
            allocation_bytes,
        });
    }
    pub fn cancel(&mut self, id: u64) -> Result<ExternalAction, ContractError> {
        self.check_attempt(id)?;
        if matches!(
            self.state.phase,
            ProvisioningPhase::Idle | ProvisioningPhase::Ready | ProvisioningPhase::Cancelled
        ) {
            return Err(ContractError::InvalidTransition);
        }
        self.state.phase = ProvisioningPhase::Cancelled;
        Ok(ExternalAction::CancelAndRetainWorking { attempt: id })
    }
}
