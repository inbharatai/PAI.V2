# DeepSeek Harness Source Architecture Audit

**Status:** accepted source audit for InBharat Harness 0.1.0-rc.1  
**Audit target:** pinned local DeepSeek Harness checkout  
**Pinned revision:** `47f943859bef60e4160492346772ded9b24f765a`  
**Audit mode:** source inspection; the upstream checkout was not modified  
**Target decision:** what to adopt for an independent, model-neutral **InBharat Harness**

## 1. Executive assessment

DeepSeek Harness is not a thin chat loop. It is a large, pre-release application platform built around a vendored and materially patched Cordis runtime. Its strongest ideas are:

1. **Reversible plugin lifecycles rather than a global singleton container.** Services, listeners, tools, prompt sections, adapters, and providers are effect-owned and unwind with their plugin fiber.
2. **A provider-neutral model seam with registration-bound call preparation.** `LlmRuntime.prepareCall()` freezes model capability/default resolution and adapter identity for exactly one dispatch.
3. **One immutable, append-only session log as the source of model context.** Turn/step boundaries, request headers, raw chunks, assembled messages, tool calls/results, inbox changes, policy changes, and extension events are durable facts.
4. **A scoped capability model.** Per-agent tool/prompt/skill contributions are resolved through `ScopeKey` ancestry, not copied into each agent.
5. **Explicit side-effect checkpoints and quiescent teardown.** Cancellation does not abandon started work; persistence is checkpointed before model calls and top-level tool effects.
6. **Strict tool inputs and canonical outputs.** Tool argument schema, output schema, content projection, presentation projection, cancellation, approval, and event logging are separated.
7. **Capability seams split into definition, provider, and consumer roles.** Filesystem, subprocess, shell, sandbox, LLM, jobs, subagents, persistence, attachments, storage, credentials, and telemetry follow this pattern.

The codebase also carries costs that InBharat should not inherit wholesale:

- The repository is very large and fragmented: 241 non-fixture `package.json` files were observed, including 39 client packages and 959 test/spec files.
- Cordis is not a stock dependency here. `vendor/README.md` records 18 local modification classes, including lifecycle hardening, transactional Loader reconciliation, lazy config resolution, HMR changes, and patch semantics. Adopting the vendored fork means owning a framework fork.
- The on-disk session format is explicitly pre-release (`SESSION_FORMAT_VERSION = 0`) with no compatibility promise or migration path.
- Many contracts rely on TypeScript declaration merging and same-process trust. They are excellent inside one TypeScript process but are not a portable cross-language protocol by themselves.
- The Web API uses trusted-authority / DNS-rebinding defenses, not authentication. Source comments explicitly say `trustedHosts` is not authentication, while sessions can execute commands as the host user.
- Worker-thread Code Mode and workflow VM execution explicitly state that they are containment, **not security boundaries**; dynamic Cordis self-modification evaluates model-authored code. These must not ship as general untrusted execution in InBharat.
- Jobs are process-local; attachment garbage collection is deferred; preset generations leak until process teardown; Web HMR is disabled; full-text session search is disabled by default; telemetry redaction has no built-in rules.

### Recommended top-level decision

Build InBharat as a **smaller event-sourced harness with a plugin-capability kernel**, not as a fork of the entire repository.

- **Adopt** the session/turn/step model, tool execution semantics, model adapter seam, scoped registrations, approval audit, persistence checkpoints, and repair strategy.
- **Adapt** the Cordis concepts behind an InBharat-owned kernel API. Either consume upstream Cordis without rescoping or reimplement only the small subset needed; do not inherit this repository's vendored fork by default.
- **Simplify** profiles, bundles, CLI, RPC, projections, and UI composition.
- **Rebuild** the external server/auth boundary and the generated Typert RPC layer around explicit versioned schemas.
- **Defer** goals, durable continuable subagents, scheduling, rich workflow orchestration, and a plugin-composed browser UI until the core log and security model are stable.
- **Reject** model-authored runtime plugins and escapable VM/worker execution as security boundaries.

## 2. Classification legend

| Decision | Meaning for InBharat |
|---|---|
| **ADOPT** | Preserve the design and most behavior; repackage/rename as needed. |
| **ADAPT** | Preserve the architecture, but change APIs, persistence, security, or model-specific assumptions. |
| **SIMPLIFY** | Keep the user need with a materially smaller mechanism. |
| **REBUILD** | Reimplement because the current subsystem is too coupled, unsafe at the intended boundary, or not portable enough. |
| **DEFER** | Useful, but not part of the independent harness foundation/MVP. |
| **REJECT** | Intentionally do not carry this behavior into the product. |

## 3. Repository and build topology

### 3.1 Layout

The repository is a pnpm/TypeScript monorepo (`package.json`, `pnpm-workspace.yaml`) with these main planes:

- `vendor/`: Cordis, Loader, Include, Group, HMR, Timer, logger, Schemastery, and Cosmokit. The sources are rescoped to `@deepseek-ai/*` and pinned in `vendor/README.md`.
- `packages/core/`: the product spine — `session`, `system-prompt`, `tools`, `scope`, `agent`, `agent-loop`, default model, tool presentation.
- `packages/llm/`: provider-neutral message/stream types and runtime plus DeepSeek, pi-ai, retry, and token-meter plugins.
- `packages/{fs,subprocess,shell,terminal,sandbox,lsp,web,skill,mcp}/`: capability definitions, providers, and model-facing consumers.
- `packages/{session,session-query,attachment,storage,settings,credentials}/`: durable state, projections, search, blobs, sidecar storage, user configuration, and secret references.
- `packages/{jobs,goal,subagent,workflow,schedule}/`: orchestration above the agent loop.
- `packages/{interaction,feedback,plan,todo,compaction,context,guard}/`: collaboration, state, context shaping, and loop policy.
- `packages/{api,typert,sdk,acp}/`: RPC generation/registry, Host APIs, JSON-RPC SDK, and ACP automation.
- `packages/{host,client,extensions}/`: Web Host, browser plugin runtime/UI, and runtime self-modification.
- `packages/bundle/{base,web-app,headless}/`: profile patch layers.
- `packages/preset/{agent-presets,persona}/`: per-agent standing compositions.
- `apps/cli`: the `dsh` profile launcher and plugin manager.
- `apps/web`: a two-line browser entry over the client shell.
- `python/`: Python SDK/runtime packaging.
- `native/landlock-run`: native Linux Landlock launcher packages.
- `examples/`, `packages/examples/`, `packages/test-support/`: runnable leaves, replay/snapshot infrastructure, and testkits.
- `docs/`: unusually strong subsystem documentation, generated catalogs, and architecture notes.

`AGENTS.md` is accurate about the intended dependency direction: extension plugins consume Service Definitions, while composition bundles may depend on concrete providers.

**Decision: ADAPT.** Keep a monorepo while the foundation is changing, but collapse the package count. A practical InBharat first cut should have roughly these packages/modules:

1. `kernel` (plugin lifecycle, DI, event bus, scopes),
2. `protocol` (versioned IDs/events/tool/model types),
3. `session` (log, surface, repair, persistence interface),
4. `agent-runtime` (agent registry and loop),
5. `tools` (registry, schemas, execution policy),
6. `models` (adapter registry and provider implementations),
7. `capabilities` (fs/process/sandbox/approval),
8. `orchestration` (jobs and one-shot subagents),
9. `server` (authenticated API),
10. `cli`, and
11. `ui`.

Do not create one npm package per UI card or per tiny provider until there are independent release/versioning needs.

### 3.2 Runtime and compiler assumptions

- Root `package.json` requires Node `^22.19.0 || >=24.0.0`, ESM, TypeScript 6, and pnpm `11.7.0`.
- Host and browser compiler faces are separate (`tsconfig.host.json`, `tsconfig.client.json`), and `tsdown` emits runtime bundles.
- Tests force workspace imports to `src` through `vite-tsconfig-paths`; built-artifact tests explicitly spawn `lib` outputs.
- `vitest.config.ts` uses fork workers and a per-file 100% coverage target over most `packages/*/*/src` files, while documenting GUI and generated-code exclusions.

