//! Deterministic execution-level routing and monotonic escalation.

use crate::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use crate::providers::{Capability, CapabilitySet};

/// Explicit execution levels. Ordering is monotonic by authority and cost.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(u8)]
pub enum ExecutionLevel {
    L0 = 0,
    L1 = 1,
    L2 = 2,
    L3 = 3,
}

impl ExecutionLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::L0 => "L0",
            Self::L1 => "L1",
            Self::L2 => "L2",
            Self::L3 => "L3",
        }
    }

    /// Parses exact case-insensitive level names.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "l0" | "0" => Some(Self::L0),
            "l1" | "1" => Some(Self::L1),
            "l2" | "2" => Some(Self::L2),
            "l3" | "3" => Some(Self::L3),
            _ => None,
        }
    }

    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self {
            Self::L0 => Some(Self::L1),
            Self::L1 => Some(Self::L2),
            Self::L2 => Some(Self::L3),
            Self::L3 => None,
        }
    }
}

/// Auditable reason for a routing decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteReason {
    ExplicitLevel,
    DirectConversation,
    DeterministicSingleAction,
    BoundedMultiStepTask,
    GoalWorkspaceTask,
}

impl RouteReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitLevel => "explicit_level",
            Self::DirectConversation => "direct_conversation",
            Self::DeterministicSingleAction => "deterministic_single_action",
            Self::BoundedMultiStepTask => "bounded_multi_step_task",
            Self::GoalWorkspaceTask => "goal_workspace_task",
        }
    }
}

/// Inputs to the pure router.
#[derive(Clone, Debug)]
pub struct RouteRequest<'a> {
    pub prompt: &'a str,
    pub explicit_level: Option<ExecutionLevel>,
    pub available_capabilities: CapabilitySet,
    pub attachment_count: usize,
}

impl<'a> RouteRequest<'a> {
    #[must_use]
    pub fn new(prompt: &'a str) -> Self {
        Self {
            prompt,
            explicit_level: None,
            available_capabilities: CapabilitySet::all_local(),
            attachment_count: 0,
        }
    }
}

/// Immutable router result, decided before agent initialization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteDecision {
    pub level: ExecutionLevel,
    pub reason: RouteReason,
    pub confidence_basis_points: u16,
    pub required_capabilities: CapabilitySet,
    pub confirmation_required: bool,
    pub rule_id: &'static str,
}

/// Deployment policy restricting which levels may execute.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoutePolicy {
    pub maximum_level: ExecutionLevel,
    pub allow_explicit_escalation: bool,
}

impl Default for RoutePolicy {
    fn default() -> Self {
        Self {
            maximum_level: ExecutionLevel::L3,
            allow_explicit_escalation: true,
        }
    }
}

/// Runtime reason a bounded level cannot safely finish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EscalationCause {
    NeedsSingleAction,
    NeedsPlanning,
    NeedsWorkspace,
    ToolRequestedBroaderScope,
}

/// Stateless deterministic router.
#[derive(Clone, Copy, Debug, Default)]
pub struct Router;

