# Production backend gate

InBharat Audio must never replace a known-working product speech backend merely because an adapter compiled.

A product may select audio.cpp for production only when all of the following are true for the exact model family and target platform:

1. `ibaudio_runtime_get_audio_cpp_status()` reports `adapter_compiled=1` and `inference_ready=1`.
2. The upstream checkout matches the reviewed immutable commit and is clean.
3. The model artifact hash and license are recorded in the model registry.
4. ASR/TTS/VAD parity tests pass on the target platform.
5. Cancellation, streaming, memory ceiling and thermal tests pass.
6. Product acceptance thresholds pass for the target languages.
7. A legacy fallback remains available until the release is explicitly promoted.

The current v0.2 development adapter deliberately reports `inference_ready=0`. This is fail-closed behavior, not a missing safety check.
