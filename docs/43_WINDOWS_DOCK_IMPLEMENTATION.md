# 43 — Windows UnoOne Dock Implementation

Decision: `WINDOWS_DOCK_SOURCE: MERGE_BLOCKED`

Source exists at `apps/dock/windows` as a small native Rust/Win32 application.
It is a host bridge for the Pocket AI pen drive, not a third product.

Implemented:

- per-user single instance through a named mutex;
- current-user install/uninstall using `%LOCALAPPDATA%\UnoOne\Dock` and
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`;
- hidden top-level Win32 message window;
- deterministic `WM_DEVICECHANGE` arrival/removal handling;
- removable-volume enumeration;
- support for either a volume-root package or `volume\UNOONE`;
- shared strict schema-v2 validation before launch;
- executable, runtime, DLL, model, mmproj, version, and vault-ID verification;
- rejection of absolute paths, traversal, symlinks, junctions, and reparse
  points;
- launch only of manifest-declared `UnoOnePower.exe`;
- `--vault-root` and `--launched-by-dock` handoff;
- duplicate-app handoff through UnoOne Power's single-instance plugin;
- notification-area icon and valid/invalid/missing/launch/disconnect balloons;
- `--install`, `--uninstall`, and `--run-once`.

Removal is independently detected by UnoOne Power every two seconds. It stops
the managed model process, stops and zeroes active recording buffers, drops the
decrypted vault, clears identity state, and returns the UI to the locked
disconnected state.

Source formatting and Cargo metadata pass. Native compile and runtime tests are
`BLOCKED_BY_ENVIRONMENT`: this host's enforced Application Control policy
blocks newly generated Rust build-script executables with OS error 4551.

Merge remains blocked until a WDAC-allowed host compiles/signs the binaries and
the 20-case physical insertion suite passes.
