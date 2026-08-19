# Windows

The release candidate supplies a CMake preset and CI-ready scripts for x64. A clean Zig `x86_64-windows-gnu` cross-build produced `libibaudio.dll` and `ibaudio.exe`; those binaries were not executed in the Linux sandbox. Windows status is therefore build-only.

The default library uses the CPU reference backend. CUDA, HIP, Vulkan inference, and DirectML are not built. The optional Vulkan loader probe does not establish device or model support. Production packaging must include only reviewed runtime dependencies and must be tested on supported Windows versions without requiring a developer toolchain.

Before a Windows runtime claim, run the C99 ABI consumer, CLI tests, malformed-input suite, concurrency/cancellation stress, repeated load/unload, install/package tests, and any selected backend/model parity suite on real Windows hardware. See `docs/PORTABILITY.md` and `reports/PLATFORM_MATRIX.md`.
