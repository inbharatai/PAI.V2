# Benchmarks

Run `scripts/benchmark.sh` after a release build. Results are written to `benchmarks/routing-latest.json`; raw validation output is retained under `reports/`.

The benchmark measures deterministic mixed routing, routing latency, false L2/L3 activation over 600 ordinary prompts, execution-level counts, and 10,000-session ownership churn. It does not measure remote-model latency or claim device-independent performance.

The validated local release gate used 100,000 mixed routes and produced zero false L2/L3 activations in the bundled corpus. RC2 nanosecond values, adversarial coverage, artifact hashes, and limitations are in `reports/HARDENING_VALIDATION.md`; `VALIDATION_EVIDENCE.md` preserves the RC1 baseline. Re-run results after source or toolchain changes rather than copying historical numbers.

Future production adapters must add startup, RSS, time-before-provider-call, tool-schema bytes, model streaming, cancellation, recovery, CPU, and platform-specific measurements without weakening the correctness gates.
