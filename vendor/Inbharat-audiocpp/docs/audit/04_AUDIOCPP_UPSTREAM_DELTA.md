# 04 — audio.cpp Upstream Delta (computed 2026-08-20)

Method: live `git ls-remote` + blob-less full-history clone of the real upstream into `/agent/workspace/audiocpp-upstream`; deltas computed with `git log`/`git diff` against the actual commits, not release-note prose.

## Pin verification

- InBharat pin: `bb15edd78b56e035967e0eb999a6b28a62337db4`, recorded upstream URL `https://github.com/ShugoAI/audio.cpp.git`, tag `release-0.6`.
- **The recorded URL no longer resolves** (org `ShugoAI` not publicly resolvable; anonymous git prompts for auth). The real upstream is **`https://github.com/0xShug0/audio.cpp`** (ShugoAI LLC's user account).
- Live `git ls-remote` confirms `refs/tags/release-0.6` = `bb15edd78b56e035967e0eb999a6b28a62337db4` exactly. **The pin is intact and matches upstream release-0.6 — no drift in what the pin points at.** `scripts/check_upstream_pin.sh` should be updated to the `0xShug0` remote or it cannot re-verify.

## Where upstream has moved

| Ref | Commit | Commits past pin |
|---|---|---|
| `release-0.6.1` | `26dcb5c4cf5aa016ae6285096a7b45f2671e5d17` | 57 |
| `main` (0.7 preview in progress) | `aec444c6007fbd3b5f57074c60875eb4e54ef8f1` | 72 |

`bb15edd7..release-0.6.1`: 205 files changed, +13192/−16563 (includes removal of the legacy 5453-line Python WebUI).

## Release-0.6.1 changes relevant to InBharat

Grouped from the 57-commit log; per-commit hashes available in the local clone.

- **ASR correctness (India-relevant):** `17e176e` Fix Qwen3-ASR language option propagation; `502b5b7` drop out-of-span chunk word timestamps. Qwen3-ASR covers `hi` and ~30 other languages — directly relevant to the en-IN/hi-IN/hinglish packs.
- **Server/API:** `eaede92` over-large requests → 400 not 500; `424dd33` recognition prompt on multipart transcriptions; `c86c14e` base64 voice reference in speech requests.
- **Backends/platforms:** `df6e948` Vulkan backend support for Windows; `8f750fb` TheRock LLVM layout in Windows HIP build; `5f620b0` fix core dump on old GPUs (build_linux.sh); `1152b7f` macOS CPU build docs; `cef76b0` classify ROCm as HIP in tests.
- **TTS fixes:** `52080cd` IndexTTS2/2.5 text normalization; `9a5eb86` IndexTTS-2 F16 weight storage; `0f7b14f` Qwen3-TTS prompt modes without ICL reference codes; `4154f7d` reset PocketTTS decoder cache per request; `52b4e2c` IndexTTS2 duration_factor speech-rate control.
- **New models:** MiniMax Music3 preview (+ several fixes), DotTTS edit inference, ACE-Step 1.5 XL DiT variants; `3407559` pre-0.7 framework building blocks; `04ba437` reusable framework runtimes.
- **WebUI:** native-UI-only (legacy Python WebUI removed), many native UI/model-manager fixes.

## main (0.7 preview) beyond 0.6.1

`e9a36d4` **asr: allocate inference graphs with ggml_gallocr, not alloc_ctx_tensors** (memory-allocation fix relevant to the A7 memory-budget blocker); `6ed7225` reusable native model package manager; PersonaPlex + release-0.7 preview models; Fish Audio multi-reference conditioning; Metal/Vulkan fixes.

## Delta vs what InBharat needs

| Area | Pin (0.6) | Current main | InBharat impact |
|---|---|---|---|
| Model families | 49 | 49 + 0.7 preview adds | Wider provider catalog |
| Qwen3-ASR language routing | buggy | fixed at 0.6.1 | **Adopt 0.6.1 fixes before any hi-IN pack relies on Qwen3-ASR** |
| Streaming transcript output | restates transcript | emits deltas (0.6.1) | Align InBharat streaming labels with delta semantics |
| Windows Vulkan | absent | present (0.6.1) | Relevant to Windows lane |
| ASR graph allocation | alloc_ctx_tensors | ggml_gallocr (main) | Addresses A7 memory blocker; evaluate |
| Stable C ABI | none | none | A1 blocker stands — provider must wrap upstream behind the InBharat C ABI regardless of pin |

## Recommendation

Do **not** jump to `main` (0.7 preview, moving target). Evaluate a controlled move from `release-0.6` to **`release-0.6.1`** as a single reviewed step: it is a tagged point-release containing the Qwen3-ASR language-propagation and streaming-delta fixes that the India packs will depend on. The A1 (no stable C ABI) blocker is unaffected by the pin move and remains the provider layer's job. Per project rule, the pin does not move without the user's explicit decision — this report is the input to that decision, not the decision.
