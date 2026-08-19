# UnoOne Pocket AI — Migration Plan

**Repository**: https://github.com/inbharatai/PAI
**Baseline tag**: `v0.4.0-alpha-v2-baseline`
**USB vault**: `D:\PAI\UNOONE\` (460 GB SanDisk)
**Date**: 2026-07-21

---

## Architecture Overview

```
UnoOne Mobile (Android)
Gemma 4 E2B, offline-first
        ↕  USB vault (encrypted)
UnoOne Power (Desktop)
Gemma 4 12B, Windows + macOS
```

Both products share:
- Same encrypted USB vault (single source of truth)
- Same memory, tasks, chats, recordings, documents, skills
- Same tool contracts and safety policies
- Same user identity and preferences

---

## Current Codebase Status (Phase 0 ✅)

### Android App — 14 Gradle modules

| Module | Purpose | Lines (approx) |
|--------|---------|----------------|
| `app` | Main app, AgentOrchestrator, UI | ~6,000 |
| `core` | CanonicalToolRegistry, ReActLoop, IntentClassifier, SafetyJudge | ~3,500 |
| `localbrain` | GemmaPlanner, PromptBuilder, RuleBasedParser | ~2,800 |
| `memory` | MemoryModule, Room DAOs | ~600 |
| `voice` | VoiceService, STT, TTS, WakeWord | ~3,000 |
| `phonecontrol` | PhoneControl, CameraX, OCR, BlindAid | ~2,500 |
| `safetyguard` | Keyword risk classification | ~400 |
| `agentrouter` | Plugin router | ~200 |
| `skills` | User skill CRUD | ~500 |
| `storage` | Room database, DAOs, entities | ~1,500 |
| `modelmanager` | Model download + verification | ~800 |
| `securebrowser` | Page Agent, browser safety | ~2,000 |
| `accessibilitycontrol` | AccessibilityService | ~600 |
| `languagepacks` | Offline speech model management | ~500 |
| `observability` | Diagnostics, timing | ~300 |

**Total**: ~256 Kotlin files, ~44,800 lines

### Web Runtime — TypeScript/Vite
- `web-runtime/page-agent-unoone/` — Page Agent running in Android WebView

### Distribution — Cloudflare Worker
- `distribution/api/` — Model catalog + signing API

### Tests
- 70 JVM unit test files
- 22 Android instrumented test files
- 2 TypeScript test files
- 1 Playwright E2E test
- 1 distribution API test

### Known Bugs (from audit)

**HIGH severity:**
1. AccessibilityNodeInfo leak in `clickNodeWithText()` — double-recycling
2. ComposeView disposal order in `FloatingAgentService.onDestroy()` — remove before dispose
3. `SecurityLevel` stored in plain SharedPreferences (should use EncryptedSharedPreferences)
4. `captureScreen()` blocks with `Thread.sleep` under `@Synchronized` lock (up to 500ms)
5. Room migration only covers v1→v2; future schema changes need migration path
6. MediaProjection token passed via Intent extras (security concern on older APIs)

**MEDIUM severity:**
7. `flaggedFlakyTools` is not thread-safe (`mutableSetOf`)
8. Agent enabled/disabled state reversible via SharedPreferences edit
9. `DataExporter` writes cleartext JSON (notes, skills, memories, action logs)
10. `DebugCommandReceiver` exported in debug builds with DUMP permission
11. `densityDpi` hardcoded to `DENSITY_MEDIUM` (160dpi) for screenshots
12. `VoiceService.hasSpeechActivity()` uses fixed RMS threshold of 500
13. `VoiceService.onTaskRemoved()` restarts foreground service from background (Android 12+ restriction)
14. `FloatingAgentService` lifecycle management incomplete (onDestroy ordering)
15. `NoteDao.search()` LIKE wildcards not escaped for % and _ characters
16. `security-crypto:1.1.0-alpha06` dependency is alpha
17. Guava 27.0.1-android from 2018 with known CVEs

---

## Phased Development Plan

### Phase 1 — Shared Contracts ✅ NEXT

**Goal**: Create platform-agnostic Kotlin contracts that both Android and desktop can use.

**Deliverables**:
- `packages/core-contracts/` — Pure Kotlin multiplatform definitions
  - `Memory.kt` — Memory data class, types (personal, preference, conversation, task, knowledge, accessibility, skill)
  - `Conversation.kt` — Chat history format
  - `Task.kt` — Task state, steps, outputs, approvals
  - `Recording.kt` — Recording object with metadata
  - `Transcript.kt` — Transcript format
  - `Summary.kt` — Summary types (short, detailed, key points, decisions, action items, follow-ups)
  - `Skill.kt` — Skill definition and execution contract
  - `Document.kt` — Document metadata
  - `Preferences.kt` — Cross-platform preferences (language, written/spoken, security level)
  - `ToolAction.kt` — Canonical UnoOne action schema
  - `AuditRecord.kt` — Audit trail format
  - `VaultMetadata.kt` — Vault manifest, schema version, device session IDs
- `packages/core-contracts/build.gradle.kts` — Kotlin multiplatform setup (JVM + Android)

**Constraints**:
- No Android dependencies in contracts
- All data classes must serialize to/from JSON
- Every record includes: id, createdAt, updatedAt, sourcePlatform, sourceDeviceId, revision, deleted, schemaVersion
- Kotlin `kotlinx.serialization` for cross-platform JSON

**Estimated effort**: 2-3 days

---

### Phase 2 — Encrypted Shared Vault (PocketMemoryVault)

**Goal**: Implement the encrypted vault that will live on the USB drive.

**Deliverables**:
- `packages/encrypted-vault/` — Pure Kotlin (JVM-compatible) vault engine
  - `PocketMemoryVault.kt` — Main vault interface
  - `VaultCrypto.kt` — Argon2id KDF + XChaCha20-Poly1305 / AES-256-GCM
  - `VaultSetup.kt` — First-time setup (password creation, recovery key, key generation)
  - `VaultUnlock.kt` — Password-only unlock (no username)
  - `VaultLock.kt` — Lock, emergency eject, inactivity timeout
  - `VaultJournal.kt` — Write-ahead journal, atomic writes, crash recovery
  - `VaultSchema.kt` — Schema versioning, migration logs
  - `VaultSession.kt` — Device session ID, file locking, conflict detection
  - `VaultIntegrity.kt` — Checksums, signed manifests, integrity verification
  - `VaultRecovery.kt` — Recovery key flow (explicit confirmation required)
- `packages/memory-engine/` — Memory retrieval service
  - `MemoryRetrievalService.kt` — Retrieves relevant memories for model context
  - `MemoryIndex.kt` — Encrypted search index

**Encryption spec**:
- Password → Argon2id (memory=256MB, iterations=3, parallelism=4) → derived key
- Data encrypted with XChaCha20-Poly1305 (primary) or AES-256-GCM (fallback)
- Separate domain keys derived from master key
- Write-ahead journal for crash recovery
- Tombstones for deletion (propagate across platforms)

**Constraints**:
- Never store password on disk
- Never store plaintext encryption keys on disk
- Derived keys in memory only while vault is unlocked
- Auto-lock on USB removal
- Auto-lock after inactivity timeout

**Estimated effort**: 5-7 days

---

### Phase 3 — Android Vault Integration

**Goal**: Connect the existing Android app to the USB vault. Preserve all current functionality.

**Deliverables**:
- `platform-adapters/android/` — Android-specific vault adapter
  - `UsbVaultConnector.kt` — Detect UnoOne USB via Storage Access Framework / USB host API
  - `AndroidVaultBridge.kt` — Bridge between Room (cache) and PocketMemoryVault (canonical)
  - `UsbDisconnectHandler.kt` — USB disconnect → flush, lock, clear cache
  - `UsbConnectHandler.kt` — USB connect → verify vault, prompt password, unlock, load context
- Modify `storage` module:
  - Room becomes temporary cache only
  - All writes go to vault first, cache second
  - On vault disconnect, stop new writes, show "Vault disconnected" message
- Modify `memory` module:
  - `MemoryModule` reads from vault (through retrieval service)
  - New memories written to vault
- Modify `app/MainActivity.kt`:
  - USB detection on launch
  - Password-only unlock flow
  - First-time setup flow

**Constraints**:
- All existing Android features must continue to work
- If USB is not connected, app shows "Connect UnoOne Pocket" screen (not crash)
- Room database is temporary cache, not canonical storage
- On USB removal: flush → close → clear keys → lock app

**Estimated effort**: 4-5 days

---

### Phase 4 — Mobile Recording Workspace

**Goal**: Add complete local recording workspace to UnoOne Mobile.

**Deliverables**:
- `packages/recording-engine/` — Cross-platform recording contracts
  - `RecordingSession.kt` — Start, pause, resume, stop, cancel
  - `RecordingMetadata.kt` — Title, type (meeting/lecture/interview/note/journal/general), bookmarks
  - `RecordingPipeline.kt` — Transcription → cleanup → summary → key points → action items
- `android-app/UnoOneAgent/recording/` — Android implementation (new module)
  - `AudioRecordingService.kt` — Encrypted chunked recording using phone microphone
  - `TranscriptionService.kt` — Offline transcription (using existing Sherpa STT)
  - `RecordingRepository.kt` — Save/load/search recordings from vault
  - `RecordingUI.kt` — Compose recording workspace UI
  - `TranscriptSearchUI.kt` — Search within transcripts
- Privacy options: save audio+transcript, transcript only, summary only, private session (no retention)

**Pipeline**: Microphone → Encrypted chunked audio → Offline STT → Transcript cleanup (LLM) → Summary → Key points → Decisions → Action items → Save to vault

**Estimated effort**: 5-6 days

---

### Phase 5 — Desktop Foundation (Tauri + USB Launch)

**Goal**: Create the Tauri desktop application shell.

**Deliverables**:
- `apps/desktop/` — Tauri application
  - `src-tauri/` — Rust backend
    - `main.rs` — App entry, USB detection, hardware profiling
    - `vault.rs` — Vault interface (calling Kotlin/Native or Rust vault library)
    - `usb.rs` — USB detection, vault health check
    - `hw_profile.rs` — RAM, CPU, GPU, VRAM, disk, USB speed
  - `src/` — React frontend
    - `App.tsx` — Desktop-first layout
    - `UnlockScreen.tsx` — Password-only unlock (no username field)
    - `SetupScreen.tsx` — First-time setup (create password, save recovery key)
    - `MainChat.tsx` — AI chat workspace
    - `TaskWorkspace.tsx` — Task management
    - `MemoryExplorer.tsx` — Browse vault memories
    - `VaultStatus.tsx` — USB vault status, lock, eject
    - `SettingsPanel.tsx` — Security, language, model settings
    - `HardwareStatus.tsx` — CPU, RAM, GPU status
    - `EmergencyStop.tsx` — Immediate stop all automation
- `distribution/usb-layout/UNOONE/START_UNOONE_WINDOWS.exe` — Windows launcher
- `distribution/usb-layout/UNOONE/START_UNOONE_MAC.command` — macOS launcher
- Portable execution from USB (no permanent installation required)

**Estimated effort**: 7-10 days

---

### Phase 6 — Gemma 4 12B Desktop Model

**Goal**: Integrate Gemma 4 12B Q4 GGUF with llama.cpp on desktop.

**Deliverables**:
- `packages/model-router/` — Model selection and routing
  - `ModelRouter.kt` — Select E2B (mobile) vs 12B (desktop) based on platform
  - `Gemma4_12BToolAdapter.kt` — Adapt 12B output to canonical tool schema
  - `HardwareProfiler.kt` — RAM, GPU, VRAM, thermal profiling
  - `ContextSizer.kt` — Dynamic context window based on available resources
- `apps/desktop/src-tauri/llama.rs` — llama.cpp integration
  - Windows: CUDA (NVIDIA), Vulkan (AMD/Intel), CPU fallback
  - macOS: Metal (Apple Silicon)
  - Context sizing: 4K default, up to 16K if RAM permits
- `distribution/usb-layout/UNOONE/MODELS/gemma4-12b-q4-gguf/` — Model directory
- Model SHA-256 verification before loading
- Hardware check before model load (refuse if insufficient resources)

**Constraints**:
- GGUF is canonical (not MLX-only) for cross-platform portability
- Model must be on USB drive
- Must refuse to load if host cannot run it safely
- Show appropriate warnings or reduced-context mode

**Estimated effort**: 5-7 days

---

### Phase 7 — Desktop Recording and Voice

**Goal**: Add recording and voice to desktop using the same recording schema.

**Deliverables**:
- `packages/voice-engine/` — Cross-platform voice contracts
- `apps/desktop/src-tauri/recording.rs` — Desktop microphone recording
- Desktop STT (Sherpa or Whisper-based)
- Desktop TTS
- Same `Recording` object saved to same vault schema
- Desktop can re-analyze mobile recordings with 12B model (without duplicating originals)

**Estimated effort**: 4-5 days

---

### Phase 8 — Browser Workspace

**Goal**: Integrate Chromium + Playwright + PageAgent policies on desktop.

**Deliverables**:
- `packages/browser-agent/` — Cross-platform browser agent contracts
- `apps/desktop/src-tauri/browser.rs` — Chromium integration
- PageAgent policies from Android `securebrowser` module
- Emergency Stop
- Execution timeline display
- Trusted-domain management
- Per-session permissions
- Encrypted audit records

**Constraints**:
- Password entry remains manual
- OTP entry remains manual
- CAPTCHA remains manual
- Payment confirmation blocked or requires explicit approval
- Destructive actions require confirmation

**Estimated effort**: 5-7 days

---

### Phase 9 — Documents and Memory Retrieval

**Goal**: Desktop document workspace and cross-platform memory retrieval.

**Deliverables**:
- `packages/document-engine/` — Document parsing contracts
- PDF, DOCX, TXT, Markdown, CSV, XLSX, PPTX, images, audio parsers
- Encrypted index (embeddings stored in vault)
- Memory retrieval service (both platforms use same retrieval contract)
- Cross-platform task continuation

**Estimated effort**: 5-6 days

---

### Phase 10 — Accessibility and Camera

**Goal**: Desktop accessibility adapters and camera support.

**Deliverables**:
- `platform-adapters/windows/` — Windows UI Automation adapter
- `platform-adapters/macos/` — macOS Accessibility API adapter
- Desktop OCR fallback (Tesseract)
- Desktop Blind View (accessibility tree reading)
- Webcam input for image understanding
- Screenshot analysis

**Estimated effort**: 4-5 days

---

### Phase 11 — Security Hardening

**Goal**: Sign manifests, verify binaries, test crash recovery.

**Deliverables**:
- Signed manifests for all executables and models
- Binary integrity checks on startup
- Model SHA-256 verification
- Recovery key testing
- Corruption testing (manual unplug during write)
- Unsafe-eject testing
- Password brute-force protections (increasing delay, lockout)
- Host storage audit (confirm no plaintext remains after logout)

**Estimated effort**: 4-5 days

---

### Phase 12 — Cross-Platform Validation

**Test matrix**:
- Android (phone)
- Windows without GPU
- Windows with NVIDIA GPU
- Windows with integrated graphics
- macOS Apple Silicon
- Low-RAM laptop (8 GB)
- 16 GB Mac
- 24 GB Windows laptop
- USB-A connection
- USB-C connection

**Mandatory tests** (from codex §25):
- Save memory on Windows → reopen on Android → verify same memory
- Modify on Android → reopen on macOS → verify update
- Record on Android → transcribe → reopen on Windows → re-analyze with 12B → both summaries visible without duplication
- Password-only unlock: no username, wrong password rejected, recovery key works through explicit flow
- USB removal immediately locks vault
- Delete on Android → reopen on Windows → confirm deleted
- Crash recovery: interrupt write → reopen → no corruption, no data loss
- Host storage audit: no plaintext chats/transcripts/preferences after secure shutdown

**Estimated effort**: 5-7 days

---

## Repository Structure

```
UnoOne-PAI/
├── apps/
│   ├── android/                    ← Current UnoOneAgent (preserved as-is)
│   └── desktop/                    ← Tauri + React desktop app
├── packages/
│   ├── core-contracts/             ← Shared Kotlin contracts (Phase 1)
│   ├── memory-engine/              ← Memory retrieval service (Phase 2)
│   ├── encrypted-vault/            ← PocketMemoryVault (Phase 2)
│   ├── agent-orchestrator/         ← Cross-platform orchestrator (later)
│   ├── model-router/               ← E2B/12B routing (Phase 6)
│   ├── voice-engine/               ← Cross-platform voice contracts (Phase 7)
│   ├── recording-engine/           ← Cross-platform recording contracts (Phase 4)
│   ├── browser-agent/              ← Cross-platform browser contracts (Phase 8)
│   ├── document-engine/            ← Cross-platform document contracts (Phase 9)
│   ├── skills-engine/              ← Cross-platform skills (later)
│   ├── task-engine/                ← Cross-platform task contracts (later)
│   └── security/                   ← Cross-platform security (Phase 11)
├── platform-adapters/
│   ├── android/                    ← Android vault + USB adapter (Phase 3)
│   ├── windows/                    ← Windows adapters (Phase 10)
│   ├── macos/                      ← macOS adapters (Phase 10)
│   └── linux/                      ← Linux adapters (later)
├── distribution/
│   └── usb-layout/                 ← USB directory structure
├── model-catalogue/                ← Model manifests and hashes
├── tests/                          ← Cross-platform integration tests
├── docs/                           ← Documentation
├── android-app/                    ← Preserved original (read-only reference)
├── web-runtime/                    ← Preserved original (read-only reference)
├── distribution-api/              ← Preserved original (read-only reference)
├── MIGRATION-PLAN.md               ← This document
├── DEVICE_VERIFICATION.md
├── STATUS.md
└── README.md
```

## Bug Fix Priority (from audit)

Fix these before or during Phase 1:

1. **SecurityLevel in plain SharedPreferences** → Migrate to EncryptedSharedPreferences
2. **FloatingAgentService ComposeView disposal order** → Remove from WindowManager before dispose
3. **AccessibilityNodeInfo double-recycling** → Fix node lifecycle
4. **captureScreen() synchronized Thread.sleep** → Use coroutine-based Image acquisition
5. **flaggedFlakyTools thread safety** → Use ConcurrentHashMap.newKeySet()
6. **DataExporter cleartext** → Encrypt exports or use vault
7. **VoiceService.onTaskRemoved background restart** → Use WorkManager
8. **NoteDao.search() LIKE wildcards** → Escape % and _
9. **Guava 27.0.1** → Update or remove (CameraX no longer requires it)
10. **security-crypto alpha** → Track stable release

## Prohibitions (from codex §26)

- ❌ Do not replace the existing Android app unnecessarily
- ❌ Do not remove current UnoOne features
- ❌ Do not separate mobile and desktop memory
- ❌ Do not use username-based login
- ❌ Do not require email for local use
- ❌ Do not store the vault password
- ❌ Do not store private data in plaintext
- ❌ Do not make the desktop host disk the canonical storage
- ❌ Do not make Android Room the canonical shared memory
- ❌ Do not make desktop SQLite outside the USB the canonical memory
- ❌ Do not use MLX as the only Power model distribution
- ❌ Do not introduce cloud fallback without explicit approval
- ❌ Do not send private recordings or documents to cloud services
- ❌ Do not allow raw model output to execute tools
- ❌ Do not weaken SafetyGuard or PageAgent
- ❌ Do not load multiple large models simultaneously
- ❌ Do not rename every package before achieving a working baseline
- ❌ Do not claim completion without Windows, macOS, and Android tests