use crate::policy::{CliPermission, CliSandbox};
use crate::{benchmark, demo};
use inbharat_harness_core::error::{ErrorCode, Failure, FailureClass, HarnessResult};
#[cfg(feature = "test-providers")]
use inbharat_harness_core::providers::EchoModelProvider;
use inbharat_harness_core::providers::{
    BasicSafetyProvider, CanonicalVerificationProvider, Capability, CapabilitySet,
    ConfirmationOutcome, StaticConfirmationProvider,
};
use inbharat_harness_core::routing::ExecutionLevel;
use inbharat_harness_core::runtime::{HarnessBuilder, RunOptions, TrajectoryMode};
use inbharat_harness_core::session::{SessionId, SessionStore};
use inbharat_harness_core::{CancellationToken, Harness, LocalExecutionBroker, RootedFs};
use std::env;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn main_entry() -> i32 {
    let result = run(env::args().skip(1).collect());
    match result {
        Ok(()) => 0,
        Err(failure) => {
            eprintln!(
                "error: {} (class={:?}, retryable={})",
                failure, failure.class, failure.retryable
            );
            2
        }
    }
}

fn run(args: Vec<String>) -> HarnessResult<()> {
    let Some(command) = args.first().map(String::as_str) else {
        print_help();
        return Ok(());
    };
    let rest = &args[1..];
    match command {
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "version" | "--version" | "-V" => {
            println!("inbharat-harness {VERSION}");
            Ok(())
        }
        "route" => command_route(rest),
        "run-task" => command_run_task(rest),
        "chat" => command_chat(rest),
        "benchmark" => command_benchmark(rest),
        "demo-website" => command_demo_website(rest),
        "info" => command_info(),
        _ => Err(Failure::invalid(
            "cli",
            format!("unknown command: {command}"),
        )),
    }
}

fn command_route(args: &[String]) -> HarnessResult<()> {
    let parsed = CommonArgs::parse(args, false)?;
    let prompt = parsed.prompt()?;
    let options = parsed.run_options();
    let harness = build_harness(&parsed)?;
    let decision = harness.route(&prompt, &options)?;
    println!(
        "{{\"level\":\"{}\",\"reason\":\"{}\",\"rule_id\":\"{}\",\"confidence_basis_points\":{},\"confirmation_required\":{},\"capabilities\":{}}}",
        escape_json(decision.level.as_str()),
        escape_json(decision.reason.as_str()),
        escape_json(decision.rule_id),
        decision.confidence_basis_points,
        decision.confirmation_required,
        json_strings(&decision.required_capabilities.names())
    );
    Ok(())
}

fn command_run_task(args: &[String]) -> HarnessResult<()> {
    let parsed = CommonArgs::parse(args, true)?;
    let prompt = parsed.prompt()?;
    let options = parsed.run_options();
    let harness = build_harness(&parsed)?;
    let cancel = CancellationToken::new();
    if let Some(store_path) = &parsed.session_dir {
        let store = SessionStore::open(store_path)?;
        let mut session = match &parsed.resume {
            Some(id) => store.resume(&SessionId::parse(id)?)?,
            None => store.create()?,
        };
        let outcome = harness.run_in_session(&mut session, &prompt, &options, &cancel)?;
        print_outcome(&outcome);
        println!(
            "session_log={}/{}.jsonl",
            store.root().display(),
            session.id()
        );
    } else {
        let (outcome, _session) = harness.run(&prompt, &options, &cancel)?;
        print_outcome(&outcome);
    }
    Ok(())
}

fn command_chat(args: &[String]) -> HarnessResult<()> {
    let parsed = CommonArgs::parse(args, false)?;
    if !parsed.prompt_parts.is_empty() {
        return Err(Failure::invalid(
            "chat",
            "chat does not accept an initial prompt",
        ));
    }
    let options = parsed.run_options();
    let harness = build_harness(&parsed)?;
    let mut session = inbharat_harness_core::Session::in_memory()?;
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    println!("InBharat Harness local chat. Type /quit to stop.");
    print!("> ");
    stdout
        .flush()
        .map_err(|error| cli_io("chat.flush", error))?;
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| cli_io("chat.read", error))?;
        if matches!(line.trim(), "/quit" | "/exit") {
            break;
        }
        if !line.trim().is_empty() {
            let outcome =
                harness.run_in_session(&mut session, &line, &options, &CancellationToken::new())?;
            println!("{}", outcome.output);
        }
        print!("> ");
        stdout
            .flush()
            .map_err(|error| cli_io("chat.flush", error))?;
    }
    Ok(())
}

