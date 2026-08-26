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
| **Platform** | Android 9+ | Windows / macOS desktop |
| **Model** | Gemma 4 E2B/E4B (LiteRT) | Gemma 4 12B Q4 GGUF (llama.cpp) |
| **UI** | Jetpack Compose | Tauri 2 + React 19 |
| **Storage** | Room cache → USB vault | RAM → USB vault |
| **Voice** | Sherpa-ONNX STT/TTS | Whisper STT / Piper TTS |
| **Eyes-free** | TalkBack, Blind Aid, Camera OCR | Screen reader, high-contrast, OCR |

The release identity is not a drive letter, volume label, or USB VID/PID.
Every host must validate `manifest.json`, `VERSION`, `VAULT/identity/vault.id`,
the declared architecture, and every required asset hash before use.

```
UnoOne Mobile (Android)          UnoOne Power (Desktop)
     Gemma 4 E2B/E4B              Gemma 4 12B Q4 GGUF
          ↕                                ↕
          └──── Shared encrypted USB vault ────┘
                 (Argon2id + AES-256-GCM default; XChaCha20-Poly1305 legacy readable)
```

## What works today

### Android assistant

- Native Kotlin and Jetpack Compose app for Android 9 and later (API 28+), organized into 15 Gradle modules.
- One local Gemma 4 E2B planning engine through LiteRT-LM, with E4B Medium mode available on devices with ≥ 10 GB RAM. Deterministic rules handle common commands before model inference.
- A canonical 42-tool registry (29 legacy + 13 atomic accessibility, messaging, and calendar tools) rejects unknown tools, validates required arguments, and checks argument types.
- Dynamic tool exposure: the orchestrator selects 2–3 candidate tools for E2B (Lite) tasks and 3–6 for E4B (Medium), preventing hallucinated tool calls and reducing context-window waste. The model never sees all 42 tools at once.
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
- Local notes, memory, Skills, activity logs, browser audit records, and preferences.
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
- Removal stops model inference, discards active recording buffers, and
  emergency-locks the vault.
- Real Tauri API calls (no mock data), real SHA-256 verification, honest error states.
- Windows bundle CI builds `UnoOnePower.exe`, `UnoOneDock.exe`, and
  `Start UnoOne.exe` together and publishes their SHA-256 sums as one artifact.

### Android Pocket AI attachment

- The existing UnoOne app handles the physical prototype's USB attach/detach
  intents; there is no companion app.
- The prototype SanDisk VID/PID is only used to offer the app. Android then
  requires Storage Access Framework access and validates schema v2, `VERSION`,
  and `vault.id` before showing Pocket AI as connected.

## Current Status

