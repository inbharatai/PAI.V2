# 42 — Current USB and GitHub Main Audit

Date: 2026-07-29
Repository: `https://github.com/inbharatai/PAI`
Physical Pocket AI: `D:\UNOONE`

## Git evidence

- remote default branch: `main`;
- remote `main`: `cb9859fa09e53b609518e09fa3011e7ce05a062e`;
- remote heads returned by `git ls-remote --heads`: `main` only;
- isolated source checkout: clean at the same full SHA before remediation;
- original `%USERPROFILE%\Documents\New project` repository: unrelated,
  unborn `master`, no commits, multiple untracked files;
- the physical Pocket AI has no `.git` directory.

The source copy on the pen drive is not current GitHub `main`. Comparing all 579
tracked files:

| Result | Count |
|---|---:|
| Same | 480 |
| Different | 70 |
| Missing on USB | 29 |

The 29 missing files include the P1 USB verifier/build scripts, desktop voice
and agent modules, current P1 documentation, and frontend/backend integration
files. The editable repository must not be maintained inside the runtime pen
drive.

## Physical device evidence

| Field | Value |
|---|---|
| Disk | `USB SanDisk 3.2Gen1` |
| Serial | `00013420082725070348` |
| USB VID/PID | `0781:55A9` |
| Bus | USB |
| Partition | GPT |
| Filesystem | exFAT |
| Volume label | `UNOONE` |
| Volume size | 494,163,460,096 bytes |
| Free space | 473,829,212,160 bytes |
| Health | Healthy |

## Complete inventory evidence

- files: 13,140;
- directories: 6,222;
- file bytes: 17,958,595,785;
- asset hashes recorded: 1,719 files / 15,715,788,924 bytes;
- complete inventory:
  `%USERPROFILE%\Documents\New project\PAI-audit-raw\usb-complete-inventory.csv`;
- SHA-256 evidence:
  `%USERPROFILE%\Documents\New project\PAI-audit-raw\usb-asset-sha256.csv`;
- GitHub comparison:
  `%USERPROFILE%\Documents\New project\PAI-audit-raw\github-main-to-usb-tracked-files.csv`.

Largest areas:

| Area | Files | Bytes |
|---|---:|---:|
| `MODELS` | 4 | 14,032,241,376 |
| `android-app` | 7,428 | 1,328,632,213 |
| `RUNTIMES` | 158 | 1,299,044,896 |
| `target` | 3,408 | 1,043,813,598 |
| `APPS` | 1,443 | 249,279,249 |

This confirms that the current pen drive contains editable source and build
trees rather than a clean user-facing runtime package.

## Key physical assets

| Asset | Evidence |
|---|---|
| `APPS\WINDOWS\UnoOnePower.exe` | Missing |
| `Start UnoOne.exe` | Missing |
| `RUNTIMES\WINDOWS\VOICE` | Missing |
| Whisper runtime/model | Missing |
| Piper runtime/model | Missing |
| CPU runtime | 51 files / 46,262,112 bytes |
| CUDA runtime | 55 files / 1,155,882,336 bytes |
| Vulkan runtime | 52 files / 96,900,448 bytes |
| Gemma 12B | Present |
| mmproj | Present |
| `manifest.json` | Present, legacy schema |
| `VERSION` | `0.5.0-alpha` |
| `vault.id` | `[REDACTED-VAULT-ID]` |

Cryptographic evidence:

| File | Bytes | SHA-256 |
|---|---:|---|
| Gemma 12B GGUF | 7,662,531,872 | `D333B368BE6CD655563FCE18AEDE26027E208FDB13816D35EB06983CE054044B` |
| mmproj GGUF | 122,031,552 | `563192209F002B0A13AF16A4992FDB9DD61187A36919EAF65F408BB47AF3D272` |
| CPU/CUDA/Vulkan `llama-server.exe` | 9,216 each | `CB2F539E1B430B2730E2A7E5E9B4B713A890E94BC5450049A5FEA7F6076BF9FF` |
| legacy `manifest.json` | 3,856 | `68E76F8E653088B972DB8BE294A57B9BFAF628A3E43EAF540739BEC6F0224398` |

## Verification result

The repository verifier was run against the physical Pocket AI. Before its
schema-v2 remediation it reported 10 declared assets OK and the desktop app
missing. With strict schema-v2 enforcement it reports:

- legacy manifest rejected;
- desktop app missing;
- exit code `1`.

Historical audit status before remediation: `USB_RUNTIME_PACKAGE: BLOCKED`

## Post-remediation physical result

The historical evidence above is retained to show the exact starting state.
On 2026-07-29 the physical device was staged transactionally from Windows
bundle CI run `30437957332` at commit
`5f710c7602b5a8fa6260184ace2a15d36525dc86`.

| Gate | Result |
|---|---|
| Schema | `2` |
| Product ID | `com.inbharatai.unoone.pocket-ai` |
| Package version | `0.5.0-alpha` |
| Strict PowerShell verification | 545/545 checks passed, exit `0` |
| Native Starter verification | exit `0` |
| Runtime assets | 158 |
| Desktop model assets | 2 |
| Voice assets | 381 |
| Mobile model assets | 2 |
| Final `manifest.json` | 214,366 bytes; SHA-256 `FCBB143C61E0D1D46A4BA35AD6CB554B2CCAF876AC6622A3EA313AE8A24A8B00` |
| Recovery copy | `RECOVERY\package-backups\20260729-091422\manifest.json` |

Final physical inventory after remediation:

- files: 13,531;
- directories: 6,270;
- file bytes: 18,233,374,779;
- free space: 473,503,367,168 bytes.

Status: `USB_RUNTIME_PACKAGE: VERIFIED_WORKING`

This status means the physical package and every declared byte passed both
validators. It does not imply Authenticode signing, manifest signatures, or
completion of live UI/model/recording acceptance on every Windows policy.
