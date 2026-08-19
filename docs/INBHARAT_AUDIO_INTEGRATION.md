# Pocket AI / InBharat Audio integration

`bharat_audio.rs` is the product adapter. It does not make the reusable InBharat Audio core depend on UnoOne.

The default config is deliberately `enabled: false`. UnoOne continues to use its existing Whisper.cpp/Piper implementation until all of these are true:

1. InBharat Audio was built with an exact clean audio.cpp source pin.
2. `ibaudio audio-cpp-status --json` reports both `adapter_compiled=true` and `inference_ready=true`.
3. The runtime's reviewed commit exactly equals the Pocket AI speech config commit.
4. The chosen audio.cpp model family and assets are present inside the Pocket AI package.
5. The Pocket AI package manifest has verified those assets before launch.
6. English/Hindi/Hinglish acceptance, cancellation, streaming, RAM/thermal and fallback tests have passed.

The adapter invokes `audiocpp_cli` directly, never through a shell, and confines model/output paths to the Pocket AI root. Failure of any gate returns control to the legacy voice backend.
