# Validation Matrix

Every capability, its evidence level, and where it stands. Levels: **VERIFIED_WORKING** (implemented + exercised in this tree's own gates), **BUILDS_NOT_RUNTIME_TESTED**, **IMPLEMENTED_NOT_TESTED**, **PARTIALLY_IMPLEMENTED**, **BLOCKED_BY_ENVIRONMENT**, **EXPERIMENTAL**, **DEFERRED**. Hardware-only validation is **PENDING**, never claimed.

## Core runtime (Linux x86_64 host)

| Capability | Evidence | Level |
|---|---|---|
| C99 ABI (79 core symbols, versioned, SONAME 1) | abi-c99 lane + ABI gate | VERIFIED_WORKING |
| Runtime/session lifecycle, LRU, single-flight | lifecycle/concurrency lanes | VERIFIED_WORKING |
| Reference ASR/TTS/VAD via provider registry | provider + unit + streaming lanes | VERIFIED_WORKING |
| Jobs / cooperative cancellation / barge-in | cancellation + concurrency lanes | VERIFIED_WORKING |
| Streaming (chunked, revisable partials, honest labels) | streaming + metadata lanes | VERIFIED_WORKING |
| PCM/WAV processing | unit + fuzz + malformed lanes | VERIFIED_WORKING |
| Cache/path policy (fail-closed, SHA-256 admission) | unit lane, hardening A-01/A-09 | VERIFIED_WORKING |
| CLI + benchmarks | cli + benchmark lanes | VERIFIED_WORKING |
| Bharat adaptation layer (Indian Unicode scripts, code-mix metadata, normalization, transliteration) | language lane | VERIFIED_WORKING |
| Bharat Speech Mesh evidence routing, ambiguity abstention, output arbitration | speech-mesh lane | VERIFIED_WORKING |
| 22 scheduled-language pack catalog + claim validator | language-packs lane | VERIFIED_WORKING as catalog/infrastructure; neural quality PENDING |
| Hash-verified hot-swap pack metadata registry + bounded LRU | pack-registry tamper/path/LRU lane | VERIFIED_WORKING |
| Reversible personal adaptation (names/pronunciation/domain/script) | personal-adaptation fingerprint/rollback lane | VERIFIED_WORKING |
| Stage-gated speech-to-speech orchestration | speech-to-speech success/abstain/cancel lane | VERIFIED_WORKING as orchestration; neural TTS all-22 PENDING |
| Native transport frame codec (bounds-checked) | transport lane | VERIFIED_WORKING |
| MCP gateway (control plane, no PCM) | live stdio JSON-RPC exercise | VERIFIED_WORKING |
| Sanitizers ASan+UBSan (all 24 tests) | sanitizer gate | VERIFIED_WORKING |

## Providers

| Provider | Locality | Inference status | Level |
|---|---|---|---|
| `reference` | local-native | deterministic engines, executed in gates | VERIFIED_WORKING (as reference engines, not speech recognition) |
| `audiocpp` Silero VAD | local-native adapter | real neural offline + incremental VAD through C ABI | VERIFIED_WORKING on Linux x86_64 host |
| `audiocpp-asr` Qwen3-ASR | local-native adapter | real neural ASR through C ABI; tested English; Hindi model coverage candidate | VERIFIED_WORKING only for tested host cases; not all-22 |
| `ai4bharat` IndicConformer | local-service candidate | explicit all-22 STT candidate; no Python/NeMo stack in sandbox | PENDING per-language benchmark; BLOCKED_BY_ENVIRONMENT here |
| `sarvam` | remote | spec; gated OFF; UNAVAILABLE; no network client | BLOCKED_BY_ENVIRONMENT |

## Platforms

| Platform | Evidence | Level |
|---|---|---|
| Linux x86_64 | full release + sanitizer gates, install/consumer | VERIFIED_WORKING |
| Windows x64 | Zig 0.13 cross-build: DLL + CLI + MCP PE binaries, hashes, never executed | BUILDS_NOT_RUNTIME_TESTED |
| macOS | none | BLOCKED_BY_ENVIRONMENT |
| Android arm64 | NDK r27c API-26 build: AArch64 core + JNI shared libraries, dependency/hash evidence; no emulator/device run | BUILDS_NOT_RUNTIME_TESTED — device PENDING |
| Linux ARM64 / Pi | pi profile builds on host; no ARM64 run | BUILDS_NOT_RUNTIME_TESTED — device PENDING |
| Backends: CUDA/HIP/Metal/Vulkan/NNAPI/CoreML/DirectML | catalog rows / loader probe only | PARTIALLY_IMPLEMENTED / DEFERRED |

## Gated experimental modules (IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=ON only)

| Module | Verdict | Level |
|---|---|---|
| conversation_state | FSM, name/code agree | EXPERIMENTAL (near production; needs broader transition tests) |
| turn_manager | rule-based classifier | EXPERIMENTAL (relabel from "semantic") |
| codeswitch_detector | now UTF-8-correct via language layer | EXPERIMENTAL |
| environment_adapter | energy-gate DSP | EXPERIMENTAL (comments corrected) |
| context_aware_output | rule table, volume-only apply | EXPERIMENTAL |
| prosody_controller | write-only via ABI, no output path | EXPERIMENTAL (gated) |
| voice_clone_engine | enrollment/consent registry, no synthesis | EXPERIMENTAL (gated) |
| neural_codec | scalar quantize + sine synth, no NN/RVQ | EXPERIMENTAL (gated) |

## Explicit non-claims

- Reference ASR/TTS are a signal analyzer and a tone synthesizer — **not** speech recognition or a natural voice. No WER/quality claim is made for them.
- No all-22 neural STT/TTS accuracy claim. All 22 packs and evidence gates exist; most task rows remain PENDING until same-language benchmarks run.
- No ARM64/Android device claim until physical-device inference passes.
- No accelerator inference claim; CPU is the only locally validated backend.
- No remote inference; remote providers are gated OFF and unimplemented beyond the spec seam.
