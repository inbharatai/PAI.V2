# Changelog

## 0.1.0-rc.2

- Preserved bounded prior conversation history across resumed turns.
- Restored real release-mode C ABI panic containment by using unwind semantics.
- Rejected redundant authority escalation and made L0 model requirements explicit.
- Validated provider model catalogues, model requests, tool schemas, run options, and retry ceilings.
- Added a hard model-stream chunk-count limit and deterministic malformed-JSON campaign.
- Flattened cancellation ancestry to avoid recursive stack exhaustion and duration overflow.
- Pinned allowlisted subprocess executables at broker creation and validated environment entries.
- Added Windows and Android Rust cross-checks plus expanded security regression coverage.

## 0.1.0-rc.1

- Initial independent Rust workspace.
- Deterministic L0/L1/L2/L3 routing and monotonic escalation.
- Provider-neutral echo/mock/request-id replay streaming and prepared-call registry.
- Capability manifests, dynamic tools, bounded loops, cancellation, recovery, and structured failures.
- Format-v1 append-only sessions with JSONL persistence, resume, fork, replay, and repair.
- Rooted filesystem, allowlisted subprocess broker, jobs, scoped subagents, attachments, credential refs, and metrics.
- CLI, routing/session-churn benchmark, demonstration website generator, examples, scripts, documentation, and versioned cancellable C ABI.
