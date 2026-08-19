# UnoOne Mobile Audit — Bug Findings (Phase 0)

Date: 2026-07-21
Baseline: v0.4.0-alpha-v2

## HIGH Severity

### 1. AccessibilityNodeInfo double-recycling
**File**: `accessibilitycontrol/src/main/java/com/unoone/agent/accessibilitycontrol/UnoOneAccessibilityService.kt:50-78`
`clickNodeWithText()` recycles parent nodes in a loop, but child nodes from `findAccessibilityNodeInfosByText()` may be double-recycled if a clickable parent is found and recycled, then the child is also recycled in the `finally` block.

**Fix**: Track which nodes have been recycled. Avoid recycling a node that has already been recycled.

### 2. FloatingAgentService ComposeView disposal order
**File**: `app/src/main/java/com/unoone/agent/FloatingAgentService.kt:230-235`
In `onDestroy()`, `disposeComposition()` is called before removing ComposeViews from the WindowManager. On some devices, this throws `IllegalArgumentException` ("View not attached to window manager").

**Fix**: Remove views from WindowManager first, then dispose composition.

### 3. SecurityLevel stored in plain SharedPreferences
**File**: `app/src/main/java/com/unoone/agent/safety/SecurityLevel.kt:42-45`
The `SecurityLevel` preference controls whether dangerous actions are auto-approved. It is stored in plain `SharedPreferences("unoone_settings", MODE_PRIVATE)`. A rooted device could change `security_level` from `STANDARD` to `OFF`, disabling all safety guards. Meanwhile, `PrivacySettingsViewModel` correctly uses `EncryptedSharedPreferences`.

**Fix**: Migrate `SecurityLevel` and `agent_enabled` to `EncryptedSharedPreferences`.

### 4. captureScreen() blocks with Thread.sleep under synchronized lock
**File**: `phonecontrol/src/main/java/com/unoone/agent/phonecontrol/ScreenshotCapture.kt:146-153`
`captureScreen()` polls `acquireLatestImage()` up to 10 times with 50ms sleeps while holding `@Synchronized`. This blocks any other thread from calling `installProjection()`, `clearProjection()`, or another `captureScreen()` for up to 500ms.

**Fix**: Use coroutine-based `withTimeoutOrNull` instead of `Thread.sleep`. Remove `@Synchronized` and use a `Mutex`.

### 5. Room migration only covers v1→v2
**File**: `storage/src/main/java/com/unoone/agent/storage/db/UnoOneDatabase.kt:36-58`
Only `MIGRATION_1_2` exists. Future schema changes need migration paths. If version is bumped without migration, Room will crash.

**Fix**: Add `fallbackToDestructiveMigration()` as safety net, and ensure all future schema changes add proper migrations.

### 6. MediaProjection token passed via Intent extras
**File**: `app/src/main/java/com/unoone/agent/screenshot/MediaProjectionService.kt:54-61`
The `EXTRA_RESULT_DATA` is passed through `Intent` extras. On pre-Android 12, this could theoretically be intercepted by a malicious app.

**Fix**: Ensure the service is not exported and the Intent is secured with signature-level permissions.

---

## MEDIUM Severity

### 7. flaggedFlakyTools is not thread-safe
**File**: `app/src/main/java/com/unoone/agent/AgentOrchestrator.kt:404`
`flaggedFlakyTools` is `mutableSetOf<String>()` (LinkedHashSet), not thread-safe.

**Fix**: Replace with `ConcurrentHashMap.newKeySet<String>()`.

### 8. Agent enabled/disabled state reversible via SharedPreferences
**File**: `app/src/main/java/com/unoone/agent/UnoOneApplication.kt:70-73`
`agent_enabled` in plain SharedPreferences can be tampered on rooted devices.

**Fix**: Move to EncryptedSharedPreferences with integrity check.

### 9. DataExporter writes cleartext JSON
**File**: `app/src/main/java/com/unoone/agent/data/DataExporter.kt:37-65`
Exports notes, skills, memories, and action logs as plaintext JSON. May contain private content.

**Fix**: Encrypt exports or route through vault. Redact `inputText` from ActionLogEntity.

### 10. DebugCommandReceiver exported with DUMP permission
**File**: `app/src/debug/AndroidManifest.xml`
`exported="true"` with `android:permission="android.permission.DUMP"`. If debug APK leaks, DUMP permission is the only gate.

**Fix**: Ensure release builds exclude this. Add proguard/manifest merger rule to strip it.

### 11. densityDpi hardcoded to DENSITY_MEDIUM
**File**: `phonecontrol/src/main/java/com/unoone/agent/phonecontrol/ScreenshotCapture.kt:188`
Virtual display always uses 160dpi regardless of device.

**Fix**: Use actual display metrics: `context.resources.displayMetrics.densityDpi`.

