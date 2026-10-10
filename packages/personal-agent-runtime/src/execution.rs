//! Native-only execution adapter; inert records never become dispatch authority.
//! Uses the existing ledger/outbox and shared causal append, without changing the wire schema.
use super::*;
use c::authority::{approve_locally, LocalGrant};

#[derive(Clone)]
pub struct DraftPermit {
    approved_revision: u64,
    task_id: String,
    grant: LocalGrant,
    source: ReviewedSource,
    coding_check: bool,
}
/// Declarative local UI selection, not an execution grant. Stored only in the native permit.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum ReviewedSource {
    #[default]
    None,
    Notes {
        record_ids: Vec<String>,
        query: String,
    },
    SelectedFile {
        path: String,
    },
}
impl ReviewedSource {
    pub fn validate(&self) -> c::Result<()> {
        match self {
            Self::None => Ok(()),
            Self::Notes { record_ids, query } => check(
                !record_ids.is_empty()
                    && record_ids.len() <= 8
                    && record_ids
                        .iter()
                        .all(|id| uuid::Uuid::parse_str(id).is_ok())
                    && query.len() <= 256
                    && !query.trim().is_empty(),
                "Choose 1–8 exact note IDs and a bounded search query",
            ),
            Self::SelectedFile { path } => check(
                !path.is_empty() && path.len() <= 4096 && std::path::Path::new(path).is_absolute(),
                "Select one absolute file path inside an existing folder grant",
            ),
        }
    }
}
impl DraftPermit {
    /// Only the distinct native coding-check review may select this fixed adapter.
    pub fn for_coding_check(mut self) -> c::Result<Self> {
        check(
            matches!(self.source, ReviewedSource::SelectedFile { .. }),
            "Coding check requires exact file selection",
        )?;
        let mut grant = self.grant.grant().clone();
        grant.scopes.tools = vec!["coding.python.check".into()];
        self.grant = approve_locally(
            grant.clone(),
            &grant.replica_id,
            "local-reviewed-isolated-python-check",
            grant.issued_at_ms,
            grant.stop_generation,
        )?;
        self.coding_check = true;
        Ok(self)
    }

    pub fn source(&self) -> &ReviewedSource {
        &self.source
    }

    pub fn grant(&self) -> &LocalGrant {
        &self.grant
    }
}

/// Accept-for-review is not execution permission. Call only after a distinct main-host
/// user action reviews the fixed draft-only template. It has no tools, files or network.
pub fn approve_draft(
    view: &View,
    task_id: &str,
    expected_revision: u64,
    now: u64,
    generation: u64,
) -> c::Result<DraftPermit> {
    approve_with_source(
        view,
        task_id,
        expected_revision,
        now,
        generation,
        ReviewedSource::None,
    )
}

