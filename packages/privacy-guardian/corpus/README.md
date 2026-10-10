# Authored guardian fixture corpus

`v1/corpus.json` — 60 items (30 SCAM, 30 LEGIT) **authored by the implementer for testing**. No real
victims, accounts, or captured messages; lookalike domains are fictional. The same author wrote the rules
and the corpus, so the numbers below are a regression floor, not a detection rate for real-world scams.

Measured by `cargo test -p unoone-privacy-guardian --test corpus` and pinned in `v1/metrics.json`
(the test fails on drift so changes are deliberate):

| metric | v1 |
|---|---|
| harmful misses (ALLOW on SCAM) | 0 |
| under-severity (WARN where BLOCK expected) | 0 |
| false alarms (WARN/BLOCK on LEGIT expected ALLOW) | 2 — `legit-hard-01` regional suffix `google.co.in` (SuffixSwap), `legit-hard-03` third-party payment processor link whose display text names the shop (DestinationMismatch). Both are WARN with a verification route, not BLOCK. |
| legit BLOCKED | 0 |
| tolerated reviews by design | 2 — first payment to a new payee; urgent message adding a new recipient |

Items `scam-hard-*` were written to stress gaps (compromised known account asking for gift-card codes,
short brand label, link shortener without urgency, payee name variant, sensitive share to a known contact).
Two of them (`scam-hard-01`, `scam-hard-03`) were initially missed; the fixes (credential-request +
attachment rule, bounded shortener list) are in guardian v1 and the corpus was not softened.

The Kotlin mirror (`android-app/.../core/guardian`) runs the same file and must reach identical decisions.
