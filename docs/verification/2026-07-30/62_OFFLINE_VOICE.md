# 62 — Offline Voice (bundled Whisper + Piper only)

**Status: VERIFIED_WORKING** (bundled-binary pipeline, real audio produced and
transcribed this session).

## Round trip actually executed

1. **Piper synthesis** (bundled
   `D:\UNOONE\RUNTIMES\WINDOWS\VOICE\piper.exe` + manifest-declared
   `voice.onnx` / `voice.onnx.json`):
   input text `This is a Pocket AI offline voice pipeline test.` →
   `speech.wav` (195,232 B, 4.23 s audio), real-time factor 0.06.

2. **Whisper transcription** (bundled
   `D:\UNOONE\RUNTIMES\WINDOWS\VOICE\whisper.exe` + manifest-declared
   `whisper-base.en.bin`) on that WAV → transcript file containing
   **` This is a Pocket AI offline voice pipeline test.`** — verbatim round
   trip in 5.32 s.

Both binaries and models are manifest-declared and hash-verified by the
545-entry strict pass. No network was used; both tools are fully offline.

## Not covered here (human gate / environmental)

- Live microphone capture through the app's recording pipeline (needs a human
  to grant mic + speak; the app requires an unlocked vault).
- Audible playback confirmation (I cannot listen; WAV bytes were validated by
  successful re-transcription instead).
- Piper languages beyond the single bundled English voice — no other voice
  models are staged, so only English is honestly enabled.
- Temp-file cleanup in the app path (code-verified in recording.rs on the fix
  branch: temp WAV is written outside the vault, deleted, and the deletion is
  re-checked).
