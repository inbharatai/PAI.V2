# UnoOne Model Licenses

> **Updated 2026-10-07:** replaced the "Gemma 2B IT candidate" and "Piper / Kokoro TTS" entries
> with what actually ships (Android: Gemma 4 E2B/E4B `.litertlm`, Coqui VITS English TTS, Meta MMS
> VITS Indic TTS, Zipformer wake-word model, EfficientDet-Lite2, ML Kit OCR; desktop drive: Gemma 4
> 12B/E4B/E2B GGUF + mmproj, Whisper base.en, Piper Bryce, Qwen3-ASR, OmniVoice). Recorded an **OPEN
> licence-review item**: the repository's own `vendor/Inbharat-audiocpp/docs/ALL_22_LANGUAGE_STRATEGY.md`
> lists Meta MMS-TTS as CC-BY-NC-4.0 ("non-commercial fallback only"), which contradicts the previous
> "all Apache-2.0 / commercial OK" summary. Licences below are what a source in this repository
> states (the Sherpa ASR row is carried over unchanged from the previous register); anything else is
> marked "not recorded — OPEN". This file makes no legal determination.

## Policy

Before any model is downloaded or distributed with UnoOne, the following must be verified and documented in this file:

1. **License** — commercial use allowed?
2. **File Size** — what is the total on-device size?
3. **RAM Requirement** — minimum / recommended RAM for inference?
4. **Android Compatibility** — does it run on Android (LiteRT / ONNX / TFLite)?
5. **Integration Path** — verified Kotlin/Java example exists?
6. **Attribution** — any required attribution text?

---

## OPEN licence-review item (owner) — Android Indic TTS

The six Indic TTS packs in `android-app/UnoOneAgent/modelmanager/src/main/assets/models_manifest.json`
(`sherpa-tts-hin`, `-ben`, `-tam`, `-tel`, `-kan`, `-mal`; versions `mms-vits-*`, downloaded from
`willwade/mms-tts-multilingual-models-onnx`) are Meta **MMS** VITS models. The repository's own
strategy document, `vendor/Inbharat-audiocpp/docs/ALL_22_LANGUAGE_STRATEGY.md` (TTS portfolio
table), records **Meta MMS-TTS** as **CC-BY-NC-4.0 — "non-commercial fallback only; not production
portfolio"**. This register previously listed Android TTS as Apache-2.0 and commercial-use "Yes",
but that entry described Piper/Kokoro voices that do not ship, and no source in the repository
supports it for the shipped models.

- **Status: OPEN — not resolved.** The models remain in the manifest; nothing was removed.
- **Owner action:** confirm the licence of the exact ONNX exports and of the upstream MMS
  checkpoints, and decide how Indic TTS may be distributed, before any commercial or store release.
- The English TTS checkpoint (Coqui VITS LJSpeech) has no licence recorded in the repository either
  and should be reviewed in the same pass.

---

## Gemma 4 (Local LLM, Android)

