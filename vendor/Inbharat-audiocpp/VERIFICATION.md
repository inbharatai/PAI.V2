# Verify This Build Yourself

Self-contained, paste-ready steps to independently confirm the tree. Works on Linux/macOS with a C++17 toolchain, CMake ≥ 3.20, and Ninja. On Windows use Git Bash; use `python` (not `python3` — the Windows `python3` is the Store stub).

## 1. Clean release build + tests + ABI

```sh
cmake -S . -B build/linux-release -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DIBAUDIO_BUILD_TESTS=ON -DIBAUDIO_BUILD_CLI=ON
cmake --build build/linux-release --parallel
ctest --test-dir build/linux-release --output-on-failure
```

**Expected:** `100% tests passed, 0 tests failed out of 15`. Then the ABI gate:

```sh
python3 scripts/check_abi.py build/linux-release/libibaudio.so abi/ibaudio_symbols_v1_core.txt
# (Windows/Git Bash: use `python` instead of `python3`)
```

**Expected:** `ABI OK: 79 exported ibaudio_* symbols match v1 manifest`.

## 2. Sanitizers (AddressSanitizer + UndefinedBehaviorSanitizer)

```sh
./scripts/run_sanitizers.sh
```

**Expected:** `100% tests passed, 0 tests failed out of 15`, exit code 0. (Requires a platform with sanitizer runtime support; on such platforms absence of the runtime is an environment limitation, not a code failure.)

## 3. Benchmarks

```sh
./scripts/run_benchmarks.sh
```

**Expected:** exits 0 and writes a JSON line with `asr`/`vad`/`tts` mean timings (sub-millisecond on a modern CPU). These are deterministic-reference timings, not neural inference claims.

## 3b. audio.cpp adapter build — real neural VAD and ASR

This build adds real neural inference via pinned audio.cpp (release-0.6.1). It needs the pinned upstream built and a licensed ASR model. See `README.md` → "audio.cpp adapter build" for the exact commands.

```sh
# After building per README, verify neural VAD (bundled Silero weights, no download):
build/adapter/ibaudio vad --model audiocpp-silero-vad-v1 --input tests/fixtures/speech_440hz_16k_mono.wav --json
# and neural ASR (licensed, hash-verified Qwen3-ASR model you supply):
build/adapter/ibaudio asr --model audiocpp-qwen3-asr-v1 --input <speech.wav> --json
```

**Expected:** VAD returns a JSON `segments` array; ASR returns a JSON `transcript`. On real speech (e.g. a LibriSpeech clip) ASR returns a non-empty English transcript. If the ASR model directory is absent or fails its hash check, ASR reports UNAVAILABLE — that is correct behavior, not a defect. The pinned model hash is recorded in `licenses/MODEL_LICENSES.json` (`audiocpp-qwen3-asr-v1`).

## 4. Experimental research modules (off by default)

```sh
cmake -S . -B build/experimental -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DIBAUDIO_BUILD_TESTS=ON -DIBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=ON
cmake --build build/experimental --parallel
python3 scripts/check_abi.py build/experimental/libibaudio.so abi/ibaudio_symbols_v1.txt
```

**Expected:** builds cleanly, `ABI OK: 94 exported ibaudio_* symbols`. The 15 extra symbols are the gated placeholder modules (`neural_codec`, `prosody_controller`, `voice_clone_engine`).

## 5. MCP gateway smoke test

```sh
echo '{"jsonrpc":"2.0","id":1,"method":"server/discover"}' | build/linux-release/ibaudio-mcp
echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"audio.models","arguments":{}}}' | build/linux-release/ibaudio-mcp
```

**Expected:** a `server/discover` result naming `ibaudio-mcp` / protocol `2026-07-28`, and a model catalog listing the three available reference engines plus `kws-deferred-v1` with `available:false`.

## What a real discrepancy looks like vs harmless variance

- **Real problem:** a test fails, the ABI gate reports a mismatch, the sanitizer gate reports a leak/error, or the MCP gateway returns a JSON-RPC error. That is a genuine regression — report it.
- **Harmless variance:** benchmark mean times differ from `reports/` (CPU-dependent); sanitizer gate skipped on a platform without a sanitizer runtime; symbol *order* in `nm` output differs. These are environment differences, not defects.

## Honest boundaries (not defects)

- Reference ASR/TTS are a signal analyzer and a tone synthesizer — they do not recognize language or produce a natural voice. This is by design.
- Android/ARM64 are build-scaffold only; device validation is PENDING by design.
- Sarvam/AI4Bharat providers return UNAVAILABLE; no remote/network code runs. This is by design (offline-first).
