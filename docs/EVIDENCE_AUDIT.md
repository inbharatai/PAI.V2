# PAI Evidence-Based Audit — 2026-07-23 12:00 UTC

> Historical snapshot: this document predates the 2026-07-29 Pocket AI
> auto-launch remediation. Current release decisions live in
> `docs/51_FINAL_ACCURACY_AND_RELEASE_MATRIX.csv`; host Ollama/LM Studio
> fallbacks described below were removed and are not current behavior.

**Commit:** 05a5a5e (pendrive minimal-dependency implementation — 6 phases)
**Auditor:** Claude Code
**OS:** Windows 11 Home, Application Control policy blocks Rust build scripts
**Hardware:** Unknown GPU (not yet detected by nvidia-smi in this session)

---

## Status Legend

| Status | Meaning |
|--------|---------|
| VERIFIED_WORKING | Build passes, runtime tested, output evidence attached |
| BUILDS_NOT_RUNTIME_TESTED | Compiles/builds but never executed |
| IMPLEMENTED_NOT_TESTED | Code is complete but not tested in Tauri runtime |
| PARTIALLY_IMPLEMENTED | Structure exists, some logic works, core missing |
| BLOCKED_BY_ENVIRONMENT | Cannot verify due to OS/hardware/tooling |

---

## 1. Frontend (React + Vite)

| Component | Status | Evidence |
|-----------|--------|----------|
| Vite build | VERIFIED_WORKING | `vite build` exits 0, 4 assets produced |
| ChatView | IMPLEMENTED_NOT_TESTED | HTTP client to `/v1/chat/completions` with multimodal Content support; needs running model |
| VaultView | BUILDS_NOT_RUNTIME_TESTED | Shows real Tauri API calls but never executed in Tauri runtime |
| MemoryExplorer | BUILDS_NOT_RUNTIME_TESTED | Same — Tauri API calls but never executed |
| DocumentsView | IMPLEMENTED_NOT_TESTED | Backend processes PDF/DOCX/XLSX/PPTX/TXT/MD/CSV/HTML; frontend not runtime-tested |
| BrowserWorkspace | IMPLEMENTED_NOT_TESTED | Backend uses Tauri WebView bridge; frontend not runtime-tested |
| AccessibilityView | IMPLEMENTED_NOT_TESTED | Backend has OCR/Blind View via mmproj + camera info; frontend not runtime-tested |
| RecordingView | IMPLEMENTED_NOT_TESTED | Backend has cpal + hound + vault encryption; frontend has false-success catch blocks (see §2) |
| UnlockScreen | BUILDS_NOT_RUNTIME_TESTED | Tauri API calls but never executed |
| tauri.ts | IMPLEMENTED_NOT_TESTED | Real `invoke()` calls including `vaultWriteRecord`, `performOcr`, `describeImage`, `encodeImageForVision` |

## 2. False-Success Paths (Remaining Issues)

These are places where the UI shows success even when the backend fails:

### RecordingView.tsx

**Line ~89-91:** `stopRecording` catch block swallows error — UI shows "Recording stopped" even if backend failed:
```ts
try {
  await tauriApi.stopRecording();
} catch {
  // If Tauri not available, still update UI
}
```

**Line ~115-118:** `startRecording` catch block enters recording state even on failure:
```ts
} catch (e) {
  setError(e instanceof Error ? e.message : String(e));
  // Still allow UI recording even if Tauri is not available (for development)
  setIsRecording(true);
```

**Lines ~130, 135:** Pause/resume and bookmark errors silently swallowed:
```ts
} catch {} // pause
} catch {} // resume
} catch {} // bookmark
```

### VaultView.tsx

**Line ~86:** Emergency lock failure silently swallowed:
```ts
try { await tauriApi.lockVault(); } catch {}
```

### MemoryExplorer.tsx, DocumentsView.tsx

**Lines ~37-39, 40:** Vault detection failures silently set empty state without informing user.

