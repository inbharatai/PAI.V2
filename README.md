# UnoOne Pocket AI (PAI)

**Private AI on a pen drive — two host platforms, one encrypted vault, zero cloud.**

> **Patent pending** — Indian provisional application **202631102427** (ref E106/3399/2026-KOL), filed 2026-08-25. See [PATENT.md](PATENT.md).

Pocket AI is the physical UnoOne pen drive. Its models, runtimes, applications,
identity, and encrypted vault live on that removable device. Windows and
Android are hosts for the same Pocket AI; UnoOne Dock is only a per-user
Windows bridge that detects and opens it. The host disk is never the canonical
copy.

| | UnoOne Mobile | UnoOne Power |
|---|---|---|
| **Platform** | Android 9+ | Windows desktop (the Rust workspace also builds and unit-tests on Linux and macOS in CI; there is no macOS app yet) |
| **Model** | Gemma 4 E2B (LiteRT-LM); an E4B profile is defined but not yet loaded by the app | Gemma 4 12B/E4B/E2B Q4 GGUF, RAM-tiered (llama.cpp) |
| **UI** | Jetpack Compose | Tauri 2 + React 19 |
| **Storage** | SQLCipher Room cache → USB vault | RAM → USB vault |
| **Voice** | Sherpa-ONNX STT/TTS | InBharat Audio (Qwen3-ASR / omnivoice, acceptance-gated) with legacy Whisper/Piper as the explicit policy fallback |
| **Eyes-free** | TalkBack, Blind Aid, Camera OCR | Screen reader, high-contrast, OCR, camera blind-aid describe + narration |

The release identity is not a drive letter, volume label, or USB VID/PID.
Every host must validate `manifest.json`, `VERSION`, `VAULT/identity/vault.id`,
the declared architecture, and every required asset hash before use.

```
UnoOne Mobile (Android)          UnoOne Power (Desktop)
     Gemma 4 E2B                  Gemma 4 12B / E4B / E2B GGUF
          ↕                                ↕
          └──── Shared encrypted USB vault ────┘
                 (Argon2id + AES-256-GCM default; XChaCha20-Poly1305 legacy readable)
```

