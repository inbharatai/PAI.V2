# P1 Desktop Feature Completion — Baseline Trace

## 1. Branch and Starting Point

| Field | Value |
|-------|-------|
| Branch | `remediation/p1-desktop-feature-completion` |
| HEAD SHA | `c2e4cfae97045911f4a077cacc6725bef053af8a` |
| Parent SHA | `634ece78f479683c392a21074dc8e5ce7f2fe49a` (`remediation/p0-desktop-runtime`) |
| Audit date | 2026-07-26 |
| Platform scope | Windows desktop only (macOS explicitly excluded from this branch) |

Parent branch state: P0 remediation complete. Cross-platform vault fixes landed (AES-256-GCM default, HKDF-SHA-256 domain keys, deterministic vectors verified). Desktop Rust build passes `cargo check` and `cargo clippy -- -D warnings`. Frontend passes `npm run build` and `npm run lint` with one accepted unused-var warning (`ModelManager.selectedModelPath`, intentionally wired for future load/start). No mock data remains in the desktop app.

## 2. Repository Module Inventory

### Rust backend (`apps/desktop/src-tauri/src/`)

| Module | Lines | Purpose |
|--------|-------|---------|
| `main.rs` | 1,047 | Tauri bootstrap, vault commands, hardware profiling, settings, command registration |
| `recording.rs` | 391 | Recording state machine, cpal + hound integration, vault write |
| `voice.rs` | 563 | Whisper.cpp STT / Piper TTS availability and invocation |
| `browser.rs` | 356 | Controlled WebView bridge, action translation |
| `accessibility.rs` | 329 | Vision/OCR/camera adapters via llama-server |
| `llama.rs` | 1,720+ | Model manager, llama-server lifecycle, OpenAI-compatible completions |
| `documents.rs` | 1,160+ | Document parsing, memory TF-IDF search |
| `agent.rs` | 540+ | Agent loop / tool orchestration |
| `safety.rs` | 870+ | SafetyGuard with audit log |
| `security.rs` | 1,080+ | Manifest, verification, emergency lock |

### Frontend (`apps/desktop/src/src/`)

| File | Lines | Purpose |
|------|-------|---------|
| `App.tsx` | 172 | View routing, unlock/main lifecycle, auto-lock |
| `components/RecordingView.tsx` | 331 | Recording controls and recent-recording list |
| `components/BrowserWorkspace.tsx` | 50 | Placeholder (Phase 4 target) |
| `components/AccessibilityView.tsx` | 249 | Accessibility settings (Phase 5 target) |
| `components/ChatView.tsx` | 278 | Chat / agent loop UI |
| `components/ModelManager.tsx` | 315 | Model load/start UI |
| `lib/tauri.ts` | 346 | Typed Tauri invoke bindings |

### Build configuration

| File | Notes |
|------|-------|
| `apps/desktop/src-tauri/Cargo.toml` | `cpal 0.15`, `hound 3.5`, `tauri 2`, `tokio 1` |
| `apps/desktop/src-tauri/tauri.conf.json` | CSP: `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'` |
| `apps/desktop/src/package.json` | React 19, Vite 8, `oxlint` lint gate |

## 3. Tauri Command Inventory

Commands relevant to P1 are grouped below with **truthful current status**.

### Recording

| Command | Rust File | Frontend Caller | Current Status |
|---------|-----------|-----------------|----------------|
| `start_recording` | `recording.rs:103` | `RecordingView.tsx:112` | **BUILDS_NOT_RUNTIME_TESTED** — opens default input device via cpal, samples device config, but does **not** keep the cpal stream alive; audio never reaches buffer. |
| `pause_recording` | `recording.rs:180` | `RecordingView.tsx:127/134` | **IMPLEMENTED_NOT_TESTED** — mutates session state only. |
| `resume_recording` | `recording.rs:197` | `RecordingView.tsx:127/134` | **IMPLEMENTED_NOT_TESTED** — mutates session state only. |
| `stop_recording` | `recording.rs:216` | `RecordingView.tsx:88` | **BUILDS_NOT_RUNTIME_TESTED** — WAV encoding and vault write exist, but currently succeeds with empty buffer and returns `Stopped` without a vault record. |
| `add_bookmark` | `recording.rs:293` | `RecordingView.tsx:145` | **IMPLEMENTED_NOT_TESTED** — mutates bookmark vector only. |

