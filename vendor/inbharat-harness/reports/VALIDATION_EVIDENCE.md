# InBharat Harness 0.1.0-rc.1 — Validation Evidence

**Validated:** 2026-08-17T16:22:23Z

**Repository:** `inbharat-harness`

**Target:** `x86_64-unknown-linux-musl`
**Result:** **PASS**

## Environment

- `rustc 1.97.1 (8bab26f4f 2026-07-14)`, LLVM 22.1.6
- `cargo 1.97.1 (c980f4866 2026-06-30)`
- Amazon Linux 2023, kernel 6.18.40, x86_64
- Native `cc`: unavailable; the build used Rust's self-contained musl standard library and `rust-lld`.
- Full raw environment: `ENVIRONMENT.txt`.

## Exact final gate

The workspace was cleaned before the final gate. Every command below returned exit status 0, in this order:

1. `cargo clean --manifest-path Cargo.toml --target x86_64-unknown-linux-musl` — `FINAL_CLEAN.log`
2. `cargo fmt --manifest-path Cargo.toml --all --check` — `FINAL_FORMAT.log` (empty output means no format diff)
3. `cargo check --manifest-path Cargo.toml --target x86_64-unknown-linux-musl --workspace --all-targets` — `FINAL_CHECK.log`
4. `cargo test --manifest-path Cargo.toml --target x86_64-unknown-linux-musl --workspace` — `FINAL_TEST.log`
5. `cargo clippy --manifest-path Cargo.toml --target x86_64-unknown-linux-musl --workspace --all-targets -- -D warnings` — `FINAL_CLIPPY.log`
6. `cargo build --manifest-path Cargo.toml --target x86_64-unknown-linux-musl --release --workspace` — `FINAL_BUILD_RELEASE.log`
7. `scripts/smoke.sh` — `FINAL_CLI_SMOKE.log`
8. `ITERATIONS=100000 scripts/benchmark.sh` — `FINAL_BENCHMARK.log`
9. External C11 ABI compile and run — `FINAL_C_ABI_COMPILE.log`, `FINAL_C_ABI_RUN.log`

For target commands, `CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER` pointed to the installed Rust toolchain's `rust-lld`.

## Tests

**47 passed; 0 failed; 0 ignored.**

| Lane | Passed | Covered |
|---|---:|---|
| Core unit tests | 15 | routing, provider stream, complete mock-memory lifecycle, jobs, session repair/replay, tool exposure/schemas, JSON Unicode/strictness, filesystem/process denials |
| Cancellation/recovery integration | 2 | bounded provider retry, model cancellation/join |
| False-activation integration | 2 | 600 ordinary prompts plus adversarial agent/file/build wording |
| Provider/mode integration | 7 | prepared-call one-shot, request-id replay provider, trajectory scaling, attachments, credential refs, scoped subagents, L0 model capability |
| Routing/tool integration | 9 | four levels, monotonic escalation, L1 single action, L2 tool continuation, L3 verified rounds, scoped two-file responsive website creation, unique correlations, allow/unavailable confirmation audit |
| Security integration | 6 | traversal, absolute path, symlink/mkdir escape, atomic write, default process denial, noisy-pipe timeout, process cancellation |
| Session lifecycle integration | 5 | resume, fork, replay, torn-tail truncation, unknown tool/approval repair, checksum tamper detection |
| C ABI unit smoke | 1 | create, route, run, owned-byte free, cancellation create/request/cancellable run/destroy, harness destroy |

Raw names and timings are in `FINAL_TEST.log`. Clippy completed with `-D warnings`; `FINAL_CLIPPY.log` contains no diagnostics.

## CLI smoke

`scripts/smoke.sh` exercised all required commands and authority postures:

- `info` reported format 1, event version 1, C ABI 1, std-only dependencies, all levels/providers, and secure defaults;
- `route` returned expected L0, L1, L2, and L3 JSON decisions;
- `run-task` executed L0, L2, L3 (with workspace + one-shot confirmation), L1 read/write, and direct-argv subprocess paths;
- a write without authority was required to fail and did not create a file;
- an authorized write and allowlisted `sleep 0` subprocess succeeded;
- durable `--session-dir` create and `--resume` produced a balanced second turn;
- piped `/quit` exercised `chat`;
- `demo-website` generated and verified `index.html` and `style.css` under a root-confined path;
- `benchmark` completed a 1,000-route smoke, 600-prompt confusion set, and 10,000-session churn.

