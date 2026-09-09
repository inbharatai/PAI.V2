# Android arm64

## Profile

The first profile is `arm64-v8a`, min API 26, C++17, CPU-only correctness, no OpenMP, no host-native ISA, and no upstream adapter. Gradle builds `libibaudio.so` plus thin `libibaudio_jni.so` using one `libc++_shared`. Kotlin owners retain parents and expose an executor; JNI only translates bounded arrays/strings/handles and error statuses. RC2 converts Java UTF-16 to standard UTF-8 explicitly, converts native UTF-8 back through UTF-16 rather than modified UTF-8, bounds PCM/result allocations, rejects negative durations, and contains native allocation failures as Java exceptions.

`vulkanProbe` adds loader/symbol detection. A successful `vkGetInstanceProcAddr` lookup still reports `ADAPTER_UNAVAILABLE`; it is not a device/graph/model support claim. A requested probe runtime safely falls back to CPU because Kotlin opts into fallback.

## Storage and lifecycle

Use app-private regular files for future approved models and supply `allowedModelRoot`. Do not pass compressed APK paths or content URIs. Close Session → Model → Runtime. Close rejects live children rather than leaking. Run inference on the executor, never main/AudioRecord/AAudio callback threads. On memory pressure/background, cancel/join work then unload children deterministically.

## Required device gates before support label

1. arm64 clean configure/link and dependency audit (`readelf -d` from NDK);
2. JNI exceptions/status mapping and process-death/recreation;
3. synthetic WAV/ASR/TTS/VAD smoke, stream chunking, malformed input;
4. 10,000 lifecycle/cancel cycles under instrumentation;
5. foreground/background, storage loss, cache eviction, memory pressure;
6. PSS/mapped/backend bytes, load/first/steady latency, thermals;
7. at least one physical low/mid/high device class;
8. Vulkan flavor only after actual model operation/parity/fallback tests.

The current sandbox has no Android SDK/NDK, emulator, or device. Therefore Android status is **source scaffold, not built here**. CI defines a build-only arm64 lane; its success still does not change device evidence.

## BUILD-ONLY verification status (2026-08-31, Windows host)

The scaffold IS cross-compiled on a Windows x64 host with NDK r25.1.8937393 (arm64-v8a, android-28): `libibaudio.so`, `libibaudio_jni.so`, and every release-candidate test executable (including the readiness-probe and sherpa seam tests) compile and link cleanly — once plain and once with ASan+UBSan — via `scripts/build_android_scaffold.sh`. This is **BUILD-ONLY evidence**: nothing executed on a device or emulator, and no physical-device claim is made. The Gradle module's `ndkVersion` is property-overridable (`-Pibaudio.ndkVersion=…`) and defaults to r25.1.8937393 so the verified toolchain is what builds out of the box. The eight device gates above remain unchanged and unset.
