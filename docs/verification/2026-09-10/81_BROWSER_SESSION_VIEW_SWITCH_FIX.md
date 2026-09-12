# 81 — browser.act Was Structurally Unusable: Session Died on View Switch

**Date:** 2026-09-10
**Scope:** `apps/desktop` (browser backend, BrowserWorkspace view)
**Posture:** fourth live-caught defect during user-perspective pendrive acceptance.

## 1. Symptom (live, through the real app UI)

The documented browser lane contract — "Requires an active browser session
(the user opens the BrowserWorkspace first)" — is impossible to satisfy:

1. On the Browser view, click **Start Session** → session active, separate
   browser OS window opens, `example.com` loads. ✔
2. Switch to the **Chat** view (where the chat input lives) and ask the agent
   to use its browser tool. ✔
3. The agent's `browser.act` call returns *"No active browser session."*

The session was alive 60 seconds earlier and nothing stopped it — except
leaving the view.

## 2. Root cause

`BrowserWorkspace` is a *view inside the main window*, and its React unmount
cleanup called `stopSession()`:

```tsx
useEffect(() => {
  return () => { void stopSession(); };
}, []);
```

Switching to the Chat view unmounts the BrowserWorkspace component → the
session is torn down and the separate browser OS window destroyed. But the
chat input — the only place the user can ask the agent to browse — is on the
Chat view. So every path from "session started" to "agent asked to browse"
crosses a view switch, and the session never survives it. The
`browser_session_status` query did not exist either, so the remounted view
could not even re-sync with backend state.

## 3. Fix

- **`browser.rs`** — new `browser_session_status` command reporting
  `active` (session present **and** its OS window alive), `window_label`,
  `current_url`, `current_title`. `BrowserSession` derives `Clone`.
- **`BrowserWorkspace.tsx`** — the unmount cleanup no longer stops the
  session (the session and its separate OS window are backend-owned and
  survive view switches); on mount the view re-syncs `sessionActive` from
  `browser_session_status`. Stopping still happens only through the explicit
  **Stop Session** button.

## 4. Why CI never caught it

Browser tests cover argument validation and the action execution contract in
isolation; no test mounts/unmounts the real view while a session is bound, and
no automated test walks the user path Browser→Chat→agent-browser-call.

## 5. Post-fix acceptance (must be re-run live)

Start Session on the Browser view → switch to Chat → ask the agent to
navigate and report the page title: the session survives the switch and the
agent's `browser.act` executes against the live window.