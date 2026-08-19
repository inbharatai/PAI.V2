# 52 — Pocket AI Physical Release Evidence

Date: 2026-07-29

Physical product: `D:\UNOONE`

Volume label: `UNOONE`

Filesystem: exFAT

Package version: `0.5.0-alpha`

## Decision

`USB_RUNTIME_PACKAGE: VERIFIED_WORKING`

Pocket AI is the physical pen drive. Windows, Dock, Starter, and Android are
host interfaces for the models, runtimes, identity, and encrypted vault on
that device. The host filesystem is not the canonical product copy.

The current physical package passed two independent verification layers:

- strict PowerShell verifier: 545/545 checks, exit `0`;
- native `Start UnoOne.exe --verify-only`: exit `0`.

This decision covers package structure and byte integrity. It does not claim
that the unsigned executables will run under every WDAC/AppLocker policy, that
the manifest is cryptographically signed, or that every desktop UX feature has
completed prepared-host acceptance.

## Source and CI evidence

| Evidence | Result |
|---|---|
| Windows source commit | `5f710c7602b5a8fa6260184ace2a15d36525dc86` |
| Windows bundle workflow | `30437957332` — passed |
| Desktop CI workflow | `30437908042` — passed |
| Windows bundle contents | Power, Dock, Starter, `SHA256SUMS.txt` |
| Voice smoke | Real Whisper transcript plus valid 93,344-byte Piper WAV |
| Large-manifest regression | 381 voice assets; native Starter exit `0` |
| Mobile protection | 344 blobs match `mobile-golden-baseline-v2` |

The implementation and this evidence were fast-forwarded to `main` at
`5e8c722895cce088b27bde71dc043ea2fb9aeaa5`. Exact-commit main checks passed:

| Main workflow | Run |
|---|---|
| Mobile Protection | `30439438831` |
| Android CI | `30439439139` |
| Desktop CI | `30439439144` |
| Pocket AI Windows Bundle | `30439439032` |

The large-manifest regression specifically proves the Windows stack-overflow
fix. The SHA-256 validator formerly allocated a 1 MiB buffer on the Windows GUI
thread stack; it now allocates that buffer on the heap.

## Final manifest

| Field | Value |
|---|---|
| Product ID | `com.inbharatai.unoone.pocket-ai` |
| Schema | `2` |
| Bytes | 214,366 |
| SHA-256 | `FCBB143C61E0D1D46A4BA35AD6CB554B2CCAF876AC6622A3EA313AE8A24A8B00` |
| Windows runtimes | 158 |
| Desktop Gemma/mmproj models | 2 |
| Voice assets | 381 |
| Mobile models | 2 |

The schema rejects wrong product/version/architecture, missing or duplicate
required assets, path traversal, absolute paths, symlinks/reparse points,
vault identity mismatch, size mismatch, and SHA-256 mismatch.

## Windows applications

| File | Bytes | SHA-256 |
|---|---:|---|
| `APPS\WINDOWS\UnoOnePower.exe` | 13,361,664 | `FF9D853265DCD4492765C3D3BF8464EC226213BEF3285823F32A7D94102EE491` |
| `APPS\WINDOWS\UnoOneDock.exe` | 523,776 | `A34EAA8BD90A722A039622B84CF3AE14B394A579E37CB30EA62FCEFB3A87404E` |
| `Start UnoOne.exe` | 492,544 | `F08A931327C0DDEA1F89B9F9CE2688E115D20B3B67FDD58F633129D14028100C` |

## Model evidence

| File | Bytes | SHA-256 |
|---|---:|---|
| `MODELS\DESKTOP\Gemma-12B\gemma-4-12B-it-Q4_K_M.gguf` | 7,662,531,872 | `D333B368BE6CD655563FCE18AEDE26027E208FDB13816D35EB06983CE054044B` |
| `MODELS\DESKTOP\Gemma-12B\mmproj-gemma-4-12B-it-f16.gguf` | 122,031,552 | `563192209F002B0A13AF16A4992FDB9DD61187A36919EAF65F408BB47AF3D272` |
| `MODELS\DESKTOP\whisper-base.en.bin` | 147,964,211 | `A03779C86DF3323075F5E796CB2CE5029F00EC8869EEE3FDFB897AFE36C6D002` |
| `MODELS\DESKTOP\voice.onnx` | 63,531,379 | `DC9CAA6C313199FFB5AC698B6E542FA6CBA388AEAF2731E25262E33B9810AEF1` |
| `MODELS\DESKTOP\voice.onnx.json` | 4,966 | `7CEB1BC4AF6D4E41B6D1EDBB86C67E91E01EAA71F66DB4CD0AE92AC704D415BE` |

Voice sources are pinned to:

- [whisper.cpp v1.9.1 Windows x64](https://github.com/ggml-org/whisper.cpp/releases/tag/v1.9.1);
- Whisper `base.en`;
- [Piper 2023.11.14-2 Windows AMD64](https://github.com/rhasspy/piper/releases/tag/2023.11.14-2);
- Piper `en_US-bryce-medium`, whose
  [model card](https://huggingface.co/rhasspy/piper-voices/blob/main/en/en_US/bryce/medium/MODEL_CARD)
  declares public-domain training data.

The Lessac voice was deliberately excluded because its
[upstream dataset terms](https://www.cstr.ed.ac.uk/projects/blizzard/2013/lessac_blizzard2013/license.html)
are not suitable for unrestricted commercial distribution.

## Transaction and recovery

The staging operation backed up existing targets, copied the full application
and voice payloads, generated schema v2, ran both validators, and would have
restored the previous state on any failure.

The immediately preceding manifest remains at:

`RECOVERY\package-backups\20260729-091422\manifest.json`

Its SHA-256 is:

`68E76F8E653088B972DB8BE294A57B9BFAF628A3E43EAF540739BEC6F0224398`

Final physical inventory:

| Field | Value |
|---|---:|
| Files | 13,531 |
| Directories | 6,270 |
| File bytes | 18,233,374,779 |
| Volume bytes | 494,163,460,096 |
| Free bytes | 473,503,367,168 |

## Remaining production gates

- Authenticode-sign Windows executables and runtime DLLs.
- Add cryptographic manifest/catalogue signing; SHA-256 fields alone provide
  integrity checking, not publisher authenticity.
- Test insertion, Dock auto-launch, removal cleanup, unlock, direct Gemma
  inference, microphone/Whisper, Piper playback, OCR, recording privacy, and
  browser work on a prepared Windows host.
- Test Android USB-C attach, Storage Access Framework selection, validation,
  and cross-platform encrypted-vault round trip on physical phones.
- Build and test the macOS host.

No Windows security policy was weakened to obtain these results.
