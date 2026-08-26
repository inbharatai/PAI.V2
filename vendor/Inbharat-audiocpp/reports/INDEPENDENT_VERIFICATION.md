# Independent Adversarial Verification — 2026-08-20

Performed by a separate verifier against the tracked tree (commit `ed0901c`), with all builds in scratch dirs and the tracked tree left unmodified. The verifier was instructed to be adversarial and try to break the invariants.

## Result: 8/8 CONFIRMED, 0 VIOLATED

1. **Clean release build + ABI** — 15/15 tests; default build exports exactly 79 symbols (independently re-counted), experimental build (`IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=ON`) exports exactly 94. Experimental build runs 16/16 (one extra gated-module test); `IBAUDIO_REMOTE_PROVIDERS=ON` build 15/15.
2. **ASan+UBSan gate** — 15/15, sanitizers confirmed genuinely linked (`libasan.so.6`, `libubsan.so.1`), not a no-op gate.
3. **Strict path policy fails closed (dynamic probe)** — `ibaudio_sha256_file` on `/etc/hostname` with no `allowed_model_root` → `SECURITY_ERROR`; `..` traversal payload rejected; inside-root file accepted (positive control).
4. **Remote provider gate** — `route()` checks `caps.remote && !remote_allowed` before scoring; forcing Sarvam into a live registry with remote disallowed still routes to local providers.
5. **Gated module symbols** — absent from default `libibaudio.so`, present in experimental build; delta is exactly the 15 gated symbols.
6. **MCP gateway** — real stdio JSON-RPC: `server/discover`, `tools/list`, `tools/call audio.models` (4 models, KWS honestly deferred), `tools/call audio.detect_language` on mixed text (nonzero hindi/hinglish, honestly labeled "not acoustic LID").
7. **Supply-chain / fake-inference audit** — zero network calls, zero subprocess, zero fake inference. The reference engines are transparently labeled; the only `Placeholder` is inside the gated voice_clone module, disclosed in docs and CMake.
8. **Tracked tree unmodified** — `git status` clean before and after.

## One observation for follow-up — RESOLVED (commit 7280e3f)

`ProviderRegistry::route()`'s remote gate was correctly implemented but not wired into production model resolution. This is now fixed: the capability router drives production model resolution via `resolve_for_family` + `serves_family`, the remote policy is enforced there (new `allow_remote_providers` runtime option, default offline), and an anti-rot test (`test_remote_gate` in the `ibaudio.provider` lane) asserts the gate against the live registry with local+remote stubs — a refactor that bypasses the gate now fails the test. Verified 15/15 release + 15/15 ASan/UBSan after the change.