> **Note:** The backend implementations are real — cpal captures audio, hound encodes WAV, vault-core encrypts. The false-success paths are a frontend error-handling issue, not a missing-backend issue.

## 3. Hardcoded Paths

| File | Line | Hardcoded Path | Platform | Fix Required |
|------|------|---------------|----------|--------------|
| `RecordingView.tsx` | 39 | `D:\\UNOONE` | Windows only | ❌ Must use `detectVault()` result |
| `VaultView.tsx` | 72 | `D:\\UNOONE` | Windows only | ❌ Same |
| `main.rs` | 135 | `"D:\\UNOONE", "E:\\UNOONE", "F:\\UNOONE"` | Windows only | ❌ Must scan removable drives |
| `main.rs` | 139 | `"/Volumes/PAI/UNOONE", "/Volumes/UNOONE"` | macOS only | ❌ Hardcoded volume names |

No cross-platform removable-drive detection exists. macOS and Linux will fail unless volumes happen to match these exact names.

## 4. Rust Backend — Implementation Status

All 8 modules have real implementations. WDAC blocks local builds; CI is configured for verification.

| Module | Status | Implementation |
|--------|--------|----------------|
| `main.rs` | IMPLEMENTED_NOT_TESTED | USB vault detection, manifest validation, hardware profiling, vault-core integration, `vault_write_record` command |
| `llama.rs` | IMPLEMENTED_NOT_TESTED | Model manager with CUDA/Metal/Vulkan/CPU detection, mmproj vision, Content enum (Text/Multimodal), WDAC fallback (detect_inference_backend checks llama-server/Ollama/LM Studio), health check with Ollama fallback |
| `safety.rs` | IMPLEMENTED_NOT_TESTED | SafetyGuard (STANDARD/RELAXED/OFF), blocked actions, harm detection |
| `recording.rs` | IMPLEMENTED_NOT_TESTED | cpal microphone capture, hound WAV encoding, vault-core XChaCha20-Poly1305 encryption, 4 privacy levels (Full, TranscriptOnly, SummaryOnly, PrivateSession) |
| `browser.rs` | IMPLEMENTED_NOT_TESTED | Tauri WebView bridge with `__unooneBrowserBridge` JS for DOM query/click/type/extract/fill/scroll/screenshot; no Playwright/Chromium dependency |
| `documents.rs` | IMPLEMENTED_NOT_TESTED | PDF (lopdf), DOCX/XLSX/PPTX (zip+quick-xml), TXT/MD/CSV/HTML parsing; TF-IDF search |
| `accessibility.rs` | IMPLEMENTED_NOT_TESTED | OCR and image description via Gemma mmproj model, camera info via getUserMedia, encode_image_for_vision base64 pipeline |
| `security.rs` | IMPLEMENTED_NOT_TESTED | Vault writes wired (vault_write_record), SHA-256 manifest verification, crash recovery, emergency lock |

**Note:** `cargo check --workspace` passes with 0 errors, 6 warnings (all pre-existing). WDAC prevents local Rust binary compilation and testing; CI (GitHub Actions) runs fmt/check/test/clippy on Windows + macOS.

## 5. Model Loading & Inference

| Claim | Reality | Evidence |
|-------|---------|----------|
| "Gemma 4 12B Q4_K_M loaded" | **File may be MISSING** | Previous audit could not find the 7.14 GiB model on USB. Verify manually. |
| "llama-server.exe present" | Stub (9 KB launcher) | Thin wrapper that loads `llama-server-impl.dll`. WDAC blocks execution. |
| "Inference verified via Ollama proxy" | **VERIFIED** | Ollama on port 11434 responds to chat completions. |
| `detect_inference_backend()` | IMPLEMENTED | Checks ports 8342 (llama-server), 11434 (Ollama), 1234 (LM Studio). Returns backend type and port. |
| `check_model_health()` | IMPLEMENTED | Tries llama-server /health first, falls back to Ollama /api/tags. |
| mmproj vision | IMPLEMENTED | `ModelConfig.mmproj_path` loads vision model via `--mmproj` flag; `Content::with_image()` sends multimodal requests. |

