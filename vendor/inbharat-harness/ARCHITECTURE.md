# Architecture

## Trust boundary

The trusted computing base is deliberately fixed: router, route policy, budgets, cancellation, session state machine, event validation, permission/confirmation ordering, tool dispatch, and recovery. Replaceable providers cannot widen a denial.

```text
CLI / C ABI / tests
        |
        v
Router -> L0 direct | L1 one action | L2 finite loop | L3 goal loop
        |                    |
        +---- session log ---+
                 |
 validate -> authorize -> confirm -> budget -> sandbox -> execute
                 |                                      |
          provider registry                       execution broker
```

## Dependency direction

- `core` depends only on `std`.
- `cli` and `ffi` depend on `core`.
- `ffi` translates stable C values and contains no routing or policy logic.
- No crate depends on Node.js, a model vendor, a network client, a database, or a plugin runtime.

## Execution levels

- **L0:** one model request, no dynamic tools, no agent continuation.
- **L1:** a strict anchored command grammar dispatches exactly one tool. The model loop is never initialized.
- **L2:** one finite turn with bounded steps, tool calls, retries, time, and output.
- **L3:** the same auditable state machine plus goal, workspace, round, job, and subagent budgets. It never silently downgrades to unrestricted L1.

## Provider seams

`ModelProvider`, `MemoryProvider`, `SafetyProvider`, `PermissionProvider`, `ConfirmationProvider`, `VerificationProvider`, `SandboxProvider`, and `CredentialProvider` are synchronous `Send + Sync` traits. Synchronous contracts keep the core runtime-free and make the C ABI practical; implementations may internally bridge to async I/O. Cancellation is an explicit argument.

`ModelRegistry::prepare` captures the exact provider registration and immutable request snapshot. `PreparedModelCall` permits one dispatch only.

## Tools and authority

Every tool manifest fixes id, semantic version, schemas, required capabilities, supported levels, determinism, side effect, confirmation, concurrency, timeout, output bound, verification, and compensation statement. Model-visible tools are regenerated for each request from the level and capability set.

Dispatch order is fixed:

1. resolve visibility;
2. validate arguments;
3. authorize each capability;
4. obtain one-shot confirmation when needed;
5. reserve budget;
6. resolve the same execution world through the sandbox provider;
7. execute with cancellation;
8. bound canonical/model output;
9. verify postconditions;
10. append correlated result events.

## Session model

The format-v1 JSONL envelope contains format, session id, contiguous sequence, time, type, event version, replay requirement, previous checksum, checksum, and typed data. Accepted events are appended to storage before being published in memory. Turn, step, attempt, request, model chunks, assembled messages, tool calls/results, verification, failure, cancellation, recovery, job, attachment, and credential-reference facts are explicit.

Crash repair never blindly reruns a started tool. It writes `TOOL_OUTCOME_UNKNOWN`, closes the step, and closes the turn. Fork accepts only a completed-turn boundary under an expected source revision, and imports source transcript as explicitly untrusted context.

## Concurrency and ownership

The first candidate uses blocking provider calls and OS threads for jobs. Every background job has one owner, token, state, and join handle. Cancellation is first-cause-wins and a job is not reported stopped until joined. One-shot subagents must narrow parent capabilities and remain within depth/output limits.

## Compatibility

Session format, event version, manifest version, and C ABI start at version 1. Unknown required event types fail loud. Public C symbols contain `_v1`; structs start with `struct_size`; all handles and byte buffers have one documented owner.
