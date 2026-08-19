# Optional audio.cpp adapter boundary

The reviewed upstream is pinned to `bb15edd78b56e035967e0eb999a6b28a62337db4` (`release-0.6`). **No upstream source, model, voice, or asset is copied into this repository.** The default build neither reads nor links audio.cpp.

`-DIBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=ON -DIBAUDIO_AUDIO_CPP_SOURCE_DIR=/path/to/pristine/audio.cpp` enables only the isolated adapter scaffold. Configure fails if the checkout is modified or at another commit. It still exposes no upstream model: each selected family needs a reviewed file closure, SPDX provenance, immutable model hash/license, parity tests, cooperative cancellation hooks, and platform evidence before its adapter can become executable.

Do not point this option at a moving branch, do not add unreviewed model assets, and do not patch the pristine checkout in place. Use the ordered `patches/` queue after review.