**Decision: ADAPT.** Preserve separate Host/browser build faces and source-plane versus artifact-plane testing. Use the package manager version from `packageManager` via Corepack; do not tolerate a different local pnpm silently.

## 4. Cordis, plugins, DI, events, and effects

### 4.1 Context and service resolution

Key implementation:

- `vendor/cordis/src/context.ts`: `Context`, `Context.extend()`, `Context.isolate()`, `Context.intercept()`.
- `vendor/cordis/src/reflect.ts`: `ReflectService`, Proxy traps, `provide()`, `get()`, `set()`, `mixin()`, and dependency notifications.
- `vendor/cordis/src/service.ts`: `Service`, registration through `ctx.reflect.provide()`, isolation filtering, intercept config resolution.
- `vendor/cordis/src/registry.ts`: `Inject`, `Plugin`, `RegistryService`, `ctx.inject()`, `ctx.plugin()`.
- `vendor/cordis/src/fiber.ts`: `Fiber`, `FiberState`, `effect()`, dependency epochs, load/unload/reload, quiescent disposal.
- `vendor/cordis/src/events.ts`: `EventsService` and `emit`, `parallel`, `serial`, `bail`, `waterfall`.

A `Context` is a proxy-backed service repository. Plugins declare required services via `inject`; their fiber remains pending until all implementations are active. Implementations are registered by isolation symbol, not just by string key. Removing a service notifies dependent fibers, which unload and may later reload against a new implementation.

The strongest primitive is `Fiber.effect()`: setup and every yielded cleanup are tied to one owner, cleanup is reverse-ordered, asynchronous disposal is awaited, and reentrant unload cases have been hardened locally. Registries use this primitive so a registration returns its exact disposer.

**Decision: ADOPT the semantics; ADAPT the implementation.** InBharat should have:

- named service definitions,
- required/optional dependencies,
- scoped service resolution,
- effect-owned registrations,
- deterministic reverse teardown,
- quiescent async disposal,
- dependency replacement/reload,
- explicit event dispatch modes.

Do not expose a general Proxy-based context as the only API across all internal and external boundaries. Add typed `ServiceToken<T>` values and a non-proxy container API for foundation code. A Cordis compatibility layer can supply `ctx.foo` ergonomics for plugins.

### 4.2 Event modes

`EventsService.waterfall()` is around-middleware: a listener must call `next()` to delegate, and omission short-circuits. `serial()` bails on the first non-null/non-false/non-undefined value; `parallel()` waits for all listeners and aggregates failures; `emit()` is synchronous and does not contain listener failures by itself.

The harness frequently builds contained emitters around raw Cordis dispatch because one synchronous `emit` listener can starve later listeners. Examples include:

- `packages/core/agent/src/dispatch.ts`: `agentEvents()`;
- `packages/core/session/src/index.ts`: `invokeContainedSessionObservers()`;
- `packages/llm/llm/src/index.ts`: `LlmRuntime.emitAdaptersUpdated()`;
- `packages/subagent/subagent/src/lifecycle.ts`: `createLifecycleEmitter()`.

**Decision: ADAPT.** Keep the modes, but make failure containment a property of the event declaration rather than hand-built at each call site. Suggested declaration fields: `mode`, `failurePolicy` (`propagate`, `contain`, `aggregate`), `scopePolicy`, and whether returned promises are awaited.

### 4.3 Vendoring risk

`vendor/README.md` records extensive behavior changes to Cordis/Loader/Include. The most material are reentrant fiber disposal hardening, transactional Loader updates and rollback, lazy config resolution, exact-path HMR, inserted-row patchability, serialized Include updates, durable writes, and `disabled` expression interpolation.

**Decision: REJECT the rescoped vendored-fork strategy.** Either:

1. upstream the required lifecycle fixes and depend on a published Cordis release, or
2. extract an InBharat kernel with only the needed behavior and its own tests.

The current fork is good engineering, but it is an ongoing framework-maintenance commitment independent of the harness product.

## 5. Loader configuration, profiles, bundles, and agent presets

### 5.1 Loader tree

Key implementation:

- `vendor/loader/src/index.ts`: `Loader`, lazy interpolation, service readiness, import and entry lifecycle hooks.
- `vendor/loader/src/config/entry.ts`: `Entry`, transactional update/replacement and rollback.
- `vendor/loader/src/config/group.ts`: `EntryGroup`, concurrent application and rollback.
- `vendor/loader/src/config/tree.ts`: `EntryTree`, import, create/update/remove, settlement.
- `vendor/loader/src/config/isolate.ts`: `Realm`, `LocalRealm`, `GlobalRealm`, entry `isolate` and `intercept` semantics.
- `vendor/include/src/index.ts`: patch application and YAML `!!js` expression support.

Rows are identified by stable `id`, import a plugin by module specifier, and carry `config`, `inject`, `isolate`, `intercept`, and `disabled`. Load order is service-driven, not row order. Entry replacement imports first, disposes the previous fiber, starts the candidate, and restores the old plugin on failure.

**Decision: SIMPLIFY.** Preserve stable row IDs, typed config validation, explicit dependency injection, isolation labels, and transactional reload. Remove arbitrary `!!js` evaluation from ordinary user configuration. Replace it with a small declarative interpolation vocabulary (`env`, `homePath`, platform condition, CLI value) whose inputs can be audited and serialized.

### 5.2 Product profiles and bundles

Key implementation:

- `packages/boot/app-boot/src/profile.ts`: `PROFILE_TEMPLATES`, `initProfile()`, `loadProfile()`, `resolveBundleDir()`, `composeEntries()`.
- `apps/cli/src/profile-boot.ts`: `prepareProfile()`, `composeProfile()`, `runProfile()`.
- `packages/bundle/base/cordis.patch.yml`: the full shared product assembly.
- `packages/bundle/web-app/cordis.patch.yml`: Web Host/client/UI assembly and disabling host-plane agent tools in favor of presets.
- `packages/bundle/headless/cordis.patch.yml`: one-shot task surface.
- Bundle manifests: `packages/bundle/*/package.json` under `dsh.bundle.patch`.

A profile is a directory under `$DSH_HOME/profiles/<name>` with its own manifest, dependencies, and patch layer. A bundle is an npm package exporting one patch file. Effective order is:

1. bundle patches in profile order,
2. profile `cordis.patch.yml`,
3. home `cordis.patch.yml`,
4. CLI `--patch` overlays,
5. launcher-owned overlays such as telemetry disable and shipped preset root.

`apps/cli/src/plugin.ts` forwards package management to pnpm and automatically adds dependencies declaring `dsh.bundle` to the profile bundle list.

**Decision: SIMPLIFY.** InBharat should initially ship two declarative manifests (`headless`, `server`) plus a user override file. Use explicit merge semantics with a schema and provenance output. Do not embed a package manager as a profile-management command in the MVP; install plugins through an administrator-controlled registry/allowlist. Preserve `--dump-config` and provenance because it is operationally valuable.

### 5.3 Per-agent presets

Key implementation:

- `packages/preset/agent-presets/src/index.ts`: `AgentPresets`, standing mount generations, `mount()`, `composeFrom()`, `recompose()`, `standingKeyFor()`.
- `packages/preset/agent-presets/src/mount.ts`: `PresetTree`, `mountPreset()`, `inactiveRows()`, `leakedServices()`, `serviceForAgent()`.
- `packages/preset/persona/src/index.ts`: scoped persona replacement.
- `packages/core/scope/src/index.ts`: `bindScopeParent()` and scope ancestry.

A preset is mounted once per generation under a standing scope. Each agent is parented to that scope, so tool/prompt/skill registrations are shared while per-session state remains keyed by `Session`/`Agent`. A child joins the exact generation its parent uses, avoiding composition drift. The mount rejects inactive rows and process-global service leaks. Blank sessions may be recomposed; once a session has produced work, the composition is locked.

