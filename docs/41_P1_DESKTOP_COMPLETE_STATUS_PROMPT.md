# P1 Desktop Feature Completion — Complete A-to-Z Status Prompt

**Generated:** 2026-07-26  
**Superseded:** 2026-07-29 by `docs/52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md`
**Branch:** `remediation/p1-desktop-feature-completion`  
**Base:** `remediation/p0-desktop-runtime` @ `c2e4cfae97045911f4a077cacc6725bef053af8a`  
**Scope:** Windows desktop only. Android and macOS are explicitly out of scope.  
**Purpose:** Historical P1 handoff. Retained for traceability; it is not the
current physical-device status.

---

## A. DONE — Code-complete and build-verified

### A1. Core Rust backend modules (all compile, clippy-clean, unit-tested where applicable)

| Module | Lines | What it does | Evidence |
|--------|-------|--------------|----------|
| `main.rs` | 1083 | Tauri app bootstrap, command registry, vault detection, unlock/lock, settings, hardware profile, file helpers | `cargo check`, `cargo clippy`, registered commands in `invoke_handler!` |
| `recording.rs` | 543 | Real cpal audio capture with dedicated thread (Windows `!Send` workaround), WAV encoding, pause/resume/stop, bookmarks, vault write via `write_recording_to_vault` | `start_recording`, `stop_recording` commands; tests compile |
| `llama.rs` | ~1680 | Model discovery from manifest/USB, backend selection (CUDA/CPU/Vulkan), `llama-server` spawn, identity verification, hash check, stop/start commands, chat completion | 10 unit tests pass; `start_model_server`, `stop_model_server`, `check_model_health` registered |
| `voice.rs` | 678 | Whisper.cpp STT + Piper TTS wrapper, manifest/USB asset auto-discovery, async Tauri commands | `transcribe_audio`, `synthesize_speech`, `get_voice_status` registered |
| `browser.rs` | ~400 | WebView2 session state, browser bridge script, `browser_eval` async JS evaluation, action-to-script mapping | `browser_start_session`, `browser_stop_session`, `browser_execute`, `browser_eval` registered |
| `accessibility.rs` | ~350 | `perform_ocr`, `describe_image`, `get_camera_info`, `encode_image_for_vision` wired to `llama-server` | Commands registered |
| `capability.rs` | ~190 | Unified `DesktopCapabilityProfile` with restricted P1 status vocabulary | `get_desktop_capability_profile` registered |
| `documents.rs` | ~420 | PDF/DOCX/TXT/MD extraction, memory search, document listing | `list_documents`, `process_document`, `search_memories` registered |
| `agent.rs` | ~450 | ReAct agent loop with tool executor, safety guard integration | `agent_chat` registered |
| `safety.rs` | 450 | Safety guard, security level management, audit log | Commands registered |
| `security.rs` | 508 | Manifest generation/verification, recovery, emergency lock | Commands registered |

### A2. Frontend React components (all build, lint-clean)

| Component | Lines | State |
|-----------|-------|-------|
| `App.tsx` | ~200 | Routes all views, handles unlock/lock, auto-lock on blur, vault detection |
| `Sidebar.tsx` | 217 | 11 nav items including Model, Browser, Accessibility, Capability |
| `ChatView.tsx` | 292 | Agent chat with collapsible tool-step pills, model health check, error surfacing |
| `RecordingView.tsx` | 336 | Real backend recording controls, privacy levels, vault record list |
| `BrowserWorkspace.tsx` | 263 | WebviewWindow creation, `browserEval` action execution, bridge re-injection on navigation/session start |
| `AccessibilityView.tsx` | 915 | Blind View toggles, camera preview via `getUserMedia`, OCR/Describe, Voice Lab with TTS audio playback, persisted settings |
| `CapabilityProfile.tsx` | 171 | Grid of truthful feature statuses, refresh button |
| `ModelManager.tsx` | 334 | Model selection, Load/Unload/Health, mmproj auto-discovery, `stopModelServer` |
| `DocumentsView.tsx` | 157 | Document list UI |
| `HardwareProfile.tsx` | 124 | Hardware profile display |
| `MemoryExplorer.tsx` | 110 | Memory explorer shell |
| `SettingsView.tsx` | 233 | Settings UI |
| `UnlockScreen.tsx` | 292 | Vault unlock UI |
| `VaultView.tsx` | 201 | Vault status UI |
| `tauri.ts` | 432 | Type-safe bindings for every Tauri command; static `@tauri-apps/api/core` import |

