# InBharat Audio — universal speech runtime

A self-contained C++17 universal speech runtime: a stable, versioned C99 ABI (`IBAUDIO_API_VERSION` 1.0) that dispatches ASR/TTS/VAD to **pluggable providers** through a capability router, with a deterministic Bharat adaptation layer, modular language packs, an MCP gateway, and a native binary streaming transport. Applications call InBharat Audio; InBharat Audio calls whatever engine is underneath — audio.cpp is one provider, not the product.

**Status:** hardened local build. The default build is dependency-free and runs deterministic reference engines (a signal analyzer, a tone synthesizer, frame-energy VAD — *not* trained speech models). An opt-in adapter build (`IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=ON`) adds **real neural inference** via pinned audio.cpp: Silero VAD (bundled weights) and Qwen3-ASR (licensed Apache-2.0, hash-verified model supplied by the caller). See "Evidence and support labels" — nothing is claimed beyond its measured evidence.

## What is implemented

**Core (always built, dependency-free):**
- One exported C99 header `include/inbharat/ibaudio.h`; **79** `ibaudio_*` symbols (core build) and SONAME major 1. An experimental build adds 15 gated placeholder symbols (94 total).
- Opaque runtime/model/session/job/stream/buffer handles; C++ exceptions contained behind an exception firewall; thread-local structured errors; versioned extensible structs.
- **Provider registry + capability router** (`src/provider.*`): providers declare evidence-backed capabilities; the router selects by task/language/streaming/privacy with a hard remote-offline gate. The router drives production model resolution; an anti-rot test asserts the remote gate.
- Reference engines as the built-in `reference` provider; all inference (sync ASR/TTS/VAD, async jobs, streaming partials/final) dispatches through the provider vtable.
- Session single-flight, cooperative cancellation, barge-in, bounded stream queues (4096-event ceiling), bounded model LRU.
- **Bharat adaptation layer** (`src/language/`): strict UTF-8 decoding and Unicode-script metadata for Devanagari, Bengali-Assamese, Gurmukhi, Gujarati, Odia, Tamil, Telugu, Kannada, Malayalam, Perso-Arabic, Ol Chiki, Meetei Mayek and Latin; Indian numeric normalization (lakh/crore/₹/percent); deterministic Devanagari↔Roman transliteration. Script is never presented as proof of language.
- **Bharat Speech Mesh** (`src/mesh/`): evidence-gated provider routing by language/task/device/privacy/true-streaming/memory/quality/confidence, close-score abstention and deterministic multi-provider output arbitration.
- **Hash-verified hot-swap pack registry** (`src/packs/`): 22 scheduled-language manifests, activation-time SHA-256 verification, path containment and bounded metadata LRU. Integrity is not misrepresented as publisher signature/authenticity.
- **Reversible personal adaptation** (`src/adaptation/`): language-scoped names, pronunciation, acronyms, domain terms and transcript corrections with fingerprints and per-patch rollback—no continuous neural-weight mutation.
- **Stage-gated speech-to-speech orchestration** (`src/pipeline/`): VAD → STT → optional translation → TTS with stage confidence/latency, abstention, cancellation and no silent missing-stage fallback.
- **Native binary PCM transport** (`src/transport/`): bounds-checked frame codec for cross-process audio (no JSON/base64).
- **`ibaudio` CLI** (`info/models/asr/tts/vad/benchmark/diagnostics`) and **`ibaudio-mcp`** MCP gateway (dual-era, control-only, no PCM over MCP).
- **Build profiles** (`IBAUDIO_PROFILE=core|minimal|india|desktop|pi|android|full`) and **all 22 scheduled-language packs** under `packs/`. Packs start PENDING and become VERIFIED only with same-language/task/device report+hash evidence.

**Experimental (gated, `IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=ON`):** `neural_codec`, `prosody_controller`, `voice_clone_engine` are placeholders whose names overclaim their algorithms; they are excluded from the stable ABI by default. See `docs/audit/03_CURRENT_PRODUCTION_VS_RESEARCH.md`.

**audio.cpp provider (opt-in adapter build):** wraps pinned audio.cpp (release-0.6.1, `26dcb5c4`, upstream `github.com/0xShug0/audio.cpp`). Pristine-pin and cleanliness enforced at configure time. VAD is real (bundled Silero); ASR is real with a licensed, hash-verified model; TTS is gated (no licensed TTS model vendored yet). Upstream STL/exceptions never cross the C ABI.

## Build and test

```sh
./scripts/configure_build_test.sh        # clean release build + tests + ABI gate
./scripts/run_sanitizers.sh              # ASan+UBSan (where sanitizer runtime is available)
./scripts/run_benchmarks.sh
```

Manual:

```sh
cmake -S . -B build/release -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build build/release --parallel
ctest --test-dir build/release --output-on-failure
```

**audio.cpp adapter build** (real neural VAD/ASR):

