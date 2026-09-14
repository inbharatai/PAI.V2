# 102 — Final Live Acceptance: Physical Pocket AI Drive (D:\UNOONE)

**Date:** 2026-09-13 → 2026-09-14 (acceptance window 2026-09-08 → 2026-09-14)
**Drive build:** main `3a73998` (PRs #1–#31 merged: #30 = agent budget round-division, #31 = defects #27+#28), staged 2026-09-14, 545/545 USB asset checks green
**Method:** live user-perspective testing through the real app (CDP into the shipped WebView, real mic, real camera, real disk artifacts), every defect fixed end-to-end (branch → CI → merge → re-stage → live re-test)

## 1. Defect ledger (live-caught, all fixed + re-verified unless noted)

| # | Doc | Defect | Status |
|---|---|---|---|
| 8–16 | (2026-09-12 cycle) | baseline, budgets, capability profile, browser UX, vision lab, scroll trap, speech assets | FIXED, live-verified |
| 17 | 92 | capability profile stale after model load | FIXED (PR #20), live-verified |
| 18–19 | 93–94 | browser session UX, vision lab snapshot pipeline | FIXED, live-verified |
| 20 | 95 | scroll trap + blind-view describe + camera feedback | FIXED, live-verified |
| 21 | 96 | speech playback asset protocol | FIXED, live-verified (speech 7/7) |
| 22 | 97 | harness_chat dies on fs.resolve absolute-path escape | FIXED; long-coding re-run = doc 101 §4 |
| 23 | 98 | vision describe IPC drop (UI stuck "Describing") | FIXED, live-verified (describe normal + maximized) |
| 24 | 99 | TTS cold-start timeout kills auto-speak | FIXED, live-verified |
| 25 | 100 | TTS 180s runtime deadline exceeds on descriptions | FIXED, live-verified (trimmed-notice, asset-scope refusals) |
| 26 | 101 | full-access budget round-division capped agent runs at 10 steps | FIXED (PR #30 merged `5b8f06d`), live re-run = §3 below |
| 27 | 103 | tab navigation unmounts ChatView — chat conversation destroyed + in-flight reply discarded | FIXED (PR #31 merged `3a73998`), live re-verify = §3 |
| 28 | 104 | temperature 0.7 on tool-bearing requests made the 12B stochastically ignore the tool protocol (2746-token prose dump, zero tool calls) | FIXED (PR #31: 0.2 tool-bearing / 0.7 toolless + pinning tests), live re-verify = §3 |

## 2. Capability panel — final truthful state (live on the re-staged drive)

Verified live 2026-09-13 12:11 UTC through the shipped Capabilities tab:

- USB Vault — Verified Working
- Real Audio Recording — Verified Working
- Browser Workspace — Verified Working
- Vision / OCR / Camera — Verified Working
- Offline STT / TTS — Verified Working
- Local Model Inference — Verified Working
- Documents & Memory — Verified Working
- Security & Manifest — Verified Working (22 manifest entries green)
- Hardware Profile — Builds, Not Runtime Tested *(honest: host hardware probing is build-time only)*
- Accessibility — **Verified Working** (flipped 2026-09-13: blind-aid lanes live-verified end-to-end — screen describe, OCR, camera, auto-speak)
- Agent Loop — see §3

## 3. Long-coding acceptance (the keystone lane)

*(result recorded after the run completes — defect #26 fix re-run on the re-staged drive)*

## 4. Speech acceptance (final state)

- Speech-verify suite: **7/7 PASS** (docs 96/98/99/100) — synthesis, playback, describe-then-speak, OCR lane, camera lane, deadline behavior.
- STS engine round-trip (2026-09-13, `sts-engine-roundtrip.js`): drive TTS synthesized a known 18-word sentence → on-device Qwen3-ASR (production SpeechRouter) transcribed it with **100% word recall, exact match** — real speech in, correct transcript out, fully offline.
- STS chat chain (2026-09-13, `sts-send.js`): mic → record → on-device ASR → transcript in input box → send → transcript message in chat history.
- Recording privacy (TRANSCRIPT_ONLY): audio destroyed post-ASR (recording count +0), only the encrypted transcript record persists (+1 per run).
- Acoustic human-speech capture: live-verified 2026-09-08 (drive A→Z, 100% ASR recall). This session's host has no physical speakers (render = line-out, Headphone endpoint unplugged) — loopback speech cannot reach the mic; the engine round-trip closes the verification gap.
- Attachments (2026-09-14, `attach-accept.js`, ALL PASS): hand-built PDF with a secret line — backend
  extractor parsed it, attachment pill rendered, answer quoted the secret verbatim ("UNOONE-7741");
  code file attached — answer gave the exact constant ("KIWI-3391"); Hindi question about the PDF —
  genuine Hindi answer ("संलग्न दस्तावेज़ में गुप्त कोड यह है: UNOONE-7741") with spoken audio mounted.
  Auto-speak toggle live-verified (reply audio mounted with no Speak click). Failure honesty also verified:
  while the model server was mid-restart, the lane surfaced a visible "Cannot connect to Gemma 4"
  error — no silent drop. Doc 89 §4 is fully ticked.
- Chat voice input chain (`sts-send.js`) and engine-level real-speech round-trip with 100% word recall
  close the STS lane; mic acoustic capture of human speech was live-verified 2026-09-08.

## 5. Full panel sweep (2026-09-14, final build)

*(every nav panel visited as a user through the shipped WebView — recorded after the sweep)*

## 6. README + git accuracy audit (2026-09-14)

*(README rows updated to match the final staged drive; git main == drive source)*

## 7. Remaining honest gaps

- Hardware Profile lane stays "Builds, Not Runtime Tested" by design (runtime host probing is build-gated).
- Hindi voice INPUT (mic) on this acceptance host: no physical speakers, so real Hindi speech cannot
  reach the mic acoustically; the multilingual ASR engine is covered by contract suites + CI, and mic
  acoustic capture itself was live-verified with a human voice 2026-09-08 (100% recall).