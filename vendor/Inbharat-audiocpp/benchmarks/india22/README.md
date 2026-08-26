# India-22 Speech Benchmark Harness

This directory intentionally contains **no unlicensed audio corpus**. Add licensed, held-out audio and JSONL references locally; do not train on them.

STT result rows:

```json
{"id":"as-street-0001","language":"as-IN","reference":"...","hypothesis":"..."}
```

Score and require every Scheduled language:

```sh
python scripts/score_india22_stt.py results.jsonl --require-all-22 --output reports/india22-stt.json
```

The scorer reports per-language WER and CER separately and fails when a language is missing. A single aggregate is insufficient: a strong Hindi route must not hide a failed Santali or Bodo route.

TTS result rows include `reference_text`, an `independent_asr_transcript`, `ttfa_ms`, `rtf`, and optional `native_mos_ratings` (1–5):

```sh
python scripts/score_india22_tts.py tts-results.jsonl --require-all-22 --output reports/india22-tts.json
```

The TTS scorer reports intelligibility CER, time-to-first-audio, real-time factor and native MOS only when ratings were actually supplied. It never fabricates MOS from a proxy.

A release benchmark should include at least these condition labels in its external case manifest: quiet, office, street, vehicle, far-field, 8kHz telephony, code-mix, proper names, dates, currency, addresses and app names. The evaluator should additionally record provider/model hash, pack hash, device, backend, true-streaming class, TTFT, RTF and peak RAM.
