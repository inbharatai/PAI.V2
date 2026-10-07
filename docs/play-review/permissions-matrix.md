# UnoOne — Permissions Matrix (Google Play Review)

**Updated 2026-10-07.**

This document lists every permission, foreground service and receiver that UnoOne
(`com.unoone.agent`) declares. For each one it gives why the code needs it, its protection level,
and the Play-review risk it carries.

**Sources of truth:**

- `android-app/UnoOneAgent/app/src/main/AndroidManifest.xml`.
- The library-module manifests that merge into it. `phonecontrol` adds `VIBRATE` and `voice` adds
  `RECORD_AUDIO`. Both are already declared by `:app`. Every other module manifest is empty.
- `app/src/debug/AndroidManifest.xml`, for debug builds only.

Permissions contributed by third-party AARs (ML Kit, MediaPipe, LiteRT-LM, Sherpa-ONNX, CameraX)
were **not** verified. That needs a built merged manifest
(`app/build/intermediates/merged_manifests/release/`), so check it before submission.

## Permissions

| Permission | Protection level | Used by | Why | Play risk |
|---|---|---|---|---|
| `INTERNET` | normal | `web_search` tool (RAGManager); model and language-pack downloads (Model Status / Language Packs → `ModelInstaller`); Secure Browser WebView (`secure_browser_task`, approved origins only) | (1) A DuckDuckGo lookup (`html.duckduckgo.com`). It is CONFIRM-tier, so each call needs one tap under the default `STANDARD` level. When there is no network, the offline-first guard returns an explicit offline message. The Privacy Settings "Online Web Search (RAG)" switch is currently **not** read by the `web_search` executor (see `docs/SAFETY.md` §4). (2) User-initiated downloads from the URLs pinned in `models_manifest.json`. (3) PageAgent tasks on the approved origins in `ApprovedOriginPolicy`. | Low — normal permission, not runtime. |
| `ACCESS_NETWORK_STATE` | normal | `ActionExecutor.isOnline()` | Detect connectivity so `web_search` fails fast offline instead of waiting on a 5s socket timeout. | Low. |
| `RECORD_AUDIO` | dangerous (runtime) | `VoiceService` (wake word + STT), `voice_recording`, voice input | Capture speech for offline transcription and voice memos. Requested in the first-launch setup flow (`PermissionManager.REQUIRED_PERMISSIONS`). If it is denied, `VoiceService` does not start. | Medium — microphone; justify in Data Safety. |
| `MODIFY_AUDIO_SETTINGS` | normal | Declared for the voice pipeline | No call that changes audio settings (`setMode`, `setSpeakerphoneOn`, `requestAudioFocus`, …) was found in the app sources. `VoiceCapturePolicy` only *reads* the audio mode, and that needs no permission. Candidate for removal. | Low. |
| `CAMERA` | dangerous (runtime) | `open_camera`, `detect_objects` (Blind Aid) | Launch the camera app. Blind Aid runs real-time on-device object detection through CameraX + MediaPipe EfficientDet-Lite2. Requested at first use by the tool safety pipeline. | Medium — camera; justify in Data Safety. |
| `WRITE_EXTERNAL_STORAGE` / `READ_EXTERNAL_STORAGE` (`maxSdkVersion=28`) | dangerous ≤ SDK 28 | legacy only | App-private storage via `getExternalFilesDir()`; these are legacy stubs for API 28. No code requests them. | Low. |
| `VIBRATE` | normal | Blind Aid haptics (`BlindAidManager`) | Tactile feedback for obstacle alerts. | Low. |
| `FOREGROUND_SERVICE` | normal | All FGS | Required to start any foreground service. | Low. |
| `FOREGROUND_SERVICE_MICROPHONE` | normal | `VoiceService` | Background wake-word + STT capture. | Medium — mic in background; user-perceptible notification required. |
| `FOREGROUND_SERVICE_MEDIA_PROJECTION` | normal | `MediaProjectionService` | Holds the MediaProjection token for on-device screenshot OCR (`ocr_screen`, `describe_scene`, and the screen-OCR fallback). Android 14+ requires a running `mediaProjection` FGS before the projection is created. | Medium — screen capture. Android's screen-capture consent dialog is shown whenever no projection token is held, and an ongoing notification is shown while the service runs. |
| `FOREGROUND_SERVICE_SPECIAL_USE` | normal | `FloatingAgentService` | Persistent floating bubble overlay agent UI. | **High** — Play reviews all `TYPE_SPECIAL_USE`; justification required (see `foreground-service-justification.md`). |
| `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` | normal | `MainActivity` first-launch setup | Opens the system "ignore battery optimizations" request (plus an OEM autostart screen where available) so background voice is not killed. The boot receiver's own comment notes that Android 15 may refuse a boot-time microphone FGS start unless the app is exempt from battery optimization. | Medium — justify. Today the request is shown automatically in the first-launch setup flow, not from an explicit user action. |
| `POST_NOTIFICATIONS` | dangerous (runtime, API 33+) | FGS notifications | Post the persistent foreground-service notifications. Requested in the first-launch setup flow. | Low. |
| `RECEIVE_BOOT_COMPLETED` | normal | `BootCompletedReceiver` | Optional auto-launch: starts `VoiceService` after boot **only if** the user turned on "Start automatically when the phone starts" in Settings (default OFF) **and** UnoOne is enabled. | Low — normal; the auto-start is opt-in. |
| `READ_CALENDAR` | dangerous (runtime) | `check_calendar`, `check_calendar_conflict`, `create_calendar_event` | Read events and check for conflicts. Events are *created* through the calendar app's insert screen (`ACTION_INSERT`), not by writing to the provider. Requested at first use. | Medium — calendar data; first read is user-initiated. |
| `SYSTEM_ALERT_WINDOW` | special (Settings) | `FloatingAgentService` | Draw the floating bubble over other apps. | **High** — overlay; justify; user must grant via Settings. |

