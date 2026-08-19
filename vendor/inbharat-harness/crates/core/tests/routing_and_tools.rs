mod common;

use common::{AllowPermission, ConfirmYes, GrantSandbox, TempDir};
use inbharat_harness_core::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use inbharat_harness_core::providers::{
    CanonicalVerificationProvider, Capability, CapabilitySet, EnforcementQuality,
    MockModelProvider, MockStep, VerificationProvider,
};
use inbharat_harness_core::routing::{
    EscalationCause, ExecutionLevel, RoutePolicy, RouteRequest, Router,
};
use inbharat_harness_core::runtime::{HarnessBuilder, RunOptions, TrajectoryMode};
use inbharat_harness_core::{
    CancellationToken, EventData, LocalExecutionBroker, RootedFs, Session, Value,
};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn routes_all_four_levels_deterministically() -> HarnessResult<()> {
    let router = Router;
    let cases = [
        ("hello", ExecutionLevel::L0),
        ("read file README.md", ExecutionLevel::L1),
        ("research and compare these two formats", ExecutionLevel::L2),
        (
            "build a complete website in the workspace and test it",
            ExecutionLevel::L3,
        ),
    ];
    for (prompt, expected) in cases {
        let decision = router.route(&RouteRequest::new(prompt), RoutePolicy::default())?;
        assert_eq!(decision.level, expected);
    }
    Ok(())
}

#[test]
fn escalation_is_monotonic_and_one_level_only() -> HarnessResult<()> {
    let router = Router;
    let l0 = router.route(&RouteRequest::new("hello"), RoutePolicy::default())?;
    let l1 = router.escalate(
        &l0,
        EscalationCause::NeedsSingleAction,
        RoutePolicy::default(),
    )?;
    assert_eq!(l1.level, ExecutionLevel::L1);
    assert!(
        router
            .escalate(&l0, EscalationCause::NeedsWorkspace, RoutePolicy::default())
            .is_err()
    );
    Ok(())
}

#[test]
fn mock_model_selects_visible_read_tool_then_completes() -> HarnessResult<()> {
    let temp = TempDir::new("tool-loop")?;
    fs::write(temp.path().join("note.txt"), "bounded content").map_err(|error| {
        inbharat_harness_core::Failure::invalid("test.write", error.to_string())
    })?;
    let filesystem = RootedFs::new(temp.path())?;
    let execution = Arc::new(LocalExecutionBroker::new(filesystem, Vec::<String>::new()));
    let capabilities = CapabilitySet::from_slice(&[Capability::Model, Capability::FileRead]);
    let harness = HarnessBuilder::new(execution)?
        .register_model(Arc::new(MockModelProvider::new([
            MockStep::ToolCall {
                call_id: "call-1".to_owned(),
                tool_id: "fs.read".to_owned(),
                arguments: "{\"path\":\"note.txt\"}".to_owned(),
            },
            MockStep::Text("verified bounded content".to_owned()),
        ])))?
        .permission_provider(Arc::new(AllowPermission))
        .confirmation_provider(Arc::new(ConfirmYes))
        .verification_provider(Arc::new(CanonicalVerificationProvider))
        .sandbox_provider(Arc::new(GrantSandbox {
            granted: capabilities.clone(),
            quality: EnforcementQuality::InProcessFence,
        }))
        .build();
    let options = RunOptions {
        explicit_level: Some(ExecutionLevel::L2),
        provider: "mock".to_owned(),
        model: "mock-v1".to_owned(),
        capabilities,
        trajectory: TrajectoryMode::Diagnostic,
        ..RunOptions::default()
    };
    let (outcome, session) = harness.run(
        "use the tools to inspect the note",
        &options,
        &CancellationToken::new(),
    )?;
    assert_eq!(outcome.output, "verified bounded content");
    assert_eq!(outcome.tool_calls, 1);
    assert_eq!(outcome.steps, 2);
    assert!(session.replay()?.balanced);
    Ok(())
}

#[test]
fn l1_executes_exactly_one_action() -> HarnessResult<()> {
    let temp = TempDir::new("l1")?;
    fs::write(temp.path().join("one.txt"), "one").map_err(|error| {
        inbharat_harness_core::Failure::invalid("test.write", error.to_string())
    })?;
    let harness = HarnessBuilder::local(temp.path())?.build();
    let options = RunOptions::default();
    let (outcome, _session) =
        harness.run("read file one.txt", &options, &CancellationToken::new())?;
    assert_eq!(outcome.decision.level, ExecutionLevel::L1);
    assert_eq!(outcome.tool_calls, 1);
    assert_eq!(outcome.steps, 1);
    assert_eq!(outcome.output, "one");
    Ok(())
}

