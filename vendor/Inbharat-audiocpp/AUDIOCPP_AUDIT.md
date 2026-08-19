# audio.cpp source architecture and Android portability audit

**Status:** accepted source audit for InBharat Audio 0.1.0-rc1  
**Audited checkout:** pinned local audio.cpp release 0.6 checkout  
**Pinned revision:** `bb15edd78b56e035967e0eb999a6b28a62337db4` (`Expose NeuTTS and SenseVoice controls in native UI (#228)`)  
**Audit mode:** source inspection; the upstream checkout was not modified.  
**Baseline evidence:** a portable Linux CPU/core configuration and `audiocpp_cli` build passed using local CMake, Ninja, and Zig/Clang. Eight selected framework tests passed; `model_spec_system_test` failed because its external `miotts` spec was not installed in the test working directory, and `audio_utility_api_test` exceeded the 120-second sandbox command window. No model-weight, Android, GPU, or full-catalog parity claim is made.

## 1. Executive conclusion

The repository is a large C++17 audio-inference framework built around a vendored ggml/GGUF stack. Its internal layering is coherent: package specs resolve model resources, loaders create loaded-model objects, loaded models create task sessions, and sessions build ggml graphs over CPU/CUDA/HIP/Vulkan/Metal backends. It already contains meaningful portability work for Linux, Windows, and macOS and some Android-aware code inherited from ggml and SentencePiece.

It is **not yet an Android library product**. There is no Android/JNI target, no stable C ABI, no Gradle project, no Android asset/file-descriptor abstraction, no AAudio/Oboe integration, no Android test lane, and no production notice bundle. A direct top-level Android build also inherits host-oriented defaults and app targets.

### Production blockers

| ID | Severity | Blocker | Exact source evidence | Required disposition |
|---|---:|---|---|---|
| A1 | Blocker | No library ABI/JNI surface; public APIs expose STL, exceptions, RTTI, `std::filesystem`, and C++ virtual classes. | `include/engine/framework/runtime/{model.h,registry.h,session.h}`; no JNI source or exported C facade in the checkout. | Add an out-of-tree/overlay shared-library target and a versioned C ABI with opaque handles; keep JNI thin. |
| A2 | Blocker | Android cross-build defaults are unsafe: native host ISA is forced on and OpenMP controls are split. | Top `CMakeLists.txt`: `ENGINE_ENABLE_NATIVE_CPU` defaults `ON` (lines 52–90) and is forced into `GGML_NATIVE` (181–185); `ENGINE_ENABLE_OPENMP` does not set `GGML_OPENMP` (168–170, 248–254). ggml ARM native probing executes target compiler/run probes in `external/ggml/src/ggml-cpu/CMakeLists.txt:121–175`. | For Android set `ENGINE_ENABLE_NATIVE_CPU=OFF`, `ENGINE_ENABLE_OPENMP=OFF`, and `GGML_OPENMP=OFF`; patch top CMake to synchronize the two OpenMP options and default native CPU off when cross-compiling/Android. |
| A3 | Blocker | Default CMake declares CLI, server, converter, perf, and parity executables unconditionally; Gradle/default-all builds can try to build host applications. | `CMakeLists.txt:1343–1479`; app build switches exist only for tests/examples/warmbench. | Add `AUDIOCPP_BUILD_RUNTIME`, `AUDIOCPP_BUILD_CLI`, `AUDIOCPP_BUILD_SERVER`, and `AUDIOCPP_BUILD_GGUF_TOOL` guards, or make the Android wrapper build only its named target. |
| A4 | Blocker | SentencePiece static Android link does not propagate `liblog`, although bundled protobuf-lite calls `__android_log_write`. | Top CMake forces `SPM_ENABLE_SHARED=OFF` (`CMakeLists.txt:202–204`). `external/sentencepiece/third_party/protobuf-lite/common.cc:52–158` uses Android logging, but `external/sentencepiece/src/CMakeLists.txt:231–234` links `log` only to shared targets; static target at 237–241 omits it. | Link final JNI target with `log` immediately; patch `sentencepiece-static` to propagate `log` under `ANDROID`. |
| A5 | Blocker | Model I/O is path-only and assumes ordinary seekable files; APK/AAB assets are not directly consumable. | `ModelLoadRequest::model_path` in `runtime/model.h`; `BinaryBlob::read_binary_blob` in `src/framework/io/binary.cpp`; `ResourceBundle` and `TensorSource` use filesystem paths throughout. | Copy models into app-private regular files before load, or add a file/fd+offset abstraction. Do not pass `AAsset` pseudo-paths. |
| A6 | Blocker | Standalone GGUF sidecars materialize through `std::filesystem::temp_directory_path()` with no caller-supplied Android cache root. | `materialize_gguf_sidecars` in `src/framework/assets/tensor_source.cpp:1661–1688`. | Add a runtime cache-directory option and atomic, locked extraction. Until patched, use external sidecars in an app-private model directory or verify the device temp path is writable. |
| A7 | Blocker for most models | Memory policy is desktop-scale. Numerous graph/context defaults are hundreds of MiB to several GiB, and a loaded model often uploads weights per session. | Examples: Parakeet `session.h:41–43` (3072/1024/256 MiB); Qwen3 TTS `session.h:68–70` (4096/1536/1536 MiB constant contexts); Higgs STT `session.h:62–65`; HeartMuLa and IndexTTS2 headers. `BackendWeightStore` allocates caller-requested descriptor context without the host-memory fitting used by `ConstantTensorCache`. | Establish Android model allowlists and memory budgets; start with small ASR/VAD/TTS families, one live session, quantized packages, and model-specific reduced arenas validated on devices. |
| A8 | Compliance blocker | There is no consolidated third-party notice package; two vendored source directories lack a local license file, and model package specs contain no license metadata. | Root `LICENSE`; `external/{ggml,cJSON,sentencepiece}/LICENSE`; no license under `external/libyaml` or `external/llama_tokenizer`; no `NOTICE*`; no `license` fields in `model_specs/*.json`. | Resolve provenance and add notices before distributing an AAR/APK or weights. Model licenses must be reviewed separately from code. |

### Vulkan status

Vulkan is a **second-stage backend, not the Android baseline**. The engine exposes it (`BackendType::Vulkan`, `init_backend`) and the vendored backend supports many audio-relevant operations, including convolution and transpose convolution. However, Android cross-configuration needs a host `glslc` and a host shader-generator build (`external/ggml/src/ggml-vulkan/CMakeLists.txt`), mobile driver limits are operation- and shape-dependent, and the engine has no `ggml_backend_sched` CPU fallback. One unsupported operation invalidates a single-backend graph. Treat Vulkan as a per-model/per-precision allowlist with CPU session recreation on failure.

## 2. Repository and target inventory

### Scale at the pinned revision

