# 83 — Browser Lane: Every Verified Action Failed on Real Windows (Double-Encoded Eval Results)

**Date:** 2026-09-12
**Scope:** `apps/desktop/src-tauri/src/browser.rs` (eval result decoding + regression tests)
**Posture:** sixth live-caught defect during user-perspective pendrive acceptance.

## 1. Symptom (live, through the real app UI)

- Chat, full-access session, browser session Active: asking the agent to
  open https://example.com made it call `browser.act` (navigate) **12 times in
  a row**, each returning `"Navigation to 'https://example.com' did not reach
  a loadable page within 20s"`, until the harness step budget died
  (`budget_exceeded:agent.loop`), the bridge fell back to the read-only legacy
  agent, and the user got *"I don't have access to a web browser"* — a false
  capability denial from a session whose footer advertises the browser.
- The user-driven Browser view failed identically: clicking Navigate reported
  `FAILED: Navigation … did not reach a loadable page within 20s`.
- The page actually loaded every time (the navigate eval ran); only the
  *verification* of it failed.

## 2. Root-cause isolation (live forensics)

Probes against the running app, all through CDP into the real webview:

1. **`browser_eval` (async command path) with the exact page-info envelope
   script:** returns in 3 ms with correct data — but the raw string is
   `"{\"ok\":true,…}"` — **a JSON string containing the JSON envelope**.
2. **`browser_execute` with `GetPageInfo`:** fails instantly with
   `"Could not read page info — bridge unreachable"`.
3. **`browser_execute` with `ExtractPageText`:** fails with
   `"Bridge result missing 'ok' field"` — `parse_bridge_result` parses the
   outer JSON string and finds no `ok` key.
4. **`browser_execute` with `Wait`** (no eval): succeeds — command path,
   session state and window are all fine.

Conclusion: WebView2's `ExecuteScript` — what Tauri's `eval_with_callback`
bottoms out in on Windows — delivers **the JSON encoding of the script's
completion value**. Every bridge script ends by returning a string (the JSON
envelope), so the payload arrives **double-encoded**, and
`parse_bridge_result` — which expects the single-encoded envelope — rejects
every real webview result. All verified browser actions fail: the navigation
poll never sees a ready page (20 s timeout), extract/click/type/fill-form
return parse errors. The navigate eval itself still ran, which is why pages
loaded while being reported as failures, and why the model retried until the
budget died.

## 3. Why CI never caught it

The browser module's tests are deliberately deterministic ("no webview
required") and feed `parse_bridge_result` single-encoded envelopes — a shape
the real webview never produces. The encoding layer between
`eval_with_callback` and the parser was untested end-to-end.

## 4. Fix

`browser.rs`:

- New `strip_json_string_layer(&str) -> String`: if the raw callback result
  parses as a JSON string, return its inner value; otherwise pass through.
  Applied **once**, inside `eval_bridge` — the single choke point every
  browser action flows through (verified: the only `eval_with_callback`
  consumer in the workspace).
- Four regression tests pin the contract: the byte-exact double-encoded
  payload captured live must parse; single-encoded results pass through
  unchanged; non-string JSON results pass through; a plain JSON string is not
  over-stripped into a fake envelope.

Gates: desktop `cargo test` 108/108, adapter 14/14, `cargo clippy
--all-targets -D warnings` clean.

## 5. Post-fix acceptance (must be re-run live)

- Browser view → Navigate to a fresh HTTPS page → result box shows
  `OK (verified=true)` with url + title (no 20 s FAILED).
- Page Info / Extract Page Text report the real page contents.
- Chat → ask the agent to open a page and report its title: one `browser.act`
  call, correct title in the answer, no budget exhaustion, no fallback.