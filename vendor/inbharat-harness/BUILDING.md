# Testing

## Full local gate

```sh
./scripts/check.sh
```

The gate runs formatting, all-target compilation, unit/integration tests, Clippy with warnings denied, CLI build, routing smokes, task smoke, and a 1,000-iteration benchmark smoke.

## Test inventory

- unit tests in each core module;
- `false_activation.rs`: 600 ordinary prompts plus adversarial wording that must remain L0;
- `routing_and_tools.rs`: four-level routing, monotonic escalation, dynamic tool selection, L1 one-action invariant, one-shot confirmation, and approval audit pairing;
- `cancellation_recovery.rs`: provider retry bound and cancellation convergence;
- `security.rs`: traversal, absolute path, symlink escape, atomic write, process allowlist denial, noisy-output timeout, and running-process cancellation;
- `session_lifecycle.rs`: JSONL resume, balanced replay, fork, interrupted tool/approval synthesis, and tamper detection;
- `providers_and_modes.rs`: prepared-call one-shot behavior, mode-scaled trajectories, metadata/reference safety, and scoped subagents;
- FFI unit smoke: create, route, run, free bytes, destroy.

## Benchmark

```sh
ITERATIONS=100000 ./scripts/benchmark.sh
```

The JSON output records version, iteration count, 600-prompt confusion set size, false L2/L3 activations, rate, p50/p95/max routing latency, a 10,000-session create/drop churn probe, and a result checksum. Timing is environment-specific; the false-activation count is the correctness release gate. Session churn proves bounded handle ownership in this process but is not a heap-profiler leak proof.

## Portable linker behavior

`cargo-portable.sh` uses the native compiler/linker when `cc` exists. In minimal CI images it installs the Rust musl standard library and links through the toolchain's `rust-lld`. The selected target is printed by Cargo and recorded in release evidence.
