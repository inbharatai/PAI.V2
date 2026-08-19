//! Owner-scoped background jobs and optional one-shot subagent contract.

use crate::cancel::{CancelCause, CancellationToken};
use crate::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use crate::providers::CapabilitySet;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

static JOB_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Opaque process-local job identity.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct JobId(String);

impl JobId {
    fn generate() -> Self {
        let id = JOB_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(format!("j-{:08x}-{id:016x}", std::process::id()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// First-wins job state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Bounded owner-authorized job view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobSnapshot {
    pub id: JobId,
    pub owner: String,
    pub kind: String,
    pub status: JobStatus,
    pub output: String,
    pub failure: Option<Failure>,
}

struct JobRecord {
    state: Arc<Mutex<JobSnapshot>>,
    cancel: CancellationToken,
    join: Mutex<Option<JoinHandle<()>>>,
}

/// Process-local registry. It deliberately does not claim durable resume semantics.
pub struct JobRegistry {
    records: Mutex<BTreeMap<JobId, Arc<JobRecord>>>,
    max_jobs: usize,
    max_output_bytes: usize,
}

impl std::fmt::Debug for JobRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.records.lock().map_or(0, |records| records.len());
        formatter
            .debug_struct("JobRegistry")
            .field("count", &count)
            .field("max_jobs", &self.max_jobs)
            .finish_non_exhaustive()
    }
}

impl JobRegistry {
    #[must_use]
    pub fn new(max_jobs: usize, max_output_bytes: usize) -> Self {
        Self {
            records: Mutex::new(BTreeMap::new()),
            max_jobs: max_jobs.max(1),
            max_output_bytes: max_output_bytes.max(1),
        }
    }

    /// Starts an owned job. The worker owns no ambient authority beyond its closure.
    pub fn start<F>(&self, owner: &str, kind: &str, operation: F) -> HarnessResult<JobId>
    where
        F: FnOnce(CancellationToken) -> HarnessResult<String> + Send + 'static,
    {
        let mut records = self
            .records
            .lock()
            .map_err(|_| lock_failure("jobs.start"))?;
        if records.len() >= self.max_jobs {
            return Err(Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Resource,
                "jobs.start",
                "background job capacity exhausted",
            ));
        }
        let id = JobId::generate();
        let state = Arc::new(Mutex::new(JobSnapshot {
            id: id.clone(),
            owner: owner.to_owned(),
            kind: kind.to_owned(),
            status: JobStatus::Pending,
            output: String::new(),
            failure: None,
        }));
        let cancel = CancellationToken::new();
        let worker_state = Arc::clone(&state);
        let worker_cancel = cancel.clone();
        let max_output_bytes = self.max_output_bytes;
        let join = thread::Builder::new()
            .name(format!("inbharat-{}", id.as_str()))
            .spawn(move || {
                if let Ok(mut snapshot) = worker_state.lock() {
                    snapshot.status = JobStatus::Running;
                }
                let result = operation(worker_cancel.clone());
                if let Ok(mut snapshot) = worker_state.lock() {
                    if worker_cancel.is_cancelled() {
                        snapshot.status = JobStatus::Cancelled;
                        return;
                    }
                    match result {
                        Ok(mut output) => {
                            truncate_utf8(&mut output, max_output_bytes);
                            snapshot.output = output;
                            snapshot.status = JobStatus::Completed;
                        }
                        Err(failure) => {
                            snapshot.failure = Some(failure);
                            snapshot.status = JobStatus::Failed;
                        }
                    }
                }
            })
            .map_err(|error| {
                Failure::new(
                    ErrorCode::ToolFailed,
                    FailureClass::Execution,
                    "jobs.start",
                    "cannot spawn job worker",
                )
                .with_detail("io_kind", format!("{:?}", error.kind()))
            })?;
        let record = Arc::new(JobRecord {
            state,
            cancel,
            join: Mutex::new(Some(join)),
        });
        records.insert(id.clone(), record);
        Ok(id)
    }

