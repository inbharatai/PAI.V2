# 54 — Live Pocket AI Audit (re-audit of D:\UNOONE, 2026-07-30)

**Status: VERIFIED_WORKING** for the physical facts below. All claims here were
observed during this session, not carried over from earlier documents.

## Physical identity

| Fact | Value | Source |
|---|---|---|
| Drive letter / root | `D:\UNOONE` | `Get-Volume` |
| Volume label / FS / type / health | UNOONE / exFAT / Removable / Healthy | `Get-Volume` |
| Size / free | 494,163,460,096 B (460 GiB) / 473,110,806,528 B at audit start | `Get-Volume` |
| Partition | Disk 1, partition 1, 494,180,237,312 B, Basic | `Get-Partition` |
| Disk | `USB SanDisk 3.2Gen1`, bus USB, serial `00013420082725070348`, GPT | `Get-Disk` |
| `VERSION` vs manifest | both `0.5.0-alpha`, schema_version 2, product `com.inbharatai.unoone.pocket-ai` | validator |
| vault.id fingerprint | sha256 `[REDACTED-VAULT-ID-SHA256]`, **20 bytes** (hash + size only — contents never dumped) | `Get-FileHash` |

## Binary hashes verified two ways

545/545 manifest checks OK, `verify-p1-desktop-usb-assets.ps1 -VaultRoot D:\UNOONE -Strict`
exit code **0** (run twice during this session — once before restaging, once after).

After restaging (build `fd5a3fb`-era binaries, staged via `scripts/build-pocket-ai-windows.ps1`
with full backup/rollback protocol — backups in `D:\UNOONE\RECOVERY\package-backups\20260730-115115`
and `…\20260730-115407`):

| File | SHA-256 (manifest == drive) |
|---|---|
| `APPS/WINDOWS/UnoOnePower.exe` | `0401DAC2291DA644F769CC9A87E2E4D0BEDBA7A6620A192D10B3531B3C2A10EE` (13,915,648 B) |
| `APPS/WINDOWS/UnoOneDock.exe` | `65198279DC9A810D0F055448BE1D30376FF0172952D6B9A49C024E06E3A89CB3` (unchanged) |
| `Start UnoOne.exe` | `C49612CD1B7F48F5168CD64D3BC4FFFCC4BCC40870D33DAEF18448FADCD9CDA3` (unchanged) |
| `MODELS/DESKTOP/Gemma-12B/gemma-4-12B-it-Q4_K_M.gguf` | `D333B368BE6CD655…` per manifest (7,662,531,872 B) |
| `MODELS/DESKTOP/Gemma-12B/mmproj-gemma-4-12B-it-f16.gguf` | `563192209F002B0A…` per manifest (122,031,552 B) |
| Runtimes | 158 assets, all hash-verified (CPU + CUDA + VULKAN llama.cpp builds) |
| Voice | 381 assets (Whisper + Piper), all hash-verified |

Full per-file CSV was hashed by the recovery collector; the strict validator's own
545-entry pass is the authoritative byte-level diff against `manifest.json`.

## Embedding evidence (the failure class that bit us before)

- Byte-scan gate `scripts/verify-frontend-embedded.mjs` against the exact staged
  `D:\UNOONE\APPS\WINDOWS\UnoOnePower.exe`: **VERIFIED_WORKING**, assets found
  `index-DJEWx2Le.js` + `index-C0duoPfV.css`, none missing.
- Packaged launch: process alive 12 s+, main window "UnoOne Power", **zero TCP
  connections to :5173**, nothing listening on :5173. See 55.
- Root cause of the historic broken staging is now fixed at the
  Cargo-feature level: `tauri-macros` embeds **zero** assets unless the crate
  enables `tauri/custom-protocol`; it is now a default feature of unoone-power.

## Honesty notes

- An earlier "545/545, exFAT, 0.5.0-alpha" summary in project notes was stale:
  before restaging the drive carried desktop exe `827E872F…` with an *older*
  embedded frontend (`index-Bs9aevbq.js`).
- The first restaging attempt **failed and rolled back** (native verify-only
  exit 1) because `New-UnoOneManifestV2.ps1` wrote a UTF-8 BOM the Rust
  verifier rejects. Fixed (commit `fd5a3fb`, ported from k3 acceptance branch)
  and restaged successfully — both verification layers passed, no rollback.
