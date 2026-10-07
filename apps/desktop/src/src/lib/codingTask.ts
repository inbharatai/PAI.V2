/**
 * Stage 5 "Coding Task" — typed IPC wrappers, view types and pure helpers.
 *
 * The types mirror the serde shapes of the adapter's `CodingTaskService`
 * views (design §8.1, owner A `coding_task.rs` / `task_ledger.rs`), owner B
 * `task_diff.rs` (FileDiff, FileDecision) and owner C `task_preview.rs`
 * (LogChunk, HttpCheckRecord). Every enum below is serde `snake_case`:
 * unit variants are strings, data variants are `{ variant: { ...fields } }`,
 * except `IsolationCapability` (`tag = "state"`) and B's `FileDecision`
 * (`tag = "kind"`).
 *
 * Commands are the FROZEN §8.2 surface (owner C glue). Top-level argument
 * keys are converted snake_case → camelCase exactly like `src/lib/tauri.ts`
 * (Tauri 2 maps camelCase JS keys to snake_case Rust parameters). Nested
 * values keep their serde (snake_case) field names.
 *
 * Nothing here interprets model text. Outcome wording is derived ONLY from
 * the server's `TaskOutcome` fields; narrative journal entries are
 * `untrusted_model_text` and are never read by any helper below.
 */
import { invoke as tauriInvoke } from '@tauri-apps/api/core';

// ---------------------------------------------------------------- IPC plumbing

// IDENTICAL to the conversion in src/lib/tauri.ts (whose `invoke` helper is
// module-private; tauri.ts is deliberately not edited in Stage 5). The
// mounted test suite asserts the two function bodies stay byte-identical.
function snakeToCamelKey(key: string): string {
  return key.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());
}

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const converted = args
    ? Object.fromEntries(Object.entries(args).map(([k, v]) => [snakeToCamelKey(k), v]))
    : args;
  return tauriInvoke<T>(command, converted);
}

/** The frozen §8.2 command names (owner C, `coding_task_commands.rs`). */
export const CODING_TASK_COMMANDS = {
  capability: 'coding_task_capability',
  list: 'coding_task_list',
  open: 'coding_task_open',
  view: 'coding_task_view',
  fileDiff: 'coding_task_file_diff',
  confirmPlan: 'coding_task_confirm_plan',
  runGate: 'coding_task_run_gate',
  reviewFile: 'coding_task_review_file',
  revertFile: 'coding_task_revert_file',
  apply: 'coding_task_apply',
  revertApplied: 'coding_task_revert_applied',
  startPreview: 'coding_task_start_preview',
  stopPreview: 'coding_task_stop_preview',
  previewLogs: 'coding_task_preview_logs',
  httpChecks: 'coding_task_http_checks',
  resolveInterrupted: 'coding_task_resolve_interrupted',
  resume: 'coding_task_resume',
  cancel: 'coding_task_cancel',
  exportPatch: 'coding_task_export_patch',
} as const;

/** §8.2 event, emitted after every state-changing command; the payload carries no content. */
export const CODING_TASK_UPDATED_EVENT = 'unoone:coding-task-updated';
export interface CodingTaskUpdatedPayload {
  task_id: string;
  /** `null` when the command returned something other than a TaskView (apply, start preview, cancel…). */
  view_seq: number | null;
}

// ---------------------------------------------------------------- view types

export type IsolationCapability =
  | { state: 'unsupported'; reason: string }
  | { state: 'supported_unverified' }
  | { state: 'runtime_verified'; workspace_profile_sha256: string; probed_at_ms: number };

export type LabelSource = 'git_files_unverified' | 'user_label' | 'unknown';
export interface RepositoryLabel {
  display_root: string;
  source_id: string;
  branch: string | null;
  head_commit: string | null;
  label_source: LabelSource;
}

export type StopReason = 'budget_exhausted' | 'no_progress' | 'infrastructure' | 'oracle_denied';
export type TaskStatus =
  | 'open'
  | 'running'
  | 'awaiting_review'
  | { stopped: { reason: StopReason } }
  | { paused: { reason: 'review_required' } }
  | 'applied'
  | 'rejected'
  | 'cancelled'
  | 'closed';

export interface TaskSummary {
  task_id: string;
  objective_excerpt: string;
  status: TaskStatus | null;
  created_at_ms: number;
  view_seq: number;
  readable: boolean;
}

export type OracleVisibility = 'visible' | 'hidden';
export interface OracleDerivation {
  declared: string[];
  derived: string[];
  implementation_closure: string[];
  protected: string[];
}

export type CriterionCheck =
  | { gate_command: { command_id: string; expected_exit: number } }
  | { http: { check_id: string } }
  | 'manual';
export interface AcceptanceCriterion {
  id: string;
  text: string;
  check: CriterionCheck;
  confirmed_by_user: boolean;
}

export type PlanAuthor = 'user' | 'model';
export type StepKind =
  | 'capture' | 'edit' | 'gate' | 'repair' | 'preview_start' | 'http_check'
  | 'preview_stop' | 'review' | 'apply' | 'revert_applied';
export type EffectClass = 'ledger_only' | 'pure' | 'process_lifecycle' | 'host_write';
export interface PlannedStep {
  step_id: string;
  kind: StepKind;
  summary: string;
  effect: EffectClass;
}
export interface Plan {
  revision: number;
  author: PlanAuthor;
  steps: PlannedStep[];
  confirmed: boolean;
}

export type StepState =
  | 'planned' | 'started' | 'completed' | 'failed' | 'interrupted'
  | 'completed_by_observation' | 'abandoned' | 'skipped';
export type FailureReason =
  | 'infrastructure'
  | 'cancelled'
  | 'proposer_failed'
  | 'preview_startup_failed'
  | { worktree: { error: string } }
  | { pre_image_mismatch: { path: string } };