### Model File Status

| File | Status |
|------|--------|
| `gemma4-12b-q4-gguf/gemma-4-12B-it-Q4_K_M.gguf` | **VERIFY** — may be missing from USB |
| `gemma4-12b-q4-gguf/mmproj-gemma-4-12B-it-f16.gguf` | Present (122 MB) |
| `gemma4-e2b/gemma-4-e2b-q4_k_m.gguf` | Present (3.2 GB) |

## 6. Vault Encryption — IMPLEMENTED

| Feature | Status | Evidence |
|---------|--------|----------|
| Argon2id key derivation | ✅ IMPLEMENTED | `packages/vault-core` has full Argon2id KDF (256 MiB, 3 iterations, parallelism 4) |
| XChaCha20-Poly1305 encryption | ✅ IMPLEMENTED (legacy) | Still readable for older desktop vaults; auto-detected by 24-byte nonce |
| AES-256-GCM encryption | ✅ IMPLEMENTED (default) | New cross-platform default; Android hardware-accelerated; Rust via `aes-gcm` crate |
| Password verification | ✅ IMPLEMENTED | vault-core unlock/create/lock with Argon2id key derivation |
| Vault lock (clear keys from memory) | ✅ IMPLEMENTED | Master key zeroed on lock and drop |
| Emergency lock | ✅ IMPLEMENTED | Writes `.lock` marker + clears in-memory keys |
| Write-ahead journaling | ✅ IMPLEMENTED | PENDING → COMMITTED / ROLLED_BACK transaction states |
| HKDF-SHA-256 key isolation | ✅ IMPLEMENTED | Master key → per-domain keys (records, journal, indexes) |
| BIP-39 recovery | ✅ IMPLEMENTED | 24-word mnemonic with independent key wrapping |

## 7. Feature Implementation Status

| Feature | Status | What's Missing |
|---------|--------|---------------|
| Vault unlock/setup | ✅ IMPLEMENTED | Full Argon2id + XChaCha20-Poly1305 via vault-core |
| Chat with Gemma 4 | IMPLEMENTED_NOT_TESTED | HTTP client with multimodal Content support; needs running model for runtime test |
| Recording | ✅ IMPLEMENTED | cpal audio capture + hound WAV encoding + vault-core encryption; frontend errors now surfaced |
| Browser | ✅ IMPLEMENTED | Tauri WebView bridge with DOM manipulation JS; frontend not runtime-tested |
| OCR | ✅ IMPLEMENTED | Gemma mmproj model via llama-server; needs running model for runtime test |
| Image description | ✅ IMPLEMENTED | `describe_image()` via mmproj; needs running model for runtime test |
| Camera | ✅ IMPLEMENTED | `get_camera_info()` + `encode_image_for_vision()` base64 pipeline; frontend getUserMedia not tested |
| Document parsing | ✅ IMPLEMENTED | PDF (lopdf), DOCX/XLSX/PPTX (zip+quick-xml), TXT/MD/CSV/HTML |
| Memory search | ✅ IMPLEMENTED | TF-IDF text matching works; vector embeddings intentionally not in scope for v1 |
| SHA-256 manifest | ✅ IMPLEMENTED | `sha2` crate used in security.rs |
| USB vault detection | ✅ IMPLEMENTED | Removable-drive scan via WMI/Volumes/mount points + manifest validation; debug fallbacks are compile-time gated |
| Inference backend fallback | ✅ IMPLEMENTED | `detect_inference_backend()` checks llama-server/Ollama/LM Studio |

## 8. Android Baseline — Previously Verified

| Test | Status | Evidence |
|------|--------|----------|
| Android JVM tests (~550) | ✅ PASS | Verified on Xiaomi 14, Android 15 |
| Instrumented tests (42) | ✅ PASS | July 17, 2026, on Xiaomi 14 |
| V2 agent pipeline | ✅ MERGED | Merged to main |
| Page Agent TypeScript + unit | ✅ PASS | 8 unit tests |
| Page Agent Playwright | ✅ PASS | 5 browser scenarios |

