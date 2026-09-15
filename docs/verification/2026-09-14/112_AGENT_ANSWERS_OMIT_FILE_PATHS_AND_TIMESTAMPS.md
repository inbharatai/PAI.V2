# 112 — Defects #36 + #37: The User Could Not Find the Agent's Files, Nor Tell When Each Step Happened

**Date live-caught:** 2026-09-14 (physical drive D:\UNOONE, staged main `8b3644b` — caught by the user running their own build task through the drive UI)
**Severity:** High for the chat-alignment lane — a world-class agent tool (Codex, GLM) always shows *where* files landed and *when* each action ran; ours said "done" and went silent on both.

## 1. Live catch

The user asked the drive's chat panel (full access ON) to build a small to-do web app. The agent built the three real files and said "done". The user then asked: *"it says done but whre exacly the tool is and why cant we see when it was built like we can in other ai"*. Two concrete gaps:

- **#36 (where):** the full-access label showed the literal, unexpanded pattern `%USERPROFILE%\UnoOneAgent`, and the agent's answers said only "in your workspace" with bare filenames — the user had no path to open.
- **#37 (when):** the live activity feed and the folded step pill showed *what* the agent did but no timestamps — the run's history was invisible, unlike Codex/GLM's timestamped transcripts.

## 2. Root cause

- **#36:** `ChatView.tsx` hardcoded the label string `(workspace: %USERPROFILE%\UnoOneAgent · …)` and nothing ever asked the backend for the real path. The agent briefing (`desktop_system_prefix(true)`) never required answers to state file locations, so the model legitimately answered "in your workspace" — a 12B model will not invent the convention unprompted.
- **#37:** the `agent-progress` event payload carried only `{phase, tool, detail, code_preview}` — no clock anywhere — and neither the live feed nor the folded `AgentStep`s rendered a time.

## 3. Fix

- **#36 backend:** `desktop_system_prefix(true)` now ends with: *"When a task creates or changes files, end your answer by listing the exact ABSOLUTE path of every file you created or changed (the workspace root is {workspace}), so the user can find and open them — never say only 'in your workspace' or a bare filename."* New Tauri command `get_workspace_root` returns the real expanded root (registered in `main.rs`, wrapped in `tauri.ts`).
- **#36 frontend:** the full-access label renders `{workspaceRoot}` from `tauriApi.getWorkspaceRoot()`, falling back to the literal pattern if the invoke fails.
- **#37 backend:** `AgentProgressEvent` gains `at: String` stamped `chrono::Local %H:%M:%S` at all three emit sites (call, Ok result, Err result) via `progress_timestamp()`.
- **#37 frontend:** the live feed prefixes each line with the monospace `HH:MM:SS` stamp; folded `AgentStep`s carry `at` and the expanded step pill shows it beside the tool name.

## 4. Tests

`briefing_requires_absolute_paths_and_timestamped_progress` (harness_bridge.rs) pins: the absolute-paths clause and the no-vague-locations clause in the full-access briefing; `workspace_root()` resolves an absolute path; `progress_timestamp()` is exactly `HH:MM:SS` with valid ranges. The pre-existing `full_access_briefing_requires_iteration_on_failure` still pins defect #35's clauses.

## 5. Live acceptance (post-merge, on the re-staged drive)

Verified 2026-09-15 on the re-staged drive (main `2e811ba`) by the deploy-demo run (`C:\Users\reetu\UnoOneAgent\pai-live-test\app-ui-test\deploy-demo.js`, a build+test+deploy task through the chat panel):

- [x] The full-access label shows the real expanded workspace root (e.g. `C:\Users\reetu\UnoOneAgent`), not the literal `%USERPROFILE%` pattern — label read back as `C:\Users\reetu\UnoOneAgent`
- [x] The live activity feed shows a local `HH:MM:SS` timestamp on every tool line while the agent runs — stamps appeared from the first tool call onward (10:43:34 → …)
- [x] The agent's final answer to a build task lists the exact absolute path of every file it created (not "in your workspace") — all 4 files listed, e.g. `C:\Users\reetu\UnoOneAgent\ping-board\index.html`
- [x] After the run lands, the folded step pill still shows the timestamps on each tool call/result — 20 timestamps in the folded pill

The same run exposed **defect #38** (the deploy itself was killed at the 180s subprocess deadline — see doc 113, `2026-09-15/113_PROCESS_RUN_BACKGROUND_DEPLOY.md`).