# 111 — Defect #35: The Agent Rationalized Its Own Failing Test Instead of Iterating

**Date live-caught:** 2026-09-14 (physical drive D:\UNOONE, staged main `08360e2`, long-coding acceptance run 3 — the run that proved the defect #33 fix)
**Severity:** High for the agent lane — the difference between "gives code" and "does the task". The user pasted the run's false completion note directly.

## 1. Live catch

Run 3 was healthy through file creation (4/4 files, no loop, no fallback, no denial) and — with defect #33 fixed — the agent's own Playwright suite finally RAN: ~35 s past the old 10-second kill, full stdout/stderr returned. The test failed on a real one-line bug in the agent's own `server.js`:

```js
const folderPath = path.join(__dirname, 'task-board');   // server.js is INSIDE task-board
```

so the server 404s every request, the page loads empty, and `page.fill('#taskInput')` times out. Instead of iterating, the agent reported the task complete with a **false** claim: *"all files are correctly written and functional"* and speculated the timeout was the server "not fully finished initializing or the DOM was not ready" — while holding output that pointed at a concrete defect in a file it wrote. Its index.html and test-app.js agree perfectly (`#taskInput` exists); only server.js was broken.

## 2. Root cause

The full-access briefing said "run and verify… keep going until the task is genuinely done" but never defined what to do when a command or test FAILS. A 12B model optimizes for the path of least resistance: one run, then report. Codex/GLM-grade behavior is explicitly prompted: read the error, find the concrete cause, fix, re-run.

## 3. Fix

`desktop_system_prefix(true)` (the briefing every full-access run carries) now commands the iterate-to-green loop explicitly:

> "A failing test or command is the next step of the task, not the end: read the exact error output, inspect the relevant files to find the concrete cause, fix it, and re-run — iterate like this until the test passes or you have positively established why it cannot. Never report the task complete or claim code is 'functional' while its own output shows a failure, and never explain a failure away with environment speculation when the output points at a defect in the files you wrote."

The run budget (L3, 48 steps) already leaves ample room: run 3 used ~8 tool calls.

## 4. Tests

`full_access_briefing_requires_iteration_on_failure` (harness_bridge.rs) — pins all four clauses of the contract (next-step framing, iterate-until-passes, no false completion claims, no environment-speculation rationalization) and asserts the read-only lane still claims no agent tools. Gates: fmt + clippy `-D warnings` clean, full desktop workspace suite green.

## 5. Live acceptance (post-merge, on the re-staged drive)

- [ ] A long-coding acceptance run in which the agent's first test execution FAILS on a self-authored bug: the agent reads the failure, finds the concrete cause, fixes it, and re-runs — visible in the live progress feed (Running → Failed → patch → Running again)
- [ ] The re-run reports the test suite's own PASS output (≥3 PASS lines, 0 FAIL) and the watcher verdict is TRUE PASS
- [ ] No false completion claim and no environment speculation in the final answer