**Decision: ADAPT.** This is a sophisticated solution to per-agent capability composition and prefix stability. Keep the invariant that composition is fixed before the first model request and is logged. Simplify implementation by compiling a preset into an immutable `CapabilitySet`/`PromptSet` descriptor and binding that descriptor to the agent, rather than mounting a full Loader subtree per generation. If Cordis is retained, the standing-scope design is worth preserving.

Known inherited limitations to avoid:

- composition generations are not reclaimed (`AgentPresets.ensureStanding()` TODO),
- the file stamp watches only `agent.cordis.yml`, not adjacent assets/skills,
- health checks validate shape, not actual plugin activation,
- preset trust is presentation metadata, not enforcement.

## 6. Model adapters and streaming

### 6.1 Provider-neutral vocabulary and runtime

Key implementation:

- `packages/llm/llm/src/types.ts`: `ContentBlockMap`, `FinishReasonMap`, `StreamChunk`, `ToolSchema`, `GenerateOptions`, model/provider metadata.
- `packages/llm/llm/src/message.ts`: immutable identified `Message`, `MessageSourceMap`, `createUserMessage()`, `createAssistantMessage()`, `createToolResultMessage()`.
- `packages/llm/llm/src/index.ts`: `LlmAdapter`, `LlmRuntime`, `PreparedLlmCall`, adapter/directory registration, `prepareCall()`, `adapterStream()`.
- `packages/llm/llm/src/assembler.ts`: `BlockAssembler`.

The stream protocol supports indexed block start/delta/end frames, text, reasoning, tool-call argument deltas, usage, and a terminal finish. Adapter throws are normalized at the final adapter boundary into a terminal `error` or `aborted` finish chunk. Middleware and consumer failures remain thrown, preserving fault ownership.

`LlmRuntime.prepareCall()` is particularly strong: it captures one adapter registration, resolves exact-model metadata and adapter defaults, deep-freezes the config, exposes the retry policy/context, and allows exactly one dispatch whose call config must match. HMR cannot combine one adapter's capability resolution with another adapter's stream.

**Decision: ADOPT.** This should be the model-neutral core of InBharat with minor naming changes. Preserve:

- provider route separate from model ID,
- advisory model catalogs that do not reject unlisted IDs,
- exact-model capability resolution,
- immutable identified messages,
- block-indexed streaming,
- terminal error chunks,
- registration-bound one-shot call preparation,
- cross-provider replay-state stripping,
- provider-neutral error codes and retry hints.

Adapt `ContentBlockMap` and `FinishReasonMap` into explicit versioned protocol unions for persistence/wire use. TypeScript declaration merging can remain an internal extension convenience, not the durable schema authority.

### 6.2 Provider adapters

- `packages/llm/llm-deepseek/src/adapter.ts`: `DeepSeekAdapter`, operation-local config/key snapshot, abort and idle watchdog, HTTP error normalization.
- `packages/llm/llm-deepseek/src/sse.ts`: spec parser through `eventsource-parser`, mandatory `[DONE]`.
- `packages/llm/llm-deepseek/src/serialize.ts` and `translate.ts`: provider request/stream translation.
- `packages/llm/llm-pi-ai/src/adapter.ts`: `PiAiAdapter`, immutable provider snapshots, model/reasoning metadata, attachment resolution.

**Decision: ADAPT.** Keep DeepSeek as one optional adapter, never the default assumption of the core. Remove DeepSeek product identity, user-id headers, fixed telemetry endpoints, and default provider/model from neutral bundles. Prefer a direct OpenAI-compatible adapter plus native adapters for providers whose semantics differ. The pi-ai adapter is useful for breadth but adds a second catalog/config model; put it behind an optional package.

The adapter contract should require:

- operation-local endpoint + credential resolution,
- an immutable request snapshot,
- caller cancellation and idle timeout,
- normalized provider request IDs and retry-after values,
- explicit input/output modality declarations,
- deterministic serialization tests against captured wire requests.

## 7. Scoped tools, schemas, execution, and Code Mode

### 7.1 Scope primitive

Key implementation:

- `packages/core/scope/src/index.ts`: `ScopeKey`, `createScope()`, `bindScopeParent()`, `scopeChainOf()`, `scopeTarget()`.
- `packages/core/scope/src/store.ts`: `NamedEntries`, `AnonymousEntries`, `ScopedLayers`.

Registrations are layered global → ancestor scopes → exact scope, with nearest shadowing. Events flow upward to enclosing scope listeners, not downward. Layer mutations are effect-owned and clean up empty scope layers.

**Decision: ADOPT.** This is a reusable capability-scoping primitive. Document the two distinct operations explicitly in InBharat: registry lookup inherits down the scope chain; event observation propagates up the chain.

### 7.2 Tool registry and pipeline

Key implementation:

- `packages/core/tools/src/index.ts`: `ToolDefinition`, `ToolRuntime`, `ToolExecution`, `ToolExecutionResult`, `register()`, `restrict()`, `guard()`, `executionMode()`, `execute()` and staged scheduler methods.
- `packages/core/tools/src/schema.ts`: author DSL, `defineTool()`, compile-time inference, runtime argument validation.
- `packages/core/tools/src/json-schema.ts`: enforced JSON Schema subset and validator.
- `packages/core/agent-loop/src/tool-calls.ts`: parallel/exclusive scheduling, ordered commits, synthetic cancellation results.

The execution path is:

1. snapshot and freeze arguments;
2. resolve scoped visibility and Code Mode collapse;
3. `tools/pre-execute` allow/deny/ask waterfall;
4. monotonic guards;
5. `tools/execute` around-dispatch wrappers;
6. tool body with caller/wrapper signals fused;
7. canonical output snapshot, schema validation, content rendering, optional presentation metadata;
8. `tools/post-execute` accept/replace/block;
9. definition-owned final content transform;
10. deep-frozen `tools/result` observer notification;
11. loop-owned `tool/result` durable event in model order.

Only exact `true` from `isConcurrencySafe()` permits overlap; everything else fails closed to exclusive. Pre/post policy remains ordered even while tool bodies overlap. Started work is drained on abort, while skipped calls receive synthetic `ABORTED_BEFORE_DISPATCH` results.

**Decision: ADOPT.** This is one of the best parts of the repository. InBharat should keep the split between canonical value, model rendering, and replayable UI metadata. Do not let tools return arbitrary text as the only contract.

### 7.3 Schema format

The repository supports a deliberately small JSON Schema subset: object/array/scalars, `properties`, `required`, boolean `additionalProperties`, `items`, scalar `enum`/`const`, exact-one `oneOf`, and annotations. Unsupported keywords reject rather than being silently ignored.

**Decision: ADAPT.** Preserve fail-loud schema enforcement but use a standard JSON Schema validator/validator compiler for protocol-facing schemas. Keep a small first-party DSL for TypeScript inference. Version the supported subset and expose it through the API so MCP/remote tools cannot assume unsupported keywords are honored.

### 7.4 Code Mode and model-authored execution

- `packages/core/tools/src/code-mode.ts`: generated SDK and `run_code` transport.
- `packages/code-runtime/code-runtime-worker-thread/src/index.ts`: `WorkerThreadCodeRuntime`.
- `packages/workflow/workflow-worker-thread/src/index.ts`: worker + escapable VM workflow engine.

Both worker subsystems explicitly state they are containment, not security boundaries; Code Mode gives model code bash-equivalent trust.

**Decision: REJECT as a security boundary; DEFER as an opt-in trusted optimization.** Do not advertise worker-thread or `vm` isolation as safe execution. If InBharat later supports Code Mode, execute in the same sandbox provider used for shell commands (container/microVM/process sandbox), with an explicit risk tier and administrator policy.

## 8. Agent registry, loop, turns, steps, and live events

### 8.1 Agent interface and registry

Key implementation:

- `packages/core/agent/src/runtime-types.ts`: `Agent`, `AgentStatus`, `PreStepDecision`, live `agent/*` events.
- `packages/core/agent/src/index.ts`: `AgentRegistry`, `AgentFactory`, `AgentHandle`, transactional create/resume, initiator `AsyncLocalStorage`.
- `packages/core/agent/src/inbox.ts`: event-sourced `Inbox` projection.
- `packages/core/agent/src/dispatch.ts`: scope-coupled `agentEvents()`.

