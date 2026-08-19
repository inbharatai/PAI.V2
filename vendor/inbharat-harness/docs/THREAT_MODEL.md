# Threat Model

## Assets

- user workspace contents and integrity;
- model/provider credentials and attachment bytes;
- session confidentiality, ordering, and replay integrity;
- host CPU, memory, process table, and filesystem;
- authority decisions and confirmation audit trail.

## Trust domains

The Rust core/router/policy/session state machine is trusted. Prompts, model chunks, tool arguments, imported transcript text, attachment metadata, provider responses, subprocess output, future RPC peers, and future plugins are untrusted. Local provider implementations are privileged code but receive only explicit request values.

## Principal threats and controls

| Threat | RC control | Residual |
|---|---|---|
| Ordinary text activates an expensive agent | anchored deterministic rules; 600-prompt zero-activation gate | new grammar requires new confusion cases |
| Tool gains hidden authority | manifest capability intersection; monotonic permission; one-shot confirmation | privileged provider implementation remains trusted |
| Path/symlink escape | lexical rejection; canonical ancestor checks; component-wise mkdir | cross-process TOCTOU in shared writable trees |
| Shell injection | no shell; exact executable allowlist; direct argv | the executable may interpret dangerous arguments |
| Output/pipe exhaustion | byte limits; concurrent drain/discard; timeout/cancel | child descendants are not group-owned in RC1 |
| Infinite agent/retry | core step/tool/round/time/output limits; bounded attempts | blocking third-party provider must honor cancellation contract |
| Crash after side effect | checkpoint before dispatch; synthesize unknown outcome; never blind retry | external effect reconciliation is provider-specific |
| Approval ambiguity | asked/decided event pair; unique ids; unavailable on recovery | interactive identity assurance belongs to embedding UI |
| Secret disclosure | credential references only; no raw telemetry; bounded redacted failures | users can still type secrets into prompts |
| Log modification | contiguous sequence and FNV chain | no adversarial cryptographic authenticity or encryption |
| FFI memory unsafety | size tags, opaque handles, explicit free, panic containment | invalid foreign pointers remain caller undefined behavior |

## Out of scope for local RC1

Hostile multi-user execution, arbitrary untrusted binaries, network egress enforcement, descendant process groups, remote authentication, plugin signatures, WASM isolation, encrypted persistence, and KMS/keychain storage are not supplied. Deployments needing them must add a security-grade broker/provider and keep fail-closed behavior.
