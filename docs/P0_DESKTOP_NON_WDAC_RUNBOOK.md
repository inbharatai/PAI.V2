# P0-A Desktop Acceptance Runbook — Non-WDAC Windows Host

**Scope:** Run the remaining desktop-only acceptance gates that cannot be executed on the audit host because WDAC/AppLocker blocks unsigned native executables and unsigned Rust proc-macro DLLs.

**Prerequisites**

- Windows 10/11 machine **without** an active WDAC/AppLocker policy blocking unsigned executables, **or** a machine where the UnoOne signing certificate has been deployed to `TrustedPublisher`/`CodeIntegrity` policy.
- USB vault inserted with folder structure:
  ```
  <Drive>:\UNOONE\
  ├── manifest.json
  └── models\
      └── gemma-4-12b-Q4_K_M.gguf   (≈ 7.14 GiB)
  ```
- `llama-server.exe` and `llama-server-impl.dll` present in `apps/desktop/src-tauri/llama/` (or wherever the desktop app expects the backend binary).
- Rust toolchain installed (`cargo`, `rustc`).
- PowerShell 5.1 or PowerShell 7.

---

## Step 1 — Build the Desktop Rust Backend

From the repo root:

```powershell
cd C:\Path\To\UnoOne-PAI
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Expected: all unit tests pass, including the 7 mockito-based `llama.rs` failure-path tests.

If the build fails with a WDAC-like error (`os error 4551`, CodeIntegrity Event IDs 3077/3033), the host is still enforcing a blocking policy — move to a different machine or update the WDAC policy to allow the UnoOne signer.

---

## Step 2 — Build the Tauri App

```powershell
cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Expected: `unoone-power.exe` (or equivalent Tauri binary) produced in `target/debug/` (or `target/release/` if using `--release`).

---

## Step 3 — Run the PowerShell Acceptance Script

The script covers all 19 desktop acceptance items and emits a JSON report.

```powershell
powershell -ExecutionPolicy Bypass -File apps/desktop/scripts/test-llama-server.ps1
```

What it verifies:

1. USB vault `UNOONE` folder is reachable.
2. `manifest.json` is present and parses correctly.
3. Model file `models/gemma-4-12b-Q4_K_M.gguf` exists.
4. Model SHA-256 matches the manifest entry.
5. A dynamic free TCP port is selected.
6. `llama-server.exe` starts on that port with `--model` pointing at the USB model.
7. `/health` returns `status: ok` and `model_loaded: true`.
8. `/v1/models` returns the expected model id.
9. Server identity matches the manifest SHA-256.
10. `/v1/chat/completions` answers a tiny smoke prompt.
11. Process shutdown is clean (no orphan `llama-server.exe`).
12–19. JSON report is written to `apps/desktop/scripts/llama-acceptance-report.json` with per-item status.

Expected final output: `ACCEPTANCE: PASSED (19/19)` and a JSON report.

---

## Step 4 — Manual End-to-End Smoke (Optional but Recommended)

1. Launch the Tauri desktop app.
2. Navigate to the **Vault** view.
3. Unlock the vault with the test passphrase.
4. Write a small text memory and confirm it persists after restart.
5. If an Android device with the vault is available, write on one platform and read on the other (see `P0_ANDROID_HARDWARE_RUNBOOK.md`).

---

## Step 5 — Report Results

If all steps pass, update `docs/EVIDENCE_AUDIT.md` §14:

- Change `Desktop cargo test --no-run` to `✅ PASS`.
- Change `test-llama-server.ps1 runtime` to `✅ PASS`.
- Attach the generated `llama-acceptance-report.json` to the audit evidence.

Only after all environmental gates pass should `remediation/p0-desktop-runtime` be merged to `main`.