### Voice

| Command | Rust File | Frontend Caller | Current Status |
|---------|-----------|-----------------|----------------|
| `get_voice_status` | `voice.rs:529` | `lib/tauri.ts:333` (not yet wired into a view) | **BUILDS_NOT_RUNTIME_TESTED** — checks `RUNTIMES/<OS>/VOICE/` and PATH for `whisper.exe`/`main.exe`/`piper.exe` plus model files. |
| `transcribe_audio` | `voice.rs:545` | `lib/tauri.ts:334` | **BUILDS_NOT_RUNTIME_TESTED** — shell invocation to Whisper exists but `whisper_model_path` is never configured by the frontend, so it will error. |
| `synthesize_speech` | `voice.rs:555` | `lib/tauri.ts:336` | **BUILDS_NOT_RUNTIME_TESTED** — shell invocation to Piper exists but `piper_model_path` is never configured. |

### Browser Workspace

| Command | Rust File | Frontend Caller | Current Status |
|---------|-----------|-----------------|----------------|
| `browser_start_session` | `browser.rs:185` | `lib/tauri.ts` has **no binding** | **IMPLEMENTED_NOT_TESTED** — returns session-initialized message; webview creation must happen on frontend. |
| `browser_stop_session` | `browser.rs:207` | `lib/tauri.ts` has **no binding** | **IMPLEMENTED_NOT_TESTED** — clears session state. |
| `browser_execute` | `browser.rs:217` | `lib/tauri.ts` has **no binding** | **IMPLEMENTED_NOT_TESTED** — translates actions to bridge scripts but does not execute them. |
| `get_browser_bridge_script` | `browser.rs:352` | `lib/tauri.ts` has **no binding** | **IMPLEMENTED_NOT_TESTED** — returns `__unooneBrowserBridge` script. |

### Accessibility / Vision / Camera

| Command | Rust File | Frontend Caller | Current Status |
|---------|-----------|-----------------|----------------|
| `get_accessibility_status` | `accessibility.rs:63` | `AccessibilityView.tsx:14` | **VERIFIED_WORKING** — reads NVDA/JAWS tasklist and registry theme. |
| `perform_ocr` | `accessibility.rs:141` | `lib/tauri.ts:302` | **BUILDS_NOT_RUNTIME_TESTED** — sends image to llama-server; model must be loaded. |
| `describe_image` | `accessibility.rs:214` | `lib/tauri.ts:303` | **BUILDS_NOT_RUNTIME_TESTED** — sends image to llama-server; model must be loaded. |
| `get_camera_info` | `accessibility.rs:282` | `lib/tauri.ts` has **no binding** | **IMPLEMENTED_NOT_TESTED** — returns placeholder `CameraFrame`. |
| `encode_image_for_vision` | `accessibility.rs:306` | `lib/tauri.ts` has **no binding** | **BUILDS_NOT_RUNTIME_TESTED** — returns base64 data URI. |

### Vault / Hardware / Settings (baseline dependencies)

| Command | Status | Notes |
|---------|--------|-------|
| `detect_vault` | **VERIFIED_WORKING** | USB detection on D:\\UNOONE validated in P0. |
| `unlock_vault` | **VERIFIED_WORKING** | Argon2id + AES-256-GCM verified against Android. |
| `get_hardware_profile` | **VERIFIED_WORKING** | WMI/PowerShell queries. |
| `get_voice_status` | already listed |  |

## 4. USB Asset Structure (detected on this host)

