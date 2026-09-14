# 89 — Chat Panel STS (Voice In/Out) and Document/File Attachments

**Date:** 2026-09-12
**Scope:** `apps/desktop/src/src/components/ChatView.tsx`, `apps/desktop/src-tauri/src/documents.rs` (`parse_attached_document`), `apps/desktop/src/src/lib/tauri.ts`
**Posture:** user-directed capability gap ("the chat panel should be able to
understand sts and also should be able to accept images docs files etc"),
shipped as PR #16.

## 1. The gap (live, through the real app)

The chat panel accepted typed text and images only:

- No voice input — speech lived in the Recordings/Accessibility views, but
  the chat itself was keyboard-only.
- No voice output — replies were text-only, no way to hear them.
- No documents — the file picker's `accept` list was `image/png,image/jpeg,image/webp,image/gif`
  and anything else was silently discarded.

## 2. Design

### STS in — mic button rides the audited recording pipeline

The composer's mic button calls the same `start_recording`/`stop_recording`
commands the Recordings view uses, at `TRANSCRIPT_ONLY` privacy:

- Audio is transcribed on-device, then destroyed (`AudioDisposition::TranscribeThenDestroy`
  in `recording-policy`) — it never reaches durable storage.
- Only the **encrypted transcript** is kept (a vault record).
- The frontend reads it back with `vault_read_record` and drops the text
  into the input box; the user can edit before sending.

No new audio code paths, no WebView `getUserMedia`, no raw host paths —
the chat inherits the pipeline that is already policy-tested for
retention, privacy levels, and fail-closed behavior.

### STS out — Speak button + auto-speak

Every assistant reply gets a **🔊 Speak** button that synthesizes through
`synthesize_speech` (the same offline TTS lane verified in doc 88's speech
round-trip) and plays inline via a bounded `<audio>` element
(`convertFileSrc`). An **auto-speak toggle** (persisted) voices replies as
they land. Replies over 2000 characters are cut with a spoken
"[reply truncated for speech]" notice. A voice-language selector
(en/hi/hinglish, persisted) feeds both directions.

### Attachments — images, documents, text/code

- **Images** keep the existing mmproj vision lane (unchanged).
- **PDF/DOCX/XLSX/PPTX** go to a new `parse_attached_document` command:
  the WebView picker yields base64 (never a host path), the backend
  enforces a 20 MiB hard cap, decodes, writes a temp file, parses it
  with the **same audited extractors the document lane uses** (`lopdf`
  for PDF, zip+XML for DOCX/XLSX/PPTX), deletes the temp file, and
  returns the extracted text (8000-byte cap with a `[Truncated — …]`
  notice). Extracted text travels as a labelled
  `[attached file: name (kind)]` block in the prompt.
- **Text/code files** (txt/md/csv/json/yaml/…/py/rs/ts, 30+ extensions)
  are read client-side with a 256 KB cap.
- Up to 4 images + 4 files per turn; unsupported types surface a clear
  error, never a silent drop.

## 3. Tests

- 5 new Rust tests for `parse_attached_document`: text round-trip,
  truncation notice, 20 MiB cap rejection, invalid base64 rejection,
  binary junk reported as text does not panic.
- `cargo test --workspace` green (23 suites), `cargo clippy -D warnings`
  clean, `cargo fmt` clean, frontend `tsc -b && vite build` clean.

## 4. Live acceptance (post-merge, on the re-staged drive)

- [x] Mic → speak a sentence → transcript lands in the input box → send
      — VERIFIED 2026-09-13 live on the drive (sts-send.js): mic → 6s record →
      on-device Qwen3-ASR → transcript in `textarea.chat-input` → Enter →
      transcript message rendered in chat history. Acoustic capture of real
      human speech was live-verified 2026-09-08 (drive A→Z, 100% ASR recall);
      this session's host has no physical speakers (render = line-out,
      Headphone endpoint unplugged) so loopback speech cannot reach the mic.
      Engine-level real-speech round-trip closed the gap (2026-09-13,
      sts-engine-roundtrip.js): drive TTS synthesized a known 18-word sentence
      → on-device `transcribe_audio` (same production SpeechRouter) returned
      it with 100% word recall, exact match. STS engine verdict: real speech in
      → correct transcript out, fully offline.
- [ ] Reply Speak button produces playable audio; auto-speak toggle works
      — Speak button VERIFIED 2026-09-13 (speech-verify Check D + camera-accept:
      `<audio aria-label="Synthesized speech playback">` mounts, playhead
      advances, plays to completion t==duration). Auto-speak toggle still to
      verify on the final re-staged build.
- [ ] Attach a PDF and ask a question about its contents — answer must
      quote the document
- [ ] Attach a code file and ask about it
- [ ] Hindi voice input and spoken reply
- [x] Audio is destroyed (no new recording audio in VAULT/recordings),
      only the encrypted transcript record exists
      — VERIFIED 2026-09-13 live (sts-accept.js, two runs): recording count
      unchanged (+0 files in VAULT/recordings audio), encrypted records +1 per
      run; recording privacy is TRANSCRIPT_ONLY (audio destroyed post-ASR,
      only the encrypted transcript record persists).

*(results recorded here after the final drive re-stage)*