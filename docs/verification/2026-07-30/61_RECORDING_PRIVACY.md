# 61 — Recording Privacy

**Status: BUILDS_NOT_RUNTIME_TESTED** for live-microphone behavior;
**VERIFIED_WORKING** for the retention-policy logic (20/20 unit tests in
`unoone-recording-policy`, CI-gated before packaging).

## Source-level guarantees now in force

- `TRANSCRIPT_ONLY` and `SUMMARY_ONLY` never write the captured audio into the
  vault (was a single `Full | … | …` match arm).
- Transcription actually runs (it was never called at all).
- Temp WAV for STT is written OUTSIDE the vault, deleted, and the deletion is
  re-verified by re-checking the path.
- Buffers are zeroized (overwrite → clear → release capacity).
- Zero-sample capture reports `ERROR` with an actionable message instead of a
  silent `STOPPED`.
- `SUMMARY_ONLY` produces an explicit `SUMMARIZATION_NOT_IMPLEMENTED` warning
  in the outcome and is **disabled in the UI** — until a summariser exists it
  is not a selectable data-loss trap.
- `RecordingOutcome` reports `samples_captured`, what persisted,
  `retention_verified`, warnings and a truthful user message; the UI surfaces
  all of it.

## The physical acceptance journey (human gate, requires unlocked vault + mic)

Record/stop under each of FULL / TRANSCRIPT_ONLY / SUMMARY_ONLY /
PRIVATE_SESSION and enumerate the vault to prove which records exist; verify
temp WAV is gone; pause/resume, bookmarks, cancel, zero-samples, mic denial,
device loss, USB removal mid-recording, WAV validity, plaintext leakage scan.

This session is unattended and cannot grant microphone permission or type the
vault password, so those remain BLOCKED_BY_ENVIRONMENT and must be run by a
human with the mic. Nothing here claims they pass.
