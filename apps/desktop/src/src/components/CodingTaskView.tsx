import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import {
  CODING_TASK_UPDATED_EVENT,
  EFFECT_LABEL,
  RESOLUTION_LABEL,
  STEP_STATE_LABEL,
  appendLogChunk,
  canApply,
  capabilityInfo,
  codingTaskApi,
  criterionStatus,
  effectiveDecision,
  emptyLogState,
  failureLabel,
  formatBytes,
  formatTime,
  isClosedStatus,
  isPaused,
  normalizeDecision,
  observationLabel,
  outcomeBadges,
  previewStatusLabel,
  repositoryBadge,
  shortHash,
  staleFiles,
  taskStatusLabel,
  validatePreviewUrl,
  variantData,
  variantOf,
  type AcceptanceCriterion,
  type ApplyReport,
  type ChipTone,
  type CodingTaskUpdatedPayload,
  type DecisionKind,
  type DiffSummary,
  type FileDiff,
  type GateRecord,
  type GateTarget,
  type HttpStatus,
  type Hunk,
  type IsolationCapability,
  type ReconcileItem,
  type ReviewResolution,
  type TaskSummary,
  type TaskView,
  type UiLogState,
  type CapabilityInfo,
} from '../lib/codingTask';
import { openTaskPreviewWindow } from '../lib/taskPreviewWindow';
import {
  INDEX_LABEL,
  canProposeCandidate,
  checkExitLabel,
  kindLabel,
  knowledgeApi,
  newUiEventId,
  refLabel,
  shortDigest,
  supportReason,
  type CandidateProposalView,
  type LearningVerificationView,
  type RelevantPatternsView,
  type VerificationPreview,
} from '../lib/knowledge';

// Stage 5 "Coding Task" view (design §8.3). The view is a REVIEW surface:
// - on mount it calls only coding_task_capability, coding_task_list and
//   coding_task_view; every other command needs a click (preview log polling,
//   read-only, runs only while a preview is running);
// - review / apply / resolve / resume are never called without a click, and
//   every review/apply click echoes the hashes and view_seq that are on screen;
// - every status word comes from the server's TaskOutcome; model narrative is
//   shown only inside the collapsed "Assistant notes (unverified model text)".

const LOG_POLL_MS = 1000;
const LOG_LIMIT = 200;

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

const DECISION_LABEL: Record<DecisionKind, string> = {
  pending: 'Undecided',
  accepted: 'Accepted',
  rejected: 'Rejected',
  partially_accepted: 'Partly accepted',
};
const CHANGE_LABEL: Record<string, string> = { added: 'Added', modified: 'Modified', deleted: 'Deleted' };

function Chip({ tone, children, ct }: { tone: ChipTone; children: ReactNode; ct?: string }) {
  return (
    <span className={`ct-chip ct-tone-${tone}`} data-ct={ct}>
      {children}
    </span>
  );
}

function criterionCheckLabel(c: AcceptanceCriterion): string {
  switch (variantOf(c.check)) {
    case 'gate_command': {
      const d = variantData<{ command_id: string; expected_exit: number }>(c.check, 'gate_command');
      return `gate command ${d?.command_id ?? '?'} expects exit ${d?.expected_exit ?? '?'}`;
    }
    case 'http': return `HTTP check ${variantData<{ check_id: string }>(c.check, 'http')?.check_id ?? '?'} (HTTP level)`;
    case 'manual': return 'manual check';
    default: return 'unknown check';
  }
}

function HunkRows({ hunk }: { hunk: Hunk }) {
  let oldNo = hunk.old_start;
  let newNo = hunk.new_start;
  return (
    <>
      {hunk.lines.map((line, i) => {
        let o = '';
        let n = '';
        let sign = ' ';
        if (line.tag === 'context') {
          o = String(oldNo++);
          n = String(newNo++);
        } else if (line.tag === 'delete') {
          o = String(oldNo++);
          sign = '-';
        } else {
          n = String(newNo++);
          sign = '+';
        }
        return (
          <tr key={i} className={`ct-diff-${line.tag}`}>
            <td className="ct-ln" aria-label={o ? `old line ${o}` : undefined}>{o}</td>
            <td className="ct-ln" aria-label={n ? `new line ${n}` : undefined}>{n}</td>
            <td className="ct-sign">{sign}</td>
            <td className="ct-code">{line.text.replace(/\r?\n$/, '')}</td>
          </tr>
        );
      })}
    </>
  );
}

