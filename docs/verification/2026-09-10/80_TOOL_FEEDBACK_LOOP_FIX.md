# 80 — The Agent Looped Until Budget Death: Tool-Feedback Transcript Fix

**Date:** 2026-09-10
**Scope:** `packages/pai-harness-adapter` (llama_local provider)
**Posture:** third live-caught defect during user-perspective pendrive acceptance,
found immediately after the sandbox fix (doc 79) let tool calls through.

## 1. Symptom (live, through the real app UI)

With the sandbox fence granting full-access tools, asking the agent to actually
use one:

> "Create a file named live-test-20260910.txt in your workspace containing …"

ran the full 48-step L3 budget and then failed:

```text
[console.warning] Harness bridge fell back to legacy agent:
budget_exceeded:agent.loop: agent step budget exhausted before completion
```

The legacy fallback then answered "I don't have the ability to create or write
new files" — the exact false claim the whole acceptance run exists to eliminate.

## 2. Root cause

The harness core records a tool exchange as a flat `Tool`-role message:

```text
tool={tool_id} call={call_id} result={content}
```

It never puts the assistant's own tool-call turn into the model history — the
core's `ModelMessage` is role+content only. So every follow-up request rendered
to llama-server as:

```text
system …
user: create the file…
tool: tool=fs.write call=r-1-2-1 result=Wrote 41 bytes…
tool: tool=fs.read  call=r-1-3-1 result=PAI live…
```

with **no assistant tool-call turn anywhere**. The OpenAI protocol (and the
Gemma chat template behind `--jinja`) requires the assistant `tool_calls`
message to precede its `tool` results. Without it the model never sees that it
already issued the call, so it re-issues it every step — a perfect tool-call
loop — until the 48-step budget dies and the bridge silently falls back.
Tool calling itself was healthy (the model emitted well-formed calls;
`--jinja` was on; the sandbox granted them); only the feedback path was broken.

## 3. Fix

`packages/pai-harness-adapter/src/llama_local.rs` — the request-building is
extracted into `build_openai_messages()` and now reconstructs the canonical
conversation from the harness transcript:

- Each run of consecutive `Tool`-role messages is parsed
  (`parse_tool_transcript`) into `(tool_id, call_id, result)`.
- The run is emitted as one assistant message carrying the grouped
  `tool_calls` (id + function name), followed by each result as a `tool`
  message with its matching `tool_call_id`.
- Foreign-shaped tool messages pass through unchanged rather than being
  force-fit into a structure the model could misread.

Six unit tests pin the reconstruction: round-trip parsing, markers inside
result content, foreign-shape rejection, the canonical pair layout,
pass-through for non-transcript tool messages, and untouched plain history.

## 4. Why CI never caught it

The adapter's protocol-building path was only exercised against a live server
in manual tests; the unit suite covered policy/memory, not transcript
rendering. And no CI job runs a multi-step model+tool loop against a real
llama-server — the loop signature (48 re-issued calls) only appears live.

## 5. Post-fix acceptance (must be re-run live)

- The file-creation question completes in a handful of steps: the model sees
  its own calls and their results, finishes with text, no fallback.
- fs.write/fs.read/workspace.search/workspace.patch/process.run/browser.act
  all complete end-to-end through the real app UI.