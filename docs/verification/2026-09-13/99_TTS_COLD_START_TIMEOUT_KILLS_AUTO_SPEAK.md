# Defect #24 — Blind-aid auto-speak silently dies: 90s TTS bound rejects mid-cold-start, empty catch swallows it

**Date live-caught:** 2026-09-13 (physical drive D:\UNOONE, deployed build main @ 383d15c — found during defect #23 live re-verification)
**Date fixed:** 2026-09-13
**Severity:** High — a blind user gets the describe TEXT but never hears it, and is never told speech failed. The visible text is invisible to the target user; the spoken lane is the product.

## 1. Symptom (live)

Post-PR-#27 re-test of **Describe Screen** (Screen Reader Description ON):
1. Capture + describe now complete (defect #23 fixed — llama-server log shows the full 368-token prompt eval + 1166-token generation, 147.5s total).
2. The result text renders in the Vision Lab.
3. `isProcessingVision` clears exactly **~90s after describe completion** — and **no `<audio>` element ever mounts**, no speech, no error shown. The empty `catch {}` in `speakText` swallowed the `withVisionTimeout` rejection.

## 2. Root-cause isolation (live drive)

| Probe | Result |
|---|---|
| Timeline of the wedged run | Click 07:48:37 → describe settles ~07:51:08 (147.5s backend, llama-server log) → `running` clears 07:52:37 = **+89s** — the 90s `withVisionTimeout(…, 'Speech synthesis')` bound rejecting mid-synthesis |
| Direct CDP `invoke('synthesize_speech', {text, vaultRoot, language})` with the SAME ~1400-char describe text | **189.5s wall**, success, 136.4s WAV written to `D:\UNOONE\VAULT\recordings\tts_*.wav` — but `processing_time_ms: 8857` |
| The 180s gap | **TTS engine per-call cost**: the describe run was the first TTS call in the relaunched process; the engine spends minutes loading before the actual synthesis (later live probes: 30 chars→21.9s, 84 chars→44.4s — ~15–20s fixed overhead + ~0.3–0.4 s/char, so the cost recurs for every long text, not only on cold start). The follow-on defect #25 (doc 100) isolates the full mechanism: long descriptions also exceed the runtime's hard 180s inference deadline. A second UI run confirmed the 90s bound kills warm calls too — its WAV completed backend-side at 08:05:44, six seconds after the frontend abandoned it |
| UnoOnePower CPU during the hung window | Idle — the engine work is not in-process CPU |

The 90s bound (added as defense-in-depth in PR #27) was tuned for warm-engine latency; the first call after every app launch takes ~190s. On a cold app the bound rejects during model load, every time.

## 3. Fix

| # | Layer | Change |
|---|---|---|
| 1 | `AccessibilityView.tsx` `speakText` | Bound raised 90s → **300s** (matches the describe bound; observed cold-start 189.5s + 9s synthesis fits with margin) with a comment citing the measurement |
| 2 | `AccessibilityView.tsx` `speakText` | The silent `catch {}` now sets a visible `speechNotice`: "Spoken description unavailable: …" — a blind user must be TOLD the speech lane failed; also surfaces `audio_path: null` results ("returned no audio") |
| 3 | `AccessibilityView.tsx` render | `speechNotice` block (role="status") shown next to the vision result |

## 4. Verification

- [x] Frontend builds + lints clean (pre-existing warnings only, none in edited regions)
- [ ] CI green
- [ ] **Live re-verify after re-stage:** cold-launch app → Describe Screen → description spoken end-to-end (audio mounts + playhead advances), first call, within 300s
- [ ] **Live re-verify:** warm second call completes quickly
- [ ] Defect #23 doc 98 §4 live checklist (describe-normal) — the describe half PASSED live 2026-09-13 (result rendered after 147.5s); auto-speak half is this defect's lane

## 5. Note

The TTS engine itself is healthy on the drive: omnivoice synthesized a 136.4s WAV in 8.9s of processing (22 050 Hz) and wrote it inside the vault. The defect was purely the frontend bound + the silent swallow.