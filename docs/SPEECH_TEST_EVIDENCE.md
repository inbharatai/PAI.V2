# Speech Test Evidence — per toolchain

Companion to `docs/SPEECH_ARCHITECTURE.md`. Every row below records the
**evidence tier** for a speech-related claim, per the standing rule: *do not
claim support for a platform unless it was actually executed on that
platform*. Tiers:

- **SOURCE** — written/committed, never executed anywhere.
- **BUILD-ONLY** — compiled for the target, never executed.
- **CI** — executed on a hosted runner.
- **EMULATOR** — executed on an emulated device.
- **HOST** — executed on the working machine (Windows 11 x64, MSVC).
- **PHYSICAL** — executed on the named physical device (date + log).

## Windows desktop (HOST — the only PHYSICAL product plane)

| Claim | Evidence | Tier |
|---|---|---|
| InBharat Audio runtime builds + 26/26 ctest suites green on MSVC | `vendor/Inbharat-audiocpp` ctest run 2026-08-31 (unit, streaming, lifecycle, cancellation, malformed, stress, fuzz-gate, language-pack validator, audio-cpp status, sherpa seam) | HOST |
| `ibaudio.dll` exports match the reviewed ABI core manifest (80/80) | dumpbin /EXPORTS vs `abi/ibaudio_symbols_v1_core.txt`, byte-exact set comparison 2026-08-31 | HOST |
| Readiness is runtime-probed (compiled adapter + missing models ⇒ NOT ready) | `ibaudio_audio_cpp_status_tests` — `compiled_adapter_with_missing_models_is_not_ready` | HOST |
| Assamese routes to IndicConformer seam, never Qwen3; missing assets fail closed; remote rejected offline | `ibaudio_sherpa_provider_tests` (StubQwen3/StubRemote providers) | HOST |
| Language contract (as→as-IN, hi→hi-IN, hinglish→codemix, fr passthrough, en-US idempotent, empty/malformed rejected) | `speech-contracts` Rust unit tests + `bharat_audio` tests in the workspace suite (262 green, 2026-08-31) | HOST |
| Desktop production gate requires runtime `inference_ready`; commit mismatch rejected; non-compiled adapter never passes | `unoone-power` `bharat_audio` tests (86 green, 2026-08-31) | HOST |
| Speech manifest integrity: swapped speech model / edited acceptance attestation detected by the DesktopLaunch sweep; PackageIdentity stays fast | `unoone-usb-manifest` 13/13 + end-to-end generator→validator fixture run 2026-08-31 | HOST |
| Real ASR + TTS inference on the physical pendrive | `SPEECH/acceptance/audio-cpp.acceptance.v1.json` attested 2026-08-27, all four SHA-256s re-verified against the CLIs and models on the drive | HOST (physical drive) |

Not claimed on Windows: ASan/UBSan for the Rust desktop crates (MSVC toolchain
has no Rust sanitizer lane; the C++ vendor side runs its own sanitizer
support on other toolchains). Any sanitizer evidence for Windows is absent,
not pending.

## Android (BUILD-ONLY + HOST unit tests — no device claims)

| Claim | Evidence | Tier |
|---|---|---|
| `libibaudio.so` + `libibaudio_jni.so` + all RC test executables cross-compile for arm64-v8a (NDK r25.1.8937393, android-28) | `scripts/build_android_scaffold.sh --tests`, 60/60 targets, 2026-08-31; recorded in `vendor/Inbharat-audiocpp/ANDROID.md` | BUILD-ONLY |
| Same tree compiles with ASan+UBSan | `--tests --sanitizers` lane, 2026-08-31 | BUILD-ONLY |
| Kotlin language-contract mirror behaves identically to the Rust table | `:voice:testDebugUnitTest` on host, 52 tests / 0 failures (incl. `VoiceLanguageCanonicalizeTest` 12/12), 2026-08-31 | HOST (JVM unit tests; no device) |
| Kotlin alias table == `languages.v1.json` | `scripts/check_speech_language_sync.py` (fails CI on drift; verified pass + fail paths) | CI-enforced |

Android device gates (the eight in `ANDROID.md`) are **unset**: no emulator
run, no physical run, no instrumented execution this cycle. The Gradle
module's `ndkVersion` is property-overridable and defaults to the
BUILD-ONLY-verified r25.1.

## Linux x86_64 / macOS / Raspberry Pi ARM64

| Claim | Evidence | Tier |
|---|---|---|
| Rust desktop speech code compiles on Linux | planned `ubuntu-latest` lane in desktop CI | CI |
| macOS desktop speech code compiles + unit tests | existing `macos-latest` lane in desktop CI | CI |
| Raspberry Pi speech runtime | `distribution/pocket-ai-pi/` scripts (network-default-deny, real-artifacts-only) | SOURCE |

No Linux, macOS, or RPi speech **execution** has ever been claimed or
recorded by this project. RPi support in docs means "the scripts exist and
are fail-closed", nothing more.

## CI coverage of the speech contract

- **mobile-protection.yml** — protected Android tree hash + per-file SHA-256
  manifest (`scripts/MOBILE_PROTECTED_TREE`, `scripts/MOBILE_GOLDEN_HASHES.txt`).
- **android-ci.yml** — runs `scripts/check_speech_language_sync.py` as a hard
  gate (Kotlin table == Rust/JSON table) and the voice unit tests.
- **desktop-ci.yml** — Rust speech-contract + desktop gate tests on
  windows-latest and macos-latest.
- ABI invariant checks (export list == manifest) run wherever a matching
  toolchain exists; on Windows this was a host dumpbin comparison, not CI.

## Known limitations (do not silently "fix" in docs)

1. IndicConformer native runtime is **not linked**; Assamese ASR is
   unavailable end-to-end.
2. The desktop route is CLI-subprocess, buffered-final; no streaming claims.
3. The vendored Android JNI scaffold is not wired into the UnoOne app.
4. The `audio-cpp` acceptance attestation is generated on-device and its
   trust comes from the package manifest HMAC + the manifest hash of the
   acceptance file; there is no signature over it beyond that.
5. ASan/UBSan for Windows-hosted Rust code is unavailable by toolchain, not
   by omission.