# 56 — First-use vault identity invariance (§10)

**Status: VERIFIED_WORKING** at the source + test level (5 new tests, all passing,
58/58 crate tests). Physical on-drive first-use is a human gate (needs a copy of
a packaged drive and interactive setup).

## Guarantees implemented

1. **Byte-identical vault.id across first-use setup.** `create_with_vault_id`
   preserves the packaged bytes verbatim; test asserts byte equality before/after.
2. **No silent re-initialisation.** `create`/`create_with_vault_id` refuse
   (`NotPermitted`) when a header already exists or a *different* identity is on
   disk — and the test proves the refused attempt wrote NOTHING (identity file
   untouched, no header created).
3. **Correct password unlocks after restart; wrong password fails** — tested via
   drop/reopen.
4. **Empty vault id rejected.**
5. Recovered-session fix carried: `vault.rs` never rewrites identical vault.id
   bytes even with whitespace differences (preserves `manifest.vault.id_sha256`).

## Drive-side fact

`D:\UNOONE\VAULT\identity\vault.id` fingerprint (hash+size only):
sha256 `[REDACTED-VAULT-ID-SHA256]`, 20 bytes.
`VAULT/header/header_a.json` exists — this vault is already initialised, so any
create attempt against it is now refused by design.

## Tests

`cargo test -p unoone-vault-core`: 58 pass, 0 fail, including
`identity_tests::{first_use_setup_preserves_vault_id_bytes,
setup_refuses_to_reinitialise_existing_vault,
create_with_vault_id_refuses_mismatched_existing_identity,
correct_password_unlocks_after_restart_wrong_fails,
create_with_vault_id_rejects_empty_id}`.