fn command_benchmark(args: &[String]) -> HarnessResult<()> {
    let mut iterations = 10_000_usize;
    let mut output = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--iterations" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    Failure::invalid("benchmark", "--iterations requires a value")
                })?;
                iterations = value.parse().map_err(|_| {
                    Failure::invalid("benchmark", "--iterations must be an integer")
                })?;
                index += 2;
            }
            "--output" => {
                output = Some(PathBuf::from(args.get(index + 1).ok_or_else(|| {
                    Failure::invalid("benchmark", "--output requires a value")
                })?));
                index += 2;
            }
            unknown => {
                return Err(Failure::invalid(
                    "benchmark",
                    format!("unknown option: {unknown}"),
                ));
            }
        }
    }
    benchmark::run(iterations, output.as_deref(), VERSION)
}

fn command_demo_website(args: &[String]) -> HarnessResult<()> {
    let mut output = "demo-site".to_owned();
    let mut title = "InBharat Harness Demo".to_owned();
    let mut force = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--output" | "--title" => {
                let flag = args[index].as_str();
                let value = args.get(index + 1).ok_or_else(|| {
                    Failure::invalid("demo-website", format!("{flag} requires a value"))
                })?;
                if flag == "--output" {
                    output = value.clone();
                } else {
                    title = value.clone();
                }
                index += 2;
            }
            "--force" => {
                force = true;
                index += 1;
            }
            unknown => {
                return Err(Failure::invalid(
                    "demo-website",
                    format!("unknown option: {unknown}"),
                ));
            }
        }
    }
    demo::create(&output, &title, force)
}

fn command_info() -> HarnessResult<()> {
    println!("InBharat Harness {VERSION}");
    println!("session_format=1 event_version=1 c_abi=1");
    println!("dependencies=rust-std-only");
    println!("levels=L0,L1,L2,L3");
    println!(
        "providers=model,memory,safety,permission,confirmation,verification,sandbox,credential"
    );
    println!("secure_defaults=read-only,no-process,no-network,no-secret-values,telemetry-off");
    println!("commands=route,run-task,chat,benchmark,demo-website,info");
    #[cfg(feature = "test-providers")]
    println!("model_execution=echo-test-provider");
    #[cfg(not(feature = "test-providers"))]
    println!(
        "model_execution=none-bundled;L1-deterministic-tools-work;model-turns-need-a-real-ModelProvider"
    );
    io::stdout()
        .flush()
        .map_err(|error| cli_io("info.flush", error))
}

fn build_harness(parsed: &CommonArgs) -> HarnessResult<Harness> {
    let root = parsed.root.as_deref().unwrap_or(Path::new("."));
    let filesystem = RootedFs::new(root)?;
    let execution = Arc::new(LocalExecutionBroker::new(
        filesystem,
        parsed.allowed_programs.clone(),
    ));
    let granted = parsed.run_options().capabilities;
    let builder = HarnessBuilder::new(execution)?
        .permission_provider(Arc::new(CliPermission {
            granted: granted.clone(),
            ask_for_side_effects: !parsed.yes,
        }))
        .confirmation_provider(Arc::new(StaticConfirmationProvider {
            outcome: if parsed.yes {
                ConfirmationOutcome::AllowedOnce
            } else {
                ConfirmationOutcome::Unavailable
            },
        }))
        .verification_provider(Arc::new(CanonicalVerificationProvider))
        .safety_provider(Arc::new(BasicSafetyProvider))
        .sandbox_provider(Arc::new(CliSandbox {
            granted,
            trusted_process: parsed.trusted_process,
        }));
    // Only the test-providers build registers a (synthetic) model. The
    // production CLI bundles no dummy provider, so model-requiring tasks
    // fail-closed at the runtime capability gate (runtime.rs run_model_loop),
    // while L1 deterministic tool execution still works because it dispatches a
    // registered tool directly and needs no model.
    #[cfg(feature = "test-providers")]
    let builder = builder.register_model(Arc::new(EchoModelProvider::default()))?;
    Ok(builder.build())
}