export interface StepView {
  step_id: string;
  attempt: number;
  state: StepState;
  effect: EffectClass;
  failure: FailureReason | null;
}
export interface JournalView {
  seq: number;
  at_ms: number;
  event: string;
  untrusted_model_text: boolean;
  excerpt: string | null;
}

export type ToolStatus = 'ok' | { partial: { failed: number } } | 'not_run';
export type CheckErrorKind = 'runner_failure' | 'timeout' | 'output_limit' | 'cancelled';
export type CheckStatus =
  | 'not_run'
  | { passed: { gate: string } }
  | { failed: { gate: string; command: string; exit: number | null } }
  | { error: { gate: string; kind: CheckErrorKind } }
  | { stale: { gate: string } };
export type HttpStatus = 'passed' | 'failed' | 'not_run' | 'stale';
export type PreviewStatus =
  | 'not_applicable'
  | 'not_started'
  | 'startup_failed'
  | { ready: { http: HttpStatus } }
  | 'stopped';
export type BrowserStatus = 'not_verified_by_product';
export type GoalStatus =
  | 'unverified'
  | { unmet: { criteria: string[] } }
  | 'checks_passed_pending_review'
  | 'accepted'
  | 'rejected';
export type ReviewStatus =
  | { pending: { n: number } }
  | { decided: { accepted: number; rejected: number } }
  | { stale: { n: number } };
export type ApplyStatus =
  | 'not_applied'
  | { applied: { files: string[]; worktree: string } }
  | 'interrupted'
  | 'reverted';
export interface Risk {
  id: string;
  summary: string;
}
export interface TaskOutcome {
  tool_status: ToolStatus;
  build_status: CheckStatus;
  test_status: CheckStatus;
  preview_status: PreviewStatus;
  browser_status: BrowserStatus;
  goal_status: GoalStatus;
  review_status: ReviewStatus;
  apply_status: ApplyStatus;
  unresolved_risks: Risk[];
}

export type Change = 'added' | 'modified' | 'deleted';
/** Owner B's real `FileDecision` (`#[serde(tag = "kind")]`). */
export type FileDecision =
  | { kind: 'pending' }
  | { kind: 'accepted' }
  | { kind: 'rejected' }
  | { kind: 'partially_accepted'; hunks: number[]; composed_sha256: string };
export type DecisionKind = FileDecision['kind'];
export interface DiffSummary {
  path: string;
  change: Change;
  base_sha256: string | null;
  new_sha256: string | null;
  hunk_count: number;
  binary: boolean;
  truncated: boolean;
  /** B's tagged shape; A's Pass-1 stand-in shape is also decoded (see decisionKind). */
  decision: FileDecision | unknown;
  stale: boolean;
}

export type Termination = 'completed' | 'timeout' | 'output_limit' | 'cancelled' | 'runner_failure';
export type GateRole = 'build' | 'test' | 'lint' | 'oracle';
export interface CommandSummary {
  id: string;
  role: GateRole;
  argv: string[];
  status: number | null;
  termination: Termination;
  stdout_total_bytes: number;
  stderr_total_bytes: number;
  stdout_retained_bytes: number;
  stderr_retained_bytes: number;
  truncated: boolean;
  log_sha256: string;
  excerpt: string;
}
export interface BlobRef {
  kind: string;
  sha256: string;
  size: number;
}
export interface GateRecord {
  gate_run_id: string;
  working_set_sha256: string;
  plan_sha256: string;
  workspace_profile_sha256: string;
  commands: CommandSummary[];
  logs: BlobRef[];
  termination: Termination;
  elapsed_ms: number;
  at_ms: number;
}
export type GateTarget = 'current' | 'accepted_composition';

export type StartupFailure =
  | 'timeout'
  | { exited: { status: number | null } }
  | { http: { status: number } }
  | 'runner_failure';
export type ReadyState =
  | { ready: { after_ms: number } }
  | { startup_failed: { reason: StartupFailure } };
export interface ServiceDescriptor {
  service_id: string;
  task_id: string;
  tree_sha256: string;
  spec_sha256: string;
  workspace_profile_sha256: string;
  bridge_port: number;
  started_at_ms: number;
  ready: ReadyState;
}
export type EvidenceLevel = 'http_level' | 'browser_level' | 'manual';
export interface HttpCheckResult {
  id: string;
  status: number | null;
  passed: boolean;
  body_sha256: string | null;
  excerpt: string;
  elapsed_ms: number;
  failure: string | null;
}
export interface HttpCheckRecord {
  tree_sha256: string;
  service_id: string;
  evidence_level: EvidenceLevel;
  results: HttpCheckResult[];
  at_ms: number;
}
export interface PreviewView {
  status: PreviewStatus;
  descriptor: ServiceDescriptor | null;
  /** Only while running; the glue returns it to the main window only. */
  capability_url: string | null;
  http_checks: HttpCheckRecord | null;
  evidence_label: string;
  browser: BrowserStatus;
}

export type ReconcileObservation =
  | 'not_applicable'
  | 'process_not_owned'
  | { matches_post_image: { hashes: Record<string, string | null> } }
  | 'matches_pre_image'
  | { unknown: { per_path: Record<string, string | null>; identity_ok: boolean } };
export type ReviewResolution =
  | 'abandon'
  | 'retry_as_new_attempt'
  | 'confirm_observation'
  | 'mark_manually_resolved'
  | 'restore_pre_image';
export interface ReconcileItem {
  step_id: string;
  attempt: number;
  effect: EffectClass;
  observation: ReconcileObservation | null;
  options: ReviewResolution[];
}

export interface TaskView {
  task_id: string;
  view_seq: number;
  status: TaskStatus;
  objective: string;
  repository: RepositoryLabel;
  capability: IsolationCapability;
  oracle_visibility: OracleVisibility;
  oracle: OracleDerivation;
  acceptance: AcceptanceCriterion[];
  plan: Plan | null;
  steps: StepView[];
  journal_tail: JournalView[];
  outcome: TaskOutcome;
  diff: DiffSummary[];
  gates: GateRecord[];
  preview: PreviewView;
  reconciliation: ReconcileItem[];
  change_set_sha256: string;
  risks_sha256: string;
  residuals: string[];
}

