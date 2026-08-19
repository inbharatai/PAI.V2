# Portability and support evidence

Core code is C99/C++17 with standard threading, atomics, filesystem, and optional dynamic loader use. No OpenMP, host-native ISA, POSIX process, server, network, or upstream inference dependency is required. Windows uses Win32 only inside the optional Vulkan loader probe; Unix-like probes use `dlopen`.

Linux x86_64 is host-tested (Release and ASan+UBSan). Windows x64 cross-compiles with Zig and is build-only. macOS has a native universal preset but no local build. Android is arm64/API26 CPU-first source scaffolding and has no local NDK/device evidence. See `reports/PLATFORM_MATRIX.md` for authoritative labels.

Endianness-sensitive WAV/SHA code uses explicit bytes. Public ABI fields use fixed widths. Interleaved float PCM assumes IEEE-754 float32, asserted in implementation. Platform bindings must keep one C++ runtime policy and must never expose STL layouts.
