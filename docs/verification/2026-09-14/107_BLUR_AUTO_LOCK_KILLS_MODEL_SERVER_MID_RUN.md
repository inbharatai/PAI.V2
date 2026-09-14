# 107 — Defect #31: The Blur Auto-Lock Killed the Model Server Mid-Run

**Date live-caught:** 2026-09-14 (physical drive D:\UNOONE, staged main `ae0ab15`, during the long-coding acceptance run that validated the defect-#30 fix)
**Severity:** Critical for the agent lane — no long task can survive the user looking away.

## 1. Live catch

The second acceptance re-run was healthy: `task-board/` created, `index.html` + `server.js` + `app.js` written in sequence with cumulative prompt-token growth (the defect-#30 fix holding). At 3/4 files, the 5th llama-server request was released with **no "prompt eval time" line** — the request was aborted mid-flight — at exactly the window-blur + 5-minute mark. The CDP textContent dump then hung, and the user's capability panel showed the aftermath: "Local Model Inference: Implemented, Not Tested" — the manager was torn down while the llama-server process itself survived.

Timeline correlation: the user had switched to the Accessibility tab (window blurred) while the run continued in the hidden-but-mounted ChatView; five minutes later the auto-lock fired, `handleLock` → `stop_model_server()`, and the in-flight task died.

## 2. Root cause

`App.tsx` runs an idle auto-lock on window blur (`auto_lock_minutes`, default 5). Locking calls `handleLock`, which stops the model server — a destructive action for an *active agent run*, not just for the idle user. An in-flight run is not idle; the timer treated focus as the only signal of activity.

## 3. Fix

- **`App.tsx`**: ChatView reports run activity through the window CustomEvent `unoone:agent-activity` (`{active: boolean}` — dispatched by `setAgentActivity` around every harness call). The blur auto-lock's `lockIfIdle` defers while `agentActiveRef.current` is set, re-checking every 30 s until the run lands (or the window refocuses, which cancels the timers). The run therefore completes even if the window never regains focus; the lock then happens honestly, after the work is safe.
- **`ChatView.tsx`**: `setAgentActivity(true)` after `setIsGenerating(true)`, `setAgentActivity(false)` in the `finally` — every harness path (success, failure, abort) clears the flag, so a wedged invoke can hold off the lock at most as long as its own bound.
- **Manual lock is unchanged** — the Lock button, Ctrl+L, and pen-drive removal are explicit user intent and still act immediately, including mid-run.

## 4. Tests

- Frontend: oxlint + tsc + vite build clean. (The deferral is timer logic in the app shell; the activity source is the same `setAgentActivity` helper the chat already uses, so the contract is exercised by every live chat run.)
- Rust gates unaffected.

## 5. Live acceptance (post-merge, on the re-staged drive)

- [ ] A long-coding acceptance run completes while the window stays blurred past the 5-minute auto-lock — all 4 task-board files on disk, no aborted llama-server request, watcher verdict PASS
- [ ] After the run lands, the deferred auto-lock still fires (the vault locks once the agent is no longer active) — auto-lock is deferred, not disabled
- [ ] Manual Lock mid-run still stops everything immediately