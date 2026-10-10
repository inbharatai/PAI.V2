# unoone-privacy-guardian

Host-owned, deterministic risk check for brief §3.6. Runs at native tool boundaries **before** opening a
link, sending a message, changing a recipient/payee, sharing a file, granting a connector, spawning a child
or executing a task. Kotlin mirror: `android-app/UnoOneAgent/core/src/main/java/com/unoone/agent/core/guardian`.

## Contract

| Decision | Meaning | Enforcement (`enforce`) |
|---|---|---|
| `ALLOW` | no warning (signals may still be noted in the receipt) | proceeds |
| `WARN` | needs a fresh explicit human decision tied to the exact fingerprint (destination/amount/data) | proceeds only with `Acknowledgement::by_human(&decision, now)` for the same fingerprint, within 5 minutes |
| `BLOCK` | secrets, or high impact without a verification route | never proceeds; an acknowledgement cannot lift it |

* **Typed inputs only.** `Intent` and `Context` are built by native code. Email/PDF/web/QR/tool output/synced
  record text is `Untrusted` DATA: it can add informational signals (`CredentialRequested`, `Urgency`,
  `UntrustedInstructionsPresent`) but can never lower severity, change contacts/allowlists/recipients or acknowledge.
* **Model text only explains.** `Decision::with_model_explanation` appends a masked, bounded note; severity,
  signals and fingerprint are untouched.
* **No network.** Registrable domains come from a bounded local public-suffix snapshot (`domain.rs`,
  version 1, not the full PSL); unknown TLDs are treated as one label. No reputation lookup.
* **Secrets never leave unmasked.** `secrets::mask` runs over every fingerprint, explanation, receipt and
  correction. `Untrusted::as_prompt_data` masks before anything reaches a model prompt.
* **Outbound policy default offline.** `connector::EgressPolicy` refuses every URL unless a consented,
  unexpired `ConnectorManifest` declares the exact https host; byte and daily ceilings are enforced outside the
  model; `revoke` stops new requests. Token erasure is the credential store's duty; provider-side deletion is
  never promised.

## Signals (structural, local)

Destination vs display text, registrable-domain lookalike (homoglyph skeleton, edit distance, brand embedded
/ subdomain / suffix swap), IDN/punycode, IP literal, credentials in URL, dangerous scheme, credential-harvest
path, link shortener, lookalike recipient, new recipient, secret disclosure (OTP/password/recovery phrase/token/
card), credential request, urgency, sender auth fail/impersonation, changed payee detail, new payee, unknown
amount, bank details to unknown recipient, unexpected attachment, sensitive share, broad data export,
connector consent/broad scope/invalid manifest, child scope/network/depth, network without connector,
high-impact action, untrusted instructions present, requested by untrusted content.

## Hooks (this slice)

Rust: `personal-provider-adapters/src/guardian.rs` (Google manifest, egress in `Google::call`, gate in
`Google::commit`), `pai-harness-adapter/src/personal_children.rs` (child spawn gate, untrusted source
wrapping), `personal_execution.rs::selected_notes` (masking), desktop `providers.rs`
(`GuardianPreview`/`GuardianCorrection`, `Commit.acknowledged_fingerprint`, consent preview, OAuth link check).
Receipts/corrections go through the runtime's tiny API `Ledger::record_guardian_note` (same encrypted
ledger/outbox, `Action::Edit`, goal unchanged).

## Corpus and honest numbers

See `corpus/README.md`. Authored fixtures (not real victims); pinned metrics in `corpus/v1/metrics.json` and
golden decisions in `corpus/v1/decisions.json` (the Kotlin mirror must reproduce them exactly).

```
cargo test   -p unoone-privacy-guardian --locked --offline -j1
cargo clippy -p unoone-privacy-guardian --all-targets --locked --offline -j1 -- -D warnings
cargo run    -p unoone-privacy-guardian --example emit_decisions --locked --offline > corpus/v1/decisions.json
```
