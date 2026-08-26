# Windows + Android Build Evidence — All-22 Foundation

Date: 2026-08-21. **Evidence level: configure/build-only.** Neither binary set was executed on its target OS/device in this sandbox. Do not label either platform runtime-supported from this report alone.

## Windows x64

Toolchain: Zig 0.13.0, target `x86_64-windows-gnu`, CMake Release, tests OFF (cross-compiled binaries cannot execute on the Linux host), audio.cpp adapter OFF.

```sh
ZIG_EXECUTABLE=/agent/workspace/zig-0.13.0/zig cmake -S . -B build/windows-x64-zig -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_TOOLCHAIN_FILE=cmake/toolchains/zig-windows-x86_64.cmake \
  -DIBAUDIO_BUILD_TESTS=OFF -DIBAUDIO_BUILD_CLI=ON
ZIG_EXECUTABLE=/agent/workspace/zig-0.13.0/zig cmake --build build/windows-x64-zig --parallel
```

Result: PASS. Artifacts:

| Artifact | Size | SHA-256 |
|---|---:|---|
| `libibaudio.dll` | 1.1 MB | `20011be56b7fb9d6cbd12b64c52af87707e7891dc4cde49bef7c5b98af48f90d` |
| `ibaudio.exe` | 755 KB | `96b4148d52fe21abb571bf66ee8097b7c47c14ebaca33743ea7bcb4ce80565d6` |
| `ibaudio-mcp.exe` | 769 KB | `a3b4cbc6b44388134b6494c866289100bfb4b35e40a5fb0286060766074a1b9c` |

PE inspection: x86-64 Windows DLL/console executables. Imports are Windows/API-set CRT libraries; no `libdl` dependency after the cross-platform CMake fix. Build produced Zig/libunwind header warnings from the toolchain; first-party warnings remain errors and no first-party compile failure remains.

Status: **BUILDS_NOT_RUNTIME_TESTED**. Native Windows execution, model inference, cancellation, and performance remain PENDING on the user's Windows 11 machine.

## Android arm64-v8a

Toolchain: Android NDK r27c, API 26, `arm64-v8a`, CMake Release, JNI ON, CLI/MCP/tests OFF, Vulkan probe OFF, audio.cpp adapter OFF.

```sh
cmake -S . -B build/android-arm64-cpu -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_TOOLCHAIN_FILE=<ndk>/build/cmake/android.toolchain.cmake \
  -DANDROID_ABI=arm64-v8a -DANDROID_PLATFORM=android-26 \
  -DIBAUDIO_BUILD_TESTS=OFF -DIBAUDIO_BUILD_CLI=OFF -DIBAUDIO_BUILD_MCP=OFF \
  -DIBAUDIO_BUILD_ANDROID_JNI=ON -DIBAUDIO_ENABLE_VULKAN_PROBE=OFF
cmake --build build/android-arm64-cpu --parallel
```

Result: PASS. Artifacts:

| Artifact | Unstripped | Stripped | SHA-256 (unstripped) |
|---|---:|---:|---|
| `libibaudio.so` | 6.2 MB | 1.2 MB | `6bbfc251273fbf44fd87f9932dc6b663a56616dfde5bcbd8d214bfa1eac93964` |
| `libibaudio_jni.so` | 287 KB | 43 KB | `d854bcf0e47a3f5ca83a40588b4981d75edf5061bda0d0c5121b2f890d6ebebf` |

ELF inspection: both are AArch64 shared objects. Core needs `libdl`, `liblog`, `libandroid`, `libm`, `libc`; JNI additionally needs `libibaudio.so`. No desktop-only library dependency was observed.

Status: **BUILDS_NOT_RUNTIME_TESTED**. Emulator/instrumentation and physical-device inference, lifecycle, memory, battery and thermal evidence remain PENDING.

## Portability defects found and fixed during these builds

1. `speech_mesh.cpp` had a signed-char → unsigned-char conversion rejected by the Windows warnings-as-errors lane; fixed with explicit conversion while preserving non-ASCII bytes.
2. `libdl` was linked unconditionally because `CMAKE_DL_LIBS` reflected the host during cross-compilation; CMake now links it only when `NOT WIN32`.
