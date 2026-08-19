# Models and descriptors

Compiled descriptors and `models/registry.v1.json` list four v1 entries.

| ID | Task | Executable | Streaming truth | Artifact |
|---|---|---:|---|---|
| `reference-asr-v1` | ASR | yes | window-incremental, provisional/revisable partials; authoritative final | built-in deterministic signal analyzer |
| `reference-tts-v1` | TTS | yes | segment-chunked PCM after deterministic generation | built-in tone synthesizer; no voice/weights |
| `energy-vad-v1` | VAD | yes | stateful low-latency frame energy with hysteresis | built-in algorithm |
| `kws-deferred-v1` | KWS | no | deferred | none |

Reference ASR reports signal properties; it does not recognize language. Reference TTS maps UTF-8 bytes to tones; it is not a natural voice. These engines make API, lifecycle, audio, streaming, and platform work testable without importing unreviewed models.

Every descriptor carries a stable built-in identity SHA-256, SPDX license, source URI/revision, task/capability bits, required format, and availability reason. An external artifact load requires a regular path, optional root containment, 64-hex expected hash when verification is on, and successful SHA-256 comparison.

## Model admission gate

A neural/model adapter remains unavailable until it has immutable source and weight revisions, exact hashes/sizes, SPDX and redistribution review, voice consent/provenance where relevant, selected-source notices, CPU parity/quality fixtures, cancellation points, memory/thread budgets, honest streaming classification, and platform evidence. The broad upstream catalog is not imported.
