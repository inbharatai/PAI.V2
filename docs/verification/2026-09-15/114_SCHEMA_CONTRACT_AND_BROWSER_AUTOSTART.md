# 114 — The background Deploy Was Invisible to the Model, and the Browser Was a Dead End After Every Restart

**Date live-caught:** 2026-09-15 (physical drive D:\UNOONE, staged main `3cba03a` — caught by the deploy-demo re-run, the first live run of the defect-#38 fix)
**Severity:** High — this pair is why the deploy demo still failed on the build that *contained* the deploy fix.

## 1. Live catch

The re-staged drive (`3cba03a`, both PRs merged) re-ran the build+test+deploy demo. Items 1–5 passed again (doc 112 §5), but:

```
12:42:01→ Running node 12:45:01✓ Failed: timeout:process.run: subprocess deadline
```

- **#38 follow-up (schema):** the agent deployed **foreground** — it never passed `background:true`. The engine supported it, the briefing taught it, but the `process.run` **input schema still declared only `program` + `args` with `additionalProperties:false`**. The schema is the model-facing contract; a 12B model follows the schema, not a description sentence. The deploy lane existed in code and was invisible to the model.
- **#39 (browser dead-end):** the agent tried to show the deployed app and its `browser.act` failed: *"no active session was provided in this specific turn's context"*. The browser session lives in backend state, dies on app restart, and only the BrowserWorkspace UI could create it — the model lane has no `browser_start_session` tool. So after every restart, `browser.act` was a dead end and the chat could never open the browser itself (the user's explicit ask: *"the chat should be able to access browsers and open it too"*).

## 2. Root cause

- **#38 follow-up:** the defect-#38 fix updated the tool description, the briefing, the validation, and the executor — but not `schemas("process.run")`. Half the surface was taught, the half the model actually reads was not.
- **#39:** `browser_execute_sync` returned `failure("No active browser session. Call browser_start_session first.")` when no session was bound, and nothing in the model's tool set could bind one. The window is a frontend-created `WebviewWindow` (`about:blank`, 1280×800, label `browser-workspace`) — but the backend can create the same window itself.

## 3. Fix

- **#38 follow-up (`tools.rs`):** the `process.run` input schema now declares `background` (`type:"boolean"` with a description of the detached/pid behavior); the output schema is an honest `anyOf` of the foreground shape and the `{background, pid}` shape. A test pins that the schema declares the flag — so a future schema revert fails CI, not the live drive.
- **#39 (`browser.rs` + `harness_bridge.rs`):** new `ensure_session` — bind the live session if its window exists, **rebind if the bound window is gone** (previously a stale session eval'd into a dead window), otherwise create the `browser-workspace` window (same label/geometry as the UI) and bind it. `browser_execute_sync` now routes every action through it, so the model lane and the user lane behave identically. The `browser.act` tool description now says the browser window opens itself on the first action. The user's explicit **Stop Session** button is untouched — this removes the dead end, not the control.

## 4. Tests

- `process_run_background_returns_pid_immediately` now first asserts `input_schema.contains("background")` — the schema is the contract and this pins it.
- Harness core: all tests green after the schema change (the manifest validator accepts the `anyOf` output object).
- Desktop: `cargo fmt`/`clippy`/`test` green (133/133).

## 5. Live acceptance (post-merge, on the re-staged drive)

- [ ] The agent, asked to build + test + deploy, passes `background:true` on the server launch (feed shows `Starting node in background`, no deadline failure)
- [ ] The deployed server answers plain HTTP after the run ends (script-side GET to `http://127.0.0.1:8200` returns the app HTML)
- [ ] `browser.act` succeeds on a fresh app start with no user-opened BrowserWorkspace (the window opens itself)
- [ ] The separate browser window shows the deployed app's page (CDP target at `localhost:8200` renders "Ping Board")
- [ ] The agent's answer reports the pid and the served URL
## 6. Follow-up defect #40 (live-caught on the merged build): the backend-built window had no message loop

The §3 `ensure_session` fix shipped and the deploy demo re-ran on the re-staged drive: `background:true` **worked** (feed: `Starting node in background… Done: started in background (pid …)`), the deployed server answered HTTP after the run, and the answer reported absolute paths + pid + URL. But the browser window itself still never appeared: no CDP `/json` target, not in the window list — and every `browser.act` eval failed with `Eval failed: …`.

**Root cause (forensics: no page reload, no Crashpad dumps, window absent from CDP the instant it was "built"):** `ensure_session` ran inside the harness_chat `spawn_blocking` thread, and `WebviewWindowBuilder…build()` was called there directly. On Windows a Tauri window built off the main thread gets **no message loop**: WebView2 never initializes, evals fail, the window never registers as a CDP target, and it dies with the spawning thread. The frontend's JS `new WebviewWindow(...)` works precisely because Tauri routes that through the main thread.

**Fix:** `create_workspace_window_on_main_thread` — `app.run_on_main_thread(builder.build)` with the result returned over an mpsc channel (`WINDOW_CREATE_TIMEOUT` 10 s), followed by `wait_for_window_ready` — a bounded probe (500 ms eval timeout, 200 ms interval, `SESSION_READY_TIMEOUT` 15 s) so the first real action never races WebView2 startup. Desktop suite stays green (138/138 on the branch with the multi-agent lane; 133/133 + this change in isolation).

**§5 items re-verified after this fix lands on the drive.**
