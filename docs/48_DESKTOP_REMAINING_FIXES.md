# 48 — Desktop Remaining Fixes

## Implemented in this remediation

- strict canonical Pocket AI validation;
- Dock/Starter/Power trust-rule sharing;
- automatic USB detection and disconnect cleanup;
- managed direct `llama-server` only;
- automatic model selection/start/identity/health gate;
- mmproj path read from the strict manifest;
- no Ollama/LM Studio fallback;
- USB-only Whisper/Piper discovery (no host PATH fallback);
- fabricated Whisper/OCR/vision confidence values removed (`null` when the
  backend supplies no calibrated value);
- real Windows camera device enumeration replaces the placeholder frame;
- settings continue to persist model configuration in the vault.

## Still not release-verified

| Area | Status | Reason |
|---|---|---|
| Direct Gemma 12B inference | `BLOCKED_BY_ENVIRONMENT` | Rust/Tauri build and unsigned runtime blocked by WDAC |
| mmproj OCR/description | `BLOCKED_BY_ENVIRONMENT` | Requires built app and live model |
| Whisper/Piper package | `VERIFIED_WORKING` | Complete runtime/models/licenses are on Pocket AI; real Whisper and Piper inference passed Windows CI run `30437957332` |
| Whisper/Piper physical UX | `IMPLEMENTED_NOT_TESTED` | On-drive hashes pass, but microphone capture, audible playback, and UI flow still require a prepared-host test |
| Recording → STT → summary | `PARTIALLY_IMPLEMENTED` | Capture exists; end-to-end pipeline and privacy-mode retention still require implementation/runtime proof |
| Browser Workspace | `IMPLEMENTED_NOT_TESTED` | Source wiring exists; live WebView session not physically verified |
| Screenshots/camera capture | `PARTIALLY_IMPLEMENTED` | Camera device enumeration is real; capture remains WebView/getUserMedia |
| Agent tool parsing | `PARTIALLY_IMPLEMENTED` | Structured path exists; heuristic fallback remains |
| Shutdown/orphan cleanup | `IMPLEMENTED_NOT_TESTED` | Source cleanup added; process behavior requires runtime proof |
| Signing/WDAC | `BLOCKED_BY_ENVIRONMENT` | No code-signing certificate/build host supplied |

The physical package itself passed 545/545 strict checks and native Starter
verification. No compilation-only result is labeled `VERIFIED_WORKING`, and
package integrity must not be confused with Authenticode signing or live
feature acceptance.