```sh
# 1. Build the pinned audio.cpp (PIC + OpenMP; composite model set)
cmake -S <audio.cpp-0.6.1> -B <audio.cpp-0.6.1>/build -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_POSITION_INDEPENDENT_CODE=ON -DAUDIOCPP_MODEL_SET=custom -DAUDIOCPP_MODELS=qwen3_asr \
  -DAUDIOCPP_DEPLOYMENT_BUILD=ON
cmake --build <audio.cpp-0.6.1>/build --parallel

# 2. Build InBharat against it (pristine pin is enforced)
cmake -S . -B build/adapter -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DIBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=ON \
  -DIBAUDIO_AUDIO_CPP_SOURCE_DIR=<audio.cpp-0.6.1> \
  -DIBAUDIO_AUDIO_CPP_BUILD_DIR=<audio.cpp-0.6.1>/build \
  -DIBAUDIO_AUDIO_CPP_QWEN3_ASR_ROOT=<licensed-qwen3-asr-model-dir>
cmake --build build/adapter --parallel
```

The ASR model is **not** downloaded at runtime: supply a licensed model directory whose `model.safetensors` SHA-256 has been verified against the publisher's object id (e.g. Qwen/Qwen3-ASR-0.6B, Apache-2.0). Without it the ASR provider reports UNAVAILABLE — never a fake.

## CLI examples

```sh
build/linux-release/ibaudio models --json
build/linux-release/ibaudio asr --input tests/fixtures/speech_440hz_16k_mono.wav --stream --json
build/linux-release/ibaudio vad --input tests/fixtures/speech_440hz_16k_mono.wav --json
# adapter build, real neural:
build/adapter/ibaudio vad --model audiocpp-silero-vad-v1 --input in.wav --stream --json
build/adapter/ibaudio asr --model audiocpp-qwen3-asr-v1 --input speech.wav --json
# MCP control plane:
echo '{"jsonrpc":"2.0","id":1,"method":"server/discover"}' | build/linux-release/ibaudio-mcp
```

## Evidence and support labels

`reports/` records exact commands and results. Evidence levels are distinct: source scaffold, configure/build-only, sanitizer-tested, host-tested, emulator-tested, physical-device-tested. Currently: **host-tested** on Linux x86_64 (release 24/24 + ABI 79/79, ASan/UBSan 24/24; neural VAD and Qwen3-ASR verified through the C ABI for the tested languages). Windows x64 and Android arm64 both cross-build successfully, but remain **BUILDS_NOT_RUNTIME_TESTED** until execution on the user's Windows machine and a physical Android device. Vulkan is a loader probe, not inference support. A cross-build is never a "supported" claim.

**All-22 status:** the catalog, scripts, routing, integrity and test infrastructure cover all 22 Scheduled Indian languages; neural STT/TTS quality does not yet. IndicConformer is the primary permissive all-22 STT candidate, but same-dataset per-language evidence and Android runtime validation remain PENDING. No single permissive TTS model has adequate verified all-22 quality; the candidate portfolio and gaps are documented in `docs/ALL_22_LANGUAGE_STRATEGY.md`. "All 22 in one release" means every language is tested and reported as VERIFIED/FAILED/PENDING—not that candidate coverage is relabeled support.

## Documentation map

- [Architecture](docs/ARCHITECTURE.md), [C ABI](docs/C_API.md), [Provider API](docs/PROVIDER_API.md), [MCP gateway](docs/MCP.md), [native transport](docs/NATIVE_TRANSPORT.md)
- [All-22 strategy and model evidence](docs/ALL_22_LANGUAGE_STRATEGY.md), [language packs](docs/LANGUAGE_PACK_SPEC.md), [build profiles](docs/BUILD_PROFILES.md), [India benchmark spec](docs/INDIA_BENCHMARK.md), [benchmark report](reports/BENCHMARK_REPORT.md), [Windows/Android build evidence](reports/ALL22_PLATFORM_BUILD_EVIDENCE.md)
- [Security](docs/SECURITY.md), [model provenance](docs/MODEL_PROVENANCE.md), [validation matrix](docs/VALIDATION_MATRIX.md), [independent all-22 foundation verification](reports/ALL22_INDEPENDENT_VERIFICATION.md), [known limitations](docs/KNOWN_LIMITATIONS.md)
- Audits: [current architecture](docs/audit/01_CURRENT_AUDIO_ARCHITECTURE.md), [feature matrix](docs/audit/02_CURRENT_FEATURE_MATRIX.md), [production vs research](docs/audit/03_CURRENT_PRODUCTION_VS_RESEARCH.md), [upstream delta](docs/audit/04_AUDIOCPP_UPSTREAM_DELTA.md), [pin-move review](docs/audit/05_PIN_MOVE_REVIEW.md)
- [Verify this build yourself](VERIFICATION.md)

## License and provenance

Project code is Apache-2.0 (see `LICENSE`). Default-build inventory in `licenses/THIRD_PARTY_NOTICES.md`, model metadata in `licenses/MODEL_LICENSES.json`, upstream provenance under `third_party/audio_cpp/`. The optional audio.cpp provider links upstream (Apache-2.0) and its dependencies (ggml, sentencepiece, cJSON, libyaml) only in the adapter build; the Qwen3-ASR model is Apache-2.0 from its publisher. External distribution requires legal/package review and the user's explicit approval. No audio.cpp endorsement is implied.
