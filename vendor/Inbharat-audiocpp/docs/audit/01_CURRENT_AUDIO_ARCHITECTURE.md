# 01 — Current Audio Architecture (as audited 2026-08-20)

Scope: the tree as received in `inbharat-audio-v0.2.0-innovation.zip` (baseline commit `3bd2181`), plus one portability fix (`ecca9d9`). All line references are to that tree. Audit method: full source read of `src/`, `include/inbharat/ibaudio.h`, `src/adapters/`, `src/innovation/`, `tools/`, `tests/`, cross-checked against `abi/ibaudio_symbols_v1.txt` and the recorded gates in `reports/`.

## 1. Two-layer design

The codebase is a hardened C99 façade over deterministic reference primitives:

- **Façade layer** — validation, lifecycle counters, single-flight + cooperative-cancellation threading, structured errors, capability/streaming metadata. Files: `runtime.cpp` (615 LOC), `session.cpp` (631), `stream.cpp` (561), `facade_util.cpp` (390), `internal.hpp` (248).
- **Engine layer** — pure deterministic DSP: `audio.cpp` (600 LOC: PCM processing, energy VAD, reference ASR/TTS, WAV codec), `sha256.cpp` (181), `backend_probe.cpp` (55).

Every C entry point runs through the `guarded()` exception firewall (`internal.hpp:43-70`); six opaque handle families form a strict ownership tree (runtime → model → session → job/stream) with liveness counters; `ibaudio_buffer` carries a metrics back-pointer so runtime shutdown refuses while outputs are live.

## 2. Component map

| File | LOC | Role |
|---|---:|---|
| `src/internal.hpp` | 248 | Internal contract: limits, `guarded()`, `MetricsData`, `CancellationToken`, all opaque-handle layouts |
| `src/runtime.cpp` | 615 | Version/error/options, runtime lifecycle, 8-entry backend catalog, 4 built-in model registrations, diagnostics JSON, metrics |
| `src/session.cpp` | 631 | Sessions, sync inference, async jobs, barge-in |
| `src/stream.cpp` | 561 | Pull-based streams, event queue with coalescing + 4096 hard cap, incremental resampling |
| `src/audio.cpp` | 600 | `process_audio`, `run_energy_vad`, `run_reference_asr`, `run_reference_tts`, WAV encode/decode |
| `src/facade_util.cpp` | 390 | `model_load` (path policy + SHA-256 admission + LRU), buffer accessors, `sha256_file` |
| `src/backend_probe.cpp` | 55 | Vulkan loader probe only (never device/inference) |
| `src/sha256.cpp` | 181 | Self-contained SHA-256 |
| `src/adapters/audio_cpp/audio_cpp_adapter.cpp` | 25 | Pin-validation scaffold only; returns `DEFERRED`; not called by anything |
| `src/innovation/*` (8 files) | ~1511 | Innovation modules — see doc 03 |
| `tools/ibaudio/main.cpp` | 496 | CLI, public-ABI-only consumer |
| `android/.../ibaudio_jni.cpp` | 455 | JNI bridge, Android toolchain only |

## 3. ABI surface — 94 symbols

Runtime 11, session-family 11, model 4, job 8, stream 9, audio/WAV 4, buffer 5, version/error 5, sha256 1, innovation 34 = **94**, matching `abi/ibaudio_symbols_v1.txt`. The metadata a capability router needs already exists in ABI v1: `ibaudio_model_descriptor_v1.{task, streaming_class, capabilities, required_sample_rate}`, `ibaudio_backend_info_v1`, `ibaudio_capabilities_v1.feature_flags`.

## 4. Threading/lifecycle model

- Single-flight per session via CAS `busy` flag; contention → `BUSY` + `calls_rejected_busy` metric.
- Jobs: one `std::thread` per job, inputs copied before return, QUEUED→RUNNING→terminal under mutex+CV; cooperative cancellation polled in the engines; `job_release` always cancels+joins.
- Streams: no worker thread; work on caller threads under `stream->mutex`; events own payloads until `stream_event_release`; terminal event consumable once.
- Barge-in: caller-driven level reporting → state machine → cancels the active **job** (not streams).
- **Delicate point:** the barge-in → `active_job` cancellation path (`session.cpp:606-611`) and its implicit lock ordering against `set_job_terminal` (`:119-139`). Flagged for any refactor.
- Innovation modules sit outside this machinery: no `guarded()`, no `set_error`, no metrics, own ad-hoc locking (see doc 03).

## 5. Adapter boundary

`audio_cpp_adapter.cpp` is a 25-line pin-validation stub (`reviewed_commit()`, `availability() == DEFERRED`). The real gate is in CMake (`CMakeLists.txt:44-67`): configure-time `git rev-parse HEAD` must equal the pin and `git status --porcelain` must be clean. `AUDIOCPP_AUDIT.md` lists 8 upstream blockers (A1 no stable C ABI / STL+exceptions across API; A7 desktop-scale memory defaults) that any real provider must bridge. Seven concrete bridging requirements are recorded in the Phase-4 spec.

## 6. Extension points for provider/capability routing (ABI-v1-safe)

1. Model catalog construction in `ibaudio_runtime_create` (`runtime.cpp:390-410`) — registry already supports "registered but unavailable with reason" (`kws-deferred-v1` precedent).
2. `ibaudio_model_load` dispatch (`facade_util.cpp:85-104`) — natural router seam; `ibaudio_model` internals are private, so a provider handle can be added without ABI change.
3. Reference-engine call sites — all inference funnels through `run_reference_asr/tts` + `run_energy_vad` (~10 call sites, one signature shape). Highest-leverage seam: swap to a provider vtable with current engines as the built-in "reference" provider.
4. Backend catalog + probe loop (`runtime.cpp:370-388`, `backend_probe.cpp`) — data-driven; provider per-backend availability callbacks slot in.
5. Innovation wrappers — already create/destroy opaque handles; de-facto unregistered providers.

## 7. What this audit confirms for the refactor

The provider seam was visibly prepared (data-driven catalogs, capability bitmasks, deferred-model precedent, pinned-adapter gate) but never cut. The universal-core work is therefore an **extension** of ABI v1 — not a rewrite — and the plan's premise holds: the shell is production-hardened; the speech intelligence is reference-grade.
