# Platform evidence matrix — 0.1.0-rc1

| Platform/profile | Configure | Build | Tests/run | Evidence label |
|---|---:|---:|---:|---|
| Linux x86_64 Release CPU + Vulkan loader probe | pass | pass | 11/11 + ABI + CLI | host-tested |
| Linux x86_64 Debug ASan+UBSan | pass | pass | 11/11 | sanitizer-tested |
| Windows x64 GNU ABI via Zig cross | pass | pass (`dll`, `exe`) | not run | build-only |
| Optional pristine audio.cpp scaffold | pass | pass | pin and wrong-pin gate | build-only scaffold; no upstream linked |
| Android arm64 CPU | not configured (no SDK/NDK) | not built | not run | source/CI scaffold only |
| Android arm64 Vulkan-probe flavor | not configured | not built | not run | source/CI scaffold only; no inference support |
| macOS universal | not available on Linux host | not built | not run | preset/CI scaffold only |
| CUDA/HIP/Metal/NNAPI/CoreML/DirectML | explicit unavailable diagnostics | not built | not run | unsupported in RC1 |

Evidence labels are not interchangeable. In particular, Windows cross-build does not prove Windows execution; Android CMake/Gradle source does not prove NDK/device support; a Vulkan loader lookup does not prove a device, graph operation, model parity, or Vulkan inference.
