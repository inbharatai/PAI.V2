# 118 — Innovation-Pass Acceptance: Think-Off Vision Lanes (42–60 s → 8.2 s), Prefix Caching, Streaming Chat, and Model-Backed Memory Rerank — Including One Live-Caught Dead Gate in the Streaming Feature Itself

**Date live-caught / accepted:** 2026-09-16, final re-acceptance 2026-09-17 (physical drive `D:\UNOONE`, staged from main `40e9470`)
**Severity:** High — the flagship streaming feature shipped dead in the default configuration and was caught only by the live user-lane test on the physical drive.
**Roadmap:** `docs/INNOVATION_ROADMAP_2026-09-16.md` (Gaps 1–5 shipped; Gap 6 skipped by vendored-harness policy)

## 1. What shipped, PR by PR

| Gap | PR | Merge | What |
|---|---|---|---|
| 2b/3/4 — reasoning-aware inference layer | #54 | `59e9c2b` | One shared `InferenceRequest` carrying `disable_reasoning`; think-off pinned on the vision lanes; `cache_prompt: true` everywhere; cached-prefix tokens + real tokens/s surfaced; truthful empty-reply errors split on `reasoning` |
| 5 — model-backed memory rerank | #56 | `2c4ec7b` | `PaiVaultMemoryProvider::with_lexical_rerank` wired into the main chat + subagent lanes; fail-open contract pinned by tests |
| 1 — streaming chat | #55, then fix #58 | `1e98ea1`, then post-`74c1c7a` | Token-by-token chat answers; **#58 fixes the dead gate** (see §4) |
| Docs | #57 | `74c1c7a` | README desktop-lane section records the pass |

## 2. Think-off — the A/B evidence chain and the live result

**Upstream probe (staged Gemma 4 12B, think-on vs think-off):**

| | think-on | think-off (`enable_thinking:false` + `reasoning_budget:0`) |
|---|---|---|
| completion tokens | 376 | 55 |
| reasoning chars | 1181 | 0 |
| wall-clock | 44.6 s | 5.7 s |

**Live acceptance on the staged drive** (`thinkoff-describe-live.js`, text-output probes only, synthetic scene graded by content): **6/6 PASS** —

- `describe_ms=8200` (think-on baseline: 42–60 s) — ~6× faster, inside the <20 s win band
- `ocr_ms=3745` (think-on baseline: 12.7 s)
- quality anchors held (the model still names the red square, blue circle, green rectangle, STOP 42)

