# 90 — Defect #13: Preference Writes Break the Package Integrity Gate

**Date:** 2026-09-12
**Scope:** `apps/desktop/src-tauri/src/security.rs`
**Posture:** live-caught on the re-staged drive (bundle `03cc1b2`), fixed same day.

## 1. Live catch

Re-staged the drive with the PR-16 bundle, unlocked (fresh baseline,
23/23 entries green), then ran the Hindi speech round-trip through the
real Accessibility UI. TTS failed with:

```
speech backend is not ready: Pocket AI package integrity gate failed:
manifest_valid=false hmac_valid=true entries_failed=1;
SHA-256 mismatch: \\?\D:\UNOONE\VAULT\config\accessibility.json
```

Sequence: the baseline (bootstrapped at unlock) hashed
`VAULT/config/accessibility.json`. Selecting the Hindi voice in the
Accessibility view fires `set_accessibility_status`
(`apps/desktop/src-tauri/src/main.rs:1337`), which **rewrites that file on
every preference change** — voice language, contrast, font scale. Any
preference change therefore bricks the integrity gate, and the gate
hard-blocks TTS/STT for the rest of the session. The English round-trip
had passed earlier only because "en" was already the persisted value —
selecting it rewrote identical bytes.

This is the same defect class as #12 (runtime state pinned by a static
baseline), one layer deeper: not an empty lock marker but a legitimate
user preference file.

## 2. Fix

`is_runtime_mutated_path` now also excludes `config/accessibility.json`
— user preferences are runtime mutations **by design**, not tampering.
Additionally, `verify_manifest` skips entries at excluded paths, so
baselines generated before this fix (which still carry the stale
`accessibility.json` entry) verify green instead of failing the whole
gate — old drives recover without a re-baseline.

Everything else in `config/` (`manifest.key`, staged defaults) stays
pinned; tampering with static vault content is still caught.

## 3. Tests

- `baseline_excludes_runtime_mutated_paths` — fixture now contains
  `config/accessibility.json`; asserts it never leaks into the baseline.
- `verification_survives_accessibility_settings_change` (new) —
  1. a fresh baseline contains no accessibility entry;
  2. simulating a mid-session language change keeps verification green;
  3. an old-style manifest re-signed with a stale accessibility entry
     verifies green (skipped, not failed).

`cargo test --workspace` green, `cargo clippy --workspace --all-targets
-D warnings` clean, `cargo fmt --check` clean.

## 4. Live acceptance (post-merge, re-staged drive)

- [ ] Unlock bootstraps fresh baseline (23/23, HMAC valid)
- [ ] Switch voice language to Hindi in the Accessibility view
- [ ] Verify manifest stays green **after** the switch (verify-vault)
- [ ] Hindi TTS + STT round-trip passes
- [ ] Switch back to English; manifest still green; English TTS passes
- [ ] Chat mic STS in both languages with preference flips mid-session