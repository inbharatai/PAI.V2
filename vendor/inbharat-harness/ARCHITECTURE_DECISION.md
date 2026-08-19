# InBharat Harness Technology Decision

**Status:** accepted for 0.1.0-rc.1  
**Decision:** Use a **Rust control-plane core**, expose a **versioned C ABI**, and keep extensibility **out of process or in a capability-limited WASM runtime**. TypeScript may be shipped as an SDK/UI/plugin-authoring language, but **Node.js is not a required runtime** for the Harness core or Android product. Small platform-native helpers are permitted behind private interfaces.

## 1. Scope and decision drivers

This decision evaluates Rust, C++, TypeScript/Node, and hybrid architectures against the approved Harness brief:

- four explicit execution levels: L0 direct response, L1 deterministic action, L2 bounded agent, L3 full agent;
- low startup cost and resident memory, especially on Android;
- Linux, Windows, and macOS desktop portability;
- provider-neutral model adapters and dynamic capability exposure;
- controlled filesystem, subprocess, terminal, tool, job, cancellation, recovery, replay, and persistence surfaces;
- a plugin model that does not convert a plugin bug into process compromise;
- measurable false-agent-activation performance;
- maintainable implementation with no runtime dependence on DeepSeek, Cordis, or a model family.

The language is not the architecture. The decisive issue is whether the system can make L0/L1 cheap, isolate L2/L3 risk, express cancellation and ownership correctly, and avoid making Android carry a desktop runtime.

## 2. Evidence base and limits

### Pinned source

The inspected checkout is clean and exactly pinned at:

- the pinned local upstream checkout
- commit `47f943859bef60e4160492346772ded9b24f765a`
- commit subject: `Merge pull request #2519 from deepseek-harness/feat/npm-public`

No upstream files were modified.

### Source observations

1. **The upstream is a Node/TypeScript product, not a portable native core.** Root `package.json` requires Node `^22.19.0 || >=24.0.0`; the repository contains 248 `package.json` manifests. A source scan found approximately 497,489 `.ts` lines and 66,633 `.tsx` lines, with 1,038 test-source files.
2. **Its central architectural ideas are sound.** `docs/architecture.md` describes replaceable service/provider/consumer seams, scoped tools, durable session events, and a single concrete agent loop. `packages/core/agent-loop/src/agent.ts` and `tool-calls.ts` show explicit turn/step state, cancellation, bounded parallel dispatch, and model-ordered result commitment.
3. **Its plugin substrate is deeply coupled to Cordis.** The root README says “everything is a plugin”; services, events, reversible effects, configuration layers, HMR, and scoped contexts all use the vendored Cordis family. Reusing the implementation would preserve a Cordis dependency even if package names changed.
4. **The shipped tool pipeline is richer than a registry.** `packages/core/tools/README.md` defines schema validation, scoped visibility, allow/deny/ask policy, monotonic guards, around-dispatch timeout/retry/metrics, post-processing, canonical JSON results, output presentation, cancellation, and bounded parallel execution.
5. **The worker-thread code runtime is explicitly not a security boundary.** `packages/code-runtime/code-runtime-worker-thread/README.md` calls it “containment, not a security boundary,” notes that spawned OS processes may survive worker termination, and says intermediate binding values have no byte cap.
6. **The sandbox is filesystem-effect confinement, not complete isolation.** `docs/subsystems/sandbox.md` says network and process visibility are outside the policy vocabulary. Windows ACL and older Landlock enforcement can be partial; macOS depends on deprecated `sandbox-exec`.
7. **Android is not an upstream support target.** No Android documentation, package metadata, JNI layer, or Android build path was found. The implementation uses Node-specific APIs and native add-ons including `node-pty`, `koffi`, `sharp`, and a Linux Landlock launcher.
8. **Desktop packaging is heavy.** The implemented single-executable note records packaged artifacts “on the order of 174MB.” It targets Linux/macOS and explicitly treats Windows as a non-goal for that carrier.
9. **The upstream itself identifies a missing safety primitive relevant to this brief.** The agent-loop README states that there is no built-in turn budget. InBharat L2 requires a hard step/time/tool budget, so that omission cannot be copied.
10. **The pinned upstream is a developer preview.** Its root README warns of compatibility-breaking changes. That is appropriate as an audit source, not as InBharat’s stable runtime contract.

