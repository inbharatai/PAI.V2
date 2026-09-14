# 106 — Defect #30: The Tool Transcript Dropped the Arguments, so the Model Rewrote the Same File Forever

**Date live-caught:** 2026-09-14 (physical drive D:\UNOONE, during the long-coding acceptance re-run on staged main `a6ce389`, i.e. the run that validated the defect-#29 fix)
**Severity:** Critical for the agent lane — the loop could never progress past the first two files of any multi-file task, no matter how large the step budget was.

## 1. Live catch

The defect-#29 fix worked: the model created `task-board/`, wrote `index.html` (1,626 bytes), wrote `server.js` (901 bytes). Then the run went into a rewrite spiral the hardened watcher caught at files-frozen-2/4:

- `index.html` re-written at 09:33 (1,654 bytes), then AGAIN at 09:40 (1,140 bytes) — three different versions of the same file
- `app.js` and `test-app.js` never reached disk; 16 real model completions in ~19 minutes
- prompt tokens bounced in a 2366–2686 band with **no cumulative growth** — each follow-up request was almost the same size as the first, which is impossible in a healthy agent loop that appends every tool result

The llama-server log gave the signature (n_tokens per request: 2374, 2366, 2419, 2644, 2686, 2528, 2522, 2600, …) and the disk gave the behavior; the transcript format gave the cause.

## 2. Root cause

The runtime records each tool exchange for the model as a flat Tool-role message
`tool={id} call={call} result={content}` — **the arguments were never recorded anywhere in the model transcript.** The adapter (`llama_local.rs`) already reconstructed the canonical assistant-tool_calls + tool-result pairs (the defect-#22-era fix), but with nothing to put in `arguments` it emitted `"arguments": "{}"`.

So at step N the model saw: system + task + `wrote 1626 bytes to task-board/index.html` — but NOT what file contents it had written, and (because the assistant turn is reconstructed with empty arguments) not even which path each call used beyond the result line. The 12B at temperature 0.2 re-derived "I should start by writing index.html" every step and wrote it again. The tool results kept confirming success, so nothing ever failed — the run just orbited.

Decisive probe (`continuation-probe.js`, both variants replayed against the drive's llama-server with the exact conversation shape the adapter builds at step 3 — task + two completed writes):

| Variant | Next model action |
|---|---|
| A — empty arguments (as shipped) | `fs.write` with contents starting `<!DOCTYPE html>…` — **re-writing index.html** |
| B — real arguments echoed | `fs.write` with contents starting `const taskInput = document.getElementById('new-task')…` — **app.js, the correct next file** |

Same model, same server, same task, same step — the only difference is whether the model can see its own arguments.

## 3. Fix

- **Runtime (`runtime.rs`)**: `encode_tool_transcript` — tool exchanges now travel as a compact JSON object `{"tool":…,"call":…,"args":<canonical-JSON string>,"result":…}` (or `"error":…` on failure). JSON is unambiguous even when file contents contain the legacy ` tool=` / ` call=` / ` result=` markers, which the prefix format could not survive. Both push sites (success + error) and `derive_model_history` (prior turns — it now collects `call_id → arguments` from the session's ToolCall events) use it.
- **16 KiB args echo cap** (`TOOL_ARGS_ECHO_LIMIT_BYTES`): one huge fs.write would otherwise re-enter every subsequent request and could exhaust the 32K context on a long run; over the limit the args are truncated and an `args_truncated` note is added.
- **Adapter (`llama_local.rs`)**: `parse_tool_exchange` parses the JSON shape first and puts the REAL arguments into the reconstructed assistant `tool_calls`; the legacy prefix format still parses (empty arguments) so nothing that reads old transcripts breaks.

## 4. Tests

- Core: `tool_transcript_echoes_arguments`, `tool_transcript_truncates_huge_arguments`, `prior_turn_history_echoes_arguments_from_the_session_events`, and the end-to-end `followup_request_carries_the_tool_arguments` (a capturing two-step provider drives a real L3 harness run and asserts the follow-up request's Tool message carries the original `{"contents":…,"path":…}`).
- Adapter: `json_tool_exchange_carries_real_arguments`, `json_tool_exchange_survives_legacy_markers_inside_arguments` (contents containing all three legacy markers), `json_tool_exchange_error_variant`, `json_tool_exchange_canonicalizes_structured_results`, `legacy_transcript_still_parses_with_empty_arguments`, `json_tool_exchange_echoes_arguments_on_the_wire`.
- `cargo test -p inbharat-harness-core` 45+integration green, adapter 22/22, desktop 128/128, clippy + fmt clean on all three crates.

## 5. Live acceptance (post-merge, on the re-staged drive)

- [ ] The long-coding acceptance run completes: all 4 task-board files on disk, the model reports the node test output, watcher verdict PASS
- [ ] Capability panel Agent Loop lane flips to Verified Working on that evidence