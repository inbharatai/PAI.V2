# UnoOne Safety & Tool Risk Model

> **Updated 2026-10-07.** The Android tier lists in §3 now match the CI-synced contract
> `packages/tool-contracts/tools.v1.json` exactly. `scripts/check_tool_contract_sync.py` checks
> that contract against the Kotlin `SafetyGuard`, `ToolPermissionRegistry` and
> `CanonicalToolRegistry`. `read_screen` and `ocr_screen` are corrected to `STRONG_CONFIRM`. A
> Desktop (UnoOne Power) section has been added (§12). Sections marked *(spec)* are roadmap items
> and are not shipped.

How UnoOne decides what an agent action may do, and the roadmap to tighten it. This is the
reference behind `SafetyGuard`, `ToolPermissionRegistry`, and the Play-review safety narrative.

## 1. Two-layer risk classification (implemented)

Every tool call passes through `SafetyPipeline.classifyRisk(tool, input)`, which takes the **max**
of:

- **Tool-level risk** — `SafetyGuard.classify(toolName)` → a fixed tier per tool. A tool name that
  is not in the table defaults to `STRONG_CONFIRM`.
- **Input-level risk** — `SafetyGuard.classifyFromInput(rawText)` → keyword-based override that can
  only raise (never lower) the tier.

In `STANDARD` mode, when the on-device brain is loaded, an LLM safety judge also runs for every
non-`DIRECT` step. The judge can only escalate: UNSAFE → `BLOCK`, NEEDS_CONFIRM → at least
`CONFIRM`.

Tiers:

| Tier | Behavior |
|---|---|
| `DIRECT` | Executes immediately (notes, search, open Chrome, etc.) |
| `CONFIRM` | One-tap confirmation dialog |
| `STRONG_CONFIRM` | User must type "confirm" (destructive / automation / drafts) |
| `BLOCK` | Never executes (payments, passwords, OTP, auto-send, install, factory reset) |

## 2. Contextual draft vs. auto-send (implemented 2026-06-26)

`classifyFromInput` now distinguishes **draft** paths from **auto-send** paths:

- `send money / pay / bank / credit card / wire transfer / OTP / password` → **BLOCK**
- `auto send / send automatically / send it now` → **BLOCK**
- `draft …` or `send/… WhatsApp/email …` → **STRONG_CONFIRM** (draft; user still presses send)
- generic `send …` / `message` with no draft/app context → **BLOCK**

So "send a WhatsApp message to mom" routes to `send_whatsapp` or `draft_whatsapp_message` (both
STRONG_CONFIRM) and proceeds with confirmation instead of being hard-blocked. Auto-send of a final
message is always blocked.

## 3. Tool risk tiers (current — Android, from `tools.v1.json`)

All 47 Android entries in the contract are listed below: 42 active tools plus 5 blocked classes.
The access requirement comes from `ToolPermissionRegistry`.

- **DIRECT (17)** — runs without a confirmation dialog: `create_note`, `search_notes`,
  `summarize_text`, `speak_response`, `open_chrome`, `open_app`, `go_home`\*, `go_back`\*,
  `scroll`\*, `open_notifications`\*, `open_recents`\*, `resolve_contact`, `check_calendar`,
  `open_calendar`, `check_calendar_conflict`, `deactivate_blind_aid`, `prepare_document_fill`.
- **CONFIRM (13)** — one-tap confirmation: `voice_recording`, `web_search`, `open_url`,
  `open_camera`, `click_accessibility_node`\*, `type_into_accessibility_node`\*,
  `long_press_accessibility_node`\*, `create_skill`, `open_calendar_insert`,
  `create_calendar_event`, `open_dialer`, `share_text`, `secure_browser_task`.
- **STRONG_CONFIRM (12)** — the user must type "confirm": `system_control`\*, `read_screen`\*,
  `ocr_screen`†, `draft_email`, `send_whatsapp`, `draft_whatsapp_message`,
  `send_prepared_whatsapp`, `delete_notes`, `delete_all_notes`, `export_data`, `detect_objects`,
  `describe_scene`†.
- **BLOCK (5, `status: blocked`, no executor exists):** `access_passwords`, `install_app`,
  `make_payment`, `send_message`, `silent_control`.

