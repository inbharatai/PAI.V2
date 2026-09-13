# 96 — Defect #21: App-Wide Audio Playback Was Dead (Asset Protocol Never Enabled)

**Date:** 2026-09-12
**Scope:** `apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src/src/components/AccessibilityView.tsx`
**Posture:** live-caught on the re-staged drive (bundle `f7a6d3c`) while verifying the defect-#20 speech output, fixed same day.

## 1. Live catch

The defect-#20 "Describe Screen" flow produced a real TTS WAV
(`%TEMP%\unoone-tts\inbharat_tts_*.wav`, 383 KB, synthesized by the vault
speech engine), set it on the audio element — and the audio never played.
Probing the live element:

```
play() → NotSupportedError: The element has no supported sources
readyState 0, mediaError.code 4 (SRC_NOT_SUPPORTED)
```

Root cause: `convertFileSrc()` generates `http://asset.localhost/...`
URLs, but the **Tauri asset protocol was never enabled** — no
`protocol-asset` cargo feature, no `assetProtocol` block in
`tauri.conf.json`. Every `asset.localhost` request in the app returns
nothing, so *every* audio consumer was silently broken:

- the Blind View's spoken descriptions (the entire point of a
  Screen Reader Description toggle for a blind user),
- the Voice Lab's TTS player,
- chat's "🔊 Speak" reply playback (the player renders but has no
  loadable source).

A second, independent blocker: WebView2's autoplay policy. The blind-aid
flow calls `audio.play()` only after a capture → describe (minutes of CPU
inference) → synthesize chain — by then the click's user-gesture context
has expired, and even a loadable source would not auto-play.

## 2. Fix

- **Enable the asset protocol** (`Cargo.toml`: `protocol-asset` feature;
  `tauri.conf.json`: `assetProtocol.enable` with scope limited to the
  app's own temp output dirs — `$TEMP/unoone-tts/**`,
  `$TEMP/unoone-vision/**`, `$TEMP/unoone-browser/**` — never the vault
  or arbitrary host paths).
- **CSP**: `media-src`/`img-src` now allow `asset: http://asset.localhost blob:`.
- **Autoplay**: main window created with
  `--autoplay-policy=no-user-gesture-required` so spoken descriptions and
  spoken chat replies start without a fresh gesture. The user explicitly
  toggles speech features on, so autoplay is the expected behavior here.
- Manual `<audio controls>` fallbacks remain for every consumer.

## 3. Tests

- `unoone-power` 125/125 green, clippy clean, frontend `tsc` + build
  clean. The config itself is validated by `tauri-build` at compile
  time (this is what caught the `additionalBrowserArgs` field name and
  the missing cargo feature during the fix).

## 4. Live verification checklist (post re-stage)

All four lanes verified live 2026-09-13 on the physical drive D:\UNOONE (staged build main @ 5662b36, 545/545; `speech-verify.js` ALL PASS, exit 0):

- [x] Voice Lab: Synthesize Speech → audio element loads and plays (readyState=4, playhead advances)
- [x] Blind View with Screen Reader Description on: capture snapshot → description is spoken automatically (no manual play; audio advanced on its own — this also closes doc 98 §4's "Doc 96 blind auto-speak tick")
- [x] Chat: reply "🔊 Speak" button produces a playable audio element (readyState=4, plays)
- [x] `asset.localhost` still cannot serve paths outside the three scoped temp dirs (win.ini and a vault path both refused)