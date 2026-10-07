# UnoOne Pocket AI — features and internals

Detailed feature descriptions and implementation notes moved out of the [README](../README.md) on 2026-10-07 to keep it short. Status and dated evidence: [STATUS_AND_EVIDENCE.md](STATUS_AND_EVIDENCE.md).

## Android (UnoOne Mobile)

### Android assistant

- Native Kotlin and Jetpack Compose app for Android 9 and later (API 28+), organized into 16 Gradle modules.
- One local Gemma 4 E2B planning engine through LiteRT-LM, hash-pinned in the model manifest. Deterministic rules handle common commands before model inference. An E4B "Medium" profile is defined, hash-pinned and unit-tested, but the production load path currently loads E2B only — tier selection is not yet wired into the app.
- A canonical 42-tool registry (29 legacy + 13 atomic accessibility, messaging, and calendar tools; 5 further tools are blocked) rejects unknown tools, validates required arguments, and checks argument types. Risk classes are byte-synced in CI with `packages/tool-contracts/tools.v1.json`.
- Dynamic tool exposure: the orchestrator offers the model 2–3 candidate tools per Lite (E2B) task (3–6 are defined for E4B), preventing hallucinated tool calls and reducing context-window waste. The model never sees all 42 tools at once.
- DeterministicIntentRouter handles wake commands, language switches, blind-mode toggles, simple app launches, accessibility shortcuts, and fast replies without model involvement.
- LanguageNormalizer detects 7 languages (en, hi, bn, ta, te, kn, ml) from speech patterns, normalizes filler words, and enforces the hard output-language rule (speak Hindi → reply in Hindi). Low-confidence transcripts (< 0.5) trigger clarification instead of execution.
- ToolProposalValidator validates model proposals against the candidate set, checks required arguments and types, and always allows `speak_response` as a fallback escape hatch.
- ActionResult with verified evidence: the orchestrator independently verifies action outcomes (foreground-package checks for app launches, deterministic-action confirmation for accessibility). The model may announce success only when `verified == true`.
- Direct commands, compound tasks, model-planned actions, and Skills use the same permission, risk, confirmation, execution, verification, and audit pipeline.
- Offline Sherpa-ONNX speech recognition and speech output, with explicit model-health checks. English uses the streaming transducer; Hindi uses the Omnilingual recognizer and its own offline voice.
- Selectable English and Hindi speech profiles. The selection controls STT routing, deterministic tool-status replies, wake acknowledgement, and TTS.
- One-tap hands-free sessions that listen, run the command, speak the result, and re-arm. The foreground session and background wake service coordinate ownership of the microphone.
- Background activation uses a low-latency offline keyword spotter plus an independent bounded offline-STT fallback for one-breath English and Hindi commands. The short **"Uno"** keyword and longer activation variants are supported, and a monotonic cooldown prevents the two detectors from firing twice for one speech burst. Wake acknowledgement finishes before command capture begins, and foreground recording and TTS exclusively own the microphone to prevent self-transcription.
- Phone actions for opening apps, Calendar, Chrome, WhatsApp, the dialer, URLs, and system screens.
- Calendar events, WhatsApp messages, and emails are prepared as reviewable drafts. UnoOne does not press the external app's final Send or Save control.
- Local notes, memory, Skills, activity logs, browser audit records, and preferences; with a Pocket AI attached and unlocked, the drive vault is the canonical store for notes, memories and conversation turns.
- A floating assistant and background voice service.
- A collapsible **Agent activity** panel that shows what UnoOne understood, which checks ran, what is executing, and whether it succeeded.
- A persistent **Disable UnoOne** master control on the Agent and Settings screens. Disabled mode stops and blocks microphone capture, STT, TTS, inference, Blind Aid, screen reading, accessibility actions, browser work, floating services, pending recovery, and network-backed page activity until the user explicitly enables the app.

### Android Pocket AI attachment

- The existing UnoOne app handles the physical prototype's USB attach/detach
  intents; there is no companion app.
- The prototype SanDisk VID/PID is only used to offer the app. Android then
  requires Storage Access Framework access and validates schema v2, `VERSION`,
  and `vault.id` before showing Pocket AI as connected.

### Local model contract

UnoOne Mobile has two planning-brain profiles; the app currently loads Lite.

