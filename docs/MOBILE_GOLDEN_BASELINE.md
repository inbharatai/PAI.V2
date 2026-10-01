# UnoOne Mobile Protection — Policy and Status

**Updated:** 2026-10-01
**Protected path:** `android-app/UnoOneAgent/`
**Mechanism:** committed tree-hash pointer + per-file blob hashes

## How protection works today

The historical `mobile-golden-baseline-v2` **tag** mechanism is retired — the
tag no longer exists and the `verify-mobile-untouched.*` scripts that depended
on it were deleted on 2026-10-01 (they exited 2 unconditionally once the tag
was gone). Protection now rests on two committed files:

- `scripts/MOBILE_PROTECTED_TREE` — the expected value of
  `git rev-parse HEAD:android-app/UnoOneAgent` (a git **tree** hash: it pins
  the committed state, not the working tree).
- `scripts/MOBILE_GOLDEN_HASHES.txt` — the per-file git blob hashes of the
  protected tree, so a re-baseline review can be audited file-by-file.

CI (`.github/workflows/mobile-protection.yml`, plus the mobile-protection job
in `desktop-ci.yml`) fails any push where the actual tree hash differs from
the pointer. Because the pointer is a committed file, re-baselining is an
ordinary reviewable commit — never a workflow edit or a tag move.

## Re-baselining (the ONLY way to change Android code)

1. Commit the Android changes first.
2. Run `bash scripts/regen-mobile-golden-hashes.sh` — **after** the commit:
   the script pins `HEAD:android-app/UnoOneAgent`, so running it before
   committing baselines the OLD tree and CI fails with expected ≠ actual.
3. Commit the updated `scripts/MOBILE_PROTECTED_TREE` +
   `scripts/MOBILE_GOLDEN_HASHES.txt` together with (or immediately after)
   the change commit.

Local check (same comparison CI makes):

```bash
test "$(cat scripts/MOBILE_PROTECTED_TREE)" = "$(git rev-parse HEAD:android-app/UnoOneAgent)" && echo PASS
```

## Pocket AI integration

The UnoOne Android app handles the physical Pocket AI USB attachment.
VID/PID is only an attachment hint; product identity still requires the
schema-v2 `manifest.json`, matching `VERSION`, and matching
`VAULT/identity/vault.id` through Android's Storage Access Framework.

## What NOT to do

- Do not commit Android changes without re-baselining in the same push.
- Do not bypass manifest and vault identity validation.
- Do not hand-edit `MOBILE_PROTECTED_TREE` (regenerate it; the value must be
  the real tree hash).
- Do not reintroduce tag-based or hardcoded-commit pin protection.