fn print_outcome(outcome: &inbharat_harness_core::RunOutcome) {
    println!("{}", outcome.output);
    eprintln!(
        "level={} session={} steps={} tools={} events={} elapsed_ms={:.3}",
        outcome.decision.level.as_str(),
        outcome.session_id,
        outcome.steps,
        outcome.tool_calls,
        outcome.event_count,
        outcome.elapsed.as_secs_f64() * 1000.0
    );
}

#[derive(Debug, Default)]
struct CommonArgs {
    prompt_parts: Vec<String>,
    root: Option<PathBuf>,
    explicit_level: Option<ExecutionLevel>,
    trajectory: Option<TrajectoryMode>,
    provider: Option<String>,
    model: Option<String>,
    session_dir: Option<PathBuf>,
    resume: Option<String>,
    allowed_programs: Vec<String>,
    yes: bool,
    write: bool,
    process: bool,
    workspace: bool,
    trusted_process: bool,
}

impl CommonArgs {
    fn parse(args: &[String], allow_session: bool) -> HarnessResult<Self> {
        let mut parsed = Self::default();
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--root" | "--level" | "--trajectory" | "--provider" | "--model"
                | "--session-dir" | "--resume" | "--allow-program" => {
                    let flag = args[index].as_str();
                    let value = args.get(index + 1).ok_or_else(|| {
                        Failure::invalid("cli.args", format!("{flag} requires a value"))
                    })?;
                    match flag {
                        "--root" => parsed.root = Some(PathBuf::from(value)),
                        "--level" => {
                            parsed.explicit_level =
                                Some(ExecutionLevel::parse(value).ok_or_else(|| {
                                    Failure::invalid("cli.args", "level must be L0, L1, L2, or L3")
                                })?);
                        }
                        "--trajectory" => {
                            parsed.trajectory = Some(match value.as_str() {
                                "minimal" => TrajectoryMode::Minimal,
                                "standard" => TrajectoryMode::Standard,
                                "diagnostic" => TrajectoryMode::Diagnostic,
                                _ => {
                                    return Err(Failure::invalid(
                                        "cli.args",
                                        "trajectory must be minimal, standard, or diagnostic",
                                    ));
                                }
                            });
                        }
                        "--provider" => parsed.provider = Some(value.clone()),
                        "--model" => parsed.model = Some(value.clone()),
                        "--session-dir" if allow_session => {
                            parsed.session_dir = Some(PathBuf::from(value));
                        }
                        "--resume" if allow_session => parsed.resume = Some(value.clone()),
                        "--allow-program" => parsed.allowed_programs.push(value.clone()),
                        "--session-dir" | "--resume" => {
                            return Err(Failure::invalid(
                                "cli.args",
                                "session options are only supported by run-task",
                            ));
                        }
                        _ => {}
                    }
                    index += 2;
                }
                "--yes" => {
                    parsed.yes = true;
                    index += 1;
                }
                "--allow-write" => {
                    parsed.write = true;
                    index += 1;
                }
                "--allow-process" => {
                    parsed.process = true;
                    index += 1;
                }
                "--workspace" => {
                    parsed.workspace = true;
                    index += 1;
                }
                "--trusted-process" => {
                    parsed.trusted_process = true;
                    index += 1;
                }
                value if value.starts_with('-') => {
                    return Err(Failure::invalid(
                        "cli.args",
                        format!("unknown option: {value}"),
                    ));
                }
                value => {
                    parsed.prompt_parts.push(value.to_owned());
                    index += 1;
                }
            }
        }
        if parsed.resume.is_some() && parsed.session_dir.is_none() {
            return Err(Failure::invalid(
                "cli.args",
                "--resume requires --session-dir",
            ));
        }
        Ok(parsed)
    }

    fn prompt(&self) -> HarnessResult<String> {
        if self.prompt_parts.is_empty() {
            Err(Failure::invalid("cli.args", "prompt is required"))
        } else {
            Ok(self.prompt_parts.join(" "))
        }
    }

    fn run_options(&self) -> RunOptions {
        let mut capabilities =
            CapabilitySet::from_slice(&[Capability::Model, Capability::FileRead]);
        if self.write {
            capabilities.insert(Capability::FileWrite);
        }
        if self.process {
            capabilities.insert(Capability::ProcessSpawn);
        }
        if self.workspace {
            capabilities.insert(Capability::Workspace);
        }
        RunOptions {
            explicit_level: self.explicit_level,
            capabilities,
            provider: self.provider.clone().unwrap_or_else(default_provider),
            model: self.model.clone().unwrap_or_else(default_model),
            trajectory: self.trajectory.unwrap_or(TrajectoryMode::Standard),
            ..RunOptions::default()
        }
    }
}

