# 79 — The Full-Access Lane Was Sandbox-Dead: DesktopSandbox Provider

**Date:** 2026-09-10
**Scope:** `apps/desktop` (harness bridge sandbox wiring)
**Posture:** second live-caught defect during user-perspective pendrive acceptance,
found immediately after the harness_chat model-id fix (doc 78) restored the
request path.

## 1. Symptom (live, through the real app UI)

With harness_chat finally working (doc 78 fix staged and verified live), the
capability question answered truthfully — but the moment the agent was asked to
**use** a full-access tool, the run died:

> "Create a file named live-test-20260910.txt in your workspace containing …"

```text
[console.warning] Harness bridge fell back to legacy agent:
permission_denied:sandbox.resolve: sandbox capability is not granted
```

The read-only legacy agent took over (it cannot write, so the answer was
useless). Every full-access tool — `fs.write`, `workspace.patch`, `process.run`,
`browser.act` — failed the same way: the whole coding/automation/browser lane
has **never executed live**, on any platform, in any build.

## 2. Root cause

The harness tool pipeline resolves a sandbox grant before every tool call
(`tools.rs`: `self.sandbox.resolve(&SandboxRequest { capabilities:
manifest.required_capabilities, require_security_boundary: matches!(side_effect,
Process | Network) })`). The desktop bridge registered a permission provider
(`FullAccessPermission`), a confirmation provider and run capabilities
(`CapabilitySet::all_local()`), but **never a sandbox provider** — so
`HarnessBuilder::embedded`'s default applied:

```rust
// crates/core/src/runtime.rs
sandbox: Arc::new(LocalFenceSandboxProvider::default()),
// granted: [FileRead, Model]  ← everything else fails "not granted"
```

`fs.write` requires `FileWrite`, `workspace.search` requires `Workspace`,
`process.run` requires `ProcessSpawn` — none are in that set, so every call
failed with `permission_denied:sandbox.resolve`. Even with the capability set
fixed, `process.run` and `browser.act` declare `SideEffect::Process`, and an
`InProcessFence`-quality grant is rejected for boundary-requiring effects
("requested effect requires an OS security boundary") — the lane needs a
trusted-process posture, exactly what the harness CLI's `--trusted-process`
flag (`CliSandbox`) exists to express.

CI stayed green for the same reason as doc 78: the desktop tool tests exercise
`tool.execute()` directly, bypassing the sandbox stage, and the harness core's
own tests always construct a sandbox provider that grants the capabilities
under test.

## 3. Fix

`harness_bridge.rs` gains `DesktopSandbox`, a `SandboxProvider` mirroring the
CLI's `CliSandbox` posture:

- `granted` mirrors the run capability set — `all_local()` in full access,
  `[Model, FileRead]` in read-only chat (the capability computation was hoisted
  above the builder so one expression feeds both the provider and `RunOptions`).
- `trusted_process: full_access` — the allowlisted direct-argv
  `LocalExecutionBroker` (audited, budgeted, deadline-killed, no shell) is the
  enforcement boundary, reported honestly as quality `Partial`
  (`unoone-allowlisted-direct-argv`), never silently upgraded.
- Read-only chat keeps the default posture: reads pass behind the in-process
  fence; writes and boundary-requiring effects fail closed.

Unit test `desktop_sandbox_grants_the_full_access_tool_surface` pins the live
matrix: fs.write/fs.read/process.run/browser.act grants in full access, and
read-only denials for writes and boundary effects.

## 4. Post-fix acceptance (must be re-run live)

- The file-creation question executes `fs.write` in the workspace and reports
  the file's contents — no fallback, no `sandbox.resolve` console error.
- `workspace.search`, `workspace.patch`, `process.run` (allowlisted program)
  and `browser.act` all execute through the app with audit + budgets intact.