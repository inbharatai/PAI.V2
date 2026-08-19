mod common;

use common::TempDir;
use inbharat_harness_core::Value;
use inbharat_harness_core::error::HarnessResult;
use inbharat_harness_core::session::{EventData, SessionStore};
use std::fs::{self, OpenOptions};
use std::io::Write;

#[test]
fn durable_resume_fork_and_replay() -> HarnessResult<()> {
    let temp = TempDir::new("sessions")?;
    let store = SessionStore::open(temp.path())?;
    let mut session = store.create()?;
    session.append(EventData::TurnStart { turn: 1 })?;
    session.append(EventData::UserMessage {
        message_id: "u1".to_owned(),
        content: "hello".to_owned(),
    })?;
    session.append(EventData::StepStart {
        turn: 1,
        step: 1,
        attempt: 1,
    })?;
    session.append(EventData::RequestHeader {
        request_id: "r1".to_owned(),
        provider: "mock".to_owned(),
        model: "mock-v1".to_owned(),
        tools: Vec::new(),
        system: String::new(),
    })?;
    session.append(EventData::AssistantMessage {
        message_id: "a1".to_owned(),
        content: "hi".to_owned(),
        finish: "stop".to_owned(),
    })?;
    session.append(EventData::StepEnd {
        reason: "complete".to_owned(),
    })?;
    let boundary = session
        .append(EventData::TurnEnd {
            reason: "complete".to_owned(),
        })?
        .seq;
    session.checkpoint()?;
    let revision = u64::try_from(session.events().len()).unwrap_or(u64::MAX);
    let resumed = store.resume(session.id())?;
    assert_eq!(resumed.events().len(), session.events().len());
    assert!(resumed.replay()?.balanced);
    let fork = store.fork(&session, boundary, revision)?;
    assert_eq!(fork.events().len(), 2);
    assert!(matches!(
        fork.events()[1].data,
        EventData::UserMessage { .. }
    ));
    Ok(())
}

#[test]
fn interrupted_tool_is_repaired_without_reexecution() -> HarnessResult<()> {
    let temp = TempDir::new("repair")?;
    let store = SessionStore::open(temp.path())?;
    let mut session = store.create()?;
    session.append(EventData::TurnStart { turn: 1 })?;
    session.append(EventData::StepStart {
        turn: 1,
        step: 1,
        attempt: 1,
    })?;
    session.append(EventData::ToolCall {
        call_id: "call-unknown".to_owned(),
        tool_id: "fs.write".to_owned(),
        arguments: Value::Null,
    })?;
    let id = session.id().clone();
    drop(session);
    let mut resumed = store.resume(&id)?;
    let synthesized = resumed.recover(16)?;
    assert_eq!(synthesized, 4);
    assert!(resumed.replay()?.balanced);
    assert!(resumed.events().iter().any(|event| matches!(
        event.data,
        EventData::ToolResult {
            synthesized: true,
            ..
        }
    )));
    Ok(())
}

#[test]
fn interrupted_approval_is_closed_as_unavailable() -> HarnessResult<()> {
    let mut session = inbharat_harness_core::Session::in_memory()?;
    session.append(EventData::TurnStart { turn: 1 })?;
    session.append(EventData::StepStart {
        turn: 1,
        step: 1,
        attempt: 1,
    })?;
    session.append(EventData::ApprovalAsked {
        request_id: "approval-1".to_owned(),
        actor: "local-user".to_owned(),
        action: "fs.write".to_owned(),
        risk: "write".to_owned(),
    })?;
    assert_eq!(session.recover(8)?, 4);
    assert!(session.events().iter().any(|event| matches!(
        &event.data,
        EventData::ApprovalDecided { outcome, .. } if outcome == "unavailable"
    )));
    assert!(session.replay()?.balanced);
    Ok(())
}

#[test]
fn torn_final_record_is_dropped_before_resume() -> HarnessResult<()> {
    let temp = TempDir::new("torn-record")?;
    let store = SessionStore::open(temp.path())?;
    let session = store.create()?;
    let id = session.id().clone();
    let path = temp.path().join(format!("{}.jsonl", id));
    let valid_len = fs::metadata(&path)
        .map_err(|error| inbharat_harness_core::Failure::invalid("test.meta", error.to_string()))?
        .len();
    drop(session);
    let mut file = OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|error| inbharat_harness_core::Failure::invalid("test.open", error.to_string()))?;
    file.write_all(b"{\"format\":1").map_err(|error| {
        inbharat_harness_core::Failure::invalid("test.write", error.to_string())
    })?;
    drop(file);
    let resumed = store.resume(&id)?;
    assert_eq!(resumed.events().len(), 1);
    let repaired_len = fs::metadata(&path)
        .map_err(|error| inbharat_harness_core::Failure::invalid("test.meta", error.to_string()))?
        .len();
    assert_eq!(repaired_len, valid_len);
    Ok(())
}

#[test]
fn checksum_tampering_is_detected() -> HarnessResult<()> {
    let temp = TempDir::new("tamper")?;
    let store = SessionStore::open(temp.path())?;
    let session = store.create()?;
    let path = temp.path().join(format!("{}.jsonl", session.id()));
    let original = fs::read_to_string(&path)
        .map_err(|error| inbharat_harness_core::Failure::invalid("test.read", error.to_string()))?;
    let tampered = original.replace("session.started", "session.changed");
    fs::write(&path, tampered).map_err(|error| {
        inbharat_harness_core::Failure::invalid("test.write", error.to_string())
    })?;
    assert!(store.resume(session.id()).is_err());
    Ok(())
}
