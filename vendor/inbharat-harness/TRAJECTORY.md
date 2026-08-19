# Trajectory

Trajectory detail scales with execution level and caller-selected `TrajectoryMode`.

| Level | Default retained detail |
|---|---|
| L0 | Route, turn, request header, assembled assistant result, terminal status; no tools |
| L1 | Intent, one proposed action, authorization/confirmation, result, verification |
| L2 | Finite model steps, task-specific tool calls/results, failures, recovery, completion |
| L3 | Goal rounds, workspace actions, tool sequence, verification, recovery, jobs/subagents when used |

`Minimal` suppresses nonessential model chunks. `Standard` keeps operationally useful events. `Diagnostic` retains bounded stream detail. Budgets cap steps, tool calls, rounds, jobs, output bytes, duration, and subagent depth regardless of trajectory mode.

Model-visible information is represented by durable session facts. Event sequence numbers and checksums allow replay to detect torn tails and corruption. Ordinary conversation does not initialize the agent loop or emit agent/tool events.
