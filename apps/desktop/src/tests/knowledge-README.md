# Stage 6 mounted Knowledge / Learning / Memory tests

All commands run from the repository SOURCE ROOT.

## What this suite is

`tests/knowledge-view.test.mjs` (harness `tests/knowledge-harness.mjs`) mounts
the actual production `KnowledgeView`, `CodingTaskView` (with the Stage 6
"Learning" panel), `MemoryExplorer` and `Sidebar`, built from `src/` with the
installed React 19, ReactDOM, esbuild and `@tauri-apps/api`. Native IPC is
replaced by the official Tauri `mockIPC` test double
(`@tauri-apps/api/mocks`, `shouldMockEvents: true`). Every command a view
sends must be answered by an explicit double in the test; any other command is
recorded in `unexpected` and fails the test at teardown, together with a check
that every Tauri listener was removed at unmount.

A passing run proves mounted frontend behaviour and the exact IPC arguments
the views emit after the camelCase key conversion. It does **not** prove the
Rust glue (`apps/desktop/src-tauri/src/knowledge_commands.rs`, whose std-only
policy tests live in `knowledge_commands/glue_policy.rs`), the K1/K2 adapter
services, native Tauri IPC or event delivery, WebView2/WebKitGTK rendering,
Windows behaviour, a real vault, a real sandbox or a real file download.
Layout is not measured (JSDOM has none); the 44 px target rule is checked in
`index.css` and every control is checked to sit inside a scoped container.

Fixtures are hand-written mirrors of the frozen Stage 6 serde shapes
(design §1 K1 `knowledge_service.rs` / `knowledge_distiller.rs`, §2 K2
`task_learning.rs`) plus the Stage 5 `TaskView` fixtures reused from
`coding-task-harness.mjs`. They deliberately differ from the held-out
acceptance material (a weather-units guide: Kelvin offset, minute rounding).
One snippet carries an HTML payload to prove source text is rendered as text.

## Setup

No runtime/repository dependency or lockfile is added. JSDOM lives in a
separate tool root outside the source tree (the Stage 1 one can be reused):

    npm install --prefix ../unoone-stage1-ui-tools --no-save --package-lock=false jsdom@26.1.0
    npm --prefix apps/desktop/src ci

## Run

    CODING_TASK_TEST_TOOL_ROOT="$(cd ../unoone-stage1-ui-tools && pwd)" npm --prefix apps/desktop/src run test:knowledge

(`KNOWLEDGE_TEST_TOOL_ROOT` or `CHAT_VIEW_TEST_TOOL_ROOT` are accepted too.)
PowerShell:

    $env:CODING_TASK_TEST_TOOL_ROOT = (Resolve-Path ../unoone-stage1-ui-tools).Path
    npm --prefix apps/desktop/src run test:knowledge

Tested environment: Node 24.14.1, React/ReactDOM 19.2.7, esbuild 0.28.1,
Tauri API 2.11.1, JSDOM 26.1.0. Expected result: 24 passed (about 13 s; the
MemoryExplorer debounce tests wait on real timers).

The glue policy tests (no Tauri, no adapter) run standalone:

    rustc --edition 2021 --test apps/desktop/src-tauri/src/knowledge_commands/glue_policy.rs -o /tmp/kn_glue && /tmp/kn_glue

## Coverage map

| Requirement (design §3.2 / §4 U) | Test |
|---|---|
| Mount reads only `knowledge_status` (+ first `knowledge_list` page when initialized); no state-changing IPC without a click; listeners cleaned | `mount reads only…`, `uninitialized store…`, `StrictMode mount…`, harness teardown |
| Method label always visible (status header, Distiller, run report) | `mount reads only…`, `Distiller…` |
| Historical (audit-only) labels; current mode only with a complete exact file identity; exact `KnowledgeQuery` | `Explorer search…` |
| Kind filters Evidence/Candidate/Verified/Approved/Invalidated + list state → `KnowledgeListFilter` | `Explorer list…` |
| Badges: source / version / licence / privacy / platform / contradictory / invalidated; source text untrusted | `mount reads only…` |
| Detail: body, citations, history, incoming/outgoing edges, verification summary with real exit codes, server `allowed_actions` only | `Detail: body…`, `Detail of an approved procedure…` |
| Reject / Revoke behind confirm dialogs that send the displayed refs | `Detail: body…`, `Detail of an approved procedure…` |
| Distiller: pasted text + local file under a granted root, budget bounds, Preview (sources + hash), Run confirm sends the shown hash, exclusions with reasons, citations, contradictions labelled "heuristic", explicit index rebuild | `Distiller…` |
| Export: training export disabled, only verified/approved offered, refusals shown, private-content acknowledgement, consent confirm with the previewed hash, download of the returned JSON | `Export: training export…`, `Export without private content…` |
| Learning panel: no auto-calls; Save sends view_seq + change_set_sha256; recipe preview (cases, argv, expected exit) + hash → Verify confirm; real check exit codes; Approve sends displayed run/policy hashes; Revoke | `Learning panel: no learning IPC…` |
| Save disabled until build+tests passed and every file accepted; stale index → no patterns | `Learning panel: Save as candidate…` |
| Windows Unsupported disables Verify/Approve with the server's reason | `Windows Unsupported…` |
| Errors shown, never swallowed | `Learning errors…`, both `errors are shown…` |
| MemoryExplorer search issues `search_memories` with the typed (debounced, trimmed) query; stale answers discarded | both `MemoryExplorer…` |
| Sidebar "Knowledge" + App route | `navigation…` |
| §3.1 command names; camelCase helper byte-identical to `tauri.ts`; exact arg shape of all 19 commands | `IPC plumbing…`, `typed wrappers…` |
| Accessible names, labelled modal dialogs, appended scoped 44 px CSS | `accessibility…` |