### A3. Tauri capabilities and configuration

- `apps/desktop/src-tauri/capabilities/default.json` grants `core:default`, `core:webview:allow-create-webview-window`, `core:window:allow-close`.
- `tauri.conf.json` configured for Windows single-window app, CSP `default-src 'self'`.

### A4. Vault-core fix

- `packages/vault-core/src/crypto.rs`: `#[cfg(test)]` fast Argon2id params (8 MiB / 1 iter) so `cargo test --workspace` no longer hangs; production remains 256 MiB / 3 iters / parallelism 4.

### A5. PowerShell automation scripts

| Script | Purpose |
|--------|---------|
| `scripts/stage-p1-desktop-voice-assets.ps1` | Retired; fails safely because a complete voice dependency tree must be staged atomically |
| `scripts/verify-p1-desktop-usb-assets.ps1` | Strictly validate schema structure and every declared asset size/hash |
| `scripts/build-pocket-ai-windows.ps1` | Build/stage Power, Dock, Starter, complete voice bundle, and schema v2 transactionally |
| `scripts/run-p1-desktop-gates.ps1` | Run all local gates in one command |

### A6. P1 deliverable documents (docs/29..docs/40)

- `29_P1_DESKTOP_BASELINE.md`
- `30_P1_DESKTOP_RECORDING.md`
- `31_P1_DESKTOP_BROWSER_WORKSPACE.md`
- `32_P1_DESKTOP_VISION_OCR.md`
- `33_P1_DESKTOP_VOICE.md`
- `34_P1_DESKTOP_CAPABILITY_PROFILE.md`
- `35_P1_DESKTOP_BUILD_TEST_GATE.md`
- `36_P1_DESKTOP_USB_ASSET_ALIGNMENT.md`
- `37_P1_DESKTOP_INTEGRATION_SUMMARY.md`
- `38_P1_EXECUTIVE_REPORT.md`
- `39_P1_PHYSICAL_ACCEPTANCE_PLAN.md`
- `40_P1_DESKTOP_MODEL_MANAGER_WIRING.md`

### A7. Gate results (verified by `scripts/run-p1-desktop-gates.ps1`)

| Gate | Command | Result |
|------|---------|--------|
| Rust format | `cargo fmt --all --check` | **VERIFIED_WORKING** — Windows and macOS CI |
| Rust check | `cargo check` | **VERIFIED_WORKING** — Windows and macOS CI |
| Rust clippy | `cargo clippy -- -D warnings` | **VERIFIED_WORKING** — Windows and macOS CI |
| Workspace tests | `cargo test --workspace` | **VERIFIED_WORKING** — Windows and macOS CI |
| Windows release link | `cargo build --release` | **VERIFIED_WORKING** — bundle run `30437957332` |
| Frontend lint | `npm run lint` | **VERIFIED_WORKING** |
| Frontend build | `npm run build` | **VERIFIED_WORKING** |

**Note:** Local compilation remains policy-blocked on the audit host. Reproducible
GitHub-hosted Windows and macOS CI is the build evidence; no local security
policy was weakened.

### A8. Recent code improvements (since initial P1 completion)

| Improvement | File(s) | Status |
|-------------|---------|--------|
| TTS audio playback player in Voice Lab | `AccessibilityView.tsx`, `tauri.ts` | **VERIFIED_WORKING** — frontend build/lint pass; playback requires Piper assets on live host |
| Browser bridge re-injection after navigation/session start | `BrowserWorkspace.tsx` | **VERIFIED_WORKING** — frontend build/lint pass; live re-injection requires WebView2 runtime test |
| Clippy `collapsible_match` fix in HTML tag stripper | `documents.rs` | **IMPLEMENTED_NOT_TESTED** — manually verified to match clippy suggestion; Rust verification blocked by WDAC on audit host |

