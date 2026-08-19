# 60 — Browser Workspace

**Status: BUILDS_NOT_RUNTIME_TESTED** (source-verified; live-page journeys pending human gate).

## What changed (all defects from §4.1)

| Defect | Fix | Proof |
|---|---|---|
| `ExecuteScript` arbitrary model-driven script execution (L66) | Variant removed entirely from the enum; no replacement | compile: enum has no such variant; frontend union no longer lists it |
| `browser_execute` returned a script blob and reported success before anything ran (L237+) | The backend now executes each action via `eval_with_callback` on the bound webview and parses the returned JSON envelope; success is only reported from real page results | `browser_execute_sync` + `ok_result` |
| Screenshot: `{"note": "Frontend should use…"}`, `screenshot_path: None` | Real PNG capture of the live window region (`screenshots` crate) written to `%TEMP%\unoone-browser\browser-<ts>.png`, SHA-256 of bytes returned | `capture_screenshot` |
| Session: start returned a success string without a session | Session now binds a `window_label`; every action resolves and fails truthfully when absent | `browser_start_session`, session guard in execute |
| Escaping `url.replace('\'', "\\'")` | All interpolation via `serde_json::to_string` (JSON string literals) | `js_string_literal` + tests incl. smuggling payloads |
| No URL scheme validation | Allowlist http/https; javascript:, file:, data:, vbscript:, about: refused before any script is built; dotted-host upgrade to https; 8 KiB cap | `validate_navigation_url` + 13 tests |
| Bridge survival across navigation | The bridge is reinjected inside every eval envelope; navigate waits for readyState+URL via polling | `bridge_envelope`, `poll_page_ready` |
| Risky actions unconfirmed | In-page risk probing: submit buttons, file uploads, `a[download]` refuse with `CONFIRMATION_REQUIRED`; UI prompts and retries only after explicit consent | `risk_probe`, `BrowserWorkspace.runAction` |
| Selector-not-found lied | Bridge returns `ok:false, error:'Selector matched nothing: …'` → failure | tests + envelope parse |
| Session/profile cleanup | `Close` destroys window + clears tokens; `ClearSession` clears page storage and honestly reports that WebView2 profile data is not exposed via Tauri | actions + UI wiring |

## Deterministic tests

**35 browser tests pass** (`cargo test -p unoone-power browser`) covering
scheme validation, injection smuggling (quote/backslash/newline/unicode),
script construction, verification predicates, envelope parsing, confirmation
detection, scroll caps, history actions.

## What still needs the human gate

Click/type/fill against a real local page and a public site, bridge survival
across navigation to a different origin, screenshot pixel sanity, bad-selector
failure visible in UI, confirmation dialog UX. These require the packaged app
with an unlocked vault and interactive input — this session is unattended.
