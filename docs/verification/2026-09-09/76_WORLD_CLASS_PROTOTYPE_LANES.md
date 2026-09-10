# 76 — World-Class Prototype Lanes: Vision, Long Context, Host Cache, Playwright

**Date:** 2026-09-09
**Scope:** `apps/desktop` (+ `packages/pai-harness-adapter`)
**Posture:** prototype — showcase every capability, host-adaptive so weak hosts never break.

## 1. Vision lane (mmproj multimodal chat)

Chat now accepts images. The shipped drive carried
`mmproj-gemma-4-12B-it-f16.gguf` when this lane landed; the 2026-09-10 live
acceptance test caught that projector producing garbage vision output
(`<unused49>` spam) and it was replaced with the official BF16 mmproj — see
`docs/verification/2026-09-10/77_VISION_MMPROJ_BF16_FIX.md`. This lane wires
the mmproj into the harness chat
plane:

- **UI** (`ChatView.tsx`): paperclip button → up to 4 images (png/jpeg/webp/gif)
  as data URLs, thumbnail previews, removable before send.
- **Bridge** (`harness_bridge.rs` `harness_chat`): `parse_image_attachments`
  re-validates each data URL — media-type allowlist, base64 decode, 8 MiB per
  image, ≤ 16 MiB data URL, sha256 digest — and renders
  `AttachmentMetadata` for the harness pipeline.
- **Provider** (`pai-harness-adapter/src/llama_local.rs`): attachments are
  held as base64 bytes (`with_attachment` builder) and the **last user
  message** is rendered as OpenAI multimodal `image_url` parts when the
  request carries attachments. Missing bytes fail closed.

The attachment still flows through the non-bypassable harness pipeline
(validate → authorize → confirm → budget → sandbox → execute → bound → verify)
with metadata-only `AttachmentMetadata` at the boundary, exactly as the
harness core designed it.

## 2. Long context (8K → 32K, host-adaptive)

- `ModelConfig` gains `cache_type_k` / `cache_type_v` / `flash_attention`,
  all `#[serde(default)]` — old configs and the startup path deserialize
  unchanged.
- llama-server spawn passes `-ctk` / `-ctv` / `-fa` when set. Invalid values
  are skipped, never fatal (a config typo must not take down model loading
  on any host).
- Context select now offers 32768. Choosing ≥ 16384 auto-suggests
  `q8_0/q8_0` KV caches so a 4 GB-VRAM laptop (RTX 5050 class) fits the
  window; quantized V requires flash attention, which the shipped b10075
  server enables via `-fa auto` by default.
- Verified against the live drive binary: `llama-server.exe --help` shows
  `-fa [on|off|auto]`, `-ctk`, `-ctv`, `--mmproj`.

## 3. Speed: host-disk model cache

Launching a 7 GB Q4 model off a USB 2/3 pendrive is minutes of sequential
read. The cache streams the model to the host disk **once**:

- Cache dir: `%LOCALAPPDATA%\UnoOne\model-cache` on Windows (macOS
  `~/Library/Caches/UnoOne/model-cache`, Linux `$XDG_CACHE_HOME/UnoOne/model-cache`),
  entries keyed
  `<manifest-sha256>.gguf`.
- Staging is a **single pass**: stream-copy while hashing, digest compared
  to the manifest sha256, size compared to the source, then atomic rename
  plus a `<sha>.verified` marker storing `size:mtime` so re-verification is
  skipped while the file is unchanged.
- **Fail closed** without a manifest hash — unverified model bytes never
  reach the host disk.
- Identity stays enforced: `read_manifest_model_hash` recognizes cache
  paths (the filename *is* the manifest hash), so `MODEL_IDENTITY_POLICY =
  Strict` still requires the startup disk hash to match before the server
  is trusted.
- Commands: `model_cache_status` (cheap probe — manifest + two stats) and
  `stage_model_cache` (heavy, `spawn_blocking`).
- Model Manager shows staged status with a "Stage to fast local cache"
  button and launches from the cached copy when present.

## 4. Playwright automation through the exec lane — VERIFIED

The full-access lane (PR #2) runs allowlisted direct-argv programs inside
`%USERPROFILE%\UnoOneAgent`. `node`, `npm` and `npx` are allowlisted, and
the harness broker resolves `npx` → `npx.cmd` via `PATHEXT`, so real
Playwright automation is reachable from agent tool calls.

Live evidence (2026-09-09, this host, real network + real browser):

```
cd %USERPROFILE%\UnoOneAgent\playwright-verify
npm install playwright          # 2 packages
npx playwright install chromium # Chrome Headless Shell 153.0.8010.12
node verify.js
```

Result — navigate, read DOM, fill a form field, click a button, read the
JS-mutated DOM:

```json
{
  "navigated": "https://example.com",
  "title": "Example Domain",
  "heading": "Example Domain",
  "form_fill_and_click_result": "Hello, UnoOne Pocket AI!",
  "verdict": "PASS"
}
```

The exact commands above (`node verify.js` in the fenced workspace) are
what a `process.run` tool call executes, so the desktop agent can drive
real web automation end-to-end today.

## Honest limits

- Vision needs the mmproj sidecar present (drive has it); without it the
  server starts text-only and image sends fail with a clear error.
- 32K context on a 4 GB-VRAM host still trades KV precision (q8_0) and
  may spill to CPU — host-adaptive defaults pick the safe path.
- The model cache stores plaintext model bytes on the host disk. The
  *vault* (user data) remains encrypted; the public model file alone is
  cached, keyed and verified against the drive manifest.
- Playwright browsers download on first use (~115 MB Chromium); the
  `ms-playwright` cache lives under `%LOCALAPPDATA%` on the host.