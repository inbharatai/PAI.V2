# All-22 Foundation — Independent Adversarial Verification

Commit: `ed5a59d`. Date: 2026-08-22 (IST). Independent verifier operated read-only; scratch builds under `/tmp`.

## Final verdict

**CONFIRMED. New defects after the final gate: none.**

- Fresh Linux Release: **24/24 CTest passed**.
- Release assertions are genuinely active: every C/C++ test compile command contains `-DNDEBUG` followed later by `-UNDEBUG`.
- Fresh ASan+UBSan: **24/24 passed**; sanitizer libraries genuinely linked.
- ABI: exact **79/79** exported `ibaudio_*` symbols.
- All-22 pack catalog: exactly 22 unique Scheduled language codes; every task/platform state PENDING unless explicit report+artifact-hash evidence exists.
- Python and C++ catalog paths fail closed: hash-valid `../` traversal rejected; post-load manifest tampering rejected.
- Unicode layer: Indian script classes and invalid UTF-8 tested; deterministic `language_tag` is `und` for Latin/Devanagari too, so script is not overclaimed as language.
- Speech Mesh: PENDING/remote/over-memory/non-streaming/low-quality/low-confidence candidates excluded; ambiguous providers and conflicting outputs abstain.
- Personal adaptation: bounded, language-scoped, fingerprinted, newest-wins and individually rollbackable.
- Speech-to-speech: per-stage confidence/latency, low-confidence abstention, missing-stage refusal, pre-stage and during-TTS cancellation; cancelled TTS publishes no audio.
- India-22 scorers: per-language STT WER/CER and TTS intelligibility/TTFA/RTF; missing languages fail `--require-all-22`; MOS stays null without native ratings.
- MCP `audio.language_packs` tool and `ibaudio://language-packs` resource return exactly 22.
- Windows x64 and Android arm64 artifacts have correct target formats and hashes. Both remain **BUILDS_NOT_RUNTIME_TESTED**.
- No fake all-22 neural claim, no silent cloud fallback, no all-22 TTS claim.

## Honest release boundary

The runtime, routing, packs, integrity, reversible adaptation, speech-to-speech orchestration and evaluation gates are implemented and tested. **Neural quality for all 22 languages is not verified.** IndicConformer is a PENDING local-service STT candidate; the four-family TTS portfolio is PENDING. Same-language data, native raters, the user's Windows run and physical Android-device execution are external evidence still required.
