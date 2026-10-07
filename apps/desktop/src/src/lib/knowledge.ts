/**
 * Stage 6 "Knowledge" + coding-task "Learning" — typed IPC wrappers, view
 * types and pure helpers.
 *
 * The types mirror the serde shapes frozen in the Stage 6 design §1/§2:
 * K1 `knowledge_service.rs` / `knowledge_distiller.rs`, K2 `task_learning.rs`,
 * plus the existing Stage 2 contracts (`RecordRef`, kinds) and Stage 3/4
 * types (`RecallMode`, `TrustedSource`, `NormalizationAudit`,
 * `VerificationRecipe`). Field names are serde snake_case; plain enums are
 * snake_case strings; tagged unions carry `kind` (`KnowledgeBody`,
 * `DistillSource`, `VerificationSupport`).
 *
 * Commands are the FROZEN §3.1 surface (`knowledge_commands.rs`). Top-level
 * argument keys are converted snake_case → camelCase exactly like
 * `src/lib/tauri.ts`; nested values keep their serde field names (the Rust
 * request/event types are `deny_unknown_fields`).
 *
 * Every string that came from a source document (titles, snippets, content,
 * statements, excerpts, labels) is UNTRUSTED data: it is rendered as text
 * only and never interpreted by any helper below.
 */
import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { effectiveDecision, variantOf, type TaskView } from './codingTask';

// ---------------------------------------------------------------- IPC plumbing

// IDENTICAL to the conversion in src/lib/tauri.ts (module-private there).
// The knowledge test suite asserts the two function bodies stay byte-identical.
function snakeToCamelKey(key: string): string {
  return key.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());
}

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const converted = args
    ? Object.fromEntries(Object.entries(args).map(([k, v]) => [snakeToCamelKey(k), v]))
    : args;
  return tauriInvoke<T>(command, converted);
}

/** The frozen §3.1 command names (`knowledge_commands.rs`). */
export const KNOWLEDGE_COMMANDS = {
  status: 'knowledge_status',
  initialize: 'knowledge_initialize',
  rebuildIndex: 'knowledge_rebuild_index',
  search: 'knowledge_search',
  list: 'knowledge_list',
  detail: 'knowledge_detail',
  reject: 'knowledge_reject',
  revokeApproval: 'knowledge_revoke_approval',
  exportPreview: 'knowledge_export_preview',
  export: 'knowledge_export',
  distillPreview: 'knowledge_distill_preview',
  distill: 'knowledge_distill',
  distillRuns: 'knowledge_distill_runs',
  relevantPatterns: 'task_relevant_patterns',
  proposeCandidate: 'task_propose_candidate',
  verificationPreview: 'task_verification_preview',
  verifyCandidate: 'task_verify_candidate',
  approvePattern: 'task_approve_pattern',
  revokePattern: 'task_revoke_pattern',
} as const;

/** §3.1 event, emitted by the glue after every state-changing command; the payload carries no content. */
export const KNOWLEDGE_UPDATED_EVENT = 'unoone:knowledge-updated';
export interface KnowledgeUpdatedPayload {
  command: string;
  task_id: string | null;
}

/** K1 `DISTILL_METHOD` (the server's `method` field is what the views display). */
export const DISTILL_METHOD = 'extractive-headings-docstrings-v1; deterministic; no model; no network';
/** K1 `ExportPreview.training_export` fixed text. */
export const TRAINING_EXPORT_DISABLED = 'disabled: separate consent and licence review required';

// ---------------------------------------------------------------- shared types

