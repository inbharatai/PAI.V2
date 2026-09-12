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

- [ ] Mic → speak a sentence → transcript lands in the input box → send
- [ ] Reply Speak button produces playable audio; auto-speak toggle works
- [ ] Attach a PDF and ask a question about its contents — answer must
      quote the document
- [ ] Attach a code file and ask about it
- [ ] Hindi voice input and spoken reply
- [ ] Audio is destroyed (no new recording audio in VAULT/recordings),
      only the encrypted transcript record exists

*(results recorded here after the final drive re-stage)*