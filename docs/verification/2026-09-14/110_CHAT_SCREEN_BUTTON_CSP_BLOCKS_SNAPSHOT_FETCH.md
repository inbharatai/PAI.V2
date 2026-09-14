# 110 — Defect #34: The Chat 🖥 Button Died With "Failed to fetch" (CSP Blocked the Snapshot Read-Back)

**Date live-caught:** 2026-09-14 (physical drive D:\UNOONE, staged main `65f9adb` — caught by the user clicking the new chat screen button minutes after PR #34's features landed)
**Severity:** High for the chat-alignment lane — the new 🖥 "what is on my screen" button existed but always failed, a half-measure.

## 1. Live catch

The user pressed the chat panel's new 🖥 (screen → chat) button and got `Screen capture failed: Failed to fetch`. The backend `capture_screen_snapshot` command itself succeeded (the PNG was written to `%TEMP%\unoone-vision\`); the failure was the frontend read-back: `fetch(convertFileSrc(path))` → `http://asset.localhost/...` rejected by the WebView.

## 2. Root cause

The app CSP allowed the asset protocol for **images** (`img-src … asset: http://asset.localhost`) and **audio** (`media-src …`) — that is why TTS playback (defect #21) and `<img>` previews worked — but declared no `connect-src`. Without one, `fetch()` falls back to `default-src 'self'`, and `http://asset.localhost` is not `'self'` → the fetch is blocked and rejects with the generic `TypeError: Failed to fetch`.

The 🖥 button is the first frontend code to *fetch* an asset-protocol URL (the camera 📷 button builds a `data:` URL in-canvas; the accessibility screen-describe path passes the file path to the backend, never through the WebView), so the gap was invisible until this button shipped.

## 3. Fix

`tauri.conf.json` CSP gains an explicit connect policy:

```
connect-src 'self' ipc: http://ipc.localhost asset: http://asset.localhost
```

- `asset:` / `http://asset.localhost` — the snapshot read-back, scoped by the existing `assetProtocol.scope` (only `$TEMP/unoone-tts|vision|browser/**` is served, unchanged).
- `ipc:` / `http://ipc.localhost` — named explicitly (Tauri v2 appends its own IPC entries to a configured CSP; making them explicit keeps the policy legible and safe under future CSP refactors).

## 4. Tests

- Configuration-only change (JSON), validated by the desktop CI build (the bundle step compiles the config) and the live button test below. No other frontend code fetches an asset URL (audited: the only other `convertFileSrc` uses are `<audio>` `src` assignments, already covered by `media-src`).

## 5. Live acceptance (post-merge, on the re-staged drive)

- [x] Chat 🖥 button captures the app window and attaches the screenshot to the pending-image row with the "What is on my screen?" prefill — no error toast
  - **Proven 2026-09-14 (main `8b3644b`, re-staged drive):** click → screenshot attached as a data-URL image within seconds, prefill *"What is on my screen? Describe the content briefly and read out any visible text."*, zero error toasts (the "Failed to fetch" is gone).
- [x] Sending it returns a real description of the app UI through the chat panel (vision lane end-to-end)
  - **Proven same session:** the 1131-char answer described the actual live screen — "a terminal window… running a process related to 'Resuming PALV2'… an agentic workflow where the system is performing…" — i.e. it correctly read the visible Claude Code session on the monitor. Ground truth verified by the human driving the screen.
- [x] TTS playback (🔊 Speak) still works after the CSP change (regression guard for the media-src path)
  - **Proven same session:** the answer's 🔊 Speak produced a "synthesizing…" state and then a live `<audio>` element sourced through the asset protocol (`http://asset.localhost/D%3A%5CUNOONE%5C…tts_…`) — both the media-src path and playback wiring survived the connect-src addition.