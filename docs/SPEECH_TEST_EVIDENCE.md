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
| Pendrive re-staged from `speech/universal-audio-hardening` tip `0caca2f`: runtime-`ibaudio` (adapter ON, drive-local model roots) + `audiocpp_cli` + 3 desktop exes; manifest re-applied with speech assets bound (3 models / 3 configs / 3 acceptance / 3 runtimes); both strict verification layers + starter `--verify-only` green | Staging transaction 2026-09-03 (`build-pocket-ai-windows.ps1` -SkipBuild; 545/545 asset checks; auto-backup in `RECOVERY/package-backups/`) | HOST (physical drive) |
| Readiness is a **runtime fact on the drive**: staged `ibaudio.exe` probes ASR + VAD weights at `D:/UNOONE/SPEECH/models/*` (drive-local roots baked at configure; VAD root now CACHE-overridable — the binary no longer depends on the build host's checkout) | `ibaudio audio-cpp-status --json` from the drive: `inference_ready:true`, `reviewed_commit` = pin `26dcb5c4…`, reason "adapter assets verified locally", 2026-09-03 | HOST (physical drive) |
| Acceptance re-generated on the re-staged drive with real inference (TTS→ASR round trip, "harbor station" sentence): 90% word recall; 6.12 s of 24 kHz TTS audio | `regen` acceptance run 2026-09-03; attestation + samples re-hashed into the manifest (DesktopLaunch sweep validates them) | HOST (physical drive) |
| Full A→Z pendrive battery with per-function speed + accuracy, all green: vault wrong-password reject (fail-closed) + real unlock `1784640669-bd5146e6`; starter verify 1.3 s; PackageIdentity 0.7 s; DesktopLaunch sweep 76.7 s; cold launch stays alive; speech readiness 0.8 s; real TTS 97.7 s → real ASR 17.2 s with **100% word recall (20/20)**; tamper of one config byte detected (94.6 s) then restored → sweep passes (92.5 s) | A→Z battery logs + results CSV, 2026-09-03 (harness: `pendrive_a2z.sh`; vault probe via `unoone_vault_core::Vault::open/unlock`, password supplied only through the `DEV_UNLOCK_PW` environment variable) | HOST (physical drive) |
| Drive re-staged 2026-09-08 from the hardening branch (UnoOnePower.exe rebuilt with the cycle's speech-gate + hash-verify fixes, SHA-256 `6F84F43F…`; Dock/starter byte-identical) + full A→Z battery re-run, all green: starter verify 0.13 s; PackageIdentity 0.38 s; DesktopLaunch sweep 12.0 s; vault wrong-password reject + real unlock (open 1.2 s); cold launch stays alive; readiness probe `inference_ready:true` at pin `26dcb5c4`; real TTS 63.6 s → 6.14 s valid WAV; real ASR 11.0 s → **100% word recall (20/20)**, transcript exact; tamper detect 53.4 s → restore → sweep passes 13.1 s | Staging transaction 2026-09-08 (`build-pocket-ai-windows.ps1` -SkipBuild, 545/545 asset checks, starter `--verify-only` `{"valid": true}`) + fresh battery CSV superseding the 2026-09-03 rows (the single transient FAIL row — `vault unlock OPEN_FAIL` 0.34 s immediately after the 76 s full-sweep hash, passed on retry 2 s later — is superseded by the clean 2026-09-08 PASS) | HOST (physical drive) |
| MSVC adapter layout corrected and verified against two real builds: the VS multi-config generator emits top-level targets at `<build>/<CONFIG>/<name>.lib` but subdirectory targets (ggml, sentencepiece) at `<build>/<subdir>/<CONFIG>/<name>.lib` — the vendor CMakeLists' earlier `<build>/<CONFIG>/<subdir>/<name>.lib` paths matched no real VS build tree | Cross-checked against the 2026-08-31 build log and a fresh 2026-09-08 configure+build of the pinned clone (exit 0); fixed in `vendor/Inbharat-audiocpp/CMakeLists.txt` | HOST |

Not claimed on Windows: ASan/UBSan for the Rust desktop crates (MSVC toolchain
has no Rust sanitizer lane; the C++ vendor side runs its own sanitizer
support on other toolchains). Any sanitizer evidence for Windows is absent,
not pending.

## Android (EMULATOR instrumented evidence 2026-09-07 + BUILD-ONLY + HOST)

Instrumented execution evidence from 2026-09-07 on the host emulator
(`emulator-5554`, AVD "Medium Phone", API 36.1, x86_64, 6144 MB RAM). All
runs used the app assembled from the working tree, with the A9
foreground-service fix, the corrected model pins, and the vendored page-agent
asset in the APK.

| Claim | Evidence | Tier |
|---|---|---|
| Cloud-fallback refusal with missing models (STT whisper/transducer/omnilingual + TTS) | `SpeechNoCloudFallbackTest` 4/4 green after the A9 fix (2026-09-07) | EMULATOR |
| Headless agent logic on-device ART (notes CRUD, master-disable, safety pipeline, encrypted cache, safety guard, phone control, memory, secure browser policy) | Batch run 41/41 after two fixes: the `SafetyGuardHeadlessTest` instrumented copy re-pinned to the production risk table (`read_screen` = STRONG_CONFIRM, matching the JVM coverage pin), and the page-agent asset vendored (below) | EMULATOR |
| All 7 baseline language packs install, size+SHA-verify, and report `installed/healthy/verified` — with the re-pinned upstream TTS bytes | `LanguagePackInstallTest` 1/1, twice (gradle `connectedDebugAndroidTest` exit 0 at 15:29; direct `am instrument` exit 0 at 16:03). Root cause fixed first: upstream `willwade/mms-tts-multilingual-models-onnx` re-uploaded the 5 Indic `model.onnx` files (+76 bytes each) after the manifest was pinned; new pins taken from the verified content hash (X-Linked-ETag, cross-checked by a real 114 MB host download + SHA-256), and `gemma-4-e4b`'s rounded `sizeBytes` corrected to the true 3,659,530,240 | EMULATOR |
| Assamese (`as-IN`) refuses to install — planned, not silently active | `as-IN-standard` → `Failure: "Assamese is listed as planned; no qualified downloadable models are configured"`; state stays not-installed (asserted in the same test) | EMULATOR |
| Re-pinned TTS bytes actually synthesize (model load → ONNX inference → non-empty PCM) and ASR engines load | `SpeechEngineFunctionalTest` 1/1: TTS en PCM (26.4 s), TTS hi PCM on the new bytes (13.0 s), English transducer init Success, omnilingual init Success — `ttsFailures=0 sttFailures=0` | EMULATOR |
| Synthesized Indic speech recognized by the omnilingual ASR | `IndicSpeechRoundTripTest` 1/1: hi round trip, 41 chars | EMULATOR |
| Pack uninstall retains the shared ASR while dependents exist; repair reinstalls | `LanguagePackRepairRetainTest` 1/1 (5 m 06 s, includes a real repair re-download) | EMULATOR |
| Page-agent runtime asset packaged and byte-authentic (196,197 bytes, SHA-256 `d798e06e…`), guarded form-fill in the packaged WebView, local-page read, DOCX + PDF-AcroForm round trips with originals unchanged | `SecureBrowserPolicyHeadlessTest` 4/4 (incl. the previously failing asset gate), `SecureBrowserPageAgentFormDeviceTest` 1/1, `SecureBrowserReadPageDeviceTest` 1/1, `DocumentFillEngineDeviceTest` 2/2 — one `am instrument` batch, 13 tests / 0 failed | EMULATOR |
| Microphone FGS fail-closed (targetSDK 35 contract) | A9 fix: `VoiceService.start` refuses without RECORD_AUDIO; `startForeground` failure is caught, logged, and `stopSelf()` — the mic-revoked cold-start crash loop is gone; verified by the green re-runs above (the pre-fix batch crashed the instrumentation process) | EMULATOR |
| Local-brain qualification (Gemma eval/planner) | Honest `assumeTrue` skips — no `.litertlm` model on the emulator; `ModelPathDiagnosticTest` ran and dumped resolution | EMULATOR (documented skips) |
| Host JVM unit tests for all touched modules | `:app/:modelmanager/:voice/:languagepacks/:safetyguard/:safety:testDebugUnitTest` — BUILD SUCCESSFUL, 2026-09-07 | HOST (JVM) |
| `libibaudio.so` + `libibaudio_jni.so` + all RC test executables cross-compile for arm64-v8a (NDK r25.1.8937393, android-28) | `scripts/build_android_scaffold.sh --tests`, 60/60 targets, 2026-08-31; recorded in `vendor/Inbharat-audiocpp/ANDROID.md` | BUILD-ONLY |
| Same tree compiles with ASan+UBSan | `--tests --sanitizers` lane, 2026-08-31 | BUILD-ONLY |
| Kotlin language-contract mirror behaves identically to the Rust table | `:voice:testDebugUnitTest` on host, 52 tests / 0 failures (incl. `VoiceLanguageCanonicalizeTest` 12/12), 2026-08-31 | HOST (JVM unit tests) |
| Kotlin alias table == `languages.v1.json` | `scripts/check_speech_language_sync.py` (fails CI on drift; verified pass + fail paths) | CI-enforced |

No **physical-device** run has been made this cycle — every row above is
emulator-tier. The Gradle module's `ndkVersion` is property-overridable and
defaults to the BUILD-ONLY-verified r25.1.

## Linux x86_64 / macOS / Raspberry Pi ARM64

| Claim | Evidence | Tier |
|---|---|---|
| Rust desktop speech code compiles + unit tests on Linux | `ubuntu-latest` matrix lane in desktop CI (tauri webkit2gtk deps installed) | CI |
| Universal InBharat Audio library builds on Linux: all release-candidate test suites + ELF exports match the ABI v1 core manifest | `vendor-audio-linux` job in desktop CI (cmake/Ninja, adapter OFF, `check_abi.py` on `libibaudio.so`) | CI |
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
  windows-latest, ubuntu-latest, and macos-latest; `speech-language-sync` job
  (same sync check, catching Rust/JSON-side table edits); `vendor-audio-linux`
  job building the universal library, running all release-candidate test
  suites, and ABI-checking `libibaudio.so` against the v1 core manifest.
- ABI invariant checks now run in CI on ELF (Linux). The Windows-side export
  comparison (dumpbin vs core manifest, 80/80) remains a host verification;
  MSVC is not available on hosted runners for this project's BuildTools
  configuration.

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