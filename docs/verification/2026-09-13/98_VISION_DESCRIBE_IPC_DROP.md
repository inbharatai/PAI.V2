# Defect #23 — Describe Screen: describe invoke silently dropped after sync capture; capture fails on maximized windows

**Date live-caught:** 2026-09-13 (physical drive D:\UNOONE, deployed build from main @ 82242f1)
**Date fixed:** 2026-09-13 (branch `defect-23-vision-describe-robustness`, PR #27)
**Severity:** Critical — the blind-aid "what is on my screen right now" lane is unusable and wedges the UI forever with no error.

## 1. Symptom (live, 4-for-4 reproducible)

Clicking **Describe Screen** in the Accessibility view:
1. The capture PNG is saved to `%TEMP%\unoone-vision\screen-*.png` (capture succeeds).
2. The UI shows "Running vision model…" (`isProcessingVision=true`).
3. The `describe_image` invoke NEVER settles — no error, no vision result, no spoken audio. The button stays disabled for 5+ minutes (observed indefinitely); only remounting the view (navigating away and back) clears the wedge.

A second failure mode appeared on re-test: with the app window **maximized**, the capture itself fails with
`Screen capture failed: No display contains the browser window: Monitor is invalid`.

## 2. Root-cause isolation (all on the live drive, no source changes)

| Probe | Result |
|---|---|
| Direct CDP `invoke('describe_image', {imagePath})` with the wedged click's PNG | **151s, real description** (model read the test terminal text verbatim) — backend + model path fully healthy |
| `detect_inference_backend` / `check_model_health` while wedged | Both answer instantly — manager lock free, server alive |
| llama-server CPU sampling during wedged describe | Idle — no completion was ever processed for the UI's request |
| netstat on the llama-server port | LISTEN only (one short-lived client connection; no sustained inference connection) |
| Invoke spies on `__TAURI__.core.invoke` and `__TAURI_INTERNALS__.invoke` | Zero app calls captured even when the click demonstrably dispatched (bundled API binds invoke at module load) — spy approach discarded as blind |
| GDI `CopyFromScreen` probe + `GetWindowRect` | Session not locked; display present (1280×720); **maximized window rect = (−7,−7)–(1288,680)** → `Screen::from_point(−7,−7)` correctly fails: "Monitor is invalid" |

**Mechanism (mode 1 — the wedge):** `capture_screen_snapshot` was a SYNC Tauri command. Tauri v2 runs non-async commands on the MAIN/UI thread, so the GDI screen capture blocked the UI thread immediately before the adjacent `describe_image` IPC request; that request was then silently dropped (promise never settles — Tauri sends no response for a request the WebView2 IPC channel never delivered). The 3-for-3→4-for-4 capture→describe adjacency, versus direct invokes (main thread idle) always succeeding, is the signature.

**Mechanism (mode 2 — maximized failure):** `screenshots::Screen::from_point` was fed the window's outer top-left. A maximized window reports (−7,−7) (drop-shadow frame extends off-display), which no display contains → hard error. **Describe Screen failed outright for every maximized window** — the most common window state for a real user.

## 3. Fix (PR #27)

| # | Layer | Change |
|---|---|---|
| 1 | `accessibility.rs` | `capture_screen_snapshot` → `async` + `tauri::async_runtime::spawn_blocking` (capture fully off the UI thread; the IPC channel can never be blocked by it) |
| 2 | `accessibility.rs` | `get_camera_info` → `async` + `spawn_blocking` (it previously ran a **PowerShell process synchronously on the main thread** — multi-second full-UI freeze on every Camera info refresh) |
| 3 | `accessibility.rs` | `save_vision_snapshot` → `async` (camera → describe adjacency has the same shape) |
| 4 | `browser.rs` | `capture_probe_point(x, y, w, h)` = window **center** (guaranteed inside a visible window) + primary-display fallback; regression tests use the exact live-caught geometry (−7,−7, 1295×687 on a 1280×720 display) |
| 5 | `llama.rs` | `send_completion`'s reqwest client bounded: 600s total / 10s connect (was unbounded — a hung server wedged every caller forever, incl. agent loops) |
| 6 | `llama.rs` | llama-server stdout/stderr now rotate into `%TEMP%\unoone-logs/llama-server.log` (was `Stdio::null()` — server-side failures were undiagnosable on the drive) |
| 7 | `AccessibilityView.tsx` | `withVisionTimeout` bounds every vision invoke (capture 30s, describe/OCR 300s, synthesize 90s): the UI always settles — a real result or an honest timeout error, buttons re-enable. No more silent forever-wedge even if some future IPC drop recurs |

## 4. Verification

- [x] 128 backend tests green (3 new probe-point regression tests, incl. the exact maximized-window geometry)
- [x] Frontend builds + lints clean
- [x] PR #27 CI green (Rust Backend windows/ubuntu/macos, Frontend Build, all gates)
- [x] **Live re-verify after re-stage** (2026-09-13, main @ 5662b36, 545/545 staged): UI Describe Screen completes → description text + auto-speak (`vision-accept.js describe-normal` PASS — result rendered ~2.5 min, audio mounted and playhead advanced)
- [x] **Live re-verify** (same session): Describe Screen with the window MAXIMIZED — previously "Monitor is invalid", now full result + speech actively playing (`vision-accept.js describe-maximized` PASS)
- [x] **Live re-verify** (same session): OCR on a captured frame through the UI — read the app's own UI text verbatim (`vision-accept.js ocr` PASS)
- [x] **Live re-verify** (2026-09-13, same session): camera snapshot → describe lane (cameraBlindAid + screenReaderDescription) — `camera-accept.js` PASS: live preview, snapshot captured, camera-scene result rendered (~2.1 min), spoken excerpt played. (The first run had in fact already succeeded — 1058-char result + trimmed-notice + 16.44 s speech played to completion — but its watcher used the stale `What the model sees:` regex and false-FAILED; watcher fixed to detect the bare result div + real playhead movement, then re-run clean.)
- [x] Doc 96 blind auto-speak checklist tick — `speech-verify.js` ALL PASS (7/7, exit 0) live 2026-09-13 on the staged build: Voice Lab plays, blind describe speaks automatically, chat Speak plays, asset.localhost refuses out-of-scope paths

## 5. Why the wedge was so hard to pin

Three concurrent red herrings: (a) a `/Describing/i` watcher regex that matched nothing (deployed text is "Running vision model…"), (b) invoke spies that were structurally blind to bundled-app calls, and (c) a stale-audio false positive in the first speech probe. The decisive moves were error-regex completeness (`Screen capture failed:` was never matched), Win32 `GetWindowRect` vs display bounds, and the same-PNG direct-invoke control (151s success against a wedged UI invoke on the identical payload).