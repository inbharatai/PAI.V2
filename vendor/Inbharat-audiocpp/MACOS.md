# macOS

The source and C ABI are designed for Apple Silicon and a native macOS CMake preset is included. No macOS runner, Xcode toolchain, Metal compiler, or physical Apple Silicon host was available in the local sandbox, so macOS is not built or runtime-tested in 0.1.0-rc2.

The default CPU reference backend should be validated first. Metal remains explicitly unavailable until a model adapter, graph parity tests, startup/first-inference metrics, memory measurements, and sustained-operation tests pass on real hardware. A successful cross-compile would not be sufficient evidence.

Before a macOS support claim, run all CTest lanes, ABI/install consumers, CLI tests, malformed/fuzz/stress suites, repeated lifecycle tests, and notarized packaging checks on supported Apple Silicon versions.
