# InBharat Audio adversarial hardening audit

Audit target: `0.1.0-rc1` at `461606efc85c0252034928f3f2332b34e9625d0e`  
Hardened result: `0.1.0-rc2` on the local `hardening-audit` branch  
Disposition: no confirmed Critical findings; all confirmed High and Medium findings below are fixed and regression-tested.

## Confirmed findings

| ID | Severity | Affected area | Finding and impact | Resolution and regression evidence |
|---|---|---|---|---|
| A-01 | High | `ibaudio_model_load`; `ibaudio_sha256_file` | `strict_path_policy=1` enforced containment only when `allowed_model_root` was non-empty. The default empty root therefore failed open and allowed hashing/loading metadata from arbitrary regular host files. | Strict mode now requires an explicit root for every external path and checks containment before file-type probing. Default-runtime and allowed/outside-root regressions verify fail-closed behavior. |
| A-02 | High | Stream event queue | Configured queue size was soft for every non-droppable event and TTS generated all chunks synchronously before polling. Large TTS or rapid VAD boundaries could grow the queue and owned buffers without a hard ceiling. | Stale provisional/diagnostic events are dropped first, contiguous TTS audio is coalesced after the soft limit, and an absolute 4,096-event ceiling clears payloads and emits terminal cancellation. Regressions verify full TTS frame preservation and VAD flood cancellation. |
| A-03 | Medium | `ibaudio_session_create`; VAD stream/offline math | `vad_min_speech_ms` and `vad_min_silence_ms` were unbounded. Extreme values could overflow derived arithmetic and alter detection state; barge threshold bounds were also incomplete. | VAD minima are restricted to 1–60,000 ms and barge threshold to -160..12 dBFS. Malformed configuration regressions cover overflow-scale values. |
| A-04 | Medium | Asynchronous job start | ASR/VAD copied caller PCM and TTS copied text before acquiring the session single-flight slot. Concurrent rejected starts could still allocate/copy large inputs, creating avoidable memory/CPU denial and inconsistent admission order. | Public start functions acquire ownership first under an RAII reservation, copy only after admission, and retain busy state only after worker creation. Duplicate async starts are regression-tested as `BUSY` with null output. |
| A-05 | Medium | JNI/Kotlin text and array boundary | JNI used modified UTF-8 APIs as if they were standard UTF-8, mishandling supplementary Unicode/NUL. Several native allocations could throw across JNI, PCM copies were weakly bounded, and negative durations wrapped to large unsigned values. | RC2 explicitly converts Java UTF-16 ↔ standard UTF-8 with surrogate handling/replacement, bounds PCM and VAD arrays, maps allocation failures to Java exceptions, and rejects negative durations in Kotlin and JNI. The translation unit compiles with strict warnings against a JNI ABI stub; physical Android remains pending. |
| A-06 | Medium | Buffer constructors and stream enqueue | Raw output buffers were published before payload allocation/assignment completed. Allocation failure could leak the buffer and its live-buffer metric, permanently wedging parent teardown. Stream enqueue had equivalent ownership gaps. | Constructors and facade paths now retain `unique_ptr` ownership until fully initialized and increment metrics only on publication. Enqueue destroys incoming payloads on merge/push failure. ASan leak detection passes. |
| A-07 | Low | Stream cancellation error state | `ibaudio_stream_finish` returned `CANCELLED` without updating thread-local error details, leaving a stale unrelated last error. | Finish now records a structured lifecycle cancellation. Regression verifies status and last-error agreement. |
| A-08 | Low | `ibaudio_session_get_barge_in_state` | The only mutex-taking C function outside the guarded exception boundary could theoretically leak a C++ `system_error` across C. | The function now uses the common guarded boundary. |
| A-09 | Low | Standalone hashing | Hashing had no explicit 4 GiB policy cap although model admission did. | The same 4 GiB limit now applies before hashing. |
| A-10 | Low | Malformed/stress depth | Deterministic fuzz loops were relatively short and no race sanitizer lane was retained. | WAV cases increased to 50,000, PCM pipelines to 10,000, VAD/TTS queue floods were added, Release and ASan+UBSan pass, and a static-library ThreadSanitizer build passes all CTest lanes. Zig shared-library TSan was rejected because the runtime crashed before `main`; the static lane avoids duplicate sanitizer-runtime linkage.

## Validation lanes

- Clean Linux Release build, C99 consumer, 58-symbol ABI check, CLI and metadata tests.
- ASan+UBSan with leak detection and all CTest lanes.
- Static-library ThreadSanitizer with all CTest lanes.
- Deterministic malformed WAV/PCM campaigns, lifecycle loops, concurrent sessions, cancellation races, queue floods, fixture reproducibility, and benchmark generation.
- Strict JNI C++17 warning-as-error compile against a local JNI ABI stub.
- Windows x64 cross-build and optional clean audio.cpp pin/wrong-pin scaffolding checks.

Raw logs are retained as `reports/HARDENING_*` files. Final counts and artifact hashes are recorded in `HARDENING_VALIDATION.md` after the clean release gate.

## Residual risks, not misrepresented as fixed

- Raw stale pointers and concurrent parent destruction remain C-caller undefined behavior; bindings must own handles.
- Path containment uses canonicalization and retains a hostile cross-process rename/symlink TOCTOU residual; production external model loading should use descriptor-relative handles/openat-style brokers.
- TTS stream start still generates synchronously; use jobs for cancellable generation.
- Cooperative cancellation cannot preempt an uncooperative future neural backend call.
- Android NDK/device lifecycle, Bluetooth, phone interruptions, thermals, battery, macOS/Metal, Windows runtime, CUDA, HIP, and real Vulkan inference remain pending.
- Reference ASR/TTS are deterministic framework engines, not language-quality claims; no external model licence or corpus validation is implied.