impl Router {
    /// Routes using anchored grammars. Unrecognized input always fails cheap to L0.
    pub fn route(
        &self,
        request: &RouteRequest<'_>,
        policy: RoutePolicy,
    ) -> HarnessResult<RouteDecision> {
        let prompt = request.prompt.trim();
        if prompt.is_empty() {
            return Err(Failure::invalid("router.route", "prompt cannot be empty"));
        }
        if prompt.len() > 1024 * 1024 || prompt.contains('\0') {
            return Err(Failure::invalid(
                "router.route",
                "prompt exceeds the routing text boundary",
            ));
        }

        let decision = if let Some(explicit) = request.explicit_level {
            if !policy.allow_explicit_escalation && explicit > ExecutionLevel::L1 {
                return Err(Failure::new(
                    ErrorCode::RouteDenied,
                    FailureClass::Policy,
                    "router.route",
                    "explicit agent levels are disabled",
                ));
            }
            decision_for(explicit, RouteReason::ExplicitLevel, "explicit.v1", prompt)
        } else if is_explicit_prefix(prompt, "/l3") || is_explicit_prefix(prompt, "agent:l3") {
            decision_for(
                ExecutionLevel::L3,
                RouteReason::ExplicitLevel,
                "prefix.l3.v1",
                prompt,
            )
        } else if is_explicit_prefix(prompt, "/l2") || is_explicit_prefix(prompt, "agent:l2") {
            decision_for(
                ExecutionLevel::L2,
                RouteReason::ExplicitLevel,
                "prefix.l2.v1",
                prompt,
            )
        } else if is_workspace_goal(prompt) {
            decision_for(
                ExecutionLevel::L3,
                RouteReason::GoalWorkspaceTask,
                "workspace.goal.v1",
                prompt,
            )
        } else if is_single_action(prompt) {
            decision_for(
                ExecutionLevel::L1,
                RouteReason::DeterministicSingleAction,
                "single.action.v1",
                prompt,
            )
        } else if is_bounded_task(prompt) {
            decision_for(
                ExecutionLevel::L2,
                RouteReason::BoundedMultiStepTask,
                "bounded.task.v1",
                prompt,
            )
        } else {
            decision_for(
                ExecutionLevel::L0,
                RouteReason::DirectConversation,
                "default.direct.v1",
                prompt,
            )
        };

        if decision.level > policy.maximum_level {
            return Err(Failure::new(
                ErrorCode::RouteDenied,
                FailureClass::Policy,
                "router.route",
                "selected level exceeds deployment policy",
            )
            .with_detail("selected", decision.level.as_str())
            .with_detail("maximum", policy.maximum_level.as_str()));
        }
        if !decision
            .required_capabilities
            .is_subset_of(&request.available_capabilities)
        {
            return Err(Failure::new(
                ErrorCode::CapabilityUnavailable,
                FailureClass::Policy,
                "router.route",
                "required capability is unavailable",
            ));
        }
        Ok(decision)
    }

    /// Escalates by at most one level and never widens past policy.
    pub fn escalate(
        &self,
        current: &RouteDecision,
        cause: EscalationCause,
        policy: RoutePolicy,
    ) -> HarnessResult<RouteDecision> {
        let target = match cause {
            EscalationCause::NeedsSingleAction => ExecutionLevel::L1,
            EscalationCause::NeedsPlanning => ExecutionLevel::L2,
            EscalationCause::NeedsWorkspace | EscalationCause::ToolRequestedBroaderScope => {
                ExecutionLevel::L3
            }
        };
        if target <= current.level {
            return Err(Failure::new(
                ErrorCode::RouteDenied,
                FailureClass::Policy,
                "router.escalate",
                "escalation cause does not require a higher execution level",
            ));
        }
        let Some(next) = current.level.next() else {
            return Err(Failure::new(
                ErrorCode::RouteDenied,
                FailureClass::Policy,
                "router.escalate",
                "already at maximum execution level",
            ));
        };
        if target != next {
            return Err(Failure::new(
                ErrorCode::RouteDenied,
                FailureClass::Policy,
                "router.escalate",
                "multi-level escalation is forbidden",
            ));
        }
        if next > policy.maximum_level {
            return Err(Failure::new(
                ErrorCode::RouteDenied,
                FailureClass::Policy,
                "router.escalate",
                "escalation exceeds deployment policy",
            ));
        }
        Ok(decision_for(
            next,
            match next {
                ExecutionLevel::L0 => RouteReason::DirectConversation,
                ExecutionLevel::L1 => RouteReason::DeterministicSingleAction,
                ExecutionLevel::L2 => RouteReason::BoundedMultiStepTask,
                ExecutionLevel::L3 => RouteReason::GoalWorkspaceTask,
            },
            "runtime.escalation.v1",
            "",
        ))
    }
}

fn decision_for(
    level: ExecutionLevel,
    reason: RouteReason,
    rule_id: &'static str,
    prompt: &str,
) -> RouteDecision {
    let lower = prompt.to_ascii_lowercase();
    let mut capabilities = CapabilitySet::new();
    let mut confirmation_required = false;
    match level {
        ExecutionLevel::L0 => {
            capabilities.insert(Capability::Model);
        }
        ExecutionLevel::L1 => {
            if lower.starts_with("read file ")
                || lower.starts_with("show file ")
                || lower.starts_with("list files")
            {
                capabilities.insert(Capability::FileRead);
            } else if lower.starts_with("write file ") {
                capabilities.insert(Capability::FileWrite);
                confirmation_required = true;
            } else if lower.starts_with("run command ") {
                capabilities.insert(Capability::ProcessSpawn);
                confirmation_required = true;
            }
        }
        ExecutionLevel::L2 => {
            capabilities.insert(Capability::Model);
        }
        ExecutionLevel::L3 => {
            capabilities.insert(Capability::Model);
            capabilities.insert(Capability::Workspace);
            confirmation_required = true;
        }
    }
    RouteDecision {
        level,
        reason,
        confidence_basis_points: match reason {
            RouteReason::ExplicitLevel => 10_000,
            RouteReason::DirectConversation => 9_500,
            RouteReason::DeterministicSingleAction => 9_900,
            RouteReason::BoundedMultiStepTask => 9_000,
            RouteReason::GoalWorkspaceTask => 9_800,
        },
        required_capabilities: capabilities,
        confirmation_required,
        rule_id,
    }
}

