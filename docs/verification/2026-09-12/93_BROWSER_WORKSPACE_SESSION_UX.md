# 93 — Defect #18: Browser Workspace UX — Navigate Without a Session Fails, Page Loads Unseen

**Date:** 2026-09-12
**Scope:** `apps/desktop/src/src/components/BrowserWorkspace.tsx`
**Posture:** live-caught on the re-staged drive (bundle `03cc1b2`), fixed same day.

## 1. Live catch

The user reported: "the browser session is slow and i tried
google.com it failed". Reproduction through the real UI found the
underlying failures:

1. Typing `https://www.google.com` and pressing **Navigate** without
   first pressing **Start Session** shows a developer-facing error:
   `FAILED: No active browser session. Call browser_start_session
   first.` — a Rust command name shown to an end user. Nothing hints
   that a "session" must be started; the address bar looks like a
   browser's.
2. When the flow *does* run (session started, navigate clicked —
   ~1 s each, so the lane is not actually slow), the page renders in a
   separate `browser-workspace` OS window that opens **behind** the
   main window. google.com loads fine (title "Google" confirmed over
   CDP) — but unseen, which reads as "failed".

## 2. Fix

`BrowserWorkspace.tsx`:

- **Navigate auto-starts the session.** `startSession()` now resolves
  a promise when the window exists (or fails), and `navigate()` calls
  it before the Navigate action. The user never needs to know
  sessions exist: type a URL → page appears.
- **The browser window comes to the front** — on session start and
  after every navigate (`setFocus()`), and it opens centered instead
  of at an arbitrary default position.
- **Re-clicking Navigate after closing the pop-up reuses the window**
  when it still exists (label collision handled) instead of erroring.

## 3. Tests

Frontend build + `tsc --noEmit` clean; no Rust changes. Live
checklist below.

## 4. Live verification checklist (post re-stage)

Verified live on the re-staged drive (bundle `f08f8b8`),
2026-09-12 (`browser-retest.js` PASS):

- [x] Fresh app → Browser view → type google.com → Navigate: page
      appears in front without pressing Start Session first (cold
      Navigate auto-started the session; google.com loaded in ~2 s and
      showed as a live CDP target)
- [ ] Session indicator shows active; Stop Session closes the window
- [ ] Navigate after manually closing the window recovers
- [ ] Chat-side browser.act lane still works (same session state)