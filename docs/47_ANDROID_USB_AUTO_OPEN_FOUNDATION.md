# 47 — Android USB Auto-Open Foundation

Date: 2026-07-29  
Branch: `codex/android-usb-auto-open`  
Base: `cb9859fa09e53b609518e09fa3011e7ce05a062e`

## Decision

`ANDROID_AUTO_OPEN_SOURCE: READY_FOR_DEVICE_TEST`

The source foundation is implemented in the existing UnoOne Mobile application.
No companion Android application or replacement interface was created.

## Physical prototype identity

Windows inspection of the connected Pocket AI prototype reported:

- disk: `USB SanDisk 3.2Gen1`;
- serial: `00013420082725070348`;
- USB storage parent: `VID_0781&PID_55A9`;
- Android device filter: vendor `1921` (`0x0781`), product `21929` (`0x55A9`).

VID/PID is used only to route the attachment to UnoOne. It is not trusted as
Pocket AI identity. The selected storage tree must also pass schema-v2 product,
VERSION, and vault-ID validation.

## Implemented

- `USB_DEVICE_ATTACHED` and `USB_DEVICE_DETACHED` handling in `MainActivity`;
- prototype device-filter metadata;
- `singleTask` `onNewIntent` handling;
- explicit Android USB permission request;
- package-scoped permission callback;
- Storage Access Framework `OpenDocumentTree` fallback for mass storage;
- persisted read permission where the document provider supports it;
- truthful persistent Compose status for access required, validation,
  connected, invalid, and disconnected states;
- a real `:vault` Android library module;
- canonical `UNOONE/manifest.json`, `VERSION`, and
  `VAULT/identity/vault.id` validation;
- ordinary/unsupported USB rejection without opening content.

## Verification

Command:

```powershell
$env:ANDROID_HOME="$env:LOCALAPPDATA\Android\Sdk"
.\gradlew.bat :vault:testDebugUnitTest :app:compileDebugKotlin
```

Result: `BUILD SUCCESSFUL`

- vault identity unit tests: passed;
- new vault module debug compilation: passed;
- existing app with USB integration: passed Kotlin compilation.

Command:

```powershell
.\gradlew.bat :vault:lintDebug :app:lintDebug
```

Result: `BUILD SUCCESSFUL`; app lint reported no new issues.

## Hardware tests still required

- Android presents UnoOne for the physical USB-C attachment;
- user association/default-app behavior on the target phone;
- USB permission callback on the target Android build;
- document provider exposes the mass-storage volume;
- persisted tree URI survives process restart and reinsertion;
- detach event arrives during model load and active interaction;
- E2B/E4B model selection reads the canonical Pocket AI tree;
- invalid manifest and mismatched vault ID are rejected on-device.

No physical-phone claim is made from compilation or unit tests.