## What works today

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
tested (see [Latest verified results](#latest-verified-results)) but have not
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

### Android Pocket AI attachment

- The existing UnoOne app handles the physical prototype's USB attach/detach
  intents; there is no companion app.
- The prototype SanDisk VID/PID is only used to offer the app. Android then
  requires Storage Access Framework access and validates schema v2, `VERSION`,
  and `vault.id` before showing Pocket AI as connected.

## Current Status

| Component | Status |
|-----------|--------|
| Physical Pocket AI | **INTEGRITY-VERIFIED PROTOTYPE, RELOADED 2026-10-04** from `main` `e3b64f4` after the original stick reported Windows media errors (Event 51/153) that formatting did not cure. Reloaded complete from git and hash-verified keepers: `SOURCE` (byte-exact `git archive 52a2ec0`), three desktop GGUF tiers with per-tier mmproj, both mobile `.litertlm` models, the Android APK, runtimes, the speech plane, and a **fresh vault** (past chat/memory deliberately not carried — owner decision). The manifest was regenerated off-drive against byte-identical keepers; `Start UnoOne.exe --verify-only` on the reloaded drive: `valid=true`, 0 failures; owner-verified unlock and a working session. Known defect: the app can crash (stack overflow on a spawned thread) when a mid-sweep device-removal IO error fires — hardware-conditional, not reproducible on a healthy filesystem. **The drive does not yet carry the 2026-10-07 build** — restage with [`scripts/Stage-PocketAiDrive.ps1`](#update-a-pocket-ai-drive). Earlier live acceptance on the pre-reload drive (three cycles, 2026-09/10) covered unlock, BootGate model load, chat-memory recall, audited folder grants and the in-chat grant card, `doc.create`/`fs.write`/`fs.copy`/`web.preview`, browser-session persistence across restarts, and the Gmail popup shim; dated evidence: `docs/verification/2026-09-14/` + `2026-09-15/` |
| Desktop frontend embedding | **VERIFIED** — the historic "localhost refused to connect" drive is fixed (`tauri/custom-protocol` default feature; without it `generate_context!` embeds zero assets). A byte-level gate runs in the Windows bundle CI and passed on the staged drive binary |
| Mobile app (Android) | V2 agent pipeline + Pocket AI USB auto-open. Compiles, lints, tests, and `assembleDebug` passes in Android CI; **cross-platform vault contract proven bidirectionally in CI** (Kotlin↔Rust Argon2id + AES-GCM record layer and XChaCha20 master-key wrap — `packages/vault-core/test-vectors/`, `VaultCryptoCrossPlatformTest`). Physical phone round trip with the drive pending |
| Desktop frontend (React) | BUILDS — Vite build and oxlint in CI; real Tauri API calls, no mock data. Six local Node test suites (context selector, TS/Rust greeting parity, real-core boundary, mounted Chat, Coding Task and Knowledge views) pass locally but are **not yet wired into CI** |
| Desktop backend (Rust) | BUILDS AND TESTS — `cargo fmt --check`, `cargo check`, `cargo test --workspace` and `cargo clippy --workspace -D warnings` green on **windows-latest, ubuntu-latest and macos-latest** (Desktop CI on `05baa79`, 2026-10-07). Suites cover vault-core (Wave-1 regressions + cross-platform vectors), recording policy, text-util, usb-manifest, document migration, browser policy and workspace, chat memory, the granted-folder fence and in-chat grant approval, the corrective retry, document writers, live preview, the desktop glue, and the product adapter (180 library tests including the 2026-10-07 work). The adapter's 50 sandbox tests need Linux isolation; CI runners without bubblewrap skip them with a visible `::warning::` annotation, and they run for real on Linux hosts with bubblewrap |
| Desktop USB/runtime resilience | **VERIFIED 2026-10-05** — Power is copied into a SHA-256-addressed `%LOCALAPPDATA%\UnoOne\PowerCache\<digest>` directory and launched from that verified host copy. Automatic repair retries are bounded at 45/90/180/360/720/900 seconds, recovery checkpoints use `.partial`, and manifest generation excludes incomplete `.partial` artifacts. This protects the app and copied package; it does not claim to repair failing USB hardware |
| Windows Dock / Starter | **INTEGRITY-VERIFIED ON DRIVE** — manifest-declared, hash-verified, native `--verify-only` exits 0; transactional staging with automatic rollback proven live |
| Vault encryption (`packages/vault-core`) | IMPLEMENTED AND CORRECTNESS-HARDENED — Argon2id (256 MiB / t=3 / p=4) + AES-256-GCM for new records (legacy XChaCha20-Poly1305 stays readable, identified by nonce length) + HKDF-SHA-256 + BIP-39 recovery + write-ahead journal; transactional first-use setup refuses re-initialisation and preserves packaged `vault.id` bytes. Per-vault random salts on both the password and recovery paths. The KDF parameters are pinned as a cross-platform contract with the Kotlin `encrypted-vault` package. **Wave 1** fixed four release blockers: newest-committed-generation header slot selection, authenticated record metadata re-verified on every read, real journal transactions with fsync-and-verify before promotion, and canonical UUID v4 record IDs |
| Model inference | Bundled llama.cpp only; direct runtime test verified (real answer, 127.0.0.1-only, clean stop) — see `docs/verification/2026-07-30/59_DIRECT_GEMMA.md` |
| Offline voice | VERIFIED pipeline — bundled Piper synth → bundled Whisper transcribe round trip is verbatim; see `docs/verification/2026-07-30/62_OFFLINE_VOICE.md`. The Windows bundle CI also runs real Whisper/Piper inference on pinned assets |
| InBharat Audio speech plane | ACCEPTANCE-GATED + DEPLOYED 2026-08-26 — `vendor/Inbharat-audiocpp/` (audio.cpp @ `26dcb5c4`); Qwen3-ASR-0.6B Q8_0 + omnivoice Q8_0 GGUF at `SPEECH/models/`; 30/30 hash-bound acceptance gate green. The production voice path goes through a `SpeechRouter` (`speech.rs`) with the explicit `InbharatAudioThenLegacy` policy; both routes hash-verify their CLIs/models and report buffered-final semantics. Language tags are canonicalized through `packages/speech-contracts` and byte-sync-checked against the Android mirror in CI; Assamese routes only to IndicConformer and fails closed when absent. **Live speech matrix on the staged drive 2026-09-15: 19/19 pass** (`docs/verification/2026-09-15/116_SPEECH_LANGUAGE_MATRIX.md`) |
| Recording | IMPLEMENTED WITH ENFORCED PRIVACY — `unoone-recording-policy` makes retention decisions exhaustive; TRANSCRIPT_ONLY/SUMMARY_ONLY retain no audio; temp WAV deleted and verify-checked; zero samples reports an error. **SUMMARY_ONLY is disabled in the UI** until a summariser exists |
| Browser workspace | IMPLEMENTED AS TYPED, VERIFIED ACTIONS — no arbitrary script execution; scheme allowlist; JSON-literal escaping; submit/upload/download require explicit confirmation; real PNG screenshots with SHA-256; the popup shim is live-verified on Gmail. Live-page acceptance journeys are human-gated |
| Text handling (Indic scripts) | HARDENED — `packages/text-util` provides grapheme-cluster-safe truncation (eight byte-offset slicing sites that panicked on Devanagari/Bengali text were fixed) |
| Plaintext elimination (Wave 3) | SHIPPED — migration core (PR #11) and read path (PR #15). The 2026-10-04 reload started from a fresh vault; `migrate_plaintext_documents_to_vault` applies to older drives that still hold legacy plaintext documents (run in a human session, backup first) |
| Document parsing | IMPLEMENTED — PDF (lopdf), DOCX/XLSX/PPTX (zip+quick-xml), TXT/MD/CSV/HTML; TF-IDF keyword search (explicitly not "semantic") |
| Browser redirect policy | IMPLEMENTED — `unoone-browser-policy` verdicts (same-origin/registrable → reached, HTTPS→HTTP → failure, cross-origin → surfaced `verified=false`, never a silent success); live WebView acceptance human-gated |
| Android vault repository | SHIPPED — unlock/read/write/tombstone against the Rust vault, proven bidirectionally by cross-platform vectors and a Rust-generated synthetic-vault fixture; integrated into the app flows (PRs #20, #21: the drive vault is the canonical notes/memories store). Remaining: the physical phone ↔ drive ↔ desktop round trip |
| Accessibility (OCR, Blind View) | IMPLEMENTED — OCR/description via Gemma mmproj; confidence honestly `Option<f32>` (unmeasured, never fabricated) |
| Security (vault writes) | IMPLEMENTED — `vault_write_record` writes encrypted records; recording and document content encrypted end-to-end |
| Chat context accuracy | IMPLEMENTED AND TESTED (2026-10-07) — selector, greeting parity, real-core and mounted Chat tests pass locally; desktop Rust in CI. Not yet live-verified on the staged drive |
| Knowledge, verified learning, coding workspace | IMPLEMENTED AND TESTED (2026-10-07) — each part independently reviewed with all findings fixed; adapter suites pass with the real Linux sandbox; CI green on three OSes. **On Windows:** knowledge search, review and export and pasted-text distillation work; verified learning, local-file distillation and coding tasks (view-only) need a Linux host. Not yet staged on the drive or run on Windows hardware |
| macOS | Rust workspace compiles and unit-tests on `macos-latest` in CI; **no macOS app bundle; not tested on Mac hardware** |

Timestamped verification packages in the repo run through
`docs/verification/2026-09-16/` (docs 105–118). Later results — the
2026-10-02…05 desktop work and the 2026-10-07 knowledge/coding change set — are
recorded in this README, the commit messages, and their CI runs; they have no
separate evidence documents in the repo. `docs/verification/2026-07-30/`
(incl. `72_RELEASE_MATRIX.csv`) is a dated historical snapshot; older evidence
must be read with its dates.

### CI gate state

`main` is kept green across all gates; each workflow's own run is the
evidence, not this paragraph:

- **Desktop CI** — mobile protection, frontend build (Vite), speech
  language-table sync (`scripts/check_speech_language_sync.py`), tool-contract
  sync (`scripts/check_tool_contract_sync.py`), InBharat Audio Linux ctest +
  ABI invariant job, Rust `fmt`/`check`/`test`/`clippy` on **windows-latest,
  ubuntu-latest and macos-latest**, secret scan, artifact scan. Linux sandbox
  tests are skipped with a visible warning when the runner lacks bubblewrap.
- **Mobile Protection** (own workflow, every push) — the committed tree-hash
  pointer (`scripts/MOBILE_PROTECTED_TREE`) must equal
  `git rev-parse HEAD:android-app/UnoOneAgent`.
- **Android CI** — repository invariants, speech/tool contract sync, Page
  Agent typecheck/tests/bundle and Playwright e2e, lint, unit tests, debug APK
  assembly, on the `android-app/` and contract paths.
- **Distribution CI** — distribution API typecheck and policy tests,
  Cloudflare Worker bundle, installer PWA typecheck/tests/build, and catalogue
  signing round-trip plus invalid-payload and tamper-rejection proofs.
- **Pocket AI Windows Bundle** — strict manifest and recording-retention
  tests, builds Power/Dock/Starter together in release mode, gates frontend
  embedding, uploads the portable bundle with SHA-256 sums (14-day artifact),
  and smoke-tests real Whisper/Piper inference on pinned voice assets.

Not yet in CI: the desktop frontend Node test suites (`npm run test:*` in
`apps/desktop/src`).

## Architecture

### Android agent pipeline

```text
voice / text / floating assistant / accessibility input
                         │
                         ▼
               LanguageNormalizer
                         │
                         ▼
              DeterministicIntentRouter
              ┌────────────┼────────────┐
              │            │            │
        wake/language/  app launch/  accessibility
        blind mode     blind mode   shortcuts
        (no model)      (no model)   (no model)
                             │
                    NO_DETERMINISTIC_MATCH
                             │
                             ▼
                     ModelTierSelector *
                    ┌────────┴────────┐
                    │                 │
              E2B (Lite)        E4B (Medium) *
              2-3 tools          3-6 tools
              2 steps            4 steps
                    │                 │
                    └────────┬────────┘
                             │
                     CandidateToolSelector
                             │
                             ▼
                  fresh planning conversation
                             │
                             ▼
                       local Gemma
                             │
                             ▼
                    ToolProposalValidator
                             │
                             ▼
                permissions + SafetyGuard + security mode
                             │
                             ▼
       phone tools / notes / memory / Skills / Blind Aid / documents
                             │
                             ▼
                      ActionVerifier
                             │
                    ┌────────┴────────┐
                    verified success  unverified/partial
                             │                │
                    ObservationBuilder   ObservationBuilder
                             │                │
                    ┌────────┴────────────────┘
                    │
              AgentLoopController
              (ReAct: max steps per profile)
                    │
              speak_response
                    │
                    ▼
                  TTS out
```

\* `ModelTierSelector` and the E4B profile are implemented and unit-tested,
but the production load path currently loads E2B only.

More detail: [Android architecture](docs/ARCHITECTURE.md).

### Desktop (UnoOne Power)

```text
Start UnoOne.exe / UnoOne Dock ── strict manifest + hash validation
                │
                ▼
UnoOne Power (Tauri 2) ── runs from the digest-verified PowerCache copy
   ├── React UI: Chat · Coding Task · Knowledge · Memory · Vault · Model · Browser · …
   ├── harness_bridge ──► vendor/inbharat-harness (L0/L1/L3 routing, tools, audit)
   │      └── tools: granted-folder fence · host-command consent · browser · preview
   ├── llama.cpp server (127.0.0.1) ◄── manifest-verified GGUF tier + mmproj
   ├── SpeechRouter ──► InBharat Audio, then legacy Whisper/Piper
   └── pai-harness-adapter
          ├── knowledge records · sealed search · distiller · verification
          └── coding tasks: ledger · working set · diffs · worktree · preview bridge
                 └── generated code ──► Linux sandbox only (refused elsewhere)
                │
                ▼
      Encrypted vault on the drive (vault-core: Argon2id + AES-256-GCM)
```

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

### Encryption

- **KDF**: Argon2id (256 MiB memory, 3 iterations, parallelism 4) — `packages/vault-core`
- **Cipher**: AES-256-GCM for every new record on desktop and Android; legacy XChaCha20-Poly1305 records stay readable (identified by nonce length)
- **Key wrapping**: Password → Argon2id → KEK → wraps a random vault master key (password changes need no re-encryption)
- **Key isolation**: Master key → HKDF-SHA-256 → per-domain keys (records, journal, indexes, etc.)
- **Header**: Double-buffered (A/B slots), HMAC-SHA-256 authenticated, constant-time comparison
- **Recovery**: 24-word BIP-39 mnemonic with independent key wrapping
- **Journaling**: Write-ahead log (PENDING → COMMITTED / ROLLED_BACK) for exFAT crash safety
- **Deletion**: Tombstone records propagate across platforms
- **Password-only login**: No username, no email, no cloud account
- **Memory safety**: Master key zeroed on lock and drop; no passwords in files/logs
- **Vault writes**: `vault_write_record` encrypts content with AES-256-GCM and stores it in `VAULT/records/`; recording, document, chat, knowledge and coding-task records flow through vault-core

> Release rule: do not store sensitive data until the current commit passes CI
> and the physical Pocket AI passes strict on-drive verification.

### Safety

```
User input → Model → Parser → ToolAction → policy gates → Execution
```

- **Raw model output never executes tools directly.**
- Desktop SafetyGuard levels STANDARD (balanced), RELAXED (reduced), OFF
  (testing only); blocked actions: `shell_execute`, `file_delete_system`,
  `network_raw_socket`, `registry_modify`.
- Desktop file tools are bounded by the granted-folder fence and
  `process.run` by session host-command consent; agent processes are owned
  and terminated on lock, removal or exit. In the default full-access lane,
  confirmations are auto-approved, so desktop risk classes are labels rather
  than per-call dialogs.
- Generated code from coding tasks and verification runs only inside the
  Linux sandbox; Windows and macOS refuse before spawning.
- Android risk classes (DIRECT / CONFIRM / STRONG_CONFIRM / blocked) come from
  the CI-synced tool contract. Full tables: [docs/SAFETY.md](docs/SAFETY.md).

## Quick Start

### Android (Mobile)

```bash
git clone https://github.com/inbharatai/PAI.V2.git
cd PAI.V2/android-app/UnoOneAgent
./gradlew assembleDebug
adb install app/build/outputs/apk/debug/app-debug.apk
```

### Desktop (Power)

```bash
# Prerequisites: Rust (stable), Node 24 LTS; on Windows, MSVC Build Tools.

# 1. Build the frontend (generate_context! embeds it, so it must exist before any cargo command)
cd apps/desktop/src
npm ci
npm run build

# 2. Build all three portable Windows applications
cd ../../..
cargo build --release \
  -p unoone-power \
  -p unoone-dock-windows \
  -p unoone-starter-windows

# Or run Power from source with the embedded frontend
cargo run -p unoone-power
```

`apps/desktop/src/package.json` has no `tauri` script; hot-reload development
needs the Tauri CLI installed separately. On a prepared Pocket AI, Windows
users start at `Start UnoOne.exe`, which offers to install Dock for automatic
opening on later insertions.

### Update a Pocket AI drive

The drive is an output, not a git input: when desktop code lands, only the
three executables change. `scripts/Stage-PocketAiDrive.ps1` stages them from a
green **Pocket AI Windows Bundle** CI artifact: it verifies the bundle's
SHA-256 sums, backs up the current executables to
`RECOVERY\package-backups\<timestamp>`, copies the new ones, gates frontend
embedding, regenerates `manifest.json`, and runs `Start UnoOne.exe --verify-only`.
Models, runtimes, `VAULT` and `CONFIG` are untouched.

```powershell
# From a checkout at the bundle's commit, with apps/desktop/src/dist built:
powershell -ExecutionPolicy Bypass -File scripts\Stage-PocketAiDrive.ps1 `
    -VaultRoot "E:\" `
    -BundleDir "$env:USERPROFILE\Downloads\pocket-ai-windows-x86_64-<sha>"
```

Quit UnoOne first and back up `VAULT` before running a new build against it.

### USB Drive Setup

The USB drive must be formatted exFAT (FAT32 cannot hold the 7.14 GiB 12B
model). The desktop app detects it automatically through manifest validation.
Layout (generated and checked by `scripts/New-UnoOneManifestV2.ps1`):

```
UNOONE/
├── Start UnoOne.exe            # on-drive fallback launcher
├── manifest.json               # strict schema v2: every asset's path, size, SHA-256
├── VERSION
├── APPS/
│   ├── WINDOWS/
│   │   ├── UnoOnePower.exe
│   │   └── UnoOneDock.exe
│   └── ANDROID/
│       └── UnoOne.apk          # recorded for tamper-evidence (kind MOBILE_APP)
├── RUNTIMES/
│   └── WINDOWS/
│       ├── CPU/                # llama.cpp CPU
│       ├── CUDA/               # llama.cpp CUDA (NVIDIA)
│       ├── VULKAN/             # llama.cpp Vulkan (AMD/Intel)
│       ├── VOICE/              # legacy Whisper / Piper runtimes
│       └── AUDIO/              # InBharat Audio (audio.cpp) runtime
├── MODELS/
│   ├── MOBILE/                 # Android E2B/E4B .litertlm models
│   └── DESKTOP/
│       ├── Gemma-12B/          # gemma-4-12B-it-Q4_K_M.gguf + mmproj
│       └── …                   # optional E4B / E2B tiers, each with its own mmproj
├── SPEECH/
│   ├── config/                 # inbharat-audio.v1.json
│   └── models/                 # Qwen3-ASR / omnivoice GGUF
├── VAULT/
│   ├── identity/vault.id
│   ├── header/
│   ├── records/
│   ├── indexes/
│   ├── journal/
│   ├── transactions/
│   ├── attachments/
│   └── recovery/
├── CONFIG/
├── RECOVERY/
├── UPDATES/
├── LOGS/
└── SOURCE/                     # optional: git archive of the staged commit
```

UnoOne Dock and the desktop app discover the pen drive by:
1. Scanning removable drives via WMI (if WMI returns nothing, the desktop app
   probes drive letters `D:`–`P:` for an `UNOONE` folder — candidates only)
2. Validating schema-v2 `manifest.json`, `VERSION`, `vault.id`, architecture,
   sizes, and SHA-256 for every required application/runtime/model
3. Rejecting absolute/traversal paths, symlinks, junctions, reparse points,
   missing assets, or changed assets before launch

## Project Structure

```
PAI.V2/
├── android-app/UnoOneAgent/      # Android app, 16 Gradle modules (mobile-protected tree)
├── apps/
│   ├── desktop/
│   │   ├── src/                  # React 19 frontend: 15 components, src/lib, tests/
│   │   └── src-tauri/            # Rust backend (UnoOne Power, 27 modules)
│   ├── dock/windows/             # per-user Windows insertion monitor
│   └── starter/windows/          # on-drive fallback launcher
├── packages/
│   ├── pai-harness-adapter/      # product adapters: memory/model, knowledge, coding tasks, Linux sandbox
│   ├── capability-contracts/     # capability + knowledge record contracts
│   ├── vault-core/               # Rust vault library (+ cross-platform test vectors)
│   ├── encrypted-vault/          # Kotlin vault engine (Android)
│   ├── core-contracts/           # Kotlin contracts
│   ├── usb-manifest/             # strict schema-v2 validator
│   ├── tool-contracts/           # tools.v1.json, the CI-synced tool/risk contract
│   ├── speech-contracts/         # languages.v1.json + SpeechBackend contract
│   ├── runtime-select/           # runtime selection
│   ├── recording-policy/         # recording retention decisions
│   ├── recording-engine/         # Kotlin recording engine
│   ├── browser-policy/           # browser redirect verdicts
│   ├── document-migration/       # plaintext → vault migration
│   └── text-util/                # grapheme-safe truncation
├── platform-adapters/android/    # USB vault connector
├── distribution/                 # distribution API + catalogue
├── installer-pwa/                # installer PWA (downloads locked without a production key)
├── web-runtime/                  # Page Agent web runtime (bundled into Android)
├── SPEECH/config/                # InBharat Audio configuration
├── vendor/
│   ├── inbharat-harness/         # universal Rust control plane (nested workspace)
│   └── Inbharat-audiocpp/        # universal C++ speech plane (CMake)
├── scripts/                      # manifest/staging tools, contract sync checks, mobile baseline
├── docs/                         # architecture, safety, models, speech, verification evidence
└── .github/workflows/            # desktop-ci, mobile-protection, android-ci, distribution-ci, pocket-ai-windows
```

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

## Model Verification

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

## Tests

```bash
# Rust workspace — what Desktop CI runs on Windows, Ubuntu and macOS
# (build apps/desktop/src first: npm ci && npm run build)
cargo fmt --check
cargo test --workspace
cargo clippy --workspace -- -D warnings

# Product adapter incl. knowledge + coding workspace. On Linux with bubblewrap the
# 50 sandbox tests run for real; UNOONE_REQUIRE_ISOLATION=1 forbids skipping them.
UNOONE_REQUIRE_ISOLATION=1 cargo test -p pai-harness-adapter --lib

# Desktop frontend
cd apps/desktop/src
npm ci && npm run build && npm run lint
npm run test:context && npm run test:context:parity && npm run test:context:core
# Mounted UI suites need a separate jsdom@26.1.0 tool root (see tests/*-README.md):
npm run test:context:mounted && npm run test:coding-task && npm run test:knowledge

# Kotlin packages and Android app
cd packages/core-contracts && ./gradlew test     # 10 tests
cd packages/encrypted-vault && ./gradlew test    # 19 tests
cd android-app/UnoOneAgent && ./gradlew test     # ≈920 JVM unit tests; 60 instrumented tests in androidTest
```

Test counts for the Kotlin packages and Android app are `@Test` counts from
the source (2026-10-07).

## Latest verified results

### Knowledge and coding workspace (2026-10-07, `31c7d1d` + `05baa79`)

| Gate | Result |
|---|---|
| Desktop CI on `05baa79` | Rust `fmt`/`check`/`test`/`clippy` green on windows-latest, ubuntu-latest, macos-latest; frontend build, audio ctest, contract syncs, secret and artifact scans green |
| Pocket AI Windows Bundle on `05baa79` | Release build of Power/Dock/Starter on real Windows (MSVC), frontend-embedding gate and voice smoke test green |
| Product adapter, real Linux sandbox (Linux x86_64, bubblewrap 0.10) | 179 passed, 1 ignored helper; 50 sandbox tests ran for real (strict mode, parallel) |
| Windows-style CRLF checkout (simulated) | Adapter 179 passed, contracts 13, desktop glue 10 + 15 |
| Frontend (Node 24) | Build and lint pass; context 53, parity 458, real-core 44, mounted Chat 23, Coding Task 25, Knowledge 24 |
| Clippy `-D warnings` | Adapter and contracts clean for the Linux, Windows and macOS targets |
| Not verified | Running the new screens on Windows hardware, real Tauri IPC/WebView2, the local model with these changes, and the 40-task held-out acceptance suite |

### Desktop resilience and autonomous-agent verification (2026-10-05)

Exercised on the target Windows laptop, not inferred from build output:

| Gate | Result |
|---|---|
| GPU identity | NVIDIA GeForce RTX 5050 Laptop GPU, **8,151 MiB** total VRAM reported by `nvidia-smi`; the earlier 4 GB WMI value was not used |
| Inference topology | llama-server starts with `--parallel 1`, a 16,384-token session context, full available GPU offload, and q8_0 KV cache |
| Throughput | 128-token live completion measured **9.75 tokens/s**, up from approximately **6.1 tokens/s** with four competing slots |
| Autonomous tool turn | Live request completed in **6.23 s** with `finish_reason: tool_calls`, selected `fs.list`, and returned valid structured arguments |
| Long-running generations | SSE activity refreshes a 180 s idle timeout; a distinct 12-minute hard ceiling remains |
| USB-safe launch | Starter/Dock launch the digest-verified host-cached Power executable and pass the original package root explicitly |
| Recovered local package | Desktop copy passes `Start UnoOne.exe --verify-only` with `valid=true` and zero failures |
| Regression gates (pre-2026-10-07 code) | Desktop suite 275/275, harness-bridge subset 43/43, adapter suite 27/27, strict clippy, and release build passed |

The USB device itself still produced Windows Event 51/153 and UASPStor 129
transport resets under load. These changes make UnoOne safer and more
recoverable around that failure; they do not relabel a hardware fault as an
application defect.

### Speech + Android hardening cycle (2026-09-07)

Host (Windows 11, MSVC, Rust 1.93) and emulator (AVD "Medium Phone", API
36.1, x86_64) evidence:

| Gate | Result |
|---|---|
| Android host JVM unit tests (touched modules: app, modelmanager, voice, languagepacks, safetyguard, safety) | BUILD SUCCESSFUL |
| Language-pack install, all 7 baseline packs (en/hi/bn/ta/te/kn/ml), size + SHA-256 verified on-device | `LanguagePackInstallTest` green (5 Indic TTS pins re-pinned after an upstream re-upload; E4B size corrected) |
| Assamese refusal (planned pack, no qualified models) | Install returns Failure, state stays not-installed |
| Real ONNX TTS synthesis (en + hi) + STT engine loads | `SpeechEngineFunctionalTest` — `ttsFailures=0 sttFailures=0` |
| Indic TTS→ASR round trip | `IndicSpeechRoundTripTest` green (hi) |
| Uninstall/retain shared ASR + repair | `LanguagePackRepairRetainTest` green |
| No-cloud speech fallback refusal | `SpeechNoCloudFallbackTest` 4/4 green |
| Headless on-device agent logic batch | 41/41 green |
| Secure browser asset, guarded form fill, local-page read, DOCX/PDF round trips | 8/8 green |
| Local-brain (Gemma) device qualification | Skipped via `assumeTrue` — no `.litertlm` model on the emulator |

Older speech evidence tiers are in
[`docs/SPEECH_TEST_EVIDENCE.md`](docs/SPEECH_TEST_EVIDENCE.md).

### Physical Pocket AI release verification (2026-07-29)

| Gate | Result |
|---|---|
| Physical package | `D:\UNOONE`, exFAT, label `UNOONE`, version `0.5.0-alpha` |
| Strict on-drive verifier | Pass — 545/545 declared assets, exit `0` |
| Native Starter verifier | Pass — `Start UnoOne.exe --verify-only`, exit `0` |
| Windows applications | Power, Dock, and Starter built together and SHA-256 verified |
| Recovery | Every replaced path was staged transactionally |

The manifest provides complete path, size, and SHA-256 integrity checking; it
is not cryptographically signed, and the Windows executables are unsigned. See
[`docs/52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md`](docs/52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md).
(The drive was later reloaded on 2026-10-04 — see Current Status.)

### Android physical evidence (Xiaomi 14 `7f8cafef`, Android 15, July 2026)

| Gate | Result |
|---|---|
| Connected-device instrumentation, July 17 | `OK (55 tests)` in 206.218 seconds |
| No-network subset, July 17 | `OK (20 tests)` with Wi-Fi and mobile data disabled |
| PDF and DOCX Android round trips | `OK (2 tests)`; exact values persisted and original bytes unchanged |
| Phone actions | WhatsApp Business, Gmail, and Calendar opened with foreground-package verification; drafts remain reviewable |
| Read Screen | Read actual Accessibility Settings content and spoke the result |
| Page Agent physical WebView | Read rendered page text and executed guarded text entry; complex public-site completion not yet qualified |
| Hindi speech and Blind Aid | Hindi STT/TTS initialized; COCO labels detected and spoken in Hindi, with no stale narration after stop |
| Master disable | Blocked commands and speech, survived process restart, did not replay the old request |
| Crash/ANR/OOM scan | No UnoOne crash, ANR, OOM, or low-memory kill in the final inspected run |

Details: [Connected-device validation](docs/DEVICE_VALIDATION_2026-07-17.md)
and [Device verification](DEVICE_VERIFICATION.md). The Xiaomi 14 is the only
Android device with recorded evidence.

## Mobile Protection

The Android app is protected by a committed tree-hash pointer plus exact-file
hash verification. `scripts/MOBILE_PROTECTED_TREE` holds the expected tree hash
of `android-app/UnoOneAgent` (currently `bd97bee7…`), and CI fails any push
where `git rev-parse HEAD:android-app/UnoOneAgent` differs from the pointer.
`scripts/MOBILE_GOLDEN_HASHES.txt` carries the per-file blob hashes so the
protected tree can be audited file-by-file. Re-baselining is an ordinary
reviewable commit via `scripts/regen-mobile-golden-hashes.sh` — run it after
committing the Android changes (the script pins the committed tree), then
commit the two baseline files together.

```bash
# Local check (same comparison CI makes)
test "$(cat scripts/MOBILE_PROTECTED_TREE)" = "$(git rev-parse HEAD:android-app/UnoOneAgent)" && echo PASS
```

See [docs/MOBILE_GOLDEN_BASELINE.md](docs/MOBILE_GOLDEN_BASELINE.md) for the
full protection policy.

## Not production-ready yet

The following gates remain open:

- a second Android device and broader OEM/API matrix;
- E4B (Medium) wired into the Android load path, then loaded and tested on device;
- repeatable recorded-speech accuracy tests for every enabled language and multiple accents/noise levels;
- a controlled Blind Aid corpus covering lighting, distance, people, vehicles, phones, and product classes;
- fine-grained product recognition beyond the bundled detector;
- a sustained thermal, memory, battery, and 50-task planning/Page Agent benchmark;
- human verification of audible speech quality and TalkBack announcements;
- live voice, camera, and TalkBack UX validation;
- final visual reruns of Android document/file pickers on an unlocked device;
- approved-site-by-site Page Agent qualification and prompt-injection testing;
- the physical phone ↔ drive ↔ desktop vault round trip and the Android boot auto-launch device gates;
- release dependency/licence review, SBOM, protected signing key, and signed release APK — including an open licence item: the Android Indic TTS voices use Meta MMS VITS models that the repo's own vendor notes list as CC-BY-NC-4.0 (non-commercial) ([model licences](docs/model-licenses.md));
- Authenticode signing of the three Windows executables and cryptographic manifest signing;
- the known app crash on a mid-sweep device-removal IO error (hardware-conditional);
- production object storage, catalogue signing key, signed catalogues, deployment, update, and rollback testing;
- desktop runtime testing still open: recording with a real microphone and blind-aid camera/OCR on a sustained corpus;
- the 2026-10-07 knowledge and coding workspace: staging on the drive, running the new screens on Windows hardware, the local model with these changes, the held-out acceptance suite, and wiring the frontend test suites into CI; sandboxed execution remains Linux-only;
- a macOS app bundle and Mac hardware testing;
- WDAC policy environment testing: verify llama-server, recording, and browser work under real WDAC constraints.

The installer PWA is implemented but intentionally keeps downloads locked when
a production catalogue public key is not configured. No production deployment
or production-approved release is claimed.

## Prohibitions

- ❌ No username/email login — password-only
- ❌ No plaintext storage on disk
- ❌ No cloud fallback without explicit approval
- ❌ No raw model output executing tools directly
- ❌ No weakening SafetyGuard or PageAgent
- ❌ Host disk is not canonical — USB is the single source of truth
- ❌ No mock data, no placeholder success states, no fake functionality
- ❌ No Android changes without a reviewed golden-baseline re-baseline (`scripts/MOBILE_PROTECTED_TREE` + hashes, same push)
- ❌ No drive letter, volume label, or VID/PID as identity — only manifest validation identifies Pocket AI
- ❌ No claiming features work without test evidence (command, exit code, OS, hardware, date, commit)
- ❌ No required external runtimes — Playwright, Tesseract, or a separate Gemma download are neither shipped nor needed (the agent may install tools into a user workspace only through consented `process.run`)
- ❌ No weakening Windows Application Control to make an unsigned build appear successful

## Documentation

- [Android build and validation](android-app/UnoOneAgent/README.md)
- [Phone-control implementation](android-app/UnoOneAgent/phonecontrol/README.md)
- [Android architecture](docs/ARCHITECTURE.md) and [module walkthrough](docs/local-architecture.md)
- [Safety](docs/SAFETY.md) — Android risk tables and desktop safety
- [Models](docs/MODELS.md), [model acquisition and distribution](docs/MODEL_ACQUISITION_AND_DISTRIBUTION.md), [model licences](docs/model-licenses.md), [Blind Aid detector](docs/BLIND_AID_MODEL.md)
- [Speech architecture](docs/SPEECH_ARCHITECTURE.md), [speech model qualification](docs/SPEECH_MODEL_QUALIFICATION.md), [speech test evidence](docs/SPEECH_TEST_EVIDENCE.md), [InBharat Audio integration](docs/INBHARAT_AUDIO_INTEGRATION.md)
- [Offline Document Skills](docs/OFFLINE_DOCUMENT_SKILLS.md)
- [Installer and distribution](docs/INSTALLER_AND_DISTRIBUTION.md)
- [Mobile golden baseline](docs/MOBILE_GOLDEN_BASELINE.md)
- [Connected-device validation](docs/DEVICE_VALIDATION_2026-07-17.md) and [device verification matrix](DEVICE_VERIFICATION.md)
- [Physical release record (2026-07-29)](docs/52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md)
- [Universal capability plan and phase log](docs/UNIVERSAL_CAPABILITY_PLAN_2026-10-01.md)
- [Privacy policy](docs/play-review/privacy-policy.md) and [data safety](docs/play-review/data-safety.md)

## Patent Notice

This software is claimed in Indian provisional patent application
**202631102427** (*Portable Host-Adaptive Private Artificial Intelligence System
with Device-Resident Canonical State*), filed 2026-08-25 with the Patent Office,
Kolkata (ref E106/3399/2026-KOL; TEMP/E1/113020/2026-KOL; docket 25913). The
complete specification is due by 2027-08-25. See [PATENT.md](PATENT.md) for the
full filing record. The release identity, encrypted vault, local-only inference,
capability-gated harness execution, and 22-scheduled-language Bharat speech
runtime are among the aspects covered by the application.

## License

Proprietary — Uni Guru Technologies LLP / InBharat.ai. Repository code, libraries, model weights, and speech artifacts may use different licences or usage terms. Review and preserve the notice attached to every component before redistribution.
