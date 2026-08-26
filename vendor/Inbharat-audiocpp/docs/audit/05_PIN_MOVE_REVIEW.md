# 05 — Pin-Move Review: audio.cpp release-0.6 → release-0.6.1 (2026-08-20)

Decision input for moving the InBharat audio.cpp pin from `bb15edd7` (release-0.6) to `26dcb5c4` (release-0.6.1). Method: live clone of the real upstream (`github.com/0xShug0/audio.cpp`), `git log`/`git diff` between the two tags, and a real 0.6.1 build in this sandbox. This report is a recommendation, not the move — the pin changes only on the user's explicit approval.

## What changed (57 commits, 205 files, +13192/−16563)

Grouped by relevance to InBharat:

- **India-relevant ASR fix (the decisive one):** `17e176e` *Fix Qwen3-ASR language option propagation* — adds the language-option pass-through plus a unit test. InBharat's hi-IN / hinglish packs will rely on Qwen3-ASR (covers `hi` and ~30 languages); on 0.6 the language option is dropped, on 0.6.1 it propagates. **This directly affects the correctness of the India packs.**
- **Streaming semantics:** `502b5b7` drop out-of-span chunk word timestamps; the 0.6.1 streaming transcript output emits deltas instead of restating the transcript. Aligns with InBharat's honest streaming labels.
- **Server/API correctness:** `eaede92` over-large requests → 400 not 500; `424dd33` recognition prompt on multipart transcriptions; `c86c14e` base64 voice reference.
- **Backends/platforms:** `df6e948` Vulkan backend for Windows; `8f750fb` Windows HIP (TheRock LLVM); `5f620b0` fix core dump on old GPUs; `cef76b0` classify ROCm as HIP; `1152b7f` macOS CPU build docs.
- **Build weight:** composite model-set selection is present — `AUDIOCPP_MODEL_SET = full|core|custom` with `AUDIOCPP_MODELS` for a custom set. This is the lever for shipping only the model families InBharat needs.
- **TTS fixes:** IndexTTS2/2.5 text normalization (`52080cd`), F16 weight storage (`9a5eb86`), Qwen3-TTS prompt modes (`0f7b14f`), PocketTTS decoder cache reset (`4154f7d`), IndexTTS2 duration_factor (`52b4e2c`).
- **New models:** MiniMax Music3 preview, DotTTS edit inference, ACE-Step 1.5 XL DiT.
- **WebUI:** legacy 5453-line Python WebUI removed; native UI fixes. (Not used by InBharat.)

## Compatibility with InBharat

- **Public engine entry surface InBharat binds to: unchanged.** `git diff` of the top-level public engine headers (excluding framework internals, model-specific, and community-model headers) between the tags is empty — 0 files. The adapter's binding surface is stable across the move.
- **CMake delta** is additive: three framework sources added (`nemo_nano_codec.cpp`, `flow_sampler_runtime.cpp`, `flow_kv_cache.cpp`), the `minimax_music3` model added, and `base64.cpp` added to the server. No removal of anything InBharat references.
- **The A1 blocker stands regardless of pin:** audio.cpp still has no stable C ABI and exposes STL/exceptions across its API — that is the provider layer's job to wrap, unchanged by 0.6 → 0.6.1.

## Build evidence in this sandbox

release-0.6.1 was checked out (`git worktree`, commit `26dcb5c4`) and built with `AUDIOCPP_MODEL_SET=core`, Release, on g++ 11.5: configure clean (ggml 0.12.0, CPU backend, OpenMP found), and `bin/audiocpp_cli` (4.4 MB) linked and ran `--help` with the correct task/backend surface. **This is configure/build-only evidence** — no model was downloaded and no inference was run, so no inference-support claim is made.

## Risk assessment

- **Low risk to InBharat.** It is a tagged point release, the public binding surface is unchanged, and the build is confirmed in-environment. The main content is model additions, WebUI replacement, and targeted bug fixes.
- **One thing to watch:** 0.6.1's streaming output now emits deltas; InBharat's streaming labels should be re-confirmed against the provider when the audio.cpp provider is implemented.

## Recommendation

**Adopt release-0.6.1.** The Qwen3-ASR language-propagation fix is directly load-bearing for the hi-IN/hinglish packs, the public binding surface is unchanged, and the build is confirmed here. Do **not** chase `main` (0.7 preview — a moving target).

## Steps if approved

1. Update `IBAUDIO_PINNED_AUDIO_CPP_COMMIT` to `26dcb5c4cf5aa016ae6285096a7b45f2671e5d17` and `third_party/audio_cpp/UPSTREAM_COMMIT` (tag release-0.6.1).
2. Re-run the pristine-pin adapter gate (configure + wrong-pin rejection) against a fresh 0.6.1 checkout.
3. Re-run the full InBharat clean-rebuild gate and re-record evidence.
4. Update `docs/audit/04_AUDIOCPP_UPSTREAM_DELTA.md` to mark the move done.