export type LineTag = 'context' | 'delete' | 'insert';
export interface DiffLine {
  tag: LineTag;
  text: string;
}
export interface Hunk {
  index: number;
  old_start: number;
  old_len: number;
  new_start: number;
  new_len: number;
  lines: DiffLine[];
  hunk_sha256: string;
}
export interface FileDiff {
  path: string;
  change: Change;
  base_sha256: string | null;
  new_sha256: string | null;
  binary: boolean;
  hunks: Hunk[];
  unified: string;
  truncated: boolean;
  timed_out: boolean;
}

export type LogStream = 'stdout' | 'stderr';
export interface LogRecord {
  seq: number;
  at_ms?: number;
  stream: LogStream;
  /** C's `task_preview` serializes bytes as lowercase hex; a byte array is also decoded. */
  bytes: string | number[];
}
export interface LogChunk {
  records: LogRecord[];
  next_cursor: number;
  truncated_before_cursor?: boolean;
  first_retained_seq: number;
  retained_bytes?: number;
  cap_bytes?: number;
  dropped_bytes: number;
  dropped_records: number;
  supervisor_dropped_bytes: number;
}

export interface WorktreeBinding {
  path: string;
  dev: number;
  ino: number;
  created_at_ms: number;
}
export interface ApplyReport {
  step_id: string;
  files: string[];
  worktree: WorktreeBinding;
  post_image: Record<string, string | null>;
}

// ------------------------------------------------------------- UI events (§8.1)

export interface UiPlanConfirmEvent { task_id: string; view_seq: number; revision: number }
export interface UiReviewEvent {
  task_id: string;
  view_seq: number;
  path: string;
  decision: FileDecision;
  displayed_base_sha256: string | null;
  displayed_new_sha256: string | null;
}
export interface UiRevertEvent {
  task_id: string;
  view_seq: number;
  path: string;
  displayed_new_sha256: string | null;
}
export interface UiApplyEvent {
  task_id: string;
  view_seq: number;
  displayed_change_set_sha256: string;
  displayed_risks_sha256: string;
  acknowledged_risk_ids: string[];
}
export interface UiRevertAppliedEvent { task_id: string; view_seq: number }
export interface UiPreviewEvent { task_id: string; view_seq: number }
export interface UiReconcileEvent {
  task_id: string;
  view_seq: number;
  step_id: string;
  attempt: number;
  resolution: ReviewResolution;
}
export interface UiResumeEvent { task_id: string; view_seq: number }

/** `OpenTaskRequest` (adapter; nested values keep serde snake_case). */
export interface OpenTaskRequest {
  root: string;
  files: string[];
  primary: string;
  oracle_files: string[];
  oracle_visibility: OracleVisibility;
  objective: string;
  acceptance: AcceptanceCriterion[];
  gate_plan: unknown;
  preview: unknown | null;
  repair: { max_attempts: number; max_total_gate_ms: number };
  allowed_new_prefixes: string[];
}

// Struct-taking commands (§8.2 "Args" column lists a struct): owner C's glue
// (`coding_task_commands.rs`) takes ONE parameter per struct — `event: Ui*Event`
// and `request: OpenTaskRequest` — so the struct is sent under that top-level
// key and its own fields keep their serde snake_case names
// (`deny_unknown_fields` on the Rust side). Recorded as an interface
// assumption in the Stage 5 B-UI handoff.
const asEvent = (event: object): Record<string, unknown> => ({ event });

/** Typed wrappers over the frozen §8.2 surface. */
export const codingTaskApi = {
  capability: () => invoke<IsolationCapability>(CODING_TASK_COMMANDS.capability),
  list: () => invoke<TaskSummary[]>(CODING_TASK_COMMANDS.list),
  open: (request: OpenTaskRequest) => invoke<TaskView>(CODING_TASK_COMMANDS.open, { request }),
  view: (taskId: string) => invoke<TaskView>(CODING_TASK_COMMANDS.view, { task_id: taskId }),
  fileDiff: (taskId: string, path: string) =>
    invoke<FileDiff>(CODING_TASK_COMMANDS.fileDiff, { task_id: taskId, path }),
  confirmPlan: (ev: UiPlanConfirmEvent) => invoke<TaskView>(CODING_TASK_COMMANDS.confirmPlan, asEvent(ev)),
  runGate: (taskId: string, target: GateTarget) =>
    invoke<TaskView>(CODING_TASK_COMMANDS.runGate, { task_id: taskId, target }),
  reviewFile: (ev: UiReviewEvent) => invoke<TaskView>(CODING_TASK_COMMANDS.reviewFile, asEvent(ev)),
  revertFile: (ev: UiRevertEvent) => invoke<TaskView>(CODING_TASK_COMMANDS.revertFile, asEvent(ev)),
  apply: (ev: UiApplyEvent) => invoke<ApplyReport>(CODING_TASK_COMMANDS.apply, asEvent(ev)),
  revertApplied: (ev: UiRevertAppliedEvent) =>
    invoke<ApplyReport>(CODING_TASK_COMMANDS.revertApplied, asEvent(ev)),
  startPreview: (ev: UiPreviewEvent) => invoke<PreviewView>(CODING_TASK_COMMANDS.startPreview, asEvent(ev)),
  stopPreview: (taskId: string) => invoke<TaskView>(CODING_TASK_COMMANDS.stopPreview, { task_id: taskId }),
  previewLogs: (taskId: string, cursor: number, limit: number) =>
    invoke<LogChunk>(CODING_TASK_COMMANDS.previewLogs, { task_id: taskId, cursor, limit }),
  httpChecks: (taskId: string) => invoke<TaskView>(CODING_TASK_COMMANDS.httpChecks, { task_id: taskId }),
  resolveInterrupted: (ev: UiReconcileEvent) =>
    invoke<TaskView>(CODING_TASK_COMMANDS.resolveInterrupted, asEvent(ev)),
  resume: (ev: UiResumeEvent) => invoke<TaskView>(CODING_TASK_COMMANDS.resume, asEvent(ev)),
  cancel: (taskId: string) => invoke<null>(CODING_TASK_COMMANDS.cancel, { task_id: taskId }),
  exportPatch: (taskId: string) => invoke<string>(CODING_TASK_COMMANDS.exportPatch, { task_id: taskId }),
};

