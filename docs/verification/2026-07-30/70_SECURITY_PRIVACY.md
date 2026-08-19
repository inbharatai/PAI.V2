# 70 — Security & privacy review

## Strengthened this cycle

1. **No arbitrary script execution** — browser `ExecuteScript` removed (was
   unrestricted model-driven JS injection with naive quote escaping).
2. **URL scheme allowlist** — javascript:/file:/data:/vbscript:/about: blocked
   before script construction; JSON-literal interpolation only.
3. **Risky-action gate** — form submit/upload/download require explicit user
   confirmation (in-page probe + retry).
4. **Agent** — schema-validated tool calls, repetition circuit breaker, 240 s
   deadline, unmeasured confidence never promoted to 1.0.
5. **Vault** — transactional first-use setup; refuse re-initialisation; id
   bytes preserved; wrong-password rejection tested.
6. **Recording** — privacy levels enforced by policy crate with no wildcard
   match arms (new level = compile error until decided); temp WAV outside
   vault, delete-verified; buffers zeroized; SUMMARY_ONLY no longer a silent
   data-loss trap (disabled in UI).
7. **Mobile cache** — Room plaintext mirror now TTL-bounded and cleared on USB
   detach; detection failures are truthful; unused WAKE_LOCK dropped.
8. **devtools** Tauri feature opt-in only (cannot ship by accident).
9. **dev-bypass** feature is compile-gated and env-gated
   (`UNOONE_DEV_BYPASS=1`); never in release artifacts.

## Constraints honoured

No Defender/WDAC/AppLocker/SmartScreen/UAC/Android-permission weakening. The
llama server binds 127.0.0.1 only (observed). No model-generated shell
execution exists. No production tamper-proof claims for a writable prototype
drive.

## Known residual risks (honest)

- Mobile Room store is PLAINTEXT SQLite (no SQLCipher) — mitigated by TTL +
  clear-on-disconnect, not encryption. Recommendation: SQLCipher SupportFactory
  or removal.
- Browser screenshots store window pixels in `%TEMP%` (wiped at boot;
  not vault-encrypted).
- WebView2 profile data (cookies/cache) cannot be cleared through Tauri;
  `ClearSession` reports this honestly.
