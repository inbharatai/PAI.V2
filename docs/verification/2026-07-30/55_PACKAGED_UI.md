# 55 — Packaged UI (launched from the physical drive)

**Status: VERIFIED_WORKING** (with one disclosed limitation below).

## Evidence

1. Launched `D:\UNOONE\APPS\WINDOWS\UnoOnePower.exe` (sha256
   `0401DAC2291DA644F769CC9A87E2E4D0BEDBA7A6620A192D10B3531B3C2A10EE`, the exact
   manifest-declared binary) via `Start-Process -PassThru`.
2. After 12 s: process **alive** (`UnoOnePower`, pid 39592), `MainWindowTitle =
   "UnoOne Power"`.
3. `Get-NetTCPConnection -OwningProcess 39592`: **no connection to :5173**;
   `Get-NetTCPConnection -State Listen -LocalPort 5173`: nothing listening (no
   Vite dev server involved).
4. Embedding gate against this exact binary: VERIFIED_WORKING for both hashed
   Vite assets (`index-DJEWx2Le.js`, `index-C0duoPfV.css`). The renderer serves
   those embedded bytes over the custom `tauri://localhost` protocol, and the
   config's `frontendDist` points at the built `dist` — dev URL is inert
   config metadata (documented in the gate script).

## Disclosed limitation

Window *pixel content* is not machine-verifiable from this harness (no screen
capture review path), and no screenshot file is attached; the evidence above is
process/network/byte-level. A human eyeball pass on the rendered window remains
a standing human gate. What is PROVEN here is: the binary contains the frontend,
the app window opens from the packaged binary on the drive, and it cannot be
showing "localhost refused to connect" because it never contacts a dev server
and its assets are byte-verified present.

## Also proven this session

- `D:\UNOONE\Start UnoOne.exe --verify-only` → exit **0**, output
  `{"failure_count": 0, "failures": [], "valid": true}` (after restage).
- First staging attempt correctly **rolled back** when native verify failed —
  the transactional protocol behaves as designed.