The registry separates an agent from its owning `AgentHandle`; only the holder can dispose it. Creation/resume setup runs while the agent and session are unpublished. An optional synchronous setup commit revalidates immediately before publication. Session and agent announcements are paired with disposal on rollback.

`AgentRegistry.withInitiator()` provides same-process causal attribution without pretending that ambient context is authorization.

**Decision: ADOPT.** Preserve unpublished setup, owner handles, paired lifecycle notifications, exact instance checks, and explicit causal attribution.

### 8.2 Default loop

Key implementation:

- `packages/core/agent-loop/src/agent.ts`: `ReactLoopAgent`.
- `packages/core/agent-loop/src/index.ts`: `AgentLoop`, factory ownership, create/resume transaction.
- `packages/core/agent-loop/src/tool-calls.ts`: tool scheduler.
- `packages/core/agent-loop/src/runtime-context.ts`: durable dynamic-context projection.

A **turn** opens before input claim and closes once no work is owed. A **step** is one model request and its tool calls. The durable path is:

`turn/start → [step/start → entered user/message* → request/header/context → assistant/chunk* → assistant/message → tool/call/result* → step/end]* → turn/end`.

`agent/pre-step` can reject or rewrite the claimed message batch. `agent/request` can replace model call configuration but cannot mutate messages. `agent/request-error` can own retry. `agent/turn-stopping` can enqueue more work by steering; the loop then re-reads the inbox.

The inbox distinguishes:

- `next-turn`: one ordinary message per later turn,
- `next-step`: steering/context for the nearest step,
- waking versus non-waking delivery.

Cancellation carries structured causes (`user`, `parent`, `hook`, `disposed`), can optionally preserve pending inbox work, and waits for whole-agent quiescence through `whenIdle()`.

**Decision: ADOPT.** Keep this lifecycle. Simplify the number of extension events initially, but do not collapse turn and step: tool continuation, retries, compaction, steering, and background completion all need the distinction.

One recommended change: introduce explicit `attempt` identity within a step. Today retries are represented by multiple chunk sequences under the same turn/step. A durable attempt number makes telemetry, replay, and partial failure analysis simpler.

## 9. Session log, context, and prompts

### 9.1 Event-sourced session

Key implementation:

- `packages/core/session/src/types.ts`: `SessionHeader`, `SessionEventMap`, `SessionEvent`, `SurfaceOp`, `TurnEndReasonMap`.
- `packages/core/session/src/index.ts`: `Session`, `SessionStore`, `append()`, `deriveMessages()`, `fork()`.
- `packages/core/session/src/surface.ts`: `SurfaceManager`, `foldSurface()`, `deriveEventMessage()`.
- `packages/core/session/src/request-header.ts`: `canonicalHeader()`, `headerEquals()`, `foldRequestHeader()`.

Every append snapshots into lossless JSON, validates the fixed envelope and surface transition, deep-freezes the accepted event, commits to the in-memory log, and then emits a contained post-commit notification. Sequence numbers equal array indexes and are contiguous.

The model-visible surface is a projection over three event types: `user/message`, `assistant/message`, and `tool/result`. `surfaceOp: 'append'` adds a node; `surfaceOp: { op: 'replace' }` replaces an inclusive current surface range and must cite every shadowed node through `sourceEventSeqs`. Raw log history remains intact. This permits compaction without destroying the human transcript.

Request system text, tools, model route, reasoning effort, and adapter defaults are logged in full `request/header` snapshots. The model request is therefore reconstructable from the session log.

**Decision: ADOPT.** This should be InBharat's authoritative data plane.

Adaptations required:

- Define the event registry in a language-neutral schema catalog, not only TypeScript declaration merging.
- Give every event a version and an explicit `requiredForReplay`/`ignorable` rule.
- Start at format version 1 with migrations and compatibility tests; do not copy `SESSION_FORMAT_VERSION = 0`.
- Add a hash/checksum chain or segment checksums if tamper evidence matters.
- Make single-writer ownership explicit. Current sequence semantics assume serialized append inside one live session.

### 9.2 Prompt assembly

Key implementation:

- `packages/core/system-prompt/src/index.ts`: `SystemPrompt`, `PromptSection`, `PromptContext`, `PromptAssembly`, `assemble()`, `renderPrompt()`.
- `packages/core/agent-loop/src/runtime-context.ts`: model-visible snapshots become durable user messages.
- `packages/context/agent-instructions/src/index.ts`: baseline and touched-path instruction reconciliation.
- `packages/context/session-reference/src/index.ts`: bounded, explicitly untrusted cross-session recall.
- `packages/context/time-context/src/index.ts` and `tmux-context/src/index.ts`: optional pre-step context.

Prompt sections, dynamic runtime context, tool schema providers, and variables are scoped registries. Scoped sections/variables shadow global names. A `complete` section can replace all other prompt prose while still allowing tools/context to resolve. Tool schemas are detached and ordered before the prompt-assembly waterfall.

Runtime context is not ephemeral: it is rendered into identified user messages and appended to the session log. `session-reference` correctly labels imported session text as untrusted and instructs the model not to follow embedded directives.

**Decision: ADAPT.** Preserve ordered named sections, strict variables, scoped shadows, durable dynamic context, and untrusted-context framing. Replace free-form order numbers with named phases plus deterministic priority within a phase. Keep the rule that model-visible context is logged.

## 10. Permissions, approvals, sandbox, filesystem, process providers

### 10.1 Approval seam

Key implementation:

- `packages/interaction/user-approval/src/index.ts`: `ApprovalService`, `ApprovalRequest`, `ApprovalPolicy`, `request()`, `setPolicy()`.
- `packages/interaction/user-approval/src/types.ts`: `ApprovalRequestId`, `ApprovalOutcome`.
- `packages/core/tools/src/index.ts`: `serviceAsk()`.

Every ask is turn-enclosed and logs `approval/asked` plus exactly one `approval/decided`. Missing or throwing answerers fail closed to `unavailable`; only `allowed-once` grants. Cancellation wins over a late answer. Policy `never` rejects before dispatch and is also exposed to the model through durable runtime context.

**Decision: ADOPT.** Add actor identity, policy-rule ID, risk class, and optional expiry to the audit record. Keep grants one-shot by default. A future persistent grant must be a separate explicit policy object, never another string outcome.

### 10.2 Permission presets

- `packages/interaction/permission-presets/src/index.ts`: `PermissionPresetService` combines sandbox mode and approval policy while preserving the underlying independent event folds.

**Decision: ADAPT.** Keep named bundles for UX, but store and enforce the underlying capabilities independently. Add network, subprocess, credential, and host-integration permissions; the current preset only combines file sandbox and approval.

### 10.3 Sandbox and providers

Key implementation:

- `packages/sandbox/sandbox/src/index.ts`: `SandboxProvider`, `SandboxPolicy`, `ConfinedArgv`, enforcement facts and failure dialects.
- `packages/sandbox/sandbox-policy/src/index.ts`: `SandboxPolicyService.resolve()` and durable per-session mode.
- `packages/sandbox/sandbox-local/src/index.ts`: `LocalSandboxProvider`, Linux bwrap/Landlock, macOS Seatbelt, Windows restricted token/ACL chain.
- `packages/shell/bash-sandbox/src/index.ts`: sandbox-consuming shell executor.
- `packages/fs/fs-sandbox/src/index.ts`: in-process filesystem policy fence.
- `packages/subprocess/subprocess/src/index.ts` and `subprocess-local/src/index.ts`: managed process-tree seam and local provider.
- `packages/fs/fs/src/index.ts` and `fs-local/src/index.ts`: filesystem seam, stable target identity, atomic writes/edits, stale guards.

The design separates a per-call policy from the provider. Providers report whether enforcement is `full` or `partial`, plus backend-specific denial and runner-failure signatures. Confined mode fails closed when no runner is usable. Subprocesses scrub credential-shaped and `DSH_*` environment variables unless explicitly supplied.

