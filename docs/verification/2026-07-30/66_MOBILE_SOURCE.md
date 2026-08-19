# 66 — Mobile source fixes (M1–M3)

**Status: VERIFIED_WORKING** for build+tests+lint (local gradle gate exit 0);
runtime behavior needs the physical phone (journey 67).

## M1 — dead USB discovery path → truthful detection

- `detectVault()` no longer scans `/storage` or `getExternalFilesDirs` on
  Android 11+ where scoped storage makes it impossible; it skips to a truthful
  reason and names the supported flow (`USB_DEVICE_ATTACHED` → tree picker →
  `takePersistableUriPermission`). Below API 29 direct scanning still runs.
- The meaningless `D:\UNOONE` probe was deleted.
- `VaultDetectionResult` gains `reason`, **required** (`init { require … }`)
  whenever `detected=false` — a silent detection failure cannot be constructed
  anymore.

## M2 — Room store cache semantics

Investigation answers:
- Encrypted at rest? **No** — `DatabaseProvider` builds Room without SQLCipher
  `SupportFactory`; it is plaintext SQLite in app-private storage.
- Eviction/TTL? **None existed.** — now `VaultCacheLifecycle.evictExpired`
  (24 h TTL default) runs at every app start; DAO gains `deleteOlderThan`.
- Cleared on disconnect? **It was not.** — now
  `clearOnVaultDisconnect` runs on `ACTION_USB_DEVICE_DETACHED` (MainActivity).
- Source of truth vs mirror? Notes/memories are mirrored to the Room store as
  if permanent. Now treated as bounded cache; `model_metadata` is device-local
  and excluded from eviction by design.

## M3 — permission truthfulness

| Permission | Verdict | Evidence |
|---|---|---|
| WAKE_LOCK | **REMOVED** | no `newWakeLock`/`acquire` anywhere |
| READ_CALENDAR | USED, kept | `phonecontrol/CalendarControl.kt` queries `CalendarContract.Instances` |
| SYSTEM_ALERT_WINDOW | USED, kept | `FloatingAgentService` overlay window |
| FOREGROUND_SERVICE_MEDIA_PROJECTION | USED, kept | `screenshot/MediaProjectionService` + manifest FGS type |
| REQUEST_IGNORE_BATTERY_OPTIMIZATIONS | USED, kept | `MainActivity.requestBatteryOptimizationExemption` |
| MODIFY_AUDIO_SETTINGS | USED, kept | voice/blind-aid audio mode calls |
| VIBRATE | USED, kept | `BlindAidManager` |
| FOREGROUND_SERVICE_SPECIAL_USE | USED, kept | manifest service `specialUse` (L119) |

## Verification

- `:app:compileDebugKotlin` exit 0.
- `.\gradlew.bat clean test lint :app:assembleDebug` exit 0 (local.properties
  created with forward-slash sdk.dir per the known Windows gotcha).
- Mobile Protection CI **fails by design** on these diffs — the golden
  baseline was deliberately re-baselined separately (see PR body + 73).

## Pre-existing useful wiring (not rebuilt)

`USB_DEVICE_ATTACHED` intent filter + `@xml/unoone_usb_device_filter`,
`onNewIntent` USB handling, `takePersistableUriPermission`,
`DocumentFile.fromTreeUri`, plus `ModelTierSelector`, `CandidateToolSelector`,
`ReActLoopController` — left intact.