`WAKE_LOCK` and `WRITE_CALENDAR` are **not** declared. The manifest notes that no code path
acquires a wake lock. Calendar events are created through the calendar app's own insert screen.

### Deliberately NOT requested (Play-safe)

- `MANAGE_EXTERNAL_STORAGE` — removed; app uses app-private `getExternalFilesDir()`.
- `READ_CONTACTS` — not declared. Note that the `resolve_contact` tool still lists
  `READ_CONTACTS` as its permission requirement (`ToolPermissionRegistry`). Because the permission
  is not in the manifest, Android cannot grant it, so contact lookup cannot obtain access in this
  build. The owner must decide whether to declare it (and disclose contacts in Data Safety) or
  remove the requirement.
- `QUERY_ALL_PACKAGES` — removed; `PackageResolver` uses a curated app map, and the manifest
  `<queries>` block lists exact packages and intents. (Planned: `queryIntentActivities()` for launchable apps + user-saved shortcuts — see `docs/SAFETY.md`.)

## Foreground services

| Service | FGS type | Permission | Why user-perceptible |
|---|---|---|---|
| `screenshot.MediaProjectionService` | `mediaProjection` | `FOREGROUND_SERVICE_MEDIA_PROJECTION` | Started only after the user approves Android's screen-capture consent dialog for an OCR request. Ongoing notification, title "UnoOne screen reading active", text "Screen content stays on this device and is processed with on-device OCR." |
| `FloatingAgentService` | `specialUse` | `FOREGROUND_SERVICE_SPECIAL_USE` + `SYSTEM_ALERT_WINDOW` | The floating agent bubble. It runs while UnoOne is enabled and the user has granted "Display over other apps". Ongoing notification, title "UnoOne active", text "Floating agent is active — tap to pause / stop". The notification has no tap action or buttons; see `foreground-service-justification.md`. |
| `voice.VoiceService` | `microphone` | `FOREGROUND_SERVICE_MICROPHONE` (+ `RECORD_AUDIO` granted) | Background wake-word listening + STT. It runs while UnoOne is enabled and the microphone permission is granted, or after boot if the user opted in. Ongoing notification, title "UnoOne listening locally", text "Listening locally — Mic active. Say 'UnoOne' or 'Listen' to give a command." The text changes with state, e.g. "Paused while a phone or voice call is active". |

All three services are `exported="false"`. "Disable UnoOne" in Settings stops all three.

## Other declared components

- `accessibilitycontrol.UnoOneAccessibilityService` is guarded by `BIND_ACCESSIBILITY_SERVICE` and
  is `exported="false"`. It is off until the user enables it in Android Settings → Accessibility
  (see `accessibility-justification.md`).
- `screenshot.ScreenshotPermissionActivity` is a non-exported, transparent activity that shows
  Android's MediaProjection consent.
- `autostart.BootCompletedReceiver` is `exported="true"` and handles `BOOT_COMPLETED` and
  `QUICKBOOT_POWERON`. `BOOT_COMPLETED` is a protected broadcast that only the OS can send. The
  receiver does nothing unless the opt-in auto-start setting is on.
- **Debug builds only:** `DebugCommandReceiver` (`app/src/debug/AndroidManifest.xml`), protected
  by `android.permission.DUMP`. It is not in the release manifest.
- `MainActivity` has a `USB_DEVICE_ATTACHED` / `USB_DEVICE_DETACHED` intent filter (device filter
  `@xml/unoone_usb_device_filter`), used to detect the Pocket AI drive.

## Hardware features

- `android.hardware.microphone` (required) — core voice input.
- `android.hardware.camera` (not required) — Blind Aid + camera tool are optional.
- `android.hardware.bluetooth` (not required).
- `android.hardware.usb.host` (not required) — Pocket AI USB drive detection.

## Targeting

- `compileSdk`/`targetSdk` = 35 (Android 15), `minSdk` = 28 (Android 9).
- Android 14+ requires declared FGS types plus matching FGS permissions. All three FGS types
  (`mediaProjection`, `specialUse`, `microphone`) have their matching permission declared.
- For `specialUse`, the manifest does not declare a `PROPERTY_SPECIAL_USE_FGS_SUBTYPE` `<property>`.
  This could not be checked offline against current Play guidance; verify before submission.