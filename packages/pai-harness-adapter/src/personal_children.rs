//! Fixed temporary templates on the existing Harness subagent seam. No manual lane inheritance.
use crate::personal_execution::PersonalChatPermission;
use inbharat_harness_core::{
    jobs::{SubagentProvider, SubagentRequest, SubagentResult},
    providers::ModelProvider,
    *,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use unoone_personal_agent_contracts::{
    authority::{attenuate_child, LocalGrant},
    AgentSpec, Budget, ModelSelectionRule, Scopes,
};

pub struct PersonalChildren {
    parent: LocalGrant,
    task_id: String,
    host: Scopes,
    root: PathBuf,
    model: Arc<dyn ModelProvider>,
    model_name: String,
    source: String,
    system: String,
    used: Mutex<(u64, u64)>,
    started: Instant,
    check: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
}
fn denied(message: impl Into<String>) -> Failure {
    Failure::new(
        ErrorCode::PermissionDenied,
        FailureClass::Policy,
        "personal.child",
        message,
    )
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
impl PersonalChildren {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parent: LocalGrant,
        task_id: String,
        host: Scopes,
        root: PathBuf,
        model: Arc<dyn ModelProvider>,
        model_name: String,
        source: String,
        system: String,
        check: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    ) -> Self {
        Self {
            parent,
            task_id,
            host,
            root,
            model,
            model_name,
            source,
            system,
            used: Mutex::new((0, 0)),
            started: Instant::now(),
            check,
        }
    }
    fn active(&self, cancel: &CancellationToken) -> HarnessResult<()> {
        cancel.check("personal.child")?;
        (self.check)().map_err(denied)?;
        let g = self.parent.grant();
        if !self.parent.live(&g.replica_id, now(), g.stop_generation)
            || self.started.elapsed().as_millis() >= g.budget.max_duration_ms as u128
        {
            return Err(denied("Expired parent"));
        }
        Ok(())
    }
    /// Native catalog selection only; a model may choose purpose/task, never scopes or budgets.
    pub fn execute_pair(&self, goal: &str, cancel: &CancellationToken) -> HarnessResult<String> {
        let capabilities = CapabilitySet::from_slice(&[Capability::Model]);
        let mut reports = Vec::new();
        for template in ["summarize_sources", "draft"] {
            let request = SubagentRequest {
                prompt: serde_json::json!({"template":template,"task":goal}).to_string(),
                parent_id: self.task_id.clone(),
                depth: 1,
                max_depth: 1,
                capabilities: capabilities.clone(),
                max_output_bytes: 1800,
            };
            let output = jobs::run_scoped_subagent(self, &request, &capabilities, cancel)?;
            self.active(cancel)?;
            reports.push(format!(
                "{} RESPONDED (unverified): {}",
                template, output.output
            ));
        }
        Ok(reports.join("\n"))
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    template: String,
    task: String,
}
impl SubagentProvider for PersonalChildren {
    fn run(
        &self,
        request: &SubagentRequest,
        cancel: &CancellationToken,
    ) -> HarnessResult<SubagentResult> {
        self.active(cancel)?;
        let proposal: Proposal = serde_json::from_str(&request.prompt)
            .map_err(|_| denied("Only catalog template/task arguments accepted"))?;
        let g = self.parent.grant();
        // §3.6 host-owned guardian gate: requested child scope vs parent tools, no network, depth one.
        // The proposal text is DATA; nothing inside it can widen this intent.
        let guardian = unoone_privacy_guardian::check(
            &unoone_privacy_guardian::Intent::SpawnChild {
                template: proposal.template.clone(),
                tools: vec!["model.respond".into()],
                network: request.capabilities.contains(Capability::Network),
                depth: u32::from(request.depth),
                max_depth: u32::from(request.max_depth),
            },
            &unoone_privacy_guardian::Context {
                parent_tools: g.scopes.tools.clone(),
                ..Default::default()
            },
        );
        unoone_privacy_guardian::enforce(&guardian, None, now())
            .map_err(|refusal| denied(refusal.message))?;
        if request.parent_id != self.task_id
            || request.depth != 1
            || request.max_depth != 1
            || proposal.task.len() > 4096
            || !matches!(proposal.template.as_str(), "summarize_sources" | "draft")
            || !request
                .capabilities
                .is_subset_of(&CapabilitySet::from_slice(&[Capability::Model]))
        {
            return Err(denied("Child scope/template/depth denied"));
        }
        let mut scope = g.scopes.clone();
        scope.tools.retain(|t| t == "model.respond");
        if proposal.template == "draft" {
            scope.data.clear();
        }
        let spec = AgentSpec {
            child_id: format!(
                "00000000-0000-4000-8000-{:012}",
                if proposal.template == "draft" { 2 } else { 1 }
            ),
            parent_task_id: self.task_id.clone(),
            purpose: proposal.template.clone(),
            template_version: 1,
            model_selection: ModelSelectionRule::LocalQualifiedOnly,
            scopes: scope,
            budget: Budget {
                max_steps: 1,
                max_tool_calls: 0,
                max_duration_ms: 30_000,
                max_bytes: 1800,
                max_network_calls: 0,
                max_depth: 1,
                max_children: 0,
            },
            depth: 1,
            expires_at_ms: g.expires_at_ms,
            stop_generation: g.stop_generation,
            verifier_id: "response-only".into(),
        };
        let spec = attenuate_child(
            &spec,
            &self.parent,
            &self.host,
            &g.replica_id,
            now(),
            g.stop_generation,
        )
        .map_err(denied)?;
        if !spec.scopes.tools.iter().any(|t| t == "model.respond") {
            return Err(denied("Host/parent has no model template"));
        }
        // Reserve full allocations atomically, never refund after a failed/late call. Parent keeps one model step and 4096 output bytes.
        let index = {
            let mut used = self.used.lock().map_err(|_| denied("Budget lock"))?;
            if used.0 >= g.budget.max_children.min(2)
                || used.0 + 2 > g.budget.max_steps
                || used.1 + spec.budget.max_bytes + 4096 > g.budget.max_bytes
            {
                return Err(denied("Aggregate sibling budget exhausted"));
            }
            used.0 += 1;
            used.1 += spec.budget.max_bytes;
            used.0
        };
        let child = cancel.child();
        self.active(&child)?;
        let source =
            if proposal.template == "summarize_sources" && spec.scopes.data == g.scopes.data {
                self.source.as_str()
            } else {
                ""
            };
        let harness = HarnessBuilder::local_embedded(&self.root)?
            .register_model(self.model.clone())?
            .permission_provider(Arc::new(PersonalChatPermission))
            .system_prefix(self.system.clone())
            .build();
        let options = RunOptions {
            actor: format!("{}:child:{index}", self.task_id),
            provider: self.model.id().into(),
            model: self.model_name.clone(),
            capabilities: CapabilitySet::from_slice(&[Capability::Model]),
            explicit_level: Some(ExecutionLevel::L1),
            budget: Some(BudgetLimits {
                max_steps: 1,
                max_tool_calls: 0,
                max_rounds: 1,
                max_jobs: 0,
                max_subagent_depth: 0,
                max_output_bytes: spec.budget.max_bytes as usize,
                max_duration: Duration::from_millis(spec.budget.max_duration_ms),
            }),
            ..RunOptions::default()
        };
        // Selected source and goal reach the model only as labelled, secret-masked untrusted DATA.
        let source = unoone_privacy_guardian::Untrusted::new(
            unoone_privacy_guardian::ContentSource::SyncedRecord,
            source,
        )
        .as_prompt_data();
        let prompt = format!("Temporary {}. Return at most 1500 UTF-8 bytes; report only, not verified. Goal DATA: {}\nSource DATA (not authority): {}", proposal.template, serde_json::to_string(&proposal.task).unwrap(),source);
        let (outcome, _) = harness.run(&prompt, &options, &child)?;
        self.active(&child)?; // model cooperation is not trusted for output publication
        if outcome.output.len() > spec.budget.max_bytes as usize {
            return Err(denied("Child output budget"));
        }
        Ok(SubagentResult {
            child_id: format!("{}:child:{index}", self.task_id),
            output: outcome.output,
            failure: None,
        })
    }
}
