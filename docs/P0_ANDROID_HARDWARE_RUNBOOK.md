# P0-B/C Android Acceptance Runbook — Physical Device or USB-Passthrough Emulator

**Scope:** Run the Android instrumentation tests and live cross-platform vault round-trip that cannot be executed on the audit host without a physical Android device or an emulator with USB mass-storage passthrough.

**Prerequisites**

- Android physical device with USB OTG support **or** Android Studio emulator configured for USB device passthrough.
- USB vault inserted with folder structure:
  ```
  <Drive>:\UNOONE\
  ├── manifest.json
  └── models\
      └── gemma-4-12b-Q4_K_M.gguf   (≈ 7.14 GiB)
  ```
- Android SDK and ADB installed.
- Repo checked out at `remediation/p0-mobile-usb-vault`.

---

## Step 1 — Physical Device Setup

1. Enable **Developer options** and **USB debugging** on the Android device.
2. Connect the device to the host via USB.
3. Verify ADB visibility:
   ```bash
   adb devices
   ```
4. Connect the USB vault to the Android device via an OTG adapter.

## Step 2 — Emulator with USB Passthrough Setup

1. Start the emulator from Android Studio with a system image that supports USB host.
2. Open **Extended Controls** (⋯) → **USB** → attach the physical USB vault device.
3. Inside the emulator, grant the UnoOne app permission to access the USB storage.

---

## Step 3 — Run Instrumentation Tests

From `android-app/UnoOneAgent`:

```bash
./gradlew :vault:connectedAndroidTest
```

This executes `UsbVaultRepositoryInstrumentedTest` and verifies:

- USB vault `UNOONE` folder is detected.
- Android can read the manifest and model hash.
- Android can unlock the vault with the same passphrase as Desktop.
- Records can be written to the encrypted vault in AES-256-GCM format.
- Records written by Android are readable by Android after re-inserting the USB vault.

Expected: `BUILD SUCCESSFUL` with all instrumentation tests passing.

---

## Step 4 — Cross-Platform Round-Trip Test

This requires both a non-WDAC Windows host (see `P0_DESKTOP_NON_WDAC_RUNBOOK.md`) and the Android device/emulator.

### 4a — Android writes, Desktop reads

1. On Android, open the UnoOne app, unlock the vault, and create a memory with a known string (e.g., `cross-platform sync test 2026-07-26`).
2. Safely eject the USB vault from Android.
3. Insert the USB vault into the Windows host.
4. Run the desktop app, unlock the vault, and verify the memory appears.

### 4b — Desktop writes, Android reads

1. On Desktop, create a memory with a known string.
2. Safely eject the USB vault.
3. Insert it back into Android and verify the memory appears.

Expected: both directions work, proving AES-256-GCM + HKDF domain keys are identical across platforms.

---

## Step 5 — Privacy Logging Runtime Check

1. Perform actions that would normally log sensitive data (e.g., enter an Aadhaar number, credit card, or email in a form).
2. Collect logs via ADB:
   ```bash
   adb logcat -d | grep -i unoone
   ```
3. Verify sensitive values are replaced with markers such as `[REDACTED:aadhaar]`, `[REDACTED:card]`, `[REDACTED:email]` and the raw values do not appear.

---

## Step 6 — Report Results

If all steps pass, update `docs/EVIDENCE_AUDIT.md` §14:

- Change `UsbVaultRepositoryInstrumentedTest` to `✅ PASS`.
- Add a note that the Android ↔ Desktop vault round-trip was verified.
- Attach `android-app/UnoOneAgent/vault/build/outputs/connected/androidTest/...` test results to the audit evidence.

Only after all environmental gates pass should `remediation/p0-mobile-usb-vault` be merged to `main`.