## 9. macOS — NOT BUILT, NOT TESTED

No macOS build has been attempted. No macOS-specific code has been executed. The code is cross-platform (uses cfg(target_os)) but untested.

## 10. Cross-Platform Memory Sync — NOT TESTED

The Kotlin `core-contracts` define shared data types, but no test has verified that data written by Android can be read by Desktop, or vice versa. The Kotlin/Native interop layer for Rust does not exist.

## 11. Desktop Dependencies (Pure Rust, WDAC-Safe)

| Crate | Purpose | Size | WDAC-safe |
|-------|---------|------|-----------|
| `cpal 0.15` | Microphone audio capture | ~300 KB | ✅ |
| `hound 3.5` | WAV encoding | ~50 KB | ✅ |
| `lopdf 0.33` | PDF text extraction | ~500 KB | ✅ (nom_parser, no rayon) |
| `zip 2` | ZIP/DOCX/XLSX/PPTX parsing | ~800 KB | ✅ (deflate only, no bzip2-sys) |
| `quick-xml 0.37` | XML parsing for DOCX/XLSX/PPTX | ~200 KB | ✅ |
| `base64 0.22` | Base64 encoding for vision | ~30 KB | ✅ |
| `reqwest 0.12` | HTTP client for inference | ~500 KB | ✅ |
| `unoone-vault-core` | Argon2id + XChaCha20-Poly1305 | ~1 MB | ✅ |

**Total added: ~3.8 MB** — no C/C++ system libraries, no external runtimes.

## 12. Summary

| Category | IMPLEMENTED | IMPLEMENTED_NOT_TESTED | PARTIALLY_IMPLEMENTED | BLOCKED_BY_ENVIRONMENT |
|----------|-------------|------------------------|----------------------|----------------------|
| Frontend build | ✅ Vite | | | |
| Frontend runtime | | All 11 components | | |
| Rust backend | | All 8 modules | | WDAC blocks local test |
| Vault encryption | ✅ Full KDF+cipher+journal | | | |
| Model inference | | HTTP client + fallback | | WDAC blocks llama-server |
| Recording audio | ✅ cpal + hound + vault | | | |
| Browser workspace | ✅ WebView bridge | | | |
| Document parsing | ✅ 7 formats + TF-IDF | | | |
| OCR + Blind View | ✅ mmproj pipeline | | | |
| Android tests | ✅ Pass | | | |
| macOS | | | | ❌ Not built |
| USB detection | ✅ Removable scan | | | |

**Previous "NOT IMPLEMENTED" claims for recording, browser, accessibility, documents, and security were false at the time of the previous audit — vault-core existed but was not wired. Frontend false-success catch blocks and hardcoded USB paths have since been fixed. The remaining gaps are runtime testing on a non-WDAC machine, macOS build, live model inference, and a real Android ↔ Desktop vault round-trip.**

---

## 13. Remaining Gaps

1. **Frontend false-success paths** — FIXED. RecordingView, VaultView, MemoryExplorer, DocumentsView, HardwareProfile, SettingsView, and App.tsx now surface Tauri errors via `setError()` or `console.error` + `bootError` banner instead of silently swallowing exceptions. Verified by `npm run build` in `apps/desktop/src/`.

2. **Hardcoded USB paths** — FIXED. `apps/desktop/src-tauri/src/main.rs` already scans removable drives via WMI on Windows, `/Volumes` on macOS, and `/mnt`/`/media` on Linux, validating each candidate by `UNOONE/manifest.json` + `VERSION` + `vault.id`. The `C:\UNOONE`/`/tmp/UNOONE` paths are debug-only compile-time fallbacks. Frontend views use `tauriApi.detectVault()` instead of hardcoded letters.

3. **macOS build and test** — No macOS machine has compiled or run the Tauri app.

