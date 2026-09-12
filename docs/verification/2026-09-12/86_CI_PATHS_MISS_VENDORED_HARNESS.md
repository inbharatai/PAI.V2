# 86 — CI Path Filters Miss vendor/inbharat-harness: A Binary-Changing Commit Ships Without CI or a Bundle

**Date:** 2026-09-12
**Scope:** `.github/workflows/desktop-ci.yml`, `.github/workflows/pocket-ai-windows.yml`
**Posture:** ninth live-caught defect (pipeline gap) during pendrive acceptance.

## 1. Symptom (live)

Merged the `process.run` env fix (PR #13, touches only
`vendor/inbharat-harness/**` + docs) to main. Only **Mobile Protection** ran.
Neither **Desktop CI** nor **Pocket AI Windows Bundle** triggered — because
both workflows' `paths:` filters list `apps/**`/`packages/**`/
`vendor/Inbharat-audiocpp/**` but **not `vendor/inbharat-harness/**`**, even
though the harness crate compiles directly into `UnoOnePower.exe` via
`packages/pai-harness-adapter`.

Consequence: any harness-only change — including this acceptance cycle's
process-spawn security fix — reaches main with no desktop CI and produces no
re-built drive bundle. The staged drive would silently keep running the
pre-fix binary forever.

## 2. Fix

- `desktop-ci.yml`: added `vendor/inbharat-harness/**` to both the `push` and
  `pull_request` path filters.
- `pocket-ai-windows.yml`: added `vendor/inbharat-harness/**` and
  `Cargo.lock` (a dependency bump changes the shipped binary too; the desktop
  workflow already tracked both).

## 3. Acceptance

After merging: a commit touching `vendor/inbharat-harness/**` must run both
Desktop CI and Pocket AI Windows Bundle on main. Confirmed live with the
re-merge of the harness fix (both workflows ran, bundle artifact produced).