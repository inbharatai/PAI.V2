# Memory provider

Memory is an optional provider, not a built-in permanent-personal-memory database. The trusted core defines provider-neutral records and never grants memory access implicitly.

## Scopes

`MemoryScope` supports `none`, `conversation`, `preferences`, `relevant`, `project`, `document`, and `extended`. `none` is a deliberate no-memory mode and cannot contain records. Providers advertise supported scopes and operations through `MemoryCapabilities`.

## Operations

`MemoryProvider` exposes:

- `capabilities`
- `retrieve`
- `search`
- `store`
- `update`
- `delete`

Records carry a portable identifier, scope, namespace, bounded UTF-8 content, and bounded attributes. Queries are bounded by scope, optional namespace, text size, and result count. Storage, retention, ranking, encryption, consent, redaction, and access policy remain provider responsibilities.

## Standalone provider

`InMemoryMemoryProvider` is a deterministic, thread-safe provider for tests and embedding examples. It implements the complete lifecycle and conflict/not-found behavior but is intentionally non-durable. Production deployments should supply a provider with explicit privacy, retention, encryption, and deletion guarantees.

## Security rules

- Never store credentials or secret bytes in `MemoryRecord`.
- Memory providers cannot widen a run's capability set.
- Callers select scope explicitly; no automatic permanent memory is enabled.
- Provider errors use the structured harness failure vocabulary.
- Search results are bounded and returned in deterministic provider order.