function GateCard({ gate, stale }: { gate: GateRecord; stale: boolean }) {
  return (
    <div className="ct-gate" data-ct="gate" data-gate={gate.gate_run_id}>
      <div className="ct-gate-head">
        <strong>Gate {shortHash(gate.gate_run_id, 12)}</strong>
        <span>termination: {gate.termination.replace('_', ' ')}</span>
        <span>elapsed {gate.elapsed_ms} ms</span>
        <span title={gate.working_set_sha256}>content {shortHash(gate.working_set_sha256)}</span>
        <span>{formatTime(gate.at_ms)}</span>
        {stale && <Chip tone="warn" ct="gate-stale">Stale — content changed since this run</Chip>}
      </div>
      <table className="ct-table">
        <thead>
          <tr>
            <th scope="col">Role</th>
            <th scope="col">Command</th>
            <th scope="col">Exit code</th>
            <th scope="col">Termination</th>
            <th scope="col">Output kept</th>
          </tr>
        </thead>
        <tbody>
          {gate.commands.map(cmd => (
            <tr key={cmd.id} data-ct="gate-command" data-command={cmd.id}>
              <td>{cmd.role}</td>
              <td>
                <div><code>{cmd.id}</code></div>
                <div className="ct-argv"><code>{cmd.argv.join(' ')}</code></div>
              </td>
              <td data-ct="exit-code">{cmd.status === null ? 'none' : `exit ${cmd.status}`}</td>
              <td>{cmd.termination.replace('_', ' ')}</td>
              <td data-ct="output-bytes">
                stdout {cmd.stdout_retained_bytes}/{cmd.stdout_total_bytes} B · stderr {cmd.stderr_retained_bytes}/{cmd.stderr_total_bytes} B
                {cmd.truncated ? ' (truncated)' : ''}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {gate.commands.filter(cmd => cmd.excerpt).map(cmd => (
        <details key={`x-${cmd.id}`} className="ct-details">
          <summary>Output excerpt of {cmd.id} (untrusted process output)</summary>
          <pre className="ct-pre">{cmd.excerpt}</pre>
        </details>
      ))}
    </div>
  );
}

// Stage 6 "Learning" panel (design §3.2). Every call is a click: relevant
// patterns are read on request, "Save as candidate" echoes the displayed
// view_seq + change_set_sha256, Verify echoes the displayed recipe hash and
// Approve echoes the displayed run/policy hashes. Unsupported (Windows or a
// task the verifier cannot represent) disables Verify and Approve with the
// server's reason.
const LEARNING_PATTERN_LIMIT = 8;
const LEARNING_STATE_LABEL: Record<string, { label: string; tone: ChipTone }> = {
  verified: { label: 'Verified by a sandboxed Stage 4 run', tone: 'ok' },
  failed: { label: 'Not verified — the checks did not behave as required', tone: 'bad' },
  cancelled: { label: 'Verification cancelled', tone: 'neutral' },
  unsupported: { label: 'Verification unsupported here', tone: 'warn' },
};

function LearningPanel({ task, stale, capInfo, parentBusy }: {
  task: TaskView;
  stale: ReadonlySet<string>;
  capInfo: CapabilityInfo;
  parentBusy: boolean;
}) {
  const [patterns, setPatterns] = useState<RelevantPatternsView | null>(null);
  const [proposal, setProposal] = useState<CandidateProposalView | null>(null);
  const [preview, setPreview] = useState<VerificationPreview | null>(null);
  const [verifyConfirm, setVerifyConfirm] = useState(false);
  const [result, setResult] = useState<LearningVerificationView | null>(null);
  const [approveConfirm, setApproveConfirm] = useState(false);
  const [revokeConfirm, setRevokeConfirm] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState('');
  const mounted = useRef(false);
  const busyRef = useRef<string | null>(null);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  // A recipe preview belongs to the task content it was built from.
  useEffect(() => {
    setPreview(null);
    setVerifyConfirm(false);
  }, [task.view_seq]);

  const run = useCallback(async <T,>(label: string, fn: () => Promise<T>): Promise<T | undefined> => {
    if (busyRef.current) return undefined;
    busyRef.current = label;
    setBusy(label);
    setError('');
    try {
      return await fn();
    } catch (e) {
      if (mounted.current) setError(`${label} failed: ${errorText(e)}`);
      return undefined;
    } finally {
      busyRef.current = null;
      if (mounted.current) setBusy(null);
    }
  }, []);

  const isBusy = busy !== null || parentBusy;
  const gate = canProposeCandidate(task, stale);
  const blocked: string[] = [];
  const proposalReason = supportReason(proposal?.verification);
  const previewReason = supportReason(preview?.support);
  if (proposalReason) blocked.push(proposalReason);
  if (previewReason && previewReason !== proposalReason) blocked.push(previewReason);
  if (result?.state === 'unsupported') blocked.push('The verifier reported this verification as unsupported here.');
  if (capInfo.executionBlocked) blocked.push(`Sandboxed execution is unavailable on this system: ${capInfo.detail || capInfo.label}`);
  const verifyDisabled = isBusy || !preview || blocked.length > 0;
  const canApprove = !!(result && result.state === 'verified' && result.pattern && result.procedure_run
    && result.run_sha256 && result.policy_sha256 && !result.approved && blocked.length === 0);

  const findPatterns = () => {
    void run('Find relevant patterns', async () => {
      const view = await knowledgeApi.relevantPatterns(task.task_id, LEARNING_PATTERN_LIMIT);
      if (mounted.current) setPatterns(view);
    });
  };
  const saveCandidate = () => {
    if (!gate.ok) return;
    const event = { view_seq: task.view_seq, change_set_sha256: task.change_set_sha256, ui_event_id: newUiEventId() };
    void run('Save as candidate', async () => {
      const next = await knowledgeApi.proposeCandidate(task.task_id, event);
      if (!mounted.current) return;
      setProposal(next);
      setPreview(null);
      setResult(null);
    });
  };
  const loadPreview = () => {
    if (!proposal) return;
    const candidate = proposal.candidate;
    void run('Verification preview', async () => {
      const next = await knowledgeApi.verificationPreview(task.task_id, candidate);
      if (!mounted.current) return;
      setPreview(next);
      setResult(null);
    });
  };
  const confirmVerify = () => {
    const shown = preview;
    if (!shown || blocked.length > 0) return;
    void run('Verify', async () => {
      const next = await knowledgeApi.verifyCandidate(task.task_id, {
        candidate: shown.candidate,
        recipe_sha256: shown.recipe_sha256,
        ui_event_id: newUiEventId(),
      });
      if (!mounted.current) return;
      setResult(next);
      setVerifyConfirm(false);
    });
  };
  const confirmApprove = () => {
    const shown = result;
    if (!canApprove || !shown?.pattern || !shown.procedure_run || !shown.run_sha256 || !shown.policy_sha256) return;
    const event = {
      pattern: shown.pattern,
      procedure_run: shown.procedure_run,
      displayed_run_sha256: shown.run_sha256,
      displayed_policy_sha256: shown.policy_sha256,
      ui_event_id: newUiEventId(),
    };
    void run('Approve for reuse', async () => {
      const next = await knowledgeApi.approvePattern(task.task_id, event);
      if (!mounted.current) return;
      setResult(next);
      setApproveConfirm(false);
    });
  };
  const confirmRevoke = () => {
    const approved = result?.approved;
    if (!approved) return;
    void run('Revoke approval', async () => {
      const next = await knowledgeApi.revokePattern(task.task_id, { approved, ui_event_id: newUiEventId() });
      if (!mounted.current) return;
      setResult(next);
      setRevokeConfirm(false);
    });
  };
  const dialogKeys = (e: ReactKeyboardEvent<HTMLDivElement>, close: () => void) => {
    if (e.key === 'Escape') close();
  };

  const state = result ? LEARNING_STATE_LABEL[result.state] ?? { label: `State: ${result.state}`, tone: 'neutral' as ChipTone } : null;

  return (
    <section className="settings-section" aria-labelledby="ct-learning-title" data-ct="learning">
      <div className="settings-section-header" id="ct-learning-title">Learning (reusable patterns)</div>
      <div className="settings-section-body">
        {error && <div className="ct-alert" role="alert" data-ct="lp-error">{error}</div>}
        <p className="ct-muted">
          Nothing here runs on its own. Saving creates an unverified candidate; it becomes a verified pattern only
          through a sandboxed Stage 4 verification run, and is reused only after your explicit approval.
        </p>

        <div className="ct-row">
          <button className="btn btn-secondary" onClick={findPatterns} disabled={isBusy}>
            {busy === 'Find relevant patterns' ? 'Looking up…' : 'Find relevant patterns'}
          </button>
          {patterns && <Chip tone={patterns.index === 'fresh' ? 'neutral' : 'warn'} ct="lp-index">{INDEX_LABEL[patterns.index] ?? patterns.index}</Chip>}
        </div>
        {patterns && (
          <div data-ct="lp-patterns">
            <p className="ct-muted" data-ct="lp-patterns-note">{patterns.note}</p>
            {patterns.index !== 'fresh' && (
              <p className="ct-warn-text">The knowledge index is {patterns.index}: no patterns are returned until you rebuild it in Knowledge.</p>
            )}
            {patterns.hits.length === 0 ? <p className="ct-muted">No verified pattern matches the exact files of this task.</p> : (
              <ul className="ct-list">
                {patterns.hits.map(h => (
                  <li key={`${h.reference.logical_id}#${h.reference.revision}`} data-ct="lp-pattern">
                    <strong>{kindLabel(h.kind)}</strong> {h.title}{' '}
                    <span className="ct-muted">
                      — source {h.source.source_id} · version {shortDigest(h.source.source_version)} · licence {h.source.license} · {h.why_recalled}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}

        <div className="ct-row">
          <button className="btn btn-secondary" onClick={saveCandidate} disabled={isBusy || !gate.ok} data-ct="lp-save">
            Save as candidate
          </button>
          <span className="ct-muted">
            view {task.view_seq} · change set <code title={task.change_set_sha256}>{shortHash(task.change_set_sha256)}</code>
          </span>
        </div>
        {!gate.ok && (
          <ul className="ct-reasons" data-ct="lp-save-reasons" aria-label="Why Save as candidate is disabled">
            {gate.reasons.map(r => <li key={r}>{r}</li>)}
          </ul>
        )}

        {proposal && (
          <div className="ct-recon" data-ct="lp-proposal">
            <div>
              Candidate <code data-ct="lp-candidate">{refLabel(proposal.candidate)}</code> saved with{' '}
              {proposal.evidence.length} evidence record{proposal.evidence.length === 1 ? '' : 's'} (unverified).
            </div>
            <div className="ct-muted" data-ct="lp-support">
              {proposal.verification.kind === 'supported'
                ? `Verification supported: ${proposal.verification.cases} cases.`
                : `Verification unsupported: ${proposal.verification.reason}`}
            </div>
            <div className="ct-row">
              <button className="btn btn-secondary" onClick={loadPreview} disabled={isBusy}>
                Verification preview
              </button>
            </div>
          </div>
        )}

        {preview && (
          <div className="ct-recon" data-ct="lp-preview">
            <div>
              Recipe <code>{preview.recipe.schema}</code> · {preview.recipe.repetitions} repetitions · limits {preview.recipe.limits.cpu_seconds} s CPU,{' '}
              {preview.recipe.limits.timeout_ms} ms timeout, {preview.recipe.limits.processes} processes
            </div>
            <table className="ct-table" data-ct="lp-cases">
              <thead>
                <tr>
                  <th scope="col">Case</th>
                  <th scope="col">Kind</th>
                  <th scope="col">Command (sandboxed)</th>
                  <th scope="col">Expected exit code</th>
                </tr>
              </thead>
              <tbody>
                {preview.recipe.cases.map(c => (
                  <tr key={c.name} data-ct="lp-case">
                    <td><code>{c.name}</code></td>
                    <td>{c.kind}</td>
                    <td className="ct-argv"><code>{c.argv.join(' ')}</code></td>
                    <td data-ct="lp-expected-exit">exit {c.expected_status}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            <div className="ct-muted">
              Test files: {preview.recipe.oracle_files.join(', ') || 'none'} · code under test: {preview.recipe.implementation_files.join(', ') || 'none'}
            </div>
            <p className="ct-muted ct-hashes">Recipe hash <code data-ct="lp-recipe-hash">{preview.recipe_sha256}</code></p>
            <div className="ct-row">
              <button className="btn btn-primary" onClick={() => setVerifyConfirm(true)} disabled={verifyDisabled} data-ct="lp-verify-open">
                Verify…
              </button>
            </div>
          </div>
        )}
        {(proposal || preview || result) && blocked.length > 0 && (
          <ul className="ct-reasons" data-ct="lp-blocked" aria-label="Why Verify and Approve are disabled">
            {blocked.map(r => <li key={r}>{r}</li>)}
          </ul>
        )}

        {result && state && (
          <div className="ct-recon" data-ct="lp-result">
            <div className="ct-row">
              <Chip tone={state.tone} ct="lp-state">{state.label}</Chip>
              {result.pattern && <span className="ct-muted">pattern <code>{refLabel(result.pattern)}</code></span>}
              {result.approved && <Chip tone="ok" ct="lp-approved">Approved for reuse by you</Chip>}
            </div>
            {result.checks.length > 0 && (
              <table className="ct-table" data-ct="lp-checks">
                <thead>
                  <tr>
                    <th scope="col">Case</th>
                    <th scope="col">Snapshot</th>
                    <th scope="col">Exit code</th>
                    <th scope="col">Termination</th>
                    <th scope="col">Result</th>
                  </tr>
                </thead>
                <tbody>
                  {result.checks.map(c => (
                    <tr key={`${c.reference.logical_id}#${c.reference.revision}`} data-ct="lp-check">
                      <td><code>{c.case}</code></td>
                      <td>{c.role}</td>
                      <td data-ct="lp-check-exit">{checkExitLabel(c)}</td>
                      <td>{c.termination.replaceAll('_', ' ')}</td>
                      <td>{c.passed ? 'as required' : 'not as required'}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            <p className="ct-muted ct-hashes">
              Run <code data-ct="lp-run-hash">{result.run_sha256 ?? 'none'}</code><br />
              Policy <code data-ct="lp-policy-hash">{result.policy_sha256 ?? 'none'}</code>
            </p>
            {result.residuals.length > 0 && (
              <details className="ct-details">
                <summary>Known limits of this verification</summary>
                <ul className="ct-list">{result.residuals.map((r, i) => <li key={i}>{r}</li>)}</ul>
              </details>
            )}
            <div className="ct-row">
              <button className="btn btn-primary" onClick={() => setApproveConfirm(true)} disabled={isBusy || !canApprove} data-ct="lp-approve-open">
                Approve for reuse…
              </button>
              <button className="btn btn-danger" onClick={() => setRevokeConfirm(true)} disabled={isBusy || !result.approved} data-ct="lp-revoke-open">
                Revoke approval…
              </button>
            </div>
          </div>
        )}
      </div>

      {verifyConfirm && preview && (
        <div className="ct-dialog-backdrop">
          <div className="ct-dialog" role="dialog" aria-modal="true" aria-labelledby="ct-lp-verify-title" data-ct="lp-verify-dialog"
            onKeyDown={e => dialogKeys(e, () => setVerifyConfirm(false))}>
            <h3 id="ct-lp-verify-title">Run the verification recipe</h3>
            <p className="ct-muted">
              The {preview.recipe.cases.length} cases run {preview.recipe.repetitions} times in the sandbox against the base and the current
              content. The verifier decides; the base must fail and the current content must pass.
            </p>
            <p className="ct-muted ct-hashes">
              Candidate <code>{refLabel(preview.candidate)}</code><br />
              Recipe hash <code data-ct="lp-verify-hash">{preview.recipe_sha256}</code>
            </p>
            <div className="ct-row">
              <button className="btn btn-primary" onClick={confirmVerify} disabled={verifyDisabled}>
                {busy === 'Verify' ? 'Verifying…' : 'Confirm verify'}
              </button>
              <button className="btn btn-ghost" onClick={() => setVerifyConfirm(false)} disabled={busy === 'Verify'}>Not now</button>
            </div>
          </div>
        </div>
      )}

      {approveConfirm && result && (
        <div className="ct-dialog-backdrop">
          <div className="ct-dialog" role="dialog" aria-modal="true" aria-labelledby="ct-lp-approve-title" data-ct="lp-approve-dialog"
            onKeyDown={e => dialogKeys(e, () => setApproveConfirm(false))}>
            <h3 id="ct-lp-approve-title">Approve this pattern for reuse</h3>
            <p className="ct-muted">Your approval is recorded as explicit UI evidence. The current snapshot is captured again and must match.</p>
            <p className="ct-muted ct-hashes">
              Pattern <code>{result.pattern ? refLabel(result.pattern) : 'none'}</code><br />
              Run <code data-ct="lp-approve-run">{result.run_sha256}</code><br />
              Policy <code data-ct="lp-approve-policy">{result.policy_sha256}</code>
            </p>
            <div className="ct-row">
              <button className="btn btn-primary" onClick={confirmApprove} disabled={isBusy || !canApprove}>Confirm approval</button>
              <button className="btn btn-ghost" onClick={() => setApproveConfirm(false)} disabled={busy === 'Approve for reuse'}>Not now</button>
            </div>
          </div>
        </div>
      )}

      {revokeConfirm && result?.approved && (
        <div className="ct-dialog-backdrop">
          <div className="ct-dialog" role="dialog" aria-modal="true" aria-labelledby="ct-lp-revoke-title" data-ct="lp-revoke-dialog"
            onKeyDown={e => dialogKeys(e, () => setRevokeConfirm(false))}>
            <h3 id="ct-lp-revoke-title">Revoke the approval</h3>
            <p className="ct-muted ct-hashes">Approval <code data-ct="lp-revoke-ref">{refLabel(result.approved)}</code></p>
            <div className="ct-row">
              <button className="btn btn-danger" onClick={confirmRevoke} disabled={isBusy}>Confirm revoke</button>
              <button className="btn btn-ghost" onClick={() => setRevokeConfirm(false)} disabled={busy === 'Revoke approval'}>Not now</button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}

export function CodingTaskView() {
  const [capability, setCapability] = useState<IsolationCapability | null>(null);
  const [tasks, setTasks] = useState<TaskSummary[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<TaskView | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [localStale, setLocalStale] = useState<ReadonlySet<string>>(new Set());
  const [openPath, setOpenPath] = useState<string | null>(null);
  const [fileDiff, setFileDiff] = useState<FileDiff | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [diffError, setDiffError] = useState('');
  const [logState, setLogState] = useState<UiLogState>(emptyLogState);
  const [applyDialog, setApplyDialog] = useState<{ snapshot: TaskView; acks: ReadonlySet<string> } | null>(null);
  const [applyReport, setApplyReport] = useState<ApplyReport | null>(null);
  const [patchText, setPatchText] = useState<string | null>(null);
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [confirmRevertApplied, setConfirmRevertApplied] = useState(false);
  const [previewMsg, setPreviewMsg] = useState('');

  const mounted = useRef(false);
  const viewRef = useRef<TaskView | null>(null);
  const selectedRef = useRef<string | null>(null);
  const tasksRef = useRef<TaskSummary[] | null>(null);
  const busyRef = useRef<string | null>(null);
  // path -> new_sha256 the user saw when they decided (this session).
  const decidedAt = useRef(new Map<string, string | null>());
  const logCursor = useRef(0);
  const diffReq = useRef(0);

  useEffect(() => { logCursor.current = logState.cursor; }, [logState]);

  const acceptView = useCallback((next: TaskView) => {
    if (!mounted.current || next.task_id !== selectedRef.current) return;
    const prev = viewRef.current;
    if (prev && prev.task_id === next.task_id && next.view_seq < prev.view_seq) return;
    const reset: string[] = [];
    for (const d of next.diff) {
      if (decidedAt.current.has(d.path) && decidedAt.current.get(d.path) !== d.new_sha256) {
        reset.push(d.path);
        decidedAt.current.delete(d.path);
      }
    }
    const present = new Set(next.diff.map(d => d.path));
    setLocalStale(s => {
      const out = new Set([...s].filter(p => present.has(p)));
      for (const p of reset) out.add(p);
      return out;
    });
    viewRef.current = next;
    setView(next);
  }, []);

  const loadView = useCallback(async (taskId: string) => {
    try {
      const next = await codingTaskApi.view(taskId);
      acceptView(next);
    } catch (e) {
      if (mounted.current && selectedRef.current === taskId) setError(`Could not load the task: ${errorText(e)}`);
    }
  }, [acceptView]);

  const selectTask = useCallback((taskId: string) => {
    selectedRef.current = taskId;
    viewRef.current = null;
    decidedAt.current = new Map();
    setSelectedId(taskId);
    setView(null);
    setLocalStale(new Set());
    setOpenPath(null);
    setFileDiff(null);
    setDiffError('');
    setLogState(emptyLogState());
    setApplyDialog(null);
    setApplyReport(null);
    setPatchText(null);
    setConfirmCancel(false);
    setConfirmRevertApplied(false);
    setPreviewMsg('');
    setError('');
    setNotice('');
    void loadView(taskId);
  }, [loadView]);

  // Mount: capability, list, then the view of the newest readable task. Nothing else.
  useEffect(() => {
    let active = true;
    mounted.current = true;
    void (async () => {
      try {
        const cap = await codingTaskApi.capability();
        if (active) setCapability(cap);
      } catch (e) {
        if (active) setError(`Could not read the isolation capability: ${errorText(e)}`);
      }
      if (!active) return;
      try {
        const list = await codingTaskApi.list();
        if (!active) return;
        tasksRef.current = list;
        setTasks(list);
        const initial = [...list].filter(t => t.readable).sort((a, b) => b.created_at_ms - a.created_at_ms)[0];
        if (initial) selectTask(initial.task_id);
      } catch (e) {
        if (!active) return;
        setTasks([]);
        setError(`Could not list coding tasks: ${errorText(e)}`);
      }
    })();
    return () => { active = false; mounted.current = false; };
  }, [selectTask]);

  // Live refresh on the content-free commit event (no content polling).
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<CodingTaskUpdatedPayload>(CODING_TASK_UPDATED_EVENT, event => {
      if (disposed || !mounted.current) return;
      const { task_id: taskId, view_seq: viewSeq } = event.payload ?? ({} as CodingTaskUpdatedPayload);
      if (typeof taskId !== 'string') return;
      if (taskId === selectedRef.current) {
        if (viewRef.current?.view_seq !== viewSeq) void loadView(taskId);
      } else if (!tasksRef.current?.some(t => t.task_id === taskId)) {
        void codingTaskApi.list()
          .then(list => { if (!disposed && mounted.current) { tasksRef.current = list; setTasks(list); } })
          .catch(() => undefined);
      }
    }).then(fn => { if (disposed) fn(); else unlisten = fn; }).catch(() => undefined);
    return () => { disposed = true; unlisten?.(); };
  }, [loadView]);

  // An open apply dialog is bound to the view it was opened on.
  useEffect(() => {
    if (applyDialog && view && view.view_seq !== applyDialog.snapshot.view_seq) {
      setApplyDialog(null);
      setNotice('The task changed while the apply dialog was open. Review the current state and open it again.');
    }
  }, [view, applyDialog]);

  const openSummary = view && openPath ? view.diff.find(d => d.path === openPath) ?? null : null;

  const loadDiff = useCallback(async (taskId: string, path: string) => {
    const req = ++diffReq.current;
    setDiffLoading(true);
    setDiffError('');
    try {
      const diff = await codingTaskApi.fileDiff(taskId, path);
      if (!mounted.current || req !== diffReq.current) return;
      setFileDiff(diff);
    } catch (e) {
      if (!mounted.current || req !== diffReq.current) return;
      setFileDiff(null);
      setDiffError(`Could not load the diff: ${errorText(e)}`);
    } finally {
      if (mounted.current && req === diffReq.current) setDiffLoading(false);
    }
  }, []);

  // The open diff must always be the diff of the content on screen.
  useEffect(() => {
    if (!view || !openPath) return;
    if (!openSummary) {
      setOpenPath(null);
      setFileDiff(null);
      return;
    }
    if (fileDiff && fileDiff.path === openPath
      && (fileDiff.new_sha256 !== openSummary.new_sha256 || fileDiff.base_sha256 !== openSummary.base_sha256)) {
      setFileDiff(null);
      void loadDiff(view.task_id, openPath);
    }
  }, [view, openPath, openSummary, fileDiff, loadDiff]);

  const previewRunning = view ? variantOf(view.preview.status) === 'ready' : false;
  const serviceId = view?.preview.descriptor?.service_id ?? null;
  useEffect(() => {
    setLogState(emptyLogState());
    logCursor.current = 0;
  }, [serviceId]);

  // Bounded log tail: 1 s cursor polling, log endpoint only, only while running.
  const viewTaskId = view?.task_id ?? null;
  useEffect(() => {
    if (!viewTaskId || !previewRunning) return;
    let stopped = false;
    let inFlight = false;
    const tick = async () => {
      if (stopped || inFlight) return;
      inFlight = true;
      try {
        const chunk = await codingTaskApi.previewLogs(viewTaskId, logCursor.current, LOG_LIMIT);
        if (!stopped && mounted.current) setLogState(s => appendLogChunk(s, chunk));
      } catch {
        // A failed poll is retried on the next tick; nothing is fabricated.
      } finally {
        inFlight = false;
      }
    };
    const id = window.setInterval(() => { void tick(); }, LOG_POLL_MS);
    return () => { stopped = true; window.clearInterval(id); };
  }, [viewTaskId, previewRunning]);

  const run = useCallback(async <T,>(label: string, fn: () => Promise<T>): Promise<T | undefined> => {
    if (busyRef.current) return undefined;
    busyRef.current = label;
    setBusy(label);
    setError('');
    setNotice('');
    try {
      return await fn();
    } catch (e) {
      if (mounted.current) setError(`${label} failed: ${errorText(e)}`);
      return undefined;
    } finally {
      busyRef.current = null;
      if (mounted.current) setBusy(null);
    }
  }, []);

  const staleSet = useMemo<ReadonlySet<string>>(
    () => new Set([...(view ? staleFiles(view, decidedAt.current) : []), ...localStale]),
    [view, localStale],
  );

  // ------------------------------------------------------------- render
  const capInfo = capabilityInfo(view?.capability ?? capability);
  const execBlocked = capInfo.executionBlocked;
  const paused = view ? isPaused(view.status) : false;
  const closed = view ? isClosedStatus(view.status) : false;
  const isBusy = busy !== null;
  const applyGate = view ? canApply(view, staleSet) : { ok: false, reasons: ['No task loaded.'] };
  const urlCheck = view?.preview.capability_url ? validatePreviewUrl(view.preview.capability_url) : null;

  const header = (
    <div className="main-header">
      <h2>Coding Task</h2>
      <div className="main-header-actions ct-header-actions">
        {tasks && tasks.length > 1 && (
          <label className="ct-inline-label">
            Task
            <select
              value={selectedId ?? ''}
              onChange={e => selectTask(e.target.value)}
              disabled={isBusy}
              aria-label="Select coding task"
            >
              {tasks.map(t => (
                <option key={t.task_id} value={t.task_id} disabled={!t.readable}>
                  {t.objective_excerpt || t.task_id} — {taskStatusLabel(t.status)}
                </option>
              ))}
            </select>
          </label>
        )}
        <button
          className="btn btn-secondary btn-sm"
          onClick={() => { if (selectedRef.current) void loadView(selectedRef.current); }}
          disabled={!selectedId}
        >
          Refresh task
        </button>
      </div>
    </div>
  );

  if (tasks === null) {
    return (
      <div>
        {header}
        <div className="main-body">
          <div className="coding-task-view settings-view">
            {error && <div className="ct-alert" role="alert">{error}</div>}
            <div className="empty-state"><div className="spinner" /><p>Loading coding tasks…</p></div>
          </div>
        </div>
      </div>
    );
  }

  if (!view) {
    return (
      <div>
        {header}
        <div className="main-body">
          <div className="coding-task-view settings-view">
            {error && <div className="ct-alert" role="alert">{error}</div>}
            <div className="ct-capability-row">
              <Chip tone={capInfo.state === 'runtime_verified' ? 'ok' : capInfo.state === 'unsupported' ? 'bad' : 'neutral'} ct="capability">
                {capInfo.label}
              </Chip>
              {capInfo.detail && <span className="ct-muted">{capInfo.detail}</span>}
            </div>
            {tasks.length === 0 ? (
              <div className="empty-state">
                <h3>No coding tasks in this vault</h3>
                <p>Coding tasks are opened through the adapter in Stage 5. Each one appears here for review.</p>
              </div>
            ) : (
              <div className="empty-state"><div className="spinner" /><p>Loading the task…</p></div>
            )}
          </div>
        </div>
      </div>
    );
  }

  const task = view;
  const badges = outcomeBadges(task.outcome);
  const staleGates = new Set(
    [task.outcome.build_status, task.outcome.test_status]
      .filter(s => variantOf(s) === 'stale')
      .map(s => variantData<{ gate: string }>(s, 'stale')?.gate ?? ''),
  );
  const narratives = task.journal_tail.filter(j => j.untrusted_model_text);
  const unresolvedItems = task.reconciliation.filter(item => item.options.length > 0);
  const httpStatus = variantData<{ http: HttpStatus }>(task.preview.status, 'ready')?.http ?? null;
  const httpRecord = task.preview.http_checks;
  const httpRecordStale = !!httpRecord && (
    httpStatus === 'stale'
    || !task.preview.descriptor
    || httpRecord.service_id !== task.preview.descriptor.service_id
    || httpRecord.tree_sha256 !== task.preview.descriptor.tree_sha256
  );
  const appliedData = variantData<{ files: string[]; worktree: string }>(task.outcome.apply_status, 'applied');
  const runDisabled = execBlocked || paused || closed || isBusy;
  const diffMatches = !!(fileDiff && openSummary && fileDiff.path === openSummary.path
    && fileDiff.new_sha256 === openSummary.new_sha256 && fileDiff.base_sha256 === openSummary.base_sha256);

  // ------------------------------------------------------------- actions
  const confirmPlan = () => {
    if (!task.plan) return;
    const revision = task.plan.revision;
    void run('Confirm plan', async () => {
      acceptView(await codingTaskApi.confirmPlan({ task_id: task.task_id, view_seq: task.view_seq, revision }));
    });
  };
  const runGate = (target: GateTarget) => {
    void run(target === 'current' ? 'Run checks' : 'Run checks on accepted changes', async () => {
      acceptView(await codingTaskApi.runGate(task.task_id, target));
    });
  };
  const openDiff = (d: DiffSummary) => {
    setOpenPath(d.path);
    setFileDiff(null);
    void loadDiff(task.task_id, d.path);
  };
  const review = (d: DiffSummary, kind: 'accepted' | 'rejected') => {
    void run(kind === 'accepted' ? 'Accept file' : 'Reject file', async () => {
      const next = await codingTaskApi.reviewFile({
        task_id: task.task_id,
        view_seq: task.view_seq,
        path: d.path,
        decision: { kind },
        displayed_base_sha256: d.base_sha256,
        displayed_new_sha256: d.new_sha256,
      });
      decidedAt.current.set(d.path, d.new_sha256);
      setLocalStale(s => new Set([...s].filter(p => p !== d.path)));
      acceptView(next);
    });
  };
  const revertFile = (d: DiffSummary) => {
    void run('Revert to base', async () => {
      const next = await codingTaskApi.revertFile({
        task_id: task.task_id,
        view_seq: task.view_seq,
        path: d.path,
        displayed_new_sha256: d.new_sha256,
      });
      decidedAt.current.delete(d.path);
      acceptView(next);
    });
  };
  const startPreview = () => {
    setPreviewMsg('');
    void run('Start preview', async () => {
      await codingTaskApi.startPreview({ task_id: task.task_id, view_seq: task.view_seq });
      await loadView(task.task_id);
    });
  };
  const stopPreview = () => {
    void run('Stop preview', async () => { acceptView(await codingTaskApi.stopPreview(task.task_id)); });
  };
  const runHttpChecks = () => {
    void run('Run HTTP checks', async () => { acceptView(await codingTaskApi.httpChecks(task.task_id)); });
  };
  const openPreview = () => {
    if (!urlCheck?.ok) return;
    setPreviewMsg('Opening the preview window…');
    void openTaskPreviewWindow(urlCheck.url).then(ok => {
      if (mounted.current) setPreviewMsg(ok ? 'Preview window opened. What you see there is your own observation; UnoOne verified HTTP level only.' : 'The preview window could not be opened.');
    });
  };
  const resolve = (item: ReconcileItem, resolution: ReviewResolution) => {
    void run('Resolve interrupted step', async () => {
      acceptView(await codingTaskApi.resolveInterrupted({
        task_id: task.task_id,
        view_seq: task.view_seq,
        step_id: item.step_id,
        attempt: item.attempt,
        resolution,
      }));
    });
  };
  const resume = () => {
    void run('Resume task', async () => {
      acceptView(await codingTaskApi.resume({ task_id: task.task_id, view_seq: task.view_seq }));
    });
  };
  const exportPatch = () => {
    void run('Export patch', async () => { setPatchText(await codingTaskApi.exportPatch(task.task_id)); });
  };
  const cancelTask = () => {
    setConfirmCancel(false);
    void run('Cancel task', async () => {
      await codingTaskApi.cancel(task.task_id);
      await loadView(task.task_id);
    });
  };
  const confirmApply = () => {
    const dialog = applyDialog;
    if (!dialog) return;
    const snap = dialog.snapshot;
    const ids = snap.outcome.unresolved_risks.map(r => r.id);
    if (!ids.every(id => dialog.acks.has(id))) return;
    void run('Apply', async () => {
      const report = await codingTaskApi.apply({
        task_id: snap.task_id,
        view_seq: snap.view_seq,
        displayed_change_set_sha256: snap.change_set_sha256,
        displayed_risks_sha256: snap.risks_sha256,
        acknowledged_risk_ids: [...new Set(ids)],
      });
      if (!mounted.current) return;
      setApplyReport(report);
      setApplyDialog(null);
      await loadView(snap.task_id);
    });
  };
  const revertApplied = () => {
    setConfirmRevertApplied(false);
    void run('Revert applied changes', async () => {
      const report = await codingTaskApi.revertApplied({ task_id: task.task_id, view_seq: task.view_seq });
      if (!mounted.current) return;
      setApplyReport(report);
      await loadView(task.task_id);
    });
  };
  const dialogKeys = (e: ReactKeyboardEvent<HTMLDivElement>, close: () => void) => {
    if (e.key === 'Escape') close();
  };

  const capTone: ChipTone = capInfo.state === 'runtime_verified' ? 'ok' : capInfo.state === 'unsupported' ? 'bad' : 'neutral';
  const statusTone: ChipTone = paused ? 'warn' : variantOf(task.status) === 'stopped' ? 'bad' : 'neutral';

  return (
    <div>
      {header}
      <div className="main-body">
        <div className="coding-task-view settings-view">
          {error && <div className="ct-alert" role="alert">{error}</div>}
          {notice && <div className="ct-notice" role="status">{notice}</div>}

          {/* 1. Header */}
          <section className="settings-section" aria-label="Task">
            <div className="settings-section-body ct-task-head">
              <div className="ct-objective" data-ct="objective">{task.objective}</div>
              <div className="ct-repo">
                <code data-ct="repo-path">{task.repository.display_root}</code>
                <Chip tone="warn" ct="repo-label">{repositoryBadge(task.repository)}</Chip>
              </div>
              <div className="ct-capability-row">
                <Chip tone={capTone} ct="capability">{capInfo.label}</Chip>
                {capInfo.detail && <span className="ct-muted" data-ct="capability-detail">{capInfo.detail}</span>}
                <Chip tone={statusTone} ct="task-status">{taskStatusLabel(task.status)}</Chip>
              </div>
              <div className="ct-muted">
                Protected test files ({task.oracle_visibility === 'hidden' ? 'hidden from the assistant' : 'visible to the assistant'}):{' '}
                {task.oracle.protected.length ? task.oracle.protected.join(', ') : 'none'}
              </div>
            </div>
          </section>

          {execBlocked && (
            <div className="ct-banner" role="note" data-ct="exec-blocked">
              <strong>Sandboxed execution is unavailable on this system.</strong>{' '}
              {capInfo.detail || 'The isolation capability is not known yet.'} Running checks, the preview and
              Apply are disabled. The diff, the step log, your review decisions and patch export stay available.
            </div>
          )}
          {paused && (
            <div className="ct-banner" role="note">
              This task is paused after an interruption. Nothing is re-run automatically; choose an option for each
              interrupted step below.
            </div>
          )}

          {/* 4. Outcome */}
          <section className="settings-section" aria-labelledby="ct-outcome-title">
            <div className="settings-section-header" id="ct-outcome-title">Outcome (computed by UnoOne from recorded facts)</div>
            <div className="settings-section-body ct-outcome" data-ct="outcome">
              {badges.map(b => (
                <div key={b.key} className="ct-outcome-row">
                  <span className="ct-outcome-title">{b.title}</span>
                  <Chip tone={b.tone} ct={`chip-${b.key}`}>{b.label}</Chip>
                </div>
              ))}
            </div>
          </section>

          {/* 2. Plan and acceptance criteria */}
          <section className="settings-section" aria-labelledby="ct-plan-title">
            <div className="settings-section-header" id="ct-plan-title">Plan and acceptance criteria</div>
            <div className="settings-section-body">
              {task.plan ? (
                <div data-ct="plan">
                  <div className="ct-row">
                    {task.plan.author === 'model' && !task.plan.confirmed ? (
                      <Chip tone="warn" ct="plan-author">Proposed by assistant — confirm to use</Chip>
                    ) : (
                      <Chip tone="neutral" ct="plan-author">
                        {task.plan.author === 'model' ? 'Proposed by assistant — confirmed by you' : 'Your plan'}
                      </Chip>
                    )}
                    <span className="ct-muted">revision {task.plan.revision}</span>
                    {task.plan.author === 'model' && !task.plan.confirmed && (
                      <button className="btn btn-primary" onClick={confirmPlan} disabled={isBusy || closed}>
                        Confirm plan
                      </button>
                    )}
                  </div>
                  <ol className="ct-list">
                    {task.plan.steps.map(step => (
                      <li key={step.step_id}>
                        <code>{step.step_id}</code> {step.summary}{' '}
                        <span className="ct-muted">({step.kind.replace('_', ' ')}, {EFFECT_LABEL[step.effect] ?? step.effect})</span>
                      </li>
                    ))}
                  </ol>
                </div>
              ) : (
                <p className="ct-muted">No plan recorded.</p>
              )}
              <table className="ct-table" data-ct="criteria">
                <thead>
                  <tr>
                    <th scope="col">Acceptance criterion</th>
                    <th scope="col">Check</th>
                    <th scope="col">Confirmed by you</th>
                    <th scope="col">Status</th>
                  </tr>
                </thead>
                <tbody>
                  {task.acceptance.map(c => {
                    const st = criterionStatus(task, c);
                    return (
                      <tr key={c.id} data-ct="criterion" data-criterion={c.id}>
                        <td><code>{c.id}</code> {c.text}</td>
                        <td>{criterionCheckLabel(c)}</td>
                        <td>{c.confirmed_by_user ? 'yes' : 'no'}</td>
                        <td><Chip tone={st.tone}>{st.label}</Chip></td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          </section>

          {/* 3. Live step log */}
          <section className="settings-section" aria-labelledby="ct-steps-title">
            <div className="settings-section-header" id="ct-steps-title">Step log</div>
            <div className="settings-section-body" aria-live="polite">
              {task.steps.length === 0 ? <p className="ct-muted">No steps recorded yet.</p> : (
                <table className="ct-table" data-ct="steps">
                  <thead>
                    <tr>
                      <th scope="col">Step</th>
                      <th scope="col">Attempt</th>
                      <th scope="col">State</th>
                      <th scope="col">Effect</th>
                      <th scope="col">Failure</th>
                    </tr>
                  </thead>
                  <tbody>
                    {task.steps.map(s => (
                      <tr key={`${s.step_id}#${s.attempt}`} data-ct="step" data-step={s.step_id} data-state={s.state}>
                        <td><code>{s.step_id}</code></td>
                        <td>{s.attempt}</td>
                        <td>{STEP_STATE_LABEL[s.state] ?? s.state}</td>
                        <td>{EFFECT_LABEL[s.effect] ?? s.effect}</td>
                        <td>{failureLabel(s.failure)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
              <details className="ct-details">
                <summary>Journal ({task.journal_tail.length} most recent entries)</summary>
                <ol className="ct-journal" data-ct="journal">
                  {task.journal_tail.map(j => (
                    <li key={j.seq}>
                      <span className="ct-muted">#{j.seq} · {formatTime(j.at_ms)}</span>{' '}
                      {j.untrusted_model_text
                        ? 'narrative — unverified model text (see Assistant notes)'
                        : j.event.replaceAll('_', ' ')}
                    </li>
                  ))}
                </ol>
              </details>
            </div>
          </section>

          {/* 5. Diff viewer */}
          <section className="settings-section" aria-labelledby="ct-diff-title">
            <div className="settings-section-header" id="ct-diff-title">Changes to review ({task.diff.length} file{task.diff.length === 1 ? '' : 's'})</div>
            <div className="settings-section-body">
              {task.diff.length === 0 ? <p className="ct-muted">No changed files.</p> : (
                <ul className="ct-files" data-ct="files">
                  {task.diff.map(d => {
                    const eff = effectiveDecision(d, staleSet);
                    const stale = staleSet.has(d.path);
                    const raw = normalizeDecision(d.decision);
                    return (
                      <li key={d.path} data-ct="file" data-path={d.path} data-decision={eff}>
                        <button
                          className={`btn btn-secondary ct-file-btn ${openPath === d.path ? 'ct-selected' : ''}`}
                          onClick={() => openDiff(d)}
                          aria-label={`Show diff of ${d.path}`}
                          aria-pressed={openPath === d.path}
                        >
                          <code>{d.path}</code>
                        </button>
                        <span>{CHANGE_LABEL[d.change] ?? d.change}</span>
                        <span className="ct-muted" title={`base ${d.base_sha256 ?? 'absent'} → new ${d.new_sha256 ?? 'absent'}`}>
                          {shortHash(d.base_sha256)} → {shortHash(d.new_sha256)}
                        </span>
                        <span className="ct-muted">{d.hunk_count} hunk{d.hunk_count === 1 ? '' : 's'}{d.binary ? ' · binary' : ''}{d.truncated ? ' · diff truncated' : ''}</span>
                        <Chip tone={eff === 'pending' ? 'warn' : eff === 'rejected' ? 'neutral' : 'ok'} ct="decision">
                          {DECISION_LABEL[eff]}{eff === 'partially_accepted' && raw.kind === 'partially_accepted' ? ` (${raw.hunks.length} hunks)` : ''}
                        </Chip>
                        {stale && <Chip tone="warn" ct="stale">Stale — content changed, decide again</Chip>}
                      </li>
                    );
                  })}
                </ul>
              )}

              {openSummary && (
                <div className="ct-diff-pane" data-ct="diff-pane" data-path={openSummary.path}>
                  <div className="ct-row">
                    <strong><code>{openSummary.path}</code></strong>
                    <span className="ct-muted" data-ct="diff-hashes">
                      base <code>{openSummary.base_sha256 ?? 'absent'}</code> → new <code>{openSummary.new_sha256 ?? 'absent'}</code>
                    </span>
                  </div>
                  {diffLoading && <p className="ct-muted">Loading diff…</p>}
                  {diffError && <div className="ct-alert" role="alert">{diffError}</div>}
                  {fileDiff && !diffMatches && (
                    <p className="ct-warn-text">The loaded diff does not match the current file hashes; decisions are disabled until it reloads.</p>
                  )}
                  {fileDiff && diffMatches && (
                    <>
                      {fileDiff.binary && <p className="ct-muted">Binary file — content is not shown.</p>}
                      {fileDiff.truncated && <p className="ct-warn-text">Diff truncated at the display limit — per-hunk selection disabled.</p>}
                      {fileDiff.timed_out && <p className="ct-warn-text">Diff computation timed out — per-hunk selection disabled.</p>}
                      <p className="ct-muted">
                        Per-hunk acceptance is not available in this build (the composed-content hash it needs is not
                        exposed to the app); accept or reject the whole file.
                      </p>
                      {fileDiff.hunks.map(h => (
                        <div key={h.index} className="ct-hunk">
                          <label className="ct-check">
                            <input type="checkbox" disabled checked={false} onChange={() => undefined} aria-label={`Select hunk ${h.index + 1} of ${fileDiff.path}`} />
                            <span>Hunk {h.index + 1}: @@ -{h.old_start},{h.old_len} +{h.new_start},{h.new_len} @@</span>
                          </label>
                          <table className="ct-diff" aria-label={`Hunk ${h.index + 1} of ${fileDiff.path}`}>
                            <tbody><HunkRows hunk={h} /></tbody>
                          </table>
                        </div>
                      ))}
                    </>
                  )}
                  <div className="ct-row" role="group" aria-label={`Decision for ${openSummary.path}`}>
                    <button
                      className="btn btn-primary"
                      onClick={() => review(openSummary, 'accepted')}
                      disabled={!diffMatches || isBusy || closed}
                      aria-label={`Accept file ${openSummary.path}`}
                    >
                      Accept file
                    </button>
                    <button
                      className="btn btn-secondary"
                      onClick={() => review(openSummary, 'rejected')}
                      disabled={!diffMatches || isBusy || closed}
                      aria-label={`Reject file ${openSummary.path}`}
                    >
                      Reject file
                    </button>
                    <button
                      className="btn btn-ghost"
                      onClick={() => revertFile(openSummary)}
                      disabled={!diffMatches || isBusy || closed || paused}
                      aria-label={`Revert ${openSummary.path} to base`}
                    >
                      Revert to base
                    </button>
                  </div>
                </div>
              )}
            </div>
          </section>

          {/* 6. Checks */}
          <section className="settings-section" aria-labelledby="ct-checks-title">
            <div className="settings-section-header" id="ct-checks-title">Checks (sandboxed build and test commands)</div>
            <div className="settings-section-body">
              <div className="ct-row">
                <button className="btn btn-secondary" onClick={() => runGate('current')} disabled={runDisabled}>
                  {busy === 'Run checks' ? 'Running checks…' : 'Run checks'}
                </button>
                <button className="btn btn-secondary" onClick={() => runGate('accepted_composition')} disabled={runDisabled}>
                  {busy === 'Run checks on accepted changes' ? 'Running checks…' : 'Run checks on accepted changes'}
                </button>
                {execBlocked && <span className="ct-muted">Disabled: {capInfo.detail || 'isolation unavailable'}</span>}
              </div>
              {task.gates.length === 0 ? <p className="ct-muted">No checks have run on this task.</p> : (
                [...task.gates].reverse().map(g => <GateCard key={g.gate_run_id} gate={g} stale={staleGates.has(g.gate_run_id)} />)
              )}
            </div>
          </section>

          {/* 7. Preview */}
          {variantOf(task.preview.status) !== 'not_applicable' && (
            <section className="settings-section" aria-labelledby="ct-preview-title" data-ct="preview">
              <div className="settings-section-header" id="ct-preview-title">Preview (managed local server)</div>
              <div className="settings-section-body">
                <p className="ct-honest" data-ct="preview-honesty">HTTP-level checks only — not browser-rendered</p>
                <p className="ct-muted">Browser rendering: not verified by UnoOne — HTTP checks only. {task.preview.evidence_label}</p>
                <div className="ct-row">
                  <span>Status:</span>
                  <Chip tone={variantOf(task.preview.status) === 'startup_failed' ? 'bad' : 'neutral'} ct="preview-status">
                    {busy === 'Start preview' ? 'Starting…' : previewStatusLabel(task.preview)}
                  </Chip>
                  {task.preview.descriptor && (
                    <span className="ct-muted">
                      service {shortHash(task.preview.descriptor.service_id, 8)} · bridge port {task.preview.descriptor.bridge_port} · content {shortHash(task.preview.descriptor.tree_sha256)}
                    </span>
                  )}
                </div>
                {task.preview.capability_url && urlCheck && (
                  urlCheck.ok ? (
                    <div className="ct-row">
                      <span>URL:</span> <code data-ct="preview-url">{urlCheck.url}</code>
                    </div>
                  ) : (
                    <p className="ct-warn-text" data-ct="preview-url-rejected">
                      Preview URL rejected ({urlCheck.reason}). Only http://127.0.0.1 capability URLs are opened.
                    </p>
                  )
                )}
                <div className="ct-row">
                  <button className="btn btn-secondary" onClick={startPreview} disabled={execBlocked || paused || closed || isBusy || previewRunning}>
                    Start preview
                  </button>
                  <button className="btn btn-primary" onClick={openPreview} disabled={!urlCheck?.ok || !previewRunning || execBlocked}>
                    Open preview
                  </button>
                  <button className="btn btn-secondary" onClick={runHttpChecks} disabled={execBlocked || isBusy || !previewRunning}>
                    Run HTTP checks
                  </button>
                  <button className="btn btn-ghost" onClick={stopPreview} disabled={isBusy || !previewRunning}>
                    Stop preview
                  </button>
                </div>
                {previewMsg && <p className="ct-muted" role="status">{previewMsg}</p>}

                {httpRecord && (
                  <div data-ct="http-checks">
                    <div className="ct-row">
                      <strong>HTTP check results</strong>
                      <Chip tone="neutral" ct="http-level">HTTP-level only ({httpRecord.evidence_level.replace('_', ' ')})</Chip>
                      {httpRecordStale && <Chip tone="warn" ct="http-stale">Stale — server or content changed</Chip>}
                      <span className="ct-muted">{formatTime(httpRecord.at_ms)}</span>
                    </div>
                    <table className="ct-table">
                      <thead>
                        <tr>
                          <th scope="col">Check</th>
                          <th scope="col">HTTP status</th>
                          <th scope="col">Result</th>
                          <th scope="col">Elapsed</th>
                          <th scope="col">Response excerpt (untrusted)</th>
                        </tr>
                      </thead>
                      <tbody>
                        {httpRecord.results.map(r => (
                          <tr key={r.id} data-ct="http-result" data-check={r.id}>
                            <td><code>{r.id}</code></td>
                            <td>{r.status ?? 'no response'}</td>
                            <td>{r.passed ? 'passed' : `failed${r.failure ? `: ${r.failure}` : ''}`}</td>
                            <td>{r.elapsed_ms} ms</td>
                            <td><code className="ct-excerpt">{r.excerpt}</code></td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}

                <div data-ct="preview-logs">
                  <strong>Server log (bounded)</strong>
                  <p className="ct-muted" data-ct="log-meta">
                    {logState.last ? (
                      <>
                        Server ring: {logState.last.retained_bytes !== undefined ? `${formatBytes(logState.last.retained_bytes)} retained` : 'retained size not reported'}
                        {logState.last.cap_bytes !== undefined ? ` of ${formatBytes(logState.last.cap_bytes)} cap` : ''}
                        {' · '}dropped {logState.last.dropped_bytes} bytes / {logState.last.dropped_records} records
                        {' · '}rate-limited before the host: {logState.last.supervisor_dropped_bytes} bytes
                        {' · '}first retained record #{logState.last.first_retained_seq}
                        {logState.last.truncated_before_cursor ? ' · earlier records were evicted before they were shown' : ''}
                        {logState.uiDroppedRecords > 0 ? ` · this view keeps only the last records (${logState.uiDroppedRecords} older records hidden)` : ''}
                      </>
                    ) : previewRunning ? 'Waiting for the first log poll…' : 'Log polling runs only while the preview is running.'}
                  </p>
                  {logState.lines.length > 0 && (
                    <pre className="ct-pre ct-log" aria-label="Preview server log">
                      {logState.lines.map(l => `${l.stream === 'stderr' ? '[stderr] ' : ''}${l.text}`).join('')}
                    </pre>
                  )}
                </div>
              </div>
            </section>
          )}

          {/* 8. Unresolved risks */}
          <section className="settings-section" aria-labelledby="ct-risks-title">
            <div className="settings-section-header" id="ct-risks-title">Unresolved risks (computed by UnoOne, not by the assistant)</div>
            <div className="settings-section-body">
              {task.outcome.unresolved_risks.length === 0 ? <p className="ct-muted">No unresolved risks reported.</p> : (
                <ul className="ct-list" data-ct="risks">
                  {task.outcome.unresolved_risks.map(r => (
                    <li key={r.id} data-ct="risk" data-risk={r.id}><code>{r.id}</code> {r.summary}</li>
                  ))}
                </ul>
              )}
              <p className="ct-muted">Risk set hash <code title={task.risks_sha256}>{shortHash(task.risks_sha256)}</code></p>
              <details className="ct-details" data-ct="assistant-notes">
                <summary>Assistant notes (unverified model text) — {narratives.length}</summary>
                {narratives.length === 0 ? <p className="ct-muted">No assistant notes.</p> : (
                  <ul className="ct-list">
                    {narratives.map(n => (
                      <li key={n.seq}>
                        <span className="ct-muted">#{n.seq} · {formatTime(n.at_ms)} · unverified model text:</span>
                        <pre className="ct-pre">{n.excerpt ?? ''}</pre>
                      </li>
                    ))}
                  </ul>
                )}
              </details>
              {task.residuals.length > 0 && (
                <details className="ct-details">
                  <summary>Known limits of the task ledger</summary>
                  <ul className="ct-list">{task.residuals.map((r, i) => <li key={i}>{r}</li>)}</ul>
                </details>
              )}
            </div>
          </section>

          {/* 9. Reconciliation */}
          {task.reconciliation.length > 0 && (
            <section className="settings-section" aria-labelledby="ct-recon-title" data-ct="reconciliation">
              <div className="settings-section-header" id="ct-recon-title">Interrupted steps</div>
              <div className="settings-section-body">
                <p className="ct-muted">Nothing runs automatically. Each option below records your decision; none re-runs a step on its own.</p>
                {task.reconciliation.map(item => (
                  <div key={`${item.step_id}#${item.attempt}`} className="ct-recon" data-ct="recon-item" data-step={item.step_id}>
                    <div><code>{item.step_id}</code> (attempt {item.attempt}, {EFFECT_LABEL[item.effect] ?? item.effect})</div>
                    <div className="ct-muted">{observationLabel(item.observation)}</div>
                    {item.options.length === 0 ? <div className="ct-muted">Resolved.</div> : (
                      <div className="ct-row" role="group" aria-label={`Options for interrupted step ${item.step_id}`}>
                        {item.options.map(opt => (
                          <button
                            key={opt}
                            className={opt === 'abandon' ? 'btn btn-danger' : 'btn btn-secondary'}
                            onClick={() => resolve(item, opt)}
                            disabled={isBusy || (opt === 'restore_pre_image' && execBlocked)}
                          >
                            {RESOLUTION_LABEL[opt] ?? opt}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                ))}
                {paused && (
                  <div className="ct-row">
                    <button className="btn btn-primary" onClick={resume} disabled={isBusy || unresolvedItems.length > 0}>
                      Resume task (re-checks admission)
                    </button>
                    {unresolvedItems.length > 0 && <span className="ct-muted">Resolve every interrupted step first.</span>}
                  </div>
                )}
              </div>
            </section>
          )}

          {/* 10. Footer */}
          <section className="settings-section" aria-labelledby="ct-apply-section-title">
            <div className="settings-section-header" id="ct-apply-section-title">Apply</div>
            <div className="settings-section-body">
              <div className="ct-row">
                <button
                  className="btn btn-primary"
                  onClick={() => setApplyDialog({ snapshot: task, acks: new Set() })}
                  disabled={!applyGate.ok || isBusy}
                  data-ct="apply-open"
                >
                  Apply accepted changes to task worktree…
                </button>
                <button className="btn btn-secondary" onClick={exportPatch} disabled={isBusy}>
                  Export patch
                </button>
                {!confirmCancel ? (
                  <button className="btn btn-danger" onClick={() => setConfirmCancel(true)} disabled={isBusy || closed}>
                    Cancel task…
                  </button>
                ) : (
                  <span role="group" aria-label="Confirm task cancellation" className="ct-row">
                    <button className="btn btn-danger" onClick={cancelTask} disabled={isBusy}>Yes, cancel this task</button>
                    <button className="btn btn-ghost" onClick={() => setConfirmCancel(false)}>Keep the task</button>
                  </span>
                )}
              </div>
              {!applyGate.ok && (
                <ul className="ct-reasons" data-ct="apply-reasons" aria-label="Why Apply is disabled">
                  {applyGate.reasons.map(r => <li key={r}>{r}</li>)}
                </ul>
              )}
              {appliedData && (
                <div className="ct-row">
                  <span>Task worktree: <code>{appliedData.worktree}</code></span>
                  {!confirmRevertApplied ? (
                    <button className="btn btn-secondary" onClick={() => setConfirmRevertApplied(true)} disabled={isBusy || execBlocked}>
                      Revert applied changes…
                    </button>
                  ) : (
                    <span role="group" aria-label="Confirm revert of applied changes" className="ct-row">
                      <button className="btn btn-danger" onClick={revertApplied} disabled={isBusy}>Yes, revert the task worktree</button>
                      <button className="btn btn-ghost" onClick={() => setConfirmRevertApplied(false)}>Keep it</button>
                    </span>
                  )}
                </div>
              )}
              {applyReport && (
                <p className="ct-muted" role="status" data-ct="apply-report">
                  Wrote {applyReport.files.length} file{applyReport.files.length === 1 ? '' : 's'} in the task worktree{' '}
                  <code>{applyReport.worktree.path}</code> (step {applyReport.step_id}). Your repository was not written.
                </p>
              )}
            </div>
          </section>

          {/* 11. Learning (Stage 6) */}
          <LearningPanel key={task.task_id} task={task} stale={staleSet} capInfo={capInfo} parentBusy={isBusy} />
        </div>
      </div>

      {applyDialog && (
        <div className="ct-dialog-backdrop">
          <div
            className="ct-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="ct-apply-title"
            data-ct="apply-dialog"
            onKeyDown={e => dialogKeys(e, () => setApplyDialog(null))}
          >
            <h3 id="ct-apply-title">Apply accepted changes to the task worktree</h3>
            <p className="ct-muted">
              Only accepted content is written, into a bounded task worktree (a copy — not your repository and not a
              Git worktree). Worktree:{' '}
              {appliedData ? <code>{appliedData.worktree}</code> : 'a new task worktree in the app data folder; its path is reported after apply.'}
            </p>
            <ul className="ct-list" data-ct="apply-files">
              {applyDialog.snapshot.diff.map(d => (
                <li key={d.path}><code>{d.path}</code> — {DECISION_LABEL[effectiveDecision(d, staleSet)]}</li>
              ))}
            </ul>
            {applyDialog.snapshot.outcome.unresolved_risks.length > 0 && (
              <fieldset className="ct-fieldset">
                <legend>Tick each unresolved risk to acknowledge it</legend>
                {applyDialog.snapshot.outcome.unresolved_risks.map(r => (
                  <label key={r.id} className="ct-check" data-ct="apply-risk">
                    <input
                      type="checkbox"
                      checked={applyDialog.acks.has(r.id)}
                      onChange={e => {
                        const checked = e.target.checked;
                        setApplyDialog(d => {
                          if (!d) return d;
                          const acks = new Set(d.acks);
                          if (checked) acks.add(r.id); else acks.delete(r.id);
                          return { ...d, acks };
                        });
                      }}
                      aria-label={`Acknowledge risk ${r.id}`}
                    />
                    <span><code>{r.id}</code> {r.summary}</span>
                  </label>
                ))}
              </fieldset>
            )}
            <p className="ct-muted ct-hashes">
              Change set <code data-ct="apply-change-set">{applyDialog.snapshot.change_set_sha256}</code><br />
              Risk set <code data-ct="apply-risk-hash">{applyDialog.snapshot.risks_sha256}</code><br />
              View <code data-ct="apply-view-seq">{applyDialog.snapshot.view_seq}</code>
            </p>
            <div className="ct-row">
              <button
                className="btn btn-primary"
                onClick={confirmApply}
                disabled={isBusy || !applyDialog.snapshot.outcome.unresolved_risks.every(r => applyDialog.acks.has(r.id))}
                data-ct="apply-confirm"
              >
                {busy === 'Apply' ? 'Applying…' : 'Confirm apply'}
              </button>
              <button className="btn btn-ghost" onClick={() => setApplyDialog(null)} disabled={busy === 'Apply'}>
                Back
              </button>
            </div>
          </div>
        </div>
      )}

      {patchText !== null && (
        <div className="ct-dialog-backdrop">
          <div
            className="ct-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="ct-patch-title"
            data-ct="patch-dialog"
            onKeyDown={e => dialogKeys(e, () => setPatchText(null))}
          >
            <h3 id="ct-patch-title">Patch of accepted changes</h3>
            <p className="ct-muted">Apply it to your repository yourself; UnoOne never writes your repository.</p>
            <textarea className="ct-patch" readOnly value={patchText} aria-label="Patch text" rows={16} />
            <div className="ct-row">
              <button
                className="btn btn-secondary"
                onClick={() => {
                  const clip = typeof navigator !== 'undefined' ? navigator.clipboard : undefined;
                  if (clip?.writeText) {
                    void clip.writeText(patchText).then(
                      () => setNotice('Patch copied to the clipboard.'),
                      () => setNotice('Copy failed; select the text and copy it manually.'),
                    );
                  } else {
                    setNotice('Clipboard unavailable; select the text and copy it manually.');
                  }
                }}
              >
                Copy patch
              </button>
              <button className="btn btn-ghost" onClick={() => setPatchText(null)}>Close</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