**Full blind-aid suite on the same staged build: 13/13 PASS**, suite wall-clock 9 min → ~6.5 min (OCR 1951 ms, scene describe ~5 s, screen describe 7 s, UI what's-in-front in-panel ~8 s after press, narration first tick 2 min).

**Prefix caching:** llama-server caches across requests (probe: `cached_tokens=91` on a grown-history second call); every request now sends `cache_prompt: true` and the response surfaces `usage.prompt_tokens_details.cached_tokens` + real `timings.predicted_per_second`.

## 3. Gap 5 — memory rerank, live capability probe

`rerank-live.js` probes the exact request shape `rerank_lexical_hits` sends (think-off, temp 0.1, `max_tokens` 96, `cache_prompt`, 30 s hard deadline) against the staged model, with a crafted case where the **lexical order is provably wrong**: the query "how do I reset my wifi router password" matches a stale password note on 4 terms and the actual reset instructions on 1 term.

- **6/6 PASS** — the model replies `2,1,3`: the reset instructions (result 2) ranked first, digits parseable, **zero reasoning emitted**, `rerank_ms=7553` first run / `2202` warm — bounded far under the 30 s deadline.
- The adapter's fail-open contract is pinned by unit tests: a dead-port rerank keeps the lexical order and search still succeeds; disabled configurations and sub-3-hit queries are pure no-ops.

## 4. The live-caught dead gate (defect class: shipped-but-unreachable)

`chat-stream-live.js` — the Gap 1 user-lane test (send a real chat turn through the UI, tap the raw `chat-token` Tauri events, sample the generating bubble's answer text) — **failed on the freshly staged #55 build**:

- `chat-token` events: **0**; DOM snapshots: **1** (the whole 536-char answer landed at once); final answer quality: PASS.

Root cause: #55 gated streaming on `tools.is_empty() && token_emitter.is_some()`. But the production chat lane pins `explicit_level = Some(L3)` in **full-access mode (the default)** — every real chat turn carries tools and took the buffered path. No code review or CI check could see this; only the live user lane could.

**Fix (PR #58):** tool-bearing requests with a tap installed stream too — answer text deltas reach the sink and the `chat-token` tap live, while streamed `delta.tool_calls` fragments (llama.cpp splits one call's `arguments` across deltas) are assembled per OpenAI delta index and emitted as **complete `ModelChunk::ToolCall`s at the end of the stream**, in request order, with `FinishReason::ToolCalls` mapped exactly like the buffered path. The chunk sequence and `ModelResponse` are byte-identical to buffered; only the answer text's arrival time changes. Without a tap (the subagent lane) the buffered path stands, pinned by a test.

Wire-level tests against an SSE mock pin the whole contract: `stream:true` + tools + `tool_choice` on the wire, tap order, the exact chunk sequence including `tool:call-1/fs.read/{"path":"a.txt"}` assembled from two argument fragments, and `ToolCalls` finish.

**Live re-acceptance on the re-staged build:** see §6.

## 5. Staging

- Final bundle built from main (bundle workflow `pocket-ai-windows.yml`, run for `2c4ec7b`): self-verified against `SHA256SUMS.txt` (3/3 OK, CRLF-normalized).
- Staged via `Stage-PocketAiDrive.ps1`: embedding gate **PASS** (`VERIFIED_WORKING`, hashed assets embedded), strict schema-v2 manifest regenerated (158 runtime, 2 model, 381 voice, 2 mobile, 3 speech), `Start UnoOne.exe --verify-only`: `failure_count: 0, valid: true`. Old exes preserved in `D:\UNOONE\RECOVERY\package-backups\`.
- `SOURCE/PAI.V2` re-staged from final main via `git archive` (committed tree only) with a `SOURCE_VERSION.txt` marker; verified the staged tree carries the new lanes (`enable_thinking` present in `llama.rs`, `llama_local.rs`, `memory.rs`) and the exact vendored `inbharat-harness` + `Inbharat-audiocpp` trees both UnoOnePower and the phone models build from.

## 6. Final live verification matrix (re-staged build, main `40e9470`)

Re-staged after the #58 merge; app relaunched from the drive, vault unlocked, model `LOADED`. All numbers below are from this final staged build on the physical drive `D:\UNOONE`:

- `chat-stream-live.js` (streaming through the real UI, post-#58 build): **10/10 PASS** — 94 `chat-token` events, first token at **6323 ms** vs 17312 ms total (streamed, not buffered), 69 growing in-bubble snapshots, streamed text a prefix of the final answer, quality anchor held.
- `rerank-live.js` (model capability for the rerank): **6/6 PASS** — staged Gemma ranks the provably-correct-but-lexically-weakest reset note first (`2,1,3`), zero reasoning chars, `rerank_ms=2105` warm (7553 ms first-run, §3).
- `thinkoff-describe-live.js` (think-off regression): **6/6 PASS** — `describe_ms=7539` (think-on baseline 42–60 s), `ocr_ms=3208` (baseline 12.7 s), quality anchors held.
- blind-aid full suite (`blind-aid-ocr-live.js`), re-run with the §6a selector fix: **13 pass / 0 fail** — OCR 4209 ms with all anchors, describe names all six objects, screen-reader describe lands, chat alignment (text + image) verified, **UI what's-in-front describes the live camera ~8 s after the press** ("a large blue wall… a black mesh office chair…") and speaks it (TTS WAV by the app), narration loop first tick describes the live camera, session state restored.

### 6a. The suite's one FAIL — a live-caught **test-harness** defect, not a product one

The first blind-aid run on the final build reported `ui whatsInFront describe timed out after 300s` — 11 pass / 1 fail. A forensic probe (press the real button, poll every surface the app exposes plus the llama-server `/slots` activity) proved the **product flow completed perfectly on the same build**:

- t≈5 s: capture jpg landed, llama-server slot `busy=true` (describe in flight), think-off describe finished well inside the window
- t≈151 s: new TTS WAV landed (synthesis of the ~245-char spoken excerpt), the "Running vision model" indicator cleared, the button re-enabled, **zero error banners**

Root cause of the false FAIL: the suite's `readVisionText` grabbed the **first** `white-space:pre-wrap` div on the whole page. ChatView is always mounted, and the run had `chat-stream-live.js` exercise a chat turn in the same app session beforehand — so the first pre-wrap div was a **stale chat bubble** whose text never changes. The suite waited 300 s for that div to change while the actual vision result rendered and was spoken. Yesterday's 13/13 passed only because that run had a fresh session with no chat DOM.

Fix: the suite's result/error lookup is now **scoped to the Blind View panel** (`.settings-section-body` containing "Blind View") — it can no longer latch onto chat DOM. The forensic trace doubles as product verification: the one-press what's-in-front flow (capture → think-off describe → speak) is verified working end-to-end on the final staged build, ~151 s wall-clock with TTS synthesis dominating.

Defect class note: this is the inverse of §4's dead gate — a shipped-but-unreachable feature caught only live. Here a **test** failed only when two features are exercised in one session, which no isolated run could see either. Both directions argue for the live user-lane matrix as the acceptance bar.

## 7. Gap 6 — skipped by policy

Parallel tool calls live in the vendored `inbharat-harness` core; PAI's policy is upstream-first for vendored-core changes (the streaming and rerank work deliberately touched only the adapter and the app). Recorded as a roadmap follow-up, not a silent drop.