    /// Reads an owner-scoped snapshot.
    pub fn snapshot(&self, owner: &str, id: &JobId) -> HarnessResult<JobSnapshot> {
        let record = self.authorized_record(owner, id)?;
        let snapshot = record
            .state
            .lock()
            .map_err(|_| lock_failure("jobs.snapshot"))?
            .clone();
        Ok(snapshot)
    }

    /// Lists only jobs owned by the caller.
    pub fn list(&self, owner: &str) -> HarnessResult<Vec<JobSnapshot>> {
        let records = self.records.lock().map_err(|_| lock_failure("jobs.list"))?;
        let mut result = Vec::new();
        for record in records.values() {
            let snapshot = record.state.lock().map_err(|_| lock_failure("jobs.list"))?;
            if snapshot.owner == owner {
                result.push(snapshot.clone());
            }
        }
        Ok(result)
    }

    /// Cancels and joins one job before reporting it stopped.
    pub fn cancel_and_join(&self, owner: &str, id: &JobId) -> HarnessResult<JobSnapshot> {
        let record = self.authorized_record(owner, id)?;
        record.cancel.cancel(CancelCause::User);
        join_record(&record, Duration::from_secs(1))?;
        let snapshot = self.snapshot(owner, id)?;
        self.remove_record(id)?;
        Ok(snapshot)
    }

    /// Joins a naturally completed job and returns its final state, then releases its record.
    pub fn join(&self, owner: &str, id: &JobId) -> HarnessResult<JobSnapshot> {
        let record = self.authorized_record(owner, id)?;
        join_record(&record, Duration::from_secs(30))?;
        let snapshot = self.snapshot(owner, id)?;
        self.remove_record(id)?;
        Ok(snapshot)
    }

    /// Cancels and joins every job owned by one principal.
    pub fn dispose_owner(&self, owner: &str) -> HarnessResult<usize> {
        let records: Vec<(JobId, Arc<JobRecord>)> = {
            let records = self
                .records
                .lock()
                .map_err(|_| lock_failure("jobs.dispose"))?;
            records
                .iter()
                .filter_map(|(id, record)| {
                    let snapshot = record.state.lock().ok()?;
                    (snapshot.owner == owner).then(|| (id.clone(), Arc::clone(record)))
                })
                .collect()
        };
        for (_id, record) in &records {
            record.cancel.cancel(CancelCause::Disposed);
            join_record(record, Duration::from_secs(1))?;
        }
        let mut registry = self
            .records
            .lock()
            .map_err(|_| lock_failure("jobs.dispose"))?;
        for (id, _record) in &records {
            registry.remove(id);
        }
        Ok(records.len())
    }

    fn remove_record(&self, id: &JobId) -> HarnessResult<()> {
        self.records
            .lock()
            .map_err(|_| lock_failure("jobs.remove"))?
            .remove(id);
        Ok(())
    }

    fn authorized_record(&self, owner: &str, id: &JobId) -> HarnessResult<Arc<JobRecord>> {
        let records = self
            .records
            .lock()
            .map_err(|_| lock_failure("jobs.authorize"))?;
        let record = records.get(id).cloned().ok_or_else(|| {
            Failure::new(
                ErrorCode::NotFound,
                FailureClass::User,
                "jobs.authorize",
                "job not found",
            )
        })?;
        let authorized = record
            .state
            .lock()
            .map_err(|_| lock_failure("jobs.authorize"))?
            .owner
            == owner;
        if !authorized {
            return Err(Failure::new(
                ErrorCode::PermissionDenied,
                FailureClass::Policy,
                "jobs.authorize",
                "job belongs to another owner",
            ));
        }
        Ok(record)
    }
}

impl Drop for JobRegistry {
    fn drop(&mut self) {
        if let Ok(records) = self.records.get_mut() {
            for record in records.values() {
                record.cancel.cancel(CancelCause::Shutdown);
                let _quiesced = join_record(record, Duration::from_secs(1));
            }
        }
    }
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self::new(32, 256 * 1024)
    }
}