4. **Live model inference** — Need a non-WDAC machine to verify llama-server + mmproj works end-to-end.

5. **Gemma 12B model file** — Verify the 7.14 GiB Q4_K_M model is present on USB.

6. **Cross-platform sync** — Deterministic HKDF + AES-256-GCM cross-platform vectors exist in `docs/VAULT_CROSS_PLATFORM_VECTORS.md` and are exercised by both Kotlin and Rust unit tests. A live round-trip test still needs a non-WDAC Windows host + Android USB-passthrough device (see `docs/P0_DESKTOP_NON_WDAC_RUNBOOK.md` and `docs/P0_ANDROID_HARDWARE_RUNBOOK.md`).

---

## 14. P0 Remediation — 2026-07-26

Three authorized P0 streams are in progress. Code-level fixes were completed on
this host; the remaining blockers are environmental (WDAC policy on this Windows
machine, or physical Android hardware / USB passthrough).

### P0-A: Desktop Runtime Repair (`remediation/p0-desktop-runtime`)

| Fix | Status | Evidence |
|-----|--------|----------|
| `llama.rs` hardened `start_server` (dynamic port, /health + /v1/models identity, SHA-256 manifest check, child cleanup, failure-path unit tests) | ✅ IMPLEMENTED | `apps/desktop/src-tauri/src/llama.rs` |
| PowerShell acceptance script (19 items: port, manifest hash, identity, inference smoke, shutdown, JSON report) | ✅ IMPLEMENTED | `apps/desktop/scripts/test-llama-server.ps1` |
| Async `Send` fix for `MutexGuard<AccelerationBackend>` across await | ✅ IMPLEMENTED | Scoped guard before `.await` |
| Failure-path Rust unit tests with mockito | ✅ IMPLEMENTED | 7 tests in `llama.rs` |

| Gate | Result |
|------|--------|
| `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml` | ✅ PASS |
| `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets` | ✅ PASS (0 warnings) |
| `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --tests` | ✅ PASS |
| PowerShell syntax validation (`[PSParser]::Tokenize`) | ✅ PASS |
| `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --no-run` | ❌ BLOCKED_BY_ENVIRONMENT — WDAC Event IDs 3077/3033 block the `tauri` build script and unsigned proc-macro DLLs |
| `test-llama-server.ps1` runtime (19 acceptance items) | ❌ BLOCKED_BY_ENVIRONMENT — needs non-WDAC host + USB vault + `llama-server.exe` |

### P0-B: Mobile USB Vault Integration (`remediation/p0-mobile-usb-vault`)

Validated on its own branch on this host (branch not merged here):

| Fix | Status | Evidence |
|-----|--------|----------|
| `:vault` module wired in `settings.gradle.kts` | ✅ IMPLEMENTED | `android-app/UnoOneAgent/settings.gradle.kts` |
| `MobileVaultRepository` + `UsbVaultRepository` | ✅ IMPLEMENTED | `android-app/UnoOneAgent/vault/src/main/java/...` |
| `EncryptedRoomCacheRepository` (AES-256-GCM at rest) | ✅ IMPLEMENTED | `android-app/UnoOneAgent/vault/src/main/java/...` |
| `VaultSyncCoordinator` (USB ↔ Room cache) | ✅ IMPLEMENTED | `android-app/UnoOneAgent/vault/src/main/java/...` |
| Android HKDF alignment with Rust | ✅ VERIFIED_WORKING | `./gradlew :encrypted-vault:test` passes; regression vector `2620d038...` matches Rust reference |

| Gate | Result |
|------|--------|
| `./gradlew :vault:check` | ✅ PASS |
| `./gradlew :core:check` | ✅ PASS |
| `./gradlew :app:check` | ✅ PASS |
| `LoggerRedactionTest` | ✅ PASS |
| `EncryptedRoomCacheRepositoryTest` | ✅ PASS |
| `UsbVaultRepositoryInstrumentedTest` | ❌ BLOCKED_BY_ENVIRONMENT — needs physical Android device or emulator with USB passthrough |

