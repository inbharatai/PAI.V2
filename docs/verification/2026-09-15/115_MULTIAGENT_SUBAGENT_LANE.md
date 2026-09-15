# 115 — The Multi-Agent Lane: `agent.spawn` + `PaiSubagentProvider` (Codex/GLM-style sub-agents)

**Date:** 2026-09-15
**Scope:** desktop `harness_bridge.rs` + harness core `jobs.rs`
**User ask:** *"analyze and update if we can integrate multi agents or does our gemma and harness can work with there multi agents to complete complex or any tasks just like codex or glm can"*

## 1. Analysis (what already existed, what was missing)

The harness core already ships the full **subagent seam** with none of the consumers: `jobs.rs` defines `SubagentRequest {prompt, parent_id, depth, max_depth, capabilities, max_output_bytes}`, `SubagentResult {child_id, output, failure}`, the `SubagentProvider` trait, and — the keystone — `run_scoped_subagent`, which refuses any request that exceeds the parent's capabilities, exceeds the depth ceiling, or asks for a zero output budget, and truncates the child's output to the requested byte ceiling at a UTF-8 boundary. `Capability::Subagent` was already an enum variant with no tool that required it.

So the answer to the user's question is: **no new mechanism was needed — only a consumer.** What was missing end-to-end:

1. No tool exposed the seam to the model, so `Capability::Subagent` was dead.
2. No provider implementation ran a real child (model + tools + budget).
3. The core seam itself had **zero tests** — the scoping gate and truncation had never been exercised.
4. The parent briefing never taught when/why to delegate.

## 2. Design

- **Depth chain:** the top-level run's actor stays `local-user` (depth 0). `AgentSpawnTool::execute` parses the caller's depth out of the actor string (`actor_depth`: `subagent-<id>-d<N>` → N, anything else → 0) and spawns the child at depth+1. `SUBAGENT_DEPTH_CEILING = 2` matches `full_access_budget().max_subagent_depth`, so a depth-2 child gets `max_subagent_depth: 0` and its own `agent.spawn` calls are refused by the budget — the fringe cannot recurse.
- **Capability narrowing:** the child's `RunOptions.capabilities` = the parent's set (the request gate in `run_scoped_subagent` refuses anything wider). The child's `DesktopSandbox` gets exactly `request.capabilities`; its own `agent.spawn` requires the `Subagent` capability inside that set.
- **Child budget (`subagent_budget`):** 2,000 steps / 10,000 tool calls / 1 h wall clock / 8 MiB output — a real lane, not a token gesture — but **one round** (`max_rounds: 1`) and **no nested jobs** (`max_jobs: 0`): the child completes one task and reports.
- **Isolation:** the child gets its own `PaiLlamaLocalProvider` (same model/port), its own `PaiVaultMemoryProvider` with a **per-child conversation namespace** (`<vault>:subagent:<child_id>`) and `write_conversation: false` — it cannot pollute the user's conversation memory — plus `StaticConfirmationProvider{AllowedOnce}` so a child cannot stall the whole run on an interactive confirmation.
- **Failure as data:** `SubagentResult.failure` is reported to the parent as the tool's output (`status: failed` + the failure text), not swallowed — the parent can decide to retry, work around, or report.
- **Briefing (`subagent_system_prefix`):** the child is told it sees *nothing* of the parent conversation, must receive a COMPLETE self-contained task, never talks to the user, and must end with a report stating exact ABSOLUTE paths and exact command output as evidence. The parent briefing (`desktop_system_prefix`) teaches delegation: split independent pieces, get a fresh independent pass, treat child reports as **claims to verify**.

## 3. Tool contract (`agent.spawn`)

`required_capabilities: [Subagent]`, `supported_levels: [L3]`, `NonIdempotent`, no confirmation, 1 h timeout, 512 KiB output. Input schema accepts exactly one `task` string (≤ 32 KiB, direct validation — the shared `required_string` helper caps at 4 KiB and would silently reject legitimate large tasks). Output is `{child_id, status, output}`; the model-facing content is `Sub-agent <id> (depth d) completed|failed.\n\nReport:\n…`.

## 4. Core seam tests (new — the gate had none)

- `run_scoped_subagent_accepts_valid_request_and_truncates_output` — valid request passes; a stub whose output far exceeds the ceiling is truncated to exactly 8 bytes.
- `run_scoped_subagent_rejects_invalid_scope` — a canary provider (answers `must-not-run` if invoked) proves depth 0, depth > max_depth, zero output budget, and capability-overflow requests are all refused with `PermissionDenied` **before** the provider runs.

## 5. Desktop tests (new, suite 133 → 138)