// ---------------------------------------------------------------- enum helpers

/** Variant name of a serde externally-tagged enum value (`"x"` or `{ x: {...} }`). */
export function variantOf(value: unknown): string {
  if (typeof value === 'string') return value;
  if (value && typeof value === 'object') {
    const keys = Object.keys(value as Record<string, unknown>);
    if (keys.length === 1) return keys[0];
  }
  return 'unknown';
}
/** Payload of a serde externally-tagged data variant. */
export function variantData<T = Record<string, unknown>>(value: unknown, variant: string): T | null {
  if (value && typeof value === 'object' && variant in (value as Record<string, unknown>)) {
    return (value as Record<string, T>)[variant];
  }
  return null;
}

/**
 * Normalise a file decision. B's real type is `{ kind: ... }`; A's Pass-1
 * stand-in serialized externally tagged (`"accepted"` / `{ partially_accepted: {...} }`).
 * Anything unrecognised is treated as pending (fail closed for Apply).
 */
export function normalizeDecision(value: unknown): FileDecision {
  if (value && typeof value === 'object' && 'kind' in (value as Record<string, unknown>)) {
    const v = value as { kind: unknown; hunks?: unknown; composed_sha256?: unknown };
    if (v.kind === 'accepted' || v.kind === 'rejected' || v.kind === 'pending') return { kind: v.kind };
    if (v.kind === 'partially_accepted' && Array.isArray(v.hunks) && typeof v.composed_sha256 === 'string') {
      return { kind: 'partially_accepted', hunks: v.hunks as number[], composed_sha256: v.composed_sha256 };
    }
    return { kind: 'pending' };
  }
  const name = variantOf(value);
  if (name === 'accepted' || name === 'rejected' || name === 'pending') return { kind: name };
  if (name === 'partially_accepted') {
    const data = variantData<{ hunks: number[]; composed_sha256: string }>(value, 'partially_accepted');
    if (data && Array.isArray(data.hunks) && typeof data.composed_sha256 === 'string') {
      return { kind: 'partially_accepted', hunks: data.hunks, composed_sha256: data.composed_sha256 };
    }
  }
  return { kind: 'pending' };
}
export const decisionKind = (value: unknown): DecisionKind => normalizeDecision(value).kind;

export const isPaused = (status: TaskStatus) => variantOf(status) === 'paused';
export const isClosedStatus = (status: TaskStatus) =>
  ['applied', 'rejected', 'cancelled', 'closed'].includes(variantOf(status));

export const shortHash = (sha: string | null | undefined, n = 12) => (sha ? sha.slice(0, n) : '—');

export function formatTime(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return '—';
  return new Date(ms).toISOString().replace('T', ' ').slice(0, 19) + 'Z';
}

export function formatBytes(n: number): string {
  if (!Number.isFinite(n)) return '—';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
}

// ---------------------------------------------------------------- labels

export interface CapabilityInfo {
  state: 'unsupported' | 'supported_unverified' | 'runtime_verified' | 'unknown';
  label: string;
  detail: string;
  executionBlocked: boolean;
}
export function capabilityInfo(cap: IsolationCapability | null | undefined): CapabilityInfo {
  if (!cap) return { state: 'unknown', label: 'Isolation: checking…', detail: '', executionBlocked: true };
  switch (cap.state) {
    case 'unsupported':
      return { state: 'unsupported', label: 'Isolation: Unsupported', detail: cap.reason, executionBlocked: true };
    case 'supported_unverified':
      return {
        state: 'supported_unverified',
        label: 'Isolation: Supported, unverified',
        detail: 'The readiness probe runs before the first sandboxed run.',
        executionBlocked: false,
      };
    case 'runtime_verified':
      return {
        state: 'runtime_verified',
        label: 'Isolation: Runtime verified',
        detail: `profile ${shortHash(cap.workspace_profile_sha256)} · probed ${formatTime(cap.probed_at_ms)}`,
        executionBlocked: false,
      };
    default:
      return { state: 'unknown', label: 'Isolation: unknown state', detail: '', executionBlocked: true };
  }
}

export function repositoryBadge(repo: RepositoryLabel): string {
  switch (repo.label_source) {
    case 'git_files_unverified': {
      const branch = repo.branch ? `branch ${repo.branch}` : 'branch unknown';
      const head = repo.head_commit ? ` @ ${repo.head_commit.slice(0, 12)}` : '';
      return `${branch}${head} — from .git files — not verified`;
    }
    case 'user_label':
      return `${repo.branch ? `label ${repo.branch}` : 'label'} — your label — not verified`;
    default:
      return 'branch unknown';
  }
}

const STOP_REASON: Record<StopReason, string> = {
  budget_exhausted: 'repair budget exhausted',
  no_progress: 'no progress between attempts',
  infrastructure: 'infrastructure failure',
  oracle_denied: 'an edit to a protected test was denied',
};
export function taskStatusLabel(status: TaskStatus | null | undefined): string {
  if (status === null || status === undefined) return 'Unreadable';
  switch (variantOf(status)) {
    case 'open': return 'Open';
    case 'running': return 'Running';
    case 'awaiting_review': return 'Awaiting your review';
    case 'stopped': {
      const reason = variantData<{ reason: StopReason }>(status, 'stopped')?.reason;
      return `Stopped: ${reason ? STOP_REASON[reason] ?? reason : 'unknown reason'}`;
    }
    case 'paused': return 'Paused — your review of interrupted steps is required';
    case 'applied': return 'Applied to task worktree';
    case 'rejected': return 'Rejected';
    case 'cancelled': return 'Cancelled';
    case 'closed': return 'Closed';
    default: return 'Unknown status';
  }
}

