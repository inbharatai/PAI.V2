# Defect #25 — Every real screen description is unspokable: TTS text has no length bound and the audio runtime has a hard 180s deadline

**Date live-caught:** 2026-09-13 (physical drive D:\UNOONE, build main @ f02258b, during defect #23/#24 live re-verification)
**Severity:** High — the blind-aid describe lane renders text but speech fails on any realistic description; the surfaced notice hid the cause.

## 1. Symptom (live)

Describe Screen on the new build: describe completes (result renders), the spoken half fails, and the new defect-#24 notice reads "Speech synthesis returned no audio for this description." — which hides the actual cause.

## 2. Root-cause isolation (live, reproduced cleanly)

| Probe | Result |
|---|---|
| Exact UI payload via CDP (the 1847-char lock-screen description, vaultRoot `\\?\D:\UNOONE` from `detect_vault`, language = persisted `tts_language: "hi"`) | **180.3s → `"speech backend failure: local audio runtime exceeded 180 seconds"`** — the audio.cpp runtime kills any invocation past its 180s inference deadline |
| Short probes (30–84 chars, `en`) | 21.9s / 44.4s — engine healthy, ~15–20s fixed overhead + ~0.3–0.4 s/char |
| Same verbatim root, short text | Success — the `\\?\` prefix is fine (an earlier bash double-quote escaping artifact produced an invalid `\?\D:\UNOONE` path and a false "not configured" theory) |
| Arithmetic | 1847 chars × ~0.4 s/char ≈ **13 minutes** of synthesis for one description — 4× over the deadline; no bound on TTS text length exists anywhere in the chain |

The chain has three unbounded/hidden layers: the describe prompt produces 1000–2000 char descriptions; `speakText` passed the whole text through; and the error string from the backend was discarded by the frontend.

## 3. Fix

| # | Layer | Change |
|---|---|---|
| 1 | `AccessibilityView.tsx` | `spokenExcerpt()` — sentence-aligned trim of the spoken text to **280 chars** (~2 min worst-case synthesis, under the 180s deadline with margin). The FIRST sentences of a description are what a blind user needs immediately; the complete text is rendered right above |
| 2 | `AccessibilityView.tsx` | When trimmed, an honest notice: "Spoken the beginning of the description — the complete text is shown above." |
| 3 | `AccessibilityView.tsx` | `audio_path: null` results now surface the backend's `error` string (e.g. "local audio runtime exceeded 180 seconds") instead of the generic "returned no audio" that hid the deadline |

## 4. Verification

- [x] Frontend builds + lints clean
- [ ] CI green
- [ ] **Live re-verify after re-stage:** Describe Screen (language `en`, pinned) → description spoken within ~2 min; audio element mounts and playhead advances
- [ ] **Live re-verify:** trimmed-notice appears for long descriptions

## 5. Future work (recorded, not in scope)

- The engine-side rates (~0.3–0.4 s/char on CPU) are the real ceiling: a fast TTS engine or streamed synthesis (C-ABI `ibaudio_stream_*`) would let the blind lane speak full descriptions.
- The describe prompt could offer a concise mode for the blind auto-speak path (short summary speaks completely).