export type KnowledgeKind = 'evidence' | 'candidate' | 'verified_pattern' | 'approved_procedure' | 'invalidation';
export type EvidenceKind = 'observation' | 'artifact' | 'check_result' | 'procedure_run' | 'ui_approval';
export type EdgeKind = 'supporting' | 'contradicting' | 'derived' | 'superseding' | 'invalidated_by';
/** Stage 2 contract reference (`deny_unknown_fields`). */
export interface RecordRef {
  logical_id: string;
  revision: number;
  kind: KnowledgeKind;
  content_digest: string;
}
export interface Audit {
  actor: string;
  reason: string;
}
export type IndexState = 'fresh' | 'stale' | 'missing';
export type RecallMode = 'current' | 'historical';
export interface TrustedSource {
  source_id: string;
  source_version: string;
  source_commit: string;
  file_digest: string;
}
export interface NormalizationAudit {
  algorithm: string;
  terms: string[];
  declared_aliases_used: string[];
}

// ---------------------------------------------------------------- K1 views

export interface KnowledgeStatus {
  initialized: boolean;
  index: IndexState;
  catalog_entries: number;
  /** evidence | candidate | verified_pattern | approved_procedure | invalidation */
  counts: Record<string, number>;
  active_counts: Record<string, number>;
  method: string;
  residuals: string[];
}

export interface KnowledgeQuery {
  text: string;
  mode: RecallMode;
  platform: string;
  trusted_source: TrustedSource | null;
  kinds: KnowledgeKind[];
  limit: number;
}
export interface SourceBadge {
  source_id: string;
  source_version: string;
  source_commit: string;
  file_digest: string | null;
  license: string;
  privacy: string;
  platforms: string[];
  topics: string[];
}
export interface KnowledgeHitView {
  reference: RecordRef;
  kind: KnowledgeKind;
  title: string;
  snippet: string;
  source: SourceBadge;
  active: boolean;
  contradictory: boolean;
  invalidated: boolean;
  why_recalled: string;
  mode: RecallMode;
}
export interface KnowledgeSearchView {
  hits: KnowledgeHitView[];
  normalization: NormalizationAudit;
  index: IndexState;
  note: string;
}
export type ListState = 'all' | 'active' | 'invalidated' | 'contradictory';
export interface KnowledgeListFilter {
  kinds: KnowledgeKind[];
  state: ListState;
  offset: number;
  limit: number;
}
export interface KnowledgeListView {
  items: KnowledgeHitView[];
  total: number;
  offset: number;
}
export type KnowledgeBody =
  | { kind: 'evidence'; evidence_kind: EvidenceKind; content: string; truncated: boolean; content_sha256: string }
  | { kind: 'candidate'; statement: string; evidence: RecordRef[] }
  | { kind: 'verified_pattern'; statement: string; candidate: RecordRef; checks: RecordRef[] }
  | { kind: 'approved_procedure'; pattern: RecordRef; outcome_evidence: RecordRef[]; approval_evidence: RecordRef }
  | { kind: 'invalidation'; target: RecordRef; reason: string };
export interface EdgeView {
  relation: EdgeKind;
  target: RecordRef;
  direction: 'outgoing' | 'incoming';
}
export interface CheckSummary {
  reference: RecordRef;
  case: string;
  /** baseline | fixed */
  role: string;
  status: number | null;
  termination: string;
  passed: boolean;
}
export interface VerificationSummary {
  checks: CheckSummary[];
  repetitions: number;
  approved: boolean;
  approval: RecordRef | null;
}
export type KnowledgeAction = 'reject' | 'revoke_approval' | 'export';
export interface KnowledgeDetailView {
  reference: RecordRef;
  kind: KnowledgeKind;
  body: KnowledgeBody;
  source: SourceBadge;
  audit: Audit;
  timestamp_ms: number;
  history: RecordRef[];
  edges: EdgeView[];
  active: boolean;
  invalidated: boolean;
  contradictory: boolean;
  verification: VerificationSummary | null;
  /** Server-computed; the UI offers exactly these and nothing else. */
  allowed_actions: string[];
  residuals: string[];
}
export interface UiRejectEvent { target: RecordRef; reason: string; ui_event_id: string }
export interface UiRevokeEvent { approved: RecordRef; ui_event_id: string }
export interface UiInitEvent { ui_event_id: string }

