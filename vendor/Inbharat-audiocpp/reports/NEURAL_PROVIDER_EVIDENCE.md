# Neural Provider Evidence — audio.cpp Silero VAD (2026-08-20)

The first **real neural inference** through the InBharat C ABI, via a provider wrapping pinned audio.cpp (release-0.6, commit `bb15edd7`).

## What is real

`AudioCppProvider` (`src/adapters/audio_cpp/audio_cpp_provider.cpp`) wraps audio.cpp's **bundled Silero VAD** neural model (`silero_vad_16k.safetensors`, ships inside the pristine pinned checkout — no download, no unreviewed weights). It is compiled only under `IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=ON` with the pristine-pin gate, so the dependency-free default core never links ggml/sentencepiece.

## Measured (host: Linux x86_64, g++ 11.5, Release, CPU)

audio.cpp CLI direct (Silero VAD, 1.5 s tone fixture): wall 31.7 ms, RTF 0.021, ~47× real-time.

Through the InBharat C ABI (`ibaudio vad --model audiocpp-silero-vad-v1`), on a synthesized voiced signal (0.4 s silence / 1.2 s formant-like voiced / 0.4 s silence):

| Provider | Detected segment | Reading |
|---|---|---|
| `audiocpp-silero-vad-v1` (neural) | frames 14880–26080 (~0.93–1.63 s) | conservative neural speech bounds |
| `energy-vad-v1` (reference) | frames 6240–26080 (~0.39–1.63 s) | wider energy-threshold bounds |

The two providers genuinely differ because they are different algorithms — the neural model is more conservative on the voiced onset. This is real inference, not a passthrough.

## Honestly NOT done (gated, not faked)

- **ASR and TTS return UNAVAILABLE** in `AudioCppProvider`. audio.cpp has no bundled ASR/TTS weights, and no licensed ASR/TTS model is vendored into this tree. They are gated behind a caller-supplied licensed model path — a future phase. No speech-recognition or natural-voice claim is made.
- **Streaming VAD** (Silero's true streaming path) is declared in capabilities but the current provider call uses the offline session; streaming VAD through the ABI is a follow-up.
- **Evidence level: host-tested** for VAD inference on Linux x86_64. Android/ARM64/accelerators remain PENDING.

## Verification

- Default (adapter OFF) build: 15/15 tests + ABI 79/79, no Silero model registered, dependency-free.
- Adapter (adapter ON) build: 15/15 tests + ABI 79/79, Silero model registered and loadable, neural VAD runs through the C ABI.
- The four test adjustments made to reach this (build-aware `model_count`, position-independent KWS lookup, registry/license entries, adapter-aware metadata comparison) are test-harness correctness for the new registered model — no production logic was weakened.
