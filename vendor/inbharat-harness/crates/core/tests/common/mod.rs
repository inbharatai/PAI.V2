#![allow(dead_code)]

use inbharat_harness_core::error::HarnessResult;
use inbharat_harness_core::providers::{
    Capability, CapabilitySet, ConfirmationOutcome, ConfirmationProvider, ConfirmationRequest,
    EnforcementQuality, PermissionDecision, PermissionProvider, SandboxGrant, SandboxProvider,
    SandboxRequest,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(label: &str) -> HarnessResult<Self> {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "inbharat-test-{label}-{}-{counter}",
            std::process::id()
        ));
        let _cleanup = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).map_err(|error| {
            inbharat_harness_core::Failure::new(
                inbharat_harness_core::ErrorCode::ToolFailed,
                inbharat_harness_core::FailureClass::Execution,
                "test.tempdir",
                "cannot create temp directory",
            )
            .with_detail("io_kind", format!("{:?}", error.kind()))
        })?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.path);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AllowPermission;

impl PermissionProvider for AllowPermission {
    fn authorize(
        &self,
        _actor: &str,
        _capability: Capability,
        _resource: &str,
    ) -> HarnessResult<PermissionDecision> {
        Ok(PermissionDecision::Allow)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ConfirmYes;

impl ConfirmationProvider for ConfirmYes {
    fn confirm(&self, _request: &ConfirmationRequest) -> HarnessResult<ConfirmationOutcome> {
        Ok(ConfirmationOutcome::AllowedOnce)
    }
}

#[derive(Clone, Debug)]
pub struct GrantSandbox {
    pub granted: CapabilitySet,
    pub quality: EnforcementQuality,
}

impl SandboxProvider for GrantSandbox {
    fn resolve(&self, request: &SandboxRequest) -> HarnessResult<SandboxGrant> {
        Ok(SandboxGrant {
            world_id: request.world_id.clone(),
            backend: "test".to_owned(),
            quality: self.quality,
            granted: self.granted.clone(),
        })
    }
}
