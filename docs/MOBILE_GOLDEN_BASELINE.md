# UnoOne Mobile Golden Baseline — Protection and Status

**Updated:** 2026-07-29
**Tag:** `mobile-golden-baseline-v2`
**Commit:** `8b66e3e0fa11d462e1676db6ea936ef00f745ada`
**Protected path:** `android-app/UnoOneAgent/` (344 files)
**Status:** FROZEN after the reviewed Pocket AI USB auto-open integration

## Protection

Two verification scripts exist:

- `scripts/verify-mobile-untouched.sh` — bash script for CI/terminal
- `scripts/verify-mobile-untouched.py` — Python script for environments with Python 3
- `scripts/verify-mobile-untouched.ps1` — PowerShell script for Windows

All three compare the current state of `android-app/UnoOneAgent/` against the
`mobile-golden-baseline-v2` tag. CI also verifies every protected file against
`scripts/MOBILE_GOLDEN_HASHES.txt`.

### CI Integration

Add to any CI pipeline:

```yaml
- name: Verify mobile untouched
  run: bash scripts/verify-mobile-untouched.sh
```

Or:

```yaml
- name: Verify mobile untouched
  run: python3 scripts/verify-mobile-untouched.py
```

### Pre-commit Hook (optional)

```bash
# .git/hooks/pre-commit
bash scripts/verify-mobile-untouched.sh || exit 1
```

## Pocket AI integration

The existing UnoOne Android app now handles the physical prototype Pocket AI
USB attachment. VID/PID is only an attachment hint; product identity still
requires schema-v2 `manifest.json`, matching `VERSION`, and matching
`VAULT/identity/vault.id` through Android's Storage Access Framework.

## Current Android Status

- **Last verified commit:** `8b66e3e0fa11d462e1676db6ea936ef00f745ada`
- **Build status:** app and vault module compile
- **Tests:** vault unit tests and Android lint pass
- **Changes since baseline:** ZERO (verified by `git diff`)

## What NOT to do

- Do not edit any Kotlin file in android-app/
- Do not modify Android Gradle files
- Do not bypass manifest and vault identity validation
- Do not refactor Android modules
- Do not copy desktop code into Android
- Do not change Android dependencies
- Do not update Android documentation in a way that changes behavior
- Do not apply formatting-only changes to Android code