export interface ExportRequest { references: RecordRef[]; include_evidence_content: boolean }
export interface ExportItemPreview { reference: RecordRef; kind: KnowledgeKind; title: string; licence: string }
export interface ExportPreview {
  request_sha256: string;
  items: ExportItemPreview[];
  /** serde tuple `(RecordRef, String)` → `[ref, reason]`. */
  refused: [RecordRef, string][];
  contains_private_content: boolean;
  training_export: string;
}
export interface UiExportConsentEvent {
  request: ExportRequest;
  request_sha256: string;
  ui_event_id: string;
  acknowledged_private: boolean;
}
export interface ExportBundle { schema: string; json: string; sha256: string; items: number }

// ---------------------------------------------------------------- K1 distiller

export type DistillSource =
  | { kind: 'pasted_text'; label: string; text: string }
  | { kind: 'local_file'; root: string; path: string };
export interface DistillBudget { max_total_bytes: number; max_candidates: number; deadline_ms: number }
export interface DistillRequest {
  sources: DistillSource[];
  budget: DistillBudget;
  platform: string;
  license: string;
  topics: string[];
}
export interface UiDistillEvent { request_sha256: string; ui_event_id: string }
export interface DistillPreview { request_sha256: string }
export interface Citation { evidence: RecordRef; start: number; end: number; excerpt: string }
export interface DistilledCandidate { reference: RecordRef; statement: string; citation: Citation }
export type ExclusionReason = 'held_out_hash' | 'held_out_name' | 'duplicate' | 'too_large' | 'unreadable' | 'not_utf8' | 'budget';
export interface ExcludedSource { label: string; reason: ExclusionReason | string }
export interface ContradictionView { candidate: RecordRef; existing: RecordRef; rule: string }
export interface DistillReport {
  run_id: string;
  method: string;
  evidence: RecordRef[];
  candidates: DistilledCandidate[];
  excluded: ExcludedSource[];
  possible_contradictions: ContradictionView[];
  budget_exhausted: boolean;
  elapsed_ms: number;
  summary: RecordRef;
}
export interface DistillRunSummary {
  summary: RecordRef;
  run_id: string;
  timestamp_ms: number;
  evidence: number;
  candidates: number;
  excluded: number;
}

// ---------------------------------------------------------------- K2 learning

export interface RelevantPatternsView { index: IndexState; hits: KnowledgeHitView[]; note: string }
export interface UiProposeCandidateEvent { view_seq: number; change_set_sha256: string; ui_event_id: string }
export type VerificationSupport = { kind: 'supported'; cases: number } | { kind: 'unsupported'; reason: string };
export interface CandidateProposalView { candidate: RecordRef; evidence: RecordRef[]; verification: VerificationSupport }
export interface RunLimits { cpu_seconds: number; memory_bytes: number; processes: number; timeout_ms: number; output_bytes: number }
export type CaseKind = 'positive' | 'negative' | 'regression';
export interface VerificationCase {
  name: string;
  kind: CaseKind;
  argv: string[];
  expected_status: number;
  expected_stdout: string;
  expected_stderr: string;
  expected_files: Record<string, string>;
}
export interface VerificationRecipe {
  schema: string;
  repetitions: number;
  limits: RunLimits;
  cases: VerificationCase[];
  oracle_files: string[];
  implementation_files: string[];
}
export interface VerificationPreview {
  candidate: RecordRef;
  recipe: VerificationRecipe;
  recipe_sha256: string;
  support: VerificationSupport;
}
export interface UiVerifyCandidateEvent { candidate: RecordRef; recipe_sha256: string; ui_event_id: string }
export type LearningState = 'verified' | 'failed' | 'cancelled' | 'unsupported';
export interface LearningVerificationView {
  state: LearningState | string;
  pattern: RecordRef | null;
  procedure_run: RecordRef | null;
  run_sha256: string | null;
  policy_sha256: string | null;
  checks: CheckSummary[];
  approved: RecordRef | null;
  residuals: string[];
}
export interface UiApprovePatternEvent {
  pattern: RecordRef;
  procedure_run: RecordRef;
  displayed_run_sha256: string;
  displayed_policy_sha256: string;
  ui_event_id: string;
}
export interface UiRevokePatternEvent { approved: RecordRef; ui_event_id: string }

