# Failure and Recovery Model

`Failure` contains a stable `ErrorCode`, ownership class, bounded operation/message, retryability, optional retry-after, one-based attempt, and at most 16 bounded non-secret detail fields.

Classes separate user input, policy, resources, providers, execution, persistence, and internal ownership. Codes cover invalid input, route/permission/confirmation denial, missing capabilities, budgets, cancellation, timeout, provider/tool/verifier failure, sandbox/filesystem/process denial, corrupt sessions, exhausted recovery, conflicts, missing objects, and internal faults.

Only provider failures explicitly marked retryable are retried. Attempts are bounded by `RunOptions::recovery_attempts`; a logical step is budgeted once while each attempt has its own durable `StepStart`/request/failure record. Tool effects are never automatically retried.

Cancellation uses first-cause-wins tokens (`user`, `parent`, `deadline`, `policy`, `shutdown`, `disposed`). Started subprocesses are killed and waited; jobs are cancelled and joined. The RC does not promise descendant process-group termination.

Crash recovery pairs unanswered approvals with `unavailable`, pairs started tools with synthesized `TOOL_OUTCOME_UNKNOWN`, then closes step and turn. Every synthesized record counts against the recovery bound. UIs must present synthesized outcomes as uncertainty, not success or confirmed failure.