- Top CMake: 2,270 lines.
- `src`: 526 C/C++ source/header files, about 255k lines.
- `include`: 544 public headers, about 38k lines.
- `app`: 35 C/C++ files, about 10k lines.
- `tests`: 133 C/C++ source/header files plus Python fixtures/tools, about 31k C/C++ lines.
- Vendored trees: ggml, SentencePiece plus protobuf-lite/absl/darts, cJSON, libyaml, and a llama-tokenizer extraction.
- Model package specs: 47 JSON files; 23 schema-v1 and 24 legacy; all 47 describe a GGUF source and 44 also describe safetensors.

### Top-level CMake graph

`CMakeLists.txt` defines:

1. `engine_core` — an object library containing common runtime, IO, audio/DSP, tokenizer, module, codec, sampler, and graph code (`CMakeLists.txt:272–400`).
2. `engine_model_silero_vad` and `engine_model_marblenet_vad` — always linked built-in VAD object libraries (`403–418`). They remain present even with `AUDIOCPP_MODEL_SET=core`.
3. Forty-five selectable model object targets created by `audiocpp_add_model` (`257–270`, calls beginning at 420).
4. Generated registry include fragments:
   - `generated/model_registry_includes.inc`
   - `generated/model_registry_loaders.inc`
   generated from enabled models (`1218–1250`).
5. `engine_runtime` — a static library containing `registry.cpp`, `engine_core`, the two VAD objects, and enabled/required model objects (`1252–1265`).
6. `cjson_vendor` and `yaml_vendor` static libraries (`1267–1287`).
7. Vendored ggml and SentencePiece subdirectories (`210–211`).
8. Unconditional application/tool targets: `audiocpp_cli`, `audiocpp_server`, `audiocpp_gguf`, `model_perf`, `miocodec_wavlm_parity`, and `torch_bin_parity` (`1343–1468`), with several conditional parity helpers.
9. Optional warmbench, unit-test, and example targets controlled by `ENGINE_BUILD_WARMBENCH`, `ENGINE_BUILD_TESTS`, and `ENGINE_BUILD_EXAMPLES`.

There is no top-level install/export rule for `engine_runtime`, no Android shared-library target, and no ABI-versioned public library target.

### Build defaults with portability impact

- C11 and C++17, extensions off (`CMakeLists.txt:5–10`).
- Default single-config build is `RelWithDebInfo` (`42–44`). Debug on non-MSVC is overwritten to `-O3 -g` (`46–50`), so “Debug” is optimized.
- CPU native ISA: on by default.
- llamafile SGEMM: on by default.
- host OpenMP: on and required if enabled.
- ggml backends are statically linked by default because `BUILD_SHARED_LIBS` and `GGML_BACKEND_DL` are forced off (`181–185`).
- Full model set is the default.
- Model specs are runtime files by default; `AUDIOCPP_DEPLOYMENT_BUILD=ON` embeds them in the runtime. The GGUF converter always embeds its conversion catalog (`14–40`).
- Global project warnings include `-Wall -Wextra -Wpedantic -pedantic-errors` for GCC/Clang (`213–219`).

### Model-composite dependencies

`audiocpp_add_model` records loader includes/symbols and internal link dependencies. Notable closure growth:

- `glm_tts -> outetts -> qwen3_asr + qwen3_forced_aligner`.
- `qwen3_asr <-> qwen3_forced_aligner` form a deduplicated dependency cycle.
- `miotts -> miocodec + qwen3_asr`.
- `vietneu_tts -> moss` (the `moss` object target contains both local and nano loaders).

Only directly enabled loaders are added to the generated registry; dependency objects are linked for internal use but are not necessarily advertised. A custom Android build therefore needs size measurement after dependency closure, not only a count of requested names.

## 3. Runtime architecture

### End-to-end control path

```text
CLI / HTTP server / future JNI facade
             |
             v
ModelRegistry::inspect/load
             |
             v
IVoiceModelLoader -> ILoadedVoiceModel
             |
             v
ILoadedVoiceModel::create_task_session(TaskSpec, SessionOptions)
             |
             v
IOfflineVoiceTaskSession or IStreamingVoiceTaskSession
             |
             v
RuntimeSessionBase
  + ExecutionContext (one ggml backend handle)
  + ArtifactStore / RuntimeCache / RuntimeWorkspace
  + GraphExecutor
             |
             v
model frontend / tokenizer / encoder / decoder / codec / vocoder graphs
             |
             v
ggml backend buffer allocation and graph compute
```

The central interfaces are in:

- `runtime/model.h`: `ModelLoadRequest`, `IVoiceModelLoader`, `ILoadedVoiceModel`, metadata and capability contracts.
- `runtime/registry.h` and `src/framework/runtime/registry.cpp`: `ModelRegistry`, config filtering, generated default registry.
- `runtime/session.h`: task enum, audio/text/artifact data, request/result/event structures, offline and streaming interfaces.
- `runtime/session_base.h/.cpp`: per-session backend/context/cache/workspace ownership.
- `core/backend.h/.cpp`: backend discovery, initialization, thread configuration, graph compute, tensor upload/readback.
- `core/execution_context.h/.cpp`: RAII ownership of `ggml_backend_t`.

### ABI implications

These are good internal C++ interfaces but unsuitable as an Android ABI:

- STL containers, `std::string`, `std::optional`, `std::function`, `std::filesystem::path`, and `std::unique_ptr` cross every interface.
- Exceptions are the error channel.
- RTTI is required by production paths (`dynamic_cast` in model sessions, CLI, server, and workflow).
- Virtual multiple inheritance is used for sessions.
- There is no symbol visibility policy or ABI version negotiation.

Do not expose these classes directly through JNI or an AAR header contract. Build a stable C facade inside the same shared object and catch all C++ exceptions there.

## 4. ggml, GGUF, weights, and model specs

### Backend integration

`BackendType` in `core/module.h` exposes CPU, CUDA, HIP, Vulkan, Metal, and BestAvailable. `ensure_backends_loaded` calls `ggml_backend_load_all`; `init_backend` selects a registry/device by the registry names `CPU`, `CUDA`/`MUSA`, `ROCm`, `Vulkan`, or `MTL` (`core/backend.cpp`). CPU threads are set via `ggml_backend_reg_get_proc_address(..., "ggml_backend_set_n_threads")`.

The engine normally uses one backend per session. Graphs allocate all tensors on that backend and call `compute_backend_graph`. There is no use of `ggml_backend_sched`, so unsupported GPU operations do not fall back to CPU. `validate_backend_graph_supported` exists, but it runs only in non-`NDEBUG` builds when `ENGINE_VALIDATE_BACKEND_GRAPH=1`; many callers ignore the returned `ggml_status`. This is a material Vulkan validation risk.

### Tensor-source stack

`include/engine/framework/assets/tensor_source.h` defines:

- `TensorSource` and tensor metadata/data abstractions.
- native/F32/F16/BF16/I8 and Q2–Q8 storage controls.
- `open_tensor_source`, indexed safetensors, prefix/composite views.
- GGUF conversion and sidecar APIs.

