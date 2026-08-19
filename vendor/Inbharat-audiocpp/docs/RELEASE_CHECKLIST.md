# RC release checklist

## Code/API

- [x] C99 ABI v1, opaque handles, fixed enums, versioned structs, export manifest.
- [x] Structured errors and exception containment.
- [x] CPU backend policy and explicit unavailable accelerator rows/fallback.
- [x] Model descriptors/hashes/licenses; KWS deferred honestly.
- [x] ASR/TTS/VAD, PCM/WAV processing, jobs/streams/barge-in, lifecycle/metrics/path policy.
- [x] CLI and versioned JSON/CSV schemas.

## Verification

- [x] Strict local Debug/Release build with user-space toolchain.
- [x] Deterministic, malformed, stress, concurrent, 10k lifecycle, 100 cancellation, fuzz-style tests.
- [x] C99 ABI smoke and dynamic-symbol comparison.
- [x] ASan/UBSan clean build and 11/11 pass with leak detection.
- [ ] ThreadSanitizer/fuzzer campaign beyond deterministic harness.
- [x] Windows x64 cross-build; [ ] Windows execution and native macOS CI execution.
- [ ] Android arm64 build, dependency audit, emulator, and physical device gates.

## Provenance/release

- [x] Default build contains no upstream/model assets.
- [x] Pristine audio.cpp pin and empty patch/provenance ledger.
- [x] Apache text, notices, model-license registry, SPDX inventory.
- [x] Exact local evidence reports and known limitations.
- [ ] Counsel review before external distribution.
- [ ] Release archive/reproducibility/signing/SBOM attestation.

A local RC may be declared only with open boxes explicitly reported; it must not be relabeled production or Android/Vulkan supported.