\* Needs the UnoOne AccessibilityService. † Needs a MediaProjection (screen-capture) consent.
`detect_objects` and `open_camera` need the CAMERA runtime permission. `voice_recording` needs
RECORD_AUDIO. `check_calendar`, `check_calendar_conflict` and `create_calendar_event` need
READ_CALENDAR. `resolve_contact` lists READ_CONTACTS, but that permission is **not declared** in
the manifest (see `play-review/permissions-matrix.md`).

`read_screen` reads the accessibility tree. `ocr_screen` captures a screenshot under the
`mediaProjection` foreground service and runs on-device OCR. Both are `STRONG_CONFIRM`, because the
screen can show passwords, OTPs or banking data.

### Tool-schema alignment note (item 29, updated)

UI control has two paths:

- **Legacy `system_control(action=…)`.** It is still registered and is STRONG_CONFIRM for every
  action. The rule-based parser still emits it, e.g. for "find and click …".
- **Atomic accessibility tools** (marked \* above). These are the preferred path. Navigation
  actions are DIRECT. Taps, typing and long-presses on a specific node are CONFIRM.

The old standalone defensive entries `click`, `type`, `long_press`, `find_and_click` and `fill`
are no longer in the `SafetyGuard` table. They are not canonical tool names, so the brain rejects
them. If such a name ever reaches `SafetyGuard`, the unknown-name fallback classifies it as
`STRONG_CONFIRM`.

## 3.1 User-selected enforcement level

The stored security level is consulted by both the phone agent and Secure Browser PageAgent:

| Level | Phone agent | Secure Browser |
|---|---|---|
| `STANDARD` (default) | Judge and every confirmation/block enforced. | Confirmation, takeover and block decisions enforced. |
| `RELAXED` | Judge disabled; BLOCK enforced; confirmations auto-approve. | Standard browser action decisions remain enforced. |
| `OFF` | Judge, confirmations and blocks bypassed. | PageAgent confirmation, takeover and block decisions bypassed. |

`OFF` is deliberately unsafe and is labelled as such in Settings and Secure Browser. It allows
PageAgent to interact with forms, user-selected file inputs, credentials and payment fields without
an UnoOne safety prompt. It does not disable the exact-origin WebView message bridge, grant arbitrary
filesystem access, or add a JavaScript/native-code execution tool. Those boundaries remain in every
mode. Changing the mode is local and persistent; `STANDARD` remains the first-launch default.

## 4. Online tools toggle (spec — partial)

- `web_search` is `CONFIRM` and in `UnoOneToolSet` so the model can propose it.
- Privacy Settings already has an "Online Web Search (RAG)" switch (`online_rag_enabled`, default
  OFF, stored in EncryptedSharedPreferences). As of 2026-10-07, **the `web_search` executor does
  not read this switch.** Each call is gated only by the `CONFIRM` tier and the network check
  below.
- **Roadmap:** wire an `Online tools: OFF by default` setting. When it is OFF, exclude
  `web_search` from the Gemma tool schema so the model cannot propose it, and show a privacy
  warning when the user turns it ON.
- Until that setting ships, the offline-first guard in `ActionExecutor` (`isOnline()`) returns an
  explicit offline message when there is no network, and `web_search` never auto-opens links.

## 5. Control Mode selector (spec)

A visible in-app selector bounding what Accessibility automation may do:

| Mode | Allowed |
|---|---|
| Observe only | `read_screen`, summarize — no taps/types |
| Assist | suggest actions, ask before every tap/type |
| Agent | execute DIRECT actions, confirm risky ones (closest to the current effective behavior) |
| Lockdown | disable all automation |

Until the selector ships, the per-tool tiers in §3 are the effective control mode under the default
`STANDARD` level:

- navigation (`go_home`, `go_back`, `scroll`, `open_notifications`, `open_recents`) is DIRECT;
- node taps, typing and long-presses are CONFIRM;
- `system_control`, `read_screen`, `ocr_screen` and `describe_scene` are STRONG_CONFIRM.

`RELAXED` and `OFF` auto-approve confirmations (§3.1). The "Disable UnoOne" emergency stop in
Settings turns all automation off.

## 6. `open_app` category escalation (spec)

`open_app` is currently DIRECT. Roadmap: escalate by app category — normal apps DIRECT;
camera/settings/browser/payment/banking/password-manager/work-profile → CONFIRM; unknown
package name proposed by the LLM → CONFIRM.

