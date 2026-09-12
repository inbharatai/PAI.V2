# 84 — Vault Tools: Truthful Results, but verify_vault Was a Dead End (No Baseline, No Way to Create One)

**Date:** 2026-09-12
**Scope:** `apps/desktop/src-tauri/src/main.rs` (first-unlock baseline bootstrap)
**Posture:** seventh live-caught defect during user-perspective pendrive acceptance.

## 1. Symptom (live, through the real app UI)

Chat, full-access session: *"Use your vault tools: search my notes for SILT,
list the documents, verify the vault integrity."*

The agent called all three tools (`pai.search_notes`, `pai.list_documents`,
`pai.verify_vault`) and reported their results faithfully — tool transport and
honest reporting are correct (wire-trace verified: all three calls and their
exact result strings seen on the model-server wire).

- `search_notes` → "No encrypted-vault results for 'SILT'." — **truthful**:
  the Memory view shows "No memories yet" (the 19 encrypted records on the
  drive are system records, not notes/memories/documents).
- `list_documents` → "No documents available." — **truthful**: the Documents
  view shows "No documents yet".
- `verify_vault` → "No manifest found. Run generate_manifest first to create
  a baseline." — **truthful but a dead end**: `VAULT/config/manifest.json` did
  not exist, and **no UI component anywhere calls `generate_manifest`** — the
  tauri.ts binding exists, but zero call sites. The tool instructed the user
  to do something the application gives them no way to do.

## 2. Root cause

The vault-security baseline is a first-run artifact that nothing ever created:
not the drive staging pipeline (which generates the *package* manifest at
`D:\UNOONE\manifest.json` — a different artifact covering app/model/speech
assets), not the app at unlock, and not any UI surface.

## 3. Fix

`unlock_vault` (main.rs): on the first successful unlock — the moment that
*is* the known-good state — bootstrap the security baseline by calling
`security::generate_manifest` when `VAULT/config/manifest.json` is missing.
Best-effort only: a baseline failure never blocks unlocking, and an existing
baseline is never overwritten, so tampering-evidence semantics are unchanged.

## 4. Post-fix acceptance (must be re-run live)

Fresh unlock on the drive → `VAULT/config/manifest.json` exists → chat:
"verify the vault integrity" → `pai.verify_vault` returns a real verification
result (files verified against the baseline), not the dead-end instruction.