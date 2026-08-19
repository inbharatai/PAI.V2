# Build evidence — 0.1.0-rc1

**Captured:** 2026-08-17 UTC  
**Host:** Linux 6.18.40 x86_64, Intel Xeon @ 2.90 GHz (sandbox environment record)  
**Toolchain:** Zig 0.14.1 / Clang 19.1.7 target `x86_64-unknown-linux-musl`, CMake 3.30.5, Ninja 1.12.1.

## Linux Release — clean build: PASS

Configured with explicit user-space paths, `Release`, CLI/tests/Vulkan-loader-probe on, warnings-as-errors on, audio.cpp adapter off. The build directory was cleaned before evidence capture. `reports/linux-release-build.log` records 15/15 compilation/link steps, producing:

- `libibaudio.so.0.1.0` and SONAME symlinks `libibaudio.so.1`, `libibaudio.so`;
- `ibaudio` CLI;
- C++ test executable and C99 ABI smoke executable.

No `-march=native`, OpenMP, ggml, upstream/audio.cpp, or audio.cpp source path occurs in the default compile database. ELF DT_NEEDED (parsed by `scripts/elf_needed.py`) is:

- library: `libm.so.6`, `libc.so.6`, `ld-linux-x86-64.so.2`;
- CLI: `libibaudio.so.1`, `libc.so.6`, `ld-linux-x86-64.so.2`.

No accidental OpenMP or upstream shared dependency appears.

## Windows x64 — clean cross-build: PASS (build-only)

`windows-x64-zig` used Zig target `x86_64-windows-gnu`. `reports/windows-x64-zig-build.log` records 10/10 steps and generated `libibaudio.dll` plus `ibaudio.exe`. Binaries were not executed in this Linux sandbox; status is **build-only**, not Windows runtime-tested.

## Optional audio.cpp scaffold — clean build: PASS

Configured adapter ON against pristine `bb15edd78b56e035967e0eb999a6b28a62337db4`. The build compiled only `src/adapters/audio_cpp/audio_cpp_adapter.cpp` in addition to first-party runtime sources; no upstream header/source was compiled or linked. A deliberately wrong pin was rejected during configure (`audio-cpp-wrong-pin-rejection.log`, `EXPECTED_REJECTION_CONFIRMED`).

## Install/consumer: PASS

`cmake --install` produced library, header, targets, config, and version files. A fresh C99 consumer used `find_package(InBharatAudio CONFIG REQUIRED)`, linked `InBharat::ibaudio`, ran, and verified `ibaudio_get_api_version()`.

## Artifact hashes and sizes

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| Linux `libibaudio.so.0.1.0` | 9,678,984 | `8d96a2cb940fd3ca91c54b22c1f061ee3f6bc3342347a2a73c668c4b4d74588b` |
| Linux `ibaudio` | 5,485,064 | `44f16ad27a78a171d7cb38eecf0f5b93821690abbe5f47bf6ebec9cc54106d9a` |
| Windows `libibaudio.dll` | 1,020,416 | `b14f24752553bb5cc0d9139d0a24ab11789012d8ed864bdb06ae8174528f9f68` |
| Windows `ibaudio.exe` | 841,728 | `9ed4195452bda3f7397bddf2798e75cbd4d8500a8bbfba3276692a0566600774` |

Raw commands/results: `linux-release-{configure,clean,build}.log`, `windows-x64-zig-{configure,clean,build}.log`, `audio-cpp-adapter-*.log`, `linux-install.log`, and `install-consumer-*.log`.