Drive: `D:\UNOONE`  
Volume label: `UNOONE`  
Size: 494,163,460,096 bytes (~460 GiB)  
Validation: `manifest.json`, `VERSION`, and `VAULT/identity/vault.id` present.

```
D:\UNOONE
├── APPS\WINDOWS, APPS\MACOS, APPS\desktop
├── RUNTIMES\WINDOWS\CUDA, CPU, VULKAN
│   └── llama-server.exe, llama-server-impl.dll, llama.dll, ggml*.dll
├── MODELS\DESKTOP\Gemma-12B
├── MODELS\MOBILE\brain\gemma-4-e2b, gemma-4-e4b
├── VAULT\{header,records,indexes,journal,transactions,attachments,recovery,identity}
├── CONFIG, RECOVERY, UPDATES, LOGS
└── manifest.json, VERSION
```

**P1-relevant asset gaps identified at baseline:**

| Asset | Expected Path | Found? | Impact |
|-------|---------------|--------|--------|
| Whisper STT binary | `RUNTIMES\WINDOWS\<BACKEND>\VOICE\whisper.exe` | **NO** — `VOICE` subdirectory absent | `get_voice_status` will return `NOT_AVAILABLE`. |
| Piper TTS binary | `RUNTIMES\WINDOWS\<BACKEND>\VOICE\piper.exe` | **NO** | TTS unavailable unless added. |
| Whisper model | `MODELS\DESKTOP\whisper-base.en.bin` (expected) | **NO** in inspected tree | `transcribe` will error with no model path. |
| Piper voice model | `MODELS\DESKTOP\voice.onnx` (expected) | **NO** | `synthesize` will error with no model path. |
| Gemma 12B desktop model | `MODELS\DESKTOP\Gemma-12B\*.gguf` | directory exists (size not checked) | Required for OCR/describe_image; WDAC blocks unsigned DLLs on this host, so runtime load is **BLOCKED_BY_ENVIRONMENT**. |

## 5. Per-Feature Trace Detail

### 5.1 Recording (Phase 2 — highest priority)

Current implementation:

1. `start_recording` (`recording.rs:103`)
   - Creates a `RecordingSession` with UUID, title, timestamp.
   - Uses `cpal::default_host().default_input_device()`.
   - Picks first supported input config via `with_max_sample_rate()`.
   - Updates title with sample rate.
   - **Bug:** the cpal `Stream` is not created or stored. The local `device` and `config` variables are dropped at the end of the `match` arm. The `audio_buffer` remains empty for the entire session.
   - If no microphone exists, session is stored in `Error` state and an error is returned.

2. `pause_recording` / `resume_recording`
   - Only toggles `RecordingState`. No cpal stream pause/resume is implemented.

3. `stop_recording` (`recording.rs:216`)
   - Computes duration from `Instant`.
   - For `PrivateSession`: clears `audio_buffer`.
   - For other privacy levels: locks `audio_buffer`, encodes via `encode_wav` at hard-coded 44.1 kHz.
   - **Bug:** because the buffer is never filled, `audio_buffer.is_empty()` is true and `stop_recording` returns `Stopped` without writing a vault record. Comment at line 261 explicitly states this.
   - When non-empty, `write_recording_to_vault` creates a `RecordType::Recording` and writes via `vault.write_record` (AES-256-GCM after P0 fixes).

4. Frontend `RecordingView.tsx`
   - Calls `tauriApi.startRecording(type, privacy, vaultRoot)`.
   - On success, enters local recording state and starts a client-side timer.
   - On stop success, inserts a synthetic local recording entry using `Date.now()` and `formatTime(elapsed)` instead of the real returned `RecordingSession`. This must be fixed to use the backend result.

