//! Separate device-local encrypted provider state in existing vault-core. NEVER sync
//! this record: tokens, OAuth config and prepared execution intent are local authority.
//! The existing personal runtime receives only inert selected-task draft/receipt notes.
use crate::{
    ensure,
    oauth::{OAuthConfig, Tokens},
    *,
};
use serde::{Deserialize, Serialize};
use unoone_personal_agent_runtime as runtime;
use unoone_vault_core::{Record, RecordType, Vault};

pub const RECORD_ID: &str = "f856bc30-2860-4a47-9f9d-0e86e90e875b";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalState {
    pub version: u32,
    pub vault_id: String,
    pub config: Option<OAuthConfig>,
    pub tokens: Option<Tokens>,
    pub entries: Vec<Entry>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub review: Review,
    pub digest: String,
    pub status: CommitStatus,
    pub receipt: Option<ProviderReceipt>,
}
#[derive(Serialize)]
pub struct SourcesView {
    pub status: String,
    pub account: Option<MailAccount>,
    pub scopes: Vec<String>,
    pub configured: bool,
    pub prepared: Vec<Entry>,
    pub qualification: &'static str,
}
impl LocalState {
    pub fn view(&self) -> SourcesView {
        SourcesView { status:if self.config.is_none(){"UNCONFIGURED"}else if self.tokens.is_none(){"DISCONNECTED"}else{"CONNECTED_NOT_QUALIFIED"}.into(), account:self.tokens.as_ref().map(|t|MailAccount{email:t.account().into(),provider:"GOOGLE".into()}), scopes:self.tokens.as_ref().map(|t|t.scopes().to_vec()).unwrap_or_default(),configured:self.config.is_some(),prepared:self.entries.clone(),qualification:"No live account qualification performed. Tokens/grants never sync; provider content is untrusted data." }
    }
    pub fn prepare(&mut self, review: Review, now: u64) -> Result<()> {
        review.validate(now)?;
        ensure(
            self.entries.len() < 128,
            "Prepared ledger full; no pending entries evicted",
        )?;
        ensure(
            !self
                .entries
                .iter()
                .any(|e| e.review.operation_id == review.operation_id),
            "Operation already exists; reload/reconcile, never replay",
        )?;
        ensure(!self.entries.iter().any(|e|e.review.task_id==review.task_id && e.status==CommitStatus::NeedsReconciliation), "This task has an uncertain provider attempt; reconcile before preparing another action")?;
        if let Some(tokens) = &self.tokens {
            ensure(
                review.account == tokens.account(),
                "Connected account does not match review",
            )?;
        }
        let digest = review.digest()?;
        self.entries.push(Entry {
            review,
            digest,
            status: CommitStatus::Prepared,
            receipt: None,
        });
        Ok(())
    }
    pub fn begin_commit(
        &mut self,
        operation: &str,
        digest: &str,
        now: u64,
    ) -> Result<(Review, CapabilityGrant)> {
        let account = self
            .tokens
            .as_ref()
            .ok_or("No authorized account")?
            .account()
            .to_string();
        let entry = self
            .entries
            .iter_mut()
            .find(|e| e.review.operation_id == operation)
            .ok_or("Prepared operation missing")?;
        ensure(
            entry.status == CommitStatus::Prepared,
            "Operation already attempted; NEEDS_RECONCILIATION, never automatically retry",
        )?;
        let grant = CapabilityGrant::from_native_review(&entry.review, digest, now)?;
        grant.check(&entry.review, &account, now)?;
        entry.status = CommitStatus::NeedsReconciliation; // caller saves BEFORE network.
        Ok((entry.review.clone(), grant))
    }
    pub fn finish(&mut self, receipt: ProviderReceipt) -> Result<()> {
        let entry = self
            .entries
            .iter_mut()
            .find(|e| e.review.operation_id == receipt.operation_id)
            .ok_or("Operation missing")?;
        ensure(
            entry.status == CommitStatus::NeedsReconciliation
                && receipt.task_id == entry.review.task_id
                && receipt.account == entry.review.account
                && receipt.container == entry.review.container,
            "Receipt binding mismatch",
        )?;
        entry.status = receipt.status.clone();
        entry.receipt = Some(receipt);
        Ok(())
    }
}
pub fn load(vault: &mut Vault) -> Result<LocalState> {
    let vault_id = vault.vault_id().ok_or("Unlock local vault")?.to_string();
    let path = vault
        .vault_root()
        .join("VAULT/records")
        .join(format!("{RECORD_ID}.enc.json"));
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure(
                meta.is_file() && meta.len() <= 4 * 1024 * 1024,
                "Provider state envelope invalid; retained without reset",
            )?;
            let (record, mut bytes) = vault
                .read_record(RECORD_ID)
                .map_err(|_| "Cannot authenticate provider state; retained without reset")?;
            ensure(
                !record.tombstone && bytes.len() <= 1024 * 1024,
                "Provider state tombstoned or oversized",
            )?;
            let state: Result<LocalState> = serde_json::from_slice(&bytes)
                .map_err(|_| "Provider state malformed; retained".into());
            bytes.fill(0);
            let state = state?;
            ensure(
                state.version == 1 && state.vault_id == vault_id && state.entries.len() <= 128,
                "Provider state version/binding/bound mismatch",
            )?;
            Ok(state)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LocalState {
            version: 1,
            vault_id,
            config: None,
            tokens: None,
            entries: vec![],
        }),
        Err(_) => Err("Provider state inaccessible; no reset".into()),
    }
}
pub fn save(vault: &mut Vault, state: &LocalState) -> Result<()> {
    ensure(
        vault.vault_id() == Some(&state.vault_id),
        "Wrong local vault",
    )?;
    let mut bytes = serde_json::to_vec(state).map_err(|_| "Provider state encoding failed")?;
    ensure(
        bytes.len() <= 1024 * 1024 && state.entries.len() <= 128,
        "Provider ledger bound; nothing evicted",
    )?;
    let mut record = Record::new(RecordType::ContextSnapshot, "provider-local", "local");
    record.record_id = RECORD_ID.into();
    let result = vault
        .write_record(record, &bytes)
        .map_err(|_| "Provider state write failed; do not retry mutation".into());
    bytes.fill(0);
    result
}
/// Existing API, not a fork of task state. The provider record is committed first;
/// an interrupted task-note write is repaired by explicit reload/reconcile. Imported
/// notes remain declarative and cannot create a grant or a VERIFIED native task.
pub fn task_note(vault: &mut Vault, review: &Review, detail: &str) -> Result<()> {
    let mut ledger = runtime::load(vault)?;
    let view = ledger.view()?;
    let task = view
        .tasks
        .iter()
        .find(|t| t.spec.task_id == review.task_id && !t.deleted)
        .ok_or("Select an existing non-deleted task")?;
    ensure(
        view.conflicts.is_empty()
            && view.replica_id == review.owner_replica
            && task
                .owner_replica_id
                .as_ref()
                .is_none_or(|o| o == &view.replica_id)
            && task.status != "CANCELLED",
        "Task is conflicted, cancelled or owned by another replica; no execution",
    )?;
    if detail.starts_with("NEEDS_RECONCILIATION") {
        ensure(
            task.draft.contains(&review.operation_id) && task.draft.contains(&review.digest()?),
            "Task changed after preparation; stale review cannot dispatch",
        )?;
    }
    let note=format!("Provider operation {} · {} · {} · {}\n{}\nThis task note is declarative, not an execution grant. Full review/credentials remain device-local.",review.operation_id,review.account,review.container,review.digest()?,detail);
    ledger.apply(
        runtime::Request {
            operation_id: uuid::Uuid::new_v4().to_string(),
            expected_revision: view.revision,
            expected_replica_id: view.replica_id,
            action: runtime::Action::Edit,
            task_id: Some(review.task_id.clone()),
            text: task.spec.goal.clone(),
            draft: note,
            snooze_until_ms: None,
        },
        crate::now_ms(),
    )?;
    runtime::save(vault, &ledger)
}
