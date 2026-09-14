# 109 — Defect #33: The 10-Second process.run Deadline Killed the Agent's Own Tool Tests

**Date live-caught:** 2026-09-14 (physical drive D:\UNOONE, staged main `65f9adb`, during the long-coding acceptance run that validated the defect #29/#30/#31 fixes)
**Severity:** Critical for the "build tools, not just give codes" lane — the agent can write a test suite but can never run it.

## 1. Live catch

The acceptance run 2 was healthy end to end: `task-board/` created, `index.html`, `app.js`, `server.js`, `test-app.js` all written in sequence with no rewrite loop and no fallback (defects #29/#30 holding). The agent then ran `node task-board/test-app.js` — the Playwright suite it had just written — and the run died with:

```
→ Running node
✓ Failed: timeout:process.run: subprocess deadline exceeded
```

The agent reported it honestly ("the process timed out during the browser launch phase"), but the acceptance verdict was FAIL with 0 PASS lines: a real Playwright launch + test needs 30–120 s, and the subprocess was killed at exactly 10 seconds.

## 2. Root cause

Two independent constants that were never plumbed together:

- `ProcessSpec::new` (harness core, `execution.rs`) defaults the subprocess deadline to `Duration::from_secs(10)`.
- `ProcessTool::execute` (`tools.rs`) built the spec with only `with_max_output_bytes(self.manifest.max_output_bytes)` — the timeout was **never overridden**, so every process.run ran at the struct default.
- The process.run manifest's `default_timeout` (also 10 s, from the shared manifest helper) was validated non-zero but never enforced anywhere: `ToolDispatch::execute` runs `tool.execute` synchronously with **no second wall-clock cap** — the manifest field was dead configuration, and the spec default was the only deadline in the system.

## 3. Fix

Single source of truth: the tool manifest now owns the deadline, and the spec derives from it.

- **`execution.rs`**: new `ProcessSpec::with_timeout(Duration)` builder (mirroring `with_max_output_bytes`, with the same "tool layer must set this from the manifest" contract). The 10 s struct default remains only so a bare spec cannot hang forever.
- **`tools.rs` `ProcessTool::execute`**: the spec is now built with both `.with_max_output_bytes(...)` and `.with_timeout(self.manifest.default_timeout)` — pipe cap, output check, and kill-timer all enforce manifest values.
- **`tools.rs` `RunProcessTool::default`**: the process.run manifest `default_timeout` is raised from the shared 10 s to **180 s** (sized for a real tool test — browser launch + automation suite — while the run budget still bounds total agent wall-clock; this is the single tool-call deadline only). Cited to defect #33.

## 4. Tests

- `execution::tests::process_spec_timeout_is_overridable_and_enforced` — builder contract (default 10 s, `with_timeout(180)` replaces without mutating the original) plus end-to-end: a child that outlives a 2 s override is killed with the honest `subprocess deadline exceeded` failure (not allowed to run to completion).
- `tools::tests::process_run_deadline_is_derived_from_the_manifest` — process.run manifest carries the 180 s deadline, and a manifest timeout the child outlives (2 s vs a ~30 s process) kills the child at that deadline through `ProcessTool::execute` — proving the spec deadline comes from the manifest, not the struct default.
- Gates: `cargo fmt` clean, `clippy -D warnings` clean on the harness core and the desktop workspace, harness core 58+ green, harness workspace 8 suites green, desktop workspace 23 suites green.

## 5. Live acceptance (post-merge, on the re-staged drive)

- [ ] A long-coding acceptance run in which the agent runs its own Playwright suite (`node task-board/test-app.js`) completes **without** `subprocess deadline exceeded` — the process.run call lasts past the 10-second mark and returns real output
- [ ] The agent reports the test suite's own PASS output in its answer (≥3 PASS lines, 0 FAIL, no fallback, no denial) and the watcher verdict is TRUE PASS
- [ ] A quick process.run probe (e.g. `node -e`) still completes in seconds — the 180 s deadline is a cap, not a floor; short commands are not slowed