**Required P1 work:**
- Build a cpal input stream in `start_recording`, store it in `RecordingStateHolder` alongside the buffer, and push samples to the shared `Vec<f32>`.
- Stop the stream and drop it in `stop_recording`.
- Persist real sample rate and channel count from cpal config instead of hard-coding 44100.
- Update `RecordingView.tsx` to use the returned `RecordingSession` for the recent-recording list and to show `vault_record_id`.
- Use synthetic test data only for unit/integration tests; never fabricate real recording data in the production UI.

### 5.2 Browser Workspace (Phase 3)

Current implementation:

1. `browser.rs`
   - Defines `BrowserConfig`, `BrowserAction`, `BrowserActionResult`, `BrowserSession`.
   - `browser_start_session` stores session active flag.
   - `browser_execute` returns JSON with a JavaScript snippet to run in the webview.
   - `get_browser_bridge_script` returns `__unooneBrowserBridge`.

2. Frontend `BrowserWorkspace.tsx`
   - Entire component is a placeholder. It imports `useState` but the URL state is unused (`_url`, `_setUrl`).
   - Displays "Coming Soon" and references Playwright/Chromium (outdated; product now uses Tauri WebView2).

3. `lib/tauri.ts`
   - Has **no bindings** for `browser_start_session`, `browser_stop_session`, `browser_execute`, or `get_browser_bridge_script`.

**Required P1 work:**
- Add TypeScript types and `tauriApi` bindings for all four browser commands.
- Replace placeholder UI with a real controlled WebView workspace:
  - URL bar with navigate action.
  - WebView window using Tauri `WebviewWindow` / `Webview` APIs.
  - Inject bridge script on new-window-init / on page load.
  - Action toolbar: back/forward, refresh, screenshot, extract text, fill form, scroll.
  - Status bar showing current URL/title and SafetyGuard state.
- Wire `browser_execute` results to actual `webview.eval()` calls.
- Keep Tauri WebView2-only; no Playwright/Chromium download.

### 5.3 Vision, OCR and Camera (Phase 4)

Current implementation:

1. `accessibility.rs`
   - `perform_ocr`: reads image file, base64-encodes it, builds multimodal `InferenceRequest`, sends to `llama-server`. Depends on `model_state`.
   - `describe_image`: similar to OCR with a different system prompt.
   - `get_camera_info`: placeholder returning `width: 0, height: 0`.
   - `encode_image_for_vision`: returns data URI for an arbitrary image path.

2. Frontend `AccessibilityView.tsx`
   - Loads `get_accessibility_status` and applies high-contrast / reduced-motion / font-scale.
   - **All vision toggles are disabled** with descriptions saying "backend ready; desktop UX toggle not yet wired".
   - Camera Blind Aid, Screen Reader Description, and OCR toggles are non-functional.

3. `lib/tauri.ts`
   - Has `getAccessibilityStatus`, `performOcr`, `describeImage`.
   - Missing: `getCameraInfo`, `encodeImageForVision`.

**Required P1 work:**
- Enable the vision toggles and wire them to real commands.
- Add camera UI: device selection, live preview via `getUserMedia` inside Tauri WebView, capture button. Use synthetic frames for automated tests.
- Add image-file OCR/describe flows with file picker and result display.
- Add `getCameraInfo` and `encodeImageForVision` bindings.
- Display truthful status when model is not loaded (`BLOCKED_BY_ENVIRONMENT` on WDAC host).

### 5.4 Offline Voice Runtime (Phase 5)

Current implementation:

1. `voice.rs`
   - `VoiceModule::check_stt_availability` looks for `whisper.exe`/`main.exe` under `RUNTIMES/<OS>/VOICE/` and checks `whisper_model_path`.
   - `VoiceModule::check_tts_availability` looks for `piper.exe` under the same path and checks `piper_model_path`.
   - `transcribe` runs Whisper with `--model`, `--language`, `-otxt`, `-of`, and `audio_path`.
   - `synthesize` runs Piper via stdin, writes WAV to `VAULT/recordings` or temp.

