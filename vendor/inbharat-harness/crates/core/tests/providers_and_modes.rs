mod common;

use common::TempDir;
use inbharat_harness_core::error::{Failure, HarnessResult};
use inbharat_harness_core::jobs::{SubagentProvider, run_scoped_subagent};
use inbharat_harness_core::providers::{
    AttachmentMetadata, Capability, CapabilitySet, CredentialRef, EchoModelProvider, FinishReason,
    ModelChunk, ModelMessage, ModelProvider, ModelRegistry, ModelRequest, ModelResponse, ModelRole,
    ReplayEntry, ReplayModelProvider,
};
use inbharat_harness_core::routing::ExecutionLevel;
use inbharat_harness_core::runtime::{HarnessBuilder, RunOptions, TrajectoryMode};
use inbharat_harness_core::{CancellationToken, SubagentRequest, SubagentResult};
use std::sync::{Arc, Mutex};

#[test]
fn prepared_model_call_is_one_shot() -> HarnessResult<()> {
    let mut registry = ModelRegistry::new();
    registry.register(Arc::new(EchoModelProvider::default()))?;
    let mut prepared = registry.prepare(ModelRequest {
        request_id: "request-1".to_owned(),
        provider: "echo".to_owned(),
        model: "echo-v1".to_owned(),
        system: String::new(),
        messages: vec![ModelMessage {
            role: ModelRole::User,
            content: "hello".to_owned(),
        }],
        tools: Vec::new(),
        attachments: Vec::new(),
        max_output_bytes: 1024,
    })?;
    let token = CancellationToken::new();
    let mut sink = |_chunk| Ok(());
    let first = prepared.stream(&token, &mut sink)?;
    assert_eq!(first.text, "echo: hello");
    assert!(prepared.stream(&token, &mut sink).is_err());
    Ok(())
}

#[test]
fn replay_provider_binds_by_explicit_request_id() -> HarnessResult<()> {
    let provider = ReplayModelProvider::new([ReplayEntry {
        request_id: "recorded-7".to_owned(),
        chunks: vec![
            ModelChunk::TextDelta {
                block: 0,
                text: "recorded".to_owned(),
            },
            ModelChunk::Finish {
                reason: FinishReason::Stop,
            },
        ],
        response: ModelResponse {
            text: "recorded".to_owned(),
            finish: FinishReason::Stop,
            input_units: 1,
            output_units: 1,
            provider_request_id: Some("source-7".to_owned()),
        },
    }])?;
    let mut registry = ModelRegistry::new();
    registry.register(Arc::new(provider))?;
    let request = |request_id: &str| ModelRequest {
        request_id: request_id.to_owned(),
        provider: "replay".to_owned(),
        model: "recorded".to_owned(),
        system: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        attachments: Vec::new(),
        max_output_bytes: 1024,
    };
    let mut prepared = registry.prepare(request("recorded-7"))?;
    let mut chunks = Vec::new();
    let response = prepared.stream(&CancellationToken::new(), &mut |chunk| {
        chunks.push(chunk);
        Ok(())
    })?;
    assert_eq!(response.text, "recorded");
    assert_eq!(chunks.len(), 2);
    let mut missing = registry.prepare(request("different-request"))?;
    assert!(
        missing
            .stream(&CancellationToken::new(), &mut |_chunk| Ok(()))
            .is_err()
    );
    Ok(())
}

#[test]
fn trajectory_modes_scale_durable_chunk_detail() -> HarnessResult<()> {
    let temp = TempDir::new("trajectory")?;
    let harness = HarnessBuilder::local(temp.path())?
        .register_model(Arc::new(EchoModelProvider::default()))?
        .build();
    let minimal = RunOptions {
        explicit_level: Some(ExecutionLevel::L0),
        trajectory: TrajectoryMode::Minimal,
        provider: "echo".to_owned(),
        model: "echo-v1".to_owned(),
        ..RunOptions::default()
    };
    let diagnostic = RunOptions {
        explicit_level: Some(ExecutionLevel::L0),
        trajectory: TrajectoryMode::Diagnostic,
        provider: "echo".to_owned(),
        model: "echo-v1".to_owned(),
        ..RunOptions::default()
    };
    let (_, minimal_session) =
        harness.run("mode comparison", &minimal, &CancellationToken::new())?;
    let (_, diagnostic_session) =
        harness.run("mode comparison", &diagnostic, &CancellationToken::new())?;
    assert!(diagnostic_session.events().len() > minimal_session.events().len());
    assert!(minimal_session.replay()?.balanced);
    assert!(diagnostic_session.replay()?.balanced);
    Ok(())
}

#[test]
fn attachment_metadata_and_credentials_are_reference_only() -> HarnessResult<()> {
    let attachment = AttachmentMetadata {
        id: "blob-1".to_owned(),
        media_type: "text/plain".to_owned(),
        byte_len: 12,
        digest: "fnv-test".to_owned(),
        display_name: Some("note.txt".to_owned()),
    };
    attachment.validate()?;
    let reference = CredentialRef::new("local", "provider-key", "model")?;
    assert_eq!(reference.redacted(), "local://model/***");
    assert!(!format!("{reference:?}").contains("secret-value"));
    Ok(())
}

