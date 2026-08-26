# Build Profiles

Profiles select which optional components are compiled. The core runtime and the C ABI are **identical across every profile** — a profile changes footprint, not behavior. Select with `-DIBAUDIO_PROFILE=<name>`.

| Profile | Experimental modules | Intended target |
|---|---|---|
| `core` | OFF | smallest possible runtime + reference provider |
| `minimal` | OFF | core + CLI |
| `india` | OFF | desktop + Bharat adaptation layer + en-IN/hi-IN/hinglish packs |
| `desktop` (default) | OFF | general desktop/server |
| `pi` | OFF | ARM64 lean, CPU-first, no Vulkan probe |
| `android` | OFF | india + JNI bridge, remote providers OFF |
| `full` | ON | everything incl. gated research placeholders + pinned audio.cpp adapter scaffold |

## Verified in this sandbox

All seven profiles configure and build cleanly on Linux x86_64 with g++ 11.5. Exported `ibaudio_*` symbol count: **79** for core/minimal/india/desktop/pi/android, **94** for `full`. The `full` profile's extra 15 symbols are exactly the three gated experimental modules (`neural_codec`, `prosody_controller`, `voice_clone_engine`).

## Honesty notes

- The only component that changes the *compiled source set* today is `IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES`. The language packs and adaptation layer that differentiate `india` are **data and configuration** (under `schemas/packs/` and the future `packs/`), not separately-compiled code, so the lean profiles and `india` currently produce the same native binary. This is recorded so the profile names are not read as implying different compiled engines.
- `pi` / `android` profiles compile on the host but are **build-only** for ARM64 until physical-device evidence exists. No ARM64 runtime claim is made.
- Composite audio.cpp model-set selection (building only the model families InBharat needs from upstream) is a property of the *audio.cpp provider build*, gated by `IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER` + `IBAUDIO_AUDIO_CPP_SOURCE_DIR`, and remains a source-scaffold until the pinned adapter is implemented.