| Field | Lite (E2B) | Medium (E4B) |
|---|---|---|
| Model id | `gemma-4-e2b` | `gemma-4-e4b` |
| File | `gemma-4-E2B-it.litertlm` | `gemma-4-E4B-it.litertlm` |
| Runtime | LiteRT-LM | LiteRT-LM |
| Exact size | `2,588,147,712` bytes | `3,659,530,240` bytes |
| SHA-256 | `181938105e0eefd105961417e8da75903eacda102c4fce9ce90f50b97139a63c` | `0b2a8980ce155fd97673d8e820b4d29d9c7d99b8fa6806f425d969b145bd52e0` |
| Maximum context | 32,768 tokens | 32,768 tokens |
| Configured context | 2,048 tokens (Lite) | 4,096 tokens (Medium) |
| Minimum RAM gate | 6,144 MB | 8,192 MB |
| Recommended RAM | 8,192 MB | 10,240 MB |
| Candidate tools per task | 2–3 | 3–6 |
| Max agent steps | 2 | 4 |
| Max browser steps | 0 | 8 |
| Action temperature | 0.1 | 0.1 |
| Chat temperature | 0.3 | 0.7 |
| Tested backend on Xiaomi 14 | CPU fallback | Not loaded by the app yet; not tested on device |

The model is selected **before** a task starts and **never switches mid-task**.
`ModelTierSelector` is designed to use command complexity and device RAM
(simple deterministic commands → Lite; compound messaging/calendar/notes/web
commands → Medium when E4B is installed and RAM allows), but it is not yet
called by the production load path.


## Desktop (UnoOne Power)

### Desktop app

- Tauri 2 + React 19 desktop shell with removable-drive discovery, strict
  schema-v2 validation, hardware profiling, and vault-core integration.
- `Start UnoOne.exe` is the on-drive fallback launcher. It validates Pocket AI,
  opens UnoOne Power, and can install UnoOne Dock after explicit confirmation.
- UnoOne Dock runs per-user without administrator rights, watches Windows device
  insertion/removal events, validates the pen drive, and launches only its
  manifest-declared UnoOne Power executable.
- Power runs from a SHA-256-addressed host copy
  (`%LOCALAPPDATA%\UnoOne\PowerCache\<digest>`), so Windows never keeps
  executable pages mapped from the removable drive.
- Removal stops model inference, discards active recording buffers, and
  emergency-locks the vault.
- **BootGate (2026-10-02):** a fast identity + runtimes gate (~8–14 s)
  releases model boot from the digest-verified host cache while the full
  package sweep keeps running in the background, so the model is usable about
  15 s after the drive is detected instead of after the full sweep;
  drive-path models still wait for it. The background sweep also stages the
  ASR/TTS models to the host cache. A 13-minute cold boot measured on
  2026-10-02 was traced to Windows Defender's first-access scanning of
  same-day-staged 11 GB, not code; warm boot went 77.4 s → 40–56 s with the
  model server live at ~15 s.
- **Explicit model controls supersede automatic boot (2026-10-03):** manual
  Stop/Load in Model Manager takes precedence over BootGate auto-boot; a
  manual load hash-verifies the selected cache artifact and its vision
  projector (mmproj) before boot, then checks the running model's
  health/identity. Admission (`desktop_model_policy`) budgets against the
  system's **total RAM**: momentary free-RAM pressure never vetoes boot, but
  weights ×1.1 + projector + KV + 1 GiB must fit, unmeasured GPU memory is
  never credited, a missing tier is never silently downloaded, and invalid
  measurements fail closed. Stopping the model cancels pending loads.
- Real Tauri API calls (no mock data), real SHA-256 verification, honest error states.
- Windows bundle CI builds `UnoOnePower.exe`, `UnoOneDock.exe`, and
  `Start UnoOne.exe` together and publishes their SHA-256 sums as one artifact.

### Desktop agent lane (live-verified on the staged drive 2026-09-14/15)

- Chat goes through the **harness bridge** (`harness_bridge.rs` → vendored
  `inbharat-harness`): a routing planner (L0 deterministic fast path /
  L1 model / L3 full agent loop), a model-visible capability contract, audited
  tool calls, and per-step transcripts in the UI. The full-access L3 loop has
  a 10,000-step safety budget; the old 900 s / 48-step wall was removed after
  it killed live runs.
