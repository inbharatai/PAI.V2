# Benchmarks

`ibaudio benchmark` runs one-second reference ASR, one-second energy VAD, and a fixed reference TTS phrase. It emits schema `inbharat.ibaudio.benchmark.v1` as JSON and CSV, recording runtime version, backend, operation, iteration count, and mean wall milliseconds.

These timings validate regression/tooling only. They are not neural-model latency, real-time factor, quality, mobile thermal, memory, or accelerator claims. Run warm/cold, peak RSS, and physical-device measurements when real adapters are admitted.

```sh
ITERATIONS=20 ./scripts/run_benchmarks.sh
```

Committed benchmark examples are machine-readable but environment-specific. RC2 hardening results and the 1,000-iteration reference run are summarized in `reports/HARDENING_VALIDATION.md`; RC1 evidence remains preserved in `BUILD_EVIDENCE.md` and `TEST_EVIDENCE.md`. Never compare results without matching build type and hardware.
