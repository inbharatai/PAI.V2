//! Version 2 opt-in shared board. The v1 ledger remains an immutable local archive.
//! Causal multi-value registers, never timestamps/LWW. No execution authority lives here.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SharedIdentity {
    pub person_id: String,
    pub agent_id: String,
    pub founder_replica_id: String,
    pub replicas: BTreeMap<String, String>, // full certificate fingerprints approved locally
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SharedAssignment {
    pub owner_replica_id: String,
    pub epoch: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SharedBody {
    pub assignment: Option<SharedAssignment>,
    pub action: Action,
    pub task_id: Option<String>,
    pub draft: String,
    pub snooze_until_ms: Option<u64>,
    pub records: Vec<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SharedOperation {
    pub replica_id: String,
    pub sequence: u64,
    pub operation_id: String,
    pub predecessor_operation_id: Option<String>,
    pub context: BTreeMap<String, u64>,
    pub body: Option<SharedBody>, // authenticated skipped position, not a deletion
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shared {
    pub identity: SharedIdentity,
    pub operations: BTreeMap<String, Vec<SharedOperation>>,
    pub local_requests: Vec<Request>, // never exported
}
impl Shared {
    pub fn clock(&self) -> BTreeMap<String, u64> {
        self.operations
            .iter()
            .map(|(r, ops)| (r.clone(), ops.len() as u64))
            .collect()
    }
    pub fn validate(&self) -> c::Result<()> {
        let i = &self.identity;
        uuid(&i.person_id)?;
        uuid(&i.agent_id)?;
        uuid(&i.founder_replica_id)?;
        check(
            i.replicas.len() == 2 && i.replicas.contains_key(&i.founder_replica_id),
            "Shared identity members",
        )?;
        for (r, fp) in &i.replicas {
            uuid(r)?;
            check(
                fp.len() == 64
                    && fp
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "Fingerprint required",
            )?;
        }
        let mut ids = BTreeSet::new();
        check(
            self.operations.values().map(Vec::len).sum::<usize>() <= 2048,
            "Shared history full; no eviction",
        )?;
        for (r, ops) in &self.operations {
            check(i.replicas.contains_key(r), "Unknown replica")?;
            let mut previous = None;
            let mut old_clock = BTreeMap::new();
            for (n, op) in ops.iter().enumerate() {
                uuid(&op.operation_id)?;
                check(
                    op.replica_id == *r
                        && op.sequence == n as u64 + 1
                        && op.predecessor_operation_id == previous
                        && ids.insert(&op.operation_id),
                    "Whole history collision/gap; hold for review",
                )?;
                check(
                    op.context.len() <= 2
                        && op.context.keys().all(|k| i.replicas.contains_key(k))
                        && op.context.get(r).copied().unwrap_or(0) == n as u64
                        && op.context.values().all(|v| *v <= 2048),
                    "Invalid causal context",
                )?;
                for (k, v) in &old_clock {
                    check(
                        op.context.get(k).copied().unwrap_or(0) >= *v,
                        "Causal clock rollback",
                    )?;
                }
                for (dependency, sequence) in &op.context {
                    if *sequence > 0 {
                        if let Some(prior) = self
                            .operations
                            .get(dependency)
                            .and_then(|ops| ops.get(*sequence as usize - 1))
                        {
                            check(
                                prior
                                    .context
                                    .iter()
                                    .all(|(r, n)| *n <= op.context.get(r).copied().unwrap_or(0)),
                                "Causal cycle or missing transitive dependency",
                            )?;
                        }
                    }
                }
                old_clock = op.context.clone();
                previous = Some(op.operation_id.clone());
                if let Some(b) = &op.body {
                    if b.action == Action::Create {
                        check(
                            b.assignment
                                .as_ref()
                                .is_some_and(|a| a.owner_replica_id == *r && a.epoch == 1),
                            "Explicit initial owner/epoch required",
                        )?;
                    } else {
                        check(
                            b.assignment.is_none(),
                            "Handoff is inert; assignment cannot be overwritten",
                        )?;
                    }
                    check(
                        b.records.len() <= 8 && b.draft.len() <= 4096,
                        "Shared body bound",
                    )?;
                    check(
                        b.snooze_until_ms.is_none_or(|v| v <= 9_007_199_254_740_991),
                        "Reminder bound",
                    )?;
                    if let Some(t) = &b.task_id {
                        uuid(t)?;
                    }
                    check(
                        matches!(b.action, Action::Persona | Action::ClearPersona)
                            == b.task_id.is_none(),
                        "Shared target mismatch",
                    )?;
                    for v in &b.records {
                        let valid = match parsed(v)? {
                            c::Record::PersonalAgent(a) => {
                                b.task_id.is_none()
                                    && a.person_id == i.person_id
                                    && a.agent_id == i.agent_id
                            }
                            c::Record::Persona(p) => {
                                b.task_id.is_none()
                                    && p.person_id == i.person_id
                                    && p.provenance.replica_id == *r
                            }
                            c::Record::TaskSpec(s) => {
                                b.task_id.as_deref() == Some(&s.task_id)
                                    && s.person_id == i.person_id
                                    && s.agent_id == i.agent_id
                                    && i.replicas.contains_key(&s.origin_replica_id)
                                    && s.target_replica_id
                                        .as_ref()
                                        .is_some_and(|r| i.replicas.contains_key(r))
                                    && s.scopes.capabilities.is_empty()
                                    && s.scopes.tools.is_empty()
                                    && s.scopes.data.is_empty()
                                    && s.scopes.recipients.is_empty()
                                    && s.scopes.hosts.is_empty()
                                    && s.scopes.operations.is_empty()
                                    && s.scopes.delegation == DelegationLevel::ReadAndSuggest
                                    && s.scopes.network == NetworkPolicy::OfflineOnly
                            }
                            c::Record::TaskEvent(e) => {
                                b.task_id.as_deref() == Some(&e.task_id)
                                    && e.origin_replica_id == *r
                            }
                            c::Record::TaskReceipt(claim) => {
                                b.task_id.as_deref() == Some(&claim.task_id)
                                    && claim.replica_id == *r
                            }
                            c::Record::Handoff(h) => {
                                b.task_id.as_deref() == Some(&h.task_id) && h.from_replica_id == *r
                            }
                            _ => false,
                        };
                        check(valid, "Unsupported shared record/authority/identity")?;
                    }
                }
            }
        }
        Ok(())
    }
    pub fn append(
        &mut self,
        replica: &str,
        request: Request,
        records: Vec<Value>,
    ) -> c::Result<()> {
        let context = self.clock();
        let ops = self.operations.entry(replica.into()).or_default();
        ops.push(SharedOperation {
            replica_id: replica.into(),
            sequence: ops.len() as u64 + 1,
            operation_id: request.operation_id.clone(),
            predecessor_operation_id: ops.last().map(|o| o.operation_id.clone()),
            context,
            body: Some(SharedBody {
                assignment: if request.action == Action::Create {
                    Some(SharedAssignment {
                        owner_replica_id: replica.into(),
                        epoch: 1,
                    })
                } else {
                    None
                },
                action: request.action.clone(),
                task_id: request.task_id.clone(),
                draft: request.draft.clone(),
                snooze_until_ms: request.snooze_until_ms,
                records,
            }),
        });
        self.local_requests.push(request);
        self.validate()
    }
    pub fn receive(&mut self, replica: &str, ops: &[SharedOperation]) -> c::Result<()> {
        check(
            self.identity.replicas.contains_key(replica),
            "Unapproved replica",
        )?;
        let target = self.operations.entry(replica.into()).or_default();
        for op in ops {
            check(
                op.replica_id == replica && op.sequence > 0,
                "Replica/sequence mismatch",
            )?;
            if let Some(old) = target.get(op.sequence as usize - 1) {
                check(old == op, "Whole history collision; original retained")?;
            } else {
                check(
                    op.sequence == target.len() as u64 + 1,
                    "Missing shared predecessor",
                )?;
                target.push(op.clone());
            }
        }
        self.validate()
    }
    pub fn view(&self, ledger: &Ledger) -> c::Result<View> {
        self.validate()?;
        let all: Vec<_> = self.operations.values().flatten().collect();
        let clock = self.clock();
        let missing = all.iter().any(|o| {
            o.context
                .iter()
                .any(|(r, n)| *n > clock.get(r).copied().unwrap_or(0))
        });
        let i = &self.identity;
        let mut agent = PersonalAgent {
            agent_id: i.agent_id.clone(),
            person_id: i.person_id.clone(),
            profile_revision: 1,
            display_name: "UnoOne".into(),
            persona_revision: 1,
            conversation_refs: vec![],
            capability_preferences: vec![],
        };
        let mut persona = Persona {
            person_id: i.person_id.clone(),
            revision: 1,
            preferences: vec![],
            sensitivity: Sensitivity::Private,
            provenance: Provenance {
                source: ProvenanceSource::User,
                actor_id: i.person_id.clone(),
                replica_id: i.founder_replica_id.clone(),
                evidence_ref: None,
            },
            deleted: false,
            corrects_revision: None,
        };
        let mut conflicts = vec![];
        if missing {
            conflicts
                .push("CAUSAL_GAP: sync remaining pages; no execution or review acceptance".into());
        }
        let personal: Vec<_> = all
            .iter()
            .copied()
            .filter(|o| o.body.as_ref().is_some_and(|b| b.task_id.is_none()))
            .collect();
        let clears: Vec<_> = personal
            .iter()
            .copied()
            .filter(|o| o.body.as_ref().unwrap().action == Action::ClearPersona)
            .collect();
        let persona_heads = heads(if clears.is_empty() {
            &personal
        } else {
            &clears
        });
        if persona_heads.len() == 1 {
            for v in &persona_heads[0].body.as_ref().unwrap().records {
                match parsed(v)? {
                    c::Record::PersonalAgent(a) => agent = a,
                    c::Record::Persona(p) => persona = p,
                    _ => (),
                }
            }
        } else if persona_heads.len() > 1 {
            conflicts.push(format!(
                "PERSONA_CONFLICT: {}",
                serde_json::to_string(&persona_heads).map_err(|_| "Conflict encoding")?
            ));
            agent.display_name = "Persona conflict — review retained versions".into();
        }
        if !clears.is_empty() {
            persona.deleted = true;
            persona.preferences.clear();
        }
        let tids: BTreeSet<_> = all
            .iter()
            .filter_map(|o| o.body.as_ref().and_then(|b| b.task_id.clone()))
            .collect();
        check(tids.len() <= 128, "Shared task limit")?;
        let mut tasks = vec![];
        for tid in tids {
            let ops: Vec<_> = all
                .iter()
                .copied()
                .filter(|o| {
                    o.body
                        .as_ref()
                        .is_some_and(|b| b.task_id.as_ref() == Some(&tid))
                })
                .collect();
            if ops
                .iter()
                .any(|o| o.body.as_ref().unwrap().action == Action::Delete)
            {
                continue;
            }
            let creates: Vec<_> = ops
                .iter()
                .copied()
                .filter(|o| o.body.as_ref().unwrap().action == Action::Create)
                .collect();
            check(
                creates.len() <= 1,
                "Task creation collision; whole history held",
            )?;
            if creates.is_empty() {
                conflicts.push(format!("MISSING_TASK_HISTORY: {tid}"));
                continue;
            }
            let owner = creates[0].replica_id.clone();
            let genesis = creates[0]
                .body
                .as_ref()
                .unwrap()
                .records
                .iter()
                .find_map(|v| match parsed(v).ok()? {
                    c::Record::TaskSpec(s) => Some(s),
                    _ => None,
                })
                .ok_or("Missing creation spec")?;
            check(
                genesis.origin_replica_id == owner
                    && genesis.target_replica_id.as_deref() == Some(&owner),
                "Task creator is not declarative owner",
            )?;
            for o in &ops {
                for v in &o.body.as_ref().unwrap().records {
                    if let c::Record::TaskSpec(s) = parsed(v)? {
                        check(
                            s.origin_replica_id == genesis.origin_replica_id
                                && s.target_replica_id == genesis.target_replica_id
                                && s.deadline_ms == genesis.deadline_ms
                                && s.created_at_ms == genesis.created_at_ms
                                && s.idempotency_key == genesis.idempotency_key,
                            "Task identity/deadline/assignment collision; hold",
                        )?;
                    }
                }
            }
            let writes: Vec<_> = ops
                .iter()
                .copied()
                .filter(|o| {
                    matches!(
                        o.body.as_ref().unwrap().action,
                        Action::Create | Action::Edit
                    )
                })
                .collect();
            let versions = heads(&writes);
            let mut specs = vec![];
            for o in &versions {
                for v in &o.body.as_ref().unwrap().records {
                    if let c::Record::TaskSpec(s) = parsed(v)? {
                        specs.push(s);
                    }
                }
            }
            check(!specs.is_empty(), "Missing task spec")?;
            let mut spec = specs.remove(0);
            let conflict = versions.len() > 1;
            let draft = if conflict {
                conflicts.push(format!(
                    "TASK_CONFLICT {tid}: {}",
                    serde_json::to_string(&versions).map_err(|_| "Conflict encoding")?
                ));
                spec.goal = "Conflicting task edits — review retained versions".into();
                String::new()
            } else {
                versions[0].body.as_ref().unwrap().draft.clone()
            };
            let reminders: Vec<_> = ops
                .iter()
                .copied()
                .filter(|o| {
                    matches!(
                        o.body.as_ref().unwrap().action,
                        Action::Create | Action::Edit | Action::Accept | Action::Snooze
                    )
                })
                .collect();
            let reminder_heads = heads(&reminders);
            let snooze = if reminder_heads.len() == 1 {
                reminder_heads[0].body.as_ref().unwrap().snooze_until_ms
            } else {
                conflicts.push(format!(
                    "REMINDER_CONFLICT {tid}: {}",
                    serde_json::to_string(&reminder_heads).map_err(|_| "Conflict encoding")?
                ));
                None
            };
            let mut events = vec![];
            let mut claims = vec![];
            for o in &ops {
                for v in &o.body.as_ref().unwrap().records {
                    match parsed(v)? {
                        c::Record::TaskEvent(e) => events.push(e),
                        c::Record::TaskReceipt(_) | c::Record::Handoff(_) => claims.push(v.clone()),
                        _ => (),
                    }
                }
            }
            events.sort_by(|a, b| (a.step, &a.event_id).cmp(&(b.step, &b.event_id)));
            let fold = fold_task(&tid, &events, Some(&owner), &[])?;
            let status = if missing || conflict || reminder_heads.len() > 1 {
                "CONFLICT_REVIEW".into()
            } else {
                match fold.state {
                    FoldState::Transition(t) => {
                        serde_json::to_value(t).unwrap().as_str().unwrap().into()
                    }
                    other => format!("REVIEW_{other:?}"),
                }
            };
            tasks.push(TaskView {
                spec,
                events,
                draft,
                snooze_until_ms: snooze,
                deleted: false,
                status,
                execute_on_hydration: false,
                owner_replica_id: Some(owner),
                owner_epoch: 1,
                remote_claims: claims,
            });
        }
        Ok(View {
            revision: ledger.mutations.len() as u64 + all.len() as u64,
            replica_id: ledger.replica_id.clone(),
            agent,
            persona,
            tasks,
            pending_mutations: self.operations.get(&ledger.replica_id).map_or(0, Vec::len),
            sync_status: "SHARED_V2_MANUAL_ONLY".into(),
            conflicts,
            archived_mutations: ledger.mutations.len(),
        })
    }
}
fn heads<'a>(ops: &[&'a SharedOperation]) -> Vec<&'a SharedOperation> {
    ops.iter()
        .copied()
        .filter(|a| {
            !ops.iter().any(|b| {
                a.operation_id != b.operation_id
                    && b.context.get(&a.replica_id).copied().unwrap_or(0) >= a.sequence
            })
        })
        .collect()
}
impl Ledger {
    /// Explicit user-approved migration only. Original serialized v1 records are never rebound.
    pub fn adopt(&mut self, identity: SharedIdentity, confirmed_archive: bool) -> c::Result<()> {
        check(
            confirmed_archive,
            "Explicit archive/adoption confirmation required",
        )?;
        check(
            identity.replicas.contains_key(&self.replica_id),
            "Local replica absent",
        )?;
        if let Some(s) = &self.shared {
            return check(
                s.identity == identity,
                "Already adopted another identity; no overwrite",
            );
        }
        self.view()?;
        let mut next = self.clone();
        next.version = 2;
        next.shared = Some(Shared {
            identity,
            operations: BTreeMap::new(),
            local_requests: vec![],
        });
        next.bytes()?;
        *self = next;
        Ok(())
    }
    pub fn import_shared(&mut self, replica: &str, ops: &[SharedOperation]) -> c::Result<()> {
        check(
            replica != self.replica_id,
            "Cannot import own replica history",
        )?;
        let mut next = self.clone();
        next.shared
            .as_mut()
            .ok_or("Explicit adoption required")?
            .receive(replica, ops)?;
        next.bytes()?;
        *self = next;
        Ok(())
    }
    pub fn outbound_len(&self) -> usize {
        self.shared.as_ref().map_or(self.mutations.len(), |s| {
            s.operations.get(&self.replica_id).map_or(0, Vec::len)
        })
    }
}
