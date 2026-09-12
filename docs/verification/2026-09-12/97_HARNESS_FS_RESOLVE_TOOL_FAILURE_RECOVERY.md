# 97 — Defect #22: One Bad Tool Path Killed the Whole Agent Run (Long-Coding Denial)

**Date:** 2026-09-12
**Scope:** `vendor/inbharat-harness/crates/core/src/execution.rs`, `vendor/inbharat-harness/crates/core/src/runtime.rs`, `vendor/inbharat-harness/crates/core/tests/routing_and_tools.rs`, `apps/desktop/src-tauri/src/harness_bridge.rs`, `apps/desktop/src/src/components/ChatView.tsx`
**Posture:** live-caught during the long-coding acceptance run (task #35) on the re-staged drive (bundle `f7a6d3c`).

## 1. Live catch

The long-coding acceptance prompt ("build a task-board web app with real
files") came back as a **denial**: "I don't have direct permission to create
folders, write files to your hard drive, or execute shell commands like
`node` directly" — with a fabricated "PASS" transcript for files that were
never created. The full-access toggle was verified ON
(`checked:true` ×3). The collapsed step pill told the real story:

> 💭 Fell back to the read-only legacy agent (the primary agent pipeline
> could not start: filesystem_denied:fs.resolve: path escapes the
> configured root)

Two independent root causes, both confirmed in code:

1. **`RootedFs::lexical_join` rejected EVERY absolute path.** A 12B model
   told to build something real naturally emits absolute host paths for
   workspace files (`C:\Users\...\UnoOneAgent\task-board\index.html`). On
   Windows, `C:\` parses as `Component::Prefix`, which the fence treated as
   an escape — even though the path designates a file **inside** the
   configured workspace root. The very first `fs.write`/`fs.list` call
   failed with `filesystem_denied:fs.resolve`.
2. **One failed tool call aborted the entire L3 run** (`runtime.rs`:
   `return Err(failure)` in the tool-dispatch error arm). `harness_chat`
   surfaced that as an error, and `ChatView` silently fell back to the
   read-only legacy vault agent — which then truthfully described its own
   (read-only) abilities as the answer. The user sees a confident denial
   from "the AI" while the full-access lane is enabled.

## 2. Fix

### 2a. Absolute paths inside the root are rebased, not denied

`lexical_join` now funnels through `in_root_path`:

- Relative paths pass through unchanged (behavior identical).
- Absolute paths are accepted **only** when they lexically designate
  somewhere inside the configured root; the root prefix is stripped and the
  remainder is re-joined onto the root's canonical form, so every downstream
  fence (component re-scan, `canonicalize` + `ensure_inside`) still applies
  unchanged.
- On Windows, three spellings of the same in-root path are all accepted
  (each caught by a regression test during this fix):
  - the `\\?\` verbatim form `fs::canonicalize` stores as the root, vs. the
    plain `C:\...` volume spelling callers use (and vice versa);
  - **forward slashes** — `C:/Users/.../file.txt` — which models emit
    constantly out of JSON habit;
  - **case variants** — `c:\users\...` for root `C:\Users\...` — with a
    component-boundary guard so `C:\root-sibling` does NOT match root
    `C:\root`.
- Every genuinely-outside path — different drive, sibling directory,
  `..\` after the prefix, `/etc/passwd` — is still `escape_failure`.
- `create_dir_all` was rebased the same way (it walked raw components and
  would still have tripped on `Component::Prefix`).

The fence is unchanged in strength: the model still cannot reach outside the
workspace; it just no longer dies for spelling a workspace file the way a
human would.

### 2b. Per-call tool failures are recoverable at L3

The tool-dispatch error arm in `run_model_loop` now splits failures:

- **Fatal** (run cannot continue): `Cancelled`, `BudgetExceeded`,
  `SessionCorrupt`, `Internal`, `SandboxUnavailable` — appended to the
  session as a `Failure` event and returned, exactly as before.
- **L1 stays fatal**: it is single-action by contract; a failed call leaves
  no output to return.
- **Everything else** (bad path, missing file, denied subprocess, invalid
  arguments, timeout …) is appended as a `ToolResult` error +
  `Verification{passed:false}` and **handed back to the model as the
  tool's result** (`tool=… call=… error=…`), so the next turn can
  self-correct. Each retry consumes a step from the same bounded budget
  (`budget.reserve_step` per loop iteration, 10,000-step ceiling), so a
  flailing model cannot spin forever.

### 2c. Honesty fixes on the desktop side

- The fallback step pill now says "the primary agent pipeline **stopped**:
  …" instead of the false "could not start" (the pipeline had started and
  died mid-run).
- The full-access system briefing now states that tool paths may be
  relative to the workspace folder **or absolute inside it**, both fenced.

## 3. Tests

- `execution.rs` unit tests: absolute in-root path is rebased and works
  across `create_dir_all` / `write_text_atomic` / `read_text` / `list`;
  outside-root absolute read AND write denied with `escapes`; `..` after a
  matching root prefix denied; Windows case-variant accepted; Windows
  prefix-sibling (`<root>-sibling`) denied.
- `routing_and_tools.rs` end-to-end L3 regression
  (`l3_absolute_in_root_write_succeeds_and_escape_fails_one_call_only`):
  a scripted mock model writes via an absolute in-root path (succeeds),
  then an escaping path (fails that one call only, writes nothing outside
  the root), then a corrected relative path — and the run still completes
  with the model's final text, the escape file absent, the corrected file
  present, a `Verification{passed:false}` audit event recorded, and the
  session replay balanced.
- Pre-existing fence tests still green: `lexical_traversal_and_absolute_paths_are_denied`
  (`/etc/passwd` still denied), symlink escape, atomic-write containment.

## 4. Live verification checklist (post re-stage — this re-stage also
carries the defect-#21 asset-protocol fix)

- [ ] Re-run the long-coding acceptance: harness_chat L3 runs to
      completion — no fallback step pill, no denial answer
- [ ] Real files created in the workspace (`task-board\index.html` etc.),
      `node server.js` reachable, port 8199 serving, app viewable in the
      Browser workspace
- [ ] A deliberately malformed path in chat recovers visibly (the model
      corrects itself on the next step instead of the run dying)
- [ ] Out-of-workspace path request still denied per-call without killing
      the run
- [ ] Speech paths from defect #21 live-verified on the same build