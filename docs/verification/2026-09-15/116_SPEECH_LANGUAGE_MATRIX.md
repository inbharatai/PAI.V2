# 116 — The Speech Language Matrix: Hinglish Was Allowed but Unpronounceable, and the Contract Under-claimed the Engine by 12 Languages

**Date live-caught:** 2026-09-15 (physical drive D:\UNOONE, staged main `a712f29` — caught by the speech-matrix acceptance run, the user-directed full STT/TTS/STS sweep across every language)
**Severity:** High — a language the pack explicitly allows (`hinglish`) failed at the CLI boundary; and the provider table claimed 3 TTS languages when the staged engine verifiably speaks 15.

## 1. Live catch

The speech-matrix run (direct Tauri invokes, zero model tokens) proved the loop live per language — TTS writes a real WAV, ASR reads it back, anchors grade the roundtrip:

- `en`: TTS ok (WAV on disk), ASR roundtrip ok (harbor/windows/dawn/report all present)
- `hi`: TTS ok, ASR roundtrip ok (भारत/दिल्ली present)
- `hinglish`: **TTS FAILED** — `unsupported OmniVoice language 'hinglish'`

Forensics: `bharat_audio` passed the **raw alias string** to `audiocpp_cli`, whose `--language` accepts only **base ISO codes** (`en`, `hi`, `as`, …) plus a set of **built-in language names** (`Hindi`, `Nepali`, …). `hinglish` is neither; `hi-IN`/`en-IN`/`as-IN` would all have failed identically; only bare `en`/`hi` ever worked — by coincidence of shape, not by design.

## 2. Live probe of the engine's real vocabulary (evidence, not docs)

Probed the staged `omnivoice.gguf` through `audiocpp_cli` directly, one invocation per candidate string:

- **ISO codes accepted:** `en`, `hi`, `as`, `bn`, `gu`, `kn`, `ml`, `mr`, `pa`, `sa`, `ta`, `te`, `ur` — **15 languages total**
- **Built-in names accepted:** Hindi, Bengali, Tamil, Telugu, Marathi, Gujarati, Kannada, Malayalam, Assamese, Urdu, **Nepali, Odia**
- **ISO `ne` and `or` REJECTED** — Nepali and Odia are reachable only by their built-in names
- Name "Punjabi" rejected, but ISO `pa` works
- **Hinglish→hi roundtrip proof:** Hinglish text (`Mera naam Pocket AI hai…`) TTS'd with `hi` reads back through ASR as correct Hindi — `hi-en-codemix` is honestly served by the Hindi voice, exactly as the provider table already claimed
- **Qwen3-ASR is permissive** (accepts any language string; transcription itself is en/hi/Hinglish-quality) — its table claim stays 3 languages, unchanged

## 3. Root cause

Two defects, one boundary:

