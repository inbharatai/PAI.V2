# Audio RC2 platform evidence

| Platform/profile | Build | Runtime/tests | Label |
|---|---:|---:|---|
| Linux x86_64 Release shared | pass | 11/11 | host-tested |
| Linux x86_64 ASan+UBSan shared | pass | 11/11 | sanitizer-tested |
| Linux x86_64 TSan static | pass | 11/11 | race-sanitizer-tested |
| Windows x64 GNU cross | pass | not run | build-only |
| Android ARM64 JNI/Kotlin | JNI C++ strict host compile only | not run | source/compile scaffold |
| macOS/Apple Silicon | unavailable | not run | pending |
| Vulkan/CUDA/HIP/Metal inference | not implemented | not run | unsupported in RC2 |

Build-only and host-stub compilation are not runtime or device support claims.
