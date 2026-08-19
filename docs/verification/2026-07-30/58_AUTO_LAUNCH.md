# 58 — Auto-launch (Dock)

**Status: BLOCKED_BY_ENVIRONMENT** (interactive + physically destructive USB
hot-plug journey; unattended session).

What exists and is verifiable statically:
- `UnoOneDock.exe` staged and hash-verified (`65198279…`), single-instance
  plugin present in the app (tauri-plugin-single-instance 2.4.3).

Journey owed (human): install Dock for current user → verify
`%LOCALAPPDATA%\UnoOne\Dock\UnoOneDock.exe` + HKCU Run entry → unplug +
reinsert the drive → device event → strict validation → Power launches once →
existing instance focused (not duplicated) → correct dynamic vault root → UI
opens → removal cleanup (inference stop, recording stop+buffer discard, vault
lock, temp clear, disconnected UI, no orphan process) → reconnect.

These require physical USB cycling and interactive consent; not claimed.
