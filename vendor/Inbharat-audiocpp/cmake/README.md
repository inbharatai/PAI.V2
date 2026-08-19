# Build profiles

`CMakePresets.json` defines Linux Debug/Release/ASan+UBSan, Windows x64 (Zig cross-build), macOS universal (native macOS host), and Android arm64 CPU/Vulkan-probe profiles. The default runtime is CPU-only and does not inspect or link the upstream audio.cpp checkout.

Evidence scripts use system CMake, Ninja, and compilers by default. Set `TOOLCHAINS_ROOT` or the individual `CMAKE`, `NINJA`, `CC`, and `CXX` variables for a private toolchain. Zig cross wrappers use `zig` from `PATH` or `ZIG_EXECUTABLE`. Android presets require `ANDROID_NDK_HOME`. The Vulkan Android preset is probe-only and does not advertise Vulkan inference support.
