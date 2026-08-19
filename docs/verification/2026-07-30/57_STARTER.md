# 57 — Starter verification

**Status: VERIFIED_WORKING** for the verification path; the remaining journeys
below need a human at the machine.

## Verified this session

`D:\UNOONE\Start UnoOne.exe --verify-only` → **exit 0**, JSON output:
```json
{ "failure_count": 0, "failures": [], "valid": true }
```
Run after restaging (sha256 `C49612CD1B7F48F5168CD64D3BC4FFFCC4BCC40870D33DAEF18448FADCD9CDA3`,
unchanged from its manifest-declared value).

Also verified by design behavior: during the failed first restage, the native
verifier exited 1 (BOM'd manifest) and the staging script **rolled the package
back automatically** — the tamper-detection + rollback machinery genuinely
works end to end.

## Not exercised (BLOCKED_BY_ENVIRONMENT / human gate)

Normal start, changed drive letter, missing-app path, tampered-manifest and
hash-mismatch journeys on the LIVE drive (deliberate corruption of the only
physical package is not something to do from an unattended agent run against
a user's drive without explicit consent), ordinary-USB ignore, Dock install
accept/decline prompts, already-running instance, repeated launch. These are
interactive or destructive-by-design tests.