/// Called only by the native local review event. Peer/model data cannot call this seam.
pub fn approve_with_source(
    view: &View,
    task_id: &str,
    expected_revision: u64,
    now: u64,
    generation: u64,
    source: ReviewedSource,
) -> c::Result<DraftPermit> {
    approve_template(
        view,
        task_id,
        expected_revision,
        now,
        generation,
        source,
        false,
    )
}
#[allow(clippy::too_many_arguments)]
pub fn approve_template(
    view: &View,
    task_id: &str,
    expected_revision: u64,
    now: u64,
    generation: u64,
    source: ReviewedSource,
    children: bool,
) -> c::Result<DraftPermit> {
    source.validate()?;
    check(
        view.revision == expected_revision && view.conflicts.is_empty(),
        "Changed/conflicted task; review again",
    )?;
    let task = view
        .tasks
        .iter()
        .find(|t| t.spec.task_id == task_id)
        .ok_or("Task missing")?;
    local_owner(view, task)?;
    check(
        task.status == "READY_FOR_REVIEW"
            && task.snooze_until_ms.is_none()
            && task.spec.deadline_ms > now,
        "Task must be accepted, current and not snoozed",
    )?;
    let mut host = Scopes {
        capabilities: vec![],
        tools: vec![],
        data: vec![],
        recipients: vec![],
        hosts: vec![view.replica_id.clone()],
        operations: vec![Operation::Draft, Operation::Suggest],
        delegation: DelegationLevel::PrepareDrafts,
        network: NetworkPolicy::OfflineOnly,
    };
    match &source {
        ReviewedSource::None => {}
        ReviewedSource::Notes { record_ids, .. } => {
            host.tools.push("personal.notes.search".into());
            host.operations.push(Operation::Read);
            host.data = record_ids
                .iter()
                .map(|id| DataScope {
                    kind: DataKind::Record,
                    resource_id: id.clone(),
                    operations: vec![Operation::Read],
                })
                .collect();
        }
        ReviewedSource::SelectedFile { .. } => {
            host.tools.push("personal.file.read".into());
            host.operations.push(Operation::Read);
            host.data.push(DataScope {
                kind: DataKind::File,
                resource_id: "selected-file".into(),
                operations: vec![Operation::Read],
            });
        }
    }
    // The explicit local draft-template review is the request for this new grant;
    // imported TaskSpec scope-shaped scaffolding is neither approval nor denial.
    // It cannot contribute datasets/tools/recipients: this template grants NONE.
    // No scope is enlarged from a running parent or inferred from persona prose.
    if children {
        host.tools.push("model.respond".into());
    }
    let reviewed_template = host.clone();
    let scopes = reviewed_template.intersect(&host);
    let budget = task.spec.budget.intersect(&Budget {
        max_steps: if children { 3 } else { 1 },
        max_tool_calls: 0,
        max_duration_ms: 60_000,
        max_bytes: if children { 8192 } else { 4096 },
        max_network_calls: 0,
        max_depth: if children { 1 } else { 0 },
        max_children: if children { 2 } else { 0 },
    });
    check(
        !children
            || (budget.max_steps >= 3
                && budget.max_children >= 2
                && budget.max_depth >= 1
                && budget.max_bytes >= 7696),
        "Task budget predates child template; create a new reviewed task",
    )?;
    let expires_at_ms = now
        .saturating_add(budget.max_duration_ms)
        .min(task.spec.deadline_ms);
    let grant = CapabilityGrant {
        grant_id: id(),
        person_id: view.agent.person_id.clone(),
        replica_id: view.replica_id.clone(),
        scopes,
        budget,
        issued_at_ms: now,
        expires_at_ms,
        stop_generation: generation,
        revoked: false,
    };
    Ok(DraftPermit {
        coding_check: false,
        source,
        approved_revision: view.revision,
        task_id: task_id.into(),
        grant: approve_locally(
            grant,
            &view.replica_id,
            "local-reviewed-draft-template-v1",
            now,
            generation,
        )?,
    })
}
fn local_owner(view: &View, task: &TaskView) -> c::Result<()> {
    check(
        !task.deleted
            && !task.execute_on_hydration
            && task.owner_epoch == 1
            && task.owner_replica_id.as_deref() == Some(view.replica_id.as_str())
            && task.spec.origin_replica_id == view.replica_id
            && task.spec.target_replica_id.as_deref() == Some(view.replica_id.as_str())
            && task.remote_claims.is_empty(),
        "Foreign/ambiguous/handoff task cannot execute",
    )
}

#[derive(Debug, Clone, Copy)]
pub enum DraftPhase {
    Started,
    Responded,
    Failed,
}

