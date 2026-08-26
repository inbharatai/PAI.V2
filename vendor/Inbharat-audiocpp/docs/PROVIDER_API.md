# Provider API

The provider layer is what makes InBharat Audio a *universal* runtime rather than an audio.cpp fork. Applications call the stable C ABI; the C ABI dispatches to a provider; a provider wraps an engine. audio.cpp is one provider, not the product.

## Contract

A provider is an internal C++ class (`src/provider.hpp`) implementing:

- `capabilities()` → `ProviderCapabilities`: id, version, locality (`local-native` / `local-service` / `remote`), privacy class, evidence-backed languages, task support, and **honest** streaming flags.
- `run_asr` / `run_tts` / `run_vad` — cooperative-cancellation entry points. Default implementations return `IBAUDIO_STATUS_UNSUPPORTED`, so a provider only overrides the tasks it genuinely performs.

The layer is internal C++ only; nothing crosses the C ABI, so it can evolve without breaking ABI v1.

## Registry and routing

`ProviderRegistry` (src/provider.cpp) holds the registered providers. Registration order encodes priority. `route(task, language, require_streaming, remote_allowed)` returns the highest-priority provider that satisfies every constraint, or `nullptr` when none qualifies.

Hard rules:

- **No silent cloud fallback.** A `remote` provider is never a candidate when `remote_allowed` is false. The caller gets `UNAVAILABLE`/`DEFERRED`, not a surprise network call.
- **Capability is evidence, not filenames.** A provider may only assert a language/platform/streaming flag when the recorded evidence level supports it (see `schemas/capability-manifest.v1.schema.json`).
- **No runtime dynamic loading in v1.** Providers are compile-time built-ins or explicitly-enabled adapters, keeping the supply chain closed.

## Current providers

| Provider | Locality | What it is | Evidence |
|---|---|---|---|
| `reference` | local-native | Built-in deterministic engines (signal analyzer / tone synthesizer / energy VAD) | host-tested |
| `audiocpp` | local-native | Pinned audio.cpp adapter — scaffold only, `availability() == DEFERRED` | source-scaffold |
| `ai4bharat` | local-service | IndicConformer ASR / IndicF5 TTS via a controlled local service — spec only, no Python in core | spec |
| `sarvam` | remote | Saaras v3 STT / Bulbul v3 TTS — gated behind `IBAUDIO_REMOTE_PROVIDERS=OFF`; BLOCKED in sandbox | spec, blocked |

## Model → provider resolution

`ibaudio_model` carries a `Provider*` resolved at load time. Resolution goes through the capability router: `resolve_provider_for` (in `src/facade_util.cpp`) calls `ProviderRegistry::resolve_for_family(family, remote_allowed)`, where each provider declares the families it backs via `serves_family`. The router enforces the runtime's remote policy (`ibaudio_runtime_options_v1.allow_remote_providers`, default **0 = offline**): a remote provider is never returned when remote is disallowed, even if it backs the family. A model with no resolved provider fails inference with `UNAVAILABLE` — never a guess, never a silent cloud fallback. All six inference call sites (sync ASR/TTS/VAD, async jobs, streaming partials/final) dispatch through the provider vtable; the reference engines remain the built-in provider so existing behavior is unchanged. The `ibaudio.provider` lane covers both the public-ABI roundtrips and an anti-rot test (`test_remote_gate`) that asserts the remote gate against the live registry with local+remote stubs.