`src/framework/assets/tensor_source.cpp` implements:

- `SafeTensorSource` — parses safetensors index metadata, mmaps/reads the file through `BinaryBlob`, copies requested tensor bytes, and advises mapped ranges away after upload.
- `GgufTensorSource` — reads GGUF metadata with `gguf_init_from_file`, records physical/logical names and exact shapes, then mmaps/reads the file for on-demand tensor upload.
- `IndexedTensorSource`, `CompositeTensorSource`, and `PrefixedTensorSourceView`.
- conversion to native audio.cpp GGUF, including quantization, BNB NF4 decoding, sidecars, model-spec metadata, and temp-file rename.

`BackendWeightStore` in `core/backend_weight_store.h` builds a no-allocation ggml descriptor context, queues source uploads, allocates one backend weight buffer, uploads tensors, and clears staging memory. It changes native BF16 to F16 for Vulkan/Metal and falls back to F32 where quantized row layout is not representable.

### GGUF is audio.cpp-native, not generic llama/whisper GGUF

The custom metadata includes:

- `audiocpp.tensor_names`, `audiocpp.tensor_ranks`, `audiocpp.tensor_shapes`.
- embedded sidecar arrays.
- `audiocpp.model_spec.version/family/json`.
- source namespaces and selected weight type.

`prepare_model_directory` accepts a direct GGUF or a directory containing `model.gguf` or exactly one GGUF. Embedded sidecars are extracted to a temp cache; tensor-only GGUFs use adjacent files.

The converter entry point is `app/gguf/main.cpp`; core work is `convert_tensor_sources_to_gguf`. Quantization is selective: rank/row-block constraints, lookup/codebook exclusions, and per-tensor overrides mean a blanket “Q8” label can still describe mixed types. `docs/gguf.md` records model-specific quality and compatibility caveats; Android packaging must preserve those choices.

### Package-spec resolution and registry

`model_spec/package.cpp` uses `ScopedSpecOverride` with thread-local state. Resolution is:

1. explicit model-spec override;
2. model spec embedded in selected GGUF;
3. compiled catalog when deployment build is on;
4. external discovery near the model and from the current directory upward.

`ResourceBundle` (`assets/resource_bundle.h/.cpp`) maps logical file/tensor IDs to canonical paths and caches opened tensor sources by path. Schema-v1 metadata (`model_spec/metadata.cpp`) derives task capabilities and allowed request/session/load options. `SpecBackedVoiceModelLoader` (`runtime/spec_backed_model.h`) centralizes schema-v1 loaders; legacy models retain custom loaders.

At this revision 23 of 47 specs are schema-v1 and 24 are legacy, so behavior still differs between spec-backed and custom loaders. Exact default registry construction is `make_default_registry` in `registry.cpp`: Silero and MarbleNet are explicit, followed by CMake-generated loader calls.

### Loader symbol inventory

The selectable composites contribute these loader factories (49 factories including aliases/multi-family targets):

- Source separation: `make_mel_band_roformer_loader`, `make_bs_roformer_loader`, `make_htdemucs_loader`.
- VAD built-ins: `make_silero_vad_loader`, `make_marblenet_vad_loader`.
- ASR/alignment/diarization: `make_citrinet_asr_loader`, `make_fun_asr_nano_loader`, `make_higgs_audio_stt_loader`, `make_hviske_asr_loader`, `make_kroko_asr_loader`, `make_nemotron_asr_loader`, `make_parakeet_tdt_loader`, `make_qwen3_asr_loader`, `make_qwen3_forced_aligner_loader`, `make_sense_asr_loader`, `make_sortformer_diar_loader`, `make_vibevoice_asr_loader`, `make_voxtral_realtime_loader`.
- TTS/generation/conversion families: `make_ace_step_loader`, `make_chatterbox_loader`, `make_confucius4_tts_loader`, `make_dots_tts_loader`, `make_dramabox_loader`, `make_fish_audio_loader`, `make_glm_tts_loader`, `make_heartmula_loader`, `make_higgs_audio_tts_loader`, `make_index_tts2_loader`, `make_inflect_v2_loader`, `make_irodori_tts_loader`, `make_minimax_h3_loader`, `make_miocodec_loader`, `make_miotts_loader`, `make_moss_tts_local_loader`, `make_moss_tts_nano_loader`, `make_muscriptor_loader`, `make_neutts_loader`, `make_omnivoice_loader`, `make_outetts_loader`, `make_pocket_tts_loader`, `make_qwen3_tts_loader`, `make_rvc_loader`, `make_seed_vc_loader`, `make_stable_audio_loader`, `make_supertonic_loader`, `make_vevo2_loader`, `make_vibevoice_loader`, `make_vietneu_tts_loader`, `make_voxcpm2_loader`.

Definitions live in each family’s `session.cpp` or `loader.cpp`; their public declarations live under matching `include/engine/models` or `include/engine/community_models` paths.

## 5. ASR, TTS, VAD, codecs, preprocessing, buffers, and streaming

### Task/family coverage from package specs

| Capability | Families in `model_specs` |
|---|---|
| ASR | `citrinet_asr`, `fun_asr_nano`, `higgs_audio_stt`, `hviske_asr`, `kroko_asr`, `nemotron_asr`, `parakeet_tdt`, `qwen3_asr`, `sense_asr`, `vibevoice_asr`, `voxtral_realtime` |
| TTS | `chatterbox`, `dots_tts`, `dramabox`, `fish_audio`, `glm_tts`, `higgs_audio_tts`, `index_tts2`, `inflect_v2`, `irodori_tts`, `minimax_h3`, `miotts`, `moss_tts_local`, `moss_tts_nano`, `neutts`, `omnivoice`, `outetts`, `pocket_tts`, `qwen3_tts`, `supertonic`, `vevo2`, `vibevoice`, `vietneu_tts`, `voxcpm2` |
| Voice cloning | 18 of the above TTS families; declared in their specs. |
| Voice conversion/S2S/SVC | `chatterbox`, `miocodec`, `rvc`, `seed_vc`, `vevo2` |
| Music/SFX/edit | `ace_step`, `heartmula`, `minimax_h3`, `stable_audio`, `vevo2` |
| Separation | `bs_roformer`, `htdemucs`, `mel_band_roformer` |
| Forced alignment | `qwen3_forced_aligner` |
| Diarization | `sortformer_diar` |
| MIDI | `muscriptor` |

The package spec for legacy `miocodec` also uses a metadata task string `codec`; the runtime enum has no generic Codec task and its custom loader actually advertises VoiceConversion and SpeechToSpeech (`src/models/miocodec/loader.cpp`). Do not derive the Android API solely from legacy JSON task strings; use loader advertisements/runtime enums.

### VAD

