# Pocket AI / InBharat Audio integration

`bharat_audio.rs` is the product adapter. It does not make the reusable InBharat Audio core depend on UnoOne.

The default config is deliberately `enabled: false`. UnoOne continues to use its existing Whisper.cpp/Piper implementation until all of these are true:

1. InBharat Audio was built with an exact clean audio.cpp source pin.
2. `ibaudio audio-cpp-status --json` reports both `adapter_compiled=true` and `inference_ready=true` (inference readiness is runtime-probed against the configured model roots — a compiled adapter with no weights on disk is NOT ready).
3. The runtime's reviewed commit exactly equals the Pocket AI speech config commit.
4. The chosen audio.cpp model family and assets are present inside the Pocket AI package.
5. The Pocket AI package manifest has verified those assets — speech models, configs, acceptance attestations, and the audio runtime are declared under `platforms.windows.speech` and hashed by the desktop app's background DesktopLaunch sweep (never on the fast launch path).
6. English/Hindi/Hinglish acceptance, the library's cancellation/streaming unit tests, and the RAM/thermal and fallback tests have passed. Library-level streaming tests passing does NOT make the product route a streaming route — the product route is buffered-final (see `docs/SPEECH_ARCHITECTURE.md`).

The adapter invokes `audiocpp_cli` directly, never through a shell, and confines model/output paths to the Pocket AI root. Failure of any gate returns control to the legacy voice backend **only when the explicit policy permits legacy** — there is no silent fallback.
