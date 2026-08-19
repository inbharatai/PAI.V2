mod common;

use common::TempDir;
use inbharat_harness_core::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use inbharat_harness_core::providers::{
    FinishReason, MockModelProvider, MockStep, ModelChunk, ModelProvider, ModelRequest,
    ModelResponse,
};
use inbharat_harness_core::routing::ExecutionLevel;
use inbharat_harness_core::runtime::{HarnessBuilder, RunOptions};
use inbharat_harness_core::{CancelCause, CancellationToken};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[test]
fn retryable_provider_failure_has_bounded_recovery() -> HarnessResult<()> {
    let temp = TempDir::new("retry")?;
    let harness = HarnessBuilder::local(temp.path())?
        .register_model(Arc::new(MockModelProvider::new([
            MockStep::RetryableFailure("transient".to_owned()),
            MockStep::Text("recovered".to_owned()),
        ])))?
        .build();
    let options = RunOptions {
        explicit_level: Some(ExecutionLevel::L0),
        provider: "mock".to_owned(),
        model: "mock-v1".to_owned(),
        recovery_attempts: 1,
        ..RunOptions::default()
    };
    let (outcome, session) = harness.run("hello", &options, &CancellationToken::new())?;
    assert_eq!(outcome.output, "recovered");
    assert_eq!(outcome.steps, 1);
    assert!(session.replay()?.balanced);
    assert_eq!(harness.metrics().snapshot().recoveries, 1);
    Ok(())
}

struct ZeroByteChunkFloodProvider;

impl ModelProvider for ZeroByteChunkFloodProvider {
    fn id(&self) -> &str {
        "zero-chunk-flood"
    }

    fn models(&self) -> Vec<String> {
        vec!["zero-chunk-v1".to_owned()]
    }

    fn stream(
        &self,
        _request: &ModelRequest,
        _cancel: &CancellationToken,
        sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
    ) -> HarnessResult<ModelResponse> {
        for block in 0..20_000_u32 {
            sink(ModelChunk::Start { block })?;
        }
        Ok(ModelResponse {
            text: "unreachable".to_owned(),
            finish: FinishReason::Stop,
            input_units: 0,
            output_units: 0,
            provider_request_id: None,
        })
    }
}

#[test]
fn zero_byte_model_chunk_flood_is_bounded() -> HarnessResult<()> {
    let temp = TempDir::new("chunk-flood")?;
    let harness = HarnessBuilder::local(temp.path())?
        .register_model(Arc::new(ZeroByteChunkFloodProvider))?
        .build();
    let options = RunOptions {
        explicit_level: Some(ExecutionLevel::L0),
        provider: "zero-chunk-flood".to_owned(),
        model: "zero-chunk-v1".to_owned(),
        ..RunOptions::default()
    };
    let failure = harness
        .run("hello", &options, &CancellationToken::new())
        .err()
        .ok_or_else(|| Failure::invalid("test.chunk_flood", "chunk flood succeeded"))?;
    assert_eq!(failure.code, ErrorCode::BudgetExceeded);
    Ok(())
}

#[test]
fn unbounded_recovery_configuration_is_rejected() -> HarnessResult<()> {
    let temp = TempDir::new("unbounded-recovery")?;
    let harness = HarnessBuilder::local(temp.path())?.build();
    let options = RunOptions {
        explicit_level: Some(ExecutionLevel::L0),
        recovery_attempts: u32::MAX,
        ..RunOptions::default()
    };
    let failure = harness
        .run("hello", &options, &CancellationToken::new())
        .err()
        .ok_or_else(|| Failure::invalid("test.recovery", "unbounded recovery was accepted"))?;
    assert_eq!(failure.code, ErrorCode::InvalidInput);
    Ok(())
}

#[test]
fn model_wait_is_cancelled_and_joined() -> HarnessResult<()> {
    let temp = TempDir::new("cancel")?;
    let harness = HarnessBuilder::local(temp.path())?
        .register_model(Arc::new(MockModelProvider::new([
            MockStep::WaitForCancellation,
        ])))?
        .build();
    let options = RunOptions {
        explicit_level: Some(ExecutionLevel::L0),
        provider: "mock".to_owned(),
        model: "mock-v1".to_owned(),
        ..RunOptions::default()
    };
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let join = thread::spawn(move || harness.run("wait", &options, &worker_cancel));
    thread::sleep(Duration::from_millis(25));
    assert!(cancel.cancel(CancelCause::User));
    let result = join.join().map_err(|_| {
        Failure::new(
            ErrorCode::Internal,
            FailureClass::Internal,
            "test.cancel",
            "worker panicked",
        )
    })?;
    let failure = result.err().ok_or_else(|| {
        Failure::new(
            ErrorCode::Internal,
            FailureClass::Internal,
            "test.cancel",
            "cancelled run unexpectedly succeeded",
        )
    })?;
    assert_eq!(failure.code, ErrorCode::Cancelled);
    Ok(())
}