// ---------------------------------------------------------------- wrappers

/** Typed wrappers over the frozen §3.1 surface. Struct parameters go under their Rust parameter name. */
export const knowledgeApi = {
  status: () => invoke<KnowledgeStatus>(KNOWLEDGE_COMMANDS.status),
  initialize: (event: UiInitEvent) => invoke<KnowledgeStatus>(KNOWLEDGE_COMMANDS.initialize, { event }),
  rebuildIndex: () => invoke<KnowledgeStatus>(KNOWLEDGE_COMMANDS.rebuildIndex),
  search: (query: KnowledgeQuery) => invoke<KnowledgeSearchView>(KNOWLEDGE_COMMANDS.search, { query }),
  list: (filter: KnowledgeListFilter) => invoke<KnowledgeListView>(KNOWLEDGE_COMMANDS.list, { filter }),
  detail: (logicalId: string, revision: number | null) =>
    invoke<KnowledgeDetailView>(KNOWLEDGE_COMMANDS.detail, { logical_id: logicalId, revision }),
  reject: (event: UiRejectEvent) => invoke<KnowledgeDetailView>(KNOWLEDGE_COMMANDS.reject, { event }),
  revokeApproval: (event: UiRevokeEvent) => invoke<KnowledgeDetailView>(KNOWLEDGE_COMMANDS.revokeApproval, { event }),
  exportPreview: (request: ExportRequest) => invoke<ExportPreview>(KNOWLEDGE_COMMANDS.exportPreview, { request }),
  export: (event: UiExportConsentEvent) => invoke<ExportBundle>(KNOWLEDGE_COMMANDS.export, { event }),
  distillPreview: (request: DistillRequest) => invoke<DistillPreview>(KNOWLEDGE_COMMANDS.distillPreview, { request }),
  distill: (request: DistillRequest, event: UiDistillEvent) =>
    invoke<DistillReport>(KNOWLEDGE_COMMANDS.distill, { request, event }),
  distillRuns: () => invoke<DistillRunSummary[]>(KNOWLEDGE_COMMANDS.distillRuns),
  relevantPatterns: (taskId: string, limit: number) =>
    invoke<RelevantPatternsView>(KNOWLEDGE_COMMANDS.relevantPatterns, { task_id: taskId, limit }),
  proposeCandidate: (taskId: string, event: UiProposeCandidateEvent) =>
    invoke<CandidateProposalView>(KNOWLEDGE_COMMANDS.proposeCandidate, { task_id: taskId, event }),
  verificationPreview: (taskId: string, candidate: RecordRef) =>
    invoke<VerificationPreview>(KNOWLEDGE_COMMANDS.verificationPreview, { task_id: taskId, candidate }),
  verifyCandidate: (taskId: string, event: UiVerifyCandidateEvent) =>
    invoke<LearningVerificationView>(KNOWLEDGE_COMMANDS.verifyCandidate, { task_id: taskId, event }),
  approvePattern: (taskId: string, event: UiApprovePatternEvent) =>
    invoke<LearningVerificationView>(KNOWLEDGE_COMMANDS.approvePattern, { task_id: taskId, event }),
  revokePattern: (taskId: string, event: UiRevokePatternEvent) =>
    invoke<LearningVerificationView>(KNOWLEDGE_COMMANDS.revokePattern, { task_id: taskId, event }),
};

// ---------------------------------------------------------------- helpers

/** A fresh UI event id: 128 OS-random bits as 32 lowercase hex digits. Fails closed without WebCrypto. */
export function newUiEventId(): string {
  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  return Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('');
}

/** Unicode scalar count (the Rust bounds count scalars, not UTF-16 units). */
export const scalarLength = (text: string) => [...text].length;