**Decision: ADAPT.** Keep the seams and fail-closed behavior, but require a security-grade backend for untrusted model code:

- local development: OS sandbox with accurately reported limitations,
- server/multi-user: container or microVM with filesystem, process, network, resource, and credential isolation,
- remote provider: an execution-world identifier shared by fs/process/shell.

Do not call `fs-sandbox` a kernel boundary; its own source accepts a symlink TOCTOU residual. Windows ACL enforcement is explicitly partial. The InBharat policy should be capability-based (`read roots`, `write roots`, `network egress`, `spawn`, `host UI`, `credential refs`) rather than only three file modes.

## 11. Jobs, goals, subagents, workflows, and schedules

### 11.1 Background jobs

Key implementation:

- `packages/jobs/jobs/src/index.ts`: abstract `JobRegistry`.
- `packages/jobs/jobs/src/types.ts`: `JobStart`, `JobHooks`, `JobSnapshot`, `JobOutcome`.
- `packages/jobs/jobs-local/src/index.ts`: `LocalJobRegistry`.
- `packages/jobs/tool-jobs/src/index.ts`: `job_output`, `job_list`, `job_kill`, completion delivery.

Jobs are owner-scoped, first-wins on settlement, controller-gated before start, and cleaned up with the exact agent owner. Output reads distinguish streaming delta from idempotent final output. Completion notices are bounded to avoid self-exciting wake loops.

**Decision: ADAPT.** Keep the producer/registry/controller separation and owner authorization. Replace predictable `<kind>-N` IDs with opaque IDs. Add a durable backend before claiming resume semantics; the current local provider loses every record on process restart and can stall teardown forever if a producer's cancel returns but `done` never settles.

### 11.2 Goals

Key implementation:

- `packages/goal/goal/src/index.ts`: `GoalService`, event-sourced full snapshots, compare-and-set revisions, activation state.
- `packages/goal/goal-round-driver/src/index.ts`: automatic continuation and durability checkpointing.
- `packages/goal/tool-goal/src/index.ts`: human-authority and model completion/block policy.

Durable goal phase and revision are separate from process-local automatic-continuation authority. Resume disarms a goal; a human-authorized action must rearm it. The driver checkpoints before opening another round and has race fences for competing input and stale revisions.

**Decision: DEFER.** This is well designed but product-specific. InBharat's foundation only needs generic durable tasks/workflows and turn scheduling. Reintroduce goals later as a plugin over the event log.

### 11.3 One-shot subagents

Key implementation:

- `packages/subagent/subagent/src/index.ts`: `SubagentRuntime`, provider registry, capability checks, lifecycle events.
- `packages/subagent/subagent/src/types.ts`: `SubagentProvider`, `SubagentStartRequest`, `SubagentRun`, `SubagentResult`.
- `packages/subagent/subagent-in-process-driver/src/index.ts`: unpublished child creation, inherited policy, one-turn settlement and disposal.
- `packages/subagent/subagent-spawn-in-process/src/index.ts`: fresh child.
- `packages/subagent/subagent-fork-in-process/src/index.ts`: completed-turn prefix child.
- `packages/subagent/tool-subagent/src/index.ts`: foreground/background model-facing adapter.

Provider capabilities are checked before start; unsupported persona/tool filter/depth/output schema fails loud. Provider ownership transfers only after the child is published. A `SubagentRun` result never rejects for child-level failure and must be disposed to quiescence.

**Decision: ADAPT.** Ship one-shot fresh children first. Add fork only after request-prefix compatibility is tested across model providers. Preserve exact parent authority, depth budget in durable metadata, inherited policy snapshot, tool restriction, and holder-owned disposal.

### 11.4 Continuable subagents

Key implementation:

- `packages/subagent/subagent/src/continuation.ts`: `SubagentContinuationManager`, stable child IDs, `Activation`, cold resume, parent reports, child-first draining.
- `packages/subagent/tool-subagent-control/src/index.ts`: `send_message`, `interrupt_agent`.
- `packages/subagent/tool-subagent-report/src/index.ts`: child-scoped `report`.
- `packages/subagent/subagent/src/list-children.ts`: durable child/descendant enumeration.

A durable child can have at most one process-local activation. Its inbox is the only FIFO turn queue. Follow-up cold-resumes from persistence when absent. Interrupt preserves queued work and descendants. Parent/ancestor authorization is exact-instance and lineage based. Disposal is child-first over a dynamic ownership forest.

**Decision: DEFER.** The implementation is thoughtful but very complex. It depends on persistence, projections, precise ownership graphs, scoped compositions, and multiple race cutoffs. InBharat should first stabilize durable sessions, one-shot children, and authenticated external control. Add continuable children only with explicit conformance tests for cold resume, duplicate activation, parent loss, and process crash.

### 11.5 Workflows and schedules

- `packages/workflow/workflow/src/index.ts`: `WorkflowEngine` and lifecycle contract.
- `packages/workflow/workflow-worker-thread/src/index.ts`: worker/VM execution.
- `packages/schedule/schedule/src/index.ts`: root-agent schedule runtime and tools.

**Workflow decision: REJECT for untrusted scripts; DEFER a safe declarative workflow DSL.**  
**Schedule decision: DEFER.** Scheduling needs durable leases, clock semantics, and multi-process ownership beyond this process-local runtime.

## 12. Cancellation, recovery, resume, fork, and replay

### 12.1 Cancellation model

Cancellation is consistently represented by `AbortSignal`, structured agent causes, and owned cleanup:

- `ReactLoopAgent.cancel()` aborts active work and optionally preserves inbox state.
- `ToolRuntime` fuses caller and wrapper signals, never permits wrapper replacement to detach caller cancellation, and drains started work.
- model adapters combine consumer/caller signals and close iterators on early return;
- job/subagent/workflow owners cancel and await their resources;
- CLI shutdown escalates from graceful disposal to forced process exit in `apps/cli/src/process-shutdown.ts`.

**Decision: ADOPT.** Define one cross-subsystem cancellation contract: first cause wins, started work must settle or hit a bounded owner escalation, pending work declares whether it is preserved, and teardown awaits quiescence.

### 12.2 Crash repair

- `packages/core/session/src/repair.ts`: `interruptedTurnClosers()`.
- `packages/session/session-persistence/src/coordinator.ts`: shared write/recovery orchestration.

A complete interrupted tail is preserved. Missing tool results are synthesized with distinct `TOOL_NOT_STARTED` or `TOOL_OUTCOME_UNKNOWN`, then any open step and turn are closed with `interrupted`. A torn physical record is dropped before repair. This avoids blindly retrying potentially side-effecting tools.

**Decision: ADOPT.** This is an excellent recovery policy. Add a durable `recovery/synthesized` provenance field or envelope marker so UIs and audits do not infer synthesis only from error codes.

### 12.3 Persistence and resume

- `packages/session/session-persistence/src/index.ts`: `SessionPersistence`, `prepare()`, `load()`, `inspect()`, `readFrom()`.
- `packages/session/session-persistence-jsonl/src/index.ts`: `JsonlSessionPersistence`, atomic/lazy materialization, zstd frames, fsync and rollback.
- `packages/session/session-persistence-sqlite/src/index.ts`: `SqliteSessionPersistence`, transactional append and repair.
- `packages/session/session-checkpoint-policy/src/index.ts`: flush before model and top-level tool side effects.
- `packages/core/agent-loop/src/index.ts`: `AgentLoop.resume()` and `resumeWith()`.

**Decision: ADAPT.** Preserve the persistence coordinator, lazy materialization, immutable inspection, prepared-session ownership, revision checks, JSONL and SQLite options, and semantic checkpoints. Start InBharat with SQLite for indexed/multi-session service use and JSONL for portable local mode/export. Add migrations from day one and encrypt sensitive local stores where required.

### 12.4 Fork

- `SessionStore.fork()` in `packages/core/session/src/index.ts` takes a stable live prefix and rejects an open-turn boundary.
- `completedTurnPrefix()` in `packages/subagent/subagent-fork-in-process/src/index.ts` excludes the in-flight parent turn.

