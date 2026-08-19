# Tools, Filesystem, and Sandbox

Tool manifests are immutable and validated on registration. Duplicate ids fail; no owner can shadow another registration silently. Visibility is the intersection of execution level and session capability set, and the model receives only that generated snapshot.

The dispatch pipeline is non-bypassable: resolve, validate, authorize, confirm, budget, sandbox, execute, bound, verify, persist. Canonical output, model presentation, and replay metadata are separate.

Built-ins:

- `fs.read`: bounded UTF-8 read;
- `fs.list`: one stable bounded directory listing;
- `fs.write`: bounded sync-and-rename atomic replacement;
- `process.run`: exact allowlisted executable with direct string argv, null stdin, cleared environment, concurrent bounded pipe drains, output limit, deadline, and direct-child cancellation kill/wait.

`RootedFs` denies absolute and parent components, checks canonical target/parent ancestry, and creates directory trees one validated component at a time. It is an in-process fence with a documented TOCTOU residual, not an OS sandbox.

`SandboxProvider` must return the same `world_id` as `ExecutionBroker`; filesystem and process capabilities cannot accidentally point at different worlds. Process/network effects require more than an in-process fence. The RC has no automatic unrestricted fallback and no descendant process-group guarantee; do not allowlist programs that daemonize.
