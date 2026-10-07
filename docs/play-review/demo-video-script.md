# Play Review Demo Video Script

**Updated 2026-10-07.**

A ~2-minute screen recording showing UnoOne's sensitive surfaces are user-initiated and
transparent. Required for `AccessibilityService` and `specialUse` foreground-service review.

## Setup to show on camera

1. Device: **Xiaomi 14** (`23127PN0CG`, Android 15 / API 35) with models installed. This is the
   only device with recorded device evidence (repo-root `DEVICE_VERIFICATION.md`). No secondary device
   (e.g. Samsung S24 or Pixel 8) has been run yet, so do not present those as verified.
2. Show the Model Status screen. Every installed model shows Status **"Present and
   hash-verified"** with a green check. That includes English STT/TTS and the Gemma 4 brain
   (E2B/E4B), whose SHA-256 and size are pinned in `models_manifest.json`. A manually pushed
   Gemma file that does not match the pinned hash shows **"Present; repair required"** instead.

## Scene 1 — Onboarding & prominent disclosure (0:00–0:20)

- Launch UnoOne. Show the Accessibility prominent disclosure dialog ("…never reads your screen in
  the background…").
  - ⚠ **Prerequisite (status 2026-10-07):** this dialog is not yet implemented. Today the app shows
    only a Toast and opens Settings → Accessibility directly. See
    `accessibility-justification.md` § Prominent disclosure. Build it before recording.
- Tap enable → land in Settings → Accessibility → enable UnoOne → return.

## Scene 2 — User-initiated screen reading + control (0:20–0:50)

- Open a browser. Say/type "read screen" → **STRONG_CONFIRM** dialog ("type 'confirm' to
  proceed") → approve → app speaks the visible text via offline TTS.
- Type "find and click Login" → **STRONG_CONFIRM** dialog ("type 'confirm' to proceed") → approve
  → app scrolls/taps. Narrate: "taps and typing ask first; simple navigation such as back, home
  and scroll runs directly."

## Scene 3 — Foreground-service notifications (0:50–1:15)

- With UnoOne enabled and "Display over other apps" granted, the floating bubble is shown.
  Persistent notification: **"UnoOne active"** / "Floating agent is active — tap to pause / stop".
  There is no separate bubble toggle.
- With the microphone permission granted, the voice service runs. Notification: **"UnoOne
  listening locally"** / "Listening locally — Mic active. Say 'UnoOne' or 'Listen' to give a
  command." Narrate: "no silent background mic."
- Tap **Disable UnoOne** in Settings → both notifications disappear. The notifications themselves
  have no pause/stop buttons.

## Scene 4 — Offline voice + Blind Aid (1:15–1:40)

- Say "UnoOne, create a note: buy milk" → offline STT transcribes → note saved → spoken
  confirmation. Narrate: "fully on-device."
- Activate Blind Aid → camera preview + bounding-box overlay + haptic/tone feedback + spoken
  guidance. Narrate the in-app disclaimer: "assistive only, not a certified navigation device."

## Scene 5 — Privacy controls (1:40–2:00)

- Open Logs → show the audit trail (inputs hashed). Tap Export → JSON file generated.
- Show Settings: **Online tools: OFF**; **Allow Android system TTS fallback: OFF**.
  - ⚠ **Status 2026-10-07:** Privacy Settings has an "Online Web Search (RAG)" switch (default
    OFF), but the `web_search` executor does not read it (`docs/SAFETY.md` §4).
  - No "Allow Android system TTS fallback" setting exists in the app code.
  - Do not show either as a working control until it is implemented.
- "Delete all notes" → STRONG_CONFIRM → confirm → notes gone. Narrate: "your data is local and
  deletable."

## What the reviewer should conclude

- Accessibility is user-initiated, never background monitoring.
- Foreground services run only while UnoOne is enabled (one in-app switch stops them all), each
  with a clear ongoing notification.
- Sensitive actions require confirmation; data is local, hashed, exportable, deletable.
- The only network path (web search) is opt-in and off by default.
  - ⚠ **Not currently true (2026-10-07):** `web_search` is gated per call by CONFIRM but not by an
    off-by-default switch.
  - Model downloads and the Secure Browser also use the network (see `permissions-matrix.md`,
    `INTERNET`).