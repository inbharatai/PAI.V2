# 105 — Defect #29: No Directory Creation Anywhere in the Tool Set, and fs.write Demanded an Existing Parent

**Date live-caught:** 2026-09-14 (physical drive D:\UNOONE, during the defects-#26/#27/#28 long-coding acceptance re-run on staged main `3a73998`)
**Severity:** Critical for the agent lane — the first step of any "create a folder with files in it" task was impossible; the model looped on the failing fs.write indefinitely.

## 1. Live catch

The re-run started cleanly (task sent, Full access ON, no early fallback pill — defects #26/#27/#28 all fixed and in). But the watcher (hardened: DOM-stability + real-completions quiescence) settled after 4 minutes with 0/4 files on disk and no visible reply, and the llama-server log told the real story:

- model step 1: 297-token prompt eval (the task; system+tools slot-cached), 742-token generation (~96 s)
- steps 2+: **1-token prompt evals** — every subsequent request was (nearly) identical to the previous one, each generating ~742-768 tokens (~96 s), back-to-back, indefinitely
- total context stuck at ~2450 tokens: each iteration appended only ~1-25 tokens — the size of a small tool-error string, not a tool result
- no step pills, no files, no fallback pill, no error banner — the run was alive but making zero progress

## 2. Root cause

| Layer | Finding |
|---|---|
| Tool registry (`vendor/.../core/src/tools.rs`) | the registered fs family was **fs.read / fs.list / fs.write only** — no tool could create a directory |
| `RootedFs::write_text_atomic` (`execution.rs`) | required the parent directory to already exist: `fs::canonicalize(parent)` failed with **"target parent does not exist"** |
| `RootedFs::create_dir_all` | the fenced directory-creation walk **already existed** (per-component canonicalize + `ensure_inside`, added with the defect-#22 absolute-path work) but was reachable by nothing |
| Harness runtime | a failed tool call appends the short error and continues the loop; the local 12B at temperature 0.2 faithfully regenerated a full fs.write (~768 tokens ≈ 96 s) every step instead of discovering a mkdir it did not have |

So the task "create a folder named task-board with 4 files" was **structurally impossible**: every `fs.write` of `task-board/<file>` failed on the missing parent, and no tool could create it. Every real coding agent (Claude Code, Codex, ChatGPT) creates missing directories on write; the 12B was held to a stricter protocol than frontier models.

The diagnosis path itself was instructive: the ~96 s per failed step meant the model WAS writing full files into the tool call — proof the temperature fix (defect #28) worked — and the 1-token prompt evals (slot-cache shows near-identical requests) plus ~20-token context growth (error-string-shaped) identified a tool-continuation loop, not a retry storm and not a parsing failure. A replay probe against llama-server on a free slot confirmed the server emits proper OpenAI `tool_calls` (and even calls `fs.mkdir` first when a prompt mentions parent directories), isolating the failure to the tool layer.

## 3. Fix

- **`fs.mkdir` tool** (`MakeDirTool`, harness core): creates one directory path (parents included) through the already-fenced `RootedFs::create_dir_all` walk. `SideEffect::Write`, `ConfirmationMode::OnSideEffect`, `Capability::FileWrite` (already granted by the full-access sandbox — the fs.write grant covers it). Registered in `register_builtin_tools` and in the desktop `desktop_workspace_tools`.
- **`fs.write` auto-creates missing parents**: `write_text_atomic` now routes a nonexistent parent through the same fenced component walk before writing — so a model that never calls fs.mkdir still succeeds in one call, like every real coding agent.
- **System prompt** (desktop full-access prefix) now tells the model missing parent folders are created automatically, so it stops wasting steps.
- `ExecutionBroker` gained `create_dir_all` (single implementation on `LocalExecutionBroker`; the desktop uses that broker directly).

The fence is unchanged: parent creation goes through `ensure_no_escape` + per-component canonicalize + `ensure_inside`, and `..` components are rejected before anything touches disk.

## 4. Tests

- `write_creates_missing_parent_directories` — a write into a 3-deep never-created parent succeeds and reads back (absolute and relative spellings).
- `write_escape_through_missing_parent_is_denied` — `new/../escape-probe.txt` and `deep/../../escape-probe.txt` fail, and nothing is created on disk.
- `fs_mkdir_and_write_create_nested_paths_end_to_end` — through the full registry dispatch path: fs.mkdir creates `task-board/src`; fs.write into it works; fs.write into a never-created deeper parent auto-creates it.
- `cargo test -p inbharat-harness-core` 41+integration green, adapter 16/16, desktop 128/128, clippy + fmt clean.

## 5. Live acceptance (post-merge, on the re-staged drive)

- [ ] The long-coding acceptance re-run completes: all 4 task-board files on disk, the model reports the test output, watcher verdict PASS
- [ ] Capability panel Agent Loop lane flips to Verified Working on that evidence