- `agent_spawn_manifest_declares_the_contract` — id/schemas/required capability/L3-only/manifest validity.
- `agent_spawn_validation_rejects_malformed_calls` — missing/empty/blank/oversized/extra-key tasks rejected, valid accepted.
- `agent_spawn_refuses_depth_past_the_ceiling` — an actor at depth 2 (the fringe) gets `PermissionDenied`; `actor_depth` parses `local-user`/`-d1`/`-d2`/`bob`/`-dNaN` correctly.
- `full_access_budget_opens_the_subagent_lane` — the full-access budget's depth equals the ceiling; `subagent_budget(remaining)` math is pinned (fringe = 0).
- `subagent_briefing_demands_a_verifiable_report` — the child briefing names `agent.spawn`, demands the self-contained task, and demands exact paths/output.

## 6. Verification status

- Harness core: **52/52 green** (2 new subagent tests + the defect #42 count-metadata test), `cargo fmt` + `clippy -D warnings` clean.
- Desktop: **138/138 green**, `cargo fmt` + `clippy` clean.

### 6.1 Live result (2026-09-15, staged drive ab33734)

Task: spawn two sub-agents (one lists every `.js` file in the live-test folder with exact count and names; one reads `deploy-task.txt` and reports exact size and first 10 words), the parent must verify both reports with its own tools.

- [x] A chat task that splits into independent pieces produces `agent.spawn` in the activity feed (4 spawns visible: 2 live + 2 verification) — **PASS**.
- [x] Distinct sub-agent ids cited in the answer (`subagent-3da7e0ba`, `subagent-1767efdb`) — **PASS**.
- [x] A child's undercount surfaced as data the parent handled: the sub-agent reported 93 `.js` files, the parent's own `fs.list` verification found the true 127 and reported the mismatch as a FAILED claim — honest failure surfacing works exactly as designed — **PASS**.
- [ ] Exact `.js` count and file names in the answer — **FAIL** (see defect #42): the count was tool-reachable but the names came back incomplete because nothing stated a count the model could anchor on.
- [ ] `deploy-task.txt` size correct — **FAIL** (see defect #42): **both** the sub-agent and the parent claimed "754 characters (Verified)" for a 1,225-char file — a confabulated figure the model repeated because **no tool output ever stated a file size**. RootedFs::read_text was ruled out (it refuses oversize files, never truncates) — the cause is missing metadata, not lost data.
- [x] Parent verification confirmed the first-10-words claim (exact text matched) — **PASS**.

**Verdict: lane functional, accuracy gap real.** Spawn, ids, completion reports, verification behavior, and honest-failure surfacing all work live on the 12B; the two failures share one root cause — the tools returned bare content with no sizes or counts, so the model invented numbers. That is a tool defect, not a model excuse, and it is fixed below.

### 6.2 Defect #42 fix: tools must state exact sizes and counts (PR #46)

- `fs.read` now prepends `{path} — {bytes} bytes, {chars} chars (complete file):` to the model-facing content — the size comes from the tool, never the model's eye.
- `fs.list` now appends `(N entries in {path})` (or `{path} — empty (0 entries)`), and gains an **optional `suffix` filter** (e.g. `.js`): the result states `(M of N entries in {path} end with '.js')`, the value array contains only matching names, and the filter is **declared in the input schema** (the defect #38 lesson — the schema is the model-facing contract; a description sentence alone is invisible to a model that follows the schema).
- Unknown arguments are rejected by key (no silent extras smuggled through the optional slot).
- New core test `fs_read_and_list_state_exact_sizes_and_counts` pins: exact total count, filtered count out of total, filter named in output, non-matching entries excluded, zero-match stated honestly, stray keys rejected, exact byte/char size on read, suffix declared in the schema. Core 51 → **52 green**.
- Live re-verification (2026-09-15, drive re-staged with main a712f29): [x] **PASS, VERDICT COMPLETE** — the parent's answer now states **127 .js files** (full exact name list) and **1,225 characters**, both tool-stated, both matching ground truth computed independently (127 / 1,225). All six acceptance checks pass. Bonus evidence for the honest-failure design: sub-agent one failed with a timeout and the parent reported the failure verbatim, then completed the listing itself with its own `fs.list` — failure as data, no hallucinated rescue. The defect #42 fix turned the same 12B model from a confabulator ("754 characters, Verified") into an accurate reporter.

## 7. Honest limits (what this deliberately is not)

- Sub-agents run **sequentially** within a parent step — no parallel fan-out yet (the harness `JobRegistry` exists; wiring spawn-into-job is future work).
- The depth ceiling is a **product choice** (2), not a technical limit.
- All sub-agents share the single local Gemma model — the seam would accept a smaller/faster model for children, but `PaiSubagentProvider` pins the same `model_id` for now.