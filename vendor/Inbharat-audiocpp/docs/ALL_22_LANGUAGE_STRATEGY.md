# All-22 Indian Language Strategy — Evidence Before Support

Date: 2026-08-21. Target release platforms: Windows 11 and Android arm64. Raspberry Pi is out of scope.

## Executive decision

There is **no honest one-model answer** for accurate, lightweight, human-like STT + TTS across all 22 Scheduled Indian languages today.

- **STT:** AI4Bharat IndicConformer-600M-Multilingual is the only permissively licensed local model found that explicitly lists all 22. It is the primary all-22 candidate, but public same-dataset per-language WER evidence is incomplete and official Android support is absent. Therefore it is a candidate, not 22-language VERIFIED support.
- **TTS:** no single permissively licensed local model is proven at acceptable per-language quality across all 22. IndicF5 covers 11; Indic-Parler-TTS adds five Scheduled languages beyond those 11; Bolbosh has Kashmiri-specific evidence; Indic-Mio claims all 22 but lacks sufficient per-language quality/consent/streaming evidence for production support.
- **Language detection:** no compact permissive acoustic LID model was found with explicit, measured 22-way coverage. Script detection is not language detection. The product must combine acoustic evidence, script/text metadata, history/user policy and explicit abstention.

## STT candidates

| Candidate | Explicit Scheduled-language coverage | License | Size | Streaming truth | Windows / Android | Decision |
|---|---:|---|---:|---|---|---|
| [AI4Bharat IndicConformer-600M-Multilingual](https://huggingface.co/ai4bharat/indic-conformer-600m-multilingual) | 22 | MIT | 600M | RNNT/real-time publisher claim; production chunk semantics still need validation | Windows local-service plausible; Android community exports only partial coverage | Primary all-22 candidate, PENDING per-language gates |
| [Qwen3-ASR](https://github.com/QwenLM/Qwen3-ASR) through audio.cpp | Hindi among the Scheduled 22 | Apache-2.0 | 0.6B / 1.7B | true streaming model; current official route/backend details vary | Windows evidenced through audio.cpp; Android community path only | Keep for verified Hindi/English route, not all-22 |
| [Whisper / whisper.cpp](https://github.com/ggml-org/whisper.cpp) | generic multilingual; no explicit all-22 evidence | MIT | 39M–1.55B | repeated-window application streaming, not necessarily stateful model streaming | Windows + Android build paths exist | Engineering fallback only where same-language benchmark passes |
| [Meta MMS-1B-All](https://huggingface.co/facebook/mms-1b-all) | broad enough in principle | CC-BY-NC-4.0 | 1B | offline; true streaming unverified | no official Windows/Android support evidence | Excluded from commercial permissive portfolio |
| Sarvam Saaras v3 | publisher all-22 + English | proprietary/API; no downloadable permissive ASR checkpoint verified | unpublished | publisher streaming | proprietary edge/API | Benchmark reference, not open local provider |

## TTS portfolio

| Candidate | Scheduled-language coverage | License | Streaming truth | Decision |
|---|---:|---|---|---|
| [AI4Bharat IndicF5](https://huggingface.co/ai4bharat/IndicF5) | 11: as,bn,gu,hi,kn,ml,mr,or,pa,ta,te | MIT | offline utterance generation; chunk wrappers are not true streaming evidence | Primary candidate for its 11; native benchmark required |
| [Indic-Parler-TTS](https://huggingface.co/ai4bharat/indic-parler-tts) | 16 Scheduled languages | Apache-2.0 | optimized serving claimed; true incremental streaming unverified | Candidate for Bodo, Dogri, Manipuri, Nepali, Sanskrit; benchmark overlaps too |
| [Bolbosh](https://github.com/gaash-lab/Bolbosh) | Kashmiri | checkpoint licensing/consent needs written confirmation | offline | Conditional Kashmiri candidate |
| [Indic-Mio](https://huggingface.co/SPRINGLab/Indic-Mio) | claims all 22 + English | Apache-2.0 repository | no demonstrated true streaming | Provisional gap candidate for Konkani, Maithili, Santali, Sindhi, Urdu; cannot advertise until native tests pass |
| Meta MMS-TTS | all 22 checkpoints | CC-BY-NC-4.0 | offline | non-commercial fallback only; not production portfolio |

## Bharat Speech Mesh

The runtime must route evidence, not model names.

1. **Candidate admission:** same language + task + device must be VERIFIED with a report hash. Publisher coverage stays PENDING.
2. **Hard filters:** privacy (no silent cloud), device, true-streaming requirement, measured peak memory, quality floor and calibrated confidence.
3. **Pareto score:** quality dominates; confidence follows; measured memory and latency are bounded penalties. A small weak model never wins merely for being small.
4. **Ambiguity:** close provider scores abstain. Conflicting high-confidence transcripts abstain unless independent providers agree or one has a calibrated decisive margin.
5. **Romanized speech:** Latin script is not English evidence. Acoustic/history/user evidence must resolve Romanized Hindi, Assamese, Konkani, etc.
6. **Local adaptation:** names, lexicons, scripts and correction statistics are reversible local data; no continuous unreviewed weight mutation.

Implementation: `src/mesh/speech_mesh.*`; tests: `tests/speech_mesh_tests.cpp`.

## Pack catalog

All 22 packs live under `packs/<code>/pack.json` and are generated/validated by:

```sh
python scripts/generate_22_language_packs.py
python scripts/validate_22_language_packs.py
```

The validator requires 22 unique codes exactly, explicit PENDING/FAILED/VERIFIED per task/platform, and report+artifact hash evidence for any VERIFIED status. No pack currently advertises broad all-22 VERIFIED support.

## Release gates

Per language, before support is advertised:

- STT: same-dataset WER/CER, code-mix WER, proper-name/numeral accuracy, unknown-language false-accept rate, TTFT/RTF/RAM.
- TTS: intelligibility CER via an independent ASR, native-rater MOS/preference, Indian names/numbers/code-mix, TTFA/RTF/RAM, speaker-consent provenance.
- LID: confusion matrix across all 22 + English + unknowns; calibrated abstention; Romanized test set.
- Speech-to-speech: semantic preservation, language/script preservation, stage-level confidence and end-to-end latency.
- Windows/Android: physical execution evidence. Cross-builds remain build-only.

A release matrix may contain VERIFIED, FAILED and PENDING rows. “All 22 in one release” means all 22 are tested and reported—not that failures are hidden or converted into support claims.
