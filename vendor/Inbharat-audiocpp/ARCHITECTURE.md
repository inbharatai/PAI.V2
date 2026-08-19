# Architecture

## Layering

```text
CLI / Kotlin
      |
  thin JNI
      |
include/inbharat/ibaudio.h       versioned C99 ABI, opaque handles
      |
src/{runtime,session,stream}     policy, lifecycle, jobs, diagnostics
      |
src/{audio,sha256}               bounded deterministic primitives
      |
CPU reference engines            ASR analyzer, tone TTS, energy VAD
      |
optional adapters                 isolated; audio.cpp is deferred/default-off
```

The C header is the only stable native contract. Internal C++ layouts, STL containers, mutexes, threads, and exceptions are hidden with visibility controls. Every exported function either validates directly or executes through an exception guard.

## Runtime and BackendManager

A runtime owns immutable backend/model catalogs, cache/path policy, and atomic metrics. CPU is always selected. An explicit accelerator request fails unless runtime auto-fallback is allowed; a permitted fallback is counted and diagnosed. Vulkan probing only loads the platform loader and checks `vkGetInstanceProcAddr`; it never reports an inference adapter or device as usable.

## Models and sessions

A model is immutable metadata plus an optional verified artifact path. Built-in reference models have no external artifact. Sessions are single-flight and bind one task. Separate sessions may run concurrently; one session rejects overlapping work with `BUSY`.

## Jobs and streams

Jobs copy borrowed inputs before returning, own one worker thread, expose thread-safe state/wait/cancel/take-result, and settle before release. Cancellation is polled in preprocessing, resampling, VAD windows, ASR analysis, and TTS character/frame loops.

Streams are pull queues. No callback occurs under a native lock. Audio streams normalize chunks to mono 16 kHz incrementally and retain state. Queue limits are soft for lossless/terminal events; stale provisional/diagnostic events are dropped first. Stream finish/cancel releases the session's single-flight slot; the stream handle still owns unpolled events until release.

## Buffers

Input audio/string spans are borrowed only for synchronous calls. Jobs copy them. Output handles own immutable bytes, UTF-8, interleaved float PCM, or fixed-layout VAD segments. `ibaudio_buffer_release()` is the sole allocator boundary.

## Files and cache

The default models need no files. External artifact paths are canonicalized, constrained under `allowed_model_root` in strict mode, size-bounded, and SHA-256-verified before load. The cache directory is explicit and created at runtime construction; no current-working-directory model discovery occurs.
