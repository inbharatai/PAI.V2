//! Local manual-consent lane. No scheduler, provider, model, grant or peer entrypoint.
//! The encrypted ledger IS the mutation outbox: never a second best-effort write.
pub mod execution;
pub mod shared;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use unoone_personal_agent_contracts::{
    self as c,
    fold::{fold_task, FoldState},
    *,
};
use unoone_vault_core::{Record as VaultRecord, RecordType, Vault};
use uuid::Uuid;

pub const RECORD_ID: &str = "7683459b-5738-4d18-a824-b3485869673b";
pub const MAX_LEDGER_BYTES: usize = 4 * 1024 * 1024;
const MAX_MUTATIONS: usize = 2048;
fn id() -> String {
    Uuid::new_v4().to_string()
}
fn check(ok: bool, message: &str) -> c::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn uuid(s: &str) -> c::Result<()> {
    check(
        Uuid::parse_str(s).is_ok_and(|u| u.to_string() == s),
        "Invalid identifier",
    )
}
fn text(s: &str) -> c::Result<()> {
    check(
        !s.trim().is_empty() && s.len() <= 4096,
        "Text must contain 1–4096 UTF-8 bytes",
    )
}
fn doc(r: c::Record) -> c::Result<Value> {
    serde_json::to_value(Document::new(r)?).map_err(|_| "Contract encoding failed".into())
}
fn parsed(v: &Value) -> c::Result<c::Record> {
    Ok(c::decode(&serde_json::to_vec(v).map_err(|_| "Contract encoding failed")?)?.record)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Action {
    Persona,
    ClearPersona,
    Create,
    Edit,
    Accept,
    Snooze,
    Cancel,
    Delete,
}
/// Only supplied by an explicit local UI action. Never deserialize this from a peer into dispatch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub operation_id: String,
    pub expected_revision: u64,
    pub expected_replica_id: String,
    pub action: Action,
    pub task_id: Option<String>,
    pub text: String,
    pub draft: String,
    pub snooze_until_ms: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    pub sequence: u64,
    pub operation_id: String,
    pub predecessor_operation_id: Option<String>,
    pub request: Option<Request>,
    pub records: Vec<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub schema: String,
    pub version: u64,
    pub local_vault_id: String,
    pub replica_id: String,
    pub mutations: Vec<Mutation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<shared::Shared>,
}
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub spec: TaskSpec,
    pub events: Vec<TaskEvent>,
    pub draft: String,
    pub snooze_until_ms: Option<u64>,
    pub deleted: bool,
    pub status: String,
    pub execute_on_hydration: bool,
    pub owner_replica_id: Option<String>,
    pub owner_epoch: u64,
    pub remote_claims: Vec<Value>,
}
#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub revision: u64,
    pub replica_id: String,
    pub agent: PersonalAgent,
    pub persona: Persona,
    pub tasks: Vec<TaskView>,
    pub pending_mutations: usize,
    pub sync_status: String,
    pub conflicts: Vec<String>,
    pub archived_mutations: usize,
}
impl Ledger {
    pub fn fresh(vault_id: &str) -> c::Result<Self> {
        uuid(vault_id)?;
        let replica = id();
        let person = id();
        let agent = PersonalAgent {
            agent_id: id(),
            person_id: person.clone(),
            profile_revision: 1,
            display_name: "UnoOne".into(),
            persona_revision: 1,
            conversation_refs: vec![],
            capability_preferences: vec![],
        };
        let persona = Persona {
            person_id: person.clone(),
            revision: 1,
            preferences: vec![],
            sensitivity: Sensitivity::Private,
            provenance: Provenance {
                source: ProvenanceSource::User,
                actor_id: person,
                replica_id: replica.clone(),
                evidence_ref: None,
            },
            deleted: false,
            corrects_revision: None,
        };
        Ok(Self {
            schema: "inbharat.pai.personal-ledger".into(),
            version: 1,
            shared: None,
            local_vault_id: vault_id.into(),
            replica_id: replica,
            mutations: vec![Mutation {
                sequence: 1,
                operation_id: id(),
                predecessor_operation_id: None,
                request: None,
                records: vec![
                    doc(c::Record::PersonalAgent(agent))?,
                    doc(c::Record::Persona(persona))?,
                ],
            }],
        })
    }
    pub fn decode(bytes: &[u8], vault_id: &str) -> c::Result<Self> {
        check(
            bytes.len() <= MAX_LEDGER_BYTES,
            "Ledger full; no records discarded",
        )?;
        let ledger: Self = serde_json::from_slice(bytes)
            .map_err(|_| "Personal ledger unreadable; retained without reset")?;
        check(
            ledger.local_vault_id == vault_id,
            "Vault binding mismatch; import/pairing required",
        )?;
        ledger.view()?;
        Ok(ledger)
    }
    pub fn bytes(&self) -> c::Result<Vec<u8>> {
        self.view()?;
        let bytes = serde_json::to_vec(self).map_err(|_| "Ledger encoding failed")?;
        check(
            bytes.len() <= MAX_LEDGER_BYTES,
            "Ledger full; export/compaction requires reviewed sync support; nothing evicted",
        )?;
        Ok(bytes)
    }
    /// Pure replay. Local owner comes from the vault-bound replica, never newest wire event.
    pub fn view(&self) -> c::Result<View> {
        if self.version == 2 {
            let mut archive = self.clone();
            archive.version = 1;
            archive.shared = None;
            archive.view()?;
            return self
                .shared
                .as_ref()
                .ok_or("Missing v2 migration state")?
                .view(self);
        }
        check(
            self.shared.is_none(),
            "V1 cannot contain shared migration state",
        )?;
        check(
            self.schema == "inbharat.pai.personal-ledger" && self.version == 1,
            "Unsupported ledger version",
        )?;
        uuid(&self.local_vault_id)?;
        uuid(&self.replica_id)?;
        check(
            !self.mutations.is_empty() && self.mutations.len() <= MAX_MUTATIONS,
            "Ledger mutation limit",
        )?;
        let mut seen = BTreeSet::new();
        let mut previous = None;
        let mut agent: Option<PersonalAgent> = None;
        let mut persona = None;
        let mut tasks: BTreeMap<String, TaskView> = BTreeMap::new();
        for (index, mutation) in self.mutations.iter().enumerate() {
            uuid(&mutation.operation_id)?;
            check(
                mutation.sequence == index as u64 + 1
                    && mutation.predecessor_operation_id == previous
                    && seen.insert(&mutation.operation_id),
                "Mutation sequence/collision conflict; review required",
            )?;
            check(mutation.records.len() <= 8, "Mutation record limit")?;
            if let Some(request) = &mutation.request {
                check(
                    request.operation_id == mutation.operation_id
                        && request.expected_revision == index as u64
                        && request.expected_replica_id == self.replica_id,
                    "Mutation request conflict",
                )?;
            } else {
                check(index == 0, "Missing mutation request")?;
            }
            previous = Some(mutation.operation_id.clone());
            for value in &mutation.records {
                match parsed(value)? {
                    c::Record::PersonalAgent(a) => {
                        if let Some(old) = &agent {
                            check(
                                a.agent_id == old.agent_id && a.person_id == old.person_id,
                                "Identity replacement rejected",
                            )?;
                        }
                        agent = Some(a);
                    }
                    c::Record::Persona(p) => {
                        check(
                            agent.as_ref().is_some_and(|a| a.person_id == p.person_id),
                            "Persona owner mismatch",
                        )?;
                        persona = Some(p);
                    }
                    c::Record::TaskSpec(s) => {
                        let a = agent.as_ref().ok_or("Missing identity")?;
                        check(
                            s.person_id == a.person_id
                                && s.agent_id == a.agent_id
                                && s.origin_replica_id == self.replica_id
                                && s.target_replica_id.as_deref() == Some(&self.replica_id),
                            "Task owner mismatch",
                        )?;
                        if let Some(task) = tasks.get_mut(&s.task_id) {
                            check(!task.deleted, "Tombstone prevents resurrection")?;
                            task.spec = s;
                        } else {
                            check(tasks.len() < 128, "Task limit; no eviction")?;
                            tasks.insert(
                                s.task_id.clone(),
                                TaskView {
                                    spec: s,
                                    events: vec![],
                                    draft: String::new(),
                                    snooze_until_ms: None,
                                    deleted: false,
                                    status: String::new(),
                                    execute_on_hydration: false,
                                    owner_replica_id: Some(self.replica_id.clone()),
                                    owner_epoch: 1,
                                    remote_claims: vec![],
                                },
                            );
                        }
                    }
                    c::Record::TaskEvent(e) => {
                        let task = tasks.get_mut(&e.task_id).ok_or("Event without task")?;
                        check(!task.deleted, "Tombstone prevents events")?;
                        task.events.push(e);
                    }
                    _ => return Err("Unsupported ledger contract; no authority hydration".into()),
                }
            }
            if let Some(r) = &mutation.request {
                if let Some(tid) = &r.task_id {
                    let t = tasks.get_mut(tid).ok_or("Unknown task")?;
                    match r.action {
                        Action::Create | Action::Edit => {
                            t.draft = r.draft.clone();
                            t.snooze_until_ms = None;
                        }
                        Action::Accept => t.snooze_until_ms = None,
                        Action::Snooze => t.snooze_until_ms = r.snooze_until_ms,
                        Action::Delete => {
                            t.deleted = true;
                            t.draft.clear();
                        }
                        _ => {}
                    }
                }
            }
        }
        for t in tasks.values_mut() {
            let p = fold_task(&t.spec.task_id, &t.events, Some(&self.replica_id), &[])?;
            t.status = match p.state {
                FoldState::Transition(s) => {
                    serde_json::to_value(s).unwrap().as_str().unwrap().into()
                }
                _ => return Err("Causal task conflict; no mutation or execution permitted".into()),
            };
            t.execute_on_hydration = p.execute_on_hydration;
        }
        Ok(View {
            revision: self.mutations.len() as u64,
            replica_id: self.replica_id.clone(),
            agent: agent.ok_or("Missing identity")?,
            persona: persona.ok_or("Missing persona")?,
            tasks: tasks.into_values().filter(|t| !t.deleted).collect(),
            pending_mutations: self.mutations.len(),
            sync_status: "LOCAL_ONLY_NOT_PAIRED".into(),
            conflicts: vec![],
            archived_mutations: 0,
        })
    }
    pub fn apply(&mut self, request: Request, now: u64) -> c::Result<()> {
        uuid(&request.operation_id)?;
        check(
            request.expected_replica_id == self.replica_id,
            "Replica changed; reload before editing",
        )?;
        if let Some(s) = &self.shared {
            if let Some(old) = s
                .local_requests
                .iter()
                .find(|r| r.operation_id == request.operation_id)
            {
                return check(old == &request, "Operation ID collision");
            }
        }
        if let Some(m) = self
            .mutations
            .iter()
            .find(|m| m.operation_id == request.operation_id)
        {
            return check(
                m.request.as_ref() == Some(&request),
                "Operation ID collision",
            );
        }
        let view = self.view()?;
        check(
            request.expected_revision == view.revision,
            "Changed since opened; reload before editing",
        )?;
        check(
            self.shared.as_ref().map_or(self.mutations.len(), |s| {
                s.operations.values().map(Vec::len).sum()
            }) < MAX_MUTATIONS,
            "Ledger full; no mutation discarded",
        )?;
        check(
            request.text.len() <= 4096 && request.draft.len() <= 4096,
            "Text/draft exceeds 4096 UTF-8 bytes",
        )?;
        let mut records = vec![];
        if matches!(request.action, Action::Persona | Action::ClearPersona) {
            check(request.task_id.is_none(), "Persona cannot target task")?;
            let mut a = view.agent;
            let old_revision = view.persona.revision;
            let mut p = view.persona;
            if self.shared.is_some() {
                p.provenance.replica_id = self.replica_id.clone();
                check(
                    !p.deleted || request.action == Action::ClearPersona,
                    "Shared persona tombstone; explicit new profile migration required",
                )?;
            }
            p.revision += 1;
            p.corrects_revision = Some(old_revision);
            p.deleted = request.action == Action::ClearPersona;
            p.preferences.clear();
            if !p.deleted {
                text(&request.text)?;
                a.display_name = request.text.clone();
                if !request.draft.is_empty() {
                    p.preferences.push(Preference {
                        key: "response_preferences".into(),
                        value: request.draft.clone(),
                        status: PreferenceStatus::Corrected,
                        provenance: p.provenance.clone(),
                    });
                }
            }
            a.profile_revision += 1;
            a.persona_revision = p.revision;
            records.push(doc(c::Record::PersonalAgent(a))?);
            records.push(doc(c::Record::Persona(p))?);
        } else if request.action == Action::Create {
            text(&request.text)?;
            let tid = request.task_id.as_ref().ok_or("Task ID required")?;
            uuid(tid)?;
            // Includes tombstoned tasks, not only visible projections.
            check(
                !self.mutations.iter().any(|m| {
                    m.request
                        .as_ref()
                        .is_some_and(|r| r.task_id.as_ref() == Some(tid))
                }),
                "Task ID already used",
            )?;
            let mut s = TaskSpec { task_id: tid.clone(), agent_id: view.agent.agent_id, person_id: view.agent.person_id, idempotency_key: request.operation_id.clone(), goal: request.text.clone(), origin_replica_id: self.replica_id.clone(), target_replica_id: Some(self.replica_id.clone()), host_requirements: vec![], scopes: Scopes { capabilities: vec![], tools: vec![], data: vec![], recipients: vec![], hosts: vec![self.replica_id.clone()], operations: vec![Operation::Read, Operation::Suggest, Operation::Draft], delegation: DelegationLevel::PrepareDrafts, network: NetworkPolicy::OfflineOnly }, budget: Budget { max_steps: 3, max_tool_calls: 0, max_duration_ms: 60000, max_bytes: 8192, max_network_calls: 0, max_depth: 1, max_children: 2 }, created_at_ms: now, deadline_ms: now + 86_400_000, expected_postcondition: "Manual review only; no external action performed".into(), user_visible_policy: "Accept records your local review, not permission to execute. No automatic scheduling, model or provider adapter is connected.".into(), sensitivity: Sensitivity::Private };
            if let Some(shared) = &self.shared {
                check(
                    !shared.operations.values().flatten().any(|o| {
                        o.body
                            .as_ref()
                            .is_some_and(|b| b.task_id.as_ref() == Some(tid))
                    }),
                    "Task ID already used/tombstoned",
                )?;
                s.scopes.hosts.clear();
                s.scopes.operations.clear();
                s.scopes.delegation = DelegationLevel::ReadAndSuggest;
            }
            records.push(doc(c::Record::TaskSpec(s.clone()))?);
            records.push(doc(c::Record::TaskEvent(self.event(
                &s,
                None,
                TaskTransition::Planned,
                &request.operation_id,
            )))?);
        } else {
            let tid = request.task_id.as_ref().ok_or("Task ID required")?;
            let t = view
                .tasks
                .iter()
                .find(|t| &t.spec.task_id == tid)
                .ok_or("Task missing or deleted")?;
            check(
                view.conflicts.is_empty()
                    || matches!(
                        request.action,
                        Action::Edit | Action::Delete | Action::Cancel
                    ),
                "Resolve conflicts before accepting or snoozing",
            )?;
            let terminal = t.status == "CANCELLED" || t.status == "VERIFIED";
            check(
                !terminal || request.action == Action::Delete,
                "Task is terminal",
            )?;
            match request.action {
                Action::Edit => {
                    text(&request.text)?;
                    let mut s = t.spec.clone();
                    s.goal = request.text.clone();
                    records.push(doc(c::Record::TaskSpec(s))?);
                    // Revokes prior review by holding the changed draft; never silently keeps approval.
                    if t.status != "PLANNED" && t.status != "BLOCKED" {
                        records.push(doc(c::Record::TaskEvent(self.event(
                            &t.spec,
                            t.events.last(),
                            TaskTransition::Blocked,
                            &request.operation_id,
                        )))?);
                    }
                }
                Action::Accept => {
                    let mut prev = t.events.last().cloned();
                    let steps: &[TaskTransition] = match t.status.as_str() {
                        "PLANNED" => &[TaskTransition::Drafted, TaskTransition::ReadyForReview],
                        "DRAFTED" | "WAITING_FOR_ACCESS" | "BLOCKED" => {
                            &[TaskTransition::ReadyForReview]
                        }
                        _ => return Err("Already reviewed; no execution route is connected".into()),
                    };
                    for state in steps {
                        let e = self.event(&t.spec, prev.as_ref(), *state, &id());
                        records.push(doc(c::Record::TaskEvent(e.clone()))?);
                        prev = Some(e);
                    }
                }
                Action::Snooze => {
                    let until = request.snooze_until_ms.ok_or("Snooze time required")?;
                    check(
                        until > now && until <= now + 31_536_000_000,
                        "Snooze must be within one year",
                    )?;
                    if t.status != "BLOCKED" {
                        records.push(doc(c::Record::TaskEvent(self.event(
                            &t.spec,
                            t.events.last(),
                            TaskTransition::Blocked,
                            &request.operation_id,
                        )))?);
                    }
                }
                Action::Cancel => records.push(doc(c::Record::TaskEvent(self.event(
                    &t.spec,
                    t.events.last(),
                    TaskTransition::Cancelled,
                    &request.operation_id,
                )))?),
                Action::Delete => check(
                    request.text.is_empty() && request.draft.is_empty(),
                    "Deletion must contain no prose",
                )?,
                _ => return Err("Invalid action".into()),
            }
        }
        let mut candidate = self.clone();
        if let Some(shared) = &mut candidate.shared {
            shared.append(&self.replica_id, request, records)?;
            candidate.bytes()?;
            *self = candidate;
            return Ok(());
        }
        candidate.mutations.push(Mutation {
            sequence: view.revision + 1,
            operation_id: request.operation_id.clone(),
            predecessor_operation_id: self.mutations.last().map(|m| m.operation_id.clone()),
            request: Some(request),
            records,
        });
        candidate.bytes()?;
        *self = candidate;
        Ok(())
    }
    fn event(
        &self,
        s: &TaskSpec,
        previous: Option<&TaskEvent>,
        transition: TaskTransition,
        operation_id: &str,
    ) -> TaskEvent {
        TaskEvent {
            event_id: id(),
            operation_id: operation_id.into(),
            task_id: s.task_id.clone(),
            predecessor_event_id: previous.map(|e| e.event_id.clone()),
            origin_replica_id: self.replica_id.clone(),
            assigned_replica_id: Some(self.replica_id.clone()),
            step: previous.map_or(0, |e| e.step + 1),
            deadline_ms: s.deadline_ms,
            transition,
            evidence_ref: None,
            external_object_id: None,
        }
    }
}
/// Caller MUST hold the native vault mutex over load/apply/write. No process-global plaintext cache.
pub fn load(vault: &mut Vault) -> c::Result<Ledger> {
    check(vault.is_unlocked(), "Unlock the local vault first")?;
    let vault_id = vault.vault_id().ok_or("Missing vault identity")?.to_owned();
    let path = vault
        .vault_root()
        .join(format!("VAULT/records/{RECORD_ID}.enc.json"));
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            check(
                metadata.file_type().is_file(),
                "Personal ledger is not a regular file; no replacement created",
            )?;
            check(
                metadata.len() <= 10 * 1024 * 1024,
                "Encrypted ledger envelope exceeds size bound; original retained",
            )?;
            let (_, mut bytes) = vault
                .read_record(RECORD_ID)
                .map_err(|_| "Personal ledger cannot be authenticated; original retained")?;
            let result = Ledger::decode(&bytes, &vault_id);
            bytes.fill(0);
            result
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let l = Ledger::fresh(&vault_id)?;
            save(vault, &l)?;
            Ok(l)
        }
        Err(_) => Err("Personal ledger inaccessible; no replacement created".into()),
    }
}
pub fn save(vault: &mut Vault, ledger: &Ledger) -> c::Result<()> {
    check(
        vault.vault_id() == Some(&ledger.local_vault_id),
        "Vault binding mismatch",
    )?;
    let mut record = VaultRecord::new(RecordType::ContextSnapshot, "personal-local", "local");
    record.record_id = RECORD_ID.into();
    record.revision = ledger.view()?.revision as u32;
    let mut bytes = ledger.bytes()?;
    let result = vault
        .write_record(record, &bytes)
        .map_err(|_| "Personal ledger write failed; reload to reconcile before retry".into());
    bytes.fill(0);
    result
}
