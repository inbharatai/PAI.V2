# Local Operations

## Build and gate

```sh
./scripts/check.sh
ITERATIONS=100000 ./scripts/benchmark.sh
```

Raw final outputs belong in `reports/`; the benchmark JSON belongs in `benchmarks/routing-latest.json`. Do not enable telemetry or upload transcripts as part of the gate.

## Session store

`run-task --session-dir DIR` creates format-v1 JSONL. `--resume SESSION_ID` validates every record, checksum predecessor, lifecycle transition, and correlation before appending a new turn. Keep the directory owner-only; this crate does not encrypt it. Back up only at completed-turn/checkpoint boundaries.

## Capabilities

Start without flags. Grant `--allow-write` only for a root selected with `--root`, and pair mutating actions with `--yes`. Process execution additionally requires an exact `--allow-program NAME`, `--allow-process`, `--trusted-process`, and confirmation. RC1 local process isolation is partial; allowlist only non-daemonizing programs.

## Diagnostics

- `info`: versions, levels, providers, defaults;
- `route`: inspect the route before execution;
- `--trajectory diagnostic`: retain all stream chunks locally;
- `benchmark`: run routing confusion and 10,000-session churn probes;
- session `replay()`: validate chain and lifecycle.

Never include credential values in command lines, prompts, failure details, or reports.