1. **CLI vocabulary mismatch (#43):** the canonicalization layer (`hi` → `hi-IN`, `hinglish` → `hi-en-codemix`, `ne` → `ne-IN`) had no inverse translation back into the CLI's own vocabulary. The contract layer grew aliases over time; the CLI boundary never did.
2. **Contract under-claim:** `languages.v1.json` claimed `omnivoice_tts` = [en-IN, hi-IN, hi-en-codemix] while the staged engine verifiably speaks 15 languages. Per the evidence-before-support doctrine this was *safe* under-claiming, but evidence now exists — 12 languages were stuck behind a claim nobody had probed for.

## 4. Fix

- **`bharat_audio.rs`:** new `cli_language_for(tag)` — canonical BCP-47 → CLI vocabulary: `hi-en-codemix` → `hi` (roundtrip-proven), `ne-IN` → `Nepali` and `or-IN` → `Odia` (probed: ISO rejected, name required), `auto` passes through, otherwise the ISO base code (`hi-IN` → `hi`). Wired into **both** the transcribe and synthesize call sites (default-language path and user-language path).
- **`languages.v1.json` (provider table = truthful capability):** `omnivoice_tts` now claims all 15 live-probed languages (13 new `-IN` tags) with the probe evidence recorded in the table notes; `qwen3_asr` unchanged (3 languages — no new evidence); aliases added for all 12 new languages (`ta`/`ta-in`/`ta-IN` → `ta-IN`, …) so users can request them naturally; `VoiceLanguage.kt` CANONICAL_ALIASES mirror updated in the same commit (`check_speech_language_sync.py` green, 48 aliases).
- **Tests re-pinned to the new truth:** `resolved_key_agrees_with_provider_serves` (speech-contracts) and `provider_coverage_is_enforced_on_the_inbharat_route` (desktop) previously asserted omnivoice does NOT serve Assamese — live evidence refuted that; both now assert Assamese is served and a never-probed global tag (`fr`) stays refused.
- **Chat picker (`ChatView.tsx`):** expanded from 3 to 16 options with native-script labels; the mic passes `auto` (engine-side detect) when the selected voice language isn't ASR-served, so recording never breaks in a TTS-only language.
- **Repo `SPEECH/config/inbharat-audio.v1.json` template:** `allowed_languages` expanded to the same set (`enabled:false` unchanged in the repo; the drive copy is the pack gate).

## 5. Tests

- Desktop: 139/139 green (`cli_language_lives_in_the_engines_vocabulary` pins all 14 mapping cases, including `ne-IN` → `Nepali` and `or-IN` → `Odia`).
- speech-contracts: 18/18 green; `check_speech_language_sync.py` PASS (48 aliases, both directions).
- `cargo fmt` + `cargo clippy --all-targets -- -D warnings` green (desktop + contracts); frontend `oxlint` + `vite build` green.

## 6. Live acceptance (post-merge, on the re-staged drive)

**Executed 2026-09-15, drive re-staged with main `534cf6a`** (PR #47 merged; drive SPEECH config allowlist expanded to the same 16 tags before manifest regeneration). Direct Tauri invokes (zero model tokens) — final matrix: **19 pass / 0 fail**:

- [x] `en`: TTS PASS (190 KB WAV), ASR roundtrip PASS (harbor/windows/dawn/report all present in the transcript)
- [x] `hi`: TTS PASS, ASR roundtrip PASS (भारत/दिल्ली present)
- [x] `hinglish`: TTS PASS (**was the defect**) and ASR roundtrip PASS — live-probed bonus: the Hindi voice speaks the Hinglish sentence and Qwen3-ASR writes it back as correct Hindi in Devanagari (`मेरा नाम पॉकेट एई है और मैं आपकी मदद के लिए तैयार हूँ`, `language=hi-en-codemix`)
- [x] New languages speak — all six spot checks TTS PASS with real WAVs: `ta`, `as`, `pa`, `ur`, `ne` (via "Nepali"), `or` (via "Odia") — the CLI mapping works live on the drive
- [x] TTS-only languages are refused TRUTHFULLY by ASR with the exact provider-table message — e.g. `the ASR model 'qwen3_asr' does not serve language 'pa-IN' (provider language table)` — no silent fallback
- [x] Unclaimed `fr`: refused (ERROR, no fallback)
- [x] STS full loop with exactly ONE model call: question TTS'd (hi, 218 KB) → heard back as `भारत की राजधानी क्या है एक छोटे वाक्य में उत्तर दीजिए` → harness_chat route L0, 1 step, 0 tools, 3.9 s → answer `भारत की राजधानी नई दिल्ली है।` → reply TTS'd (111 KB)
- [x] Chat picker shows all 16 voice languages in the staged app (`en, hi, hinglish, as, bn, gu, kn, ml, mr, ne, or, pa, sa, ta, te, ur`)

Test-harness notes (script-side, not product defects): the engine's TTS/ASR success status is `AVAILABLE` (`VoiceCapabilityStatus`), not `ok`; TTS returns `\\?\`-prefixed paths; ASR input is confined to the Pocket AI root or the `<temp>/unoone-stt` capture area (`confine_audio_input`), so roundtrip copies the TTS WAV into the capture area first — the same area the mic pipeline records into.

## 7. Language answer (the user's question, verbatim truth)

- **Understand (STT):** en, hi, Hinglish — live-proven roundtrips (Qwen3-ASR). 22 Indic ASR languages are declared for IndicConformer in the contract but that model is **not staged** on the drive, so they are truthfully refused, not silently mis-served.
- **Speak (TTS):** 15 languages — English, Hindi, Hinglish (via the Hindi voice), Assamese, Bengali, Gujarati, Kannada, Malayalam, Marathi, Nepali, Odia, Punjabi, Sanskrit, Tamil, Telugu, Urdu (OmniVoice, live-probed).
- **Write (ASR transcripts):** every understood language writes into the vault as text; the chat picker now exposes all 16 voice options.