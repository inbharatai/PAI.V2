# InBharat Audio — universal native audio runtime 0.2

A C++17, C-ABI-first audio runtime for InBharat products. Production builds are intentionally free of synthetic/reference ASR/TTS and research-only heuristic modules. Real speech inference is supplied by verified audio.cpp artifacts/model packages through the product integration boundary; deterministic ASR/TTS fixtures are compiled only in explicit test builds.

**Runtime:** `0.2.0-dev.1`, API major `1`. Default production configuration includes the real frame-energy VAD and lifecycle/streaming/cancellation/diagnostics infrastructure, while real ASR/TTS availability remains fail-closed until the exact `audiocpp_cli` and model assets pass hash, license, language, platform and end-to-end acceptance gates.

Linux x86_64, Windows, macOS, Android and Linux ARM64 are treated as separate evidence targets. Cross-compilation is not accepted as physical-runtime proof.

## What is implemented

- One exported C99 header: `include/inbharat/ibaudio.h`; 58 reviewed `ibaudio_*` symbols and SONAME major 1.
- Opaque runtime/model/session/job/stream/buffer handles; C++ exceptions are contained.
- Thread-local structured errors, stable status/domain enums, versioned extensible structs.
- Borrowed synchronous inputs; copied asynchronous requests; immutable library-owned output handles.
- Explicit session single-flight (`BUSY`), parent/child teardown contracts, metrics, bounded model-key LRU.
- BackendManager behavior: portable CPU always; CUDA/HIP/Metal/NNAPI/CoreML/DirectML unavailable with reasons; optional Vulkan loader-only probe; policy-gated CPU fallback.
- Model descriptors with task, capability bitset, honest streaming class/label, SHA-256, SPDX license, source revision, availability reason.
- Production: stateful streaming energy VAD plus runtime/session/job/stream infrastructure. Synthetic ASR/TTS and deferred KWS are excluded unless explicit test-fixture mode is enabled.
- PCM validation, non-finite sanitization, channel conversion, gain, peak normalization, clipping, deterministic linear resampling, chunk continuity/discontinuity, PCM16/24/float32 WAV decode, PCM16 WAV encode.
- Cooperative asynchronous cancellation, pull events, interruption/barge-in state.
- Production `ibaudio` CLI exposes runtime/model/VAD/diagnostic surfaces; ASR/TTS fixture commands exist only in explicit test-fixture builds. Real speech acceptance is performed against the verified audio.cpp CLI/model packages.
- Synthetic deterministic fixtures are test-only; malformed corpus, lifecycle/concurrent/stress/cancellation/fuzz-style tests remain part of engineering validation.
- Android arm64 Gradle/JNI/Kotlin bridge, desktop builds, and Linux ARM64/Raspberry Pi native/cross-build tooling. CPU is the baseline; acceleration requires target-specific evidence.
- Optional isolated audio.cpp source-pin scaffold plus a real external audio.cpp production execution path in Pocket AI. The newer upstream candidate is tracked separately and cannot become approved merely because it is newer.

## Build and test with the supplied user-space toolchain

```sh
./scripts/configure_build_test.sh
./scripts/run_sanitizers.sh     # where sanitizer runtime support is available
./scripts/run_benchmarks.sh
```

Manual system-tool build:

```sh
cmake -S . -B build/release -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build build/release --parallel
ctest --test-dir build/release --output-on-failure
```

## CLI examples

Production build:

```sh
build/linux-release/ibaudio info --json
build/linux-release/ibaudio models --json
build/linux-release/ibaudio vad --input speech.wav --json
```

Fixture ASR/TTS commands are available only when `IBAUDIO_ENABLE_TEST_FIXTURE_MODELS=ON`; they must never be used as production speech evidence.

## Evidence and support labels

`reports/` records exact commands/tool versions/results. Evidence levels are deliberately distinct: source scaffold, configure/build-only, emulator-tested, and physical-device-tested. This sandbox supplies Linux host + ASan/UBSan evidence and a Windows x64 cross-build; Windows execution, Android, and macOS remain untested at runtime. Vulkan loader detection is not Vulkan inference support.

## Documentation map

- [Architecture](docs/ARCHITECTURE.md), [C ABI](docs/C_API.md), [ABI compatibility](docs/ABI_COMPATIBILITY.md)
- [Models](docs/MODELS.md), [backends](docs/BACKENDS.md), [streaming](docs/STREAMING.md)
- [Audio processing/WAV](docs/AUDIO_PROCESSING.md), [lifecycle/threading](docs/LIFECYCLE_THREADING.md)
- [Cancellation and barge-in](docs/CANCELLATION_BARGE_IN.md), [path/security](docs/SECURITY_PATH_POLICY.md)
- [CLI](docs/CLI.md), [building](docs/BUILDING.md), [testing](docs/TESTING.md), [benchmarks](docs/BENCHMARKS.md)
- [Diagnostics/metrics/cache](docs/DIAGNOSTICS_METRICS.md), [portability](docs/PORTABILITY.md), [Android](docs/ANDROID.md)
- [Linux ARM64 / Raspberry Pi](docs/LINUX_ARM64.md)
- [Derivation/provenance](docs/UPSTREAM_DERIVATION.md), [known limitations](docs/KNOWN_LIMITATIONS.md), [release checklist](docs/RELEASE_CHECKLIST.md)

## License and provenance

Project code is under Apache-2.0; see `LICENSE`. Default-build inventory is in `licenses/THIRD_PARTY_NOTICES.md`, model metadata in `licenses/MODEL_LICENSES.json`, and optional upstream provenance under `third_party/audio_cpp/`. External distribution still requires legal/package review. No audio.cpp endorsement is implied.