### P0-C: Mobile Privacy Logging Redaction (`remediation/p0-mobile-usb-vault`)

| Fix | Status | Evidence |
|-----|--------|----------|
| Structured redacted logging with Aadhaar/email/card/phone/URL-credential detectors | ✅ IMPLEMENTED | `core/src/main/java/com/unoone/agent/core/util/Logger.kt` |
| Marker self-redaction fix (`~` separator, detector ordering) | ✅ VERIFIED_WORKING | `LoggerRedactionTest` |

| Gate | Result |
|------|--------|
| `./gradlew :core:testDebugUnitTest --tests LoggerRedactionTest` | ✅ PASS |

### Cross-Platform Vault Convergence

| Fix | Status | Evidence |
|-----|--------|----------|
| Android `VaultStorage.getDomainKey()` switched from HMAC-SHA256 to HKDF-SHA256 | ✅ VERIFIED_WORKING | `packages/encrypted-vault/.../VaultStorage.kt` |
| Rust `vault-core` AES-256-GCM support via `aes-gcm` | ✅ IMPLEMENTED | `packages/vault-core/src/crypto.rs` |
| AES-256-GCM default for new records; legacy XChaCha20 readable by nonce-length detection | ✅ IMPLEMENTED | `packages/vault-core/src/vault.rs` |
| HKDF regression vector `2620d0380a68bffda15bb83301337751729230cef7252c1704e879e119775e5f` | ✅ VERIFIED | Kotlin + Rust unit tests |
| AES-256-GCM cross-platform vector `126cb954ab80e278ff2ee4f89c99140cf7b0e8469d5b291d4d0b91a56e106ed1299d877efe51de720ef0c8ab96fe2af9` | ✅ VERIFIED | Kotlin + Rust unit tests |
| Cross-platform vector documentation | ✅ IMPLEMENTED | `docs/VAULT_CROSS_PLATFORM_VECTORS.md` |

### Build Gate Summary (This Host)

| Gate | Result |
|------|--------|
| `packages/encrypted-vault` JVM tests | ✅ PASS |
| `packages/vault-core` Rust tests | ✅ PASS (53 tests) |
| `packages/vault-core` `cargo check --all-targets` | ✅ PASS |
| Desktop `cargo fmt` | ✅ PASS |
| Desktop `cargo check --all-targets` / `--tests` | ✅ PASS |
| Desktop frontend `npm run build` (`apps/desktop/src/`) | ✅ PASS |
| Desktop frontend `oxlint` (`apps/desktop/src/`) | ✅ PASS (1 accepted warning: `selectedModelPath` wired for future load/start) |
| Desktop `cargo clippy --all-targets -- -D warnings` | ✅ PASS |
| Android `:vault:check` | ✅ PASS |
| Android `:core:check` | ✅ PASS |
| Android `:app:check` | ✅ PASS |
| PowerShell acceptance script syntax | ✅ PASS |
| `packages/vault-core` `cargo clippy --all-targets -- -D warnings` | ✅ PASS |

### Honest Acceptance Verdict

- **Code-level blockers on this host:** CLEARED.
- **Build gates on this host:** ALL GREEN.
- **Remaining blockers are environmental:**
  1. Non-WDAC Windows host for desktop `cargo test`, `cargo build`, and `test-llama-server.ps1` runtime.
  2. Physical Android device or USB-passthrough emulator for `UsbVaultRepositoryInstrumentedTest`.
  3. Live end-to-end model inference and Android ↔ Desktop vault round-trip.

**Branches:** `remediation/p0-desktop-runtime` (P0-A) and `remediation/p0-mobile-usb-vault` (P0-B/C). Neither has been merged or pushed; both are awaiting environmental acceptance gates. See `docs/P0_DESKTOP_NON_WDAC_RUNBOOK.md` and `docs/P0_ANDROID_HARDWARE_RUNBOOK.md` for exact next-step commands.