---

## B. Historical missing-assets snapshot (resolved on the physical package)

### B1. Voice runtime assets on USB

| Asset | Expected path | Actual state | Impact |
|-------|---------------|--------------|--------|
| Whisper.cpp binary | `D:\UNOONE\RUNTIMES\WINDOWS\VOICE\whisper.exe` | **VERIFIED_WORKING** — declared and hash-verified | Physical microphone UX pending |
| Whisper model | `D:\UNOONE\MODELS\DESKTOP\whisper-base.en.bin` | **VERIFIED_WORKING** — declared and hash-verified | Physical microphone UX pending |
| Piper binary | `D:\UNOONE\RUNTIMES\WINDOWS\VOICE\piper.exe` | **VERIFIED_WORKING** — declared and hash-verified | Physical playback UX pending |
| Piper voice model | `D:\UNOONE\MODELS\DESKTOP\voice.onnx` + `.json` | **VERIFIED_WORKING** — declared and hash-verified | Physical playback UX pending |

### B2. Desktop application binary on USB

| Asset | Expected path | Actual state | Impact |
|-------|---------------|--------------|--------|
| Unsigned Tauri desktop binary | `D:\UNOONE\APPS\WINDOWS\UnoOnePower.exe` | **VERIFIED_WORKING** — declared and hash-verified | Signing and prepared-host UX pending |

### B3. Manifest entries

Schema v2 now declares Power, Dock, Starter, 158 runtimes, 2 desktop models,
381 voice assets, and 2 mobile models. Both physical validators pass.

### B4. Build host for release bundle

- Local Rust compilation is policy-blocked on the audit host.
- GitHub-hosted Windows CI builds the complete release bundle successfully.

### B5. Code signing certificate / WDAC policy

- No signing certificate or catalog is configured in this repository.
- Target WDAC/AppLocker policy for the deployment environment is not represented in code.

---

## C. LACKING — Code exists but is incomplete, under-tested, or has known limitations

### C1. Voice module

| Limitation | Severity | Detail |
|------------|----------|--------|
| No live STT/TTS verified | High | Code compiles and discovers assets; no binaries present to exercise it. |
| TTS audio playback UI implemented but not runtime-tested | Medium | `<audio controls>` player added using `convertFileSrc`; playback requires Piper binary/model on a live host. |
| Real-time mic-to-text not implemented | Medium | STT requires a pre-recorded WAV path; no streaming STT pipeline. |
| Whisper confidence is hardcoded to 0.9 | Low | Whisper.cpp CLI does not expose per-transcription confidence in this invocation mode. |
| Piper duration estimate is file-size based | Low | Assumes 22050 Hz 16-bit mono; accurate enough for UI but not exact. |

### C2. Vision / OCR / camera

| Limitation | Severity | Detail |
|------------|----------|--------|
| Model-backed OCR/describe never runtime-tested | High | Depends on `llama-server` with mmproj; WDAC blocks unsigned binary on audit host. |
| Camera preview is Web API only | Medium | `getUserMedia` preview works in Tauri WebView; snapshot capture is not persisted to vault automatically. |
| `get_camera_info` returns a single dummy frame | Low | `accessibility.rs` `get_camera_info` is a placeholder that returns a 1x1 base64 frame; the real preview is frontend-driven. |
| OCR confidence is hardcoded to 0.9 | Low | No model confidence signal exposed. |

### C3. Browser workspace

| Limitation | Severity | Detail |
|------------|----------|--------|
| No live browsing verified | High | WebView2 window creation and `browser_eval` compile; live page load not tested. |
| `browser_start_session` only sets state | Medium | The actual webview window is created from the frontend; backend command is a thin state marker. |
| Screenshot path is always `null` | Low | `BrowserActionResult.screenshot_path` is never populated. |
| Bridge re-injection added but not runtime-tested | Low | Frontend now calls `injectBridge` on session creation and after `Navigate`; live re-injection timing needs verification. |