struct RetryGoalVerifier {
    calls: AtomicUsize,
}

impl VerificationProvider for RetryGoalVerifier {
    fn verify(&self, tool_id: &str, _arguments: &Value, _output: &Value) -> HarnessResult<()> {
        if tool_id == "goal.complete" && self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(Failure::new(
                ErrorCode::VerificationFailed,
                FailureClass::Execution,
                "goal.verify",
                "first goal round is incomplete",
            )
            .retryable(None));
        }
        Ok(())
    }
}

#[test]
fn l3_goal_loop_continues_after_retryable_verification_failure() -> HarnessResult<()> {
    let temp = TempDir::new("l3-rounds")?;
    let harness = HarnessBuilder::local(temp.path())?
        .register_model(Arc::new(MockModelProvider::new([
            MockStep::Text("draft".to_owned()),
            MockStep::Text("verified final".to_owned()),
        ])))?
        .confirmation_provider(Arc::new(ConfirmYes))
        .verification_provider(Arc::new(RetryGoalVerifier {
            calls: AtomicUsize::new(0),
        }))
        .build();
    let options = RunOptions {
        explicit_level: Some(ExecutionLevel::L3),
        provider: "mock".to_owned(),
        model: "mock-v1".to_owned(),
        capabilities: CapabilitySet::from_slice(&[Capability::Model, Capability::Workspace]),
        ..RunOptions::default()
    };
    let (outcome, session) = harness.run(
        "build a complete verified workspace result",
        &options,
        &CancellationToken::new(),
    )?;
    assert_eq!(outcome.output, "verified final");
    assert_eq!(outcome.steps, 2);
    assert_eq!(harness.metrics().snapshot().recoveries, 1);
    assert!(
        session
            .events()
            .iter()
            .any(|event| matches!(event.data, EventData::ApprovalAsked { .. }))
    );
    assert!(session.replay()?.balanced);
    Ok(())
}

struct WebsiteVerifier {
    root: PathBuf,
}

impl VerificationProvider for WebsiteVerifier {
    fn verify(&self, tool_id: &str, _arguments: &Value, _output: &Value) -> HarnessResult<()> {
        if tool_id != "goal.complete" {
            return Ok(());
        }
        let html = fs::read_to_string(self.root.join("index.html")).map_err(|error| {
            Failure::new(
                ErrorCode::VerificationFailed,
                FailureClass::Execution,
                "website.verify",
                error.to_string(),
            )
        })?;
        let css = fs::read_to_string(self.root.join("style.css")).map_err(|error| {
            Failure::new(
                ErrorCode::VerificationFailed,
                FailureClass::Execution,
                "website.verify",
                error.to_string(),
            )
        })?;
        if !html.contains("viewport")
            || !html.contains("style.css")
            || !css.contains("@media")
            || !css.contains("display:grid")
        {
            return Err(Failure::new(
                ErrorCode::VerificationFailed,
                FailureClass::Execution,
                "website.verify",
                "generated website did not satisfy responsive build checks",
            ));
        }
        Ok(())
    }
}

#[test]
fn l3_website_creation_uses_scoped_tools_and_verifies_output() -> HarnessResult<()> {
    let temp = TempDir::new("l3-website")?;
    let execution = Arc::new(LocalExecutionBroker::new(
        RootedFs::new(temp.path())?,
        Vec::<String>::new(),
    ));
    let capabilities = CapabilitySet::from_slice(&[
        Capability::Model,
        Capability::Workspace,
        Capability::FileWrite,
    ]);
    let harness = HarnessBuilder::new(execution)?
        .register_model(Arc::new(MockModelProvider::new([
            MockStep::ToolCall {
                call_id: "write-index".to_owned(),
                tool_id: "fs.write".to_owned(),
                arguments: r#"{"path":"index.html","content":"<!doctype html><html><head><meta name='viewport' content='width=device-width,initial-scale=1'><link rel='stylesheet' href='style.css'></head><body><main>InBharat</main></body></html>"}"#.to_owned(),
            },
            MockStep::ToolCall {
                call_id: "write-style".to_owned(),
                tool_id: "fs.write".to_owned(),
                arguments: r#"{"path":"style.css","content":"main{display:grid}@media(max-width:600px){main{display:block}}"}"#.to_owned(),
            },
            MockStep::Text("responsive website created and verified".to_owned()),
        ])))?
        .permission_provider(Arc::new(AllowPermission))
        .confirmation_provider(Arc::new(ConfirmYes))
        .verification_provider(Arc::new(WebsiteVerifier {
            root: temp.path().to_path_buf(),
        }))
        .sandbox_provider(Arc::new(GrantSandbox {
            granted: capabilities.clone(),
            quality: EnforcementQuality::InProcessFence,
        }))
        .build();
    let options = RunOptions {
        explicit_level: Some(ExecutionLevel::L3),
        provider: "mock".to_owned(),
        model: "mock-v1".to_owned(),
        capabilities,
        trajectory: TrajectoryMode::Diagnostic,
        ..RunOptions::default()
    };
    let (outcome, session) = harness.run(
        "create a responsive website in the controlled workspace and verify it",
        &options,
        &CancellationToken::new(),
    )?;
    assert_eq!(outcome.output, "responsive website created and verified");
    assert_eq!(outcome.tool_calls, 2);
    assert_eq!(outcome.steps, 3);
    assert!(temp.path().join("index.html").is_file());
    assert!(temp.path().join("style.css").is_file());
    assert!(session.replay()?.balanced);
    Ok(())
}

