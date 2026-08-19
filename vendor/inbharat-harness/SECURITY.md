# Security

## Reporting

Do not publish suspected vulnerabilities with exploit details. Report them privately to the project security contact configured by the distributor. Include version, platform, minimal reproduction, and whether credentials or workspace writes are involved.

## Threat assumptions

Model output, imported transcript text, prompts, attachment metadata, remote providers, external tools, and future plugins are untrusted. A local user may intentionally grant authority, but authority is never inferred from prose.

## Defaults

- model and root-confined file reads only;
- no write, process, network, workspace, job, subagent, or credential capability unless explicitly granted;
- one-shot confirmation for mutating tools;
- empty process allowlist and direct argv only;
- child environment cleared before explicit entries are added;
- telemetry absent/off; metrics contain counters only;
- attachment bytes and credential values excluded from session events;
- strict byte, item, time, step, depth, and output limits;
- no arbitrary in-process dynamic libraries, model-authored code, shell interpolation, or Node runtime.

## Filesystem boundary

`RootedFs` rejects absolute paths and lexical parent traversal, canonicalizes existing targets and parents, denies symlink escape, bounds UTF-8 reads/writes, and uses create-new temporary files plus sync and rename for replacement. This is an **in-process policy fence**, not a kernel security boundary. Reads re-verify the opened handle's device+inode against the validation-time metadata and cap the live read stream at max_read_bytes, narrowing the read-path check-then-act window and enforcing the advertised byte bound against in-place growth. It retains a symlink/rename time-of-check/time-of-use residual against another process able to mutate the same tree (the exact-open instant cannot be closed without openat2/RESOLVE_BENEATH, unavailable in this dependency-free std build). Do not use it as multi-user hostile-code containment.

## Process boundary

`LocalExecutionBroker` resolves each exact allowlisted program id to a canonical executable path when the broker is created, then uses direct argv. It never invokes a shell or re-resolves through a caller-supplied child `PATH`. Requests and environment entries are bounded and validated, start in the authority root, receive no inherited environment, drain stdout/stderr through bounded readers, and kill/wait the direct child on cancellation or deadline. The std-only RC does **not** provide cross-platform descendant process-group termination; programs that daemonize or spawn surviving descendants must not be allowlisted. The built-in local sandbox provider rejects process/network requests because it cannot provide an OS boundary. CLI partial local execution requires explicit `--trusted-process`, `--allow-process`, `--allow-program`, and `--yes` flags.

## Persistence

The FNV-1a event chain detects accidental corruption and simple modification; it is **not cryptographic authentication**. Deployments requiring adversarial tamper evidence must add keyed signatures or a cryptographic hash in a future format version. JSONL files are not encrypted by this crate. Do not put secrets in prompts or event detail fields.

## Native ABI

Release builds retain Rust unwind semantics so every exported C boundary can catch an internal panic and return `IB_STATUS_PANIC`; changing the release profile to `panic=abort` violates this contract and is regression-tested. Invalid foreign pointers and concurrent destruction remain caller undefined behavior, as documented in the C header.

## Credentials and attachments

Only `CredentialRef` values cross the core. Secret bytes are resolved inside an authorized provider and must never enter failures, metrics, manifests, or event data. Attachments are metadata-only in this crate; an embedding store must verify media, digest, size, authorization, and retention.

## Known release-candidate limits

There is no remote authenticated API, OS sandbox backend, descendant process-group broker, network policy backend, encrypted store, KMS/keychain provider, WASM host, or durable job service. Those capabilities fail closed or are absent; they must not be advertised as supplied by the local RC.