### Local quantitative probe

A 30-sample, no-dependency Node 24 process probe in this sandbox measured:

- median cold process start: **25.26 ms**;
- p95 cold process start: **27.70 ms**;
- median initial RSS: **43.19 MiB**;
- local Node executable size: **117 MiB**.

These numbers are a runtime floor, not a DeepSeek Harness benchmark; loading plugins, persistence, model adapters, native add-ons, or a UI can only add cost. Rust/C++ toolchains were not installed in this sandbox, so no fabricated cross-language microbenchmark is reported. Rust/C++ scores below are architectural estimates to be validated by the implementation spike defined in Section 10.

## 3. Weighted decision matrix

### Scoring method

Scores are 1–5, where 5 is best. The weighted score is `sum(score × weight) / 5`, yielding a score out of 100. Weights total 100.

| Criterion | Weight | Why it matters |
|---|---:|---|
| Startup latency | 8 | L0/L1 must not pay full-agent boot cost. |
| Resident RAM | 9 | Mobile viability and concurrent sessions. |
| Binary/runtime size | 6 | Android app size and desktop distribution. |
| Android | 12 | First-class target, JNI/lifecycle/NDK constraints. |
| Desktop portability | 7 | Linux, Windows, macOS. |
| Plugin model | 8 | Providers and tools must be replaceable. |
| Async/cancellation | 8 | Streaming, tools, jobs, model I/O, shutdown. |
| Sandboxing/security | 12 | Model-directed code and tools are hostile-input surfaces. |
| Tool execution | 10 | Determinism, policy, verification, replay. |
| FFI/embedding | 6 | Android, desktop shells, Audio integration. |
| Maintainability | 8 | Small team, long-lived product contracts. |
| Implementation feasibility | 6 | Time to first reliable release. |

### Scores

| Option | Start | RAM | Size | Android | Desktop | Plugins | Async | Sandbox | Tools | FFI | Maint. | Feasible | Weighted /100 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Pure Rust core | 5 | 5 | 4 | 4 | 5 | 3 | 5 | 5 | 4 | 4 | 5 | 4 | **88.8** |
| Pure C++17/20 core | 5 | 5 | 5 | 5 | 4 | 3 | 3 | 4 | 4 | 5 | 3 | 3 | **82.2** |
| TypeScript/Node core | 3 | 2 | 1 | 1 | 5 | 5 | 5 | 2 | 4 | 2 | 4 | 5 | **62.6** |
| Rust core + mandatory Node plugin host | 3 | 2 | 2 | 2 | 5 | 5 | 5 | 4 | 5 | 5 | 3 | 3 | **72.6** |
| **Rust core + C ABI + isolated WASM/RPC plugins; optional TS SDK/UI** | 4 | 4 | 4 | 4 | 5 | 5 | 5 | 5 | 5 | 5 | 4 | 4 | **90.2** |

### Score rationale

#### Pure Rust

- Strong ownership, memory safety, structured concurrency, async networking, cancellation, and cross-platform systems libraries.
- Produces native desktop and Android libraries without shipping a VM.
- Excellent for capability handles, typed policy, append-only events, and bounded jobs.
- Scores lower on plugins because a stable Rust dynamic-library ABI does not exist. Loading arbitrary Rust `.so/.dll/.dylib` plugins would bind plugins to compiler/toolchain details.
- JNI/NDK packaging is viable but less turnkey than C++; the C ABI should be the supported embedding boundary.

#### Pure C++

- Best Android NDK maturity, lowest runtime floor, and natural interoperability with OS APIs and native libraries.
- A C ABI can be excellent, but a C++ ABI is not a stable plugin contract across compilers/standard libraries.
- The async/cancellation/state-machine work in a multi-session agent runtime is substantially more error-prone. Sanitizers help but do not provide Rust’s ownership guarantees.
- Higher expected maintenance/security cost at JSON, network, plugin, subprocess, and model-output boundaries.

#### TypeScript/Node

- Fastest source-level adaptation from the inspected upstream and excellent async/plugin ergonomics.
- A good authoring language for SDKs, desktop UI, and trusted out-of-process plugins.
- Requiring Node for the core fails the Android, size, RAM, cold-start, native-add-on, and sandboxing goals. Node worker/vm isolation is not an authority boundary.
- Upstream desktop packaging evidence (approximately 174MB) and the local runtime-only RSS/start probe confirm that this cost is structural, not hypothetical.