/// Durable plan/attempt/result through the existing encrypted ledger. A model response
/// never creates VERIFIED or ACTION_VERIFIED. Stop/lock discards output rather than saving it.
impl Ledger {
    #[allow(clippy::too_many_arguments)] // explicit independent revision/task/permit/time/generation checks at the native boundary
    pub fn record_draft_attempt(
        &mut self,
        expected_revision: u64,
        task_id: &str,
        phase: DraftPhase,
        output: &str,
        permit: &DraftPermit,
        now: u64,
        generation: u64,
    ) -> c::Result<()> {
        check(permit.task_id == task_id, "Task permit mismatch")?;
        let permitted_revision = permit.approved_revision
            + if matches!(phase, DraftPhase::Started) {
                0
            } else {
                1
            };
        check(
            expected_revision == permitted_revision,
            "One-shot draft permit no longer matches this revision",
        )?;
        let grant = &permit.grant;
        let view = self.view()?;
        check(
            view.revision == expected_revision && view.conflicts.is_empty(),
            "Task changed; discard output",
        )?;
        check(
            grant.live(&view.replica_id, now, generation)
                && grant.grant().person_id == view.agent.person_id,
            "Draft grant revoked/expired",
        )?;
        let task = view
            .tasks
            .iter()
            .find(|t| t.spec.task_id == task_id)
            .ok_or("Task deleted")?;
        local_owner(&view, task)?;
        check(
            output.len() <= 3800,
            "Draft output exceeds bounded review buffer",
        )?;
        let (transition, draft) = match phase {
            DraftPhase::Started => {
                check(
                    task.status == "READY_FOR_REVIEW",
                    "Task already attempted; no replay",
                )?;
                (
                    TaskTransition::InProgress,
                    if permit.coding_check {
                        "Native plan: isolated Python compile check of one selected granted file, 20 seconds maximum gate time; no model, edits, copy-out, apply or host-command fallback.".into()
                    } else {
                        format!("Native plan: one local-model draft; source template {}. Read-only exact local selection, no file changes, network or sends. Interrupted attempt needs review.", match permit.source() { ReviewedSource::None => "no source", ReviewedSource::Notes { .. } => "selected encrypted notes search", ReviewedSource::SelectedFile { .. } => "selected granted file" })
                    },
                )
            }
            DraftPhase::Responded => {
                check(task.status == "IN_PROGRESS", "Attempt no longer active")?;
                (
                    TaskTransition::AwaitingVerification,
                    if permit.coding_check {
                        format!(
                            "UNVERIFIED wider task — isolated coding check report only.\n{output}"
                        )
                    } else {
                        format!("RESPONDED — generated draft, not verified facts or an external effect. Review before use.\n{output}")
                    },
                )
            }
            DraftPhase::Failed => {
                check(task.status == "IN_PROGRESS", "Attempt no longer active")?;
                (
                    TaskTransition::Blocked,
                    "Native attempt failed/stopped. No automatic retry. Review and accept again."
                        .into(),
                )
            }
        };
        let operation_id = id();
        let request = Request {
            operation_id: operation_id.clone(),
            expected_revision,
            expected_replica_id: view.replica_id.clone(),
            action: Action::Edit,
            task_id: Some(task_id.into()),
            text: task.spec.goal.clone(),
            draft,
            snooze_until_ms: None,
        };
        let event = self.event(&task.spec, task.events.last(), transition, &operation_id);
        let records = vec![
            doc(c::Record::TaskSpec(task.spec.clone()))?,
            doc(c::Record::TaskEvent(event))?,
        ];
        let mut candidate = self.clone();
        if let Some(shared) = &mut candidate.shared {
            shared.append(&view.replica_id, request, records)?;
        } else {
            candidate.mutations.push(Mutation {
                sequence: view.revision + 1,
                operation_id,
                predecessor_operation_id: candidate
                    .mutations
                    .last()
                    .map(|m| m.operation_id.clone()),
                request: Some(request),
                records,
            });
        }
        candidate.bytes()?;
        *self = candidate;
        Ok(())
    }
}

/// Tiny guardian API (brief §3.6): record a privacy-guardian warning/receipt or a user
/// report/correction as an inert draft note on the task, through the SAME encrypted ledger/outbox
/// (`Action::Edit`, goal unchanged). The note must already be masked and bounded by the guardian
/// crate; this runtime only enforces prefix, size and ownership. A note never grants, suppresses a
/// warning or changes task status beyond the existing Edit rule (review hold while not PLANNED/BLOCKED).
pub const GUARDIAN_NOTE_PREFIXES: [&str; 2] = ["GUARDIAN RECEIPT v1", "GUARDIAN CORRECTION v1"];
impl Ledger {
    pub fn record_guardian_note(
        &mut self,
        expected_revision: u64,
        task_id: &str,
        note: &str,
        now: u64,
    ) -> c::Result<()> {
        check(
            GUARDIAN_NOTE_PREFIXES.iter().any(|p| note.starts_with(p)) && note.len() <= 3800,
            "Guardian note must carry the guardian prefix and stay within 3800 bytes",
        )?;
        let view = self.view()?;
        check(
            view.revision == expected_revision,
            "Ledger changed; reload before recording the guardian note",
        )?;
        let task = view
            .tasks
            .iter()
            .find(|t| t.spec.task_id == task_id)
            .ok_or("Task missing or deleted")?;
        local_owner(&view, task)?;
        check(task.status != "CANCELLED", "Task is terminal")?;
        self.apply(
            Request {
                operation_id: id(),
                expected_revision,
                expected_replica_id: view.replica_id.clone(),
                action: Action::Edit,
                task_id: Some(task_id.into()),
                text: task.spec.goal.clone(),
                draft: note.into(),
                snooze_until_ms: None,
            },
            now,
        )
    }
}

/// Source-qualified handoff to the existing Prepared lane; does not authorize a provider effect.
pub fn reviewed_task_body(view: &View, task_id: &str, revision: u64, body: &str) -> c::Result<()> {
    check(
        view.revision == revision && view.conflicts.is_empty(),
        "Task changed; review source again",
    )?;
    let task = view
        .tasks
        .iter()
        .find(|t| t.spec.task_id == task_id)
        .ok_or("Task missing")?;
    local_owner(view, task)?;
    check(
        !matches!(task.status.as_str(), "CANCELLED" | "IN_PROGRESS")
            && !body.trim().is_empty()
            && body == task.draft,
        "Exact local draft changed or task is not reviewable",
    )
}