## 7. Calendar / read_screen consent (spec)

- `check_calendar` is DIRECT after `READ_CALENDAR` is granted. Roadmap: first calendar access
  CONFIRM, or a persistent "Allow calendar reads without confirmation: OFF by default" setting.
- `read_screen` is STRONG_CONFIRM (as are `ocr_screen` and `describe_scene`). Roadmap: session
  consent ("Allow screen reading for next 10 minutes" / "only in current app" / "always ask") to
  reduce confirmation fatigue for accessibility users.
- Current behaviour to keep in mind: when a command that reaches the planner contains a screen
  keyword ("screen", "page", "button", "field", "what's on", "ocr", …), the context snapshot reads
  up to 2,000 characters of accessibility screen text into the on-device planner prompt *before*
  any tool is chosen. If that text is empty and a MediaProjection token is already held, it reads
  up to 1,000 characters of OCR text instead. The foreground app's package and activity names are
  always included. This read is on-device only, but it is not gated by the `read_screen`
  confirmation.

## 8. Blind Aid camera vs. navigation (spec — permission split done)

`detect_objects` now requires **only** the CAMERA runtime permission. The old Accessibility
requirement was removed. Detection runs through MediaPipe Tasks `ObjectDetector` with the bundled
EfficientDet-Lite2 int8 model
(`android-app/UnoOneAgent/phonecontrol/src/main/assets/models/efficientdet_lite2_int8.tflite`; see
`docs/BLIND_AID_MODEL.md`). The tool stays STRONG_CONFIRM. A separate
`blind_aid_navigation_mode` (CAMERA + Accessibility, for when UI context is needed) is still only a
roadmap idea.

## 9. Package resolution (current + roadmap)

`PackageResolver` uses a curated ~10-app map (no `QUERY_ALL_PACKAGES`). Roadmap: add
`queryIntentActivities()` for launchable apps plus user-selected app shortcuts saved locally.
`QUERY_ALL_PACKAGES` will not be requested.

## 10. Audit integrity (spec)

`AuditLogger` stores a SHA-256 of the raw input (not cleartext) and the outcome. The `action_logs`
table lives in the SQLCipher-encrypted Room cache. Rows older than 24 h are evicted at app start,
and the table is cleared when the Pocket AI vault disconnects (`VaultCacheLifecycle`). Roadmap: add
a local hash chain (`log_hash = sha256(previous_hash + timestamp + tool + args + status)`) so
exported logs can show tampering. This needs a new `previousHash` column on `ActionLogEntity` and a
Room migration. It is tracked but not yet shipped.

## 11. Privacy dashboard (spec)

A front-and-center screen showing: mic active, camera active, screen reading active, online tools
active, Android fallback active, Gemma loaded, last 10 actions, export/delete data. The building
blocks exist (audit viewer, `DataExporter`, `VoiceRuntimeState`) but are not yet consolidated into
one dashboard.

## 12. Desktop (UnoOne Power)

The desktop agent (Tauri app + vendored harness) has its own guard and boundaries. The Android
tiers and security levels above do not apply there.

**Tool risk classes (contract).** `tools.v1.json` lists 16 desktop tools. Harness confirmation
modes map to risk classes as Always → STRONG_CONFIRM, OnSideEffect → CONFIRM, Never → DIRECT.

| Risk class | Tools |
|---|---|
| DIRECT | `pai.search_notes`, `pai.list_documents`, `pai.read_document`, `pai.verify_vault`, `fs.read`, `fs.list`, `workspace.search`, `agent.spawn` |
| CONFIRM | `fs.write`, `fs.mkdir`, `fs.copy`, `workspace.patch`, `doc.create`, `browser.act`, `web.preview` |
| STRONG_CONFIRM | `process.run` |

On desktop these are classifications, not per-call dialogs:

- In the full-access agent lane, which is the default, the desktop confirmation provider answers
  `AllowedOnce` for every call.
- In the read-only lane it answers `Unavailable`, so tools that need confirmation do not run.

The effective consent points are the controls below.

**SafetyGuard levels (`apps/desktop/src-tauri/src/safety.rs`).** There are three levels:
`STANDARD`, `RELAXED` and `OFF`. The level is persisted to `VAULT/config/security.json`. If the
file is missing or unreadable, the level is `STANDARD`.

