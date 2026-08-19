# Local release-candidate status

## Disposition

**Accepted as a first local native runtime RC (`0.1.0-rc1`) within the documented scope.** Linux Release and ASan+UBSan suites pass; C ABI/export/install gates pass; Windows x64 cross-build passes; optional pristine-pin adapter scaffold gates pass.

## Complete in this RC

C99 ABI, errors/capabilities/diagnostics, CPU BackendManager and explicit accelerators, model governance metadata, reference ASR/TTS, energy VAD, deferred KWS, PCM/WAV utilities, streaming partial/final labels, jobs/cancellation/barge-in, lifecycle/threading/metrics/cache/path policy, CLI/benchmarks, deterministic/malformed/stress/concurrent/lifecycle/fuzz tests, Android/JNI/Kotlin source scaffold, desktop presets/scripts/CI, licenses/provenance/docs/evidence.

## Open before production/external platform claims

- Real licensed neural ASR/TTS/KWS adapters, quality/parity and memory evidence.
- Android NDK build, dependency audit, instrumentation/emulator and physical-device lifecycle/thermal testing.
- Windows execution tests and native macOS build/tests.
- Any accelerator graph/model/device adapter; Vulkan is probe-only.
- ThreadSanitizer and longer libFuzzer campaigns.
- External distribution counsel, archive reproducibility/signing/attestation.

No remote was created; local-only traceability commits record the release checkpoints. The pristine upstream was not modified. Final independent Release and ASan+UBSan reruns passed 11/11 tests each; see `final-release-gate.log` and `final-sanitizer-gate.log`.