- **Silero VAD** is a first-class offline and streaming session. `SileroRuntime` keeps LSTM hidden/cell/context state and runs 512-sample chunks at 16 kHz (`models/silero_vad/runtime.h/.cpp`). It is the best initial live-input reference path.
- **MarbleNet VAD** is offline-only (`models/marblenet_vad/session.h`) and computes frontend features plus a classification graph (`runtime.h`).
- `audio/chunking.h/.cpp` can turn VAD segments into bounded, padded, merged inference chunks and merge timestamp metadata.

### ASR architecture

The common pattern is:

1. input validation/mixdown/resampling;
2. Whisper/Kaldi/model-specific frontend;
3. speech/audio encoder;
4. autoregressive, CTC, RNN-T/TDT, or model-specific decoder;
5. tokenizer/postprocessor and optional timestamps;
6. long-audio fixed/VAD/windowed chunk merge.

Reusable pieces include `modules/asr_helpers`, Whisper frontend/embedding, SANM, Zipformer, TDT decoder core/runner, Qwen decoder runtime, and timestamp merge functions.

Streaming semantics are not uniform:

- Clearly stateful low-latency paths include Silero VAD, Voxtral realtime (`VoxtralRealtimeFrontendStreamState`, encoder/decode state), and Parakeet TDT (`decode_incremental`, center/left/right context buffers). Kroko and Sense also expose audio-chunk sessions.
- Qwen3 ASR performs bounded window processing and publishes transcript deltas; it is incremental at window granularity, not token-by-token recurrent audio encoding.
- Higgs STT runs each supplied audio block as an independent request, then concatenates deltas.
- Nemotron’s `NemotronASRStreamingSession::process_audio_chunk` only appends to `streaming_audio_`; inference starts in `finalize`, so its advertised “streaming” is buffered ingest rather than low-latency transcription.
- `VibeVoiceASRSession` contains streaming methods, but `VibeVoiceASRLoadedModel::create_task_session` rejects every non-offline mode and the loader advertises offline only (`src/models/vibevoice_asr/loader.cpp:124–133`). That implementation is currently unreachable through the registry.

Android must expose per-family latency semantics/capabilities rather than one boolean `streaming` flag.

### TTS and generation architecture

TTS families combine text normalization/tokenization, optional speaker/style encoders, autoregressive or diffusion/flow generation, neural audio codecs, and vocoders. Shared modules cover Qwen/Gemma/T5 decoders, attention/KV caches, sampling, speaker encoders, BigVGAN/HiFT vocoders, and text chunking.

Output “streaming” is also heterogeneous:

- Confucius pulls one synthesized text segment per `next_stream_event` and merges at finish.
- Dots synthesizes inside `start_stream` and emits codec/vocoder chunks through a callback; `next_stream_event` immediately returns no event.
- NeuTTS, OmniVoice, Supertonic, and VoxCPM2 expose chunk/event surfaces with family-specific buffering.
- Some outputs are complete segment chunks, not real-time PCM frames. Cross-fade/merge can make the final audio differ from concatenated events.

### Codecs and vocoders

There are two separate meanings of “codec”:

1. **File codec support:** native IO only reads RIFF/WAVE PCM16, PCM24, and float32 (`read_wav_f32`) and writes PCM16 WAV (`write_pcm16_wav`). There is no native MP3/AAC/Opus/FLAC container decoder. CLI workflows shell out to ffmpeg via `posix_spawnp`/`_wspawnvp`.
2. **Neural audio codecs:** framework and model internals encode/decode acoustic tokens/latents. Core APIs include `IAudioCodec`, `SEANetDecoder`, Mimi encoder/decoder runtimes, and FSQ codec decoding. Model-specific codecs include Fish Audio, Higgs, HeartMuLa, Irodori, IndexTTS2 semantic codec, Qwen3 speech tokenizer encoder/decoder, MOSS tokenizers, MioCodec, and others.

On Android use `MediaExtractor`/`MediaCodec` or app-layer decoding to float PCM and feed the runtime; do not port the ffmpeg workflow process path into the core library.

### Preprocessing and DSP

Key exact surfaces:

- WAV: `audio/wav_reader.h/.cpp`, `wav_writer.h/.cpp`.
- channel conversion: `audio/conversion.h/.cpp`.
- resampling: `resample_mono_linear`, torchaudio-compatible sinc-Hann, and optional dynamically loaded libsoxr in `audio/resampling.cpp`.
- FFT/STFT/ISTFT/mel: `audio/fft.h/.cpp`, `audio/dsp.h/.cpp`, `audio/istft_graph.h/.cpp`.
- Kaldi fbank/CMVN/LFR: `audio/kaldi_fbank.h/.cpp`.
- waveform trim, preemphasis, normalization, padding: `audio/waveform_ops.h/.cpp`.
- activity, chunking, mixing, overlap-add: `activity`, `chunking`, and `mixing` files.
- denoise/super-resolution utility models: DeepFilterNet2, RNNoise, ZipEnhancer, FlashSR.

The resampler’s libsoxr names are desktop sonames. Absence is nonfatal where callers use `resample_mono_soxr_or_linear`, but the fallback changes numerical/audio quality. Inflect v2 is different: it dynamically loads eSpeak-ng and fails without a packaged library plus data directory.

### Buffer contracts

`runtime/session.h` uses interleaved float PCM in `AudioBuffer`/`AudioChunk`. Time spans are sample indices. There is a second `engine::audio::AudioBuffer` in `audio/output.h`, with field name `channel_count`; adapters copy between them. This duplication should stay internal.

The current streaming adapter increments `AudioChunk.start_sample` by scalar interleaved sample values (`app/streaming/streaming.cpp`), while several model paths interpret spans as frames after dividing by channels. Most speech models require mono, but the contract is ambiguous for multi-channel input. The Android C ABI should define `start_frame` and `frame_count` unambiguously and normalize to mono before model sessions unless a family explicitly supports multiple channels.

`IStreamingVoiceTaskSession` is synchronous. It has no cancellation token, bounded queue, backpressure contract, or thread-safety guarantee. Callbacks execute on the inference thread. A session should be treated as single-owner and single-call-at-a-time.

## 6. Threading, memory, mmap, files, atomics, exceptions, RTTI, and alignment

### Threading

The process can create several independent thread domains:

- ggml CPU workers controlled through backend thread configuration.
- about 115 OpenMP pragmas across roughly 42 project files.
- the extracted speech FFT’s process-global thread pool, sized from `hardware_concurrency` (`speech_fft_internal.h`).
- explicit `std::thread` use in DSP and OmniVoice.
- model-internal mutexes/caches.
- server detached client threads and installer threads.

CLI calls `omp_set_num_threads`; server does not. `--threads` therefore does not cap every thread source. On Android, begin with OpenMP off and one inference dispatcher, set ggml threads explicitly (typically 2–4 after device benchmarking), and patch FFT/threaded host kernels so a runtime-wide thread budget is honored. Avoid running two heavyweight sessions concurrently.

The server’s `BusyGuard` documents and enforces the real assumption: one request at a time per loaded model. Reuse that ownership model in JNI.

