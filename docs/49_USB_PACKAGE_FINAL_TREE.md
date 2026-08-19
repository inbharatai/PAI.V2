# 49 — Pocket AI Package Tree

Decision: `USB_RUNTIME_PACKAGE: VERIFIED_WORKING`

The physical pen drive is the Pocket AI product. The intended user-facing tree
is:

```text
UNOONE/
├── Start UnoOne.exe
├── manifest.json
├── VERSION
├── APPS/
│   └── WINDOWS/
│       ├── UnoOnePower.exe
│       └── UnoOneDock.exe
├── RUNTIMES/
│   └── WINDOWS/
│       ├── CPU/
│       ├── CUDA/
│       ├── VULKAN/
│       └── VOICE/
├── MODELS/
│   ├── MOBILE/
│   └── DESKTOP/
├── CONFIG/
├── VAULT/
├── LOGS/
├── RECOVERY/
└── UPDATES/
```

Physical result on 2026-07-29:

- `Start UnoOne.exe`, `UnoOnePower.exe`, and `UnoOneDock.exe` are staged;
- `RUNTIMES\WINDOWS\VOICE` contains the complete pinned Whisper.cpp and Piper
  runtime dependency trees plus notices;
- Whisper base.en and public-domain Bryce/Piper models are staged;
- `manifest.json` is canonical schema v2;
- all 545 declared assets passed size and SHA-256 verification;
- native `Start UnoOne.exe --verify-only` exited `0`;
- the previous manifest is recoverable from
  `RECOVERY\package-backups\20260729-091422`.

The physical drive still contains historical editable source/build trees.
They are not canonical runtime inputs and are not launched by Pocket AI.
Cleaning them is a separate data-retention decision and was not required to
make the manifest-declared product package valid.

Production boundary: the manifest and executables are hash-verified but not
cryptographically signed. Code signing and prepared-host UX/inference testing
remain open release gates.
