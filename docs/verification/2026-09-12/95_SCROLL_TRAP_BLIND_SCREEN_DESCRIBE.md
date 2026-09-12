# 95 — Defect #20: Accessibility View Scroll Trap, No Screen Describe, Wedged Camera

**Date:** 2026-09-12
**Scope:** `apps/desktop/src/src/index.css`, `apps/desktop/src/src/components/AccessibilityView.tsx`, `apps/desktop/src-tauri/src/accessibility.rs`, `apps/desktop/src-tauri/src/browser.rs`, `apps/desktop/src-tauri/src/main.rs`, `apps/desktop/src/src/lib/tauri.ts`
**Posture:** live-caught on the re-staged drive (bundle `f08f8b8`), fixed same day.

## 1. Live catch

Live-testing the Blind View through the real UI after the defect-#19 fix
re-stage caught three more failures. The user reported:
"the camera is down and it doesnt detect for a blind and tells whats in
the screen and we cant scrol down to the level".

1. **Scroll trap (hard-confirmed via CDP probe).** `.main-body` has
   `flex:1; overflow-y:auto` but no `min-height:0`. Inside the
   `overflow:hidden` `.main-content`, the column-flex child grows to its
   content height (measured 2373px in a 700px window) instead of being
   constrained — so the scrollbar never engages and everything below the
   fold (Vision Lab results, Voice Lab, Keyboard section) is unreachable.
   Measured: last section bottom at 2405px vs 700px window, with no
   scrollable ancestor anywhere in the chain.
2. **No "what's on my screen" path.** The "Screen Reader Description"
   assist only describes *camera snapshots*. A blind user cannot ask the
   app to describe the app's own screen content — the exact thing the
   feature's label promises ("Describe on-screen content").
3. **Wedged camera with silent no-ops.** `captureSnapshot` returned
   silently when `videoWidth === 0` (no feedback at all — a blind user
   presses Capture and nothing happens), and once the video track ended
   (device claimed by another app, sleep/wake), "Camera On" kept the
   Start button disabled with a dead stream: the camera was permanently
   "down" until the whole app restarted. This is the user's "camera is
   down".

## 2. Fix

- **`index.css`**: `.main-body` gains `min-height:0` so the flex child is
  actually bounded and `overflow-y:auto` engages. One line, benefits every
  tall view (Accessibility, Browser, Documents, Settings…).
- **Screen describe**: new backend command
  `capture_screen_snapshot() -> path` — reuses the browser lane's real
  screen-capture machinery (refactored into shared
  `browser::capture_window_png(app, label)`) against the **main** window,
  writes a PNG into the same host-temp `unoone-vision` directory
  (`screen-*.png`, never the vault), and returns its path. New
  **Describe Screen** button in the Vision Lab: capture → describe →
  speak, identical to the snapshot path. A blind user presses one button
  and hears what the app is showing.
- **Camera hardening** (`AccessibilityView.tsx`):
  - `startCamera` always releases any previous stream and acquires a
    fresh one; the Start button re-enables the moment the live track is
    gone (`hasLiveCamera()`), so a dead camera is always restartable.
  - Track `ended` listeners surface "Camera stream ended — press Start
    Camera to retry" instead of a silent black preview.
  - `captureSnapshot` explains itself when the frame is not ready
    ("Camera is not ready yet…") instead of silently doing nothing.

## 3. Tests

- `write_vision_artifact_persists_screen_png_and_rejects_empty` — screen
  captures land in the host-temp vision dir as `screen-*.png`, stay out
  of the vault, and empty payloads are rejected.
- `png_encoder_accepts_rgba_and_round_trips` /
  `png_encoder_rejects_rgb_sized_payload_for_rgba_declared_image` — the
  capture pipeline's encode declares RGBA and round-trips; an RGB-sized
  payload fails loudly instead of in the user's face.
- `save_vision_snapshot` tests unchanged and green (now routed through
  the shared writer).
- Full `unoone-power` 125/125 green, frontend build + `tsc --noEmit`
  clean, clippy clean.
- No behavior change to the browser lane's Screenshot action
  (`capture_screenshot` now delegates to the shared helper; identical
  output paths and hashing).

## 3a. Second live catch (retest of this very fix)

The first retest of this fix on the re-staged drive caught two more
failures in it — re-tested exactly the way a user would:

1. **The scroll trap survived the one-line fix.** `.main-body` got
   `min-height:0`, but every view's root is a plain unclassed `<div>`
   between `.main-content` and `.main-body` — an unbounded child, so
   `.main-body` still grew to content height (measured: clientHeight ==
   scrollHeight == 1575 with the Keyboard section still below the
   window). Fixed globally: `.main-content > *:last-child` is now a
   bounded column flex (`flex:1; min-height:0`), which makes every
   view's `.main-body` actually scrollable.
2. **Every screenshot encode in the whole capture pipeline was broken**:
   `PNG encode error: wrong data size, expected 1967760 got 7871040`.
   `capture_area` returns RGBA (4 bytes/px) but `png::Encoder` defaults
   to RGB (3 bytes/px). This latent bug predates defect #20 — the
   browser lane's own Screenshot button (shipped earlier and never
   live-verified) fails the same way. `encode_png_rgba` now declares
   `ColorType::Rgba` / `BitDepth::Eight`, shared by both the browser
   Screenshot action and `capture_screen_snapshot`.

## 4. Live verification checklist (post re-stage)

- [ ] Accessibility view at default window size: scroll reaches the
      Keyboard section at the bottom (scrollbar engages)
- [ ] Camera: Stop Camera → Start Camera cycles repeatedly; capture
      with the preview black/not ready shows the "not ready" message
- [ ] Screen Reader Description on → Describe Screen: description of
      the app's own UI appears AND is spoken
- [ ] Screen capture file appears in `%TEMP%\unoone-vision\screen-*.png`
- [ ] Manifest stays green (captures never touch the vault)