mod common;

use common::TempDir;
use inbharat_harness_core::error::{ErrorCode, Failure, HarnessResult};
use inbharat_harness_core::execution::{ExecutionBroker, LocalExecutionBroker, ProcessSpec};
use inbharat_harness_core::{CancellationToken, RootedFs};
use std::fs;

#[test]
fn lexical_traversal_and_absolute_paths_are_denied() -> HarnessResult<()> {
    let temp = TempDir::new("traversal")?;
    fs::write(temp.path().join("inside.txt"), "inside").map_err(|error| {
        inbharat_harness_core::Failure::invalid("test.write", error.to_string())
    })?;
    let rooted = RootedFs::new(temp.path())?;
    assert!(rooted.read_text("../inside.txt").is_err());
    assert!(rooted.read_text("/etc/passwd").is_err());
    assert_eq!(rooted.read_text("inside.txt")?, "inside");
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_denied() -> HarnessResult<()> {
    use std::os::unix::fs::symlink;
    let root = TempDir::new("symlink-root")?;
    let outside = TempDir::new("symlink-outside")?;
    fs::write(outside.path().join("secret.txt"), "secret").map_err(|error| {
        inbharat_harness_core::Failure::invalid("test.write", error.to_string())
    })?;
    symlink(outside.path(), root.path().join("escape")).map_err(|error| {
        inbharat_harness_core::Failure::invalid("test.symlink", error.to_string())
    })?;
    let rooted = RootedFs::new(root.path())?;
    let result = rooted.read_text("escape/secret.txt");
    assert!(result.is_err());
    assert!(rooted.create_dir_all("escape/new-directory").is_err());
    assert!(!outside.path().join("new-directory").exists());
    Ok(())
}

#[test]
fn subprocess_is_direct_argv_and_allowlist_denied_by_default() -> HarnessResult<()> {
    let temp = TempDir::new("process")?;
    let broker = LocalExecutionBroker::new(RootedFs::new(temp.path())?, Vec::<String>::new());
    let failure = broker
        .run_process(
            &ProcessSpec::new("sh", vec!["-c".to_owned(), "echo escaped".to_owned()]),
            &CancellationToken::new(),
        )
        .err()
        .ok_or_else(|| inbharat_harness_core::Failure::invalid("test", "process succeeded"))?;
    assert_eq!(failure.code, ErrorCode::SubprocessDenied);
    Ok(())
}

#[cfg(unix)]
#[test]
fn noisy_subprocess_is_bounded_and_times_out_without_pipe_deadlock() -> HarnessResult<()> {
    let temp = TempDir::new("process-output")?;
    let broker = LocalExecutionBroker::new(RootedFs::new(temp.path())?, ["yes".to_owned()]);
    let mut spec = ProcessSpec::new("yes", Vec::new());
    spec.timeout = Duration::from_millis(30);
    spec.max_output_bytes = 1024;
    spec.environment
        .insert("PATH".to_owned(), "/usr/bin:/bin".to_owned());
    let started = Instant::now();
    let failure = broker
        .run_process(&spec, &CancellationToken::new())
        .err()
        .ok_or_else(|| Failure::invalid("test", "noisy process unexpectedly succeeded"))?;
    assert_eq!(failure.code, ErrorCode::Timeout);
    assert!(started.elapsed() < Duration::from_secs(2));
    Ok(())
}

#[cfg(unix)]
#[test]
fn running_subprocess_converges_on_cancellation() -> HarnessResult<()> {
    let temp = TempDir::new("process-cancel")?;
    let broker = LocalExecutionBroker::new(RootedFs::new(temp.path())?, ["sleep".to_owned()]);
    let mut spec = ProcessSpec::new("sleep", vec!["10".to_owned()]);
    spec.timeout = Duration::from_secs(20);
    spec.environment
        .insert("PATH".to_owned(), "/usr/bin:/bin".to_owned());
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let worker = thread::spawn(move || broker.run_process(&spec, &worker_cancel));
    thread::sleep(Duration::from_millis(25));
    cancel.cancel(CancelCause::User);
    let result = worker.join().map_err(|_| {
        Failure::new(
            ErrorCode::Internal,
            FailureClass::Internal,
            "test.process_cancel",
            "process worker panicked",
        )
    })?;
    let failure = result
        .err()
        .ok_or_else(|| Failure::invalid("test", "cancelled process unexpectedly succeeded"))?;
    assert_eq!(failure.code, ErrorCode::Cancelled);
    Ok(())
}

#[test]
fn unresolved_allowlist_entries_fail_closed() -> HarnessResult<()> {
    let temp = TempDir::new("unresolved-program")?;
    let broker = LocalExecutionBroker::new(
        RootedFs::new(temp.path())?,
        ["inbharat-program-that-does-not-exist".to_owned()],
    );
    let failure = broker
        .run_process(
            &ProcessSpec::new("inbharat-program-that-does-not-exist", Vec::new()),
            &CancellationToken::new(),
        )
        .err()
        .ok_or_else(|| Failure::invalid("test", "unresolved program unexpectedly ran"))?;
    assert_eq!(failure.code, ErrorCode::SubprocessDenied);
    Ok(())
}

#[cfg(unix)]
#[test]
fn invalid_environment_entries_are_rejected_without_panicking() -> HarnessResult<()> {
    let temp = TempDir::new("invalid-environment")?;
    let broker = LocalExecutionBroker::new(RootedFs::new(temp.path())?, ["sleep".to_owned()]);
    let mut spec = ProcessSpec::new("sleep", vec!["0".to_owned()]);
    spec.environment
        .insert("INVALID=NAME".to_owned(), "value".to_owned());
    let failure = broker
        .run_process(&spec, &CancellationToken::new())
        .err()
        .ok_or_else(|| Failure::invalid("test", "invalid environment unexpectedly ran"))?;
    assert_eq!(failure.code, ErrorCode::InvalidInput);
    Ok(())
}

#[test]
fn atomic_write_stays_inside_root() -> HarnessResult<()> {
    let temp = TempDir::new("write")?;
    let rooted = RootedFs::new(temp.path())?;
    rooted.write_text_atomic("safe.txt", "value")?;
    assert_eq!(rooted.read_text("safe.txt")?, "value");
    assert!(rooted.write_text_atomic("../unsafe.txt", "bad").is_err());
    Ok(())
}
