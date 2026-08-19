# Known limitations

- Reference ASR analyzes signal characteristics; it does not transcribe language.
- Reference TTS emits deterministic tones; it is not natural speech and has no voice identity.
- KWS returns `DEFERRED`; no keyword model is present.
- CPU is the only inference backend. Vulkan is loader-only probing; all other accelerators are unavailable.
- Linear resampling is deterministic but not production bandlimited quality.
- WAV only: PCM16, PCM24, float32 input and PCM16 output; no MP3/AAC/Opus/FLAC or RF64.
- TTS stream start generates synchronously before chunk polling; use a TTS job for cancellable generation.
- Stream queue limits are soft for terminal/VAD boundary preservation, but RC2 enforces an absolute 4,096-event cancellation ceiling and coalesces contiguous TTS chunks under pressure.
- Cooperative cancellation cannot preempt an uncooperative future backend call.
- Raw C stale-pointer misuse cannot be detected after release; bindings must encapsulate handles.
- Cache is an in-memory model identity LRU, not a persistent weight cache.
- Windows x64 cross-build evidence exists, but no Windows execution; no Android build/device or macOS evidence was produced in this Linux-only sandbox.
- No external model/asset distribution approval or legal opinion is implied.
