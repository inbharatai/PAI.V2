# 02 — Current Feature Matrix (as audited 2026-08-20)

Evidence levels (strict): **VERIFIED_WORKING** (implemented + exercised in this tree's own recorded gates), **BUILDS_NOT_RUNTIME_TESTED**, **IMPLEMENTED_NOT_TESTED**, **PARTIALLY_IMPLEMENTED**, **BLOCKED_BY_ENVIRONMENT**, **EXPERIMENTAL**, **DEFERRED**.

Audit note on symbol counts: release/sanitizer gates record 58/58 core symbols; only the separate `INNOVATION_BUILD.log` records 94/94. No single recorded gate validated the full 94-symbol innovation-inclusive build in the final release configuration. This discrepancy is carried as gap G9 and fixed in Phase 16 (one unified gate).

| Capability | Area | Implementation | Recorded evidence | Level | Gaps |
|---|---|---|---|---|---|
| C99 ABI (94 symbols, versioned structs, SONAME 1) | ABI | `ibaudio.h`, all `src/*.cpp` | ABI evidence, C99 smoke lane | VERIFIED_WORKING | G9: no unified 94-symbol final gate |
| Runtime/session lifecycle (10k cycles, LRU) | Lifecycle | `runtime.cpp`, `session.cpp` | lifecycle lane, ASan leak-clean | VERIFIED_WORKING | — |
| Reference ASR (`reference-asr-v1`) | ASR | `session.cpp`, `audio.cpp` | unit/streaming lanes, fixtures, benchmark | VERIFIED_WORKING | Signal analyzer, not speech recognition — keep the "deterministic reference" qualifier |
| Reference TTS (`reference-tts-v1`) | TTS | `session.cpp`, `audio.cpp` | unit/streaming/cancellation lanes, deterministic hash | VERIFIED_WORKING | Tone synthesizer, not a voice |
| Energy VAD (`energy-vad-v1`) | VAD | `session.cpp`, `audio.cpp` | unit/streaming, malformed regressions, benchmark | VERIFIED_WORKING | Frame-energy only; no neural VAD |
| KWS (`kws-deferred-v1`) | KWS | `session.cpp:359` stub | unit lane asserts DEFERRED | DEFERRED | Honest stub; needs licensed model + parity |
| PCM processing | Audio utils | `audio.cpp`, `facade_util.cpp` | unit + fuzz lanes, malformed fixtures | VERIFIED_WORKING | Linear resampler not bandlimited/production |
| WAV decode/encode (PCM16/24/f32) | Audio utils | `audio.cpp` | malformed lane, 50k-blob fuzz, byte-reproducibility | VERIFIED_WORKING | WAV only; no MP3/AAC/Opus/FLAC/RF64 |
| Streaming classes + honest labels | Streaming | `stream.cpp`, registry | streaming + metadata lanes | VERIFIED_WORKING | Labels honest; 4096-event ceiling in rc2 |
| Cancellation / async jobs | Jobs | `session.cpp`, `stream.cpp` | cancellation (100 cancels), concurrency lanes | VERIFIED_WORKING | Cooperative only — cannot preempt uncooperative backend |
| Barge-in state machine | Interaction | `session.cpp` | cancellation + unit lanes | VERIFIED_WORKING | Host-level-driven; no mic/playback pipeline |
| Diagnostics/metrics | Observability | `runtime.cpp` | unit + cli lanes | VERIFIED_WORKING | Process-local counters only |
| Cache/path policy (fail-closed, SHA-256 admission, 4 GiB cap) | Security | `runtime.cpp`, `sha256.cpp` | unit lane, hardening A-01/A-09 | VERIFIED_WORKING | In-memory LRU; documented residual TOCTOU |
| CLI (info/models/asr/tts/vad/benchmark/diagnostics) | Tooling | `tools/ibaudio/main.cpp` | cli lane, every command | VERIFIED_WORKING | — |
| Reference benchmarks | Performance | CLI benchmark, `benchmarks/reference_cpu.*` | reference-benchmark, final-benchmark gates | VERIFIED_WORKING | Deterministic-algorithm timings only — not neural claims |
| Backend: CPU | Backends | `runtime.cpp`, `facade_util.cpp` | every gate | VERIFIED_WORKING | Only real backend |
| Backend: Vulkan | Backends | `backend_probe.cpp` | probe-only=ON in gates | PARTIALLY_IMPLEMENTED | Loader symbol lookup only; always resolves ADAPTER_UNAVAILABLE |
| Backends: CUDA/HIP/Metal/NNAPI/CoreML/DirectML | Backends | catalog rows with NOT_BUILT reasons | docs only | DEFERRED | Zero accelerator code |
| Innovation: prosody controller | Innovation | `innovation/prosody/` | single INNOVATION_BUILD lane | IMPLEMENTED_NOT_TESTED | Write-only via ABI (compute/apply not exported); dead urgency code |
| Innovation: turn manager | Innovation | `innovation/turn/` | single lane, loose disjunctive assert | EXPERIMENTAL | Rule-based, not "semantic"; caller supplies all features |
| Innovation: conversation state | Innovation | `innovation/conversation/` | single lane, real transition asserts | EXPERIMENTAL | Closest to production; only one transition path tested |
| Innovation: environment adapter | Innovation | `innovation/environment/` | single lane, trivially-true asserts | EXPERIMENTAL | Percentile floor + amplitude gate; comments claim spectral subtraction/AEC |
| Innovation: voice clone engine | Innovation | `innovation/voice/` | single lane; test source broken (see G2) | EXPERIMENTAL | 256-bin magnitude histogram, no FFT, no synthesis path; consent gate is real |
| Innovation: code-switch detector | Innovation | `innovation/codeswitch/` | single lane, script-ratio asserts pass | EXPERIMENTAL | Byte-level UTF-8 flaw; any non-ASCII = "Devanagari"; RMS "confidence" is loudness |
| Innovation: neural codec | Innovation | `innovation/codec/` | single lane, non-null asserts only | EXPERIMENTAL | Largest description gap: claims SoundStream/RVQ, implements scalar quantize + sine synth |
| Innovation: context-aware output | Innovation | `innovation/context/` | single lane; boundary assert actually false (stale binary) | EXPERIMENTAL | apply() discards 3 of 4 computed outputs |
| audio.cpp adapter scaffold | Upstream | `adapters/audio_cpp/` | configure/build + wrong-pin-rejection logs | DEFERRED | availability()=DEFERRED; no upstream source linked |
| Android/JNI/Kotlin | Platform | `android/` | host-stub compile only; HARDENING_FINAL_JNI.log empty | BUILDS_NOT_RUNTIME_TESTED | No NDK build, emulator, or device run |
| Windows x64 build | Platform | presets | zig cross-build logs, DLL+EXE hashes | BUILDS_NOT_RUNTIME_TESTED | Never executed; no MSVC evidence |
| macOS build | Platform | preset (Darwin-conditional) | doc only | BLOCKED_BY_ENVIRONMENT | No build or test evidence |
| Linux x86_64 | Platform | presets | full release+ASan gates, install/consumer run | VERIFIED_WORKING | Only fully host-tested platform |
| ARM64 (Android/Pi) | Platform | NDK presets | never configured (no SDK/NDK) | BLOCKED_BY_ENVIRONMENT | Zero ARM64 compile/run evidence |
| Sanitizers (ASan+UBSan, static TSan) | QA | CMake option | 11/11 lanes, leak detection | VERIFIED_WORKING | Pre-date innovation sources; MSan not claimed |
| Fuzz | QA | `fuzz/wav_fuzzer.cpp`, in-process loops | deterministic loops in gates | VERIFIED_WORKING | libFuzzer harness OFF by default; no coverage-guided campaign recorded |

## Critical gaps (description exceeds evidence)

- **G1 — "Neural codec"**: claims SoundStream-style causal neural codec with RVQ at 3–18 kbps; implements linear resample + per-frame RMS scalar quantization + sine synthesis. Bitrate formula doesn't match emitted payload.
- **G2 — "Voice clone engine"**: consent/enrollment bookkeeping is real and enforced; there is no cloning/synthesis path. Test source references nonexistent `IBAUDIO_STATUS_PERMISSION_DENIED` and does not compile; the checked-in binary is stale.
- **G3 — All innovation modules**: compiled into `libibaudio.so` unconditionally (no `IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES` flag exists); exercised only in one non-gate build lane on constant synthetic buffers.
- **G4 — Reference ASR/TTS**: fully gate-tested but are a signal analyzer and tone synthesizer; external claims must keep the "deterministic reference" qualifier.
- **G5 — Android/JNI**: host-stub compile only; empty JNI evidence log; no NDK/emulator/device.
- **G6 — Vulkan**: loader probe that always resolves unavailable; CPU is the only backend.
- **G7 — Windows**: cross-built, never executed.
- **G8 — Fuzzing**: deterministic pseudo-random loops, not a coverage-guided campaign.
- **G9 — Symbol-count discrepancy**: no single gate validates the full 94-symbol manifest in the final configuration.