/// One-shot child request with inherited, narrowed authority.
#[derive(Clone, Debug)]
pub struct SubagentRequest {
    pub prompt: String,
    pub parent_id: String,
    pub depth: u8,
    pub max_depth: u8,
    pub capabilities: CapabilitySet,
    pub max_output_bytes: usize,
}

/// Child-level failures are data, not transport errors.
#[derive(Clone, Debug)]
pub struct SubagentResult {
    pub child_id: String,
    pub output: String,
    pub failure: Option<Failure>,
}

/// Optional one-shot subagent seam.
pub trait SubagentProvider: Send + Sync {
    fn run(
        &self,
        request: &SubagentRequest,
        cancel: &CancellationToken,
    ) -> HarnessResult<SubagentResult>;
}

/// Validates inherited authority and depth before invoking a provider.
pub fn run_scoped_subagent(
    provider: &dyn SubagentProvider,
    request: &SubagentRequest,
    parent_capabilities: &CapabilitySet,
    cancel: &CancellationToken,
) -> HarnessResult<SubagentResult> {
    if request.depth == 0
        || request.depth > request.max_depth
        || !request.capabilities.is_subset_of(parent_capabilities)
        || request.max_output_bytes == 0
    {
        return Err(Failure::new(
            ErrorCode::PermissionDenied,
            FailureClass::Policy,
            "subagent.start",
            "subagent depth or capability scope is invalid",
        ));
    }
    cancel.check("subagent.start")?;
    let mut result = provider.run(request, cancel)?;
    truncate_utf8(&mut result.output, request.max_output_bytes);
    Ok(result)
}

fn join_record(record: &JobRecord, timeout: Duration) -> HarnessResult<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let finished = record
            .join
            .lock()
            .map_err(|_| lock_failure("jobs.join"))?
            .as_ref()
            .is_none_or(JoinHandle::is_finished);
        if finished {
            break;
        }
        if Instant::now() >= deadline {
            return Err(Failure::new(
                ErrorCode::Timeout,
                FailureClass::Resource,
                "jobs.join",
                "job did not quiesce before the owner deadline",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
    let join = record
        .join
        .lock()
        .map_err(|_| lock_failure("jobs.join"))?
        .take();
    if let Some(join) = join {
        join.join().map_err(|_| {
            Failure::new(
                ErrorCode::ToolFailed,
                FailureClass::Execution,
                "jobs.join",
                "job worker panicked",
            )
        })?;
    }
    Ok(())
}

fn lock_failure(operation: &str) -> Failure {
    Failure::new(
        ErrorCode::Internal,
        FailureClass::Internal,
        operation,
        "job registry lock poisoned",
    )
}

fn truncate_utf8(value: &mut String, max: usize) {
    if value.len() <= max {
        return;
    }
    let mut boundary = max;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_are_owner_scoped() -> HarnessResult<()> {
        let jobs = JobRegistry::new(2, 100);
        let id = jobs.start("alice", "echo", |_cancel| Ok("done".to_owned()))?;
        let final_state = jobs.join("alice", &id)?;
        assert_eq!(final_state.status, JobStatus::Completed);
        assert!(jobs.snapshot("bob", &id).is_err());
        assert!(jobs.list("alice")?.is_empty());
        Ok(())
    }

    #[test]
    fn cancelled_job_quiesces_and_releases_its_record() -> HarnessResult<()> {
        let jobs = JobRegistry::new(2, 100);
        let id = jobs.start("alice", "wait", |cancel| {
            let _cause = cancel.wait_cancelled(Duration::from_secs(1));
            Ok("stopped".to_owned())
        })?;
        let final_state = jobs.cancel_and_join("alice", &id)?;
        assert_eq!(final_state.status, JobStatus::Cancelled);
        assert!(jobs.list("alice")?.is_empty());
        Ok(())
    }
}
