# Changelog

## 0.1.0-rc2

Adversarial hardening release: strict path policy now fails closed when no allowed root is configured; VAD and barge-in configuration is bounded; asynchronous jobs reserve single-flight ownership before copying inputs; stream queues coalesce audio and have an absolute terminal-preserving limit; cancelled stream errors are explicit; native output allocation paths are exception-safe; JNI performs standard UTF-8/UTF-16 conversion with bounded arrays and contained allocation failures; deterministic malformed campaigns were expanded; Release, ASan+UBSan, static ThreadSanitizer, Windows cross-build, ABI, and JNI syntax gates were added.

## 0.1.0-rc1

First local release candidate: versioned C99 ABI; CPU backend policy; deterministic reference ASR/TTS; energy VAD; deferred KWS; PCM/WAV processing; jobs, streaming, cancellation, barge-in; lifecycle, metrics, cache/path policy; CLI; synthetic fixtures and comprehensive tests; optional pristine-pin audio.cpp scaffold; Android/JNI/Kotlin and desktop/CI scaffolding; provenance and evidence documentation.
