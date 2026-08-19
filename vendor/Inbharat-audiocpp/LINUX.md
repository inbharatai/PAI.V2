# Linux

Linux x86_64 CPU is the host-tested platform for 0.1.0-rc2. Clean Release, ASan+UBSan, and static ThreadSanitizer builds pass all 11 CTest lanes, the 58-symbol ABI manifest, CLI coverage, fixture reproducibility, install/export, and a fresh C99 consumer.

The build requires CMake 3.20+, Ninja or another supported generator, and a C++17/C99 compiler. The validated sandbox used CMake 3.30.5, Ninja 1.12.1, and Zig/Clang targeting x86_64 Linux. No OpenMP, native-ISA flag, ggml, Node, Python runtime, or upstream audio.cpp source is required by the default library.

Linux ARM64, CUDA, HIP, and Vulkan inference remain unvalidated. Vulkan support in this RC is loader probing only. Exact commands and evidence are in `BUILDING.md` and `reports/`.
