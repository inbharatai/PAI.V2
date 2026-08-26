# India Benchmark Specification

A licensed, held-out evaluation set for Indian-language speech. This benchmark decides whether a release's India claims are real. **It is never trained on** — model and pack changes must not fit to it.

## Coverage

Languages (first wave): **en-IN, hi-IN, hi-en-codemix (Hinglish)**. Assamese (as-IN) follows as a separate validation effort.

Conditions (each language × each condition):
- quiet speech
- street noise
- vehicle
- office
- 8 kHz telephony
- different microphones
- code-mixing (mid-sentence Hindi↔English)
- Indian proper nouns (names, places — e.g. Guwahati, Nagaon, Tezpur)
- dates, currency (₹, lakh, crore), addresses, app names

## Metrics

**ASR** — WER, CER, code-mix WER, proper-noun accuracy, language-identification accuracy, time-to-first-token, real-time factor, peak RAM, model-load time.

**TTS** — time-to-first-audio, real-time factor, peak RAM, pronunciation score, code-mix pronunciation; human MOS-style evaluation where feasible.

**Platform** — binary size, model size, RAM, CPU, battery, thermal, cold start, warm start.

## Discipline

- **Licensing.** Every clip must have a recorded license permitting evaluation use. Source and license are logged per clip.
- **Held-out.** The set is never used for training, fine-tuning, or threshold fitting. A separate dev set is used for iteration.
- **Evidence level.** A benchmark result is labeled with the evidence level of the run (host/emulator/physical-device) and the provider that produced it.
- **No cherry-picking.** The full per-language × per-condition matrix is reported, including failures.

## Status

This is a specification. No benchmark corpus is bundled (licensing and recording are prerequisites). The deterministic reference engines are **not** evaluated against it — they do not recognize language, so a WER against them would be meaningless. The benchmark becomes meaningful once a real neural provider (pinned audio.cpp, AI4Bharat, or Sarvam) is producing transcripts.