### Memory ownership

- Each `RuntimeSessionBase` owns a backend handle and graph executor.
- Many families load backend weights in the session constructor; creating multiple sessions can duplicate weights/VRAM even when `ILoadedVoiceModel` shares parsed assets.
- `SafeTensorSource` and `GgufTensorSource` mmap large files, upload/copy tensors to backend buffers, then call `madvise` and/or release source mappings.
- `RuntimeWorkspace` and `RuntimeCache` retain vectors/objects until cleared or the session dies.
- KV state can retain full host float copies during import/export.
- `ConstantTensorCache` tries to fit descriptor contexts to one quarter of detected host free memory, but `BackendWeightStore` and many graph implementations do not use that guard.
- `available_host_memory_bytes` reads host-wide `/proc/meminfo` on Linux/Android; it is not Android LMKD/app memory class or per-process pressure.

Android needs a higher-level model/session memory manager, explicit unload, one active heavyweight model per backend, and measured arena overrides. Do not assume virtual `no_alloc` context size equals resident memory, but do not assume large reservations are harmless either; bionic allocator/address-space/commit behavior and device vendor kernels vary.

### mmap and file behavior

`BinaryBlob::read_binary_blob` uses `open/fstat/mmap(MAP_PRIVATE)` on Unix/Android and a vector fallback. It closes the fd after mapping and can `madvise(MADV_DONTNEED)` uploaded ranges. This is suitable for app-private regular files and poor for compressed APK assets, content URIs, network streams, or asset file descriptors with a nonzero archive offset.

GGUF sidecar materialization has three additional risks:

- cache root is implicit temp storage;
- fingerprint uses `std::hash(path:size:mtime)`, not a collision-resistant content digest;
- extraction writes destination files directly without an interprocess lock or atomic temp/rename, so concurrent loaders can observe partial files.

Patch before concurrent Android use.

### POSIX and dynamic loading

Core POSIX dependencies are limited and available in bionic: mmap/open/sysconf and `dlopen`/`dlsym`. Host application code adds sockets/poll, signals, `posix_spawnp`, waitpid, and detached threads. The recommended Android library excludes CLI/server/workflow.

`dlopen` consumers in project code are:

- optional libsoxr (`audio/resampling.cpp`);
- required eSpeak-ng for Inflect v2 (`community_models/inflect_v2/frontend.cpp`);
- CUDA driver probing (`sampling/torch_random.cpp`, irrelevant to Android).

Android linker namespaces will not discover arbitrary desktop sonames. Package explicit libraries and paths or exclude the dependent family.

### Atomics and ABI choice

The project and ggml use C/C++ atomics, mutexes, TLS, and cache-line alignment. `arm64-v8a` is the recommended first and required production ABI for realistic model address space. Do not prioritize `armeabi-v7a`: its address-space ceiling is incompatible with many contexts/models and adds 64-bit atomic/alignment/libatomic risk. Add `x86_64` only for emulator CI if size/cost justify it.

SentencePiece tries to add `atomic` on ARM-like processors, but that does not fix the separate missing Android `log` propagation.

### Exceptions and RTTI

Exceptions and RTTI are architectural requirements at this revision:

- thousands of validation/error paths throw exceptions;
- production model code uses `dynamic_cast` (for example Sense ASR, MioTTS, Qwen3 ASR, and VibeVoice ASR nested sessions);
- CLI/server/workflow use RTTI to select offline/streaming interfaces;
- SentencePiece headers also expose throwing convenience APIs.

Building with `-fno-exceptions` or `-fno-rtti` is a high-risk rewrite, not a portability flag. Keep both enabled internally. Catch `std::exception` and `...` at every exported C/JNI entry, return status codes, and never allow an exception to cross JNI.

### Alignment and byte parsing

- ggml/backend buffers and the extracted FFT have explicit alignment handling.
- `speech_fft_internal.h` uses a portable over-aligned allocator; its fallback `malloc(size + align)` should receive an overflow guard for hostile/huge dimensions.
- WAV reader interprets `std::vector<char>` storage through `int16_t*`/`float*` casts and reads RIFF integer fields in host endianness. Android arm64 is little-endian and allocators are adequately aligned in practice, but the cast has object-lifetime/strict-aliasing risk. Replace with byte assembly/`memcpy`; add WAV size overflow checks and explicit little-endian writes.
- WAV writer truncates sizes to 32-bit RIFF and does not reject output exceeding 4 GiB.

These are correctness hardening items, not the first arm64 bring-up blocker.

## 7. Vulkan, OpenMP, native flags, llamafile, and SentencePiece

### Vulkan

**Build-time:** `external/ggml/src/ggml-vulkan/CMakeLists.txt` calls `find_package(Vulkan COMPONENTS glslc REQUIRED)`, probes shader extensions by invoking host `glslc`, and builds `vulkan-shaders-gen` as an `ExternalProject`. Under NDK cross-compilation it must find host C/C++ compilers and a host toolchain. An Android NDK alone is not sufficient; CI needs host Vulkan headers/loader metadata and `glslc`, or explicit CMake paths.

**Runtime:** `ggml_backend_vk_device_supports_op` has device-, dtype-, contiguity-, shared-memory-, subgroup-, buffer-range-, and shape-specific rejection paths. Relevant examples include:

- Flash attention requires head dimensions divisible by 8 and subgroup/cooperative-matrix features.
- `CUMSUM`, `TOP_K`, `ARGSORT`, `SOLVE_TRI`, and SSM ops have device-specific limits.
- transpose-conv1d currently requires F32 inputs/weights.
- BF16/F16 copy/matmul combinations are restricted; audio.cpp converts native BF16 weights to F16 for Vulkan/Metal in `BackendWeightStore`.

**Policy:** ship CPU first. For Vulkan, run a prepare+one-inference smoke for every supported family/precision/device class, explicitly select `BackendType::Vulkan`, and recreate the session on CPU if prepare/run fails. Do not use `BestAvailable` in production because the selected backend and failure surface are less predictable.

### OpenMP

The correct Android baseline must set both:

```text
-DENGINE_ENABLE_OPENMP=OFF
-DGGML_OPENMP=OFF
```

Setting only the engine option leaves ggml’s independent default on. If OpenMP is later enabled, package the NDK OpenMP runtime correctly, avoid duplicate runtimes, and measure oversubscription against ggml/FFT workers. There is no evidence that OpenMP-on is required for correctness.

### Native CPU flags

The top-level project overrides ggml’s sensible cross-compiling default by setting `GGML_NATIVE` from an engine option that defaults on. For Android always set:

```text
-DENGINE_ENABLE_NATIVE_CPU=OFF
```

A baseline arm64 build should use the NDK’s ABI floor (ARMv8-A/NEON) and runtime feature dispatch only after validation. Do not compile a Play-distributed binary with `-mcpu=native` from the build host.

### llamafile