#### Rust + mandatory Node host

- Keeps a safer core and preserves the npm plugin ecosystem, but duplicates runtimes, shutdown semantics, logging, packaging, versioning, and IPC failure modes.
- It would still burden Android unless the Node half were omitted, creating different product architectures by platform.
- Acceptable only when Node is an **optional external plugin process** on desktop; unacceptable as a required core dependency.

#### Recommended hybrid

- Rust owns routing, sessions, model/tool/provider contracts, permissions, jobs, persistence, cancellation, recovery, sandbox brokerage, and process lifecycle.
- The C ABI is the stable embedding boundary for Android/desktop and permits selected native helpers.
- Untrusted or independently shipped extensions use versioned RPC or a capability-limited WASM component interface. TypeScript can compile/run in an external process and communicate over the same protocol.
- The core can ship without WASM, Node, a web UI, a terminal, or L3. Optional features do not inflate L0/L1.

## 4. Decision

Adopt the following architecture:

```text
Android / desktop shells / CLI / tests
                 |
          versioned C ABI
                 |
+--------------------------------------------------+
| Rust core                                       |
| router -> L0 / L1 / L2 / L3                    |
| sessions, events, budgets, permissions, jobs    |
| model adapters, tool registry, verification     |
| persistence, cancellation, recovery, telemetry  |
+----------------------+---------------------------+
                       |
             ExecutionBroker trait
      +----------------+------------------+
      |                |                  |
  native tools    OS sandbox helper   remote sandbox
      |
  optional versioned extension hosts
  - WASM component runtime (capability handles only)
  - external RPC process (TS/Node, Python, native)
```

### Core rules

1. **L0 and L1 never initialize the agent loop.** Routing returns an explicit execution level before capability or model orchestration is started.
2. **The core is privileged and small.** It is not “everything is a plugin.” Stable interfaces exist at model, tool, storage, sandbox, and UI boundaries, but the router, policy engine, session state machine, event log, and budget enforcement are non-replaceable trusted computing base.
3. **No in-process native plugin ABI.** Rust/C++ dynamic libraries are not the public plugin mechanism. Native extensions go behind the C ABI at build time or run as a separate process.
4. **No ambient authority.** Plugins receive explicit, expiring capability handles; they do not receive arbitrary filesystem, network, environment, process, or credential access.
5. **Node is optional and external.** The product works on Android and desktop without Node. A TypeScript SDK can speak the same RPC protocol used by other external clients.
6. **Every level is budgeted.** L1 has one deterministic action budget; L2 has finite plan/step/tool/time/output budgets; L3 also has workspace, job, subagent-depth, and retained-output budgets.
7. **Cancellation is hierarchical and quiescent.** Parent cancellation reaches model requests, tools, subprocess groups, jobs, plugin RPC, and persistence flush. A session is “stopped” only after owned work terminates or is explicitly classified as leaked/failed.
8. **Public FFI is C, not Rust or C++.** Opaque handles, fixed-width integers, versioned size-tagged structs, caller-supplied allocators, explicit ownership, UTF-8 byte spans, and status codes only. No exceptions or panics cross the boundary.

## 5. What to adopt, adapt, or reject from DeepSeek Harness

