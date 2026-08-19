# Android arm64 scaffold

The Android library is deliberately `arm64-v8a`, API 26+, CPU-first, C++17, OpenMP-off, and upstream-adapter-off. `cpu` is the release baseline. `vulkanProbe` only checks whether the Vulkan loader entrypoint can be loaded; it does **not** advertise Vulkan inference. Backend policy recreates/falls back to CPU only when auto-fallback is explicitly allowed.

## Build

1. Install JDK 17, Android SDK 35, NDK `27.2.12479018`, and CMake 3.30.5.
2. Set `ANDROID_HOME`/`ANDROID_SDK_ROOT`.
3. From `android/`, run `./gradlew :inbharat-audio:assembleCpuRelease` or `assembleVulkanProbeDebug`.

The wrapper JAR is not copied into this repository; generate/verify one or use Gradle 8.10.2. The current sandbox has no NDK/SDK, so Android is scaffolded but not falsely marked build-tested. See `docs/ANDROID.md` for device acceptance and lifecycle requirements.

JNI copies Kotlin arrays into bounded native vectors. It is intentionally thin: no inference runs on UI/audio callback threads, no C++ object crosses JNI, and every native exception/status becomes a Java exception. Application code owns scheduling through `AudioRuntime.executor` and must close children in session → model → runtime order.
