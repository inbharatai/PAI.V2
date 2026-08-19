# `ibaudio` CLI

The CLI is a developer/validation shell over the same C ABI; it contains no private engine path.

- `info [--json]`: version, capabilities, backend reasons.
- `models [--json]`: compiled descriptors, availability, license/hash, streaming labels.
- `asr --input FILE.wav [--stream --partials --chunk-frames N] [--json]`.
- `tts --text TEXT --output FILE.wav [--json]`.
- `vad --input FILE.wav [--threshold-dbfs DB] [--json]`.
- `benchmark [--iterations N --output-json FILE --output-csv FILE]`.
- `diagnostics`: machine-readable diagnostic snapshot.

Common policy options are `--threads`, `--cache`, `--model-root`, `--backend`, and `--no-fallback`. Explicit unavailable backend plus `--no-fallback` exits nonzero; without it, diagnostics record CPU fallback.

Input media must be WAV PCM16/24/float32. JSON strings are escaped and benchmark schemas are versioned. Exit 0 is success; argument/runtime failures use exit 2 and write details to stderr. CLI regression tests cover every command, malformed input, streamed/offline parity, deterministic TTS hash, and benchmark JSON/CSV shape.