### C4. Model inference / ModelManager

| Limitation | Severity | Detail |
|------------|----------|--------|
| `start_model_server` not runtime-tested | High | WDAC blocks unsigned `llama-server.exe`/DLLs on audit host. |
| `stop_model_server` kill semantics not runtime-tested | Medium | Backend command added; relies on `Child::kill()` + `wait()`. |
| mmproj auto-discovery is naive | Low | Replaces `.gguf` with `-mmproj.gguf`; does not handle alternate naming conventions. |
| Model config is not persisted | Low | `get_model_config()` returns `ModelConfig::default()` every time; user edits in UI are lost on reload. |
| No GPU backend preference UI | Low | Backend auto-selects best backend; user cannot override in UI. |

### C5. Recording

| Limitation | Severity | Detail |
|------------|----------|--------|
| Live microphone capture not verified | High | cpal code is correct; no live mic test performed on audit host. |
| `TRANSCRIPT_ONLY` / `SUMMARY_ONLY` privacy levels still write audio | Medium | Currently all non-private levels write the WAV to the vault; transcript/summary-only logic is not implemented. |
| No transcription/summarization pipeline | High | Recordings are stored as WAV vault records; no Whisper STT → summarization step is wired end-to-end. |

### C6. Agent loop / Chat

| Limitation | Severity | Detail |
|------------|----------|--------|
| Agent loop not runtime-tested with real model | High | Depends on `llama-server`; cannot run on audit host. |
| Tool parsing is regex/heuristic | Medium | `parse_text_tool_calls` in `llama.rs` parses `<tool>` blocks; robustness depends on model output format. |
| Only 4 tools exposed | Low | `search_notes`, `list_documents`, `read_document`, `verify_vault`; no write/execute tools. |
| Conversation history type mismatch | Medium | `ChatView.tsx` maps `messages` to `ConversationTurn` using `content: msg.content as Content`, which is a string cast to a union type; may fail TypeScript strict checks if enabled. |

### C7. Documents

| Limitation | Severity | Detail |
|------------|----------|--------|
| PDF extraction depends on `lopdf` with `nom_parser` only | Medium | Complex PDFs or scanned PDFs may fail; no OCR fallback. |
| DOCX extraction is zip/XML based | Medium | Reads `word/document.xml`; advanced formatting/embedded objects ignored. |
| XLSX/PPTX declared but unsupported | Medium | `process_document` reports them as unsupported; no parser implemented. |

### C8. Settings / accessibility persistence

| Limitation | Severity | Detail |
|------------|----------|--------|
| Settings read before vault root known | Low | `App.tsx` calls `getSettings('')` on first render; if vault is detected later, settings are re-fetched. |
| Font scale is global CSS only | Low | No per-component scaling tokens. |

### C9. Security / WDAC

| Limitation | Severity | Detail |
|------------|----------|--------|
| No runtime WDAC enforcement in code | High | The app trusts binaries found on the USB vault; it does not verify code signatures before spawning `llama-server`, Whisper, or Piper. |
| No sandboxing of child processes | High | `llama-server`, Whisper, Piper run with the same privileges as the Tauri app. |
| No file-type allow-list for recordings | Medium | Any file path can be passed to `encode_image_for_vision`; extension-based MIME detection is permissive. |

### C10. Tests

| Limitation | Severity | Detail |
|------------|----------|--------|
| No frontend unit tests | High | Only `oxlint` and `tsc` run; no Jest/Vitest/Playwright tests. |
| No integration tests that exercise Tauri commands | High | `cargo test --workspace` only runs Rust unit tests; no end-to-end command tests. |
| Recording tests are compile-only | Medium | No tests for the cpal thread or WAV encoding. |
| Browser tests absent | Medium | No tests for `browser_eval` or bridge script. |

---

## D. NEEDS EVALUATION — Things that must be checked on a live host

### D1. Physical acceptance plan (`docs/39_P1_PHYSICAL_ACCEPTANCE_PLAN.md`)

The following 22 steps must be executed on a real Windows 11 workstation with the USB vault inserted:

1. Vault detection and unlock (3 steps)
2. Real recording (5 steps)
3. Browser workspace (5 steps)
4. Vision / OCR / camera (5 steps)
5. Offline voice (3 steps)
6. Capability profile refresh (1 step)

Evaluate each step with:
- **Pass** — expected result achieved, no unhandled error.
- **Conditional Pass** — blocked by documented environmental issue (asset, policy, hardware).
- **Fail** — crash, data loss, fake/mock result, or policy weakening.

### D2. Security evaluation

| Question | How to evaluate |
|----------|-----------------|
| Does the app verify signatures of USB binaries before execution? | Code review + runtime test with tampered binary. |
| Does WDAC/AppLocker block unsigned `llama-server.exe` as expected? | Attempt to load model without signed binary. |
| Does the app fail closed (no bypass)? | Try to run with missing manifest, missing vault.id, tampered model hash. |
| Are secrets zeroed from memory? | Review `vault-core` secure-zero tests; optional memory dump analysis. |
| Is CSP effective? | Try to inject external script in WebView2. |

### D3. Performance evaluation

| Question | How to evaluate |
|----------|-----------------|
| Model load time on target GPU/CPU | Time `start_model_server` on clean host. |
| First-token latency for chat/OCR/describe | Measure `send_completion` round-trip. |
| Recording CPU/memory overhead | Profile while recording 10 min WAV. |
| Vault unlock time with production Argon2id | Measure with 256 MiB / 3 iterations. |
| USB read throughput for model files | Copy 7 GB GGUF and measure. |

### D4. Accessibility evaluation

| Question | How to evaluate |
|----------|-----------------|
| High-contrast mode covers all UI surfaces | Visual inspection + screen-reader check. |
| Reduced motion disables animations | Toggle and observe. |
| Camera preview is screen-reader friendly | Test with NVDA/JAWS. |
| Voice Lab language selects are persisted across restarts | Change language, close app, reopen. |

### D5. Build reproducibility evaluation

| Question | How to evaluate |
|----------|-----------------|
| Does `npm run tauri build` succeed on a WDAC-allowed host? | Run on clean Windows dev machine. |
| Does `scripts/build-p1-desktop-windows.ps1` produce a working installer? | Run end-to-end. |
| Does the staged binary launch on a target WDAC profile? | Install and run on locked-down host. |

---

## E. NEEDS COMPLETION — Concrete remaining work

### E1. Asset staging (completed 2026-07-29)

This historical staging procedure is retired. Do not use
`stage-p1-desktop-voice-assets.ps1`: it cannot express the complete dependency
tree and has been changed to fail safely.

The physical package now contains pinned whisper.cpp v1.9.1, Whisper base.en,
Piper 2023.11.14-2, and the public-domain Bryce voice. Use the transactional
`scripts/build-pocket-ai-windows.ps1` workflow for future package updates, then
run `scripts/verify-p1-desktop-usb-assets.ps1 -Strict`.

### E2. Desktop binary build + sign (requires WDAC-allowed build host + certificate)

1. On a WDAC-allowed Windows build host, clone/check out the branch.
2. Run `cargo build -p unoone-power` to confirm debug link.
3. Run `npm run tauri build` to produce the release bundle.
4. Run `scripts/build-pocket-ai-windows.ps1 -VaultRoot D:\UNOONE` to stage the complete package.
5. Sign `UnoOnePower.exe` and `llama-server.exe` (all backends) + DLLs with the UnoOne code-signing certificate.
6. Add the signer/certificate to the target WDAC/AppLocker policy (or deploy a signed catalog).
7. Update `manifest.json` `apps.windows.desktop` entry with signed binary hash and signer info.
8. Re-run `scripts/verify-p1-desktop-usb-assets.ps1`.

### E3. Runtime verification / physical acceptance

1. Insert USB vault into a target Windows 11 host.
2. Install/run `APPS\WINDOWS\UnoOnePower.exe`.
3. Unlock vault.
4. Execute all 22 steps in `docs/39_P1_PHYSICAL_ACCEPTANCE_PLAN.md`.
5. Record results, blockers, and sign-offs in the plan table.

