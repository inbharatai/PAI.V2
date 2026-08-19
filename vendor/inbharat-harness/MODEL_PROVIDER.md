# Provider Contracts

## Models

A provider exposes a stable id, advisory model list, and `stream` operation. The request snapshot separates provider route from model id and contains immutable messages, system text, dynamic tools, attachment metadata, and output bound. Indexed chunks support text, reasoning, tool calls, usage, and terminal finish.

The registry captures the exact provider and request in `PreparedModelCall`; one prepared call dispatches once. Provider failures use core error codes, retryability, retry-after guidance, and attempt identity. Caller cancellation remains attached through every call. `ReplayModelProvider` is a first-class diagnostic provider keyed by explicit recorded request ids, never fragile first-call order.

## Cross-cutting providers

- `MemoryProvider`: bounded scoped recall and remember; imported text remains untrusted.
- `SafetyProvider`: allow, narrow, or deny input/level; cannot grant authority.
- `PermissionProvider`: allow, ask, or deny one actor/capability/resource tuple.
- `ConfirmationProvider`: allowed-once, denied, or unavailable; unavailable fails closed.
- `VerificationProvider`: deterministic postcondition check over canonical arguments/output.
- `SandboxProvider`: binds requested capabilities to the exact execution-world id and reports full, partial, or in-process-fence enforcement.
- `CredentialProvider`: resolves a scoped `CredentialRef` directly to an authorized consumer; secret bytes are never session data.

Provider implementations are `Send + Sync`. They may bridge to async systems internally, but the core contract remains runtime-independent and cancellation-aware.