fn is_explicit_prefix(prompt: &str, prefix: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    lower == prefix || lower.starts_with(&format!("{prefix} "))
}

fn is_single_action(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    let anchored = [
        "list files",
        "read file ",
        "write file ",
        "run command ",
        "show file ",
    ];
    anchored
        .iter()
        .any(|prefix| lower == *prefix || lower.starts_with(prefix))
}

fn is_workspace_goal(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    if lower.starts_with("how ")
        || lower.starts_with("explain ")
        || lower.starts_with("outline ")
        || lower.starts_with("plan ")
        || has_any(
            &lower,
            &[
                "website outline",
                "website plan",
                "application outline",
                "application plan",
            ],
        )
    {
        return false;
    }
    let anchored = [
        "build a complete website",
        "build a complete application",
        "build a complete app",
        "implement a complete website",
        "implement a complete application",
        "implement a complete feature",
        "implement a complete service",
        "implement a complete api",
        "create a complete website",
        "create a website in ",
        "work across the codebase ",
        "fix the repository ",
    ];
    anchored.iter().any(|prefix| lower.starts_with(prefix))
        || ((lower.starts_with("implement ") || lower.starts_with("build "))
            && has_any(&lower, &[" repository", " codebase", " workspace"])
            && has_any(&lower, &[" test", " compile", " release", " complete"]))
}

fn is_bounded_task(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    let anchored = [
        "research and compare ",
        "analyze and summarize ",
        "plan and verify ",
        "investigate and report ",
        "use the tools to ",
        "perform these steps:",
    ];
    anchored.iter().any(|prefix| lower.starts_with(prefix))
}

fn has_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_anchored_actions_only() -> HarnessResult<()> {
        let router = Router;
        let action = router.route(
            &RouteRequest::new("read file README.md"),
            RoutePolicy::default(),
        )?;
        assert_eq!(action.level, ExecutionLevel::L1);
        let ordinary = router.route(
            &RouteRequest::new("I read file formats for fun"),
            RoutePolicy::default(),
        )?;
        assert_eq!(ordinary.level, ExecutionLevel::L0);
        Ok(())
    }

    #[test]
    fn escalation_cannot_skip() -> HarnessResult<()> {
        let router = Router;
        let start = router.route(&RouteRequest::new("hello"), RoutePolicy::default())?;
        let result = router.escalate(
            &start,
            EscalationCause::NeedsWorkspace,
            RoutePolicy::default(),
        );
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn redundant_escalation_cause_cannot_raise_authority() -> HarnessResult<()> {
        let router = Router;
        let l1 = router.route(
            &RouteRequest::new("read file README.md"),
            RoutePolicy::default(),
        )?;
        assert!(
            router
                .escalate(
                    &l1,
                    EscalationCause::NeedsSingleAction,
                    RoutePolicy::default(),
                )
                .is_err()
        );
        let l2 = router.route(
            &RouteRequest::new("research and compare these formats"),
            RoutePolicy::default(),
        )?;
        assert!(
            router
                .escalate(&l2, EscalationCause::NeedsPlanning, RoutePolicy::default(),)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn direct_route_declares_its_model_requirement() -> HarnessResult<()> {
        let router = Router;
        let mut request = RouteRequest::new("hello");
        request.available_capabilities = CapabilitySet::new();
        assert!(router.route(&request, RoutePolicy::default()).is_err());
        request.available_capabilities = CapabilitySet::from_slice(&[Capability::Model]);
        let decision = router.route(&request, RoutePolicy::default())?;
        assert_eq!(decision.level, ExecutionLevel::L0);
        assert!(decision.required_capabilities.contains(Capability::Model));
        Ok(())
    }
}
