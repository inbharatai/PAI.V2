# P1 Desktop Feature Completion — Physical Acceptance Plan

## 1. Purpose

This plan defines the hands-on, live-device verification steps required to move the P1 desktop feature set from "build-verified" to "physically accepted" on a real Windows workstation with the UnoOne USB vault inserted.

## 2. Prerequisites

### 2.1 Host

- Windows 11 Pro/Enterprise or Home with WebView2 Runtime installed.
- A working microphone and webcam.
- WDAC/AppLocker profile configured for the target deployment (signing certificate or allow-list).

### 2.2 USB volume

- UnoOne USB vault inserted and detected as `D:\UNOONE` (or another removable drive).
- `manifest.json`, `VERSION`, and `VAULT\identity\vault.id` present.
- Stage the following missing assets before acceptance:
  - `RUNTIMES\WINDOWS\VOICE\whisper.exe` + Whisper model (e.g., `ggml-base.en.bin`)
  - `RUNTIMES\WINDOWS\VOICE\piper.exe` + voice model (e.g., `en_US-lessac-medium.onnx` + `.json`)
  - Signed/allowed Tauri desktop binary in `APPS\WINDOWS\`
- Use `scripts/stage-p1-desktop-voice-assets.ps1` to copy voice binaries/models and update `manifest.json` SHA-256 entries.
- Use `scripts/verify-p1-desktop-usb-assets.ps1` to confirm all declared assets are present and hashes match before starting acceptance.
- Use `scripts/build-p1-desktop-windows.ps1` on a WDAC-allowed build host to build and stage the signed desktop binary.

## 3. Acceptance Procedure

### 3.1 Vault detection and unlock

| Step | Action | Expected Result | Status |
|------|--------|-----------------|--------|
| 1 | Insert UnoOne USB vault. | Desktop app detects vault; vault status shows connected. | |
| 2 | Enter vault password. | Unlock succeeds; vault status shows unlocked. | |
| 3 | Open Capabilities view. | USB Vault shows `VERIFIED_WORKING`. | |

### 3.2 Real recording

| Step | Action | Expected Result | Status |
|------|--------|-----------------|--------|
| 4 | Navigate to Recordings view. | UI renders start/pause/stop controls. | |
| 5 | Click Start Recording. | Session state changes to `RECORDING`; timer increments. | |
| 6 | Speak into microphone for 10 seconds. | Audio buffer fills; no error. | |
| 7 | Click Stop Recording. | Session state changes to `STOPPED`; a WAV recording appears in the recent list with a `vault_record_id`. | |
| 8 | Verify the vault record exists. | Use vault memory/Documents view or backend record listing. | |

### 3.3 Browser workspace

| Step | Action | Expected Result | Status |
|------|--------|-----------------|--------|
| 9 | Navigate to Browser view. | UI renders URL bar and action toolbar. | |
| 10 | Click Start Session. | A second Tauri webview window opens. | |
| 11 | Enter `https://example.com` and click Navigate. | Webview loads the page. | |
| 12 | Click Extract Text. | Result log shows page text or JSON. | |
| 13 | Click Stop Session. | Webview window closes. | |

### 3.4 Vision / OCR / camera

| Step | Action | Expected Result | Status |
|------|--------|-----------------|--------|
| 14 | Navigate to Accessibility → Blind View. | Toggles are enabled. | |
| 15 | Enable Camera Blind Aid and click Start Camera. | Camera preview appears. | |
| 16 | Click Capture Snapshot. | A snapshot thumbnail appears. | |
| 17 | Provide a local image path containing text and click Run OCR. | OCR text appears in the result panel. | |
| 18 | Click Describe Image. | A description appears in the result panel. | |

### 3.5 Offline voice

| Step | Action | Expected Result | Status |
|------|--------|-----------------|--------|
| 19 | In Accessibility → Voice & Audio, select a TTS language and enter text. | Click Synthesize Speech; a WAV path and duration appear. | |
| 20 | Play the generated WAV file. | Audio is intelligible. | |
| 21 | Provide a WAV file path and click Transcribe Audio. | Transcribed text appears. | |

### 3.6 Capability profile refresh

| Step | Action | Expected Result | Status |
|------|--------|-----------------|--------|
| 22 | Navigate to Capabilities and click Refresh. | Each feature reports the correct status for the live environment. | |

## 4. Pass/Fail Criteria

- **Pass:** All 22 steps complete with the expected results and no unhandled errors.
- **Conditional Pass:** Steps blocked by host policy or missing USB assets are recorded with the exact blocker and a remediation ticket.
- **Fail:** Any crash, data loss, mock/fake result, or security policy weakening.

## 5. Sign-Off

| Role | Name | Date | Result |
|------|------|------|--------|
| Engineer | | | |
| QA / Tester | | | |
| Security Reviewer | | | |
| Product Owner | | | |

## 6. Notes

- Use synthetic test data for any automated regression tests; live acceptance must use real microphone/camera input.
- Do not disable WDAC, Defender, or AppLocker to pass acceptance. If a binary is blocked, fix the signing/allow-list, not the policy.
- macOS and Android are out of scope for this acceptance plan.