/// Default provider id for `RunOptions` when `--provider` is not given.
///
/// The test-providers build pairs this with the registered `EchoModelProvider`
/// (`id = "echo"`) so model-turn smoke tests resolve a real provider. The
/// production build has no registered provider, so the sentinel
/// `unconfigured-provider` makes the unconfigured state explicit and lets the
/// runtime fail-closed with a truthful message rather than silently matching a
/// synthetic id.
fn default_provider() -> String {
    #[cfg(feature = "test-providers")]
    return "echo".to_owned();
    #[cfg(not(feature = "test-providers"))]
    return "unconfigured-provider".to_owned();
}

/// Default model id for `RunOptions` when `--model` is not given; see
/// [`default_provider`]. The echo provider serves `echo-v1`.
fn default_model() -> String {
    #[cfg(feature = "test-providers")]
    return "echo-v1".to_owned();
    #[cfg(not(feature = "test-providers"))]
    return "unconfigured-model".to_owned();
}

fn json_strings(values: &[&str]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!("\"{}\"", escape_json(value)))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Escape a string for embedding in a JSON string literal.
///
/// Handles the structural characters (`\` and `"`), the common whitespace
/// escapes, and every remaining C0 control character (< 0x20) via the universal
/// `\u00XX` escape that JSON requires. Without the control-char arm a value
/// containing e.g. \x08 or \x0c would produce spec-invalid JSON.
fn escape_json(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // Remaining C0 controls (e.g. \x08 backspace, \x0c form feed).
                let code = c as u32;
                out.push_str("\\u");
                out.push(char::from_digit((code >> 12) & 0xF, 16).unwrap_or('0'));
                out.push(char::from_digit((code >> 8) & 0xF, 16).unwrap_or('0'));
                out.push(char::from_digit((code >> 4) & 0xF, 16).unwrap_or('0'));
                out.push(char::from_digit(code & 0xF, 16).unwrap_or('0'));
            }
            c => out.push(c),
        }
    }
    out
}

fn cli_io(operation: &str, error: io::Error) -> Failure {
    Failure::new(
        ErrorCode::ToolFailed,
        FailureClass::Execution,
        operation,
        "local I/O failed",
    )
    .with_detail("io_kind", format!("{:?}", error.kind()))
}

fn print_help() {
    println!(
        "InBharat Harness {VERSION}\n\nUSAGE:\n  inbharat-harness <COMMAND> [OPTIONS]\n\nCOMMANDS:\n  route         Print deterministic L0/L1/L2/L3 routing JSON\n  benchmark     Measure routing and the 600-prompt false-activation set\n  demo-website  Generate a root-confined static demonstration website\n  info          Print release and security-capability information\n\nMODEL EXECUTION:\n  The standalone binary intentionally bundles no echo/mock model.\n  Real execution is supplied by the embedding product through ModelProvider.\n\nCOMMON OPTIONS:\n  --root PATH             Filesystem authority root (default .)\n  --level L0|L1|L2|L3    Explicit route override\n  --trajectory MODE      minimal, standard, or diagnostic\n  --allow-write           Grant file.write authority\n  --allow-process         Grant process.spawn authority\n  --allow-program NAME    Allow one direct-argv executable (repeatable)\n  --workspace             Grant workspace authority\n  --yes                   One-shot confirmation for side effects\n  --trusted-process       Explicitly accept partial local process isolation\n"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_json_handles_quotes_backslashes_and_control_chars() {
        // Structural characters
        assert_eq!(escape_json("a\"b"), "a\\\"b");
        assert_eq!(escape_json("a\\b"), "a\\\\b");
        // Common whitespace escapes
        assert_eq!(escape_json("a\nb"), "a\\nb");
        assert_eq!(escape_json("a\rb"), "a\\rb");
        assert_eq!(escape_json("a\tb"), "a\\tb");
        // Remaining C0 control chars -> \u00XX (backspace, form feed)
        assert_eq!(escape_json("a\u{0008}b"), "a\\u0008b");
        assert_eq!(escape_json("a\u{000c}b"), "a\\u000cb");
        // A reason string containing a quote must not break JSON output.
        let reason = "route because of \"pattern\"";
        let escaped = escape_json(reason);
        assert_eq!(escaped, "route because of \\\"pattern\\\"");
    }
}