export const STEP_STATE_LABEL: Record<StepState, string> = {
  planned: 'planned',
  started: 'started',
  completed: 'completed',
  failed: 'failed',
  interrupted: 'interrupted',
  completed_by_observation: 'completed-by-observation',
  abandoned: 'abandoned',
  skipped: 'skipped',
};
export const EFFECT_LABEL: Record<EffectClass, string> = {
  ledger_only: 'ledger only',
  pure: 'sandbox only',
  process_lifecycle: 'process lifecycle',
  host_write: 'host write',
};

export function failureLabel(f: FailureReason | null): string {
  if (!f) return '';
  switch (variantOf(f)) {
    case 'infrastructure': return 'infrastructure failure';
    case 'cancelled': return 'cancelled';
    case 'proposer_failed': return 'proposer failed';
    case 'preview_startup_failed': return 'preview startup failed';
    case 'worktree': return `worktree: ${variantData<{ error: string }>(f, 'worktree')?.error ?? 'error'}`;
    case 'pre_image_mismatch':
      return `pre-image mismatch: ${variantData<{ path: string }>(f, 'pre_image_mismatch')?.path ?? ''}`;
    default: return 'failure';
  }
}

export const RESOLUTION_LABEL: Record<ReviewResolution, string> = {
  confirm_observation: 'Confirm observation (no re-run)',
  retry_as_new_attempt: 'Retry as a new attempt',
  mark_manually_resolved: 'Mark manually resolved',
  restore_pre_image: 'Restore pre-image (worktree write)',
  abandon: 'Abandon step',
};

export function observationLabel(o: ReconcileObservation | null): string {
  if (!o) return 'No observation recorded.';
  switch (variantOf(o)) {
    case 'not_applicable':
      return 'Sandbox-only step: its result was not recorded and is never recycled.';
    case 'process_not_owned':
      return 'The managed process ended with the app; it is never re-owned by PID.';
    case 'matches_post_image': {
      const n = Object.keys(variantData<{ hashes: Record<string, unknown> }>(o, 'matches_post_image')?.hashes ?? {}).length;
      return `Worktree matches the intended post-image (${n} file${n === 1 ? '' : 's'}); nothing was written during reconciliation.`;
    }
    case 'matches_pre_image':
      return 'Worktree still matches the pre-image: the write did not happen.';
    case 'unknown': {
      const data = variantData<{ per_path: Record<string, unknown>; identity_ok: boolean }>(o, 'unknown');
      const n = Object.keys(data?.per_path ?? {}).length;
      return `Worktree state is unknown: ${n} path${n === 1 ? '' : 's'} match${n === 1 ? 'es' : ''} neither the pre-image nor the post-image; worktree identity ${data?.identity_ok ? 'unchanged' : 'changed'}.`;
    }
    default:
      return 'Unrecognised observation.';
  }
}

export function startupFailureLabel(f: StartupFailure | undefined | null): string {
  if (!f) return 'unknown reason';
  switch (variantOf(f)) {
    case 'timeout': return 'timed out waiting for readiness';
    case 'exited': {
      const status = variantData<{ status: number | null }>(f, 'exited')?.status;
      return status === null || status === undefined ? 'process killed before readiness' : `process exited (status ${status})`;
    }
    case 'http': return `readiness answered HTTP ${variantData<{ status: number }>(f, 'http')?.status ?? '?'}`;
    case 'runner_failure': return 'sandbox runner failure';
    default: return 'unknown reason';
  }
}

export function previewStatusLabel(preview: PreviewView): string {
  const status = preview.status;
  const ready = preview.descriptor ? variantData<{ after_ms: number }>(preview.descriptor.ready, 'ready') : null;
  switch (variantOf(status)) {
    case 'not_applicable': return 'No preview in this task';
    case 'not_started': return 'Not started';
    case 'startup_failed': {
      const failed = preview.descriptor
        ? variantData<{ reason: StartupFailure }>(preview.descriptor.ready, 'startup_failed')
        : null;
      return `Startup failed (${startupFailureLabel(failed?.reason)})`;
    }
    case 'ready': return ready ? `Ready after ${ready.after_ms} ms` : 'Ready';
    case 'stopped': return 'Stopped';
    default: return 'Unknown';
  }
}

// ---------------------------------------------------------------- outcome chips

export type ChipTone = 'ok' | 'bad' | 'warn' | 'neutral';
export interface OutcomeBadge {
  key: 'tool' | 'build' | 'test' | 'preview' | 'browser' | 'goal' | 'review' | 'apply';
  title: string;
  label: string;
  tone: ChipTone;
}

const CHECK_ERROR_LABEL: Record<CheckErrorKind, string> = {
  runner_failure: 'runner failure',
  timeout: 'timed out',
  output_limit: 'output limit hit',
  cancelled: 'cancelled',
};
function checkBadge(status: CheckStatus): { label: string; tone: ChipTone } {
  switch (variantOf(status)) {
    case 'not_run': return { label: 'Not run', tone: 'neutral' };
    case 'passed': {
      const gate = variantData<{ gate: string }>(status, 'passed')?.gate ?? '';
      return { label: `Passed (gate ${shortHash(gate, 8)})`, tone: 'ok' };
    }
    case 'failed': {
      const data = variantData<{ gate: string; command: string; exit: number | null }>(status, 'failed');
      const exit = data?.exit === null || data?.exit === undefined ? 'no exit code' : `exit ${data.exit}`;
      return { label: `Failed (${exit}) — ${data?.command ?? 'command'}`, tone: 'bad' };
    }
    case 'error': {
      const kind = variantData<{ kind: CheckErrorKind }>(status, 'error')?.kind;
      return { label: `Error: ${kind ? CHECK_ERROR_LABEL[kind] ?? kind : 'unknown'}`, tone: 'bad' };
    }
    case 'stale': return { label: 'Stale — content changed since the last run', tone: 'warn' };
    default: return { label: 'Unknown', tone: 'neutral' };
  }
}