export const KIND_LABEL: Record<KnowledgeKind, string> = {
  evidence: 'Evidence',
  candidate: 'Candidate',
  verified_pattern: 'Verified pattern',
  approved_procedure: 'Approved procedure',
  invalidation: 'Invalidation',
};
export const kindLabel = (kind: string) => KIND_LABEL[kind as KnowledgeKind] ?? kind;

/** The Explorer's kind filters (design §3.2: Evidence/Candidate/Verified/Approved/Invalidated). */
export const KIND_FILTERS: { kind: KnowledgeKind; label: string }[] = [
  { kind: 'evidence', label: 'Evidence' },
  { kind: 'candidate', label: 'Candidate' },
  { kind: 'verified_pattern', label: 'Verified' },
  { kind: 'approved_procedure', label: 'Approved' },
  { kind: 'invalidation', label: 'Invalidated' },
];

export const EVIDENCE_KIND_LABEL: Record<EvidenceKind, string> = {
  observation: 'observation',
  artifact: 'artifact',
  check_result: 'check result',
  procedure_run: 'procedure run',
  ui_approval: 'UI approval',
};
export const EDGE_LABEL: Record<EdgeKind, string> = {
  supporting: 'supporting',
  contradicting: 'contradicting',
  derived: 'derived from',
  superseding: 'superseding',
  invalidated_by: 'invalidated by',
};
export const INDEX_LABEL: Record<IndexState, string> = {
  fresh: 'Index: fresh',
  stale: 'Index: stale — rebuild to search new records',
  missing: 'Index: missing — rebuild to enable search',
};

export const EXCLUSION_LABEL: Record<ExclusionReason, string> = {
  held_out_hash: 'held-out / evaluation material (content hash on the exclusion list)',
  held_out_name: 'held-out / evaluation material (path name rule)',
  duplicate: 'duplicate of another source',
  too_large: 'too large for the byte budget',
  unreadable: 'unreadable (on this platform local files cannot be captured fd-safely)',
  not_utf8: 'not UTF-8 text',
  budget: 'budget exhausted before this source',
};
export const exclusionLabel = (reason: string) => EXCLUSION_LABEL[reason as ExclusionReason] ?? reason;

export const shortDigest = (sha: string | null | undefined, n = 12) => (sha ? sha.slice(0, n) : '—');
export const refKey = (ref: RecordRef) => `${ref.logical_id}#${ref.revision}#${ref.content_digest}`;
export const refLabel = (ref: RecordRef) => `${kindLabel(ref.kind)} ${ref.logical_id} r${ref.revision}`;

/** Only active, non-contradictory verified patterns / approved procedures are exportable (K1 rule). */
export const EXPORTABLE_KINDS: readonly KnowledgeKind[] = ['verified_pattern', 'approved_procedure'];
export const isExportableKind = (kind: string) => (EXPORTABLE_KINDS as readonly string[]).includes(kind);

/** §1.2 distiller bounds. */
export const DISTILL_LIMITS = {
  sources: 32,
  labelScalars: 128,
  licenseScalars: 256,
  topics: 8,
  platformScalars: 128,
  maxTotalBytes: 1024 * 1024,
  maxCandidates: 64,
  deadlineMs: 30_000,
} as const;
export const DEFAULT_BUDGET: DistillBudget = { max_total_bytes: 256 * 1024, max_candidates: 32, deadline_ms: 10_000 };

const inRange = (n: number, min: number, max: number) => Number.isInteger(n) && n >= min && n <= max;

