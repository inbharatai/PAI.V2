# Model Provenance

## Bundled (built-in) models

These are deterministic algorithms compiled into the runtime — not trained neural weights.

| ID | Task | Artifact | SHA-256 | License | Streaming (honest) |
|---|---|---|---|---|---|
| `reference-asr-v1` | ASR | built-in signal analyzer | `b73eaef0…220964` | Apache-2.0 | window-incremental, revisable partials, authoritative final |
| `reference-tts-v1` | TTS | built-in tone synthesizer | `4f334629…3ad4` | Apache-2.0 | segment-chunked deterministic PCM |
| `energy-vad-v1` | VAD | built-in algorithm | (registry) | Apache-2.0 | stateful low-latency frame energy |
| `kws-deferred-v1` | KWS | none | — | — | deferred |

Identity hashes are computed from the built-in descriptors; they are not content hashes of external weights (there are none).

## Upstream dependency

- **audio.cpp** — pinned to `bb15edd78b56e035967e0eb999a6b28a62337db4` (release-0.6), upstream `github.com/0xShug0/audio.cpp` (the recorded `ShugoAI` org URL no longer resolves; the pin-verification script targets the corrected remote). The adapter validates the pristine pin at configure time and is **deferred/default-off**; no upstream source or weights are copied into the default build. See `docs/audit/04_AUDIOCPP_UPSTREAM_DELTA.md` for the pin-vs-current analysis and the recommended 0.6.1 evaluation.

## External providers (spec seams, not bundled)

- **AI4Bharat** — IndicConformer ASR (22 scheduled languages, NeMo/PyTorch `.nemo`) and IndicF5 TTS (11 languages, HF `trust_remote_code`). Not vendored; local-service boundary only, no Python in core.
- **Sarvam** — Saaras v3 STT, Bulbul v3 TTS (remote API). Not vendored; gated behind `IBAUDIO_REMOTE_PROVIDERS=OFF`, no credentials or network client in the tree.

## Admission gate for any future neural model

A neural adapter becomes available only with: immutable source + weight revisions, exact hashes/sizes, SPDX + redistribution review, voice consent/provenance where relevant, selected-source notices, CPU parity/quality fixtures, cancellation points, memory/thread budgets, an honest streaming classification, and platform evidence. The broad upstream catalog is not imported.