#[derive(Clone, Default)]
struct HistoryProvider {
    requests: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
}

impl ModelProvider for HistoryProvider {
    fn id(&self) -> &str {
        "history"
    }

    fn models(&self) -> Vec<String> {
        vec!["history-v1".to_owned()]
    }

    fn stream(
        &self,
        request: &ModelRequest,
        _cancel: &CancellationToken,
        sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
    ) -> HarnessResult<ModelResponse> {
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| Failure::invalid("test.history", "history lock poisoned"))?;
        requests.push(request.messages.clone());
        let text = format!("turn-{}", requests.len());
        drop(requests);
        sink(ModelChunk::TextDelta {
            block: 0,
            text: text.clone(),
        })?;
        sink(ModelChunk::Finish {
            reason: FinishReason::Stop,
        })?;
        Ok(ModelResponse {
            text,
            finish: FinishReason::Stop,
            input_units: 1,
            output_units: 1,
            provider_request_id: None,
        })
    }
}

#[test]
fn resumed_turns_receive_bounded_prior_conversation_history() -> HarnessResult<()> {
    let temp = TempDir::new("history")?;
    let provider = HistoryProvider::default();
    let captured = Arc::clone(&provider.requests);
    let harness = HarnessBuilder::local(temp.path())?
        .register_model(Arc::new(provider))?
        .build();
    let options = RunOptions {
        explicit_level: Some(ExecutionLevel::L0),
        provider: "history".to_owned(),
        model: "history-v1".to_owned(),
        capabilities: CapabilitySet::from_slice(&[Capability::Model]),
        ..RunOptions::default()
    };
    let mut session = inbharat_harness_core::Session::in_memory()?;
    let first = harness.run_in_session(
        &mut session,
        "first question",
        &options,
        &CancellationToken::new(),
    )?;
    assert_eq!(first.output, "turn-1");
    let second = harness.run_in_session(
        &mut session,
        "second question",
        &options,
        &CancellationToken::new(),
    )?;
    assert_eq!(second.output, "turn-2");
    let requests = captured
        .lock()
        .map_err(|_| Failure::invalid("test.history", "history lock poisoned"))?;
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1],
        vec![
            ModelMessage {
                role: ModelRole::User,
                content: "first question".to_owned(),
            },
            ModelMessage {
                role: ModelRole::Assistant,
                content: "turn-1".to_owned(),
            },
            ModelMessage {
                role: ModelRole::User,
                content: "second question".to_owned(),
            },
        ]
    );
    Ok(())
}

struct EchoSubagent;

impl SubagentProvider for EchoSubagent {
    fn run(
        &self,
        request: &SubagentRequest,
        _cancel: &CancellationToken,
    ) -> HarnessResult<SubagentResult> {
        Ok(SubagentResult {
            child_id: "child-1".to_owned(),
            output: request.prompt.clone(),
            failure: None,
        })
    }
}

#[test]
fn subagent_authority_can_only_narrow_parent() -> HarnessResult<()> {
    let parent = CapabilitySet::from_slice(&[Capability::Model, Capability::FileRead]);
    let allowed = SubagentRequest {
        prompt: "one shot".to_owned(),
        parent_id: "parent".to_owned(),
        depth: 1,
        max_depth: 1,
        capabilities: CapabilitySet::from_slice(&[Capability::Model]),
        max_output_bytes: 100,
    };
    let result = run_scoped_subagent(&EchoSubagent, &allowed, &parent, &CancellationToken::new())?;
    assert_eq!(result.output, "one shot");
    let denied = SubagentRequest {
        capabilities: CapabilitySet::from_slice(&[Capability::ProcessSpawn]),
        ..allowed
    };
    assert!(
        run_scoped_subagent(&EchoSubagent, &denied, &parent, &CancellationToken::new(),).is_err()
    );
    Ok(())
}

#[test]
fn l0_model_call_still_requires_model_capability() -> HarnessResult<()> {
    let temp = TempDir::new("no-model")?;
    let harness = HarnessBuilder::local(temp.path())?.build();
    let options = RunOptions {
        explicit_level: Some(ExecutionLevel::L0),
        capabilities: CapabilitySet::new(),
        ..RunOptions::default()
    };
    let failure = harness
        .run("hello", &options, &CancellationToken::new())
        .err()
        .ok_or_else(|| Failure::invalid("test", "model call unexpectedly succeeded"))?;
    assert_eq!(
        failure.code,
        inbharat_harness_core::ErrorCode::CapabilityUnavailable
    );
    Ok(())
}

#[test]
fn credential_constructor_rejects_ambient_syntax() {
    let result = CredentialRef::new("env", "../../secret", "model");
    assert!(result.is_err());
    let _type_check: Option<Failure> = result.err();
}