| Component | Status |
|-----------|--------|
| Physical Pocket AI | **INTEGRITY-VERIFIED PROTOTYPE** — integrity-verified 2026-08-26 (source staged from main `20d0627`): Power `d194e662…`, Dock `5d5e9a4f…`, Starter `106d9b2d…`; strict 545/545 declared assets (size + SHA-256), starter `--verify-only` exit 0; manifest schema `2`, `pai_version 0.5.0-alpha`. Facts: `docs/verification/2026-07-30/54_LIVE_POCKET_AI_AUDIT.md` + `docs/verification/2026-08-01/` |
| Desktop frontend embedding | **VERIFIED** — root cause of the historic "localhost refused to connect" drive is fixed (`tauri/custom-protocol` default feature; without it `generate_context!` embeds zero assets). Byte-level gate passes in CI and on the staged drive binary |
| Mobile app (Android) | V2 agent pipeline + Pocket AI USB auto-open; M1-M3 truthful fixes (USB detection reasons, Room TTL/clear-on-detach cache, unused permission removed). Compiles, lints, tests, and `assembleDebug` passes; **cross-platform vault contract proven bidirectionally in CI** (Kotlin↔Rust Argon2id + AES-GCM record layer AND XChaCha20 master-key wrap — `packages/vault-core/test-vectors/`, `VaultCryptoCrossPlatformTest`). Physical phone test pending |
| Desktop frontend (React) | BUILDS — Vite build passes, oxlint clean, real Tauri API calls, no mock data |
| Desktop backend (Rust) | BUILDS AND TESTS — fmt/check/test/clippy clean on **both windows-latest and macos-latest**; 70/70+ vault-core (Wave-1 regressions + cross-platform vectors), 20/20 recording-policy, 16/16 text-util, 6/6 usb-manifest, 9/9 document-migration, 13/13 browser-policy. The deferred dead-code sweep landed: `wait_for_exit()`, `get_backend()`, the never-read config/model_info fields and `is_confirmation_required()` are gone with clippy `-D warnings` still clean |
| Windows Dock / Starter | **INTEGRITY-VERIFIED ON DRIVE** — manifest-declared, hash-verified, native `--verify-only` exits 0; transactional staging with automatic rollback proven live |
| Vault encryption (`packages/vault-core`) | IMPLEMENTED AND CORRECTNESS-HARDENED — Argon2id (256 MiB / t=3 / p=4) + AES-256-GCM for new records (legacy XChaCha20-Poly1305 stays readable, identified by nonce length) + HKDF-SHA-256 + BIP-39 recovery + write-ahead journal; transactional first-use setup refuses re-initialisation and preserves packaged `vault.id` bytes. Per-vault random salts on both the password and recovery paths. The KDF parameters are pinned as a cross-platform contract with the Kotlin `encrypted-vault` package (`SPEC_ARGON2_*` plus a `const` assertion that makes drift a compile error in release builds), because the test profile deliberately uses reduced parameters and would not catch a change that broke Android↔Windows unlock. **Wave 1** additionally fixed four release blockers: header slot selection now picks the newest committed generation (a password change written to the inactive slot used to be silently discarded on restart), record metadata is authenticated and re-verified on every read (privacy level, tombstone, type, revision and timestamps were previously editable on disk while content still decrypted), record writes are wrapped in real journal transactions with fsync-and-verify before promotion, and record IDs must be canonical UUID v4 before touching a path |
| Model inference | Bundled llama.cpp only; direct runtime test verified (real answer, 127.0.0.1-only, clean stop) — see `docs/verification/2026-07-30/59_DIRECT_GEMMA.md` |
| Offline voice | VERIFIED pipeline — bundled Piper synth → bundled Whisper transcribe round trip is verbatim; see `docs/verification/2026-07-30/62_OFFLINE_VOICE.md` |
| InBharat Audio speech plane | ACCEPTANCE-GATED + DEPLOYED 2026-08-26 — `vendor/Inbharat-audiocpp/` (audio.cpp @ `26dcb5c4`); Qwen3-ASR-0.6B Q8_0 + omnivoice Q8_0 GGUF deployed at `SPEECH/models/`; deployed `audiocpp_cli` runs real ASR (exit 0) + TTS (exit 0, 24 kHz); 30/30 hash-bound acceptance gate green (re-hash of 2 CLIs + 2 models). `get_bharat_audio_status` readiness probe wired; `transcribe`/`synthesize` implemented but intentionally NOT the active Tauri voice path yet — legacy Whisper/Piper stays active until the gate passes (gate now green) |
| Recording | IMPLEMENTED WITH ENFORCED PRIVACY — `unoone-recording-policy` crate makes retention decisions exhaustive (20/20 tests); TRANSCRIPT_ONLY/SUMMARY_ONLY retain no audio; temp WAV deleted + verify-checked; zero-samples reports an error. **SUMMARY_ONLY is disabled in the UI** (no summariser exists) until one is implemented |
| Browser workspace | IMPLEMENTED AS TYPED, VERIFIED ACTIONS — no arbitrary script execution; scheme allowlist; JSON-literal escaping; submit/upload/download require explicit confirmation; real PNG screenshots with SHA-256; 35 deterministic tests. Live-page acceptance journeys are human-gated |
| Text handling (Indic scripts) | HARDENED — `packages/text-util` provides grapheme-cluster-safe truncation. Eight sites previously sliced `&str` at raw byte offsets, which **panics** mid-character; Devanagari and Bengali code points are 3 bytes, so this crashed on ordinary Hindi/Bengali/Assamese documents. Byte budgets for the model context window remain byte budgets (snapped to cluster boundaries) and truncation notices now report real character counts instead of byte counts |
| Plaintext elimination (Wave 3) | SHIPPED — migration core (PR #11) AND read-path (PR #15): migrated records are listed with legacy titles, TF-IDF-searched when unlocked, and read via agent fallback. `migrate_plaintext_documents_to_vault` runs against the real drive in a human session (backup first); the drive has not yet been migrated |
| Document parsing | IMPLEMENTED — PDF (lopdf), DOCX/XLSX/PPTX (zip+quick-xml), TXT/MD/CSV/HTML; TF-IDF keyword search (explicitly NOT described as "semantic") |
| Browser redirect policy | IMPLEMENTED — `unoone-browser-policy` verdicts (same-origin/registrable → reached, HTTPS→HTTP → failure, cross-origin → surfaced `verified=false`, never a silent success); 13 crate tests; live WebView acceptance human-gated |
| Android vault repository | SHIPPED CORE (PR #16) — unlock/read/write/tombstone against the Rust vault, proven bidirectionally by cross-platform vectors AND a Rust-generated synthetic-vault fixture; SAF READ|WRITE grants persisted with truthful fallback; SafVaultIO device-gated. Next: integrate into app flows + phone round trip (see docs/verification/2026-08-01/75_REMAINING_WORK_ORDERED.md) |
| Accessibility (OCR, Blind View) | IMPLEMENTED — OCR/description via Gemma mmproj; confidence honestly `Option<f32>` (unmeasured, never fabricated) |
| Security (vault writes) | IMPLEMENTED — vault_write_record Tauri command writes encrypted records; recording and document content encrypted end-to-end |
| macOS | **NOT BUILT, NOT TESTED** |

See `docs/verification/2026-07-30/` for the latest timestamped verification
package (53 recovery … 73 executive report, incl. `72_RELEASE_MATRIX.csv`).
Older evidence documents are retained as dated historical snapshots and must
be read with their dates; several of their claims were re-verified or
corrected on 2026-07-30.

### CI gate state

`main` is **green across all four gates** (2026-08-01): Desktop CI
(windows-latest + macos-latest, incl. Verify Mobile Untouched),
Mobile Protection, Android CI (`invariants=0 e2e=0 lint=0 tests=0 apk=0`),
and Pocket AI Windows Bundle (incl. the recording-retention and frontend
embedding gates).

The two historically red gates were fixed by design, not by weakening:

- **Mobile protection** moved from a workflow-hardcoded commit pin to a
  committed tree-hash pointer (`scripts/MOBILE_PROTECTED_TREE`) —
  re-baselining is now an ordinary reviewable commit via
  `scripts/regen-mobile-golden-hashes.sh` (hashes computed from git blobs so
  CRLF/CRLF-free checkouts agree).
- **Android lint** baseline was regenerated after review of every accepted
  entry; `warningsAsErrors` stays `true`.

## Architecture

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
                     ModelTierSelector
                    ┌────────┴────────┐
                    │                 │
              E2B (Lite)        E4B (Medium)
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

### Local model contract

UnoOne has two planning-brain profiles.

| Field | Lite (E2B) | Medium (E4B) |
|---|---|---|
| Model id | `gemma-4-e2b` | `gemma-4-e4b` |
| File | `gemma-4-E2B-it.litertlm` | `gemma-4-E4B-it.litertlm` |
| Runtime | LiteRT-LM | LiteRT-LM |
| Exact size | `2,588,147,712` bytes | `~3,700,000,000` bytes |
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
| Tested backend on Xiaomi 14 | CPU fallback | Not yet tested on device |

The model is selected **before** a task starts and **never switches mid-task**. If E4B fails, the task stops, state is preserved, and the orchestrator offers Lite or retry. `ModelTierSelector` uses command complexity and device RAM to decide: simple deterministic commands always use Lite, compound commands (messaging, calendar, notes, web) use Medium when E4B is available and RAM ≥ 10,240 MB, otherwise fall back to Lite.

### Encryption

- **KDF**: Argon2id (256 MiB memory, 3 iterations, parallelism 4) — ✅ implemented in `packages/vault-core`
- **Cipher**: XChaCha20-Poly1305 (desktop) / AES-256-GCM (Android hardware-accelerated)
- **Key wrapping**: Password → Argon2id → KEK → wraps random vault master key (allows password changes without re-encryption)
- **Key isolation**: Master key → HKDF-SHA-256 → per-domain keys (records, journal, indexes, etc.)
- **Header**: Double-buffered (A/B slots), HMAC-SHA-256 authenticated, constant-time comparison
- **Recovery**: 24-word BIP-39 mnemonic (NOT UUID fragments) with independent key wrapping
- **Journaling**: Write-ahead log (PENDING → COMMITTED / ROLLED_BACK) for exFAT crash safety
- **Deletion**: Tombstone records propagate across platforms
- **Password-only login**: No username, no email, no cloud account
- **Memory safety**: Master key zeroed on lock and drop; no passwords in files/logs
- **Vault writes**: `vault_write_record` Tauri command encrypts content via AES-256-GCM (default) or reads legacy XChaCha20-Poly1305 records; stores in `VAULT/records/`; recording and document content flows through this pipeline

> Release rule: do not store sensitive data until the current commit passes CI
> and the physical Pocket AI passes strict on-drive verification.

### Safety Pipeline

```
User input → Model → Parser → ToolAction → SafetyGuard → Execution
```

- **Raw model output never executes tools directly**
- Three security levels: STANDARD (balanced), RELAXED (reduced), OFF (testing only)
- Blocked actions: shell_execute, file_delete_system, network_raw_socket, registry_modify
- Harm detection: system manipulation, data exfiltration, unauthorized access patterns

## Quick Start

### Android (Mobile)

```bash
# Clone and build
git clone https://github.com/inbharatai/PAI.V2.git
cd PAI.V2
./gradlew assembleDebug
# Install on device
adb install app/build/outputs/apk/debug/app-debug.apk
```

### Desktop (Power)

```bash
# Prerequisites: Rust, Node 24+, USB drive formatted exFAT with UNOONE structure

# Build frontend
cd apps/desktop/src
npm install
npm run build

# Build all three portable Windows applications
cd ../../..
cargo build --release \
  -p unoone-power \
  -p unoone-dock-windows \
  -p unoone-starter-windows

# Or run in dev mode
cd apps/desktop/src
npm run tauri dev
```

On a prepared Pocket AI, Windows users start at `Start UnoOne.exe`. The
launcher offers to install Dock for automatic opening on later insertions.

### USB Drive Setup

The USB drive must be formatted exFAT (FAT32 cannot hold the 7.14 GiB 12B model). Insert it and the desktop app detects it automatically via manifest validation.

Expected structure:
```
UNOONE/
├── Start UnoOne.exe            # on-drive fallback launcher
├── manifest.json              # Vault metadata (versioned, relative paths)
├── VERSION                    # e.g. "0.5.0-alpha"
├── APPS/
│   ├── WINDOWS/
│   │   ├── UnoOnePower.exe
│   │   └── UnoOneDock.exe
│   └── MACOS/
├── RUNTIMES/
│   ├── WINDOWS/
│   │   ├── CUDA/              # llama.cpp CUDA 12.4 (NVIDIA GPU)
│   │   ├── CPU/               # llama.cpp CPU (AVX2+ fallback)
│   │   └── VULKAN/            # llama.cpp Vulkan (AMD/Intel GPU)
│   └── MACOS/
│       └── METAL/             # llama.cpp Metal (Apple Silicon)
├── MODELS/
│   ├── MOBILE/                # Android E2B/E4B model packages
│   └── DESKTOP/
│       └── Gemma-12B/
│           ├── gemma-4-12B-it-Q4_K_M.gguf  (7.14 GiB)
│           └── mmproj-gemma-4-12B-it-f16.gguf (116 MiB)
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
└── LOGS/
```

UnoOne Dock and the desktop app discover the pen drive by:
1. Scanning all removable drives (WMI on Windows, `/Volumes/` on macOS)
2. Validating schema-v2 `manifest.json`, `VERSION`, `vault.id`, architecture,
   sizes, and SHA-256 for every required application/runtime/model
3. Rejecting absolute/traversal paths, symlinks, junctions, reparse points,
   missing assets, or changed assets before launch

## Project Structure

```
PAI/
├── android-app/UnoOneAgent/    # V2 agent + Pocket AI USB support (v2 golden baseline)
├── packages/
│   ├── core-contracts/           # Kotlin multiplatform contracts
│   ├── encrypted-vault/          # Kotlin Argon2id + XChaCha20-Poly1305 vault engine (Android)
│   └── vault-core/               # Rust vault library (desktop + shared test vectors)
├── platform-adapters/
│   └── android/                  # USB vault connector, recording engine
├── apps/
│   ├── desktop/                  # Tauri 2 + React 19 desktop app
│       ├── src/                  # React frontend (11 components)
│       └── src-tauri/            # Rust backend (8 modules: main, llama, safety, recording, browser, documents, accessibility, security)
│   ├── dock/windows/             # per-user Windows insertion monitor
│   └── starter/windows/          # on-drive fallback launcher
├── packages/usb-manifest/        # shared strict schema-v2 validator
├── scripts/
│   ├── verify-mobile-untouched.sh  # CI protection: zero changes to Android
│   ├── verify-mobile-untouched.py  # Python equivalent
│   └── verify-mobile-untouched.ps1 # PowerShell equivalent
├── docs/
│   ├── EVIDENCE_AUDIT.md         # Honest status of every feature
│   └── MOBILE_GOLDEN_BASELINE.md  # Frozen mobile baseline documentation
├── .github/workflows/
│   ├── android-ci.yml
│   ├── desktop-ci.yml            # Rust CI: fmt, check, test, clippy (Win+macOS)
│   └── distribution-ci.yml
├── vendor/                       # Vendored InBharat universal planes (self-contained)
│   ├── inbharat-harness/         # Universal Rust control/text plane (nested workspace)
│   │   └── crates/core/          # inbharat-harness-core: routing, providers, tools, execution
│   └── Inbharat-audiocpp/        # Universal C++ speech plane (CMake; STT/TTS/KWS engine)
└── STATUS.md
```

### Vendored universal planes

This repository is **self-contained**: the two InBharat universal planes it
depends on are vendored under `vendor/` rather than referenced as sibling
repositories. Pocket AI is a *product* built on top of them; product-specific
code lives in `packages/pai-harness-adapter` and the app crates, while the
universal code stays in the vendored planes and is reused across InBharat
products.

- **`vendor/inbharat-harness/`** — the universal Rust control/text plane. Its
  own nested Cargo workspace (`vendor/inbharat-harness/Cargo.toml`) provides
  `inbharat-harness-core` (routing, session, the provider model
  `ModelProvider` / `MemoryProvider` / `ToolProvider` / `PermissionProvider` /
  `SafetyProvider` / `ConfirmationProvider` / `VerificationProvider` /
  `SandboxProvider`, and deterministic L1 tool execution). The desktop backend
  and `pai-harness-adapter` depend on it via a path dependency:
  `vendor/inbharat-harness/crates/core`. The outer workspace `exclude`s this
  directory so the vendored crate resolves `*.workspace = true` inheritance
  against its *own* inner workspace root, not the outer one. The Harness core
  deliberately imports **no** Tauri / UNOONE / Pocket AI / vault code — only
  the adapter adds product concerns.
- **`vendor/Inbharat-audiocpp/`** — the universal C++ speech plane (CMake
  build): streaming/edge STT, TTS, and keyword-spotting. It ships as a
  standalone runtime binary on Pocket AI and imports **no** UNOONE UI or
  product logic.

To rebuild the universal planes from their canonical sources instead of the
vendored copies, replace `vendor/inbharat-harness` and `vendor/Inbharat-audiocpp`
with the current trees from the InBharat Harness and InBharat Audio repositories;
the two path dependencies above resolve unchanged.

### Desktop Rust Backend (`apps/desktop/src-tauri/src/`)

| Module | Purpose | Status |
|--------|---------|--------|
| `main.rs` / `startup.rs` | Pocket AI detection, strict validation, startup state machine, removal cleanup, hardware profiling, vault-core integration | IMPLEMENTED |
| `llama.rs` | Manifest-only model/runtime discovery, CUDA/Metal/Vulkan/CPU selection, verified server identity, mmproj vision | IMPLEMENTED |
| `safety.rs` | SafetyGuard (STANDARD/RELAXED/OFF), blocked actions, harm detection | IMPLEMENTED |
| `recording.rs` | Desktop recording: cpal microphone capture, hound WAV encoding, vault-core AES-256-GCM encryption, 4 privacy levels | IMPLEMENTED |
| `browser.rs` | Browser workspace: Tauri WebView bridge (no Playwright), DOM query/click/type/extract/fill/scroll/screenshot | IMPLEMENTED |
| `documents.rs` | Document processing: PDF (lopdf), DOCX/XLSX/PPTX (zip+quick-xml), TXT/MD/CSV/HTML, TF-IDF search | IMPLEMENTED |
| `accessibility.rs` | Blind View, OCR and image description via Gemma mmproj, camera info, encode_image_for_vision | IMPLEMENTED |
| `security.rs` | Manifest-integrity validation, SHA-256, vault-core encryption wiring, crash recovery, emergency lock | IMPLEMENTED |
| `bharat_audio.rs` | InBharat Audio adapter: audio.cpp ASR (Qwen3-ASR) + TTS (omnivoice), hash-bound acceptance gate, deployed-runtime SHA-256 verification | IMPLEMENTED (status probe wired; ASR/TTS entry points gated behind acceptance) |

### Desktop React Frontend (`apps/desktop/src/src/`)

| Component | Purpose | Status |
|-----------|---------|--------|
| `UnlockScreen` | Password-only vault unlock, USB detection, new vault setup | BUILDS_NOT_RUNTIME_TESTED |
| `ChatView` | Gemma 4 conversation via llama-server HTTP | IMPLEMENTED (needs running model) |
| `RecordingView` | Recording with type/privacy, pause/resume/bookmarks, vault encryption | IMPLEMENTED (backend wired, needs UI testing) |
| `MemoryExplorer` | 7 memory types, search, cross-platform sync | BUILDS_NOT_RUNTIME_TESTED |
| `VaultView` | Vault status, emergency lock | BUILDS_NOT_RUNTIME_TESTED |
| `BrowserWorkspace` | URL bar, WebView viewport, DOM bridge actions | IMPLEMENTED (backend wired, needs UI testing) |
| `DocumentsView` | Document import, search (PDF/DOCX/XLSX/PPTX/TXT/MD/CSV/HTML) | IMPLEMENTED (needs UI testing) |
| `AccessibilityView` | Blind View, OCR, high contrast, camera capture | IMPLEMENTED (backend wired, needs UI testing) |

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
| Inference verified | Historical model-content proof exists; current direct manifest-only llama-server execution still requires a prepared host whose policy permits the shipped binaries |
| Source = Destination SHA-256 | ✅ Exact match |

### Desktop dependencies and build boundary

The portable app uses the system WebView and Rust libraries for its application
logic. Model inference remains a manifest-verified llama.cpp runtime shipped on
Pocket AI. This repository does not claim that arbitrary local Application
Control policies will allow unsigned build scripts or binaries: Windows bundle
CI is the reproducible build path, and release signing plus a real prepared-host
insertion test remain mandatory production gates.

## Tests

```bash
# Core contracts (9 tests)
cd packages/core-contracts && ./gradlew test

# Vault core — Rust (CI only, WDAC blocks local builds)
cd packages/vault-core && cargo test

# Encrypted vault — Kotlin (17 tests)
cd packages/encrypted-vault && ./gradlew test

# Android app (~550 JVM tests + 42 instrumented)
cd android-app/UnoOneAgent && ./gradlew test

# Desktop CI (GitHub Actions)
# .github/workflows/desktop-ci.yml
# - Mobile protection check (verify Android untouched)
# - Frontend build (Vite)
# - Rust check/test/clippy on Windows + macOS
# - Secret scan
# - Artifact scan (no model binaries in git)
```

## Latest verified results

Physical Pocket AI release verification on July 29, 2026:

| Gate | Result |
|---|---|
| Physical package | `D:\UNOONE`, exFAT, label `UNOONE`, version `0.5.0-alpha` |
| Strict on-drive verifier | Pass — 545/545 declared assets, exit `0` |
| Native Starter verifier | Pass — `Start UnoOne.exe --verify-only`, exit `0` |
| Canonical manifest | Schema `2`, 214,366 bytes, SHA-256 `FCBB143C61E0D1D46A4BA35AD6CB554B2CCAF876AC6622A3EA313AE8A24A8B00` |
| Windows applications | Power, Dock, and Starter built together and SHA-256 verified |
| Desktop payload | 158 runtime assets, 2 Gemma/mmproj models, 381 voice assets |
| Mobile payload | 2 manifest-declared LiteRT models; 344 protected repository blobs match `mobile-golden-baseline-v2` |
| Offline voice | Real Whisper transcription and Piper WAV generation pass on `main` in run `30439439032` |
| Desktop CI | Windows and macOS fmt/check/test/clippy pass on `main` in run `30439439144` |
| Recovery | Every replaced path was staged transactionally; the pre-remediation manifest remains under `RECOVERY/package-backups/20260729-091422` |

The manifest provides complete path, size, and SHA-256 integrity checking; it
is not cryptographically signed. The current Windows executables are also
unsigned, so release signing and a prepared-host insertion/UX test remain
production gates. See
[`docs/52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md`](docs/52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md)
for the full evidence record.

Automated gates rerun on July 22, 2026 (after V2 agent architecture overhaul):

| Gate | Result |
|---|---|
| Android lint | Pass; no new issues against the existing baseline |
| Android JVM unit tests | Pass; ~550 tests, 0 failures, 0 errors |
| Debug, release, and Android-test assembly/compilation | Pass |
| Repository invariant check | Pass |
| Page Agent TypeScript and unit tests | Pass; 8 unit tests |
| Page Agent Playwright | Pass; 5 browser scenarios |

Physical evidence on the Xiaomi 14 (`7f8cafef`), Android 15:

| Gate | Result |
|---|---|
| Historical connected-device instrumentation, July 17 | `OK (55 tests)` in 206.218 seconds |
| Historical no-network subset, July 17 | `OK (20 tests)` in 77.202 seconds with Wi-Fi and mobile data disabled |
| Latest instrumentation attempt | Test APK compiled; Xiaomi rejected installation with `INSTALL_FAILED_USER_RESTRICTED` |
| PDF and DOCX Android round trips | `OK (2 tests)` in the July 17 suite; exact values persisted and original bytes unchanged |
| Phone actions | WhatsApp Business, Gmail, and Calendar opened with foreground-package verification; drafts remain reviewable |
| Read Screen | Read actual Accessibility Settings content through the accessibility/OCR path and spoke the result |
| Page Agent physical WebView | Read rendered page text and executed guarded text-entry actions; complex public-site completion is not yet qualified |
| Hindi speech and Blind Aid | Hindi STT/TTS engines initialized; person/mobile-phone and other COCO labels were detected and spoken through native Hindi narration, with no stale narration after stop |
| Master disable | Blocked commands and speech, survived process restart, and did not replay the old request after re-enable |
| Crash/ANR/OOM scan | No UnoOne crash, ANR, OOM, missing crypto class, or UnoOne low-memory kill in the final inspected run |

Current-revision checks on July 21 additionally verified a combined Hindi-language + Blind Aid command, real-time detection of `person`, `cell phone`, `bottle`, `book`, and other supported COCO classes, Hindi offline TTS generation, clean camera shutdown with no later detection callbacks, Accessibility-based Read Screen with spoken output, foreground opening of WhatsApp Business/Gmail/Calendar, review-only WhatsApp and Gmail drafts, a Calendar review form containing the exact title/date/5–6 PM time without pressing Save, Secure Browser handoff and local Page Agent model loading, and master-disable persistence across force-stop/restart. Installing the final APK reset Accessibility on HyperOS; it was explicitly re-enabled before the final cross-app verification.

Detailed evidence and honest boundaries are recorded in [Connected-device validation](docs/DEVICE_VALIDATION_2026-07-17.md) and [Device verification](DEVICE_VERIFICATION.md).

## Mobile Protection

The Android app is protected by CI golden-baseline and exact-file hash
verification. `mobile-golden-baseline-v2` points to
`8b66e3e0fa11d462e1676db6ea936ef00f745ada`, the reviewed Pocket AI USB
auto-open baseline. The original v1 tag remains historical and is not moved.

```bash
# Local check
bash scripts/verify-mobile-untouched.sh

# CI check
# .github/workflows/desktop-ci.yml — mobile-protection job
```

See `docs/MOBILE_GOLDEN_BASELINE.md` for the full protection policy.

## Not production-ready yet

The following gates remain open:

- a second Android device and broader OEM/API matrix;
- repeatable recorded-speech accuracy tests for every enabled language and multiple accents/noise levels;
- a controlled Blind Aid corpus covering lighting, distance, people, vehicles, phones, and product classes;
- fine-grained product recognition beyond the bundled COCO detector;
- a sustained thermal, memory, battery, and 50-task planning/Page Agent benchmark;
- human verification of audible speech quality and TalkBack announcements;
- final visual reruns of Android document/file pickers while the physical device is unlocked;
- approved-site-by-site Page Agent qualification and prompt-injection testing;
- release dependency/licence review, SBOM, protected signing key, and signed release APK;
- production object storage, catalogue signing key, signed catalogues, deployment, update, and rollback testing;
- E4B (Medium) model loading and inference on device;
- live voice, camera, and TalkBack UX validation;
- desktop runtime testing: recording with real microphone, browser workspace with real pages, OCR with real images;
- macOS build and testing;
- WDAC policy environment testing: verify llama-server, recording, and browser work under real WDAC constraints.

The installer PWA is implemented but intentionally keeps downloads locked when a production catalogue public key is not configured. No production deployment or production-approved release is claimed.

## Prohibitions

- ❌ No username/email login — password-only
- ❌ No plaintext storage on disk
- ❌ No cloud fallback without explicit approval
- ❌ No raw model output executing tools directly
- ❌ No weakening SafetyGuard or PageAgent
- ❌ Host disk is not canonical — USB is the single source of truth
- ❌ No mock data, no placeholder success states, no fake functionality
- ❌ No Android changes after `mobile-golden-baseline-v2` without a reviewed baseline update
- ❌ No hardcoded drive letters — discover USB via removable-drive scan + manifest validation
- ❌ No claiming features work without test evidence (command, exit code, OS, hardware, date, commit)
- ❌ No external runtimes — no Playwright, no Tesseract, no separate Gemma download
- ❌ No weakening Windows Application Control to make an unsigned build appear successful

## Documentation

- [Android build and validation](android-app/UnoOneAgent/README.md)
- [Phone-control implementation](android-app/UnoOneAgent/phonecontrol/README.md)
- [Offline Document Skills](docs/OFFLINE_DOCUMENT_SKILLS.md)
- [Connected-device validation](docs/DEVICE_VALIDATION_2026-07-17.md)
- [Device verification matrix](DEVICE_VERIFICATION.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Safety](docs/SAFETY.md)
- [Model acquisition and distribution](docs/MODEL_ACQUISITION_AND_DISTRIBUTION.md)
- [Speech model qualification](docs/SPEECH_MODEL_QUALIFICATION.md)
- [Privacy policy](docs/play-review/privacy-policy.md)
- [Data safety](docs/play-review/data-safety.md)

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
