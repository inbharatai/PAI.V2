//! Bounded opt-in peer-review lane. The local personal ledger is the sole outbox.
//! Foreign histories are retained, NEVER rewritten into the local linear ledger.
pub mod json_guard;
pub mod tls;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use unoone_personal_agent_contracts as c;
use unoone_personal_agent_runtime::{Action, Ledger};
use unoone_vault_core::{Record, RecordType, Vault};

pub type Result<T> = std::result::Result<T, String>;
pub const STORE_ID: &str = "98e180e1-d876-44c7-b069-e83e6b703070";
pub const MAX_BODY: usize = 262_144;
pub const MAX_STORE: usize = 4 * 1024 * 1024;
pub const PAGE: usize = 8;
pub fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn uuid(s: &str) -> Result<()> {
    require(
        uuid::Uuid::parse_str(s).is_ok_and(|v| v.to_string() == s),
        "Invalid identity",
    )
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub version: u32,
    pub replica_id: String,
    pub person_id: String,
    pub agent_id: String,
    pub fingerprint: String,
}
impl Offer {
    pub fn validate(&self) -> Result<()> {
        require(self.version == 1, "Unsupported pairing version")?;
        uuid(&self.replica_id)?;
        uuid(&self.person_id)?;
        uuid(&self.agent_id)?;
        require(
            self.fingerprint.len() == 64
                && self
                    .fingerprint
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "Compare complete lowercase SHA256 fingerprint",
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub persona: bool,
    pub task_ids: Vec<String>,
}
impl Selection {
    fn validate(&self) -> Result<()> {
        require(
            self.task_ids.len() <= 128
                && self.task_ids.iter().collect::<BTreeSet<_>>().len() == self.task_ids.len(),
            "Selection limit/duplicate",
        )?;
        for id in &self.task_ids {
            uuid(id)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IdentityChoice {
    SamePerson,
    KeepSeparateReview,
    UnifyArchive,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub offer: Offer,
    pub choice: IdentityChoice,
    pub selection: Selection,
    pub revoked: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskState {
    pub task_id: String,
    pub action: Action,
    pub draft: String,
    pub snooze_until_ms: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub records: Vec<Value>,
    pub task_state: Option<TaskState>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub sequence: u64,
    pub operation_id: String,
    pub predecessor_operation_id: Option<String>,
    // Exact UTF-8 payload bytes are hashed. No bespoke signing/encryption: TLS1.3 authenticates this envelope.
    pub payload: Option<String>,
    pub content_hash: String,
}
impl Change {
    fn validate(&self, owner: &Offer) -> Result<()> {
        uuid(&self.operation_id)?;
        if let Some(p) = &self.predecessor_operation_id {
            uuid(p)?;
        }
        require(self.sequence > 0 && self.sequence <= 2048, "Sequence limit")?;
        require(
            self.content_hash == hash(self.payload.as_deref().unwrap_or("").as_bytes()),
            "Payload hash mismatch",
        )?;
        if let Some(raw) = &self.payload {
            require(raw.len() <= 65536, "Change too large")?;
            json_guard::preflight(raw.as_bytes(), 65536)?;
            if raw.contains("\"context\"") {
                let op: unoone_personal_agent_runtime::shared::SharedOperation =
                    serde_json::from_str(raw).map_err(|_| "Invalid shared operation")?;
                require(
                    op.replica_id == owner.replica_id
                        && op.sequence == self.sequence
                        && op.operation_id == self.operation_id
                        && op.predecessor_operation_id == self.predecessor_operation_id,
                    "Shared envelope mismatch",
                )?;
                return Ok(());
            }
            let p: Payload = serde_json::from_str(raw).map_err(|_| "Invalid payload")?;
            require(p.records.len() <= 8, "Record limit")?;
            for v in &p.records {
                let r = c::decode(&serde_json::to_vec(v).map_err(|_| "Invalid record")?)?.record;
                let valid = match r {
                    c::Record::PersonalAgent(a) => {
                        a.person_id == owner.person_id && a.agent_id == owner.agent_id
                    }
                    c::Record::Persona(p) => {
                        p.person_id == owner.person_id
                            && p.provenance.replica_id == owner.replica_id
                    }
                    c::Record::TaskSpec(s) => {
                        s.person_id == owner.person_id
                            && s.agent_id == owner.agent_id
                            && s.origin_replica_id == owner.replica_id
                            && s.target_replica_id.as_deref() == Some(&owner.replica_id)
                    }
                    c::Record::TaskEvent(e) => {
                        e.origin_replica_id == owner.replica_id
                            && e.assigned_replica_id.as_deref() == Some(&owner.replica_id)
                    }
                    _ => false,
                };
                require(
                    valid,
                    "Unsupported record or owner mismatch; no grants/provider data",
                )?;
            }
            if let Some(t) = &p.task_state {
                uuid(&t.task_id)?;
                require(
                    t.draft.len() <= 4096
                        && !matches!(t.action, Action::Persona | Action::ClearPersona),
                    "Invalid task state",
                )?;
                require(
                    t.snooze_until_ms.is_none_or(|v| v <= 9_007_199_254_740_991),
                    "Time limit",
                )?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub version: u32,
    pub sender: Offer,
    pub choice: IdentityChoice,
    pub after: u64,
    pub changes: Vec<Change>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exchange {
    pub page: Page,
    pub want_after: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub page: Page,
    pub acknowledged: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub version: u32,
    pub vault_id: String,
    pub local: Offer,
    pub certificate: Vec<u8>,
    pub private_key: Vec<u8>,
    pub peer: Option<Approval>,
    pub received: Vec<Change>,
    pub sent_ack: u64,
    #[serde(default, skip_serializing_if = "unconfirmed")]
    pub peer_confirmed: bool,
}
fn unconfirmed(value: &bool) -> bool {
    !*value
}
impl Drop for State {
    fn drop(&mut self) {
        self.private_key.fill(0);
    }
}
#[derive(Serialize)]
pub struct Status {
    pub offer: Offer,
    pub peer: Option<Offer>,
    pub revoked: bool,
    pub received: usize,
    pub sent_ack: u64,
    pub pending: u64,
    pub message: String,
    pub tasks: Vec<RemoteTask>,
    pub persona_review: Option<String>,
}
#[derive(Serialize)]
pub struct RemoteTask {
    pub task_id: String,
    pub goal: String,
    pub draft: String,
    pub snooze_until_ms: Option<u64>,
    pub deleted: bool,
    pub status: String,
    pub execute_on_hydration: bool,
    pub events: Vec<c::TaskEvent>,
}
impl State {
    pub fn fresh(ledger: &Ledger) -> Result<Self> {
        let v = ledger.view()?;
        let (certificate, private_key) = tls::identity()?;
        Ok(Self {
            version: 1,
            vault_id: ledger.local_vault_id.clone(),
            local: Offer {
                version: 1,
                replica_id: v.replica_id,
                person_id: v.agent.person_id,
                agent_id: v.agent.agent_id,
                fingerprint: hash(&certificate),
            },
            certificate,
            private_key,
            peer: None,
            received: vec![],
            sent_ack: 0,
            peer_confirmed: false,
        })
    }
    pub fn approve(
        &mut self,
        offer: Offer,
        selection: Selection,
        choice: IdentityChoice,
        compared: bool,
        ledger: &Ledger,
    ) -> Result<()> {
        require(
            compared,
            "Compare the entire fingerprint on both screens first",
        )?;
        require(
            self.peer.is_none(),
            "Peer already bound; no silent replacement or history reset",
        )?;
        offer.validate()?;
        selection.validate()?;
        require(
            offer.replica_id != self.local.replica_id
                && offer.fingerprint != self.local.fingerprint,
            "Replicas/keys must be independent",
        )?;
        require(choice != IdentityChoice::SamePerson || (offer.person_id == self.local.person_id && offer.agent_id == self.local.agent_id), "IDENTITY_CONFLICT: independent identities; keep separate for review or cancel. Adoption requires reviewed migration; nothing overwritten")?;
        let known: BTreeSet<_> = ledger
            .mutations
            .iter()
            .filter_map(|m| m.request.as_ref().and_then(|r| r.task_id.clone()))
            .collect();
        require(
            selection.task_ids.iter().all(|i| {
                known.contains(i)
                    || (choice == IdentityChoice::UnifyArchive
                        && i == "00000000-0000-0000-0000-000000000000")
            }),
            "Select existing tasks only",
        )?;
        self.peer = Some(Approval {
            offer,
            selection,
            choice,
            revoked: false,
        });
        Ok(())
    }
    pub fn active(&self) -> Result<&Approval> {
        let p = self
            .peer
            .as_ref()
            .ok_or("Approve peer fingerprint on both screens first")?;
        require(
            !p.revoked,
            "Peer revoked; previous copies cannot be remotely erased",
        )?;
        Ok(p)
    }
    pub fn shared_identity(&self) -> Result<unoone_personal_agent_runtime::shared::SharedIdentity> {
        let peer = self.active()?;
        require(
            peer.choice == IdentityChoice::UnifyArchive,
            "Not an adoption pairing",
        )?;
        let founder = if self.local.replica_id < peer.offer.replica_id {
            &self.local
        } else {
            &peer.offer
        };
        Ok(unoone_personal_agent_runtime::shared::SharedIdentity {
            person_id: founder.person_id.clone(),
            agent_id: founder.agent_id.clone(),
            founder_replica_id: founder.replica_id.clone(),
            replicas: [
                (
                    self.local.replica_id.clone(),
                    self.local.fingerprint.clone(),
                ),
                (
                    peer.offer.replica_id.clone(),
                    peer.offer.fingerprint.clone(),
                ),
            ]
            .into_iter()
            .collect(),
        })
    }
    /// Must be persisted before saving receive cursor/ACK. Replay repairs an interrupted second write.
    pub fn merge_into(&self, ledger: &mut Ledger) -> Result<()> {
        if self.active()?.choice != IdentityChoice::UnifyArchive {
            return Ok(());
        }
        require(
            self.peer_confirmed,
            "Authenticated matching confirmation from both screens required",
        )?;
        let mut candidate = ledger.clone();
        let target = ledger;
        let ledger = &mut candidate;
        let identity = self.shared_identity()?;
        if ledger.shared.is_none() {
            ledger.adopt(identity.clone(), true)?;
        }
        require(
            ledger
                .shared
                .as_ref()
                .is_some_and(|s| s.identity == identity),
            "Explicit matching identity adoption required",
        )?;
        let ops = self
            .received
            .iter()
            .map(|c| {
                serde_json::from_str(c.payload.as_deref().ok_or("Missing causal marker")?)
                    .map_err(|_| "Invalid shared payload")
            })
            .collect::<std::result::Result<Vec<_>, &str>>()?;
        ledger.import_shared(&self.active()?.offer.replica_id, &ops)?;
        *target = candidate;
        Ok(())
    }
    pub fn page(&self, ledger: &Ledger, after: u64) -> Result<Page> {
        if self.active()?.choice == IdentityChoice::UnifyArchive {
            require(
                ledger.local_vault_id == self.vault_id
                    && ledger.replica_id == self.local.replica_id,
                "Local binding changed",
            )?;
            let identity = self.shared_identity()?;
            let mut staged = ledger.clone();
            if staged.shared.is_none() {
                staged.adopt(identity.clone(), true)?;
            }
            let ledger = &staged;
            let shared = ledger.shared.as_ref().ok_or("Explicit adoption required")?;
            require(shared.identity == identity, "Shared identity mismatch")?;
            ledger.view()?;
            let empty = vec![];
            let ops = shared.operations.get(&ledger.replica_id).unwrap_or(&empty);
            require(after <= ops.len() as u64, "Cursor ahead of shared history")?;
            let selection = &self.active()?.selection;
            let mut changes = vec![];
            for op in ops.iter().skip(after as usize).take(PAGE) {
                let mut wire = op.clone();
                let selected = op.body.as_ref().is_some_and(|b| {
                    b.task_id.as_ref().map_or(selection.persona, |id| {
                        selection.task_ids.contains(id)
                            || selection
                                .task_ids
                                .iter()
                                .any(|id| id == "00000000-0000-0000-0000-000000000000")
                    })
                });
                if !selected {
                    wire.body = None;
                }
                let raw = serde_json::to_string(&wire).map_err(|_| "Shared encoding")?;
                let ch = Change {
                    sequence: op.sequence,
                    operation_id: op.operation_id.clone(),
                    predecessor_operation_id: op.predecessor_operation_id.clone(),
                    content_hash: hash(raw.as_bytes()),
                    payload: Some(raw),
                };
                ch.validate(&self.local)?;
                changes.push(ch);
                if serde_json::to_vec(&changes)
                    .map_err(|_| "Page encoding")?
                    .len()
                    > MAX_BODY / 2
                {
                    changes.pop();
                    break;
                }
            }
            require(
                after == ops.len() as u64 || !changes.is_empty(),
                "Single change exceeds page bound",
            )?;
            return Ok(Page {
                version: 2,
                sender: self.local.clone(),
                choice: IdentityChoice::UnifyArchive,
                after,
                changes,
            });
        }
        let p = self.active()?;
        let v = ledger.view()?;
        require(
            v.replica_id == self.local.replica_id
                && v.agent.person_id == self.local.person_id
                && ledger.local_vault_id == self.vault_id,
            "Local identity changed; stop sync",
        )?;
        require(
            after <= ledger.mutations.len() as u64,
            "Cursor ahead of authoritative ledger",
        )?;
        let mut changes = Vec::new();
        for m in ledger.mutations.iter().skip(after as usize).take(PAGE) {
            let selected = m
                .request
                .as_ref()
                .and_then(|r| r.task_id.as_ref())
                .map_or(p.selection.persona, |id| p.selection.task_ids.contains(id));
            let payload = if selected {
                Some(
                    serde_json::to_string(&Payload {
                        records: m.records.clone(),
                        task_state: m.request.as_ref().and_then(|r| {
                            r.task_id.as_ref().map(|tid| TaskState {
                                task_id: tid.clone(),
                                action: r.action.clone(),
                                draft: r.draft.clone(),
                                snooze_until_ms: r.snooze_until_ms,
                            })
                        }),
                    })
                    .map_err(|_| "Payload encoding")?,
                )
            } else {
                None
            };
            let ch = Change {
                sequence: m.sequence,
                operation_id: m.operation_id.clone(),
                predecessor_operation_id: m.predecessor_operation_id.clone(),
                content_hash: hash(payload.as_deref().unwrap_or("").as_bytes()),
                payload,
            };
            ch.validate(&self.local)?;
            changes.push(ch);
            if serde_json::to_vec(&changes)
                .map_err(|_| "Page encoding")?
                .len()
                > MAX_BODY / 2
            {
                changes.pop();
                break;
            }
        }
        require(
            after == ledger.mutations.len() as u64 || !changes.is_empty(),
            "Single change exceeds page bound",
        )?;
        Ok(Page {
            version: 1,
            sender: self.local.clone(),
            choice: p.choice.clone(),
            after,
            changes,
        })
    }
    /// Pure candidate update. Host MUST save the returned state before sending an ACK.
    pub fn receive(&self, page: &Page) -> Result<Self> {
        let p = self.active()?;
        require(
            page.version
                == if p.choice == IdentityChoice::UnifyArchive {
                    2
                } else {
                    1
                }
                && page.sender == p.offer
                && page.choice == p.choice,
            "Pairing/identity choice/version mismatch; review on both screens",
        )?;
        require(
            page.changes.len() <= PAGE && page.after <= self.received.len() as u64,
            "Gap or page bound",
        )?;
        let mut next = self.clone();
        next.peer_confirmed = true;
        for (offset, ch) in page.changes.iter().enumerate() {
            ch.validate(&p.offer)?;
            require(
                ch.sequence == page.after + offset as u64 + 1,
                "Sequence gap",
            )?;
            let index = ch.sequence as usize - 1;
            if let Some(old) = next.received.get(index) {
                require(old == ch, "Immutable history conflict; original retained")?;
                continue;
            }
            require(
                ch.predecessor_operation_id == next.received.last().map(|c| c.operation_id.clone()),
                "Missing predecessor",
            )?;
            require(
                !next
                    .received
                    .iter()
                    .any(|c| c.operation_id == ch.operation_id),
                "Operation collision",
            )?;
            next.received.push(ch.clone());
        }
        if p.choice != IdentityChoice::UnifyArchive {
            next.remote_tasks()?;
        } else {
            let ops = next
                .received
                .iter()
                .map(|c| {
                    serde_json::from_str(c.payload.as_deref().ok_or("Missing causal marker")?)
                        .map_err(|_| "Invalid shared payload")
                })
                .collect::<std::result::Result<Vec<_>, &str>>()?;
            unoone_personal_agent_runtime::shared::Shared {
                identity: next.shared_identity()?,
                operations: [(p.offer.replica_id.clone(), ops)].into_iter().collect(),
                local_requests: vec![],
            }
            .validate()?;
        }
        require(
            next.received.len() <= 2048
                && serde_json::to_vec(&next)
                    .map_err(|_| "Store encoding")?
                    .len()
                    <= MAX_STORE,
            "Peer review history full; nothing evicted",
        )?;
        Ok(next)
    }
    pub fn remote_tasks(&self) -> Result<Vec<RemoteTask>> {
        if self
            .peer
            .as_ref()
            .is_some_and(|p| p.choice == IdentityChoice::UnifyArchive)
        {
            return Ok(vec![]);
        }
        let mut tasks: BTreeMap<String, RemoteTask> = BTreeMap::new();
        for change in &self.received {
            if let Some(raw) = &change.payload {
                let payload: Payload =
                    serde_json::from_str(raw).map_err(|_| "Invalid stored payload")?;
                for value in payload.records {
                    match c::decode(&serde_json::to_vec(&value).map_err(|_| "Invalid record")?)?
                        .record
                    {
                        c::Record::TaskSpec(s) => {
                            let task = tasks.entry(s.task_id.clone()).or_insert(RemoteTask {
                                task_id: s.task_id,
                                goal: String::new(),
                                draft: String::new(),
                                snooze_until_ms: None,
                                deleted: false,
                                status: "PEER_REVIEW_ONLY".into(),
                                execute_on_hydration: false,
                                events: vec![],
                            });
                            if !task.deleted {
                                task.goal = s.goal;
                            }
                        }
                        c::Record::TaskEvent(e) => {
                            let t = tasks
                                .get_mut(&e.task_id)
                                .ok_or("Task event predecessor missing")?;
                            require(t.events.len() < 1024, "Event limit")?;
                            t.events.push(e);
                        }
                        _ => (),
                    }
                }
                if let Some(t) = payload.task_state {
                    let task = tasks
                        .get_mut(&t.task_id)
                        .ok_or("Task predecessor missing")?;
                    if matches!(t.action, Action::Delete) {
                        task.deleted = true;
                        task.goal.clear();
                        task.draft.clear();
                        task.snooze_until_ms = None;
                        task.status = "DELETED".into();
                    } else if !task.deleted {
                        match t.action {
                            Action::Create | Action::Edit => {
                                task.draft = t.draft;
                                task.snooze_until_ms = None;
                            }
                            Action::Snooze => task.snooze_until_ms = t.snooze_until_ms,
                            Action::Accept => task.snooze_until_ms = None,
                            _ => (),
                        }
                    }
                }
            }
        }
        for task in tasks.values_mut().filter(|t| !t.deleted) {
            let fold = c::fold::fold_task(
                &task.task_id,
                &task.events,
                self.peer.as_ref().map(|p| p.offer.replica_id.as_str()),
                &[],
            )?;
            task.status = match fold.state {
                c::fold::FoldState::Transition(t) => format!(
                    "PEER_REVIEW_ONLY / {}",
                    serde_json::to_value(t)
                        .map_err(|_| "Status encoding")?
                        .as_str()
                        .unwrap_or("UNKNOWN")
                ),
                other => format!("PEER_REVIEW_ONLY / {other:?}"),
            };
        }
        Ok(tasks.into_values().collect())
    }
    pub fn persona_review(&self) -> Result<Option<String>> {
        if self
            .peer
            .as_ref()
            .is_some_and(|p| p.choice == IdentityChoice::UnifyArchive)
        {
            return Ok(None);
        }
        let mut name = None;
        let mut persona = None;
        for change in &self.received {
            if let Some(raw) = &change.payload {
                json_guard::preflight(raw.as_bytes(), 65536)?;
                let p: Payload = serde_json::from_str(raw).map_err(|_| "Invalid payload")?;
                for value in p.records {
                    match c::decode(&serde_json::to_vec(&value).map_err(|_| "Record encoding")?)?
                        .record
                    {
                        c::Record::PersonalAgent(a) => name = Some(a.display_name),
                        c::Record::Persona(p) => persona = Some(p),
                        _ => (),
                    }
                }
            }
        }
        Ok(persona.map(|p| {
            if p.deleted {
                "Peer preferences cleared (tombstone). Local persona unchanged.".into()
            } else {
                format!(
                    "{} — {} (PEER_REVIEW_ONLY; local persona unchanged)",
                    name.unwrap_or_default(),
                    p.preferences
                        .iter()
                        .map(|p| p.value.clone())
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            }
        }))
    }
    pub fn status(&self, ledger: &Ledger) -> Result<Status> {
        Ok(Status {
            offer: self.local.clone(),
            peer: self.peer.as_ref().map(|p| p.offer.clone()),
            revoked: self.peer.as_ref().is_some_and(|p| p.revoked),
            received: self.received.len(),
            sent_ack: self.sent_ack,
            pending: (ledger.outbound_len() as u64).saturating_sub(self.sent_ack),
            message: if self
                .peer
                .as_ref()
                .is_some_and(|p| p.choice == IdentityChoice::UnifyArchive)
            {
                "SHARED_V2: active board uses causal union. Old local history archived, not rebound. Conflicts require review; receipt/handoff claims inert. No execution or automatic reminders.".into()
            } else {
                "PEER_REVIEW_ONLY: foreign histories remain separate, identity/persona/branch conflicts require reviewed migration. No local ledger overwrite, grant or execution. Fixed selection; no compaction/re-pair/reset. Revocation cannot erase previous copies.".into()
            },
            tasks: self.remote_tasks()?,
            persona_review: self.persona_review()?,
        })
    }
}
pub fn load(vault: &mut Vault, ledger: &Ledger) -> Result<State> {
    require(vault.is_unlocked(), "Unlock vault first")?;
    let path = vault
        .vault_root()
        .join(format!("VAULT/records/{STORE_ID}.enc.json"));
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let s = State::fresh(ledger)?;
            save(vault, &s)?;
            Ok(s)
        }
        Err(_) => Err("Peer store inaccessible; no reset".into()),
        Ok(m) => {
            require(
                m.file_type().is_file() && m.len() <= 10 * 1024 * 1024,
                "Invalid peer store envelope",
            )?;
            let (_, mut bytes) = vault
                .read_record(STORE_ID)
                .map_err(|_| "Peer store authentication failed; original retained")?;
            let result = (|| {
                require(bytes.len() <= MAX_STORE, "Peer store size")?;
                let s: State = serde_json::from_slice(&bytes)
                    .map_err(|_| "Peer store incompatible; no reset")?;
                require(
                    s.version == 1
                        && s.vault_id == ledger.local_vault_id
                        && s.local.replica_id == ledger.replica_id
                        && hash(&s.certificate) == s.local.fingerprint
                        && s.received.len() <= 2048,
                    "Peer store identity/version conflict",
                )?;
                s.local.validate()?;
                s.remote_tasks()?;
                Ok(s)
            })();
            bytes.fill(0);
            result
        }
    }
}
pub fn save(vault: &mut Vault, state: &State) -> Result<()> {
    require(
        vault.is_unlocked() && vault.vault_id() == Some(&state.vault_id),
        "Unlock matching local vault first",
    )?;
    let mut bytes = serde_json::to_vec(state).map_err(|_| "Store encoding")?;
    require(bytes.len() <= MAX_STORE, "Peer store full; nothing evicted")?;
    let mut r = Record::new(RecordType::ContextSnapshot, "peer-local", "local");
    r.record_id = STORE_ID.into();
    let result = vault
        .write_record(r, &bytes)
        .map_err(|_| "Peer store write failed; no ACK permitted".into());
    bytes.fill(0);
    result
}