**Decision: ADAPT.** Make fork a persistence-level operation that works for live and cold sessions under an optimistic source revision. Fork only at a balanced turn end and record source session, source revision, and inclusive boundary. Do not inherit process-local inbox or approval state implicitly.

### 12.5 Replay

- `packages/test-support/llm-replay/src/index.ts`: `deriveReplayScript()`, `ReplayEntry`, request substitution, session-script binding.
- `vitest.snapshot.config.ts`: keyless replay is the default; record/refresh are explicit.

Replay derives model responses from durable `assistant/chunk` events, handles child scripts after `seedLength`, and requires explicit sidecars for thrown/hanging streams that a completed log cannot reconstruct.

**Decision: ADAPT.** Promote deterministic replay from test support into a first-class diagnostic provider, but bind scripts by explicit recorded request/attempt IDs rather than first-call order. The source itself notes concurrent subagents need a first-call ordinal.

## 13. Attachments, storage, settings, and credentials

### 13.1 Attachments

- `packages/attachment/attachment/src/index.ts`: abstract `AttachmentStore`.
- `packages/attachment/attachment/src/types.ts`: `ImageAttachmentRef`, image limits.
- `packages/attachment/attachment-local/src/index.ts` and `store.ts`: content-addressed local image backend.
- `packages/host/apiproxy/src/api-proxy.ts`: batch validation/commit and authorized retrieval paths.

Images are validated before any member of a batch is saved, committed before their session event, addressed by digest-backed opaque IDs, and revalidated on read.

**Decision: ADOPT.** Generalize from images to typed blobs while retaining media-specific validators. Add reference counting/mark-and-sweep over session lineage and fork references; current objects are retained indefinitely.

### 13.2 Generic storage and domain form

- `packages/storage/storage/src/index.ts`: `Storage`, `BackendRegistry`, mounted forms.
- `packages/storage/storage-json/src/index.ts`: JSON backend.
- `packages/storage/storage-sqlite/src/index.ts`: SQLite backend.
- `packages/storage/storage-domain/src/index.ts`: `DomainFacility`, schema-validated sidecar domains.

**Decision: ADAPT.** Keep sidecar state separate from the session log when it is not model history. Simplify to one transactional storage interface initially. Retain domain version stamps, schema validation, single-open ownership, and explicit routing. Avoid synchronous SQLite APIs on server hot paths.

### 13.3 Settings

- `packages/settings/settings/src/index.ts`: `SettingsProvider`, namespace registration, layering, revisions, redaction, serialized writes.
- `packages/settings/settings-file/src/index.ts`: comment-preserving YAML/JSON provider, atomic writes, watcher reconciliation.

Settings resolve schema defaults → composition base → user section. Writes can merge, replace, or mutate a path with compare-and-set revision checks. Secret-role fields can be redacted for wire descriptions.

**Decision: ADAPT.** Preserve namespaces, layered defaults, revisioned writes, redaction, atomic file updates, and last-good hot reload. Fix the documented property-key hazard before reuse: `cloneJsonShaped()`/`mergeLayers()` contain a TODO to use property-safe construction for keys such as `__proto__`.

### 13.4 Credentials

- `packages/credentials/credentials/src/index.ts`: `CredentialProvider`, `credentialRef()`, `resolve()`, `describe()`, `set()`, `unset()`.
- `packages/credentials/credentials-local/src/index.ts`: `LocalCredentialProvider` and strict source precedence.

The local precedence is inherited environment → managed credentials file → project `.env` → user `.env`. The managed file is owner-only, atomically written, comment-preserving, hot-reloaded, and never materialized into process environment. Writes reject when an inherited environment value would shadow them. Diagnostics never print secret values.

**Decision: ADOPT.** Add OS keychain/KMS providers and scoped reference authorization. MCP headers, model adapters, web providers, and remote sandbox credentials should all carry `CredentialRef`, never literals in plugin config.

## 14. Session projections, query, telemetry, and feedback

### 14.1 Projections and query

- `packages/session/session-projection/src/index.ts`: `ProjectionDefinition`, `SessionProjectionRegistry`, snapshot/checkpoint/restore.
- `packages/session/session-projection-cache`: durable projection checkpoint carrier.
- `packages/session-query/session-query/src/index.ts`: `SessionQueryEngine`, live-preferred corpus, exact reads/traces/filtering.
- `packages/session-query/session-query-sqlite`: full-text provider.

Projection units are pure `init/apply/view` folds with a `stateVersion`, eagerly driven over committed events. Same-state reference means no change notification. Durable checkpoint rows are non-authoritative shortcuts and are discarded/refolded on version or log-watermark mismatch.

**Decision: ADAPT.** Preserve pure folds and checkpoint invalidation. InBharat can avoid a dynamic projection registry at first: compile a fixed set of core projections and add plugin projections once the event schema registry is stable. Query should always prefer live state and fall back to persistence under a source revision.

### 14.2 Telemetry

- `packages/session/session-telemetry/src/index.ts`: `SessionTelemetryBackend`, `SessionTelemetryCoordinator`, ledger/ops records, `session-telemetry/record` waterfall.
- `packages/session/session-telemetry-otel/src/index.ts`: `OpenTelemetrySessionBackend` and `FULL` / `FEEDBACK_ONLY` / `DISABLED` modes.

The capture service mirrors complete session event data and provides a synchronous redaction waterfall, but ships no redaction rules. Full mode therefore exports raw captured data. OTel transport batching/retry remain SDK-owned, with a harness-owned shutdown bound. Default mode is disabled.

**Decision: ADAPT.** Keep default-off and OTel, but make safe redaction/allowlisting mandatory before any exporter can activate. Prefer structured metrics/traces by default; raw transcript export should require explicit per-deployment and preferably per-session consent. Remove product-specific production endpoints and anonymous identity from the neutral core.

### 14.3 Feedback

`packages/feedback/message-feedback/src/index.ts` demonstrates a good sidecar pattern: feedback is keyed to exact persisted assistant-message identity, versioned with compare-and-set, stored outside the transcript, and checked against durable session history.

**Decision: ADAPT.** Keep as an optional domain plugin. Do not couple feedback submission to raw transcript upload unless the user sees and accepts that policy.

## 15. UI, server, RPC, and CLI

### 15.1 Host HTTP and API

Key implementation:

- `packages/host/webserver/src/index.ts`: `WebServer`, exact/prefix routes, upgrade routes, fallback seat, index taps, connection teardown.
- `packages/host/apiproxy/src/index.ts`: `ApiProxyService`.
- `packages/host/apiproxy/src/api-proxy.ts`: large Host domain API, session streams, approvals/questions, attachments, settings, credentials, model catalog, exports.
- `packages/client/connection/src/index.ts`: HTTP/WebSocket bridge, authority trust fence, loopback-only privileged methods.
- `packages/api/gateway/src/index.ts`: `TypertGatewayService`.
- `packages/typert/{generator,registry,loader,protocol}`: TypeScript graph generation and runtime descriptors.

The Web transport correctly distinguishes ordinary catalog calls from privileged host/configuration calls and pins the latter to loopback. However, source comments explicitly say the host allowlist is a DNS-rebinding fence, not authentication. A trusted LAN origin can still create an agent that runs the deployment's normal command/file tools.

**Decision: REBUILD the external boundary.** InBharat should have:

- authenticated users/service principals,
- per-session ACLs,
- CSRF/origin protection in addition to authentication,
- explicit capabilities for host-native dialogs and credential/settings writes,
- rate limits and request/body limits,
- auditable RPC method schemas,
- no unauthenticated non-loopback command-execution surface.

`WebServer` itself is small and reusable (**ADOPT**), but the exposed product API and trust model must be rebuilt.

### 15.2 Typert

Typert generates strict Remote invocation descriptors from TypeScript and falls back to source-signature markers when strict artifacts are unavailable. `TypertGatewayService` resolves service/context/lookup providers and validates request/result codecs.

