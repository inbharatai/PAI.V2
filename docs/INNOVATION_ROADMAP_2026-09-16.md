# Innovation pass — world-class gap analysis (task #69)

**Date:** 2026-09-16 · **Author:** live acceptance campaign (defects #6–#44)
**Method:** every item below is grounded in a latency, defect, or behavior
actually measured on the staged drive (`D:\UNOONE`, main `534cf6a`/`ce49048`)
during the 2026-09-12 → 2026-09-16 user-perspective acceptance campaign.
No speculative roadmap items. Codex/GLM are the user-named comparison bar.

## Shipped in this pass (PR #49)

1. **Reasoning-aware describe budgets + truthful empty-reply errors**
   (defect #44): Gemma 4 emits chain-of-thought into `reasoning_content`
   before the visible answer; the old 320-token scene_summary budget was
   consumed entirely by planning → empty description → the blind-aid lane
   spoke silence. Measured need: ~620 completion tokens. Budget 320 → 1024;
   an empty visible reply is now a surfaced error, never silent success.
2. **One-breath describe prompt** (measured A/B on the live drive, same
   image): 619 → 385 completion tokens, 73.6s → 51.9s wall-clock, identical
   answer quality. ~30% faster narration for the flagship accessibility lane.

## Gap 1 — Streaming responses (the single biggest Codex/GLM-parity gap)

**Observed:** `harness_chat` and every vision call block until completion.
A 52s describe or a multi-step L3 agent run shows the user **nothing**
until it finishes. Codex/GLM stream tokens and show live agent steps.

**Fix:** llama-server already speaks SSE. Stream completion deltas through
Tauri events to the UI (token-by-token chat, live agent step pills), and
for the blind-aid narration pipeline, **speak the first sentence as soon as
it is generated** (TTS chunking) instead of waiting for the full
description. Perceived latency drops 5–10× for long answers.

**Impact/effort:** high / medium (bridge + ChatView).

## Gap 2 — Blind-aid latency floor (52s measured post-#49)

**Observed:** generation runs at ~8 tok/s with mmproj on the target host;
385 tokens ≈ 52s. Prompt work is done (one-breath); the remaining
bottleneck is generated-token count and speed.

**Options, in order:**
- (a) **Speak-first-sentence streaming** (pairs with Gap 1): blind user
  hears the first object report in ~5–8s.
- (b) **Think-off at the chat-template level** if the Gemma template
  supports disabling reasoning (needs a template probe; would cut ~300
  planning tokens → ~85-token answers ≈ 11s).
- (c) Capture at lower resolution for narration ticks (fewer image
  tokens; vision prompt is small today so this is minor).

**Target:** <15s perceived first-audio. **Impact/effort:** high / medium.

## Gap 3 — Prefix caching is not engaged (token economy, bootstrapped-user directive)

**Observed:** live probes report `cached_tokens: 0`. Every harness step,
describe call, and STS turn re-processes the system prompt + capability
contract from scratch. The agent's L3 loop re-sends a large static prefix
48 times per run.

**Fix:** pin the harness conversation to one llama-server slot with
`--cache-reuse`, and order messages so the static prefix (system prompt,
tool contract, history) precedes the per-step delta. Prompt processing
drops ~30–50% for agent loops; battery/thermals improve on laptops.

**Impact/effort:** medium / low–medium.

## Gap 4 — A shared reasoning-aware inference layer

**Observed:** defect #44 lived only in the describe path because each
caller hand-rolls its own request. The chat path has the same latent risk:
any small budget + long reasoning = silent empty answer.

**Fix:** one inference helper used by chat/OCR/describe/harness: budget =
planning + answer; strip `reasoning_content` from user-visible text;
expose thinking as a collapsible "reasoning" step pill (Codex-style
transparency) instead of discarding it.

**Impact/effort:** high (accuracy + honesty) / medium.

## Gap 5 — Memory retrieval is lexical-only

**Observed:** capability audit (2026-09-09) — memory search is lexical;
TF-IDF exists only for documents. Semantic matching is absent.

**Fix (cheap, no new model):** rerank the top-N lexical hits with the
local Gemma model ("which of these notes is most relevant to the query")
before injection into agent context. Defer embedding models (cost, size).

**Impact/effort:** medium / low.

## Gap 6 — Sequential tool execution

**Observed:** the L3 loop executes one tool call per step even when
independent (two `fs.read`s, `agent.spawn` + `fs.list`).

**Fix:** batch tool_calls with no data dependencies into one execution
round; keep audit per-call. Measured agent runs should cut ~20–30%.

**Impact/effort:** medium / medium (harness executor is vendored —
upstream-first per the harness policy).

## Recommended order

1. Gap 1 streaming (perceived-latency parity, user-visible)
2. Gap 4 reasoning layer (accuracy + no silent failures)
3. Gap 3 prefix caching (token economy — bootstrapped-user directive)
4. Gap 6 parallel tools
5. Gap 2(b) think-off probe + speak-first TTS
6. Gap 5 lexical rerank

Items 2 and 3 are prerequisites for the rest; Gap 1 is the headline.