The vendored llamafile SGEMM source has ARM NEON and optional dot-product paths and is not intrinsically x86-only. It is therefore not a hard arm64 compile blocker. It is, however, enabled by audio.cpp rather than ggml’s upstream default, adds a large intrinsic-heavy translation unit, and has not been validated here on Android. Disable it for the first deterministic build:

```text
-DENGINE_ENABLE_LLAMAFILE=OFF
```

Re-enable only after CPU correctness, binary-size, thermal, and latency comparisons on the minimum supported devices.

### SentencePiece

SentencePiece is built from source as a static library with bundled protobuf-lite. Its processor wrapper caches processors globally by a hash of the extracted piece inventory (`tokenizers/sentencepiece.cpp`). The cache is mutex-protected but unbounded and hash collisions are not verified against the full inventory. This is acceptable for a small fixed model set but should be included in lifecycle/memory review.

Android-specific actions:

- final JNI link must include `log`;
- keep exceptions/TLS enabled;
- use arm64 first;
- consider building only processor sources rather than trainer targets/tools for packaging hygiene (the current subdirectory declares trainer and CLI targets even though `EXCLUDE_FROM_ALL` keeps them out of the default dependency closure).

## 8. CLI, server, examples, and tests

### CLI

`app/cli/main.cpp` is the reference orchestrator. It supports loader inspection, device listing, model/package overrides, all task enums, offline batch execution, raw PCM stdin streaming, result sinks, workflows, metrics, and VAD chunk output. Request conversion lives in `app/cli/request.cpp`; stream bridging is `app/streaming/*`; batch/workflow/file output is in `app/workflow/*`.

It is useful as the semantic reference for JNI, but should not be linked wholesale into Android. Host-only concerns include stdin/TTY behavior, ffmpeg process spawning, terminal output, POSIX/Windows entry points, and filesystem workflows.

### Server

`audiocpp_server` is a custom socket HTTP/1 server with one detached thread per client (`app/server/http.cpp`), static/streaming responses, chunked live PCM ingest, SSE, OpenAI-like speech/transcription routes, per-model lazy load/unload, and an embedded WebUI. `ServerState::LoadedModel` owns one model/session and serializes access with `BusyGuard` (`app/server/runtime.h`).

It is not an Android service layer:

- no TLS/auth;
- large default request-body limits;
- detached native threads;
- signal/process assumptions;
- model installer invokes repository Python tooling;
- WebUI assets and demo voices are embedded during CMake configure.

Use Kotlin/Binder/service APIs for Android lifecycle and permissions. Reuse the one-session-per-model serialization concept, not the HTTP server.

### Examples

- `examples/ggml_simple_inference.cpp`: raw ggml context/graph example.
- `examples/model_spec_demo/*`: package-spec/resource-discovery examples.
- `examples/xcode/Qwen3TTSDemo`: Apple bridge example, useful conceptually but not an Android ABI.
- Docker/browser-extension examples are deployment examples, not native-library tests.

### Tests and CI

`ENGINE_BUILD_TESTS=ON` registers up to 61 CTest names, including:

- tokenizer, JSON/spec, safetensors/GGUF, WAV, DSP, chunking, text, sampler, diffusion, backend-device, convolution/attention, streaming adapter, HTTP, server guard/config/installer tests;
- optional Fun-ASR model/GGUF tests gated on local paths/backend;
- Parakeet golden/streaming tests that return skip code 125 without multi-GB weights.

`ENGINE_BUILD_WARMBENCH=ON` adds many family-specific model load/inference probes. `tests/warmbench.py` and path-comparison Python tools drive larger validation.

The checked-in Linux/macOS/Windows GitHub workflows run loader/spec sync and compile CLI/server/converter. They do **not** enable `ENGINE_BUILD_TESTS` or run CTest. There is no Android workflow.

Tests are not automatically compatible with a cross-build: CTest would try to execute target Android binaries on the host, and some tests embed source-tree paths. Keep host CTest as a required gate and create a small Android instrumentation/`adb` native-test target for file IO, SentencePiece, GGUF, CPU backend, streaming, and selected model smoke tests.

## 9. Licensing and distribution audit

This section is an engineering inventory, not legal advice.

### Located licenses

- Project: Apache-2.0, root `LICENSE`.
- ggml: MIT, `external/ggml/LICENSE`.
- cJSON: MIT, `external/cJSON/LICENSE`.
- SentencePiece: Apache-2.0, `external/sentencepiece/LICENSE`.
- SentencePiece bundled absl: Apache-2.0.
- SentencePiece bundled darts-clone: BSD-style 3-clause.
- SentencePiece bundled esaxx: MIT-style.
- SentencePiece bundled protobuf-lite: BSD-style 3-clause.
- Extracted speech FFT: BSD-3-Clause terms embedded at the top of `src/framework/audio/detail/speech_fft_internal.h`; binary redistribution requires reproducing the notice.
- RNNoise converted utility assets: BSD-3-Clause noted in `assets/framework/audio_utilities/rnnoise/MANIFEST.md`.
- llamafile SGEMM has an MIT notice in `external/ggml/src/ggml-cpu/llamafile/sgemm.cpp`.

### Gaps

- No license file or copyright notice was found in the vendored `external/libyaml` slice.
- No license file or source notice was found in `external/llama_tokenizer`; provenance appears implicit rather than documented locally.
- No root `NOTICE` or generated third-party notice bundle exists.
- Native WebUI dependencies have a package lock but no committed notice bundle.
- `model_specs/*.json` do not record model/code/weight licenses. Model weights, tokenizer data, voices, and eSpeak data may carry terms independent of this source tree.

### Distribution requirement

Before Android release, produce a machine-audited Software Bill of Materials and `THIRD_PARTY_NOTICES` shown in-app and packaged in the AAR/APK. Resolve libyaml/llama-tokenizer provenance, preserve MIT/BSD notices, include Apache license text/required notices, and add license metadata/allowlist enforcement to model download manifests. Do not assume GGUF conversion changes the underlying model license.

## 10. Baseline build and test commands

All builds should be out of tree; the paths below keep generated files outside the pinned checkout.

### 10.1 Verify source and loader/spec synchronization

```bash
SRC=${INBHARAT_AUDIO_RND}/upstream/audio.cpp

git -C "$SRC" rev-parse HEAD
# Must print bb15edd78b56e035967e0eb999a6b28a62337db4
git -C "$SRC" status --short
# Must be empty.

python3 "$SRC/tools/check_loader_catalog_sync.py" --self-test
python3 "$SRC/tools/check_loader_catalog_sync.py"
```

### 10.2 Upstream-like host CPU build

```bash
SRC=${INBHARAT_AUDIO_RND}/upstream/audio.cpp
BLD=${INBHARAT_AUDIO_RND}/builds/linux/upstream-cpu

cmake -S "$SRC" -B "$BLD" -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DENGINE_ENABLE_CUDA=OFF \
  -DENGINE_ENABLE_HIP=OFF \
  -DENGINE_ENABLE_VULKAN=OFF \
  -DENGINE_ENABLE_METAL=OFF

cmake --build "$BLD" --parallel --target \
  engine_runtime audiocpp_cli audiocpp_server audiocpp_gguf
```

