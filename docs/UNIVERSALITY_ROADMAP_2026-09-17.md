# Universality Roadmap — One Harness, One Audio Engine, One Tool Vocabulary, Every Device

**Date:** 2026-09-17
**Directive (user, verbatim intent):** the harness and audio.cpp must be universal for both mobile and desktop — one codebase, both platforms — and so must the tools usage.
**Trigger:** the mobile-version check found the staged phone APK predating the entire speech-language contract (fixed same day: fresh build staged from main `a7773fd`); the audit below records what is shared, what is mirrored, and what is still divergent.

## 1. Universality audit — where we actually stand (2026-09-17)

| Plane | Desktop (UnoOnePower) | Android (UnoOneAgent) | Verdict |
|---|---|---|---|
| Model | Gemma 4 12B via llama.cpp CPU server (vendored llama) | Gemma 4 E2B/E4B via LiteRT (`*.litertlm`, drive `MODELS/MOBILE/brain`) | Shared family, engine per platform by design (CPU server vs. mobile runtime) |
| Agent harness | vendored `inbharat-harness` (execution levels, budgets, audit ledger) via `pai-harness-adapter` | own Kotlin agent stack (`agentrouter`, `localbrain`, `skills`, `safetyguard`) — zero references to the harness crate | **DIVERGENT** |
| Audio engine | vendored `Inbharat-audiocpp` (OmniVoice TTS, Qwen3-ASR, CPU, 15 languages) | Sherpa-ONNX (`com.github.k2-fsa:sherpa-onnx:v1.13.3` JitPack AAR) | **DIVERGENT engines, ONE shared contract** |
| Speech language contract | `packages/speech-contracts/languages.v1.json` embedded in Rust (`include_str!`) | `VoiceLanguage.kt` `CANONICAL_ALIASES` mirror | **UNIVERSAL** — CI-enforced by `scripts/check_speech_language_sync.py` (drift fails CI) |
| Tool vocabulary | 26+ harness tools (fs, process.run, workspace.search/patch, browser.act, vision, TTS) at L1/L2/L3 | 13 atomic Kotlin tools (`phonecontrol`: BlindAid, Calendar, EmailDraft, OCR, Screenshot, Packages, document…) | **DIVERGENT** — no shared tool contract |
| Vault / memory | encrypted vault, model-backed rerank | own `vault`/`memory` modules, same concepts | conceptually parallel, separately implemented |
| Source of truth | `vendor/inbharat-harness` + `vendor/Inbharat-audiocpp` staged in `SOURCE/PAI.V2` from final main | same vendored trees present on the drive — but the Android build does not yet compile from them | trees universal, **builds not yet** |

**The pattern that already works:** the speech-language contract proves the mechanism — one JSON source of truth, an embedded implementation on each platform, and a CI checker that fails the build if either side drifts. That is the template for everything below.

## 2. The plan — contract-first, engine-second, harness-third

### Phase U1 — Tool-contract universality (contract, CI, cheap) — NEXT
One `tools.v1.json` in `packages/tool-contracts/` (mirroring `speech-contracts`):
- canonical tool registry: id, name, description, parameter schema, safety class — covering BOTH the desktop harness tools and the Android atomic tools, with per-platform availability flags
- Rust embed (`include_str!`) consumed by the harness adapter; Kotlin mirror object consumed by `phonecontrol`
- `scripts/check_tool_contract_sync.py` — fails CI on drift, exactly like the speech checker
- Android tools get canonical ids so a user's muscle memory ("ask it to read this aloud", "open my calendar entry") behaves identically on both devices

### Phase U2 — Audio engine universality (build audiocpp for Android)
- Add Android NDK targets (arm64-v8a first, armeabi-v7a second) to the vendored `Inbharat-audiocpp` build (CMake; the tree already builds desktop MSVC + Linux CI, so the C++ is portable; JNI surface is the new work)
- Wire `VoiceService`/TTS+STT engines behind the SAME Kotlin engine interface, with `InbharatAudioEngine` as primary and Sherpa-ONNX as automatic fallback when an NDK `.so`/model is unavailable
- Same `languages.v1.json` table drives both engines — the CLI-vocabulary translation (defect #43) moves into the shared contract layer so it can never diverge again
- Acceptance: the 19/19 speech matrix passes on the SAME OmniVoice/Qwen3 models on desktop AND phone — one engine, one voice, fifteen languages, every device

### Phase U3 — Harness universality (the big one)
- Expose the vendored harness core (routing, execution levels, budgets, the signed audit ledger) over a C ABI / UniFFI surface compiled for Android
- `agentrouter` keeps its Kotlin face but delegates execution discipline — levels, budgets, ledger — to the SAME Rust core the desktop uses; Android tools become harness tool-providers through the U1 contract
- The phone inherits the desktop's audit guarantees (signed ledger, fail-closed sandbox policy) instead of re-implementing approximations
- Acceptance: identical budget/audit behavior proven by one shared test contract run on both platforms

### Phase U4 — Drive-level truth (already partially shipped)
- The phone APK must be rebuilt and re-staged on every main that touches `android-app/` or the vendored trees — the same end-to-end lane the desktop bundle has (branch → CI → merge → bundle → stage → live re-test). This check caught a real defect on 2026-09-17: the staged APK was six weeks stale, predating the speech-language contract entirely.

## 3. What shipped 2026-09-17 (this pass)

- Staged phone APK was **stale**: built 2026-08-05, contained no trace of the canonical speech-language contract (`hinglish` absent from every dex) while the repo's `voice` module had moved through five commits (canonical mirror, voice-stack A1–A8, A9 install-blockers, defect #43, alias-pin-48).
- Fresh `assembleDebug` built from final main `a7773fd`; `VoiceLanguageCanonicalizeTest` 13/13 pins the 48-alias contract; APK verified to carry the contract (`hinglish` present in dex).
- Staged to `D:\UNOONE\APPS\ANDROID\UnoOne.apk` with the staging convention (old copy backed up under `RECOVERY\package-backups\20260917T…\`), SHA-256 source↔dest identity confirmed, full-drive `Start UnoOne.exe --verify-only` re-run **valid, 0 failures**.
- Auto-launch mechanism root-caused and restored after a drive replug went silent: the tray dock (the only insertion watcher — Windows blocks raw USB AutoRun) had been killed during an app relaunch, and its `HKCU\...\Run` entry only fires at logon. Dock restarted → it detected the validated drive and launched the app immediately, proving the mechanism end-to-end.

## 4. Open items
- Live on-phone acceptance (install the fresh APK, run the speech matrix + blind-aid lanes on the handset) — needs the device connected; no phone was attached during this pass.
- U1–U3 as scheduled engineering work; U1 is deliberately first because it is contract-only, low-risk, and unblocks the vocabulary for both U2 and U3.