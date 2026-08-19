# InBharat Audio 0.1.0-rc2 hardening validation

Validated: 2026-08-17T18:55:24Z  
Baseline head: `461606efc85c0252034928f3f2332b34e9625d0e`  
Source-fix commit: `7d0924e`  
Result: PASS within the documented local boundary.

## Clean gates

- Linux x86_64 Release shared-library build: pass.
- Release CTest: 11/11 passed, including unit, streaming, lifecycle, concurrency, cancellation, malformed, stress, deterministic fuzz, C99 ABI, CLI, and metadata lanes.
- ASan+UBSan with leak detection: 11/11 passed.
- Static-library ThreadSanitizer lane: 11/11 passed with halt-on-error. The Zig shared-library TSan runtime crashed before `main`, so it is not counted; static linkage avoids duplicate sanitizer-runtime linkage.
- ABI export check: 58/58 `ibaudio_*` symbols matched.
- Install/export and fresh C99 `find_package` consumer: pass; consumer verified API 1.0 and runtime `0.1.0-rc2`.
- Fixture byte-for-byte regeneration: pass.
- JNI C++17 warning-as-error compile against a local JNI ABI stub: pass.
- Windows x64 cross-build: pass as build-only evidence.
- Clean pinned audio.cpp adapter scaffold: pass; deliberately wrong pin: rejected.

## Adversarial coverage added

- 50,000 deterministic random WAV inputs and 10,000 random PCM conversion pipelines in Release, ASan+UBSan, and static TSan lanes.
- Strict-path mode with no allowed root, outside-root hashing, and outside-root artifact load fail closed.
- Extreme VAD minima and invalid barge threshold rejected before arithmetic.
- Duplicate asynchronous job start returns `BUSY` before copying input.
- TTS queue pressure preserves all 77,760 expected frames while staying within the soft queue plus terminal event.
- Alternating 1 ms VAD boundaries exceed the absolute queue ceiling and converge to one terminal cancellation without leaked buffers.
- Cancelled stream finish returns matching structured last-error state.
- Existing 10,000 lifecycle loops, 100 cancellation loops, concurrent sessions, 5,000 stream chunks, malformed fixtures, low-copy/audio/WAV/hash, and backend fallback tests remain passing.

## CPU benchmark

1,000 iterations on the sandbox CPU:

- Reference ASR mean: 0.229853 ms.
- Energy VAD mean: 0.126018 ms.
- Reference TTS mean: 0.524235 ms.

These deterministic reference-engine timings are environment-specific and are not neural-model performance claims.

## Artifacts

Hashes and sizes are in `HARDENING_ARTIFACTS.sha256` and `HARDENING_ARTIFACT_SIZES.txt`.

## Residual boundary

No claim is made for physical Android lifecycle/thermal behavior, macOS/Metal, Windows runtime, CUDA/HIP/Vulkan inference, neural English/Hindi/Hinglish accuracy, uncooperative backend preemption, or safe use of invalid/stale C pointers. See `HARDENING_AUDIT.md` and `docs/KNOWN_LIMITATIONS.md`.
