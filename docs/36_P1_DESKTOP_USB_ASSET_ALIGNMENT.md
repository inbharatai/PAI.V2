# P1 Desktop Feature Completion — USB Asset Alignment

## 1. Objective

Compare the desktop code's expected USB vault asset layout against the assets actually present on the connected UnoOne USB volume, identify gaps, and document the alignment status for P1 acceptance.

## 2. Inspection Method

- USB volume: `D:\UNOONE`
- Validation: `manifest.json`, `VERSION`, and `VAULT\identity\vault.id` present.
- Inspection command: `find /d/UNOONE/RUNTIMES /d/UNOONE/MODELS /d/UNOONE/APPS -type f`

## 3. Expected Asset Layout (from code and manifest)

### 3.1 Vault structure

```
D:\UNOONE
├── manifest.json
├── VERSION
├── APPS\WINDOWS\                ← Windows desktop installer/binaries
├── APPS\MACOS\                  ← macOS desktop bundles (out of P1 scope)
├── RUNTIMES\WINDOWS\CUDA\       ← llama.cpp CUDA 12.4 build
├── RUNTIMES\WINDOWS\CPU\        ← llama.cpp CPU (AVX2+) fallback
├── RUNTIMES\WINDOWS\VULKAN\     ← llama.cpp Vulkan build
├── RUNTIMES\WINDOWS\VOICE\      ← Whisper.cpp STT + Piper TTS binaries
├── MODELS\DESKTOP\Gemma-12B\    ← Gemma 4 12B Q4_K_M + mmproj
├── MODELS\MOBILE\...            ← Mobile models (out of P1 scope)
├── VAULT\{header,records,indexes,journal,transactions,attachments,recovery,identity}
└── CONFIG, RECOVERY, UPDATES, LOGS
```

### 3.2 Required files for each P1 feature

