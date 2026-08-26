# Linux ARM64 / Raspberry Pi

## Status: CROSS-BUILD PLANNED — runtime unvalidated (PENDING)

The `pi` build profile produces the lean runtime (reference provider, Bharat adaptation layer, no Vulkan probe, CPU-first). There is **no ARM64 runtime evidence** in this tree: no cross-build run, no Pi execution. Per the evidence rule, ARM64 is not claimed until speech actually runs on the hardware.

## Approach

- Same C ABI and provider architecture as every other platform — no `#ifdef RASPBERRY_PI` spread through the core. Platform specifics live under `platform/linux-arm64/`.
- **CPU first.** Optional Vulkan only after real Pi measurements; a loader probe is never an inference claim.
- Cross-compilation (e.g. the Zig aarch64 toolchain under `cmake/toolchains/`) is recorded as **build-only** evidence, never as device validation.

## What a release needs before any ARM64/Pi claim

1. aarch64 cross-compile of `libibaudio.so` (build-only evidence).
2. On-device run on real ARM64 hardware (Pi 4/5 or similar): ASR/TTS/VAD smoke, streaming, cancellation.
3. Memory and thermal profile on-device.

## Evidence template

Record into `reports/`: SoC/board, OS, kernel, compiler/toolchain, build command, on-device test output, peak RSS, and thermal behavior. Mark ARM64 `physical-device-tested` only with that record. Until then every ARM64/Pi row in the validation matrix is PENDING.