### 10.3 Deterministic portable host build with unit tests

This deliberately disables native ISA, llamafile, and both OpenMP layers so failures are not hidden by host-specific features.

```bash
SRC=${INBHARAT_AUDIO_RND}/upstream/audio.cpp
BLD=${INBHARAT_AUDIO_RND}/builds/linux/audit-portable-tests

cmake -S "$SRC" -B "$BLD" -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DENGINE_ENABLE_CUDA=OFF \
  -DENGINE_ENABLE_HIP=OFF \
  -DENGINE_ENABLE_VULKAN=OFF \
  -DENGINE_ENABLE_METAL=OFF \
  -DENGINE_ENABLE_NATIVE_CPU=OFF \
  -DENGINE_ENABLE_LLAMAFILE=OFF \
  -DENGINE_ENABLE_OPENMP=OFF \
  -DGGML_OPENMP=OFF \
  -DENGINE_BUILD_TESTS=ON \
  -DENGINE_BUILD_EXAMPLES=ON \
  -DENGINE_BUILD_WARMBENCH=OFF \
  -DAUDIOCPP_MODEL_SET=full

cmake --build "$BLD" --parallel
ctest --test-dir "$BLD" --output-on-failure -j 4
```

Run a second Debug/ASan/UBSan host lane after the baseline. Keep in mind that top CMake changes Debug optimization to `-O3 -g`; pass explicit sanitizer flags and inspect `compile_commands.json`.

### 10.4 Host Vulkan compile and operation tests

```bash
SRC=${INBHARAT_AUDIO_RND}/upstream/audio.cpp
BLD=${INBHARAT_AUDIO_RND}/builds/linux/audit-vulkan

cmake -S "$SRC" -B "$BLD" -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DENGINE_ENABLE_VULKAN=ON \
  -DENGINE_ENABLE_CUDA=OFF \
  -DENGINE_ENABLE_HIP=OFF \
  -DENGINE_ENABLE_METAL=OFF \
  -DENGINE_ENABLE_NATIVE_CPU=OFF \
  -DENGINE_ENABLE_OPENMP=OFF \
  -DGGML_OPENMP=OFF \
  -DENGINE_BUILD_TESTS=ON

cmake --build "$BLD" --parallel
ENGINE_VALIDATE_BACKEND_GRAPH=1 \
  ctest --test-dir "$BLD" --output-on-failure \
  -R 'backend_device_resolution|conv_lowering_matrix|conv_transpose_fast_path|encoder_module'
```

A compile check is not a model-runtime claim. Run family/precision smoke tests separately on the intended GPU/driver.

### 10.5 Android arm64 static compile probe

This only proves that archives compile; it does **not** catch final JNI link errors or run code.

```bash
SRC=${INBHARAT_AUDIO_RND}/upstream/audio.cpp
BLD=${INBHARAT_AUDIO_RND}/builds/android/arm64-core
NDK=/absolute/path/to/android-ndk

cmake -S "$SRC" -B "$BLD" -G Ninja \
  -DCMAKE_TOOLCHAIN_FILE="$NDK/build/cmake/android.toolchain.cmake" \
  -DANDROID_ABI=arm64-v8a \
  -DANDROID_PLATFORM=android-26 \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DENGINE_ENABLE_CUDA=OFF \
  -DENGINE_ENABLE_HIP=OFF \
  -DENGINE_ENABLE_METAL=OFF \
  -DENGINE_ENABLE_VULKAN=OFF \
  -DENGINE_ENABLE_NATIVE_CPU=OFF \
  -DENGINE_ENABLE_LLAMAFILE=OFF \
  -DENGINE_ENABLE_OPENMP=OFF \
  -DGGML_OPENMP=OFF \
  -DENGINE_BUILD_TESTS=OFF \
  -DENGINE_BUILD_EXAMPLES=OFF \
  -DENGINE_BUILD_WARMBENCH=OFF \
  -DAUDIOCPP_DEPLOYMENT_BUILD=ON \
  -DAUDIOCPP_MODEL_SET=core

cmake --build "$BLD" --parallel --target engine_runtime
```

Then build an actual Android `SHARED` smoke target that links `engine_runtime`, `ggml`, `log`, and the wrapper. That final link is mandatory because static archive creation will not reveal missing `__android_log_write` or PIC closure problems.

### 10.6 Android Vulkan compile probe

Add `-DENGINE_ENABLE_VULKAN=ON` only after CPU succeeds. Provide a host `glslc`/Vulkan SDK and, if auto-detection fails, `GGML_VULKAN_SHADERS_GEN_TOOLCHAIN` or the corresponding host compiler/tool paths. Build and run on-device; do not infer Android support from the Linux Vulkan CI build.

## 11. Recommended Android ABI and runtime architecture

### Layering

```text
Kotlin/Java API
  AudioCppRuntime / ModelHandle / SessionHandle / StreamHandle
              |
              v
Thin JNI registration layer
  - direct ByteBuffer or bounded copies
  - no inference on UI or real-time audio callback thread
              |
              v
Versioned C ABI (hidden C++ implementation)
              |
              v
audio.cpp C++ ModelRegistry / model / session
              |
              v
ggml CPU baseline; optional Vulkan build flavor with CPU fallback
```

### C ABI shape

Use opaque handles and versioned POD structures, for example:

```c
typedef struct ac_runtime ac_runtime;
typedef struct ac_model ac_model;
typedef struct ac_session ac_session;
typedef struct ac_stream ac_stream;

typedef struct {
    uint32_t struct_size;
    uint32_t sample_rate;
    uint32_t channels;
    uint64_t frame_count;
    const float * interleaved_f32;
} ac_audio_view_v1;

typedef enum {
    AC_OK = 0,
    AC_INVALID_ARGUMENT,
    AC_NOT_SUPPORTED,
    AC_IO_ERROR,
    AC_OUT_OF_MEMORY,
    AC_BACKEND_ERROR,
    AC_CANCELLED,
    AC_INTERNAL_ERROR
} ac_status;
```

Required functions should cover:

- ABI/version query and device enumeration.
- runtime creation with cache directory, thread budget, log callback, and backend policy.
- registry/loader advertisement as JSON or caller-owned POD arrays.
- model load/unload and inspection.
- session create/prepare/run/destroy.
- stream start/push-frames/poll-or-callback/finish/cancel.
- result/buffer ownership and explicit release.
- thread-local or handle-local last-error retrieval.

Rules:

- no STL, C++ classes, exceptions, `filesystem::path`, JNI types, or allocator ownership cross the C boundary;
- every struct starts with `struct_size`/version;
- define callback thread and buffer lifetime precisely;
- use frame indices in the public API, not ambiguous scalar-sample indices;
- catch all exceptions at the boundary;
- mark all other symbols hidden and export only the C/JNI surface.

