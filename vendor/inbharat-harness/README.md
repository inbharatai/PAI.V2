# InBharat Harness

A compact, model-neutral Rust control plane for deterministic local assistant and agent execution. This repository is the hardened second local release candidate (`0.1.0-rc.2`). The core and CLI use **only the Rust standard library**. Production builds expose no synthetic model provider by default; `EchoModelProvider`/`MockModelProvider` require the explicit `test-providers` feature and are intended only for engineering tests.

## What is implemented

- deterministic routing to **L0** direct response, **L1** one action, **L2** finite agent, and **L3** goal/workspace execution;
- one-level-at-a-time escalation with explicit policy ceilings;
- provider-neutral, indexed streaming with registration-bound one-shot prepared calls;
- offline echo, scripted mock, and request-id-bound replay providers;
- dynamic tool exposure from immutable capability manifests;
- provider traits for memory, safety, permission, confirmation, verification, sandbox, credentials, and models;
- append-only format-v1 JSONL session events with contiguous sequence numbers, versioned envelopes, a checksum chain, checkpoints, repair, resume, fork, and replay;
- minimal/standard/diagnostic trajectories;
- hard step, tool, round, job, depth, time, and output budgets;
- hierarchical first-cause-wins cancellation;
- bounded provider recovery without blind side-effect replay;
- root-confined filesystem tools, direct-argv allowlisted subprocesses, process-local background jobs, and scoped one-shot subagents;
- attachment metadata and credential references (never secret literals);
- transcript-free in-process metrics;
- a size-tagged versioned C ABI and C header;
- CLI commands: `chat`, `route`, `run-task`, `benchmark`, `demo-website`, and `info`.

## Secure defaults

The default local builder grants model use and root-confined reads. Writes require a capability plus one-shot confirmation. Process execution, network, ambient credentials, raw telemetry, and unrestricted plugins are disabled. The local root fence is **not** advertised as an OS sandbox; requests requiring a security boundary fail closed.

## Quick start

```sh
./scripts/build.sh
./scripts/test.sh

# Inspect a route without running an agent
./target/x86_64-unknown-linux-musl/debug/inbharat-harness route "hello"

# Offline local response
./target/x86_64-unknown-linux-musl/debug/inbharat-harness run-task "hello"

# Deterministic read-only action
./target/x86_64-unknown-linux-musl/debug/inbharat-harness run-task --root . "read file README.md"

# Confusion-set and latency benchmark
./scripts/benchmark.sh
```

On systems with a C toolchain the scripts use the native target. In minimal environments without `cc`, `scripts/cargo-portable.sh` uses Rust's self-contained musl target and `rust-lld`.

## Workspace

| Crate | Responsibility |
|---|---|
| `inbharat-harness-core` | Trusted router, budgets, sessions, providers, tools, execution, jobs, recovery, metrics |
| `inbharat-harness-cli` | Local CLI and benchmark harness |
| `inbharat-harness-ffi` | Business-logic-free C ABI translation layer |

Read [ARCHITECTURE.md](ARCHITECTURE.md), [SECURITY.md](SECURITY.md), [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md), [docs/ROUTING.md](docs/ROUTING.md), [docs/SESSION_FORMAT.md](docs/SESSION_FORMAT.md), [docs/MANIFESTS.md](docs/MANIFESTS.md), [docs/C_ABI.md](docs/C_ABI.md), and [TESTING.md](TESTING.md) before embedding or extending the runtime.

## Release-candidate limits

This is a local, synchronous foundation rather than a network service. It does not include an HTTP server, a real remote model adapter, durable jobs across process restart, a WASM host, or a kernel-grade cross-platform sandbox. The echo/mock providers allow complete offline conformance testing. See [docs/RELEASE_CANDIDATE.md](docs/RELEASE_CANDIDATE.md) for the exact boundary.

## Independence

The implementation was written independently from architecture audit findings. No source code was copied from, and no files were written to, the pristine upstream checkout. See [docs/SOURCE_LEDGER.md](docs/SOURCE_LEDGER.md).

## License

Licensed under the MIT License.


## Linux ARM64 / Raspberry Pi

Use `scripts/build-linux-arm64.sh`. The release gate requires a real AArch64 executable and physical-Pi provider tests; see `docs/LINUX_ARM64.md`.

## Production vs research features

Default production core features are empty. Synthetic model providers require `test-providers`; the separate v0.2 research innovation modules require `research-innovations`. Product builds such as Pocket AI should enable neither unless a deliberate acceptance plan requires them. This prevents unconnected research code or deterministic test providers from silently becoming part of a shipping agent runtime.
