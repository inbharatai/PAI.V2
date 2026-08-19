# audio.cpp upstream candidate tracking

The production adapter remains pinned to the last locally reviewed commit until a full
source/build/model-family validation pass is completed.  A newer public upstream commit
may be recorded here for the **next review**, but recording it does not make it approved.

- Canonical upstream: `https://github.com/0xShug0/audio.cpp`
- Current public candidate observed 2026-08-19: `26dcb5c4cf5aa016ae6285096a7b45f2671e5d17`
- Candidate release line: audio.cpp 0.6.1
- Current InBharat approved integration pin: see `IBAUDIO_PINNED_AUDIO_CPP_COMMIT` in `CMakeLists.txt`

## Promotion gate

A candidate may replace the approved pin only after all of the following pass on the
actual source checkout and model assets: clean immutable Git pin, licence inventory,
Windows/Linux/macOS/Android/Pi builds where supported, ASR/TTS/VAD parity tests,
English/Hindi/Hinglish acceptance corpus, cancellation and streaming tests, bounded
memory/thermal tests, model SHA-256 verification, and legacy fallback validation.
