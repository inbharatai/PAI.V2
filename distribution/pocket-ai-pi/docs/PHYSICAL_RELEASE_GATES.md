# Pocket AI Pi physical release gates

All gates are pass/fail and must be captured against the exact hardware revision, Pi model/RAM, OS image, kernel, firmware, binaries, model hashes and speech-model hashes.

1. Package manifest verification passes before any model starts.
2. Vault identity and encrypted record read/write/reopen/recovery pass.
3. Surprise USB removal during idle, inference, memory write, ASR and TTS causes cancellation and no plaintext residual files.
4. Harness L0/L1/L2 routes execute against real providers; denied capabilities fail closed; no generic shell tool is model-visible.
5. Model server binds only to 127.0.0.1, reports the expected model id and uses the manifest-hashed GGUF.
6. ASR executes real audio.cpp model inference on recorded English/Hindi/Hinglish acceptance samples and produces non-empty transcript; WER/CER must be measured, not guessed.
7. TTS executes real audio.cpp inference and generates a decodable WAV; latency/RTF and intelligibility are recorded.
8. VAD is tested with silence, speech, background noise and interruption/barge-in sequences.
9. 30-minute sustained mixed workload records max RSS, load, temperature, throttling flags and latency drift. Vulkan may ship only if output parity and stability pass on the exact image.
10. Reboot, power-loss/journal recovery and corrupted-asset/hash-mismatch tests all fail/recover as designed.
11. Offline/airplane operation proves core chat, memory and speech do not require cloud connectivity.
12. Physical mic kill switch prevents capture irrespective of software state; software cannot override it.
