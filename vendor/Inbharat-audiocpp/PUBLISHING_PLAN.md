# Publication plan

Status: local only. Do not publish, push, tag remotely, or create a public repository without explicit approval.

Before publication:

1. Re-run clean Release and sanitizer builds, all CTest lanes, ABI/export checks, fixture reproduction, benchmarks, install/export, and a fresh C99 consumer.
2. Execute Windows, macOS, Android NDK/emulator, and physical Android gates before making those support claims.
3. Validate every selected neural model for accuracy, latency, TTFA/RTF, memory, stability, streaming class, and licence on each advertised backend.
4. Regenerate source/binary SBOMs, third-party notices, model licence manifests, hashes, dependency closure, and vulnerability scans.
5. Scan history and archives for secrets, private paths, caches, models, voices, generated audio, and non-redistributable binaries.
6. Produce deterministic archives, SHA-256 manifests, provenance, signing, and attestations in an approved release environment.
7. Obtain security, product, legal, privacy, model/voice, trademark, and accelerator-redistribution approval.

The local RC is a framework and deterministic reference implementation. It is not proof of neural ASR/TTS quality, Android lifecycle behavior, mobile thermals, or accelerator inference.