/**
 * The outcome chips. Input is ONLY `TaskOutcome` (server-computed, never
 * model-written): no narrative, excerpt or journal text is read here.
 * There is deliberately no "done"/"implemented" wording; the strongest goal
 * label before the user's own acceptance is "Checks passed — awaiting your review".
 */
export function outcomeBadges(outcome: TaskOutcome): OutcomeBadge[] {
  const tool = (() => {
    switch (variantOf(outcome.tool_status)) {
      case 'ok': return { label: 'OK (tool calls only — not a build or goal result)', tone: 'neutral' as ChipTone };
      case 'partial': {
        const failed = variantData<{ failed: number }>(outcome.tool_status, 'partial')?.failed ?? 0;
        return { label: `${failed} tool call${failed === 1 ? '' : 's'} failed`, tone: 'bad' as ChipTone };
      }
      default: return { label: 'None run', tone: 'neutral' as ChipTone };
    }
  })();
  const preview = (() => {
    const status = outcome.preview_status;
    switch (variantOf(status)) {
      case 'not_applicable': return { label: 'Not part of this task', tone: 'neutral' as ChipTone };
      case 'not_started': return { label: 'Not started', tone: 'neutral' as ChipTone };
      case 'startup_failed': return { label: 'Startup failed', tone: 'bad' as ChipTone };
      case 'stopped': return { label: 'Stopped', tone: 'neutral' as ChipTone };
      case 'ready': {
        const http = variantData<{ http: HttpStatus }>(status, 'ready')?.http ?? 'not_run';
        const map: Record<HttpStatus, { label: string; tone: ChipTone }> = {
          passed: { label: 'Server ready · HTTP checks passed', tone: 'ok' },
          failed: { label: 'Server ready · HTTP checks failed', tone: 'bad' },
          not_run: { label: 'Server ready · HTTP checks not run', tone: 'neutral' },
          stale: { label: 'Server ready · HTTP checks stale', tone: 'warn' },
        };
        return map[http] ?? { label: 'Server ready', tone: 'neutral' as ChipTone };
      }
      default: return { label: 'Unknown', tone: 'neutral' as ChipTone };
    }
  })();
  const goal = (() => {
    const g = outcome.goal_status;
    switch (variantOf(g)) {
      case 'unverified': return { label: 'Unverified', tone: 'neutral' as ChipTone };
      case 'unmet': {
        const criteria = variantData<{ criteria: string[] }>(g, 'unmet')?.criteria ?? [];
        return { label: `Not met${criteria.length ? ` (${criteria.join(', ')})` : ''}`, tone: 'bad' as ChipTone };
      }
      case 'checks_passed_pending_review': return { label: 'Checks passed — awaiting your review', tone: 'warn' as ChipTone };
      case 'accepted': return { label: 'Accepted by you', tone: 'ok' as ChipTone };
      case 'rejected': return { label: 'Rejected by you', tone: 'neutral' as ChipTone };
      default: return { label: 'Unknown', tone: 'neutral' as ChipTone };
    }
  })();
  const review = (() => {
    const r = outcome.review_status;
    switch (variantOf(r)) {
      case 'pending': {
        const n = variantData<{ n: number }>(r, 'pending')?.n ?? 0;
        return { label: `${n} file${n === 1 ? '' : 's'} awaiting your decision`, tone: (n ? 'warn' : 'neutral') as ChipTone };
      }
      case 'decided': {
        const d = variantData<{ accepted: number; rejected: number }>(r, 'decided');
        return { label: `Decided: ${d?.accepted ?? 0} accepted, ${d?.rejected ?? 0} rejected`, tone: 'neutral' as ChipTone };
      }
      case 'stale': {
        const n = variantData<{ n: number }>(r, 'stale')?.n ?? 0;
        return { label: `${n} decision${n === 1 ? '' : 's'} reset — content changed`, tone: 'warn' as ChipTone };
      }
      default: return { label: 'Unknown', tone: 'neutral' as ChipTone };
    }
  })();
  const apply = (() => {
    const a = outcome.apply_status;
    switch (variantOf(a)) {
      case 'not_applied': return { label: 'Not applied', tone: 'neutral' as ChipTone };
      case 'applied': {
        const d = variantData<{ files: string[]; worktree: string }>(a, 'applied');
        const n = d?.files.length ?? 0;
        return { label: `Applied ${n} file${n === 1 ? '' : 's'} to task worktree ${d?.worktree ?? ''}`.trim(), tone: 'ok' as ChipTone };
      }
      case 'interrupted': return { label: 'Interrupted — see interrupted steps', tone: 'bad' as ChipTone };
      case 'reverted': return { label: 'Reverted', tone: 'neutral' as ChipTone };
      default: return { label: 'Unknown', tone: 'neutral' as ChipTone };
    }
  })();
  return [
    { key: 'tool', title: 'Tool ops', ...tool },
    { key: 'build', title: 'Build', ...checkBadge(outcome.build_status) },
    { key: 'test', title: 'Tests', ...checkBadge(outcome.test_status) },
    { key: 'preview', title: 'Preview (HTTP level)', ...preview },
    { key: 'browser', title: 'Browser rendering', label: 'Not verified by UnoOne — HTTP checks only', tone: 'neutral' },
    { key: 'goal', title: 'Goal', ...goal },
    { key: 'review', title: 'Your review', ...review },
    { key: 'apply', title: 'Apply', ...apply },
  ];
}

