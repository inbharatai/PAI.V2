# Diagnostics, metrics, and cache

`ibaudio_runtime_get_diagnostics_json` returns owned UTF-8 using schema `inbharat.ibaudio.diagnostics.v1`: runtime/API versions, thread/determinism/path policy, selected/fallback backend, every accelerator's compiled/availability/reason state, and every model's availability, streaming class/label, hash, and SPDX license.

`ibaudio_runtime_get_metrics` takes an atomic snapshot of runtime/model/session/job/stream counts, cache hits/misses, input/output frames, `BUSY` rejections, fallback count, process-wide structured-error count, and live owned buffers. Counters are operational telemetry, not billing. Reset preserves `runtimes_created=1` and the actual live-buffer count so lifecycle safety is not hidden; error reset is process-wide across runtimes.

The RC cache is a bounded, mutex-protected in-memory LRU of model identity+verified-hash keys. It reports repeat-load hits but does not retain weights or persist data. `max_cached_models=0` disables it. Cache directory is still explicit because future approved adapters may need atomic artifacts; RC1 only creates/validates the directory and writes no model sidecar.

Snapshots may race with concurrent work and are individually consistent atomic values, not one globally transactional instant. Avoid high-frequency polling on real-time audio threads. Diagnostic strings contain paths and should be redacted before user telemetry if path disclosure matters.
