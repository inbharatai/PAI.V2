# 113 — Defect #38: The Agent Could Build and Test an App, but Never Deploy It

**Date live-caught:** 2026-09-15 (physical drive D:\UNOONE, staged main `2e811ba` — caught by the deploy demo run, `deploy-demo.js`, build+test+deploy task)
**Severity:** High for the agent lane — a world-class coding agent (Codex, GLM) can leave a server running and show you the page; ours killed every deploy at the deadline.

## 1. Live catch

The deploy demo asked the drive's chat panel (full access ON) to build a ping-board web app, test it with its own Playwright suite, **and deploy it**. Items 1–5 passed (doc 112 §5: real workspace root in the label, timestamped feed, absolute paths in the answer, the agent's own test suite run green). Item 6 failed:

```
[05:22:45] Working… 10:45:37→ Running node 10:48:38✓ Failed: timeout:process.run: subprocess deadline
[05:23:49] browser tab shows deployed "Ping Board" app: NO ✗
```

The agent did everything right: it ran `node ping-board/server.js` to deploy — and the harness killed the server at the 180s subprocess deadline, because **a server never exits**. The agent honestly reported the limitation ("I cannot keep a persistent background process running"). The deployed app was unreachable; nothing opened in the Browser tab. This also blocks the user's explicit ask: *"the chat should be able to access browsers and open it too"* — the browser lane exists (`browser.act`), but there was nothing left alive to open.

## 2. Root cause

`process.run` has exactly one mode: foreground, output-piped, bounded by the subprocess deadline. That is correct for a test run (bounded, observable, killed on time). But a **deploy** is the opposite shape: the process must outlive the tool call, its output is irrelevant, and the deadline must never apply. There was no second mode, so every deploy was structurally impossible — no prompt could fix it.

## 3. Fix

A `background: true` argument on `process.run` selects a second, explicitly-different execution mode:

- **`execution.rs` (harness core):** new `DetachedSpawn { pid: Option<u32> }` and `ExecutionBroker::spawn_detached` (default impl = denied, so no other execution world accidentally gains it). `LocalExecutionBroker::spawn_detached` spawns the allowlisted program with **null stdio** — a piped stdio with no reader would block the child, so output is deliberately not captured. The allowlist lookup and args/env bounds checks are extracted into `validated_program()`, shared verbatim with the foreground `run_process` — the background lane cannot be weaker than the foreground lane.
- **`tools.rs`:** `process.run` accepts `program`, `args`, and optional `background` (must be boolean). With `background:true` it calls `spawn_detached` and returns immediately: model content says *started in background (pid N); output is not captured — check the effect itself (e.g. browser.act to the served URL), and stop it later by pid*, value `{background: true, pid: N}`. The manifest description teaches the lane.
- **`harness_bridge.rs` briefing:** the full-access briefing now says *"Deploy long-running processes (servers, watchers): pass background:true to process.run — it returns immediately with the pid and the process keeps running. Its output is NOT captured, so verify the effect itself (browser.act to the served http://localhost:PORT and check the page) and report the pid in your answer so the user can stop it later."*
- **Progress phrasing:** the feed distinguishes the modes — `Starting node in background` vs `Running node`.

## 4. Tests

- `execution.rs` — `spawn_detached_returns_immediately_and_child_outlives_the_call`: spawn is < 5s, pid > 0, the child is verifiably still alive afterwards, allowlist denial still applies, and the test reaps the child.
- `tools.rs` — `process_run_background_returns_pid_immediately`: the tool returns < 5s with a positive pid and the "started in background" model content; child reaped.
- `harness_bridge.rs` — `briefing_requires_absolute_paths_and_timestamped_progress` pins the new `background:true to process.run` and `browser.act to the served` clauses; `progress_detail_phrases_each_tool_concretely` pins `Starting node in background`.
- Harness core: **49/49 green**; desktop: **133/133 green, clippy clean**.

## 5. Live acceptance (post-merge, on the re-staged drive)

- [ ] The agent, asked to build + test + deploy an app, uses `background:true` on the server launch (no `timeout:process.run: subprocess deadline` failure)
- [ ] The server is still alive after the run finishes (the reported pid responds on the served port)
- [ ] The agent opens the deployed app via `browser.act` and the Browser tab shows the running app's page
- [ ] The agent's answer reports the pid and the served URL (per the briefing clause)
- [ ] The feed shows the `Starting node in background` phrasing with its timestamp