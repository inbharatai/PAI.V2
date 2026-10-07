# Data Safety Form (Google Play Console)

**Last updated:** 2026-10-07

Content for the Play Console Data Safety section. UnoOne is an **offline-first** app: the vast
majority of data stays on-device. The only network path is the opt-in `web_search` tool.

## Data collected & shared

**UnoOne does not sell or share user data with third parties.** All user content (notes, skills,
memory, conversation history, action logs, voice transcriptions) is stored locally on the device in
a Room database under app-private storage. That database is encrypted with SQLCipher. When the
user attaches and unlocks their own encrypted Pocket AI USB drive, notes, memories, skills and
conversation turns are also written to the vault on that drive, which then holds the canonical
copy. It is never uploaded by the app.

| Data type | Collected? | Purpose | On-device? | Transmitted off-device? |
|---|---|---|---|---|
| Notes, skills, memory, conversation history, preferences | Yes (user-created) | Core app function | Yes (SQLCipher-encrypted Room DB, app-private; also the user's own encrypted Pocket AI USB vault when attached and unlocked) | No |
| Voice audio / transcriptions | Yes (user-initiated) | Offline STT/TTS, voice memos | Yes (Sherpa on-device) | No |
| Screen text (accessibility tree) | Only in response to a user command: `read_screen`, a UI-control tool, or a command that refers to on-screen content | User-initiated screen reading & control | Yes | No |
| Camera frames | Only during active Blind Aid | On-device object detection | Yes (CameraX + MediaPipe EfficientDet on-device) | No |
| Calendar events | Only on explicit `check_calendar` / `check_calendar_conflict` | Read today's events / create event | Yes (read into the response) | No |
| Photos / files | No | — | — | — |
| Location | No | — | — | — |
| Personal ID / contact list | No | — | — | — |
| Web search query | Only when user enables online tools & invokes `web_search` | Optional online lookup | Query sent to DuckDuckGo (html.duckduckgo.com); results (title/URL/snippet/domain) returned to the user | **Yes — only this path, opt-in, off by default** |

## Encryption in transit

The optional `web_search` path uses HTTPS to `html.duckduckgo.com`.

## Encryption at rest

Local data is stored in app-private storage. The Room database (notes, skills, memory,
conversation history, action logs) is encrypted at rest with SQLCipher. Its key is random and is
wrapped by the Android Keystore. Privacy settings use `androidx.security`
EncryptedSharedPreferences. Action-log inputs are **hashed** (SHA-256), never stored in cleartext.
Records written to the user's Pocket AI USB vault are encrypted with AES-256-GCM. The vault's
master key is unlocked with the user's vault password (Argon2id key derivation) or with the
recovery phrase issued when the vault was created. Each record's metadata (record ID, record
type, timestamps) is stored in readable form alongside the encrypted content.

## Data deletion

Users can delete their data in-app at any time:
- "Delete all notes" (`delete_all_notes`, STRONG_CONFIRM).
- Delete individual notes by query (`delete_notes`).
- Uninstall any downloaded model (`Model Status → Uninstall`).
- Export all data (`export_data`) as JSON for review or migration.
- Uninstalling the app removes all app-private data (notes, skills, memory, logs, models). Data
  already written to the user's own Pocket AI USB vault stays on that drive and is not deleted by
  uninstalling the app.

No account is required. There is no server-side user data to delete because no user content is
uploaded.

## Functionality that uses data

- **App functionality:** notes, skills, memory, voice, accessibility control, Blind Aid, calendar.
- **Personalization:** memory/preferences improve command context; stored locally.
- No advertising, no analytics, no crash reporting that leaves the device.

## Family / children

UnoOne is not targeted at children. No age-gating is implemented because no account and no
data collection exists.

## Update intent

If a future version adds an online sync or account, this section and the privacy policy will be
updated before that version ships.