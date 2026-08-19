# 64 — Documents, memory, vault

**Status: BUILDS_NOT_RUNTIME_TESTED** (document journeys);
**VERIFIED_WORKING** for vault-core invariants (58/58 tests).

## Source-level facts verified this session

- TF-IDF label honesty: `documents.rs` documents TF-IDF scoring explicitly as
  TF-IDF ("instead of fake relevance=0.5"). No "semantic"/"vector
  search"/"embeddings" claims in code or UI (grep over src + packages clean).
- Vault identity, reinit refusal, wrong-password rejection: 5 tests in
  `identity_tests` (see 56).
- Vault structure on the live drive is complete (identity/header/records/
  journal/locks/snapshots/transactions/attachments/recovery directories all
  present).

## Document journeys owed (human gate)

TXT/MD/CSV/HTML/PDF/DOCX/XLSX/PPTX parse, malformed files, duplicate names,
large documents, legacy formats; canonical encrypted copy in vault vs
plaintext store scan; search/delete/tombstone; restart persistence.
A/B header newest-valid handling, password change, write-ahead journal crash
recovery, path traversal, malformed record IDs, tampering, wrong vault ID,
unsupported schema, emergency lock. These run against the physical vault with
interactive unlock; an unattended run cannot enter the vault password.