**Decision: REBUILD.** For an independent and potentially multi-language harness, use explicit versioned RPC schemas (OpenAPI/JSON Schema/Protobuf) as source of truth. Do not derive wire contracts from TypeScript parameter names or source text. Keep the useful concepts: contextual receiver identity, lookup providers, exact argument sets, cancellation, and typed business errors.

### 15.3 Browser client architecture

Key implementation:

- `apps/web/src/main.ts`: `AppWebEntry` bootstrap.
- `packages/client/web/src/boot.tsx`: browser kernel, boot manifest, module prefetch, client Cordis Loader, activation sweep.
- `packages/client/modules/src/index.ts`: Host scan of `dsh.client` packages, boot graph, plugin bundle route.
- `packages/client/runtime/src/client/index.ts`: session/workspace object layer and stream handling.
- `packages/client/ui-slots/src/index.ts`: strongly typed `SlotMap`, `SlotCore`, store/inject/render shares.
- `packages/client/ui-layout/src/client/index.ts`, `ui-conversation`, `ui-tool`, and other `ui-*` packages: feature composition.

The UI is itself a plugin system. The Host scans Loader rows, serves hashed client bundles, injects a boot graph into HTML, and the browser creates a second Cordis tree. A typed slot registry composes layout and feature seats. Boot fails loud if any entry remains inactive.

**Decision: REBUILD a simpler UI; DEFER browser-side arbitrary plugin loading.** Preserve:

- projection-driven client state,
- a narrow event stream plus baseline/resync,
- pure tool presentation metadata,
- keyed renderer fallback,
- boot that fails visibly rather than rendering a partial app.

Do not initially reproduce 39 client packages, dual Host/client plugin manifests, a browser Loader, HMR bundle graph, or the full slot type system. Start with one React application and an explicit extension registry compiled at build time. Add runtime UI plugins only after origin isolation, signing, and API compatibility are defined.

### 15.4 CLI and automation protocols

Key implementation:

- `apps/cli/src/bin.ts`: dynamic mode dispatch.
- `apps/cli/src/args.ts`: launcher flags and pass-through app arguments.
- `apps/cli/src/profile-boot.ts`: profile boot and signal shutdown.
- `apps/cli/src/process-shutdown.ts`: bounded escalation.
- `packages/boot/cmdline/src/index.ts`: app-owned CLI parsing via `cmdlineArgs` and `appExit`.
- `packages/acp/acp/src/index.ts`: ACP stdio server.
- `packages/sdk/server/src/server.ts` and `packages/sdk/protocol/src/types.ts`: small JSON-RPC SDK runtime.

**CLI decision: SIMPLIFY.** Keep `server`, `run`, `config dump`, `doctor`, and explicit plugin management. Keep bounded signal shutdown and app-owned subcommand parsing. Drop automatic pnpm forwarding from the foundational CLI.

**ACP decision: ADAPT.** ACP is useful for editor/automation integration but currently exposes a narrow text-only subset and no resume. Keep it as an adapter over the same authenticated/session service, not a separately owned agent implementation.

**JSON-RPC SDK decision: SIMPLIFY.** Consolidate ACP/JSON-RPC/Web RPC around one application service and one event envelope. Protocol adapters should differ only in transport and capability projection.

## 16. Adjacent capability decisions

| Subsystem | Source anchors | Decision | InBharat note |
|---|---|---:|---|
| Compaction | `packages/compaction/compaction`, `compaction-basic`, `compaction-tool-result-pruner` | **ADAPT** | Keep log-surface replacement, balanced tool pairing, and overflow retry; initially use deterministic pruning before model summaries. |
| Spill/output retention | `packages/spill/*`, `packages/util/output-retention` | **ADOPT** | Preserve full artifact + bounded preview + locator; add GC. |
| Skills | `packages/skill/*` | **ADAPT** | Keep layered catalog and lazy bodies; add trust/signature policy and treat skill text as untrusted unless explicitly trusted. |
| MCP | `packages/mcp/mcp-client` | **ADAPT** | Keep qualified names and effect-owned reconnect; route auth headers through credential refs and sandbox stdio servers. |
| Terminal/LSP/Web tools | `packages/terminal`, `packages/lsp`, `packages/web` | **ADAPT** | Optional capabilities behind the same tool/sandbox/policy contracts. |
| Plan/Todo | `packages/plan/plan-mode`, `packages/todo/tool-todo` | **SIMPLIFY** | Keep as event-sourced UI/model state plugins, not kernel responsibilities. |
| E2B | `packages/e2b/*` | **DEFER** | Revisit as a remote execution-world provider after local interfaces stabilize. |
| Hook bridges | `packages/hooks/*` | **DEFER** | Useful adapters, but not foundation. |
| Dynamic self-modification | `packages/extensions/*`, especially `tool-cordis` and `cordis-host-runner` | **REJECT** | Model-authored JS/plugins can change Host and browser runtime. Do not ship in a general-purpose trusted product process. |
| Runtime diagnostics/invariants | `packages/runtime-diagnostics/invariants`, package `invariant.ts` companions | **ADOPT** | Preserve executable invariants, but consolidate them into kernel conformance suites. |

## 17. Recommended InBharat target architecture

### 17.1 Kernel plane

Define a small kernel with these explicit types:

```ts
interface Plugin {
  id: string
  requires: ServiceToken<unknown>[]
  start(ctx: PluginContext): void | Promise<void> | Disposable | AsyncDisposable
}

interface PluginContext {
  services: ServiceResolver
  effects: EffectOwner
  events: TypedEventBus
  scope: ScopeId
}
```

Required semantics:

- one owner for every registration/resource,
- reverse, awaited teardown,
- dependency gating and replacement,
- scoped layers and parent chains,
- contained/propagating event failure declared per event,
- no ambient authorization from a context object.

### 17.2 Protocol/data plane

Use a language-neutral event envelope:

```json
{
  "format": 1,
  "sessionId": "...",
  "seq": 42,
  "time": 1730000000000,
  "type": "assistant.message",
  "eventVersion": 1,
  "requiredForReplay": true,
  "data": {},
  "surface": { "op": "append", "sources": [40, 41] }
}
```

Core invariants:

- contiguous single-writer sequence,
- immutable identified messages,
- full request-header snapshot before dispatch,
- raw streaming chunks plus assembled assistant message,
- tool call/result correlation,
- every model-visible input reconstructable from log,
- surface replacement cites every shadowed node,
- migrations for every format/event version.

### 17.3 Model plane

Expose:

- `ModelProviderRegistry`,
- `prepareCall(config) -> PreparedCall`,
- provider/model catalogs,
- `AsyncIterable<ModelChunk>`,
- attachment resolver,
- provider-neutral retry/error taxonomy.

No provider identity, API endpoint, telemetry header, or default model belongs in the kernel.

### 17.4 Tool/policy plane

Use one scoped registry and one pipeline:

`resolve → validate args → pre-policy/approval → monotonic guards → around execution → validate canonical output → post-policy → render → commit`.

Separate:

- execution value,
- model-facing content,
- durable UI metadata,
- error identity,
- additional context,
- turn-conclusion marker.

### 17.5 Security plane

Treat the model, remote clients, MCP servers, imported session text, and plugin packages as separate trust domains.

Minimum server posture:

- authenticated principal for every non-loopback request,
- per-session authorization,
- explicit admin permission to install/load plugins,
- network egress policy,
- sandbox execution world for shell/code/tool servers,
- secret references resolved only inside authorized providers,
- telemetry off by default with mandatory redaction policy,
- no model-authored code in the Host/browser process.

### 17.6 Persistence plane

MVP:

- SQLite session/event store with transactional append, revisions, and indexes,
- optional JSONL export/import and local portable backend,
- blob store for attachments/spills,
- settings and credentials in separate stores,
- crash repair using synthetic unknown-outcome tool results,
- checkpoint before model and top-level tool side effects.

### 17.7 Orchestration plane

MVP order:

