# Foreground Service Justification (Google Play Policy)

**Updated 2026-10-07.**

Google Play requires every foreground service to have three things: an appropriate type, the
matching foreground-service permission, and a user-perceptible reason (the user started the task,
or there is a clear notification).

UnoOne declares **three** foreground services, each with its own type, plus one boot receiver
(`android-app/UnoOneAgent/app/src/main/AndroidManifest.xml`):

- `MediaProjectionService` — `mediaProjection`;
- `FloatingAgentService` — `specialUse`;
- `VoiceService` — `microphone`;
- `BootCompletedReceiver` — the boot receiver.

All three services are `exported="false"`. The in-app **"Disable UnoOne"** switch (Settings)
stops all three and keeps them stopped until the user re-enables UnoOne.

## 1. `FloatingAgentService` — `foregroundServiceType="specialUse"`

**Permission:** `FOREGROUND_SERVICE_SPECIAL_USE` + `SYSTEM_ALERT_WINDOW`.

**Why a foreground service:** the floating agent bubble (overlay) must stay alive across other apps
so the user can invoke voice/text commands from anywhere. Without a foreground service, Android
kills the overlay process under memory pressure, breaking the core use case.

**Why `specialUse`:** the bubble is a persistent on-screen agent interface. It does not fit the
camera/health/location/media/microphone/phone-call/connected-device/data-sync categories, so it is
a special-use persistent UI surface.

**User-perceptibility:**

- **When it starts.** The bubble service starts when UnoOne is enabled and the user has granted
  "Display over other apps". That is a Settings grant the user makes explicitly; the app asks for
  it in the first-launch setup. There is no separate in-app bubble on/off toggle.
- **Notification.** A persistent, ongoing notification is shown: title **"UnoOne active"**, text
  **"Floating agent is active — tap to pause / stop"**. In the current build this notification
  has no tap action and no action buttons.
- **How it stops.** The service stops when the user turns UnoOne off ("Disable UnoOne").

**Play `specialUse` declaration:** in the Play Console FGS type form, select "Special Use". Provide
this justification and a demo video (see `demo-video-script.md`) showing the bubble and its
persistent notification. The manifest does not currently declare a
`PROPERTY_SPECIAL_USE_FGS_SUBTYPE` `<property>` for this service.

## 2. `VoiceService` — `foregroundServiceType="microphone"`

**Permission:** `FOREGROUND_SERVICE_MICROPHONE` + `RECORD_AUDIO` (runtime).

**Why a foreground service:** background wake-word detection + offline STT transcription require a
live microphone session that must survive screen-off and app backgrounding so the user can say
"UnoOne …" and have the device respond.

**Why `microphone`:** the service captures audio — the microphone FGS type is the exact match.

**User-perceptibility:**

- **When it starts.** The service starts when UnoOne is enabled and the user has granted
  `RECORD_AUDIO`. Without that grant the service is not started. It can also start after boot,
  but only if the user opted in (§4).
- **Notification.** A persistent, ongoing notification is shown: title **"UnoOne listening
  locally"**, text **"Listening locally — Mic active. Say 'UnoOne' or 'Listen' to give a
  command."** The text tracks the state, e.g. "Paused while a phone or voice call is active" or
  "Processing command locally...".
- **How it stops.** No silent background microphone use occurs. When the user turns UnoOne off,
  the service is stopped and microphone capture stops.

## 3. `MediaProjectionService` — `foregroundServiceType="mediaProjection"`

**Permission:** `FOREGROUND_SERVICE_MEDIA_PROJECTION`.

**Why a foreground service:** on-device screenshot OCR (`ocr_screen`, `describe_scene`, and the
OCR fallback for screens without accessibility labels) needs a MediaProjection token. Android 14+
requires a running `mediaProjection` foreground service before the projection is created. This
service owns that token.

**Why `mediaProjection`:** the service holds a screen-capture session — the exact match.

**User-perceptibility:**

- **When it starts.** It starts only after the user approves Android's own screen-capture consent
  dialog, which `ScreenshotPermissionActivity` shows when an OCR request needs a token. If the
  user denies, the service is not started. Both OCR tools are STRONG_CONFIRM in `SafetyGuard`.
- **Notification.** While it runs, an ongoing notification is shown: title **"UnoOne screen
  reading active"**, text **"Screen content stays on this device and is processed with on-device
  OCR."**
- **How it stops.** The service stops when the projection is stopped, or when UnoOne is turned
  off.

## 4. `BootCompletedReceiver` (`RECEIVE_BOOT_COMPLETED`) — not a service

This receiver handles `BOOT_COMPLETED` (and the OEM `QUICKBOOT_POWERON`). It starts `VoiceService`
after boot only when both conditions hold:

- the user turned on **"Start automatically when the phone starts"** in Settings (default OFF);
- UnoOne is enabled.

Otherwise it does nothing. `BOOT_COMPLETED` is a protected broadcast that only the OS can send. If
Android refuses a boot-time microphone FGS start, the failure is logged; it is not retried in a
loop.

## Notifications

All three services post a persistent, ongoing notification that shows the task state, per Google's
"users should be aware of the ongoing task" requirement. In the current build these notifications
have **no** pause/stop action buttons or tap action. The user stops the services from inside the
app ("Disable UnoOne").
