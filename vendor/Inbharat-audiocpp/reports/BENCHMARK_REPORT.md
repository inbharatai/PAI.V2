# Benchmark Report

Two benchmark artifacts exist, each honest about what it measures.

## 1. Reference-engine CPU microbenchmarks

`ibaudio benchmark` measures the deterministic reference engines through the CLI. Recorded reference numbers live in `benchmarks/reference_cpu.{json,csv}` and `reports/BENCHMARK_EVIDENCE.md`. These are **deterministic-algorithm timings, not neural inference or latency claims**. The current tree reproduces them; run:

```sh
build/linux-release/ibaudio benchmark --iterations 50 --output-json benchmarks/run.json --output-csv benchmarks/run.csv
```

## 2. Wrapper-overhead benchmark (provider dispatch)

`reports/WRAPPER_OVERHEAD.md` measures whether the universal-core provider indirection adds latency over calling the reference engines directly. Measured on this host (50 iterations, Release, CPU): ASR −2.3%, VAD −3.4%, TTS −0.4% — **no measurable regression**, all within noise. Regression budget: a wrapped mean exceeding the direct mean by >5% on the same host/build is investigated.

## What is NOT benchmarked (and why)

- **Neural ASR/TTS quality and speed** — there is no real neural provider wired yet; the reference engines do not recognize language. The India benchmark (`docs/INDIA_BENCHMARK.md`) becomes meaningful once a real provider (pinned audio.cpp / AI4Bharat / Sarvam) is producing transcripts.
- **GPU/accelerator performance** — CPU is the only backend; no accelerator inference exists to measure.
- **Device (Android/ARM64) performance** — no device run; PENDING.

No performance claim is made anywhere without a corresponding measurement in this file or `reports/WRAPPER_OVERHEAD.md`.
