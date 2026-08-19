# Upstream derivation and optional audio.cpp adapter

The architecture/licensing drafts reviewed pristine audio.cpp `release-0.6`, commit `bb15edd78b56e035967e0eb999a6b28a62337db4`. RC1 uses that review to define an ABI/adapter boundary but copies **no audio.cpp implementation or asset**. The new reference runtime is not marketed as a broad audio.cpp fork or as neural parity.

`third_party/audio_cpp/` records the pin, empty SPDX selected closure, and empty patch ledger. The default build does not access upstream. Enabling `IBAUDIO_ENABLE_AUDIO_CPP_ADAPTER` requires a separate checkout at the exact clean commit and only compiles an isolated deferred scaffold with no upstream include/link.

A real selected adapter must: identify every upstream/transitive file; preserve applicable Apache/MIT/BSD notices; mark modifications; use an ordered patch queue; resolve missing provenance; admit one model family at a time with immutable licensed weights; contain exceptions; add cancellation hooks; verify CPU parity/memory/streaming truth; regenerate SBOM/symbols; and run platform gates. Do not import the upstream catalog or bundled assets by default.
