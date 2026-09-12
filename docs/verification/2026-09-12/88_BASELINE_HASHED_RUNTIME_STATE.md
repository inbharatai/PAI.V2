# 88 — The Security Baseline Hashed Runtime-Mutable State, Breaking the Speech Lane on Every Lock

**Date:** 2026-09-12
**Scope:** `apps/desktop/src-tauri/src/security.rs` (baseline scan exclusions)
**Posture:** live-caught during speech acceptance on the final build.

## 1. Symptom (live, through the real app)

Speech round-trip probe through the app's Tauri commands:

- `synthesize_speech` "The harbor station opens its tall windows at dawn."
  → **PASS**: 3.7 s of 22050 Hz audio in VAULT/recordings.
- `transcribe_audio` on the file TTS just produced → **PASS, word-perfect
  round-trip**: "The harbor station opens its tall windows at dawn." (100%
  word recall).
- `transcribe_audio` on the drive's pinned acceptance fixture → **PASS**,
  transcript matches the pinned sample.
- `synthesize_speech` Hindi → **FAIL**:

```
speech backend is not ready: Pocket AI package integrity gate failed:
manifest_valid=false hmac_valid=true entries_failed=1;
SHA-256 mismatch: \\?\D:\UNOONE\VAULT\locks\.vault-locked
(expected e3b0c44…empty-hash, got 4567c3d9…)
```

## 2. Root cause

The InBharat Audio lane gates on `security::verify_manifest` — the vault
**security baseline** (`VAULT/config/manifest.json`, bootstrapped at first
unlock). `scan_directory` hashed *everything* under VAULT with no exclusions
— including:

- `locks/.vault-locked` — rewritten on every lock/unlock **by design**
  (write-only marker, no reader anywhere),
- `recordings/` — TTS output and user recordings are created at runtime,
- `config/manifest.json` — the manifest would hash itself on regeneration.

So the baseline was invalid the moment the app locked the vault after
bootstrapping (expected hash = the empty file's hash, actual = the marker's
content). Every audio.cpp-gated request (Hindi TTS — the legacy fallback
covers only English) then failed the integrity gate. The English TTS/ASR
pair kept working only because it routes through the ungated legacy plane.

## 3. Fix

`SecurityManager::scan_directory` now skips runtime-mutable VAULT state:
`locks/`, `recordings/`, and `config/manifest.json`. The baseline pins
static vault content only (identity, config keys, records) — which is the
only thing a static baseline *can* verify.

## 4. Regression tests

- `baseline_excludes_runtime_mutated_paths` — locks/recordings/manifest.json
  never appear as entries; identity + static content stay covered.
- `verification_survives_lock_and_recording_writes` — the exact live
  sequence (lock marker rewritten + new recording) verifies clean, while
  tampering with static content is still detected.

## 5. Post-fix acceptance (live, on the re-staged drive)

Delete the drive's stale `VAULT/config/manifest.json` (it contains the
pinned lock-marker hash) so first unlock re-bootstraps a fresh baseline →
Hindi TTS synth → ASR round-trip → all pass with the gate green.