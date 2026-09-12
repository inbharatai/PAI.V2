# 91 — Defect #16: The Full-Access Session Budget Was Rejected by the Harness

**Date:** 2026-09-12
**Scope:** `vendor/inbharat-harness/crates/core/src/runtime.rs`, `apps/desktop/src-tauri/src/harness_bridge.rs`, `apps/desktop/src-tauri/Cargo.toml`
**Posture:** live-caught on the re-staged drive (bundle `03cc1b2`), fixed same day.

## 1. Live catch

With the Full access toggle checked, a long-coding request ("build a
multi-file web app, use Playwright, serve on port 8199") was answered
with a denial:

> My current capabilities are limited to search_notes, list_documents,
> read_document, and verify_vault.

The toggle was ON, so the model should have had the full L3 tool set
(fs read/write, workspace search/patch, allowlisted process.run,
browser automation). Calling `harness_chat` directly over CDP returned
the real error the UI had swallowed:

```
invalid_input:run.options: custom execution budget exceeds hard
safety bounds
```

The UI's fallback path silently re-routed the request to the legacy
vault-only agent, whose four read-only tools the model then truthfully
described. The user saw a "no" with no hint of why.

## 2. Root cause

PR #15 raised the desktop full-access lane's session budget to
`max_output_bytes: 64 MiB` (512 steps, 1024 tool calls, 6 h). But the
harness's `validate_run_options` still capped any custom budget at
8 MiB — a bound written when budgets were per-tool-shaped, never
revisited when the desktop lane grew. So **every** full-access
harness_chat call died at run-options validation, before a single
model call — and the lane had never actually worked live. The earlier
"tool-loop" fix (#11, PR #13) had been verified with the toggle OFF.

## 3. Fix

Two layers, both in one PR:

1. **Validator bound** (`runtime.rs::validate_run_options`): the
   cumulative session-output ceiling is now 64 MiB. The per-item caps
   elsewhere (tools.rs, providers.rs, value.rs) stay at 8 MiB — this
   change only widens what a *session* may accumulate, not any single
   tool or model call.
2. **Per-call clamp** (`runtime.rs` run loop): each `ModelRequest`
   gets `budget.limits().max_output_bytes.min(8 MiB)` —
   `model.prepare` rejects requests above 8 MiB, so without the clamp
   a 64 MiB session budget would have failed the *next* validation
   layer instead.

The desktop side extracts `full_access_budget()` in
`harness_bridge.rs` (single source of truth) and the production
dependency stays feature-free: `inbharat-harness-core` gains the
`test-providers` feature only as a **dev-dependency**, so
`EchoModelProvider` can drive an end-to-end regression run.

## 4. Regression tests

- `inbharat-harness-core`: `production_full_access_budget_passes_validation`
  — the exact production budget shape validates; 64 MiB + 1 stays
  rejected (the runaway backstop holds).
- `unoone-power` (desktop): `full_access_budget_is_accepted_by_the_harness`
  — `HarnessBuilder::local_embedded` + `EchoModelProvider` +
  `StaticConfirmationProvider{AllowedOnce}`, `RunOptions` with
  `CapabilitySet::all_local()`, explicit L3, and the production
  budget. Asserts `run()` survives validation *and* completes with a
  balanced session replay — the failure mode that shipped (throw at
  validate) can no longer pass silently.

## 5. Live verification checklist (post re-stage)

- [ ] Full access ON → long-coding request runs the L3 lane
      (`harness_chat` returns route "full-access-l3", no fallback)
- [ ] Agent writes multi-file app via fs tools (no capability denial)
- [ ] Playwright runs via `process.run`, exit 0
- [ ] App served on localhost:8199 opens in the app's Browser workspace
- [ ] Full access OFF → same request stays vault-only (rollback intact)