Exact output is in `FINAL_CLI_SMOKE.log`.

## Release benchmark

From `benchmarks/routing-latest.json` and `BENCHMARK.log`:

| Metric | Value |
|---|---:|
| Mixed routing iterations | 100,000 |
| Ordinary confusion prompts | 600 |
| False L2/L3 activations | **0** |
| False activation rate | **0.00000000** |
| Routing p50 | 1,425 ns |
| Routing p95 | 1,780 ns |
| Routing max | 222,113 ns |
| Session create/drop churn | 10,000 sessions / 10,000 start events |
| Session churn total | 45,287,359 ns |
| Deterministic level checksum | 240,000 |

Timing is specific to this container. The correctness gate is zero false agent activations; the session churn is an ownership/churn probe, not a heap-profiler proof.

## Dependency evidence

`Cargo.lock` contains only the three workspace packages. `DEPENDENCY_TREE.txt` shows:

- CLI -> core
- FFI -> core
- core -> no external crate

No Node runtime, network client, database, async runtime, serialization crate, or other third-party crate is present.

## Release artifacts

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| `target/x86_64-unknown-linux-musl/release/inbharat-harness` | 886,216 | `229f603ded4f9ac1100a3baaa25c4ebcc19a44abdcfdfec8ba7ed603f463aa20` |
| `target/x86_64-unknown-linux-musl/release/libinbharat_harness.a` | 24,447,804 | `77b8204ea4ac9323081850f231dae815544ace0c5c1ac140127e46303602871b` |
| `target/harness-c-abi-smoke` | 8,835,656 | `fe37680b04f268b0a26e01dc4b7d94f774710228755e00d4ffda9b9c9834047f` |

Raw values: `ARTIFACT_SIZES.txt`, `ARTIFACTS.sha256`.

## C ABI evidence

`llvm-nm` found all 11 versioned external symbols recorded in `crates/ffi/abi-v1.json`:

- `ib_harness_api_version_v1`
- `ib_harness_create_v1`, `ib_harness_destroy_v1`
- `ib_harness_route_v1`
- `ib_harness_run_v1`, `ib_harness_run_with_cancel_v1`
- `ib_harness_cancel_create_v1`, `ib_harness_cancel_request_v1`, `ib_harness_cancel_destroy_v1`
- `ib_harness_bytes_free_v1`
- `ib_harness_status_message_v1`

Raw symbol output: `ABI_SYMBOLS.txt`. `ABI_MANIFEST_CHECK.log` confirms manifest = header = archive at 11 symbols with no missing/extra names. The Rust ABI smoke passed. The external `examples/c_abi_smoke.c` consumer also compiled with Zig C11 against the musl static archive using warnings-as-errors, linked with libunwind, and ran successfully (`FINAL_C_ABI_COMPILE.log`, `FINAL_C_ABI_RUN.log`).

## Repository integrity

- Pristine upstream revision remained `47f943859bef60e4160492346772ded9b24f765a`.
- Upstream `git status --porcelain=v1` was empty after implementation and validation.
- The local repository has no remotes and uses local-only traceability commits, as requested.
- No upstream source was copied; architectural inputs and independence are recorded in `docs/SOURCE_LEDGER.md`.
- Raw state: `REPOSITORY_STATE.txt`.

## Explicit RC limits

This local RC does not claim a production remote model adapter, remote authenticated server, kernel-grade sandbox, descendant process-group termination, encrypted store, KMS/keychain provider, WASM/RPC plugin host, durable jobs after restart, Android/JNI package, or cryptographically authenticated event log. These are absent or fail closed and are documented in `SECURITY.md`, `docs/THREAT_MODEL.md`, and `docs/RELEASE_CANDIDATE.md`.
