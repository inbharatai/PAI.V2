# Test evidence — 0.1.0-rc1

## Linux Release: PASS

Clean Release build, CTest `--parallel 2`: **11/11 passed, 0 failed**. Raw output is `linux-release-ctest.log`.

| Test | Covered evidence |
|---|---|
| `unit` | API/version, diagnostics, fallback, descriptor hashes/licenses, deferred KWS, ASR/TTS/VAD expected outputs, channel/resample/gain/normalization/clipping/non-finite PCM, WAV, SHA-256, strict root/hash policy, metrics |
| `streaming` | uneven ASR chunks, revisable partials/final parity, gaps/discontinuity, VAD boundaries, TTS PCM chunks, terminal polling/ownership |
| `lifecycle` | null/idempotent release, parent `BUSY`, live-buffer `BUSY`, **10,000** load/create/reset/release cycles and cache hits |
| `concurrency` | same-session `BUSY`, eight independent sessions × 20 calls |
| `cancellation` | **100** immediate idempotent TTS cancellations plus barge-in job interruption |
| `malformed` | four malformed RIFF fixtures and invalid audio/header/pointer shapes |
| `stress` | **5,000** 16-frame stream pushes with soft queue/backpressure behavior |
| `fuzz` | **5,000** deterministic random WAV blobs + **1,000** random PCM conversion pipelines |
| `abi-c99` | C99 compile, enum/layout static assertions, runtime/model catalog smoke |
| `cli` | every command, streamed/offline parity, malformed failure, deterministic TTS hash, benchmark JSON/CSV |
| `metadata` | compiled descriptors match registry and model-license hashes/licenses/availability/labels |

Synthetic fixture regeneration passed byte-for-byte (`fixture-reproducibility.log`). No external audio or model asset was used.

## ABI: PASS

`check_abi.py` parsed ELF `.dynsym`: **58/58 expected `ibaudio_*` exports**, no missing/extra API symbol. See `linux-release-abi.log` and `ABI_EVIDENCE.md`.

## Scope boundary

This validates deterministic reference behavior and native contracts. It does not validate neural ASR/TTS quality, Android devices, Windows runtime behavior, macOS, or accelerator inference.
