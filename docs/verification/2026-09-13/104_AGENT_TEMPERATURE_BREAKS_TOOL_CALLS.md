# 104 — Defect #28: Agent Temperature 0.7 Makes the 12B Ignore Its Tools on Build Tasks

**Date live-caught:** 2026-09-13 (physical drive D:\UNOONE, defect-#26 fix build, long-coding acceptance run 2)
**Severity:** Critical — the flagship "autonomous full-access agent" intermittently answers "build me X" tasks by pasting code into the chat instead of creating the real files. Same user-facing symptom as defects #22/#26 (no artifacts on disk), third distinct root cause: this time the model made **zero tool calls**.

## 1. Live catch

The long-coding task ("create task-board with 4 files, run the Playwright test, report the output") was sent with full access ON (toggle verified `checked:true` live) on the defect-#26-fixed build (10,000-step budget). The llama-server log for the whole run shows **exactly one real completion**: 612-token prompt → **2746-token generation → EOS** (5.6 min), no tool-call steps, no files created. The model pasted the four files' code as prose. (The answer text itself was invisible due to defect #27 — it was discarded on tab switch — but the log's single-completion shape is unambiguous: an agentic run needs one completion per step; this run had one and stopped.)

The same prompt in the previous day's run had driven 10 tool steps (before dying at the old #26 budget). Same model, same prompt, same tools — the only variance is sampling. The provider sends `temperature: 0.7` (`pai-harness-adapter/src/llama_local.rs`) with `tools` + `tool_choice: "auto"`: high enough temperature that the 12B's tool-format adherence is a coin flip on long build tasks.

## 2. Root cause

| Probe | Result |
|---|---|
| Full access toggle (live, through the shipped UI) | ON — `checked:true`, L3 route with fs/process/browser tools registered |
| Defect #26 fix (budget) | working — the run did not hit any step budget |
| llama-server log, whole run | 1 real completion (task 7281: 612-token prompt, 2746-token output, finish=stop), 0 tool-call steps |
| Provider wire request (`llama_local.rs`) | `"temperature": 0.7` with `tools` + `"tool_choice": "auto"` |
| Prior run (2026-09-12, same prompt) | 10 tool steps at ~1.7 min each — tool calls DO work when the model emits them |

Sampling at 0.7 makes protocol adherence stochastic; a local 12B needs low temperature when a structured tool-call format is the required output.

## 3. Fix

`packages/pai-harness-adapter/src/llama_local.rs`: tool-bearing requests now go out at **temperature 0.2** (format-faithful, near-deterministic tool use); toolless Q&A keeps 0.7. Two pinning tests: `tool_bearing_requests_pin_low_temperature` (asserts temperature 0.2 + tools + tool_choice on the wire) and `toolless_requests_keep_default_temperature` (asserts 0.7 and no tools key).

## 4. Verification

- [x] Adapter tests green (new pins included)
- [x] Full backend test suite green, clippy clean, fmt clean
- [ ] CI green
- [ ] **Live re-verify after re-stage:** long-coding acceptance — harness_chat L3 runs the whole task to real artifacts (4 files on disk, node server, Playwright self-test with reported PASS output), no prose-dump answer, no fallback pill