# Reference benchmark evidence

Release CPU benchmark, 20 iterations, Intel Xeon @ 2.90 GHz sandbox:

| Operation | Mean ms |
|---|---:|
| deterministic reference ASR (1 s signal) | 0.396632 |
| energy VAD (1 s signal) | 0.125541 |
| deterministic reference TTS phrase | 0.579507 |

Machine-readable records: `benchmarks/reference_cpu.json` and `.csv`; raw stdout: `reports/reference-benchmark.log`.

These are microbenchmarks of local deterministic algorithms. They do not measure neural inference, audio quality, real-time capture/playback, peak RSS, mobile thermals, or accelerators and must not be presented as such.
