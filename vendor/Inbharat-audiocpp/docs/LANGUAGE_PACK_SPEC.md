# Language Pack Specification

A language pack is a **versioned, installable bundle** of deterministic adaptation assets for one language or code-mix. Packs make Android/Pi deployments clean: only the packs a deployment needs are installed. A pack is *not* a trained model — it is the deterministic layer (normalization, script policy, lexicon, transliteration, provider routing) that sits above whichever provider executes the actual inference.

## Layout

```
packs/<pack-id>/
    manifest.json            # capability manifest (schemas/capability-manifest.v1.schema.json)
    normalization.json       # number/currency/date rules and toggles
    lexicon.json             # user-approved pronunciation / entity corrections
    transliteration.json     # script-policy config + custom grapheme overrides
    providers.json           # ordered provider priorities for this language
    benchmark/               # licensed evaluation set + expected outputs (never trained on)
    quality.json             # thresholds a release must meet on the benchmark
```

## Rules

1. **Deterministic only.** Normalization, script policy, lexicon, and transliteration are explicit rules. A pack must not ship a model pretending to be a rule, and must not require network access.
2. **Evidence-gated.** `manifest.json` fields follow the capability manifest schema: a language/platform/streaming claim requires the matching evidence level. A pack ships with `evidence_level` reflecting what has actually been run.
3. **User-approved lexicon.** `lexicon.json` corrections (names like Reeturaj, Guwahati, Nagaon, Tezpur, Bhaswati) are applied to ASR output without retraining any model. They are local, inspectable, and reversible.
4. **Script policy is explicit.** A Hindi utterance may render as Devanagari or Romanized Hindi depending on the pack's `transliteration.json` policy and the caller's request. Both are first-class.
5. **No training on the benchmark.** `benchmark/` is a held-out evaluation set. Model/pack changes must never fit to it.

## Ship-readiness gates

A pack may only ship when: its manifest validates against the schema; every normalization/transliteration rule has a passing test in the language test lane; and the pack's claimed `evidence_level` matches recorded evidence. Assamese (as-IN) is staged as a **separate validation effort** — it is not bundled into the default packs until its own evidence exists.

## Current packs

| Pack | Scope | Status |
|---|---|---|
| `en-IN` | Indian English normalization, ₹/lakh/crore, dates/times | rules implemented + tested |
| `hi-IN` | Hindi Devanagari script policy + romanization, shared normalization | rules implemented + tested |
| `hi-en-codemix` | Hinglish segment-level tagging + per-script policy | rules implemented + tested |
| `as-IN` | Assamese | **staged — separate validation effort, not in default build** |