| Field | Value |
|-------|-------|
| **Model** | Gemma 4 E2B IT (`gemma-4-E2B-it.litertlm`, Lite, default) and Gemma 4 E4B IT (`gemma-4-E4B-it.litertlm`, Medium) |
| **Source** | Hugging Face `litert-community/gemma-4-E2B-it-litert-lm` and `litert-community/gemma-4-E4B-it-litert-lm` (URLs pinned in `models_manifest.json`) |
| **License** | [Gemma Terms of Use](https://ai.google.dev/gemma/terms) — the licence the README records for the Gemma 4 desktop model; not re-checked against the upstream model cards of these exact LiteRT artifacts in this pass |
| **File Size** | 2,588,147,712 B (E2B), 3,659,530,240 B (E4B) — SHA-256 pinned in the manifest |
| **RAM** | min 6,144 / recommended 8,192 MB (E2B); min 8,192 / recommended 10,240 MB (E4B) — provisional product gates (`BrainModel.kt`) |
| **Android Path** | LiteRT-LM (`litertlm-android` 0.13.1) via `GemmaPlanner` |
| **Verified Example** | E2B loaded on the Xiaomi 14 (CPU backend), 18/18 eval tool-match 2026-07-14 |
| **Status** | SHIPPED (manifest-pinned); device qualification incomplete — E4B not device-tested |

**Attribution:**
> Built with Gemma by Google.

**Next Steps:**
- [x] Download and verify on Xiaomi 14 (E2B load + 18/18 eval, 2026-07-14).
- [ ] Load and evaluate E4B on a device.
- [ ] Check inference speed (< 2s for short prompt).
- [ ] Verify JSON structured output reliability.
- [ ] Confirm the Gemma terms against the upstream model cards for the exact shipped artifacts.

---

## Sherpa-ONNX ASR

| Field | Value |
|-------|-------|
| **Model** | English Zipformer transducer and Omnilingual ASR 300M CTC INT8 |
| **Source** | https://github.com/k2-fsa/sherpa-onnx |
| **License** | Apache-2.0 |
| **File Size** | ~70 MB English; 292,571,207-byte Omnilingual archive |
| **RAM** | Device qualification required; Indic pack declares a 2 GB minimum |
| **Android Path** | sherpa-onnx Android AAR / JNI |
| **Verified Example** | sherpa-onnx Android examples |
| **Status** | APPROVED |

Note (2026-10-07): the Apache-2.0 entry above is carried over from the previous register; the
repository records no per-checkpoint licence for these exact exports. Confirm it in the same review
as the TTS item.

**Attribution:**
> Speech recognition powered by sherpa-onnx (Apache-2.0).

**Next Steps:**
- [x] Integrate sherpa-onnx AAR into `voice` module.
- [x] Run native-script functional speech gates on the primary device.
- [ ] Complete controlled WER/CER, noise, accent and second-device qualification.

---

## Sherpa-ONNX TTS (Android)

| Field | Value |
|-------|-------|
| **Model** | English: Coqui VITS LJSpeech (`csukuangfj/vits-coqui-en-ljspeech`, manifest `sherpa-tts-en`) + bundled `espeak-ng-data`. Indic: Meta MMS VITS ONNX exports per language (`willwade/mms-tts-multilingual-models-onnx`, manifest `sherpa-tts-{hin,ben,tam,tel,kan,mal}`) |
| **Source** | Hugging Face URLs pinned in `models_manifest.json` |
| **License** | Indic MMS-TTS: **CC-BY-NC-4.0 per `vendor/Inbharat-audiocpp/docs/ALL_22_LANGUAGE_STRATEGY.md` — OPEN review (see above)**. English Coqui LJSpeech: not recorded in this repository — OPEN |
| **File Size** | ~114 MB per voice model (+ ~9 MB `espeak-ng-data` for English) |
| **RAM** | Manifest gate 512 MB per pack; device-measured RAM not recorded |
| **Android Path** | sherpa-onnx Android AAR / JNI (`OfflineTtsVitsModelConfig`) |
| **Verified Example** | Primary Xiaomi 14 passed native-script MMS-TTS→STT round trips for the six Indic languages (`SPEECH_MODEL_QUALIFICATION.md`) |
| **Status** | SHIPPED in the manifest (integrity-pinned); **licence review OPEN** |

**Attribution:**
> Text-to-speech powered by sherpa-onnx.
> (Attribution text required by the Coqui LJSpeech and MMS checkpoints: to be settled in the licence review.)

**Next Steps:**
- [x] Select default voices (Coqui VITS for English, one MMS VITS model per Indic language).
- [ ] Test latency on Xiaomi 14.
- [ ] Owner licence review of MMS-TTS and Coqui LJSpeech (OPEN item above).

---

## Wake word (Android manifest id `vad`)

| Field | Value |
|-------|-------|
| **Model** | English streaming Zipformer transducer INT8 (`csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26`) — the same encoder/decoder/joiner files as `sherpa-asr-en`, used by the Sherpa-ONNX KeywordSpotter for the English wake word. No Silero VAD model ships in the Android manifest |
| **Source** | Hugging Face URLs pinned in `models_manifest.json` |
| **License** | Same checkpoint as the English ASR entry above |
| **File Size** | ~70 MB (files shared with `sherpa-asr-en`) |
| **RAM** | Manifest gate 256 MB |
| **Android Path** | Built into sherpa-onnx |
| **Status** | SHIPPED |

---

## Blind Aid object detector (Android)

| Field | Value |
|-------|-------|
| **Model** | EfficientDet-Lite2 int8 (`phonecontrol/src/main/assets/models/efficientdet_lite2_int8.tflite`, bundled in the APK) |
| **Source** | Google MediaPipe model storage (see [`BLIND_AID_MODEL.md`](BLIND_AID_MODEL.md)) |
| **License** | Apache 2.0 (per `BLIND_AID_MODEL.md`) |
| **File Size** | 7,515,971 B |
| **Android Path** | MediaPipe Tasks Vision `ObjectDetector` (`BlindAidManager`) |
| **Status** | SHIPPED |

---

## Punctuation Model (Optional)

| Field | Value |
|-------|-------|
| **Model** | sherpa-onnx punctuation model |
| **Source** | sherpa-onnx releases |
| **License** | Apache-2.0 |
| **File Size** | ~10–50 MB |
| **Status** | DEFERRED — nice to have; not shipped (no entry in `models_manifest.json`) |

---

## OCR Model

| Field | Value |
|-------|-------|
| **Model** | Android: Google ML Kit on-device text recognition (`com.google.mlkit:text-recognition` + `text-recognition-devanagari` 16.0.0, model bundled with the library). Desktop: OCR runs through the Gemma GGUF + mmproj (no separate OCR model) |
| **Source** | Google Maven (Android); see the desktop section below |
| **License** | ML Kit terms: not recorded in this repository — OPEN |
| **Status** | SHIPPED (library dependency); terms review OPEN |

---

## Desktop (Pocket AI drive) models

These ship on the staged Pocket AI drive, not in the APK and not in Git.

| Model | Role | Licence as recorded in this repository | Repository source |
|---|---|---|---|
| Gemma 4 12B IT Q4_K_M GGUF + same-tier mmproj | flagship desktop planner, vision, OCR | [Gemma Terms of Use](https://ai.google.dev/gemma/terms) | README "Model Verification" |
| Gemma 4 E4B / E2B GGUF + same-tier mmproj | Medium / Lite desktop tiers | not separately recorded (same model family) — OPEN | README "Desktop model ladder"; [`MODEL_ACQUISITION_AND_DISTRIBUTION.md`](MODEL_ACQUISITION_AND_DISTRIBUTION.md) |
| Whisper `base.en` (`whisper-base.en.bin`, whisper.cpp v1.9.1) | legacy English STT | MIT (Whisper / whisper.cpp row) | `vendor/Inbharat-audiocpp/docs/ALL_22_LANGUAGE_STRATEGY.md`; [`52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md`](52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md) |
| Piper `en_US-bryce-medium` (`voice.onnx`, Piper 2023.11.14-2) | legacy English TTS | model card declares public-domain training data; voice and runtime licence not recorded — OPEN | [`52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md`](52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md) |
| Qwen3-ASR-0.6B (Q8_0 GGUF) | InBharat Audio ASR | Apache-2.0 | `vendor/Inbharat-audiocpp/licenses/MODEL_LICENSES.json` |
| OmniVoice (Q8_0 GGUF) | InBharat Audio TTS | not recorded in this repository — OPEN | `SPEECH/config/inbharat-audio.v1.json` |

Runtime binaries (llama.cpp, whisper.cpp, Piper, the audio.cpp CLI, the sherpa-onnx AAR, LiteRT-LM,
MediaPipe) carry their own licences, separate from model licences, and are not recorded in this
register — OPEN.

---

## License Summary Table

| Model | License (per repository source) | Commercial (per that source) | Attribution Required | Status |
|-------|---------|------------|----------------------|--------|
| Gemma 4 E2B/E4B `.litertlm` (Android) | Gemma Terms | Per Gemma Terms — owner to confirm | Yes | Shipped (pinned) |
| Gemma 4 12B/E4B/E2B GGUF + mmproj (desktop) | Gemma Terms (12B recorded) | Per Gemma Terms — owner to confirm | Yes | Shipped on drive |
| Sherpa-ONNX ASR | Apache-2.0 (carried over; per-checkpoint not recorded) | Yes | Recommended | Approved |
| Wake word (`vad`, EN Zipformer) | Same as EN ASR | Same as EN ASR | Recommended | Shipped |
| TTS — English Coqui VITS LJSpeech | Not recorded — OPEN | Unknown | TBD | Shipped; review OPEN |
| TTS — Indic Meta MMS VITS | **CC-BY-NC-4.0** (ALL_22_LANGUAGE_STRATEGY.md) | **Non-commercial per that source** | TBD | Shipped; **licence review OPEN** |
| Blind Aid EfficientDet-Lite2 | Apache 2.0 | Yes | Recommended | Shipped |
| ML Kit text recognition | Not recorded — OPEN | Unknown | TBD | Shipped (library) |
| Whisper base.en (desktop) | MIT | Yes | Recommended | Shipped on drive |
| Piper Bryce voice (desktop) | Not recorded — OPEN (public-domain training data per model card) | Unknown | TBD | Shipped on drive |
| Qwen3-ASR-0.6B (desktop) | Apache-2.0 | Yes | Recommended | Shipped on drive |
| OmniVoice (desktop) | Not recorded — OPEN | Unknown | TBD | Shipped on drive |
| Punctuation | Apache-2.0 | Yes | Recommended | Deferred (not shipped) |

---

## Compliance Checklist

- [ ] All approved model licenses allow commercial distribution. (**Blocked** by the OPEN MMS-TTS item and the "not recorded" rows above.)
- [ ] Attribution text included in Settings → About → Open Source.
- [ ] Model files are not modified in a way that violates license terms.
- [ ] No GPL/AGPL models used without legal review.