#[test]
fn repeated_l1_turns_keep_unique_correlations() -> HarnessResult<()> {
    let temp = TempDir::new("l1-repeated")?;
    fs::write(temp.path().join("one.txt"), "one").map_err(|error| {
        inbharat_harness_core::Failure::invalid("test.write", error.to_string())
    })?;
    let harness = HarnessBuilder::local(temp.path())?.build();
    let mut session = Session::in_memory()?;
    for _turn in 0..2 {
        harness.run_in_session(
            &mut session,
            "read file one.txt",
            &RunOptions::default(),
            &CancellationToken::new(),
        )?;
    }
    assert!(session.replay()?.balanced);
    Ok(())
}

#[test]
fn mutating_l1_action_has_exactly_one_audited_confirmation() -> HarnessResult<()> {
    let temp = TempDir::new("l1-confirm")?;
    let execution = Arc::new(LocalExecutionBroker::new(
        RootedFs::new(temp.path())?,
        Vec::<String>::new(),
    ));
    let capabilities = CapabilitySet::from_slice(&[
        Capability::Model,
        Capability::FileRead,
        Capability::FileWrite,
    ]);
    let harness = HarnessBuilder::new(execution)?
        .permission_provider(Arc::new(AllowPermission))
        .confirmation_provider(Arc::new(ConfirmYes))
        .sandbox_provider(Arc::new(GrantSandbox {
            granted: capabilities.clone(),
            quality: EnforcementQuality::InProcessFence,
        }))
        .build();
    let options = RunOptions {
        capabilities,
        ..RunOptions::default()
    };
    let (outcome, session) = harness.run(
        "write file confirmed.txt approved",
        &options,
        &CancellationToken::new(),
    )?;
    assert_eq!(outcome.tool_calls, 1);
    let written = fs::read_to_string(temp.path().join("confirmed.txt"))
        .map_err(|error| inbharat_harness_core::Failure::invalid("test.read", error.to_string()))?;
    assert_eq!(written, "approved");
    let asked = session
        .events()
        .iter()
        .filter(|event| matches!(event.data, EventData::ApprovalAsked { .. }))
        .count();
    let decided = session
        .events()
        .iter()
        .filter(|event| matches!(event.data, EventData::ApprovalDecided { .. }))
        .count();
    assert_eq!(asked, 1);
    assert_eq!(decided, 1);
    assert!(session.replay()?.balanced);
    Ok(())
}

#[test]
fn unavailable_confirmation_fails_closed_and_is_audited() -> HarnessResult<()> {
    let temp = TempDir::new("l1-no-confirm")?;
    let execution = Arc::new(LocalExecutionBroker::new(
        RootedFs::new(temp.path())?,
        Vec::<String>::new(),
    ));
    let capabilities = CapabilitySet::from_slice(&[
        Capability::Model,
        Capability::FileRead,
        Capability::FileWrite,
    ]);
    let harness = HarnessBuilder::new(execution)?
        .permission_provider(Arc::new(AllowPermission))
        .sandbox_provider(Arc::new(GrantSandbox {
            granted: capabilities.clone(),
            quality: EnforcementQuality::InProcessFence,
        }))
        .build();
    let options = RunOptions {
        capabilities,
        ..RunOptions::default()
    };
    let mut session = Session::in_memory()?;
    let result = harness.run_in_session(
        &mut session,
        "write file denied.txt blocked",
        &options,
        &CancellationToken::new(),
    );
    assert!(result.is_err());
    assert!(!temp.path().join("denied.txt").exists());
    assert!(session.events().iter().any(|event| matches!(
        &event.data,
        EventData::ApprovalDecided { outcome, .. } if outcome == "unavailable"
    )));
    assert!(session.replay()?.balanced);
    Ok(())
}
