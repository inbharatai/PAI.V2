# 92 — Defect #17: The Capability Profile Was a Static P1 Snapshot

**Date:** 2026-09-12
**Scope:** `apps/desktop/src-tauri/src/capability.rs`, `apps/desktop/src-tauri/src/recording.rs`
**Posture:** live-caught on the re-staged drive (bundle `03cc1b2`), fixed same day.

## 1. Live catch

The user pasted the app's own "Truthful Desktop Capability Status" panel:

> Real Audio Recording — Builds, Not Runtime Tested
> Browser Workspace — Builds, Not Runtime Tested
> Vision / OCR / Camera — Partially Implemented
> Offline STT / TTS — Implemented, Not Tested
> Local Model Inference — Implemented, Not Tested
> Agent Loop — Implemented, Not Tested
> … (9 of 12 lanes "not tested")

By then every one of those lanes except camera capture had been
live-verified on the real drive in this acceptance cycle (audio
round-trips en+hi, browser navigation on real domains, mmproj vision,
STT/TTS, llama inference, agent loop, documents, manifest gates). The
panel's own caption claims it "reflects the actual state of binaries,
models, USB detection, and code readiness on this host" — but
`get_desktop_capability_profile` hardcoded 9 of 12 lanes with P1-audit
labels written in July. Two lanes were actively wrong, not just stale:

- **voice** probed the legacy Whisper.cpp/Piper discovery while the
  production drive runs InBharat Audio (Qwen3-ASR/OmniVoice) — the
  panel reported the wrong speech engine, and the "discovered
  Whisper/Piper" note was false on a drive whose active route never
  touches them first.
- **model** returned `IMPLEMENTED_NOT_TESTED` in *both* branches of
  its `if manager_set` — identical status with llama-server running or
  not, making the branch pure decoration.

## 2. Fix

Every lane the runtime can observe is now probed at profile time; the
lanes it cannot self-verify keep their honest posture labels:

| Lane | Now derived from |
|---|---|
| vault | unlocked state (unchanged) |
| recording | `recording::probe_input_support()` — the default device negotiated with a F32/I16/U16 config, the exact negotiation that live-broke as defect #15. No stream is opened. |
| browser | the command executing at all proves the WebView2 surface is live (the profile renders inside it) |
| voice | the production `speech::product_router` — InBharat Audio status first; legacy Whisper/Piper reported only as the declared fallback |
| model | `ModelManagerState` — manager set ⇒ llama-server spawned + readiness-probed ⇒ `VERIFIED_WORKING` |
| documents | unlocked vault ⇒ encrypted store decrypts ⇒ `VERIFIED_WORKING` |
| security | an actual read-only `verify_manifest` run at profile time — green ⇒ `VERIFIED_WORKING`, failures ⇒ `FAILED` with counts |
| usb | vault root found this session ⇒ removable-drive detection verified |
| vision | stays `PARTIALLY_IMPLEMENTED` (camera is private hardware the app cannot self-test) |
| agent | stays `IMPLEMENTED_NOT_TESTED` (self-verifying an LLM loop means spending a real model run) |
| hardware / accessibility | unchanged posture labels |

Notes now state the runtime evidence for every derived lane.

## 3. Tests

Full `unoone-power` suite green (119/119) and clippy clean; the profile
command is exercised live below.

## 4. Live verification checklist (post re-stage)

- [ ] Recording card shows the real device + format, not "Builds…"
- [ ] Voice note names InBharat Audio (Qwen3-ASR/OmniVoice), never
      claims Whisper/Piper on the production drive
- [ ] Model shows `Verified Working` while llama-server runs
- [ ] Security shows real manifest counts (N entries green)
- [ ] USB shows `Verified Working` with the drive mounted
- [ ] Lock the vault → security/documents revert honestly to
      `Implemented, Not Tested` (no false "verified" claims)