1. foreground turns and tools,
2. durable sessions/resume,
3. process-local jobs with opaque IDs,
4. one-shot fresh subagents,
5. balanced-prefix fork,
6. durable job backend,
7. continuable subagents,
8. declarative workflows/goals/schedules.

### 17.8 Transport/UI plane

Implement one application service with explicit RPC schemas. Adapt it to:

- CLI one-shot/headless,
- authenticated HTTP + WebSocket/SSE,
- ACP,
- SDK JSON-RPC.

The UI should consume baselines plus ordered event/projection updates. Keep tool render intent pure and replayable, but compile the renderer registry into the application at first.

## 18. Baseline build and test commands

Sources: root `package.json`, `AGENTS.md`, `docs/testing.md`, and `.github/workflows/ci.yml`.

### 18.1 Prerequisites

```sh
node --version                 # must satisfy ^22.19.0 || >=24.0.0
corepack enable
corepack prepare pnpm@11.7.0 --activate
pnpm --version                 # expected 11.7.0 from packageManager
```

Observed in this audit environment:

```text
node v24.14.1
pnpm 10.34.3
```

Node satisfies the engine; the active pnpm does **not** match the pinned package manager.

### 18.2 Install/build/static checks

```sh
pnpm install --frozen-lockfile
pnpm run clean
pnpm run build
pnpm run typecheck
pnpm run lint
pnpm run duplication
pnpm run hygiene
pnpm run doc-sync
pnpm run website:build
```

Build decomposition:

```sh
pnpm run build:lib:host
pnpm run build:lib:client
pnpm run build:web
```

### 18.3 Test tiers

```sh
pnpm run test                 # Vitest unit tests
pnpm run test:coverage        # CI coverage gate; per-file 100% over most package src
pnpm run test:e2e             # real provider APIs; suites self-skip without keys
pnpm run test:snapshot        # keyless recorded LLM replay and expected outputs
pnpm run test:web             # build + browser snapshot lane
pnpm run test:web:perf
pnpm run test:web:stress
pnpm run test:gui
```

Snapshot maintenance:

```sh
pnpm run test:snapshot:record   # calls real API; updates fixtures
pnpm run test:snapshot:refresh  # replays fixtures; updates expected output
```

Focused test form:

```sh
pnpm exec vitest run path/to/test.spec.ts
pnpm run test:snapshot -- -t '<scenario>'
```

### 18.4 CI aggregate gates

```sh
pnpm run check:ci
pnpm run check:ci:static
pnpm run check:ci:coverage
pnpm run check:ci:snapshot
pnpm run check:ci:artifacts
pnpm run check:ci:consumers
pnpm run check:ci:linux-primary
pnpm run check:ci:windows-complete
pnpm run check:node-compat
```

### 18.5 Product smokes

```sh
pnpm dsh --profile headless "task"
pnpm dsh web
pnpm dsh --profile web --dump-config
pnpm run demo:acp
```

### Audit execution note

No build, test, install, clean, or source-launch command was run during this audit because the user required that the upstream checkout not be modified. `build` emits `lib/` artifacts, `clean` deletes outputs, and several product/test paths create runtime state. Only read-only revision/status/version and source-inspection commands were used.

## 19. Primary blockers and open decisions for InBharat

1. **Kernel choice:** consume upstream Cordis, extract this fork, or implement a smaller kernel. The current harness depends on local Cordis/Loader behavior not available from a stock package.
2. **Compatibility policy:** define event/session format v1 and migrations before production data exists. DeepSeek Harness explicitly has no compatibility promise yet.
3. **Security boundary:** decide local single-user versus authenticated multi-user service. The current Web trust fence is not authentication, and model tools can execute host commands.
4. **Execution backend:** choose container/microVM/remote sandbox for untrusted work. Worker threads and VM contexts are not sufficient.
5. **Plugin trust/distribution:** signed/allowlisted packages versus arbitrary npm/profile plugins. Do not copy `dsh plugin` package-manager forwarding without a supply-chain policy.
6. **Provider neutrality:** remove DeepSeek defaults, product identity, attribution headers, telemetry endpoint, and onboarding assumptions from core/bundles.
7. **Durable orchestration scope:** decide whether jobs and subagents survive process restart in the first release. Current jobs do not; continuable subagents do but require substantial machinery.
8. **RPC source of truth:** explicit protocol schemas versus TypeScript-reflection generation. Multi-language independence strongly favors explicit schemas.
9. **Telemetry/privacy:** define default event allowlist/redaction and consent. The current telemetry seam can export raw session bodies when enabled.
10. **Attachment/spill retention:** define garbage collection across resume/fork lineage; current local attachment objects are indefinite.
11. **UI extensibility:** build-time extension registry versus runtime browser plugins. Runtime plugins should wait for signing, isolation, and compatibility guarantees.
12. **Settings hardening:** fix property-safe handling for special keys and define credential/KMS providers before exposing remote configuration writes.

## 20. Recommended first implementation slice

A defensible first InBharat milestone is:

1. kernel with effect-owned plugins, service tokens, event modes, and scopes;
2. model-neutral messages/chunks and `PreparedModelCall`;
3. immutable session log with turn/step/request/tool events and migrations;
4. SQLite persistence plus repair/checkpoint policy;
5. agent registry, inbox, loop, cancellation, and quiescent handles;
6. scoped tool registry with JSON Schema argument/output validation;
7. approval + capability policy;
8. filesystem/process adapters over one sandbox execution world;
9. one direct OpenAI-compatible provider plus one DeepSeek provider plugin;
10. CLI `run` and authenticated server API;
11. deterministic replay provider and conformance snapshots;
12. simple projection-driven UI.

Only after that foundation passes crash, cancellation, replay, provider-switch, and security conformance should InBharat add durable jobs, forked/continuable subagents, goals, scheduling, workflow scripts, or runtime UI/plugin loading.

---

## 21. Condensed classification matrix

| Major subsystem | Decision |
|---|---:|
| Monorepo/build faces/test tiers | **ADAPT** |
| Cordis lifecycle/DI/effects concepts | **ADOPT** |
| Rescoped vendored Cordis fork | **REJECT** |
| Loader rows/isolation/transactional reload | **SIMPLIFY** |
| Product profiles/bundles | **SIMPLIFY** |
| Per-agent standing presets | **ADAPT** |
| Model vocabulary, adapters registry, prepared calls, streaming | **ADOPT** |
| DeepSeek/pi-ai concrete adapters | **ADAPT** |
| Scoped tool registry and execution pipeline | **ADOPT** |
| Tool schema DSL/validator | **ADAPT** |
| Code Mode worker execution | **DEFER** / not a security boundary |
| Agent registry, handle ownership, inbox, loop | **ADOPT** |
| Session log, surface, request-header reconstruction | **ADOPT** |
| Prompt sections/context/instructions | **ADAPT** |
| Approval audit and fail-closed answerers | **ADOPT** |
| Permission presets | **ADAPT** |
| Sandbox/fs/subprocess/shell provider seams | **ADAPT** |
| Background jobs | **ADAPT** |
| Goals | **DEFER** |
| One-shot subagents | **ADAPT** |
| Continuable subagents | **DEFER** |
| Worker/VM workflow scripts | **REJECT** for untrusted execution |
| Schedules | **DEFER** |
| Crash repair | **ADOPT** |
| Resume/fork/replay | **ADAPT** |
| Session persistence/checkpoints | **ADAPT** |
| Attachments | **ADOPT** |
| Generic storage/domain sidecars | **ADAPT** |
| Settings | **ADAPT** |
| Credentials | **ADOPT** |
| Projections/query | **ADAPT** |
| Telemetry | **ADAPT** |
| Host Web server primitive | **ADOPT** |
| External API trust/auth boundary | **REBUILD** |
| Typert generated RPC | **REBUILD** |
| Browser plugin loader/slot-heavy UI | **REBUILD** / runtime extension **DEFER** |
| CLI | **SIMPLIFY** |
| ACP | **ADAPT** |
| SDK JSON-RPC | **SIMPLIFY** |
| MCP/skills/optional capability providers | **ADAPT** |
| Dynamic model-authored Cordis plugins | **REJECT** |
