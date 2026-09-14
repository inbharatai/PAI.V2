# 103 — Defect #27: Switching Tabs Destroys the Chat Conversation (and Silently Discards an In-Flight Agent Run)

**Date live-caught:** 2026-09-13 (physical drive D:\UNOONE, during the defect-#26 acceptance re-run)
**Severity:** High — silent data loss of the user's entire conversation, and any agent task still running when the user switches tabs completes on the backend with its result thrown away.

## 1. Live catch

During the long-coding acceptance run, the capability panel was checked (a normal user action — look at another tab while the agent works). On returning to the Chat tab, the conversation was **gone**: the task prompt, the step pills, everything. `ChatView` holds its messages in `useState([])` (`ChatView.tsx:45`) and `App.tsx` renders it conditionally per tab — navigating away unmounts the component and React discards its state.

Worse: `handleSend`'s `await tauriApi.harnessChat(...)` keeps running in the backend after unmount. When it resolves, `setMessages(prev => [...prev, assistantMessage])` targets an unmounted component — a no-op. The run's answer (and its artifacts report) is **silently discarded**. In the live run this compounded the diagnosis: the L3 run completed a 2746-token answer (llama-server log, task 7281, 5.6 min) whose text was never shown to anyone, and no files existed on disk to check (that failure is defect #28, doc 104 — this defect is why it was invisible).

## 2. Root cause

| Layer | Finding |
|---|---|
| `App.tsx renderView()` | `case 'chat': return <ChatView />` — ChatView exists only while the chat tab is active |
| `ChatView.tsx` | `const [messages, setMessages] = useState<ChatMessage[]>([])` — the conversation lives only in component state; no persistence, no lift |
| `ChatView.tsx handleSend` | the `harnessChat` promise resolves after unmount; its result lands in a no-op `setMessages` |

## 3. Fix

`App.tsx`: ChatView is now mounted once for the session and hidden with `display: none` while another view is active (`renderView()` returns `null` for the chat case). The conversation, pending attachments, and any in-flight run's result all survive tab switches; the chat panel behaves like every chat app a user has used. Other views remain conditional — only the conversation-bearing view needs session lifetime.

## 4. Verification

- [x] `cargo`-side unaffected; frontend `tsc -b && vite build` clean
- [ ] **Live re-verify after re-stage:** send a task, switch to Capabilities and back mid-run — conversation intact, run continues, answer appears
      — VERIFIED 2026-09-14 live on the re-staged drive (build `3a73998`, `tabswitch-verify.js`):
      a question was sent and the app was immediately navigated to Capabilities, then Recordings,
      while the reply was still generating. On both other tabs the chat DOM stayed alive
      (`visible=false` — mounted and hidden, exactly the fix's contract), the in-flight reply
      LANDED while the app was on another tab (bubbles 2 → 4, user question + assistant answer
      "12 is bigger"), and returning to Chat showed the full conversation intact.
- [ ] Long-coding acceptance re-run passes with the watcher attached while the chat stays open
      — *(result recorded in doc 101 §4 / doc 104 §4 after the run)*