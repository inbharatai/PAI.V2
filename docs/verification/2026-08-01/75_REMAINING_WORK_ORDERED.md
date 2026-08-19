# 75 — Remaining work, ordered (updated 2026-08-02 as of main 0741da4; drive still staged from 2b543a3-era)

What is DONE (merged to main, CI green on the triggered gates):

- frontend embedding root cause fixed + byte gate green in CI and on drive
- recording privacy retention enforced; SUMMARY_ONLY UI-disabled
- browser typed+verified actions; redirect policy (PR #12)
- agent honesty (Option<f32> confidence, schema validation, repetition breaker)
- vault correctness Waves 1 (PR #8) + identity invariance + transactional setup
- Wave 3 migration core (PR #11) + read-path (PR #15)
- cross-platform crypto contract vectors, both directions (PR #13, #14)
- MobileVaultRepository (PR #16): unlock/read/write/tombstone, 17 vault-tests
- mobile protection tree-pointer redesign; lint baseline; dead-code sweep
- drive restaged from 2b543a3-era binaries: Power 1821A65D (embedding VERIFIED
  on drive), Dock E81D433E, Starter 729B3A52; 545/545 strict; starter exit 0
- Kotlin AAD escaping parity with serde_json + 10 pinned escaping vectors
  (PR #18); all four gates green on main f0a037b
- bidirectional envelope seal (PR #19): committed Kotlin-authored envelope,
  Android CI proves writeRecord reproduces it BYTE-FOR-BYTE, Desktop CI reads
  it through the real Vault::open → unlock → read_record incl. the
  aad_version-2 tamper check — a phone-written record provably reads on the
  laptop, machine-checked every run
- Room cache encrypted at rest (PR #20): SQLCipher SupportOpenHelperFactory
  under a Keystore-wrapped passphrase; reset-on-upgrade/key-loss equals the
  routine detach-clear; EncryptedCacheHeadlessTest is the device-side proof
  (queued for physical acceptance)
- vault wiring complete (PR #21): drive vault is the canonical notes/memories
  store — in-banner Compose unlock (Argon2id off-thread, backlog drain),
  write-through from NotesViewModel + ActionExecutor (incl. voice memos and
  bulk deletes with tombstones), MemoryModule preferences/corrections via
  callback with revisioned upserts (same record id, revision+1), offline
  pending-writes + pending-tombstones queue, schema v3. 832 local unit tests
  green + all CI checks green on 258cf3d

## Tier 1 — do next (small, high value)

1. **Run Wave 3 migration on the real drive** — interactive: launch the
   drive app, unlock, call `migrate_plaintext_documents_to_vault`, then
   enumerate VAULT/documents + VAULT/memory to prove plaintext is gone and
   records list/search still work (read-path now honors migrated records).
   SAFETY: back up first (RECOVERY/package-backups pattern); synthetic-tested.

(Former items 2 and 3 — encrypted Room cache, MobileVaultRepository app-flow
integration — are DONE above via PR #20 and PR #21.)

## Tier 2 — physical acceptance (human gates, all scripted in §9 handover)

4. **Android phone round trip** (§9/#67-68): USB-C attach → SAF grant →
   unlock via MobileVaultRepository → write shared record → Windows reads it
   → Windows updates it → Android sees tombstone. No private data in logcat.
5. **Recording-privacy physical enumeration** (61): each mode, prove which
   vault records exist after stop; PRIVATE_SESSION keeps nothing. Needs a
   microphone-consenting human — most important user-facing promise.
6. **Browser redirect live validation** (RequiresReview UI path) + live typed
   actions with screenshots against a controlled page.
7. **Auto-launch journeys** (58): dock install, unplug/reinsert, single
   instance focus, removal cleanup, reconnect.
8. **First-use vault on a packaged COPY** — prove vault.id/manifest/version
   byte-invariance physically (source-tested; physical pending).

## Tier 3 — larger product/engineering

9. **§10 hardware**: signed read-only SYSTEM partition + encrypted VAULT
   partition + A/B UPDATE partition; Authenticode-signed exes + Ed25519
   release manifest (public key embedded in Starter/Dock/Power). Only after
   the vault is plaintext-free (#1 done).
10. **Summariser** (Gemma) so SUMMARY_ONLY can be re-enabled truthfully.
11. **macOS**: out of scope in every claim made so far; no builds exist.

## Known small debts (not blocking)

- Rust header comments claim base64 for fields that are actually HEX —
  cosmetic doc fix; code is correct, Kotlin matches code.
- Migrated Document listings report size 0/unknown type until a metadata
  v2 envelope records them (recorded honestly, not fabricated).
- Desktop `list_documents` for migrated originals shows DocumentType::Txt
  generically — same cause as above.
