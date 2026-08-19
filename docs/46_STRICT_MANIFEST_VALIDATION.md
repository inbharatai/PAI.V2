# 46 — Strict Manifest Validation

Status: `VERIFIED_WORKING`

`packages/usb-manifest` is the single validator used by Dock, Starter, and
UnoOne Power.

Schema v2 verifies:

- exact product ID and schema version;
- PAI `VERSION`;
- Windows platform and architecture;
- vault ID path, optional expected ID, and hash;
- desktop, Dock, and Starter paths, sizes, and SHA-256;
- every required runtime executable and DLL;
- model and mmproj sizes and SHA-256;
- declared Whisper/Piper runtime/model assets;
- non-empty relative paths only;
- no `..`, absolute path, symlink, junction, reparse point, or canonical escape.

Failures are returned as structured codes and paths. A modified executable is
never launched.

`scripts/New-UnoOneManifestV2.ps1` generates the schema from physical staged
files and includes both Windows assets and mobile E2B/E4B files. `-Apply` first
backs up the old manifest. `scripts/verify-p1-desktop-usb-assets.ps1 -Strict`
now rejects legacy manifests.

The validator crate passes fmt/check/test/clippy on Windows and macOS in
Desktop CI run `30437908042`. The Windows bundle was built in run
`30437957332`.

A synthetic schema-v2 Pocket AI containing Power, Dock, Starter, one runtime,
one desktop model, one mobile model, VERSION, and vault ID was generated and
then verified with `-Strict`: 5/5 Windows launch checks passed.

The physical `D:\UNOONE` package was then verified independently on
2026-07-29: all 545 declared assets passed, and the native Starter verifier
exited `0`. A separate 381-voice-asset regression manifest also passed after
the SHA-256 read buffer was moved from the Windows GUI thread stack to the
heap.

The schema provides hash integrity, not a cryptographic signature. Signed
manifests and Authenticode verification remain future production hardening.
