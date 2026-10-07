# AccessibilityService Justification (Google Play Policy)

**Updated 2026-10-07.**

UnoOne declares an AccessibilityService (`com.unoone.agent.accessibilitycontrol.UnoOneAccessibilityService`).
Google Play permits AccessibilityService use only for apps whose core purpose is to help users
with disabilities, or where the user explicitly enables the service for a clearly disclosed,
user-initiated purpose. This document states that purpose.

## What the service does

UnoOne is an **offline-first, voice-controllable accessibility agent**. The AccessibilityService
is the "hands" of the agent and is used for exactly two user-initiated purposes:

1. **Screen reading.**
   - `read_screen` reads visible text from the accessibility tree, so a blind/low-vision or
     voice-only user can ask "what's on my screen?" and hear the answer spoken via offline TTS.
   - When the user's command refers to on-screen content (it contains words such as "screen",
     "page", "button", "field" or "what's on"), up to 2,000 characters of visible text are read
     into the on-device planner. This lets the agent act on the element the user means.
   - The foreground app's package/activity name is also used as planner context.
   - The `ocr_screen` fallback for unlabeled apps does **not** use the AccessibilityService. It uses
     Android's MediaProjection screen-capture consent and on-device OCR.
2. **User-initiated UI control.** The service performs a UI action the user explicitly asked for by
   voice or text, e.g. "find and click Login", "fill username with …", "scroll down". It does this
   through:
   - the atomic accessibility tools: `go_home`, `go_back`, `scroll`, `open_notifications`,
     `open_recents`, `click_accessibility_node`, `type_into_accessibility_node` and
     `long_press_accessibility_node`;
   - the legacy `system_control` tool, with actions `click`, `type`, `fill`, `scroll_up/down`,
     `swipe`, `go_back`, `go_home`, `open_notifications`, `open_recents`, `find_and_click`,
     `long_press` and `read_screen`.

## What it does NOT do

- It does **not** monitor screen content in the background. While UnoOne is enabled, the service
  only records the foreground app's package/activity name when the window changes. Screen text is
  read only in response to a user command, as described above.
- It does **not** read notifications, credentials, payment fields, or messages except in response
  to a user command:
  - `read_screen`;
  - a UI-control tool;
  - a command that refers to on-screen content (planner context, above).
- It does **not** transmit accessibility data off-device. Accessibility-tree content is processed
  locally by the on-device model. No network path sends it:
  - the `web_search` tool sends only the search query;
  - the Secure Browser and model downloads do not use accessibility data.
- It does **not** auto-perform actions. Every action originates from an explicit user command.
  `SafetyGuard` then applies the risk class of each tool, kept in sync with
  `packages/tool-contracts/tools.v1.json` by CI. Under the default `STANDARD` security level:

  | Tools | Risk class | What the user sees |
  |---|---|---|
  | `go_home`, `go_back`, `scroll`, `open_notifications`, `open_recents` (navigation) | `DIRECT` | Runs without a confirmation dialog. |
  | `click_accessibility_node`, `type_into_accessibility_node`, `long_press_accessibility_node` | `CONFIRM` | One-tap confirmation. |
  | `system_control` (every action) and `read_screen` | `STRONG_CONFIRM` | The user must type "confirm". |

  If the user lowers the security level to `RELAXED` or `OFF` in Settings, confirmations are
  auto-approved.

## User control & transparency

- The service is **off by default**. The user must enable it in Android Settings → Accessibility,
  via a guided in-app permission flow, and can disable it at any time.
- Every executed action is written to a local **audit log** (input hashed, never stored in
  cleartext) viewable in-app under Logs, and exportable via `export_data`.
- A persistent foreground notification is shown while the agent is active.
- "Disable UnoOne" in Settings stops the agent. While UnoOne is disabled, the service ignores
  accessibility events.

## Control modes (planned — see docs/SAFETY.md)

The roadmap adds an explicit in-app **Control Mode** selector so the user can bound what the
service may do: Observe-only (read screen), Assist (suggest, ask before every tap), Agent
(execute direct actions, confirm risky ones), Lockdown (disable automation). Until then, the
per-tool risk classes above (DIRECT / CONFIRM / STRONG_CONFIRM) are the effective control mode.

## Prominent disclosure

The in-app permission flow shows, before routing the user to Settings:
> "UnoOne needs Accessibility to read what's on your screen and to tap/type/fill when you ask it
> to. It never reads your screen in the background and never sends screen content off your phone.
> You can turn this off any time in Settings → Accessibility."

**Status 2026-10-07 — not implemented.** This dialog does not exist in the app code. Today:

- the first-launch setup shows only a Toast: "Enable UnoOne Accessibility Service for native-app
  and external-browser automation.";
- the permission pipeline opens Settings → Accessibility directly;
- `accessibility_service_config.xml` has no `android:description`.

The disclosure has to be built in the app before this section is true.
