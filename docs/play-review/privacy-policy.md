# UnoOne Privacy Policy

**Last updated:** 2026-10-07

UnoOne ("the app") is built privacy-first. This policy explains what the app does with your data.

## 1. The short version

UnoOne runs **on your device**. Your notes, skills, memory, conversation history, voice
transcriptions, and action logs are stored locally on your phone and are not uploaded to any
server. If you attach and unlock your own Pocket AI USB drive, your notes, memories, skills and
conversation history are also saved to the encrypted vault on that drive. No account is
required.

The only time data leaves your device is if **you** turn on the optional **Online tools** setting
and ask the agent to **search the web**. That single feature sends your search query to
DuckDuckGo over HTTPS and shows you the results. It is **off by default**.

## 2. Data we store on your device

- **Notes, skills, memory, and preferences** — you create these; they live in the app's private
  storage, with notes, skills and memory in an encrypted (SQLCipher) database. When your Pocket AI
  USB drive is attached and unlocked, notes, memories and skills are also written to the encrypted
  vault on that drive, which then holds the main copy.
- **Conversation history** — the commands you give and the agent's spoken replies are saved as
  conversation turns in the same encrypted database. When your Pocket AI USB drive is attached and
  unlocked, they are also saved in its vault.
- **Voice transcriptions** — when you use voice input or record a voice memo, audio is transcribed
  on-device by Sherpa-ONNX and the text is saved as a note.
- **Action logs** — a local audit trail of agent actions. The raw text you typed/spoke is
  **hashed** (one-way) before storage; it is not stored in readable form.
- **Downloaded models** — optional on-device AI models (STT/TTS/Gemma) you install from the Model
  Status screen or push manually. They live in app-private storage.

## 3. Data that may leave your device (opt-in only)

- **Web search** — only if you enable Online tools and invoke a web search. Your search query is
  sent to `html.duckduckgo.com` over HTTPS. Results (title, URL, snippet, source domain) are shown
  to you. No other app content is sent.
- **Accessibility / camera / calendar / microphone** — these are processed **on-device only**.
  Screen content, camera frames, calendar events, and audio are never transmitted off your device.

## 4. Permissions

UnoOne requests microphone, camera, calendar, overlay, and accessibility permissions for its
voice, Blind Aid, calendar, and phone-control features. Each is requested at the point of use and
can be revoked in Android Settings. See `permissions-matrix.md` for the full list and rationale.

## 5. Data retention & deletion

- Delete notes in-app ("delete all notes" or by query).
- Export everything (`export_data`) as JSON.
- Uninstall models from Model Status.
- Uninstalling the app deletes all app-private data. Data already saved to your own Pocket AI USB
  vault stays on that drive and is **not** deleted by uninstalling the app.
- Because no user content is uploaded, there is no server-side copy to delete.

## 6. Children

UnoOne is not directed at children. No account and no data collection means no age-gating is
needed.

## 7. Security

The app's local database is encrypted with SQLCipher, using a key protected by the Android
Keystore. Privacy settings use EncryptedSharedPreferences. Records saved to your Pocket AI USB
vault are encrypted with AES-256-GCM. The vault is unlocked with your vault password or its
recovery phrase. Record metadata such as IDs, record type and timestamps is stored in readable
form. Action-log inputs are hashed. The optional web-search path uses HTTPS. The app does not
request broad storage or all-package access.

## 8. Changes

If a future version adds an account or cloud sync, this policy will be updated before that version
ships, and you will be asked to consent.

## 9. Contact

For privacy questions, open an issue at the project repository. This is a local-first research/
assistive app; there is no server infrastructure holding your data.