| Upstream concept | Decision | InBharat treatment |
|---|---|---|
| Event-sourced session facts | **Adopt** | Versioned append-only event envelope; reconstruct model-visible requests and replay from durable facts. |
| Service/provider/consumer capability seams | **Adopt, simplify** | Rust traits and explicit registries; no Cordis runtime dependency. |
| One concrete loop behind an interface | **Adopt** | One auditable L2/L3 state machine, not multiple subtly different loops. |
| Scoped tool schemas and visibility | **Adopt** | Visibility is generated per request/session and enforced by authority, not only presentation. |
| Model adapter registry and streaming seam | **Adopt** | Provider-neutral stream events, retry classification, usage, cancellation. |
| Tool policy pipeline | **Adopt** | Validate -> authorize -> budget -> dispatch -> verify -> persist -> present. Denials are monotonic. |
| Parallel-safe tool classification with model-ordered commits | **Adopt** | Bounded scheduler; exclusive calls are barriers; results/events remain model ordered. |
| Reversible plugin lifecycle/effects | **Adapt** | Owned registrations and RAII guards; extension unload cannot remove another owner’s registration. |
| Filesystem/subprocess/execution-world abstraction | **Adopt** | One broker so filesystem and subprocess capabilities cannot point at different worlds accidentally. |
| Worker-thread model-written code runtime | **Reject as security boundary** | Optional WASM or remote sandbox; host process treats code as hostile. |
| “Everything is a plugin” | **Reject for the TCB** | Core policy/router/session/budget logic is fixed and reviewable. |
| Arbitrary model-written live plugin mount/unmount | **Reject by default** | Experimental desktop-only feature, if ever enabled, must be isolated and capability-limited. |
| Cordis config tree/HMR as product foundation | **Reject** | Declarative InBharat config with strict schema, static core, explicit extension manifests. |
| Node-native add-ons for PTY/FFI/image handling | **Reject as required dependencies** | Platform adapters behind Rust traits/C ABI; compile only where supported. |
| Filesystem-only sandbox vocabulary | **Adapt** | Policy also covers network, process creation, environment, credentials, device access, and resource ceilings. |
| Upstream lack of built-in turn budget | **Reject** | Budgets are mandatory and enforced below plugins. |
| 248-package monorepo decomposition | **Reject** | Start with a small crate set organized by trust boundary, not one package per feature. |

## 6. Proposed module boundaries

Keep the first implementation deliberately small:

- `inbharat-harness-core`: execution levels, router decision, session state machine, events, budgets, cancellation, errors.
- `inbharat-harness-model`: provider-neutral request/stream contracts and adapter registry.
- `inbharat-harness-tools`: schema vocabulary, tool registry, authorization, dispatch, result verification.
- `inbharat-harness-exec`: filesystem/subprocess/sandbox broker and platform implementations.
- `inbharat-harness-store`: append-only log, snapshots, migration, credential references.
- `inbharat-harness-ffi`: versioned C ABI, no business logic.
- `inbharat-harness-cli`: desktop CLI; optional feature.
- `inbharat-harness-rpc`: external plugin/client protocol; optional feature.
- `inbharat-harness-wasm`: WASM extension host; optional and deferred until native L0–L2 gates pass.

Do not add a crate merely to mirror every upstream package. Add a boundary only when it has independent ownership, threat, dependency, or release semantics.

## 7. Tool and plugin contracts

### Tool manifest

Every tool declares:

- stable id and semantic version;
- JSON-compatible input and output schemas;
- required capabilities (filesystem roots, network destinations, credentials, process types);
- determinism/idempotence classification;
- concurrency classification;
- default timeout, output budget, and side-effect class;
- verification function and rollback/compensation statement where applicable;
- supported execution levels.

### Dispatch order

The non-bypassable order is:

1. resolve visible tool from the session capability set;
2. validate and freeze arguments;
3. authorize capability and confirmation policy;
4. reserve step/time/output/resource budgets;
5. dispatch through the execution broker;
6. cooperatively cancel or hard-kill out-of-process work on deadline;
7. validate canonical output;
8. run deterministic postcondition verification;
9. append call/result/verification events in model order;
10. release budget and capability leases.

Plugins may observe or narrow a decision; they cannot widen a core denial.

### Extension isolation

- **WASM:** no WASI preview-1 ambient filesystem; use a component interface with host-provided handles. Fuel, epoch interruption, linear-memory cap, output cap, and wall-clock deadline are required.
- **External RPC:** authenticated local channel, length-prefixed or framed protocol, version negotiation, request ids, cancellation messages, bounded frames, heartbeat, process-group ownership, and fail-closed disconnect handling.
- **Trusted built-ins:** statically linked or workspace crates; still use the same tool contract and policy path.

## 8. Android and desktop implications

### Android

- Build Rust as NDK-compatible shared libraries and expose only the C ABI to thin JNI/Kotlin bindings.
- Do not expose Tokio, Rust futures, file descriptors, or Rust-owned strings to JNI. Use opaque job/session handles and callbacks or polling.
- Android L0/L1 builds may omit terminal, arbitrary subprocess, WASM, server, and desktop plugin hosts entirely.
- Sandboxing must respect the Android application sandbox rather than pretending desktop bwrap/Seatbelt semantics exist. Any subprocess feature is capability- and build-profile-gated.
- Lifecycle hooks must support backgrounding, foregrounding, network loss, process recreation, and cooperative cancellation.