/** Acceptance criterion status, derived from server gate/HTTP records only. */
export function criterionStatus(view: TaskView, c: AcceptanceCriterion): { label: string; tone: ChipTone } {
  const check = c.check;
  if (variantOf(check) === 'manual') return { label: 'Manual — your judgement', tone: 'neutral' };
  if (variantOf(check) === 'gate_command') {
    const data = variantData<{ command_id: string; expected_exit: number }>(check, 'gate_command');
    if (!data) return { label: 'Not run', tone: 'neutral' };
    const staleGates = new Set(
      [view.outcome.build_status, view.outcome.test_status]
        .filter(s => variantOf(s) === 'stale')
        .map(s => variantData<{ gate: string }>(s, 'stale')?.gate ?? ''),
    );
    for (let i = view.gates.length - 1; i >= 0; i -= 1) {
      const gate = view.gates[i];
      const command = gate.commands.find(cmd => cmd.id === data.command_id);
      if (!command) continue;
      if (staleGates.has(gate.gate_run_id)) return { label: 'Stale', tone: 'warn' };
      if (command.status === null) {
        return { label: `Failed (no exit code: ${command.termination.replace('_', ' ')})`, tone: 'bad' };
      }
      return command.status === data.expected_exit && command.termination === 'completed'
        ? { label: `Passed (exit ${command.status})`, tone: 'ok' }
        : { label: `Failed (exit ${command.status})`, tone: 'bad' };
    }
    return { label: 'Not run', tone: 'neutral' };
  }
  if (variantOf(check) === 'http') {
    const id = variantData<{ check_id: string }>(check, 'http')?.check_id;
    const http = variantData<{ http: HttpStatus }>(view.preview.status, 'ready')?.http;
    const result = view.preview.http_checks?.results.find(r => r.id === id);
    if (!result) return { label: 'Not run', tone: 'neutral' };
    if (http === 'stale' || variantOf(view.preview.status) !== 'ready') return { label: 'Stale', tone: 'warn' };
    return result.passed
      ? { label: `Passed (HTTP ${result.status ?? '—'}, HTTP level only)`, tone: 'ok' }
      : { label: `Failed (HTTP ${result.status ?? 'no response'})`, tone: 'bad' };
  }
  return { label: 'Not run', tone: 'neutral' };
}

// ---------------------------------------------------------------- review / apply

/**
 * Paths whose shown decision must be treated as reset: the server flags them
 * `stale`, or the user decided at a content hash that is no longer current
 * (`decidedAt`: path -> new_sha256 the user saw when clicking).
 */
export function staleFiles(view: TaskView, decidedAt?: ReadonlyMap<string, string | null>): string[] {
  return view.diff
    .filter(d => d.stale || (decidedAt?.has(d.path) === true && decidedAt.get(d.path) !== d.new_sha256))
    .map(d => d.path);
}

/** The decision that counts: a stale decision is shown and treated as pending. */
export function effectiveDecision(d: DiffSummary, stale: ReadonlySet<string>): DecisionKind {
  return stale.has(d.path) ? 'pending' : decisionKind(d.decision);
}

export interface ApplyGate {
  ok: boolean;
  reasons: string[];
}
/** Apply is possible only when every changed file is (freshly) decided. */
export function canApply(view: TaskView, stale: ReadonlySet<string>): ApplyGate {
  const reasons: string[] = [];
  const cap = capabilityInfo(view.capability);
  if (cap.state === 'unsupported') reasons.push(`Apply unavailable on this system: ${cap.detail}`);
  if (isPaused(view.status)) reasons.push('Task is paused: resolve the interrupted steps first.');
  if (variantOf(view.status) === 'running') reasons.push('A run is in progress.');
  if (isClosedStatus(view.status)) reasons.push(`Task is ${taskStatusLabel(view.status).toLowerCase()}.`);
  if (view.reconciliation.some(item => item.options.length > 0)) reasons.push('Interrupted steps need your decision.');
  if (view.diff.length === 0) reasons.push('No changed files.');
  const undecided = view.diff.filter(d => effectiveDecision(d, stale) === 'pending');
  if (undecided.length > 0) {
    reasons.push(`${undecided.length} file${undecided.length === 1 ? '' : 's'} still need${undecided.length === 1 ? 's' : ''} your decision.`);
  }
  const accepted = view.diff.filter(d => {
    const k = effectiveDecision(d, stale);
    return k === 'accepted' || k === 'partially_accepted';
  });
  if (view.diff.length > 0 && undecided.length === 0 && accepted.length === 0) {
    reasons.push('No accepted file: nothing to apply.');
  }
  if (!/^[0-9a-f]{64}$/.test(view.change_set_sha256) || !/^[0-9a-f]{64}$/.test(view.risks_sha256)) {
    reasons.push('The task view carries no change-set or risk hash.');
  } else if (riskSetHash(view.outcome.unresolved_risks) !== view.risks_sha256) {
    reasons.push('The displayed risk list does not match the server risk hash.');
  }
  return { ok: reasons.length === 0, reasons };
}

// ---------------------------------------------------------------- hashing

const K = [
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];
const rotr = (x: number, n: number) => (x >>> n) | (x << (32 - n));

/** SHA-256 (lowercase hex) of the UTF-8 bytes of `text`. Pure; no WebCrypto needed. */
export function sha256Hex(text: string): string {
  const bytes = new TextEncoder().encode(text);
  const total = Math.ceil((bytes.length + 9) / 64) * 64;
  const buf = new Uint8Array(total);
  buf.set(bytes);
  buf[bytes.length] = 0x80;
  const view = new DataView(buf.buffer);
  view.setUint32(total - 8, Math.floor(bytes.length / 0x20000000));
  view.setUint32(total - 4, (bytes.length * 8) >>> 0);
  const h = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
  const w = new Array<number>(64);
  for (let off = 0; off < total; off += 64) {
    for (let i = 0; i < 16; i += 1) w[i] = view.getUint32(off + i * 4);
    for (let i = 16; i < 64; i += 1) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3);
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
    }
    let [a, b, c, d, e, f, g, hh] = h;
    for (let i = 0; i < 64; i += 1) {
      const s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
      const ch = (e & f) ^ (~e & g);
      const t1 = (hh + s1 + ch + K[i] + w[i]) >>> 0;
      const s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (s0 + maj) >>> 0;
      hh = g; g = f; f = e; e = (d + t1) >>> 0; d = c; c = b; b = a; a = (t1 + t2) >>> 0;
    }
    h[0] = (h[0] + a) >>> 0; h[1] = (h[1] + b) >>> 0; h[2] = (h[2] + c) >>> 0; h[3] = (h[3] + d) >>> 0;
    h[4] = (h[4] + e) >>> 0; h[5] = (h[5] + f) >>> 0; h[6] = (h[6] + g) >>> 0; h[7] = (h[7] + hh) >>> 0;
  }
  return h.map(x => x.toString(16).padStart(8, '0')).join('');
}

