# Adopt, adapt, simplify, rebuild, defer, reject

This decision record is derived from the pinned DeepSeek Harness source audit at commit `47f943859bef60e4160492346772ded9b24f765a`. Detailed file and symbol evidence is in `DSH_AUDIT.md`.

## Adopt

- Append-only session facts as the source of replayable model context.
- Explicit turn, step, request, tool, verification, failure, and cancellation lifecycles.
- Provider-neutral model calls with streaming chunks and prepared one-shot dispatch.
- Strict tool inputs, canonical outputs, dynamic visibility, and monotonic authorization.
- Shared execution-world ownership for filesystem and subprocess operations.
- Cancellation propagation, persistence checkpoints, bounded repair, and audited approval decisions.

## Adapt

- Replace Cordis services with small Rust traits and immutable registries owned by the trusted core.
- Replace per-agent plugin trees with explicit capability sets and provider instances.
- Keep provider seams for models, memory, safety, permissions, confirmations, verification, sandboxing, credentials, and execution without allowing those providers to widen core policy.
- Keep jobs and subagents, but require finite budgets, parent relationships, scoped authority, and cancellation.
- Keep event-sourced sessions while using an InBharat-owned, versioned line format and repair rules.

## Simplify

- Four explicit execution levels replace an always-on heavyweight agent path.
- L0 exposes no tools; L1 executes exactly one deterministic action; L2 uses a finite loop; L3 owns goal rounds and workspace budgets.
- One CLI and three Rust crates replace the upstream multi-hundred-package runtime.
- Declarative manifests and typed builders replace layered runtime patch trees.
- A root-confined direct-argv execution broker replaces ambient shell authority.

## Rebuild

- The router, budget engine, event format, permission pipeline, C ABI, session repair, root fence, subprocess broker, and release CLI are independent implementations.
- External authentication, remote RPC, and production OS sandbox integrations must be built around explicit versioned protocols; the upstream web trust boundary is not reused.

## Defer

- Remote production model adapters, authenticated server and UI, encrypted persistence, durable cross-restart jobs, WASM/RPC extension hosts, OS-native keychains, and platform sandbox helpers.
- Continuable subagents, schedules, and rich goal orchestration beyond the bounded interfaces in this release candidate.

## Reject

- A required DeepSeek model, GLM model, Node runtime, or Cordis runtime.
- “Everything is a plugin” inside the trusted computing base.
- Model-authored in-process plugins or worker/VM execution presented as a security boundary.
- Unrestricted shell, filesystem, network, environment, credential, or subprocess access.
- Huge trajectories for ordinary L0 conversation.
