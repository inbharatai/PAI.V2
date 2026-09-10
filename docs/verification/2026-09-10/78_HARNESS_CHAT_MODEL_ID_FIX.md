# 78 — harness_chat Was Dead on Windows: Model-Id Path Backslash Fix

**Date:** 2026-09-10
**Scope:** `apps/desktop` (harness bridge, legacy agent fallback, ChatView)
**Posture:** live-caught defect during user-perspective pendrive acceptance.

## 1. Symptom (live, through the real app UI)

Asked through the production Chat UI (full access enabled, header showing
"Full access — read/write files, run commands, drive the browser"):

> "…my capabilities are strictly confined to the encrypted USB vault where I
> reside… I cannot see, read, or write any files on your computer's hard
> drive… I am an 'in-vault' assistant."

The model answered with the exact script of the **legacy fallback agent's**
system prompt — and the WebView console showed why:

```
Harness bridge fell back to legacy agent:
conflict:model.register: provider id or advertised model catalogue is invalid or duplicated
```

**harness_chat has never succeeded live on Windows.** Every chat silently
fell back to the read-only legacy ReAct agent, whose prompt told the model it
was vault-confined — the direct origin of the false self-knowledge reported
by the user. The full-access lane, the system-prefix briefing (doc 77-era
fix), and the vision attachments lane were all effectively unreachable in the
shipped app, because they all ride `harness_chat`.

## 2. Root cause

llama-server advertises `/v1/models` ids derived from the model's launch
path. On Windows that is the full path with backslashes — live-observed:

- drive launch: `\\?\D:\UNOONE\MODELS\DESKTOP\Gemma-12B\gemma-4-12B-it-Q4_K_M.gguf`
- cache launch: `C:\Users\reetu\AppData\Local\UnoOne\model-cache\D333B368….gguf`

The vendored harness `ModelRegistry::register` validates model ids against
`[A-Za-z0-9._-/:]` — **no backslash** — so registering the provider failed on
every single call, on every Windows launch, drive or cache.

The same briefing sent *manually* as a system message to the identical live
server produced a perfectly truthful, tool-specific answer — proving the
model obeys the briefing and the defect was purely the dropped request path.

## 3. Fix

1. **`harness_bridge.rs`** — `registry_safe_model_id()` reduces the
   server-reported id to its basename (cache: the manifest-sha256 filename;
   drive: the model filename) before registering the provider and building
   the request, keeping both sides of the provider's strict
   `request.model == self.model_id` check consistent. Unit test
   `registry_safe_model_id_strips_windows_paths` covers the live-observed
   cache path, the `\\?\` drive path, POSIX paths, and plain ids.
2. **`ChatView.tsx`** — the legacy rollback is no longer silent. A fallback
   now attaches a visible step: the reason plus an honest note that the
   fallback can only read vault records, so its answers may understate the
   session's abilities.
3. **`agent.rs`** — the legacy system prompt no longer tells the model it
   "run[s] entirely on the user's encrypted USB vault" (a false identity the
   model internalized as confinement). It now describes the assistant as
   running locally on the user's computer, lists the tools this lane really
   has, and forbids claiming both broader access and false confinement.

## 4. Why this was caught only now

CI exercises the harness pipeline with synthetic model ids (never a live
llama-server `/v1/models` response), and the UI fallback deliberately treated
bridge failures as a rollback path — correct for a build that lacks the
command, wrong for a bridge that breaks at runtime. The live user-path test
was the only thing that could catch it.

## 5. Post-fix acceptance (must be re-run live)

- `harness_chat` completes through the real app UI with the model loaded from
  the host cache and from the drive.
- The same capability question answers truthfully (workspace path, program
  allowlist, browser, vault tools — matching the registered toolset).
- No fallback step pill appears.
- Vision attachments (which ride the same bridge) deliver a correct
  image-grounded answer through the app.