function compareUtf8(a: string, b: string): number {
  const ea = new TextEncoder().encode(a);
  const eb = new TextEncoder().encode(b);
  const n = Math.min(ea.length, eb.length);
  for (let i = 0; i < n; i += 1) if (ea[i] !== eb[i]) return ea[i] - eb[i];
  return ea.length - eb.length;
}

/**
 * Client recomputation of the server's `risks_sha256`:
 * sha256(serde_json::to_vec(&BTreeSet<&str> of risk ids)) — a compact JSON
 * array of the unique ids in UTF-8 byte order. Used only to check that the
 * risk list on screen is the one the server hashed; the value SENT in
 * `UiApplyEvent` is always the server's displayed `risks_sha256`.
 */
export function riskSetHash(risks: readonly Risk[]): string {
  const ids = [...new Set(risks.map(r => r.id))].sort(compareUtf8);
  return sha256Hex(JSON.stringify(ids));
}

// ---------------------------------------------------------------- preview

/** §6.5: the only acceptable capability URL shape (loopback IPv4, port, 128-bit token). */
// Stricter subset of the §6.5 regex `…:\d{1,5}/…`: no leading-zero ports.
export const TASK_PREVIEW_URL_PATTERN = /^http:\/\/127\.0\.0\.1:([1-9]\d{0,4})\/__pai\/open\?t=[0-9a-f]{32}$/;

export type PreviewUrlCheck = { ok: true; url: string; port: number } | { ok: false; reason: string };
export function validatePreviewUrl(url: unknown): PreviewUrlCheck {
  if (typeof url !== 'string' || url.length === 0) return { ok: false, reason: 'no preview URL' };
  if (url.length > 128) return { ok: false, reason: 'URL too long' };
  const match = TASK_PREVIEW_URL_PATTERN.exec(url);
  if (!match) {
    return { ok: false, reason: 'not a http://127.0.0.1:<port>/__pai/open?t=<32 hex> capability URL' };
  }
  const port = Number(match[1]);
  if (!Number.isInteger(port) || port < 1 || port > 65535) return { ok: false, reason: 'port out of range' };
  return { ok: true, url, port };
}

export function decodeLogBytes(bytes: string | number[]): string {
  let raw: Uint8Array;
  if (typeof bytes === 'string') {
    if (bytes.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(bytes)) return '[undecodable log bytes]';
    raw = new Uint8Array(bytes.length / 2);
    for (let i = 0; i < raw.length; i += 1) raw[i] = parseInt(bytes.slice(i * 2, i * 2 + 2), 16);
  } else if (Array.isArray(bytes)) {
    raw = Uint8Array.from(bytes.map(b => (Number.isInteger(b) && b >= 0 && b <= 255 ? b : 0x3f)));
  } else {
    return '[undecodable log bytes]';
  }
  return new TextDecoder('utf-8', { fatal: false }).decode(raw);
}

/** Client-side bound on the displayed preview log (the server ring is 64 KiB). */
export const UI_LOG_MAX_RECORDS = 400;
export const UI_LOG_MAX_CHARS = 64 * 1024;
export interface UiLogLine {
  seq: number;
  stream: LogStream;
  text: string;
}
export interface UiLogState {
  lines: UiLogLine[];
  cursor: number;
  uiDroppedRecords: number;
  last: Omit<LogChunk, 'records'> | null;
}
export const emptyLogState = (): UiLogState => ({ lines: [], cursor: 0, uiDroppedRecords: 0, last: null });

export function appendLogChunk(state: UiLogState, chunk: LogChunk): UiLogState {
  const seen = new Set(state.lines.map(l => l.seq));
  const fresh = chunk.records
    .filter(r => !seen.has(r.seq))
    .map(r => ({ seq: r.seq, stream: r.stream, text: decodeLogBytes(r.bytes) }));
  let lines = [...state.lines, ...fresh];
  let dropped = state.uiDroppedRecords;
  if (lines.length > UI_LOG_MAX_RECORDS) {
    dropped += lines.length - UI_LOG_MAX_RECORDS;
    lines = lines.slice(lines.length - UI_LOG_MAX_RECORDS);
  }
  let chars = lines.reduce((n, l) => n + l.text.length, 0);
  while (chars > UI_LOG_MAX_CHARS && lines.length > 1) {
    chars -= lines[0].text.length;
    lines = lines.slice(1);
    dropped += 1;
  }
  const meta: Omit<LogChunk, 'records'> = {
    next_cursor: chunk.next_cursor,
    truncated_before_cursor: chunk.truncated_before_cursor,
    first_retained_seq: chunk.first_retained_seq,
    retained_bytes: chunk.retained_bytes,
    cap_bytes: chunk.cap_bytes,
    dropped_bytes: chunk.dropped_bytes,
    dropped_records: chunk.dropped_records,
    supervisor_dropped_bytes: chunk.supervisor_dropped_bytes,
  };
  const next = Number.isFinite(chunk.next_cursor) ? chunk.next_cursor : state.cursor;
  return {
    lines,
    cursor: Math.max(state.cursor, next),
    uiDroppedRecords: dropped,
    last: meta,
  };
}
