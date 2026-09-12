# 85 — process.run: Empty-Child-Environment Crashes Node at Init (exit 134 CSPRNG assert)

**Date:** 2026-09-12
**Scope:** `vendor/inbharat-harness/crates/core/src/execution.rs` (`run_process` env scrub)
**Posture:** eighth live-caught defect during user-perspective pendrive acceptance.

## 1. Symptom (live, through the real app UI)

Full-access chat: *"Use Playwright from your exec lane: write a script that
opens https://en.wikipedia.org/wiki/India and prints the page title and first
paragraph, then run it."*

The agent did everything right: `fs.write` created `pw-live-test.js`
(`chromium.launch` → `goto` → title + first paragraph), then `process.run`
`node pw-live-test.js` → **exit 134**, stderr:

```
Assertion failed: ncrypto::CSPRNG(nullptr, 0), at node::InitializeOncePerProcessInternal
```

The agent honestly reported the failure. But the tool lane was broken for
every non-trivial Node program — the user's "make it do a long coding, use
playwright, test all coding functions" directive cannot work while any
substantial child crashes at init.

## 2. Root cause

`LocalExecutionBroker::run_process` spawned allowlisted children with
`.env_clear().envs(&spec.environment)` — an **entirely empty environment**
unless the model supplied env vars. On Windows, Node's per-process crypto
initialization resolves through `SystemRoot`; with no environment it asserts
and aborts (exit 134). `node --version` survives only because it takes an
early fast path, which made the lane look healthy in shallow probes.

Empirically isolated (Python `subprocess`, outside the app):

| Child env | `node --version` | `node pw-live-test.js` |
|---|---|---|
| `{}` (empty) | ok | **exit 134 CSPRNG assert** |
| `SystemRoot` only | ok | exit 1 (module resolution — expected) |

## 3. Fix

Replace the bare clear with a **scrubbed system baseline**: `run_process`
still calls `.env_clear()` (nothing is inherited implicitly) but then layers
a machine-level allowlist — `SystemRoot`, `SystemDrive`, `PATH`, `PATHEXT`,
`COMSPEC`, `TEMP`/`TMP`/`TMPDIR`, `OS`, `NUMBER_OF_PROCESSORS`,
`PROCESSOR_ARCHITECTURE`, `HOME`, `LANG` — before `spec.environment`, which
still overrides the baseline. No user-profile, session, or secret-bearing
variables are forwarded (regression test asserts `USERPROFILE` /
`OPENAI_API_KEY` / `DEV_UNLOCK_PW` are dropped).

## 4. Regression tests

- `child_env_baseline_keeps_system_keys_case_insensitively`
- `child_env_baseline_drops_non_system_values`
- `spawned_children_receive_the_system_baseline` (cfg(windows), real spawn):
  `cmd /C echo %SystemRoot% %BASELINE_PROBE%` with `BASELINE_PROBE` supplied
  via `spec.environment` → both expand, proving baseline + layering reach the
  child. Under the old code `%SystemRoot%` would print literally.

## 5. Post-fix acceptance (must be re-run live)

On the staged drive: agent writes a Playwright script → `process.run
node …` exits 0 with real page content from a real domain, then serves the
built app and views it in the browser workspace.