# 77 — Live Pendrive Acceptance: Vision Defect Found and Fixed (mmproj F16 → BF16)

**Date:** 2026-09-10
**Scope:** physical Pocket AI drive `D:\UNOONE` — desktop model + mmproj assets
**Posture:** live user-path acceptance testing, every result reproduced against
the drive's own runtimes and models.

## 1. What the live test caught

Driving the drive's llama-server exactly the way the desktop app does
(`--jinja`, chat completions, data-URL image parts), text chat is correct but
**every image request degenerated into repeated `<unused49>` tokens** until the
length limit — with the model clearly embedding the image (49 vision tokens
observed per image in a two-image delta test).

- Text: "17 * 23 is 391, and it is not divisible by 3" — correct, clean stop.
- Vision: `<unused49>` spam, finish reason `length`.
- Reproduced identically through the app's request shape, the OpenAI chat
  endpoint, and every raw `/completion` `image_data` framing variant — the
  raw path never embeds at all in the bundled b10075 build.

## 2. Root cause — the drive's F16 mmproj, not the runtime framing

This is upstream llama.cpp issue
[#24146 — Gemma 4 12B vision outputs `<unused49>` tokens](https://github.com/ggml-org/llama.cpp/issues/24146),
confirmed independently by
[Unsloth (HF discussion #10)](https://huggingface.co/unsloth/gemma-4-12b-it-GGUF/discussions/10):

- The `patch_embeddings_0` matmul is a 6912-wide reduction; F16 accumulation
  **overflows to Inf/NaN on high-contrast patches**, so the model receives
  garbage image embeddings and emits junk tokens.
- The drive's `mmproj-gemma-4-12B-it-f16.gguf` (sha256 `563192209F002B0A…`,
  122,031,552 B) carries exactly these F16 patch-embedding weights.
- The vision encoder itself is healthy in this build: `llama-mtmd-debug`
  encode passes, projector metadata parses clean (`gemma4uv`, projection_dim
  3840, image 224/16), and CUDA + CPU runtimes are byte-identical builds.

## 3. Fix applied — official ggml-org BF16 mmproj

Swapped the drive asset (nothing else changed — same runtime, same model):

| | Before | After |
|---|---|---|
| File | `mmproj-gemma-4-12B-it-f16.gguf` | `mmproj-gemma-4-12B-it-bf16.gguf` |
| Source | broken F16 patch-embedding revision | `ggml-org/gemma-4-12B-it-GGUF` (official) |
| Size | 122,031,552 B | 175,115,616 B |
| sha256 | `563192209F002B0A…` | `9B1EDFA05B634728CA4BFD60B4E6B278E95166C078FA54AE4FA83E680112FD1D` |

BF16/F32 patch-embedding weights do not overflow, per both upstream reports.
The drive manifest was regenerated (`New-UnoOneManifestV2.ps1 -Apply`) so the
integrity gate covers the new file; the app resolves the mmproj by manifest
kind `MMPROJ`, so no code change was needed. The broken F16 file was moved
off-drive (kept as evidence, never re-staged).

## 4. Live verification after the swap

Same test image (synthetic 256×256 PNG: red left half, blue right half, green
center stripe), same server binary from `RUNTIMES\WINDOWS\CUDA`, official
mmproj now read from the drive path:

> "The image features a vertical split layout with a large red section on the
> left and a large blue section on the right, separated by a thin green line
> down the center."

Correct colors, correct layout, `finish_reason: stop`. **Vision lane: PASS.**

## 5. Honest limits

- The runtime itself is stock llama.cpp b10075; the *soft-token budget* half of
  issue #24146 (resize alignment) lives in the runtime, not the mmproj. Our
  images embed at the reference 49 tokens/image, so that half did not bite —
  but a runtime update should be considered before claiming arbitrary-image
  robustness.
- Audio input through the same projector (`gemma4ua`) is untested live; the
  server flags it experimental upstream.