### Lifecycle and concurrency

1. One process-wide runtime/registry.
2. One loaded heavyweight model per backend by default; optional small VAD can coexist after measurement.
3. One session owns one backend context and is serialized.
4. One dedicated inference executor; do not run inference on the UI thread or AudioRecord/AAudio callback.
5. Explicit unload on lifecycle/memory-pressure events.
6. LRU only at the model level, not implicit session duplication.
7. Treat cancellation as cooperative between graph/token/chunk steps. Current ggml calls cannot safely be force-cancelled. For Vulkan robustness, consider an isolated Android service process so a wedged driver/inference process can be restarted without corrupting the UI process.

### Model and file policy

- Store models as ordinary files under app-private storage or an approved external app directory.
- Verify digest, expected size, package spec, family, and license before load.
- Prefer one standalone audio.cpp GGUF per model directory, but inject an app cache root for sidecars or keep required sidecars external until extraction is patched.
- Use `AUDIOCPP_DEPLOYMENT_BUILD=ON` for safetensors/legacy packages when no external model-spec directory is guaranteed; new GGUFs should carry embedded specs.
- Do not rely on process current working directory for spec discovery.
- Decode Android media containers in the app/media layer to normalized mono float PCM at the model’s expected rate.

### Backend packaging

- **Phase 1:** arm64-v8a CPU-only, native flags off, llamafile off, OpenMP off, explicit ggml thread count.
- **Phase 2:** a Vulkan build flavor that still includes CPU. Select Vulkan explicitly after device enumeration and a model-specific prepare/smoke. On failure, destroy and recreate the session on CPU; there is no mixed graph fallback.
- Avoid `GGML_BACKEND_DL`/CPU-all-variants for the first Android release. Android linker namespaces, plugin packaging, and multiple C++ runtimes add risk without solving the primary model-memory problem.
- Do not use `BestAvailable` for production routing.

### Build target

Create an Android overlay target rather than patching upstream immediately:

```cmake
add_subdirectory(${AUDIOCPP_SOURCE_DIR} audiocpp-upstream)
add_library(audiocpp_jni SHARED audiocpp_c_api.cpp audiocpp_jni.cpp)
set_target_properties(audiocpp_jni PROPERTIES
    CXX_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN YES)
target_link_libraries(audiocpp_jni PRIVATE engine_runtime ggml log android)
```

Configure only `audiocpp_jni` from Gradle. Add `-Wl,--no-undefined` and inspect final DT_NEEDED entries. If the AAR contains more than one native shared library using C++, use one consistent `libc++_shared.so`; if it contains exactly one JNI library, a static libc++ policy can be considered, but must remain consistent across all native dependencies.

## 12. Patch plan and risk

| Priority | Patch | Risk | Notes |
|---|---|---:|---|
| P0 | Add Android wrapper target and versioned C ABI outside upstream. | Medium | New surface; no model math changes. |
| P0 | Android/cross defaults: native off; synchronize engine/ggml OpenMP; app target guards; PIC. | Low | CMake-only; test all desktop lanes. |
| P0 | Link `log` for SentencePiece static Android builds. | Low | One-target CMake fix. |
| P0 | Add runtime cache-root injection and atomic/locked GGUF sidecar extraction. | Medium | Affects standalone GGUF path resolution and concurrency. |
| P0 | Add Android arm64 compile/link/instrumentation CI. | Low | Infrastructure only. |
| P0 | Produce SBOM/notices and resolve missing libyaml/llama-tokenizer/model-license provenance. | Process | Release gate, not a code behavior change. |
| P1 | Establish model-specific Android arena/thread/precision allowlists and memory telemetry. | Medium–High | Wrong reductions can fail graph construction or change outputs. Validate each family. |
| P1 | Add path/fd+offset reader abstraction if zero-copy AAB assets are required. | High | Touches `ResourceBundle`, `TensorSource`, safetensors/GGUF readers, mmap lifetime, and sidecars. Copy-to-files is safer initially. |
| P1 | Make backend graph-status failures mandatory and run support validation in production prepare paths. | Medium | May reveal latent backend failures currently ignored; desirable but behavior changes from silent continuation to error. |
| P1 | Normalize streaming capability metadata and document latency class; remove or enable unreachable VibeVoice-ASR streaming code. | Medium | API/behavior compatibility risk. |
| P1 | Add runtime-wide thread budget/cancellation checks. | High | Cross-cuts ggml, OpenMP, FFT, generation loops, and callbacks. |
| P2 | Replace WAV typed pointer casts with explicit little-endian parsing; add RIFF size limits/RF64 policy. | Low–Medium | Well-contained; add parity tests. |
| P2 | Bound/verify global tokenizer and frontend caches. | Medium | Can affect performance and shared lifetimes. |
| Avoid | `-fno-exceptions`, `-fno-rtti`, changing STL classes into exported ABI, or broad model math rewrites during bring-up. | Very high | These are architectural rewrites and will obscure portability failures. |

## 13. Minimum Android acceptance gates

1. Clean arm64 CPU configure, archive build, and final `SHARED` JNI link with no undefined symbols.
2. Final dependency audit (`llvm-readelf -d`) shows only intended NDK/system libraries and the chosen C++ runtime; no accidental OpenMP or host library dependency.
3. On-device unit smoke: filesystem/temp/cache, mmap, SentencePiece, safetensors, GGUF with external and embedded sidecars, CPU backend enumeration, one tiny graph, WAV input, and streaming adapter.
4. At least one small model per shipped task runs prepare + inference under low-memory and repeated load/unload tests.
5. Peak Java/native PSS, mapped bytes, backend bytes, load latency, first-run latency, steady latency, and thermal behavior are recorded by model/precision/device.
6. Concurrent calls to the same session are rejected/serialized; unload while running is impossible by construction.
7. Exceptions are contained at C/JNI; invalid files return stable status codes and never abort the process.
8. Vulkan, if shipped, passes model/precision/device allowlist tests with backend-graph validation enabled and a verified CPU recreation path.
9. App background/foreground, process death, storage revocation, partial model download, and cache eviction are tested.
10. Third-party notices and model-license metadata are present in the distributable and visible in-app.

## 14. Overall recommendation

Use audio.cpp as an **internal C++ engine behind a narrow Android C ABI**, not as a public C++ SDK and not by embedding its CLI/server. Begin with an arm64, CPU-only, custom/core model build; disable native flags, llamafile, and both OpenMP layers; keep exceptions and RTTI; store models as verified app-private files; serialize one session on a dedicated inference executor; and instrument memory aggressively. Add Vulkan as an explicitly selected build flavor only after per-model graph and device validation. This architecture minimizes ABI churn, loader/linker surprises, thread oversubscription, and unrecoverable GPU failures while preserving the upstream runtime/model implementation.
