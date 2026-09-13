# 101 — Defect #26: The "No-Cap" Full-Access Budget Secretly Capped Every Agent Run at 10 Steps

**Date live-caught:** 2026-09-13 (physical drive D:\UNOONE, staged main @ 5662b36, during the long-coding acceptance re-run — the Agent Loop capability lane)
**Severity:** Critical — every full-access agent task needing more than 10 model steps dies mid-run into a read-only fallback that then confidently tells the user "I cannot write files", with the full-access toggle ON. This is the same user-facing symptom as defect #22 (doc 97), from a different root cause, and it re-broke the long-coding lane the defect-#22 fix had reopened.

## 1. Live catch

The long-coding acceptance task ("build a task-board web app: 4 complete files, a node server, a Playwright self-test, report the exact output") came back as a denial — "As an AI assistant running in this environment, I do not have direct access to your computer's file system…" — followed by the four files' code pasted into the chat. Zero artifacts on disk (`task-board/` never created), full access verified `checked:true`, no visible error. The truth was inside a **collapsed step pill** (its text is invisible to `innerText` until expanded):

> 💭 Fell back to the read-only legacy agent (the primary agent pipeline stopped: `budget_exceeded:agent.loop: agent step budget exhausted before completion`)

So the harness L3 run STARTED (defect #22's fix works — no more `fs.resolve` death), ran for ~17 minutes, and died at its step budget — after which ChatView fell back to the read-only legacy agent (defect #22's honesty fix: the pill now says "stopped"), which truthfully described its own read-only abilities.

## 2. Root-cause isolation

| Probe | Result |
|---|---|
| The desktop full-access budget (`harness_bridge.rs full_access_budget`) | Already at the validator ceiling: 10,000 steps, 100,000 tool calls, **1,000 rounds**, 24 h, 64 MiB — the "no cap" directive was implemented as literally as `validate_run_options` permits |
| Harness goal loop (`runtime.rs run_goal_loop`) | `steps_per_round = (limits.max_steps / max_rounds).max(1)` = 10,000 / 1,000 = **10 model steps per round**, then `run_model_loop(..., steps_per_round)?` — the `?` makes the loop-exhaustion failure (`budget_exceeded:agent.loop: agent step budget exhausted before completion`) propagate fatally out of the goal loop; later rounds are never reached |
| The shared `Budget` (`budget.rs reserve_step`) | Already enforces the GLOBAL 10,000-step total across all rounds (`self.steps` accumulates session-wide) — the per-round division is a redundant second cap layered on top of a real one |
| The desktop's verifier | `CanonicalVerificationProvider` — `verify()` returns `Ok` for any output under 8 MiB, so goal-verification retries (the only thing rounds exist for) can never trigger on this lane |
| Live timeline | Task sent 10:33:48 UTC → answer (fallback denial) landed 10:50 — ~17 min ≈ 10 model completions at ~8.4 t/s on the 12B, consistent with 10 steps, and ~15–25 steps is what the task actually needs |

The failure chain: a task needing >10 steps dies at step 11 with a fatal BudgetExceeded → ChatView falls back to the read-only agent → the user gets a confident "I cannot write files" denial while the full-access toggle is ON. The "no cap" budget directive was silently capped at 10 effective steps by the round division.

## 3. Fix

| # | Layer | Change |
|---|---|---|
| 1 | `harness_bridge.rs full_access_budget()` | `max_rounds: 1_000` → **`1`**, with the full live-caught analysis in the doc comment. One round = the model loop is passed the whole `max_steps` (10,000) cap; the shared `budget.reserve_step` still bounds the global total. Rounds only exist to retry goal-verification failures, and this lane's verifier passes everything — rounds 2+ were dead config whose only live effect was the division |
| 2 | `harness_bridge.rs` test `full_access_budget_is_accepted_by_the_harness` | Now pins `max_rounds == 1` and `max_steps == 10_000` so the division trap cannot silently return |

Not changed (recorded, deliberately): the harness's per-round division semantics stay as-is for other consumers (`vendor` behavior is pinned by its own test suite). The desktop lane owns its budget shape.

## 4. Verification

- [x] Backend 128/128 green, `cargo clippy -D warnings` clean, `cargo fmt --check` clean
- [ ] CI green
- [ ] **Live re-verify after re-stage:** long-coding acceptance — harness_chat L3 runs the whole task to real artifacts (4 files on disk, node server, Playwright self-test with reported output), no fallback pill (watcher now reads collapsed pills via `textContent`), no denial
- [ ] Capability panel `agent` lane flips to VERIFIED_WORKING citing this doc + the live run