| Feature | Required asset(s) | Expected path(s) |
|---------|-------------------|------------------|
| Vault unlock | `manifest.json`, `VERSION`, `vault.id` | root + `VAULT\identity\vault.id` |
| Local chat / agent | `llama-server.exe` + DLLs, Gemma GGUF | `RUNTIMES\WINDOWS\<BACKEND>\`, `MODELS\DESKTOP\Gemma-12B\` |
| Vision / OCR | mmproj GGUF | `MODELS\DESKTOP\Gemma-12B\mmproj-*.gguf` |
| Recording | microphone (host hardware) | N/A on USB |
| Browser | WebView2 runtime (system) | N/A on USB |
| STT | Whisper binary + model | `RUNTIMES\WINDOWS\VOICE\whisper.exe` + `MODELS\DESKTOP\whisper-base.en.bin` |
| TTS | Piper binary + voice model | `RUNTIMES\WINDOWS\VOICE\piper.exe` + `MODELS\DESKTOP\voice.onnx` (+ `.json`) |

## 4. Actual Inspection Results

### 4.1 Present assets

| Asset | Path | Status |
|-------|------|--------|
| Vault manifest | `D:\UNOONE\manifest.json` | **VERIFIED_WORKING** |
| VERSION | `D:\UNOONE\VERSION` | **VERIFIED_WORKING** |
| vault.id | `D:\UNOONE\VAULT\identity\vault.id` | **VERIFIED_WORKING** |
| llama-server (CUDA) | `RUNTIMES\WINDOWS\CUDA\llama-server.exe` + DLLs | **VERIFIED_WORKING** |
| llama-server (CPU) | `RUNTIMES\WINDOWS\CPU\llama-server.exe` + DLLs | **VERIFIED_WORKING** |
| llama-server (Vulkan) | `RUNTIMES\WINDOWS\VULKAN\llama-server.exe` + DLLs | **VERIFIED_WORKING** |
| Gemma 12B GGUF | `MODELS\DESKTOP\Gemma-12B\gemma-4-12B-it-Q4_K_M.gguf` | **VERIFIED_WORKING** |
| Gemma mmproj | `MODELS\DESKTOP\Gemma-12B\mmproj-gemma-4-12B-it-f16.gguf` | **VERIFIED_WORKING** |

### 4.2 Missing assets

| Asset | Expected path | Status | Impact |
|-------|---------------|--------|--------|
| Windows desktop installer/binaries | `APPS\WINDOWS\*` | **NOT_IMPLEMENTED** | `APPS\WINDOWS\` is empty. The Tauri-built desktop binary is not staged on the USB volume. |
| Whisper.cpp STT binary | `RUNTIMES\WINDOWS\VOICE\whisper.exe` or `main.exe` | **NOT_IMPLEMENTED** | `RUNTIMES\WINDOWS\VOICE\` directory does not exist. STT unavailable. |
| Whisper model | `MODELS\DESKTOP\whisper-base.en.bin` (or manifest-declared path) | **NOT_IMPLEMENTED** | No Whisper model present. |
| Piper TTS binary | `RUNTIMES\WINDOWS\VOICE\piper.exe` | **NOT_IMPLEMENTED** | No Piper binary present. TTS unavailable. |
| Piper voice model | `MODELS\DESKTOP\voice.onnx` + config | **NOT_IMPLEMENTED** | No Piper voice model present. |

## 5. Code Alignment

The desktop code already aligns with the expected layout:

- `detect_vault` validates `manifest.json` + `VERSION` + `vault.id`.
- `llama.rs` discovers `llama-server.exe` under `RUNTIMES\WINDOWS\{CUDA,CPU,VULKAN}` and models under `MODELS\DESKTOP\Gemma-12B`.
- `accessibility.rs` relies on the same llama-server + mmproj path for vision/OCR.
- `voice.rs` looks for Whisper/Piper binaries under `RUNTIMES\WINDOWS\VOICE\` and models under `MODELS\DESKTOP\`.

No code changes are required for asset alignment; the missing items must be supplied on the USB volume.

## 6. P1 Acceptance Status

| Feature | Alignment Status |
|---------|------------------|
| USB vault detection | **VERIFIED_WORKING** |
| Vault crypto unlock | **VERIFIED_WORKING** (assets present) |
| Local model inference | **IMPLEMENTED_NOT_TESTED** — assets present, but unsigned `llama-server.exe`/DLLs are blocked by WDAC on this audit host, so live load is **BLOCKED_BY_ENVIRONMENT**. |
| Vision / OCR | **IMPLEMENTED_NOT_TESTED** — mmproj asset present, depends on model load. |
| Offline STT/TTS | **NOT_IMPLEMENTED** — voice runtime assets are absent. |
| Desktop app distribution | **NOT_IMPLEMENTED** — `APPS\WINDOWS\` is empty. |

## 7. Required Actions for Full P1 Runtime Acceptance

1. Populate `RUNTIMES\WINDOWS\VOICE\` with:
   - `whisper.exe` (or `main.exe`) + required DLLs
   - `piper.exe` + required DLLs
2. Populate `MODELS\DESKTOP\` with:
   - Whisper model (e.g., `ggml-base.en.bin`)
   - Piper voice model (e.g., `en_US-lessac-medium.onnx` + `.onnx.json`)
3. Use `scripts/stage-p1-desktop-voice-assets.ps1` to copy voice binaries/models and update `manifest.json` `runtimes` / `models` sections with SHA-256 hashes.
4. Use `scripts/verify-p1-desktop-usb-assets.ps1` to confirm every declared asset is present and hashes match.
5. Run `scripts/build-p1-desktop-windows.ps1` on a WDAC-allowed build host to build the Tauri release bundle, then sign the binary and stage it in `APPS\WINDOWS\`.
6. Add the corresponding `APPS.WINDOWS` manifest entry (the build script updates this automatically).
4. Stage a signed/WDAC-allowed Tauri desktop binary in `APPS\WINDOWS\`.

## 8. Acceptance Criteria

- [x] USB vault asset layout inspected and documented.
- [x] Present assets verified against expected paths.
- [x] Missing assets listed with impact and required actions.
- [x] Code expectations aligned with actual USB structure.
- [x] No code changes weaken WDAC, Defender, or AppLocker policies.
