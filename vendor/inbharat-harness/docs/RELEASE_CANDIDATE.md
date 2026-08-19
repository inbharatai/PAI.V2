# Release Candidate Scope

`0.1.0-rc.2` is the adversarially hardened local vertical slice for architectural validation. It can route all four levels, stream offline providers, select and execute capability-scoped tools, persist/recover/replay/fork sessions, cancel work, run owned jobs/subagents, generate metrics, and embed through C.

It intentionally does not claim:

- a production remote model/network adapter;
- a remote authenticated server;
- kernel-grade local sandboxing or descendant process-group termination on Linux/macOS/Windows/Android;
- durable jobs or continuable subagents after process restart;
- encrypted SQLite storage or full-text queries;
- a WASM/RPC plugin host;
- Android JNI packaging or desktop installers;
- cryptographically authenticated event logs.

Those omissions fail closed or are absent rather than simulated. The release gate validates the std-only core, release-mode panic containment, static C library and external C consumer, musl Linux binary, Windows and Android cross-checks, deterministic routing, false-activation corpora, bounded model streams and retries, prior-turn context, cancellation, recovery, persistence, root confinement, pinned subprocess resolution, Clippy, and CLI smokes.