2. Frontend
   - Accessibility view shows STT/TTS language selectors but they are **disabled**.
   - No voice runtime status panel or model-path configuration UI.
   - Recording view mentions Whisper/Piper in an info box but has no controls.

3. `lib/tauri.ts`
   - Bindings for `getVoiceStatus`, `transcribeAudio`, `synthesizeSpeech` exist but are not called.

**Required P1 work:**
- Add a dedicated voice runtime panel or integrate into `AccessibilityView` / `RecordingView`.
- Show `get_voice_status` result truthfully (`NOT_AVAILABLE` until assets are placed).
- Allow the user to point to (or auto-discover) Whisper and Piper binaries + models from the USB `RUNTIMES` and `MODELS` directories.
- Wire "transcribe last recording" and "speak text" actions.
- Keep STT/TTS offline; no cloud services.

### 5.5 Unified Capability Profile (Phase 6)

Current implementation:
- Each feature reports its own status independently.
- No single `DesktopCapabilityProfile` command or UI.

**Required P1 work:**
- Add a Rust `DesktopCapabilityProfile` struct and a `get_desktop_capability_profile` Tauri command.
- Aggregate:
  - Vault: unlocked? disk usage.
  - Recording: microphone present? recording state.
  - Voice: STT available? TTS available? language.
  - Browser: webview available? session active?
  - Vision: model loaded? camera present?
  - Security: current SafetyGuard level.
- Add a frontend capability/status overlay (e.g., in `SettingsView` or as a small status bar).
- Use only the allowed status vocabulary.

## 6. Build and Test Baseline

| Gate | Command | Current Result on this host |
|------|---------|------------------------------|
| Rust format | `cargo fmt --check` (project root) | **VERIFIED_WORKING** |
| Rust check | `cargo check` | **VERIFIED_WORKING** |
| Rust lint | `cargo clippy -- -D warnings` | **VERIFIED_WORKING** (with P0 fixes) |
| Rust tests | `cargo test` | **BUILDS_NOT_RUNTIME_TESTED** — WDAC blocks unsigned native tests that load proc-macro DLLs. |
| Frontend lint | `cd apps/desktop/src && npm run lint` | **VERIFIED_WORKING** (one accepted warning) |
| Frontend build | `cd apps/desktop/src && npm run build` | **VERIFIED_WORKING** |
| Tauri build | `cargo tauri build` | **BUILDS_NOT_RUNTIME_TESTED** — WDAC blocks unsigned `llama-server.exe` and proc-macro DLLs at bundle/link time. |

## 7. Status Vocabulary Reference

Only the following status terms will be used in P1 deliverables:

- `VERIFIED_WORKING` — tested and observed to work on this host.
- `BUILDS_NOT_RUNTIME_TESTED` — compiles/passes lint, but WDAC or missing assets prevent live runtime verification.
- `IMPLEMENTED_NOT_TESTED` — code is present and compiles, no runtime test attempted.
- `PARTIALLY_IMPLEMENTED` — some paths work, others missing.
- `NOT_IMPLEMENTED` — intentionally left out or not yet authored.
- `BLOCKED_BY_ENVIRONMENT` — WDAC, Defender, AppLocker, missing USB asset, or missing microphone/camera prevents verification.
- `FAILED` — build/test/lint fails.

## 8. P1 Work Sequence

Per the authorisation, work proceeds in this order:

1. **Recording** — real cpal streaming + vault write + frontend truthfulness.
2. **Browser Workspace** — replace placeholder with Tauri WebView integration.
3. **Vision / Camera / OCR** — enable toggles, wire backend commands, add camera preview.
4. **Voice** — Whisper/Piper runtime integration and frontend wiring.
5. **Unified capability status** — aggregate profile + truthful UI.
6. **USB asset alignment** — document missing runtime assets and acceptance plan.

No Android code will be modified. No merge or push will occur. macOS will not be addressed. TF-IDF remains the accepted search method. Embedding search is not in scope.