### 12. VoiceService hasSpeechActivity fixed RMS threshold
**File**: `voice/src/main/java/com/unoone/agent/voice/VoiceService.kt:555-566`
Hardcoded threshold of 500 doesn't adapt to device microphone sensitivity or ambient noise.

**Fix**: Implement adaptive threshold with ambient noise calibration.

### 13. VoiceService.onTaskRemoved restarts from background
**File**: `voice/src/main/java/com/unoone/agent/voice/VoiceService.kt:626-633`
On Android 12+, `startForegroundService()` from background throws `ForegroundServiceStartNotAllowedException`.

**Fix**: Use `WorkManager` or `AlarmManager` for restart instead.

### 14. FloatingAgentService lifecycle management incomplete
**File**: `app/src/main/java/com/unoone/agent/FloatingAgentService.kt:122-135`
Lifecycle transitions in `onStartCommand` go CREATED→STARTED→RESUMED, but `onDestroy` disposes views before lifecycle state transitions.

**Fix**: Transition lifecycle DOWN (ON_PAUSE→ON_STOP→ON_DESTROY) before view cleanup.

### 15. NoteDao.search() LIKE wildcards not escaped
**File**: `storage/src/main/java/com/unoone/agent/storage/dao/NoteDao.kt:25-26`
`LIKE '%' || :query || '%'` doesn't escape `%` and `_` in user input.

**Fix**: Escape special characters in the query before passing to LIKE, or use FTS.

### 16. security-crypto alpha dependency
**File**: `app/build.gradle.kts:111`
`security-crypto:1.1.0-alpha06` is alpha quality.

**Fix**: Track stable release. Consider platform-level encryption as alternative.

### 17. Guava 27.0.1-android (2018)
**File**: `app/build.gradle.kts:120`
Contains known CVEs. CameraX likely no longer requires it.

**Fix**: Update to latest or remove if CameraX doesn't need it.

---

## LOW Severity

### 18. Image not closed in ScreenshotCapture exception path
**File**: `phonecontrol/src/main/java/com/unoone/agent/phonecontrol/ScreenshotCapture.kt:164-169`
If `bitmap.copyPixelsFromBuffer()` throws, the `image` is not closed.

**Fix**: Use `try/finally` to close `image`.

### 19. InputSanitizer only filters English patterns
**File**: `core/src/main/java/com/unoone/agent/core/util/InputSanitizer.kt:13-21`
`DANGEROUS_PATTERNS` only matches English prompt-injection phrases. Indic languages bypass the filter.

**Fix**: Add Hindi/Bengali/Tamil/etc. injection patterns.

### 20. ActionLog toolArgsJson leaks key names
**File**: `app/src/main/java/com/unoone/agent/AgentOrchestrator.kt:1010`
Stores `{"privateArgKeys":["content","title"]}` which leaks information about user actions.

**Fix**: Hash or remove arg key names in audit logs.

### 21. MemoryModule loads all memories for relevance matching
**File**: `memory/src/main/java/com/unoone/agent/memory/MemoryModule.kt:65-98`
Linear scan of all memories of each type. Doesn't scale.

**Fix**: Use Room FTS or LIKE queries with relevance scoring.

### 22. System.currentTimeMillis() in entity defaults
**File**: `storage/src/main/java/com/unoone/agent/storage/entity/ActionLogEntity.kt:27`
Timestamp set at construction, not insertion. Delay between construction and insertion causes timestamp drift.

**Fix**: Set timestamp in DAO `@Insert` method instead.

### 23. AccessibilityService static instance pattern
**File**: `accessibilitycontrol/src/main/java/com/unoone/agent/accessibilitycontrol/UnoOneAccessibilityService.kt:197-201`
Common pattern but could briefly return stale reference between service destruction and recreation.

**Fix**: Already mitigated by null checks. Low priority.

### 24. Room schema export may not be configured
**File**: `storage/src/main/java/com/unoone/agent/storage/db/UnoOneDatabase.kt:28`
`exportSchema = true` but `room.schemaLocation` may not be in Gradle config.

**Fix**: Add `ksp { arg("room.schemaLocation", "$projectDir/schemas") }` to storage build.gradle.kts.

### 25. Overly broad ProGuard keep rules
**File**: `app/proguard-rules.pro`
Keeps entire packages (`com.google.mediapipe.**`, `com.google.mlkit.**`, `com.k2fsa.sherpa.onnx.**`).

**Fix**: Target specific classes used by the app to reduce APK size.

---

## Summary

| Severity | Count |
|----------|-------|
| HIGH | 6 |
| MEDIUM | 11 |
| LOW | 8 |
| **Total** | **25** |

Priority fixes for Phase 1: #3 (SecurityLevel encryption), #2 (ComposeView disposal), #7 (flaggedFlakyTools thread safety), #4 (screenshot blocking), #9 (DataExporter cleartext).