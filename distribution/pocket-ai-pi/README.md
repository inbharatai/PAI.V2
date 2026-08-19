# Pocket AI Pi — Linux ARM64 production distribution

This directory is the product-specific Raspberry Pi / Linux ARM64 layer. It does **not** fork the reusable InBharat Harness or InBharat Audio cores. It consumes their verified AArch64 artifacts and adds Pocket AI policy: one canonical encrypted vault, verified models, local-only inference, lifecycle shutdown, and fail-closed packaging.

## No dummy rule

No executable is checked in as a placeholder. `build-package.py` accepts only real files that already exist, copies them, hashes them, and writes the package manifest. Missing Harness, Audio, audio.cpp, llama.cpp, speech acceptance, or model evidence is a build failure.

## Build order

1. Build Harness ARM64 with `universal/inbharat-harness/scripts/build-linux-arm64.sh`.
2. Build the generic InBharat Audio ARM64 core with `universal/inbharat-audio/scripts/build_linux_arm64.sh` for portability checks. For the final Pocket AI speech artifact, use `build_pocket_ai_linux_arm64.sh` with `IBAUDIO_AUDIO_CPP_SOURCE_DIR` pointing to the exact reviewed upstream checkout.
3. Build a real ARM64 `audiocpp_cli` from that same reviewed audio.cpp source, stage the real ASR/TTS models, then run `scripts/run-real-audio-acceptance.sh` on the physical Pi.
4. Build/obtain real ARM64 llama.cpp CPU, and optionally Vulkan, binaries.
5. Run `scripts/build-package.py` with those exact artifacts.
6. Copy the generated package content to the Pocket AI USB layout, alongside the encrypted `VAULT` and real `MODELS`.
7. On the physical Pi, run `scripts/pi-preflight.sh`, `scripts/verify-package.py`, then `scripts/launch-runtime.sh`.

## Runtime invariants

- Linux ARM64 only.
- Everything binds to loopback by default.
- Network capability in Harness is denied unless a product policy explicitly grants it.
- The model file must be inside the Pocket AI root and must match a caller-supplied SHA-256.
- The canonical memory authority remains the encrypted Pocket AI vault.
- Arbitrary shell tools are not exposed to models.
- Bharat Audio cannot activate without real end-to-end speech acceptance evidence.
- CPU is the default. Vulkan is an opt-in backend only after real Pi benchmark/parity/thermal validation.
- Removing the Pocket AI drive triggers cancellation/shutdown/lock policy in the embedding runtime; the included watcher can signal a supervisor when the identity disappears.

## Physical-device release gates

A production label requires evidence from the actual Pi model and OS image: Harness route/tool/memory tests, model identity and inference tests, real ASR/TTS/VAD tests, 30-minute sustained thermal/load test, USB surprise-removal test, vault recovery test, reboot test, and airplane/offline test. A cross-compile alone is not a release pass.
