# InBharat Audio Speech Architecture (PAI V2)

Status: audited + hardened 2026-08-31. This document is the plain-English
contract for how speech works across PAI V2 / UnoOne today, what it honestly
does, and what it must never claim. It exists so that no surface — UI, docs,
marketing, manifest, or commit message — can overstate capability that the
assets on disk do not back.

## One architecture, five platforms

**InBharat Audio is an independent universal speech runtime** living in
`vendor/Inbharat-audiocpp/`. PAI V2 / UnoOne is only a *consumer* of it —
nothing in this repository may redesign it as a PAI-specific library. The
same runtime contract serves Windows (HOST-verified), macOS/Linux
(CI/SOURCE), Raspberry Pi/ARM64 (SOURCE), and Android (BUILD-ONLY scaffold
today).

The runtime is pinned to the reviewed audio.cpp checkout
(`third_party/audio_cpp`, commit `26dcb5c4cf5aa016ae6285096a7b45f2671e5d17`).
A CMake configure gate fails the build on pin mismatch or a dirty checkout.
Do not touch the pin without a full re-review of the upstream tree.

## Plane map — who actually speaks today

| Plane | Engine | Evidence tier | Notes |
|---|---|---|---|
| Windows desktop product ASR/TTS | Qwen3-ASR + OmniVoice via `audiocpp_cli` | HOST (real acceptance runs recorded in `SPEECH/acceptance/`) | The only PHYSICAL-verified speech path |
| Windows desktop legacy voice | Whisper (base.en) + Piper behind `LegacyVoiceBackend` | HOST | Kept, wrapped, policy-gated — see below |
| Android app | Sherpa-ONNX (transducer / omnilingual / VITS-MMS) | SOURCE + CI unit tests; no device evidence claimed | The app's real engine today |
| Vendor Android scaffold (`libibaudio_jni`) | C++ cross-compile | BUILD-ONLY (NDK r25.1, arm64-v8a, plain + ASan/UBSan) | Migration target, not wired into the app |
| IndicConformer (Assamese + 21 more) | sherpa-onnx seam, family `indicconformer-asr` | SOURCE (seam only — runtime not linked, always UNAVAILABLE) | See truthfulness rules |
| Raspberry Pi | same CLIs via `distribution/pocket-ai-pi` scripts | SOURCE | Not executed in this cycle |

## Streaming semantics — stated plainly

The desktop InBharat Audio route is **`buffered-final`**. The engine buffers
the complete utterance and emits one final transcript. It is **not**
stateful streaming: there are no incremental partial results, and nothing in
UI or docs may use the word "streaming" about this path. The library's
`ibaudio_stream_*` API and cancellation token are not on the product path
yet; the Rust route drives the CLI with a deadline kill
(`run_command_timeout`). `StreamingClass` in `packages/speech-contracts`
exists precisely so every backend declares what it really is
(`stateful-streaming` / `segment-chunked` / `buffered-final`) and
`BharatAudioStatus.streaming_class` surfaces that string to the UI unchanged.

## Desktop integration route — compat now, C ABI next

Today the desktop app talks to InBharat Audio by **spawning two CLIs**:
`ibaudio` (status/readiness) and `audiocpp_cli` (inference). This is the
compatibility route: simple, fail-closed, reviewable. The target route is
loading `ibaudio.dll` through the stable C ABI directly (the vendor
publishes `abi/ibaudio_symbols_v1.txt`; desktop export checks against the
core manifest already pass 80/80 on Windows). Migration rules:

1. All speech code goes through the `SpeechBackend` trait
   (`packages/speech-contracts`); recording/UI/agent code never sees a CLI.
2. The CLI implementation stays the fallback behind the same trait; the
   C-ABI implementation replaces it without touching callers.
3. Gate order is a security property and must survive any migration:
   manifest checks → `verify_pocket_ai_package` (HMAC) → `verify_acceptance`
   (SHA-256 of both runtime CLIs and both model trees against the acceptance
   attestation) → **only then** the first CLI spawn (`query_readiness`) →
   `check_runtime_status` (adapter compiled + commit match + runtime-probed
   `inference_ready`). No executable is spawned before its bytes and the
   model bytes have been hash-verified — the acceptance check runs BEFORE
   the spawn, not after. (Acceptance-gate digests are memoized per
   (path, size, mtime) with a TTL so an unchanged 2.5 GB model tree is not
   re-read on every request; any change to a file is a cache miss and a full
   re-hash.)