- **Chat context accuracy (2026-10-07):** restored vault history stays visible
  on screen, but the model receives only context selected for the current
  task. A standalone greeting sends no history and skips long-term memory
  search; an explicit named continuation selects only the matching task; an
  unrelated new request starts a new task boundary. History is sent as whole
  user/assistant pairs within a byte budget; a request larger than the
  harness's 65,536-byte memory-query limit disables long-term memory search
  and is still sent in full.
- **Progress-aware local inference (updated 2026-10-05):** plain and
  tool-bearing main-agent turns stream over SSE from the local llama-server
  (`chat-token` Tauri events). Tool-call fragments are assembled before the
  completed call reaches the agent loop, and child agents use the same SSE
  transport without duplicating their tokens in the parent UI. Each received
  chunk refreshes the 180 s idle deadline, while a separate 12-minute hard
  ceiling still stops a genuinely stuck generation.
- **Reasoning-aware inference layer (2026-09-16):** the vision lanes
  (describe/OCR) pin think-off (`enable_thinking: false`,
  `reasoning_budget: 0`), measured live on the staged drive at scene describe
  **42–60 s → 8.2 s** and OCR **12.7 s → 3.7 s** with quality anchors held.
  Every request sends `cache_prompt: true`.
- **Model-backed memory rerank (2026-09-16):** vault memory search hits
  (≥3 lexical hits) are reordered by one bounded think-off completion on the
  same verified local model (top 8 candidates, 30 s hard deadline, fail-open
  to the lexical order — retrieval can never regress).
- **Persistent conversation memory (2026-10-02):** every completed chat turn
  is written to the unlocked vault as an encrypted `MESSAGE` record
  (AES-256-GCM, like every new vault record), and a new session restores the
  most recent 30 turns for display with an honest banner. The drive vault is
  the single memory plane. Stopped runs are never saved, tombstones hold
  across sessions, and a locked vault degrades to session-only chat (never
  plaintext). Long replies are spoken in sentence-bounded ~280-char chunks so
  CPU-bound TTS stays inside the 180 s process deadline.
- **Tool surface (FullAccess permission, default ON):** `fs.read/write/list`,
  `fs.mkdir` and `fs.copy` (byte-exact for binary files) through the
  multi-root granted-folder fence (workspace root plus user-approved folders;
  each grant validated — absolute, existing, never a drive root, never inside
  the encrypted package — audited as a vault `AuditRecord` and revocable; a
  tool touching an ungranted folder opens an in-chat approval card);
  `doc.create` writing real PDF/DOCX/MD/TXT through the same fence;
  `process.run` with allowlisted direct-argv execution and a 180 s deadline
  (background deploys supported); `workspace.search` and `workspace.patch`;
  `browser.act` typed browser actions over the audited WebView bridge
  (including an idempotent same-window popup shim, live-verified on Gmail's
  Sign-in button 2026-10-03); `web.preview` (a live preview window of a site
  the agent is building — no web server); and `agent.spawn` sub-agents.
- **Session host-command consent + owned processes (2026-10-03):**
  `process.run` runs only while the user has enabled host commands for that
  session (audited, revocable). Every process the agent spawns is owned: lock,
  permission revocation, drive removal, or app exit terminates them all
  (Windows Job Objects; 32-process session cap), and process leases are
  generation-invalidated. Folder grants authorize file tools only, never
  commands.
- **Prose-dump corrective retry:** when a 12B-class model answers a task with
  a pasted code block instead of tool calls, the runtime issues exactly one
  bounded corrective nudge; a model that already acted is never corrected.
- **Vision attachments:** chat can attach images through the audited
  attachment pipeline (mmproj vision).
- **Browser lane:** the agent opens and drives a real WebView window, with
  session-aware status and truthful no-browser refusals. Browser windows share
  the host's WebView2 profile, so one manual login survives restarts on that
  machine (host-local, not vault-encrypted); no cookie values or credentials
  are stored, and the agent never asks for or types the user's credentials.
- **Live website preview (2026-10-02):** `web.preview` mirrors the site into a
  bounded `%TEMP%\unoone-preview` tree and reloads its own capability-less
  window when the site's files change.