### Desktop

- Use OS-native confinement where available, but report enforcement quality explicitly.
- Linux: namespaces/seccomp/Landlock or container/remote execution, with functional probes.
- macOS: prefer a separate helper/app-sandbox/XPC design over relying on deprecated `sandbox-exec`.
- Windows: Job Objects, restricted tokens/AppContainer where feasible, explicit ACL boundaries, and hard-link/reparse-point tests.
- “Sandbox unavailable” is an error for a confined request; silent full-access fallback is forbidden.

## 9. Non-negotiable risks and controls

| Risk | Non-negotiable control |
|---|---|
| False agent activation makes simple requests slow/expensive | L0/L1 routing is deterministic first; benchmark confusion set; release gate on false activation and latency. |
| Plugin becomes ambient-code execution | No arbitrary in-process plugin loading; WASM/RPC capabilities only; signed/hashed manifests. |
| Worker/thread cancellation leaks subprocesses | Process-group/Job Object ownership; terminate-and-join; leaked child is a failed run. |
| “Sandbox” protects files but not network/process/credentials | Multi-dimensional policy and enforcement report; deny when required dimension is unavailable. |
| L2 runs indefinitely | Hard step/tool/time/token/output budgets enforced in the core, not by a prompt or plugin. |
| L3 corrupts a user workspace | Ephemeral or explicitly approved workspace; read-before-write, diff, build/test verification, atomic publish. |
| FFI drift or unsound ownership | Versioned size-tagged C structs, ABI tests from C/Kotlin/C++, allocator discipline, panic containment. |
| Event log stores secrets or huge outputs | Secret references only, redaction before persistence, bounded previews plus content-addressed spill, retention policy. |
| Rust ecosystem/plugin pressure reintroduces Node as mandatory | Architecture test and packaging gate: core/Android artifact dependency graph must contain no Node runtime. |
| Upstream architecture is copied wholesale | Maintain an idea/source ledger; implement project-specific contracts and tests; preserve MIT notice for any copied source. |

## 10. Required implementation spike before final ADR

The recommendation is strong, but final acceptance must be measured. Build the same thin vertical slice in Rust and, only if contested, C++:

1. route 1,000 mixed requests across L0/L1/L2/L3;
2. stream a mock model response;
3. execute one read-only and one mutating tool through policy;
4. cancel during model I/O and during a spawned process;
5. persist and replay the event log;
6. expose create/send/poll/cancel/dispose through C and JNI smoke tests;
7. package Linux x86_64, Windows x86_64, macOS arm64, Android arm64-v8a;
8. measure cold/warm startup, idle/active RSS, artifact size, cancellation convergence, and 10,000-session leak behavior.

### Acceptance gates

- L0 median routing under 2 ms on reference desktop hardware and no model/plugin initialization.
- L1 deterministic action dispatch under 10 ms excluding the action itself.
- Android arm64-v8a library and JNI smoke compile; no Node files in the package.
- Cancellation joins all owned tasks/processes within the configured grace period.
- FFI symbols and struct layouts match a checked ABI manifest.
- False L2/L3 activation stays below the approved confusion-set threshold; no silent downgrade from L3 to unrestricted L1 execution.
- Core artifact works with model, storage, and tool mocks and no network.

## 11. Final recommendation

Proceed with the **Rust + C ABI + isolated extension-host hybrid**. Treat DeepSeek Harness as an architecture reference and conformance corpus, not as a runtime dependency or a source-to-source port.

The decisive reasons are:

1. Android cannot responsibly inherit a Node/Cordis/native-addon stack.
2. The strongest upstream ideas—event sourcing, capability seams, scoped tools, model-ordered bounded dispatch, and quiescent cancellation—map naturally to Rust without Cordis.
3. Security requires process/WASM authority boundaries, not only worker-thread containment.
4. A C ABI gives stable embedding and future interoperation with InBharat Audio without coupling either project’s implementation language.
5. Optional TypeScript SDKs preserve developer ergonomics without taxing every user or every platform.