## Readiness is a runtime fact, not a build fact

`inference_ready` is derived at runtime by probing the configured model
roots for actual weights (`.gguf` for Qwen3-ASR — the loader is
extension-only — and any regular file for the Silero VAD root). A binary
compiled against the adapter with no models on disk reports
`inference_ready=false` with a named reason ("not configured", "missing or
not a directory: …", "contains no .gguf weights: …"). No code path may
report readiness from a compile-time `#ifdef`. Production gates fail closed
on all three facts: adapter compiled, pinned commit match, and
runtime-probed readiness.

## Language truthfulness rules

Single source of truth: `packages/speech-contracts/languages.v1.json`,
mirrored in Android's `VoiceLanguage.kt` and enforced by
`scripts/check_speech_language_sync.py` in CI. The rules:

- Aliases: `as`/`as-IN` → `as-IN`; `hi` → `hi-IN`; `hinglish` →
  `hi-en-codemix`; `en` → `en-IN` (PAI alias). Matching is ASCII
  case-insensitive.
- Unknown tags pass through with BCP-47 case normalization and are **never
  re-rooted to a region**: `fr` stays `fr`, `en-US` stays `en-US`.
- Malformed or empty language input is rejected, never guessed, never
  silently bypassed.
- `auto` is the reserved detect sentinel; no provider may claim it.
- Provider coverage is truthful capability, not aspiration. **Qwen3-ASR
  covers Hindi (and hi-en-codemix) but NOT Assamese.** Assamese routes to
  the IndicConformer family (`indicconformer-asr`), which fails closed
  (UNAVAILABLE) until real local `.onnx` assets ship. Editing a manifest
  line can never add coverage — only the table plus shipped assets can.

## Assamese status, honestly

Assamese has a language pack (`packs/as-IN/`), a canonical route
(`indicconformer-asr`, with `sherpa-onnx-indicconformer` as the leading
STT candidate), and a seam provider behind `IBAUDIO_ENABLE_SHERPA_ONNX`
(OFF by default). The native sherpa-onnx runtime is **not linked** this
cycle: `run_asr` returns `IBAUDIO_STATUS_UNAVAILABLE` even when assets are
present. **Assamese speech-to-text does not work yet.** Any surface claiming
otherwise is a bug.

## Legacy Whisper/Piper status — stated plainly

The legacy Whisper/Piper plane is **still active on the desktop product**
as the policy-gated fallback: `SpeechRouter` selects
`LegacyVoiceBackend` **only** when the InBharat gate has failed AND the
explicit policy permits legacy. It now behaves like the rest of the system:
deadline timeout on the whisper subprocess (no unbounded `.output()`),
binary/model hash verification (not just `Path::exists()`), and errors
returned as `error:` — never smuggled into transcript text. The pendrive
`RUNTIMES/WINDOWS/VOICE` plane ships those binaries and they are hashed in
the manifest. Legacy is a compat route, not the target: it stays until the
InBharat route covers its use cases, then it is deleted, not expanded.

## Package integrity for speech assets

Everything speech-related on the drive is now integrity-bound
(`platforms.windows.speech` in the schema-v2 manifest: SPEECH_MODEL,
SPEECH_CONFIG, ACCEPTANCE, SPEECH_RUNTIME kinds). Per the background-
verification policy, the fast `PackageIdentity` launch path hashes **none**
of the speech assets; the desktop app's background `DesktopLaunch` sweep
hashes all required ones and gates inference on the result. A swapped
`qwen3-asr.gguf` or an edited acceptance attestation is detected there.

## What counts as evidence

Every speech claim must carry its tier: SOURCE (written, never run),
BUILD-ONLY (compiled for a target, never executed), CI (executed on a
runner), EMULATOR, HOST (executed on this machine), PHYSICAL device run
(named device, date, log). Nothing in this cycle claims PHYSICAL Android
support. The per-toolchain evidence table lives in
`docs/SPEECH_TEST_EVIDENCE.md`.