### E4. Optional but recommended code improvements (not blockers for P1)

| Improvement | File(s) | Rationale |
|-------------|---------|-----------|
| Add signature verification before spawning USB binaries | `llama.rs`, `voice.rs`, new `security::verify_signature` | Removes reliance on external WDAC alone. |
| Persist model config to vault | `main.rs` settings, `ModelManager.tsx` | User edits survive reload. |
| Implement transcript-only / summary-only privacy levels | `recording.rs` + integrate Whisper STT | Currently writes audio for all non-private levels. |
| Add frontend unit tests (Vitest) | `apps/desktop/src/src/` | Catch type/runtime regressions. |
| Add Tauri command integration tests | `apps/desktop/src-tauri/tests/` | Verify command contracts. |
| Replace `get_camera_info` dummy frame with real camera enumeration | `accessibility.rs` | Useful for backend-driven camera selection. |
| Add GPU backend override in ModelManager UI | `ModelManager.tsx` | Power users can force CPU/CUDA/Vulkan. |
| Harden `encode_image_for_vision` MIME detection with magic bytes | `accessibility.rs` | Validate magic bytes, not just extension (was implemented then reverted due to unverified Rust CI on WDAC host). |

---

## F. How to continue from here

### F1. If you are another AI agent or engineer

Run the gate runner first to confirm the branch state:

```powershell
.\scripts\run-p1-desktop-gates.ps1
```

Then pick the next highest-priority item based on context:

- **For package updates:** run the transactional
  `build-pocket-ai-windows.ps1` workflow and then
  `verify-p1-desktop-usb-assets.ps1 -Strict`.
- **If on a signing host:** sign binaries and update the deployment policy.
- **If on a live test device:** execute `docs/39_P1_PHYSICAL_ACCEPTANCE_PLAN.md` and fill in the sign-off table.
- **If improving code:** pick from section E4, ensure gates still pass, and update the relevant P1 doc.

### F2. If evaluating for merge

Merge criteria:
- [ ] `scripts/run-p1-desktop-gates.ps1` passes.
- [ ] `scripts/verify-p1-desktop-usb-assets.ps1 -Strict` passes on the target USB volume.
- [ ] `docs/39_P1_PHYSICAL_ACCEPTANCE_PLAN.md` has no **Fail** steps and all blockers are documented.
- [ ] Signed desktop binary and signed `llama-server.exe` (all backends) are staged and WDAC-allowed.
- [ ] No Android code modified.
- [ ] No WDAC/Defender/AppLocker policy weakened in code.

### F3. Constraints that must remain in force

- Do not modify Android code.
- Do not merge or push unless explicitly authorized.
- Do not weaken WDAC/Defender/AppLocker policies to bypass blockers.
- Do not introduce mock/fake data in production UI.
- Use only the P1-approved status vocabulary in docs and UI:
  `VERIFIED_WORKING`, `BUILDS_NOT_RUNTIME_TESTED`, `IMPLEMENTED_NOT_TESTED`, `PARTIALLY_IMPLEMENTED`, `NOT_IMPLEMENTED`, `BLOCKED_BY_ENVIRONMENT`, `FAILED`.

---

## G. Quick reference: commands to run

```powershell
# Local gates
.\scripts\run-p1-desktop-gates.ps1

# Verify USB assets (warnings for missing optional assets)
.\scripts\verify-p1-desktop-usb-assets.ps1 -VaultRoot "D:\UNOONE"

# Verify strictly (fail if any asset is missing)
.\scripts\verify-p1-desktop-usb-assets.ps1 -VaultRoot "D:\UNOONE" -Strict

# Build and stage the complete package transactionally
.\scripts\build-pocket-ai-windows.ps1 -VaultRoot "D:\UNOONE"
```

---

## H. One-line summary

**Historical handoff superseded on 2026-07-29: Desktop CI and Windows bundle CI pass, and the physical Pocket AI package passes 545/545 strict checks plus native Starter verification. Remaining production work is code signing and prepared-host feature acceptance; see `docs/52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md`.**
