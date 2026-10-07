# Stage 5 mounted Coding Task view tests

All commands run from the repository SOURCE ROOT.

## What this suite is

`tests/coding-task-view.test.mjs` mounts the actual production `CodingTaskView`
(and `Sidebar`) with the installed React 19, ReactDOM, esbuild and
`@tauri-apps/api` bindings. Native IPC is replaced by the official Tauri
`mockIPC` test double (`@tauri-apps/api/mocks`, `shouldMockEvents: true`).
Every command the view sends must be answered by an explicit double in the test;
any other command is recorded in `unexpected` and fails the test at teardown,
together with a check that every Tauri listener was removed at unmount.

A passing run proves mounted frontend behaviour and the exact IPC arguments
the view emits after the camelCase key conversion. It does **not** prove the
Rust glue (`coding_task_commands.rs`), native Tauri IPC or event delivery,
WebView2/WebKitGTK rendering, Windows behaviour, a real vault or a real
sandbox. Layout is not measured (JSDOM has none); the 44 px target rule is
checked in `index.css` and every control is checked to sit inside a scoped
container.

Fixtures are hand-written mirrors of the adapter's serde shapes
(`coding_task.rs` / `task_ledger.rs` owner A, `task_diff.rs` owner B,
`task_preview.rs` owner C) and deliberately differ from the held-out
acceptance fixtures (temperature/duration modules, an `/api/trees` site).
The model narrative fixture is labelled untrusted and must never change a chip.

## Setup

No runtime/repository dependency or lockfile is added. JSDOM lives in a
separate tool root outside the source tree (the Stage 1 one can be reused):

    npm install --prefix ../unoone-stage1-ui-tools --no-save --package-lock=false jsdom@26.1.0
    npm --prefix apps/desktop/src ci

## Run

    CODING_TASK_TEST_TOOL_ROOT="$(cd ../unoone-stage1-ui-tools && pwd)" npm --prefix apps/desktop/src run test:coding-task

(`CHAT_VIEW_TEST_TOOL_ROOT` is accepted as a fallback.) PowerShell:

    $env:CODING_TASK_TEST_TOOL_ROOT = (Resolve-Path ../unoone-stage1-ui-tools).Path
    npm --prefix apps/desktop/src run test:coding-task

Tested environment: Node 24.14.1, React/ReactDOM 19.2.7, esbuild 0.28.1,
Tauri API 2.11.1, JSDOM 26.1.0. Expected result: 25 passed (about 8–9 s; the
preview and interrupted-step tests wait on real 1 s timers).

## Coverage map

| Requirement | Test |
|---|---|
| Mount calls only `coding_task_capability`, `coding_task_list`, `coding_task_view`; listeners cleaned on unmount | `mount reads only…`, `StrictMode mount…`, harness teardown |
| Repo path + branch label shown as "from .git files — not verified" / "branch unknown" | `mount reads only…`, `repository label sources…` |
| Distinct Tool / Build / Tests / Preview / Browser / Goal / Review / Apply chips; narrative never yields success wording | `model narrative "all passed" + real gate exit 2…`, `outcome chip ladder…` |
| Gate results with real exit codes and retained/total bytes | `model narrative…`, `Run checks sends the gate target…` |
| Plan + acceptance criteria, assistant plan needs Confirm | `assistant-proposed plan…` |
| Per-file diff viewer, accept/reject per file with displayed hashes and `view_seq` | `per-file review…` |
| Apply disabled until every file is decided; confirm sends the displayed hashes; apply never called without the click | `per-file review…`, `the apply dialog is bound to its view…` |
| Stale badge resets decisions when content changes | `stale badge resets decisions…` |
| Interrupted steps: explicit options, no automatic resolve/resume | `interrupted steps show explicit options…` |
| Preview: URL, "HTTP-level checks only — not browser-rendered", bounded logs + truncation metadata, 1 s cursor polling only while running, window only on click | `managed preview…`, `preview start is a click…`, `preview window helper retargets…` |
| URL validation (127.0.0.1 capability URLs only) | `preview URL validation…` |
| Unresolved risks from the server; risk hash cross-check | `per-file review…`, `the apply dialog is bound…`, `pure helpers…` |
| Windows `Unsupported` disables run/preview/apply; diff, review, export stay usable | `Windows Unsupported…` |
| Buttons labelled, dialogs modal/labelled, 44 px scoped rule | both `accessibility…` tests |
| Sidebar nav + App route | `navigation…` |
| §8.2 command names; camelCase conversion byte-identical to `src/lib/tauri.ts` | `IPC plumbing…` |
| Exact IPC argument shape of every command (`taskId`… top-level camelCase; `event` / `request` struct parameters with serde snake_case fields, matching owner C's glue) | `typed wrappers: exact argument shape…` (16 commands) + `mount reads only…` (the 3 reads) |
