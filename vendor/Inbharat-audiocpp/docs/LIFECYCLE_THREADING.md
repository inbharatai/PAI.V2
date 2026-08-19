# Lifecycle and thread-safety

| Object | Thread contract |
|---|---|
| runtime | concurrent catalog/metrics/model creation; cache guarded |
| model | immutable; concurrent session creation; release requires no sessions |
| session | single-flight; overlapping run/job/stream returns `BUSY` |
| job | cancel/info/wait are thread-safe; result is taken once |
| stream | push/finish/cancel/poll serialized by internal mutex; one logical producer recommended |
| buffer | immutable after publication; release once through pointer-to-handle |

Destruction never silently abandons children. Runtime release returns `BUSY` for live models or owned buffers; model release for sessions; session release for jobs/streams or active work. Job release requests cancellation, joins its worker, and frees untaken output. Stream release drops queued payloads and releases the single-flight slot.

The C API cannot prevent use of a raw stale pointer after a successful release; it clears the caller's pointer and tests generation-like misuse through strict ownership. Language bindings must hide native pointer construction. Kotlin owners retain parents and make close retryable when native returns `BUSY`.

Do not run inference on UI or real-time audio callback threads. Android exposes a single-thread executor to application code. Separate sessions can run concurrently, but a production neural adapter may impose a runtime/backend model concurrency budget.
