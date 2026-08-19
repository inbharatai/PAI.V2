# Upstream patch ledger

Pinned upstream: audio.cpp release 0.6, commit `bb15edd78b56e035967e0eb999a6b28a62337db4`.

## Current release candidate

No upstream file is modified, copied, compiled, or linked by the default InBharat Audio build. The optional scaffold in `src/adapters/audio_cpp/audio_cpp_adapter.cpp` verifies the exact Git revision and requires a clean checkout at configure time. It currently exposes provenance/build metadata only; it does not claim upstream model parity.

| InBharat file | Upstream file/symbol | Reason | Platform | Behaviour change | Test | Upstream revision |
|---|---|---|---|---|---|---|
| `src/adapters/audio_cpp/audio_cpp_adapter.cpp` | None linked | Establish a reviewable future adapter boundary | all | Adds provenance probe only | pin acceptance and wrong-pin rejection | `bb15edd78b56e035967e0eb999a6b28a62337db4` |
| `CMakeLists.txt` | Git checkout metadata | Refuse wrong or dirty upstream trees | all | Configure fails closed | `reports/audio-cpp-*-pin*.log` | same |

## Required format for future patches

Every reused or changed upstream path must record file, function or symbol, reason, platform, behaviour change, test, exact revision, licence, and notice impact before merge. Do not make random deep changes to ggml. CPU correctness and model-specific parity evidence are required before any accelerator claim.
