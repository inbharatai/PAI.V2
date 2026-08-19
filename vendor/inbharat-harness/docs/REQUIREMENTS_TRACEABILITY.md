# Requirements Traceability

| Requirement | Implementation | Evidence |
|---|---|---|
| L0/L1/L2/L3 routing and escalation | `routing.rs`, `runtime.rs` | `false_activation`, `routing_and_tools` |
| Model-neutral streaming | `providers.rs` registry/echo/mock/prepared call | provider unit + integration tests |
| Capability manifests/tool exposure | `tools.rs` | exposure unit + mock tool-loop test |
| Memory/safety/permission/confirmation/verifier/sandbox traits | `providers.rs` | compile-time implementations and policy tests |
| Append-only events/trajectories | `session.rs`, `TrajectoryMode` | session/mode tests |
| Cancellation/failures/recovery | `cancel.rs`, `error.rs`, runtime retry, repair | cancellation/recovery tests |
| L1 single action | `parse_l1`, `run_l1` | L1 exact-one test |
| L2 finite loop/L3 goal loop | `run_model_loop`, `run_goal_loop`, `BudgetLimits` | tool-loop and routing tests |
| Controlled filesystem/process | `execution.rs`, built-in tools | traversal/symlink/allowlist/output-timeout/process-cancel tests |
| Jobs/subagents | `jobs.rs` | job unit + scoped subagent test |
| Resume/fork/replay | `SessionStore`/`Session` | session lifecycle tests |
| Attachments/credentials/metrics | metadata/ref types and `Metrics` | provider/mode tests |
| CLI commands | CLI `app.rs`, benchmark, demo | `scripts/smoke.sh` |
| Hundreds of ordinary prompts | 600-prompt generated confusion set | integration test + benchmark JSON |
| Versioned C ABI | `crates/ffi` | create/route/run/cancel/free/destroy unit test, header, symbol manifest |
| Secure defaults/examples/docs/scripts | root docs, examples, scripts | release evidence |