- Honest-failure design throughout: sub-agent timeouts, tool failures, and
  budget exhaustion surface in the UI instead of silent fallbacks. Evidence:
  `docs/verification/2026-09-14/` and `2026-09-15/` (docs 105–116).

### Knowledge and coding workspace (landed 2026-10-07, PR #62)

New desktop capabilities in `packages/pai-harness-adapter`,
`packages/capability-contracts` and the desktop app. They are implemented and
tested (see [Latest verified results](STATUS_AND_EVIDENCE.md#latest-verified-results)) but have not
yet been staged onto the physical drive or exercised on Windows hardware.

- **Encrypted knowledge records** in the existing vault — no second store:
  Evidence, Candidate, VerifiedPattern, ApprovedProcedure and Invalidation
  records. Evidence is immutable, revisions are audited, revocation propagates
  to dependants, and no record by itself grants execution.
- **Encrypted search:** an HMAC-sealed postings index; a search decrypts only
  candidate records. "Current" recall requires an exact source, version,
  commit, file digest and platform match; stale, revoked or contradicted
  material appears only in explicit historical mode. Search is lexical, not
  semantic.
- **Verified learning (Linux only):** a candidate fix is promoted only after
  really running its tests in a Linux sandbox (dropped capabilities,
  bubblewrap namespaces, read-only filesystem except a 16 MiB `/tmp`, no
  network, prlimit, seccomp): the baseline must fail, the fix must pass
  repeatedly, and test files must be byte-identical. Receipts are signed;
  approval for reuse comes only from an explicit UI event; revocation is
  enforced at reuse. On Windows and macOS this refuses before spawning
  anything.
- **Coding Task workspace:** a durable encrypted task ledger records each step
  before its side effects, so crash recovery never replays a write. Edits
  happen in an isolated working set; build/test gates run in the sandbox with
  real exit codes (protected test files, hidden-test text and oracle tampering
  are guarded); per-file diffs carry review decisions bound to the exact
  content hash; **apply** writes only to a separate task worktree
  (`%LOCALAPPDATA%\UnoOne\coding-tasks`); a bounded repair loop stops with
  evidence; dynamic sites preview through an authenticated 127.0.0.1 bridge.
  **Opening a task needs the Linux sandbox.** On Windows and macOS the Coding
  Task screen is view-only: it can show, review and export tasks created on a
  Linux host that shares the vault, but it cannot open new tasks, run checks,
  preview or apply.
- **Local distiller + Knowledge screens:** pasted text (any platform) or files
  from granted folders (Linux only for now — fd-safe file capture is
  unavailable elsewhere) become evidence plus cited candidates (deterministic extraction:
  headings, docstrings, first lines — not semantic understanding), with
  held-out material excluded, explicit budgets, no network and no automatic
  promotion. The Knowledge view offers Explorer, Detail (reject/revoke behind
  confirmations), Distiller and consented Export of verified/approved items
  only; there is no training export. Possible contradictions are reported as
  a heuristic, never persisted.
- **Learning panel** on the Coding Task screen: relevant verified patterns for
  the exact file, save a reviewed task as a candidate, preview the
  verification recipe, verify, approve, revoke (verify and approve need the
  Linux sandbox).
- **Memory Explorer search** works again (debounced, stale responses
  discarded).

### Desktop Rust Backend (`apps/desktop/src-tauri/src/`)

| Module | Purpose |
|--------|---------|
| `main.rs` / `startup.rs` | Pocket AI detection, strict validation, startup state machine, removal cleanup, hardware profiling, vault-core integration, command registration |
| `boot_trace.rs` | Boot waterfall trace (`%TEMP%\unoone-logs\boot-trace.log`) |
| `llama.rs` / `gguf_meta.rs` | Manifest-only model/runtime discovery, CUDA/Vulkan/CPU selection, verified server identity, mmproj vision; GGUF metadata and host-adaptive context budgets |
| `desktop_model_policy.rs` | Model admission against total RAM (12B/E4B/E2B tiers) |
| `speech.rs` / `voice.rs` / `bharat_audio.rs` | SpeechRouter (`InbharatAudioThenLegacy`), voice capability probes, the InBharat Audio adapter with hash-bound acceptance |
| `capability.rs` | Host hardware/capability profile shown in the UI |
| `harness_bridge.rs` (+ `chat_context.rs`) | The production chat path through the vendored harness; std-only chat-context assembly (greeting guard, whole history pairs, byte budgets) |
| `agent.rs` | Full-access agent lane: tools, budgets, audit, `agent.spawn` sub-agents |
| `granted_fs.rs` | Multi-root granted-folder fence for file tools |
| `desktop_process.rs` | Session host-command consent and owned-process lifecycle |
| `doc_writer.rs` | Pure-Rust PDF/DOCX/MD/TXT writers for `doc.create` |
| `preview.rs` | `web.preview` live website preview |
| `chat_memory.rs` | Persistent conversation memory (vault `MESSAGE` records) |
| `env_learning.rs` | Bounded environment-learning records from completed agent runs |
| `tool_contract_pin.rs` | Pins every registered desktop tool to `packages/tool-contracts/tools.v1.json` |
| `coding_task_commands.rs` | Coding Task Tauri commands (main-window-only for state changes) |
| `knowledge_commands.rs` | Knowledge and task-learning Tauri commands |
| `safety.rs` | SafetyGuard (STANDARD/RELAXED/OFF), blocked actions, harm detection |
| `recording.rs` | Recording: cpal capture, WAV encoding, vault encryption, 4 privacy levels |
| `browser.rs` | Browser workspace: typed WebView bridge actions |
| `documents.rs` / `document_migration.rs` | Document parsing and TF-IDF search; plaintext→vault migration and read path |
| `accessibility.rs` | Blind View, OCR and image description via Gemma mmproj, screen capture |
| `security.rs` | Manifest-integrity validation, SHA-256, encryption wiring, crash recovery, emergency lock |

### Desktop React Frontend (`apps/desktop/src/src/`)

| Component | Purpose | Status |
|-----------|---------|--------|
| `UnlockScreen` | Password-only vault unlock, USB detection, new vault setup | IMPLEMENTED (live-unlocked on the staged drive) |
| `Sidebar` | Navigation: Chat, Recordings, Memory, Vault, Model, Browser, Documents, Accessibility, Capabilities, Hardware, Settings, Coding Task, Knowledge | IMPLEMENTED |
| `ChatView` | Conversation via the harness bridge (agent lane, attachments, 16-language voice picker); restored history shown with per-task model context | IMPLEMENTED (live-verified before the 2026-10-07 context changes; mounted tests pass) |
| `CodingTaskView` | Coding tasks: plan, gates, per-file review, apply, preview, Learning panel | IMPLEMENTED (mounted tests; not yet run in the real app) |
| `KnowledgeView` | Knowledge Explorer, Detail, Distiller, Export | IMPLEMENTED (mounted tests; not yet run in the real app) |
| `RecordingView` | Recording with type/privacy, pause/resume/bookmarks, vault encryption | IMPLEMENTED (needs UI testing) |
| `MemoryExplorer` | Memory types and search | IMPLEMENTED (search fixed 2026-10-07; mounted tests) |
| `VaultView` | Vault status, emergency lock | BUILDS, NOT RUNTIME-TESTED |
| `ModelManager` | Model server start/stop, cache status, context profile | IMPLEMENTED (live-driven) |
| `BrowserWorkspace` | URL bar, WebView viewport, DOM bridge actions | IMPLEMENTED (live-verified) |
| `DocumentsView` | Document import and search | IMPLEMENTED (needs UI testing) |
| `AccessibilityView` | Blind View, OCR, camera blind-aid describe + narration, high contrast | IMPLEMENTED (OCR live-verified; the describe-lane defect #44 fix re-verified on the re-staged drive) |
| `CapabilityProfile` / `HardwareProfile` | Host capability/hardware report | IMPLEMENTED |
| `SettingsView` | FullAccess agent toggle, host commands, folder grants, accessibility | IMPLEMENTED |

## Desktop model verification

| Property | Value |
|----------|-------|
| Model file | `gemma-4-12B-it-Q4_K_M.gguf` |
| Size | 7,662,531,872 bytes (7.14 GiB) |
| SHA-256 | `D333B368BE6CD655563FCE18AEDE26027E208FDB13816D35EB06983CE054044B` |
| GGUF version | 3 |
| Architecture | `gemma4` |
| Quantisation | Q4_K_M |
| Source | Google Gemma 4 12B IT, GGUF Q4_K_M by llama.cpp community |
| Licence | [Gemma Terms of Use](https://ai.google.dev/gemma/terms) |
| Inference verified | Live on the staged drive (2026-09-15): chat, one-call speech-to-speech loop, OCR, vision describe, multi-agent tool runs. Re-verified from the recovered desktop package (2026-10-05): manifest-valid launch, GPU model load, SSE tool call, clean local health check |
| Native context (read from the artifact) | **131,072 tokens** — `gemma4.context_length`, parsed by `gguf_meta.rs` |
| Session context | **Host-adaptive** — `start_server` clamps the requested context to the artifact's trained context and the host RAM tier (≥24 GiB → 32,768; ≥12 GiB → 16,384; else 4,096), logging every clamp reason and showing it in the Model panel. Unreadable metadata is surfaced as "unverified", never guessed |
| KV-cache cost | Derived from the artifact's shape (48 layers, 16 KV heads, 512 head_dim → ~0.8 MiB/token at q8_0): 32,768 tokens ≈ 25.5 GiB, displayed in the UI |
| Long conversations | **Trimmed to the granted window, visibly** — oldest turns are dropped first (never the system prompt, tools, or the current user turn) and the reply carries a context note |
| Source = Destination SHA-256 | ✅ Exact match |

### Desktop model ladder (12B / E4B / E2B)

The package manifest may declare three desktop GGUF tiers — the flagship 12B
(`gemma-4-12B-it-Q4_K_M.gguf`, 7.14 GiB), Medium E4B (`4,977,171,584` B), and
Lite E2B (`3,106,738,272` B), each with its own same-tier mmproj projector.
Model Manager lists whatever tiers the manifest declares and the host admits:
a tier boots only if the host's **total RAM** clears both the tier floor
(**E2B 4 GiB, E4B 8 GiB, 12B 16 GiB**) and the footprint budget
(`apps/desktop/src-tauri/src/desktop_model_policy.rs`).

### Desktop dependencies and build boundary

The portable app uses the system WebView and Rust libraries for its
application logic. Model inference remains a manifest-verified llama.cpp
runtime shipped on Pocket AI. This repository does not claim that arbitrary
local Application Control policies will allow unsigned build scripts or
binaries: Windows bundle CI is the reproducible build path, and release
signing plus a real prepared-host insertion test remain production gates.

All three shipped executables embed the UnoOne brand icon (multi-resolution
`apps/desktop/src-tauri/icons/icon.ico`; Dock and Starter via `winresource`).
`tauri_build` re-embeds the icon only when `tauri.conf.json` changes — after
replacing icon files in an existing target dir, touch the config or build
clean.

### Vendored universal planes

This repository is **self-contained**: the two InBharat universal planes it
depends on are vendored under `vendor/` rather than referenced as sibling
repositories. Pocket AI is a *product* built on top of them; product-specific
code lives in `packages/pai-harness-adapter` and the app crates, while the
universal code stays in the vendored planes and is reused across InBharat
products.

- **`vendor/inbharat-harness/`** — the universal Rust control/text plane. Its
  own nested Cargo workspace provides `inbharat-harness-core` (routing,
  session, the provider model `ModelProvider` / `MemoryProvider` /
  `ToolProvider` / `PermissionProvider` / `SafetyProvider` /
  `ConfirmationProvider` / `VerificationProvider` / `SandboxProvider`, and
  deterministic L1 tool execution). The desktop backend and
  `pai-harness-adapter` depend on it via a path dependency
  (`vendor/inbharat-harness/crates/core`). The outer workspace `exclude`s this
  directory so the vendored crate resolves `*.workspace = true` inheritance
  against its own inner workspace root. The Harness core imports **no**
  Tauri / UNOONE / Pocket AI / vault code — only the adapter adds product
  concerns.
- **`vendor/Inbharat-audiocpp/`** — the universal C++ speech plane (CMake
  build): streaming/edge STT, TTS, and keyword spotting. It ships as a
  standalone runtime binary on Pocket AI and imports **no** UNOONE UI or
  product logic.

To rebuild the universal planes from their canonical sources, replace
`vendor/inbharat-harness` and `vendor/Inbharat-audiocpp` with the current trees
from the InBharat Harness and InBharat Audio repositories; the path
dependencies resolve unchanged.
