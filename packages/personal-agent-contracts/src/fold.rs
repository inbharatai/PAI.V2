//! Pure conservative projection of an immutable task log. No timestamps/LWW,
//! effects, queues, ownership negotiation, authentication or replay are performed.
use crate::{authority::VerifiedReceipt, *};
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoldState {
    Empty,
    WaitingForOwner,
    MissingPredecessor,
    Conflict,
    Transition(TaskTransition),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProjection {
    pub state: FoldState,
    pub event_ids: Vec<String>,
    /// ALWAYS false, including a native-verified historical receipt.
    pub execute_on_hydration: bool,
}
/// `owner` must be a separately resolved authenticated assignment, not a guess
/// taken from the newest event. Forks/ID collisions/cycles fail closed for review.
pub fn fold_task(
    task_id: &str,
    events: &[TaskEvent],
    owner: Option<&str>,
    verified: &[VerifiedReceipt],
) -> Result<TaskProjection> {
    ensure(events.len() <= 1024, "event batch limit")?;
    let mut ids: BTreeMap<&str, &TaskEvent> = BTreeMap::new();
    let mut ops: BTreeMap<&str, &TaskEvent> = BTreeMap::new();
    let mut conflict = false;
    for event in events {
        Document::new(Record::TaskEvent(event.clone()))?;
        ensure(event.task_id == task_id, "mixed task batch")?;
        if ids
            .get(event.event_id.as_str())
            .is_some_and(|old| **old != *event)
            || ops
                .get(event.operation_id.as_str())
                .is_some_and(|old| **old != *event)
        {
            conflict = true;
        }
        ids.insert(&event.event_id, event);
        ops.insert(&event.operation_id, event);
    }
    let result = |state| TaskProjection {
        state,
        event_ids: ids.keys().map(|s| s.to_string()).collect(),
        execute_on_hydration: false,
    };
    if conflict {
        return Ok(result(FoldState::Conflict));
    }
    if ids.is_empty() {
        return Ok(result(FoldState::Empty));
    }
    let Some(owner) = owner else {
        return Ok(result(FoldState::WaitingForOwner));
    };
    if ids
        .values()
        .any(|e| e.assigned_replica_id.as_deref() != Some(owner) || e.origin_replica_id != owner)
    {
        return Ok(result(FoldState::WaitingForOwner));
    }
    let roots: Vec<_> = ids
        .values()
        .filter(|e| e.predecessor_event_id.is_none())
        .collect();
    if ids.values().any(|e| {
        e.predecessor_event_id
            .as_ref()
            .is_some_and(|p| !ids.contains_key(p.as_str()))
    }) {
        return Ok(result(FoldState::MissingPredecessor));
    }
    if roots.len() != 1 || roots[0].transition != TaskTransition::Planned {
        return Ok(result(FoldState::Conflict));
    }
    let mut current = *roots[0];
    let mut visited = 1;
    loop {
        let next: Vec<_> = ids
            .values()
            .filter(|e| e.predecessor_event_id.as_deref() == Some(&current.event_id))
            .collect();
        if next.is_empty() {
            break;
        }
        if next.len() != 1 {
            return Ok(result(FoldState::Conflict));
        }
        let child = *next[0];
        if child.step != current.step + 1
            || child.deadline_ms != current.deadline_ms
            || !allowed(current.transition, child.transition)
        {
            return Ok(result(FoldState::Conflict));
        }
        current = child;
        visited += 1;
        if visited > ids.len() {
            return Ok(result(FoldState::Conflict));
        }
    }
    if visited != ids.len() {
        return Ok(result(FoldState::Conflict));
    }
    let status = if current.transition == TaskTransition::Verified
        && !verified.iter().any(|r| {
            let r = r.receipt();
            r.task_id == task_id
                && r.operation_id == current.operation_id
                && r.replica_id == owner
                && current
                    .evidence_ref
                    .as_ref()
                    .is_some_and(|e| r.after_evidence_refs.contains(e))
                && current.external_object_id == r.external_object_id
        }) {
        TaskTransition::AwaitingVerification
    } else {
        current.transition
    };
    Ok(result(FoldState::Transition(status)))
}
fn allowed(from: TaskTransition, to: TaskTransition) -> bool {
    use TaskTransition::*;
    if matches!(from, Verified | Cancelled) {
        return false;
    }
    if matches!(to, Cancelled | Blocked) {
        return true;
    }
    matches!(
        (from, to),
        (Planned, WaitingForAccess | Drafted | InProgress)
            | (WaitingForAccess, Drafted | ReadyForReview | InProgress)
            | (Drafted, ReadyForReview)
            | (ReadyForReview, InProgress)
            | (InProgress, AwaitingVerification)
            | (AwaitingVerification, Verified)
            | (Blocked, WaitingForAccess | ReadyForReview)
    )
}
