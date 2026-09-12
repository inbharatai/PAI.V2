# 94 — Defect #19: Blind View Snapshots Were a Dead End

**Date:** 2026-09-12
**Scope:** `apps/desktop/src-tauri/src/accessibility.rs`, `apps/desktop/src-tauri/src/main.rs`, `apps/desktop/src/src/lib/tauri.ts`, `apps/desktop/src/src/components/AccessibilityView.tsx`
**Posture:** live-caught on the re-staged drive (bundle `03cc1b2`), fixed same day.

## 1. Live catch

Live-tested the Blind View (Vision Assist) through the real UI:
camera preview and **Capture Snapshot work** (getUserMedia + canvas;
the button flips to "Camera On", a snapshot lands in the DOM). But
the snapshot is a dead end:

- Captures live only as DOM thumbnails under a label that literally
  says "Captured snapshots (preview only)".
- `describe_image`/`perform_ocr` — the mmproj vision pipeline that is
  the entire point of a *Blind* View — take a **file path the user
  must type by hand**. A blind user cannot read a description text
  box, and typing paths is not an accessibility flow.
- Nothing is spoken: even a successful describe prints text a blind
  user cannot see.

## 2. Fix

- New backend command `save_vision_snapshot(data_url) -> path`
  (`accessibility.rs`): decodes the captured frame's data URL and
  writes it to `%TEMP%\unoone-vision\snapshot-<UTC ts>.<ext>` (host
  temp area — never the read-mostly vault package), with the
  extension derived from the MIME type.
- `captureSnapshot` now saves every capture through that command and
  feeds the returned path into the vision-lab image path, so
  **Run OCR / Describe work on the live camera frame immediately**.
- When the **Screen Reader Description** assist is enabled, capturing
  a snapshot **auto-describes it and speaks the result** through the
  vault speech engine (reusing the voice-lab player, which already
  routes through the defect-#14 `\\?\`-prefix playback fix). A blind
  user points the camera, taps capture, and hears what it sees.
- Describe results are spoken even when triggered manually via the
  button; speech failure is non-fatal (the visible text remains the
  durable output).

## 3. Tests

- `save_vision_snapshot_persists_a_readable_jpeg` — a real data URL
  round-trips to a byte-identical `.jpg` in the temp area.
- `save_vision_snapshot_rejects_non_image_and_garbage` — non-image
  and non-base64 payloads are rejected.
- Full `unoone-power` 120/120 green, frontend build + `tsc` clean,
  clippy clean.

## 4. Live verification checklist (post re-stage)

Verified live on the re-staged drive, 2026-09-12:

- [x] Camera On → Capture Snapshot: file appears in
      `%TEMP%\unoone-vision\`, vision-lab path fills automatically
      (`snapshot-20260912T140421.152.jpg`, auto-filled within 2 s of
      capture)
- [x] With Screen Reader Description on: capture → description text
      appears (full scene description of the live camera frame appeared
      in the Vision Lab; the 12B CPU describe runs longer than the
      120 s poll window of the first retest — the result was present in
      the DOM on inspection). Spoken playback required defect #21's
      asset-protocol fix (see doc 96).
- [ ] Run OCR on the captured frame returns transcribed text
- [x] Snapshots never touch the vault (manifest stays green —
      `verify_manifest` 22/22, 0 failed, after multiple captures)