`review_action` runs before execution only for:

- the vault read tools (`pai.*`);
- `browser.act`;
- the legacy desktop ReAct loop (`agent.rs`).

It does **not** run for the harness built-ins (`fs.*`, `process.run`) or for the `workspace.*`,
`doc.create` and `web.preview` adapters. Those are bounded by the folder fence and the
host-command consent described below.

| Level | Blocked tool names | Confidence floor (applies only when a score is reported) |
|---|---|---|
| `STANDARD` | `shell_execute`, `file_delete_system`, `network_raw_socket`, `registry_modify` | 0.7 |
| `RELAXED` | `shell_execute`, `registry_modify` | 0.5 |
| `OFF` | none | 0.0 |

- **Pattern check (every level, including `OFF`).** A string parameter is rejected if it contains
  a listed system-manipulation, exfiltration or privilege pattern, e.g. `rm -rf`, `format c:`,
  `shutdown`, `reg add`, `chmod 777`, `nc -e`, `sudo su`.
- **Blocked names.** These are a defensive denylist. None of them is a tool id in
  `tools.v1.json`.
- **Confidence floor.** Harness calls carry no confidence score, so the floor never applies to
  them.
- **System-path redaction.** `STANDARD` also computes one, but the harness bridge uses only the
  approve/deny verdict. File reach is therefore bounded by the folder fence below, not by the
  redaction.

**Granted-folder fence for file tools (`granted_fs.rs`, `harness_bridge.rs`).**

- `fs.*`, `workspace.*` and `doc.create` work only inside the agent workspace folder and any extra
  folders the user granted. The workspace folder is the one the user granted, or
  `%USERPROFILE%\UnoOneAgent` (`$HOME/UnoOneAgent`) if none has been granted; that default folder
  is created automatically.
- Extra folders can be granted in Settings or through an in-chat approval card. The card waits
  60 s and denies by default.
- Each folder is its own `RootedFs` fence: canonicalization, containment, symlink and TOCTOU
  checks, and 2 MiB read / 4 MiB write limits.
- Routing between folders fails closed.
- Folder grants never authorize programs.

**Session host-command consent for `process.run` (`desktop_process.rs`, `main.rs`).**

- `process.run` is refused unless the user has turned on host commands for the current session in
  Settings.
- Turning them on requires an unlocked vault. The setting is not persisted. Enabling and revoking
  are both audited in the vault.
- A task already running when permission is granted must be restarted to use it.
- Programs come from a fixed allowlist. The allowlist includes shells (`bash`, `sh`, `cmd`,
  `powershell`, `pwsh`), so it is **not** a containment boundary.
- Host commands run with the user's own file and network access and are **not sandboxed**.
- Bounds: at most 32 owned processes, size limits on arguments and environment, and a filtered
  environment.

**Owned-process termination.** Each host command is spawned in its own process group (Unix) or a
kill-on-close Job Object (Windows) and is tracked. `stop_desktop_work` closes admission and kills
every owned process tree (SIGKILL to the group, or `TerminateJobObject`). It runs in these cases:

- the user revokes host-command permission;
- the vault is locked (`lock_vault`);
- the Pocket AI drive is removed (mount monitor → `cleanup_after_removal`);
- the app exits.

A program that deliberately leaves its process group is not tracked.

**Generated code from coding tasks and verification runs (Linux only).** Coding-task gates and
previews (`CodingTaskService`) and knowledge verification runs execute generated code only inside
a Linux sandbox:

- `setpriv` with no_new_privs and every capability dropped;
- `bwrap` with `--unshare-all` and user namespaces disabled;
- read-only `/`, `/dev` and runtime binds, no `/proc`, and a 16 MiB writable `/tmp` (the
  workspace mode adds a bounded `/work` tmpfs);
- `prlimit`.

The backend's program hashes are pinned, and a readiness probe must pass before it is used.
Coding tasks open only on a root inside a folder the user already granted.

On **Windows and macOS this sandbox is unavailable**. The service returns `IsolationUnavailable`
*before* any process is spawned, and coding tasks cannot be opened. This sandbox does **not**
cover the chat agent's `process.run`, which runs unsandboxed host commands under session consent
(above).

Limits the code itself states:

- per-process rlimits plus a soft, host-side RSS watchdog;
- no cgroup quota.
