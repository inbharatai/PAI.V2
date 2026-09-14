# 108 — Blind-Aid/OCR Chat Alignment, Live Agent Progress, and the End of the Silent Read-Only Downgrade

**Date:** 2026-09-14 (user-directed features + defect #32, live-caught the same day)
**Severity:** The silent-downgrade defect was Critical for the agent lane (a build request turned into a pasted tutorial); the other two are the user's core UX asks.

## 1. Live catch (defect #32)

At 13:06, a user-style build request ("…create a simple to-do list web app") was answered with prose — "As an AI assistant, I do not have direct access to your local file system…" — followed by pasted code for all four files instead of built files on disk. The full-access toggle was ON. What actually happened: the harness call threw, the frontend silently fell back to the legacy read-only vault agent (the defect-#16 posture), and that smaller agent *truthfully* described its own narrower abilities. The user watched a build request become a tutorial and reasonably concluded the tool "only gives codes."

The failure mode is worse than a crash: nothing errors, nothing is flagged, and the model's refusal is accurate for the lane it was actually run in.

## 2. Fix

### 2a. No silent downgrade with full access on (`ChatView.handleSend`)

When the harness call fails and `fullAccess` is ON, the legacy fallback is no longer taken. The user gets the honest pipeline error: "Agent pipeline stopped: … The task was NOT run — no files were written and no commands were executed. Retry once the model is back…". The read-only lane keeps the fallback (there the two paths are capability-equivalent) with its loud fallback step banner.

### 2b. Live agent progress (Codex/GLM-style activity feed)

- **Backend (`harness_bridge.rs`)**: every registered desktop tool is wrapped in `ProgressTool`, which emits a Tauri `agent-progress` event per call (`phase: "call"`, tool, human `detail` — e.g. `Writing app/index.html (1024 bytes)` — and for `fs.write` a 240-char `code_preview`) and per result (`Done: …` / `Failed: …`, result truncated at 160 chars). Emission is best-effort: a UI failure can never break a tool run. `progress_detail`/`progress_code_preview` are pure functions.
- **Frontend (`ChatView.tsx`)**: the generating bubble now renders the live feed — spinner + the last 8 events (→ call, ✓ result), fs.write previews shown as code blocks. When the run lands, the whole recorded stream is folded into the message's step pill (with the honest harness route/counts telemetry line) — the activity is preserved, not ephemeral. The stream is mirrored into a ref so the post-`await` fold sees events that arrived during the call (the state closure would be stale — caught before build, not after).

### 2c. OCR + blind aid aligned with the chat panel (single conversation)

- **Bridge**: window CustomEvent `unoone:ask-in-chat` `{text?, imageDataUrl?}` — ChatView attaches the image (deduped, ≤4) and/or prefills the input; `App.tsx` also switches to the chat view so the answer lands in front of the user. ChatView is always mounted (defect #27 architecture), so the bridge works from any tab.
- **Chat camera/screen buttons**: the chat input row gained 📷 (one getUserMedia frame → vision attachment, with a what's-in-front question prefilled) and 🖥 (screen snapshot → same path through the audited attachment pipeline). OCR text and vision results no longer dead-end in the Accessibility panel — "Ask in Chat" hands them to the agent.

### 2d. Phone-parity blind aid (what's in front + speak)

- **`accessibility.rs`**: `describe_image` takes a `mode`; `describe_prompt_for("scene_summary")` is the blind-navigation voice — 2–4 short spoken sentences, objects + positions + visible text, under 60 words, 320-token budget at temperature 0.3 (it must fit the ~280-char spoken excerpt budget; the long detailed prompt stays the default for everything else).
- **`AccessibilityView.tsx`**: a big one-press **"What's in front of me?"** button (starts the camera if needed, captures, describes in scene_summary, speaks); **"Narrate My Surroundings"** — a live loop that captures every ~25 s, describes in scene_summary, and speaks only when the scene changed (word-overlap similarity > 0.8 skipped = same scene, mirroring the phone narrator's throttle), with a 3-consecutive-error backstop that stops the loop with an honest message (defect-#23 posture) and session-only state that resets when the loop stops.

## 3. Tests

- Rust (`harness_bridge.rs`): `progress_detail_phrases_each_tool_concretely` (per-tool human phrasing, generic degradation on missing args, 160-char canonical-JSON fallback for unknown tools), `progress_code_preview_shows_only_bounded_fs_write_heads` (fs.write only, 240-char bound, empty/missing → None).
- Rust (`accessibility.rs`): `describe_prompt_for_scene_summary_is_short_and_spoken_style` (scene_summary spoken-style and ≤320 tokens at low temperature; every other mode keeps the detailed prompt).
- Frontend: oxlint 0 warnings, tsc + vite build green. (The bridge events and loop are exercised live in §5.)
- cargo fmt + clippy `-D warnings` + full test suite green on the desktop crate.

## 4. What this closes

- Defect #32 (silent read-only downgrade produced truthful-sounding refusals; same symptom family as #16)
- User ask: "the chat panel should show what it is doing what codes its writing … like codex, glm" — live per-tool activity feed with code previews, preserved in the step pill
- User ask: "the tool should be able to build tools not just give coes" — failures are now loud; the path that builds is the only path shown
- User ask: "the ocr extraction and the blind aid should all be alligned with the chat panel" — both lanes continue in the chat
- User ask: "the blind aid should recognize whats in front and detect and speak just like the unone phone does" — what's-in-front button + change-detecting live narration loop

## 5. Live acceptance (post-merge, on the re-staged drive)

- [x] A full-access build request visibly streams per-tool activity (Writing/Reading/Running lines + code previews) in the chat while it runs, and the run builds real files
  - **Proven 2026-09-14, run 4 (main `8b3644b`, re-staged drive):** the generating bubble streamed the full trail live — `→ Creating directory task-board ✓ Done: created directory task-board → Writing task-board/index.html ✓ Done: wrote 1137 bytes → … → Running node ✓ Done: status=Some(0) stdout: Server running at http://localhost:8199 PASS: … FAIL: …` — captured by the monitor's 2-minute snapshots through the run, and the run's real files verified on disk (4/4) with the suite passing.
- [x] With the model server down + full access on, a build request surfaces "Agent pipeline stopped: …" — no read-only refusal, no pasted code
  - **Proven 2026-09-14 (main `8b3644b`, re-staged drive):** server stopped via the Model tab, full access ON, build request sent → the chat surfaced verbatim: *"Agent pipeline stopped: Local model has not passed identity verification. The task was NOT run — no files were written and no commands were executed. Retry once the model is back (its state is in the Model Manager, or reload the app)."* Probe verdict: no "As an AI assistant…" refusal, no pasted `<!DOCTYPE html>`, no fallback pill.
- [ ] Blind aid: "What's in front of me?" speaks a short scene summary of a real camera frame
- [ ] "Narrate My Surroundings" speaks the scene, stays quiet when nothing changed, and stops honestly on repeated errors
- [ ] OCR/vision result → "Ask in Chat" lands in the chat panel with the text/image attached and the view switched
- [ ] Chat 📷/🖥 buttons produce a vision attachment and a describable answer through the chat panel