/** Client-side bounds check before a preview is requested (the server re-checks everything). */
export function distillRequestProblems(req: DistillRequest): string[] {
  const out: string[] = [];
  if (req.sources.length < 1) out.push('Add at least one source.');
  if (req.sources.length > DISTILL_LIMITS.sources) out.push(`At most ${DISTILL_LIMITS.sources} sources per run.`);
  for (const s of req.sources) {
    if (s.kind === 'pasted_text') {
      if (!s.label.trim() || scalarLength(s.label) > DISTILL_LIMITS.labelScalars) {
        out.push(`Every pasted source needs a label of 1–${DISTILL_LIMITS.labelScalars} characters.`);
      }
      if (!s.text.trim()) out.push('A pasted source is empty.');
    } else if (!s.root || !s.path.trim()) {
      out.push('Every local file needs a granted folder and a relative path.');
    }
  }
  const b = req.budget;
  if (!inRange(b.max_total_bytes, 1, DISTILL_LIMITS.maxTotalBytes)) out.push(`Byte budget must be 1–${DISTILL_LIMITS.maxTotalBytes}.`);
  if (!inRange(b.max_candidates, 1, DISTILL_LIMITS.maxCandidates)) out.push(`Candidate budget must be 1–${DISTILL_LIMITS.maxCandidates}.`);
  if (!inRange(b.deadline_ms, 1, DISTILL_LIMITS.deadlineMs)) out.push(`Deadline must be 1–${DISTILL_LIMITS.deadlineMs} ms.`);
  if (!req.platform.trim() || scalarLength(req.platform) > DISTILL_LIMITS.platformScalars) out.push('Platform is required.');
  if (!req.license.trim() || scalarLength(req.license) > DISTILL_LIMITS.licenseScalars) {
    out.push(`Licence is required (at most ${DISTILL_LIMITS.licenseScalars} characters; "unknown" is allowed).`);
  }
  if (req.topics.length > DISTILL_LIMITS.topics) out.push(`At most ${DISTILL_LIMITS.topics} topics.`);
  return [...new Set(out)];
}

export const parseTopics = (text: string) => text.split(',').map(t => t.trim()).filter(Boolean);

/** Best-guess applicability platform string ("windows" | "macos" | "linux"); always user-editable. */
export function defaultPlatform(): string {
  const ua = typeof navigator !== 'undefined' ? navigator.userAgent || '' : '';
  if (/windows/i.test(ua)) return 'windows';
  if (/mac os|macintosh/i.test(ua)) return 'macos';
  return 'linux';
}

/** Current mode needs a complete exact file identity (Stage 3 rule). */
export function completeTrustedSource(source: TrustedSource): TrustedSource | null {
  const trimmed = {
    source_id: source.source_id.trim(),
    source_version: source.source_version.trim(),
    source_commit: source.source_commit.trim(),
    file_digest: source.file_digest.trim(),
  };
  return Object.values(trimmed).every(v => v.length > 0) ? trimmed : null;
}

export interface Gate { ok: boolean; reasons: string[] }

/**
 * "Save as candidate" is offered only when the server's task view shows the
 * build and tests passed on the current content and every changed file is
 * (freshly) accepted. The server re-checks and answers `NotReady` otherwise.
 */
export function canProposeCandidate(view: TaskView, stale: ReadonlySet<string>): Gate {
  const reasons: string[] = [];
  if (variantOf(view.outcome.build_status) !== 'passed') reasons.push('The build has not passed on the current content.');
  if (variantOf(view.outcome.test_status) !== 'passed') reasons.push('The tests have not passed on the current content.');
  if (view.diff.length === 0) reasons.push('No changed files.');
  const notAccepted = view.diff.filter(d => effectiveDecision(d, stale) !== 'accepted');
  if (notAccepted.length > 0) {
    reasons.push(`${notAccepted.length} changed file${notAccepted.length === 1 ? ' is' : 's are'} not accepted by you.`);
  }
  if (!/^[0-9a-f]{64}$/.test(view.change_set_sha256)) reasons.push('The task view carries no change-set hash.');
  return { ok: reasons.length === 0, reasons };
}

export function supportReason(support: VerificationSupport | null | undefined): string | null {
  if (!support) return null;
  return support.kind === 'unsupported' ? support.reason : null;
}

/** Real exit code wording for a check (never inferred from `passed`). */
export function checkExitLabel(check: CheckSummary): string {
  return check.status === null || check.status === undefined
    ? `no exit code (${check.termination.replaceAll('_', ' ')})`
    : `exit ${check.status}`;
}
