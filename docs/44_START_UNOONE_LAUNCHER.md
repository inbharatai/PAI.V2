# 44 — Start UnoOne Launcher

Status: `IMPLEMENTED_NOT_TESTED`

`apps/starter/windows` builds the release-candidate `Start UnoOne.exe` that belongs
at the root of the Pocket AI pen drive.

Behavior:

1. derives the Pocket AI root from its own executable path;
2. loads the shared schema-v2 manifest;
3. validates product/version/vault identity and every required Windows launch
   asset;
4. refuses modified or incomplete packages;
5. launches only the validated manifest-declared UnoOne Power executable;
6. passes `--vault-root` and `--launched-by-starter`;
7. offers “Enable automatic launch on this computer”;
8. invokes the validated UnoOne Dock `--install` only after confirmation.

It does not install an `autorun.inf` and does not claim that Windows permits raw
removable-drive AutoRun.

`scripts/build-pocket-ai-windows.ps1` stages the launcher, Dock, and Power
binaries only after backing up replaced USB files under
`RECOVERY\package-backups\<UTC timestamp>`.

Native compilation and UI/runtime verification are blocked by the same WDAC
policy described in document 43.
