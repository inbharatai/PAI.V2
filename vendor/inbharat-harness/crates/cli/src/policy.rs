use inbharat_harness_core::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use inbharat_harness_core::providers::{
    Capability, CapabilitySet, EnforcementQuality, PermissionDecision, PermissionProvider,
    SandboxGrant, SandboxProvider, SandboxRequest,
};

#[derive(Clone, Debug)]
pub(crate) struct CliPermission {
    pub(crate) granted: CapabilitySet,
    pub(crate) ask_for_side_effects: bool,
}

impl PermissionProvider for CliPermission {
    fn authorize(
        &self,
        _actor: &str,
        capability: Capability,
        _resource: &str,
    ) -> HarnessResult<PermissionDecision> {
        if !self.granted.contains(capability) {
            return Ok(PermissionDecision::Deny {
                rule_id: "cli-capability.v1".to_owned(),
                reason: "capability flag was not supplied".to_owned(),
            });
        }
        if self.ask_for_side_effects
            && matches!(capability, Capability::FileWrite | Capability::ProcessSpawn)
        {
            Ok(PermissionDecision::Ask)
        } else {
            Ok(PermissionDecision::Allow)
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CliSandbox {
    pub(crate) granted: CapabilitySet,
    pub(crate) trusted_process: bool,
}

impl SandboxProvider for CliSandbox {
    fn resolve(&self, request: &SandboxRequest) -> HarnessResult<SandboxGrant> {
        if !request.capabilities.is_subset_of(&self.granted) {
            return Err(Failure::new(
                ErrorCode::PermissionDenied,
                FailureClass::Policy,
                "cli.sandbox",
                "sandbox capability was not granted",
            ));
        }
        if request.require_security_boundary && !self.trusted_process {
            return Err(Failure::new(
                ErrorCode::SandboxUnavailable,
                FailureClass::Policy,
                "cli.sandbox",
                "process execution requires an explicit trusted sandbox provider",
            ));
        }
        Ok(SandboxGrant {
            world_id: request.world_id.clone(),
            backend: if self.trusted_process {
                "explicit-trusted-local".to_owned()
            } else {
                "rooted-fs-fence".to_owned()
            },
            quality: if self.trusted_process {
                EnforcementQuality::Partial
            } else {
                EnforcementQuality::InProcessFence
            },
            granted: self.granted.clone(),
        })
    }
}
