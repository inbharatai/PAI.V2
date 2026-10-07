import { useCallback, useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import { tauriApi } from '../lib/tauri';
import { formatTime, type ChipTone } from '../lib/codingTask';
import {
  DEFAULT_BUDGET,
  DISTILL_LIMITS,
  EDGE_LABEL,
  EVIDENCE_KIND_LABEL,
  INDEX_LABEL,
  KIND_FILTERS,
  KNOWLEDGE_UPDATED_EVENT,
  TRAINING_EXPORT_DISABLED,
  checkExitLabel,
  completeTrustedSource,
  defaultPlatform,
  distillRequestProblems,
  exclusionLabel,
  isExportableKind,
  kindLabel,
  knowledgeApi,
  newUiEventId,
  parseTopics,
  refKey,
  refLabel,
  scalarLength,
  shortDigest,
  type DistillReport,
  type DistillRequest,
  type DistillRunSummary,
  type DistillSource,
  type ExportBundle,
  type ExportPreview,
  type ExportRequest,
  type KnowledgeDetailView,
  type KnowledgeHitView,
  type KnowledgeKind,
  type KnowledgeListFilter,
  type KnowledgeListView,
  type KnowledgeQuery,
  type KnowledgeSearchView,
  type KnowledgeStatus,
  type KnowledgeUpdatedPayload,
  type ListState,
  type RecallMode,
  type RecordRef,
  type SourceBadge,
  type TrustedSource,
} from '../lib/knowledge';

// Stage 6 "Knowledge" view (design §3.2). A REVIEW surface over the one
// canonical encrypted vault:
// - on mount it reads only knowledge_status (and knowledge_list when the
//   store is initialized); every state-changing command needs a click and
//   reject / revoke / distill / export need a second confirming click that
//   sends the refs and hashes on screen;
// - the actions offered on a record are exactly the server's
//   `allowed_actions`;
// - every title, snippet, statement, excerpt and content string comes from a
//   source document and is shown as untrusted text only;
// - the deterministic method, the "heuristic" contradiction rule and the
//   historical (audit-only) recall mode are always labelled.

type Tab = 'explorer' | 'detail' | 'distiller' | 'export';
const TABS: { id: Tab; label: string }[] = [
  { id: 'explorer', label: 'Explorer' },
  { id: 'detail', label: 'Detail' },
  { id: 'distiller', label: 'Distiller' },
  { id: 'export', label: 'Export' },
];
const LIST_LIMIT = 50;
const SEARCH_LIMIT = 16;
const EXPORT_LIST_LIMIT = 100;
const MAX_QUERY_SCALARS = 256;
const MAX_REASON_SCALARS = 1024;
const MAX_EXPORT_REFS = 64;
const EMPTY_SOURCE: TrustedSource = { source_id: '', source_version: '', source_commit: '', file_digest: '' };
const LIST_STATES: { id: ListState; label: string }[] = [
  { id: 'all', label: 'All states' },
  { id: 'active', label: 'Active only' },
  { id: 'invalidated', label: 'Invalidated only' },
  { id: 'contradictory', label: 'Contradictory only' },
];

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

type Results =
  | { type: 'list'; view: KnowledgeListView; filter: KnowledgeListFilter }
  | { type: 'search'; view: KnowledgeSearchView; query: KnowledgeQuery };

interface ExportChoice { reference: RecordRef; title: string; hit: KnowledgeHitView | null }

/** Listed exportable records first, then refs added from Detail; deduplicated by exact reference. */
function mergeExportChoices(listed: readonly KnowledgeHitView[], extra: readonly RecordRef[]): ExportChoice[] {
  const seen = new Set<string>();
  const out: ExportChoice[] = [];
  for (const hit of listed) {
    if (seen.has(refKey(hit.reference))) continue;
    seen.add(refKey(hit.reference));
    out.push({ reference: hit.reference, title: hit.title, hit });
  }
  for (const ref of extra) {
    if (seen.has(refKey(ref))) continue;
    seen.add(refKey(ref));
    out.push({ reference: ref, title: refLabel(ref), hit: null });
  }
  return out;
}

function Chip({ tone, children, kn }: { tone: ChipTone; children: ReactNode; kn?: string }) {
  return (
    <span className={`ct-chip ct-tone-${tone}`} data-kn={kn}>
      {children}
    </span>
  );
}

function SourceBadges({ source }: { source: SourceBadge }) {
  return (
    <span className="kn-badges" data-kn="source-badges">
      <Chip tone="neutral" kn="badge-source">source {source.source_id}</Chip>
      <Chip tone="neutral" kn="badge-version">version {shortDigest(source.source_version)}</Chip>
      <Chip tone="neutral" kn="badge-licence">licence {source.license}</Chip>
      <Chip tone="neutral" kn="badge-privacy">privacy {source.privacy}</Chip>
      <Chip tone="neutral" kn="badge-platform">platform {source.platforms.length ? source.platforms.join(', ') : 'none'}</Chip>
    </span>
  );
}

function StateBadges({ active, contradictory, invalidated }: { active: boolean; contradictory: boolean; invalidated: boolean }) {
  return (
    <>
      {invalidated && <Chip tone="bad" kn="badge-invalidated">Invalidated</Chip>}
      {contradictory && <Chip tone="warn" kn="badge-contradictory">Contradictory</Chip>}
      {!active && !invalidated && <Chip tone="neutral" kn="badge-inactive">Not active</Chip>}
    </>
  );
}

function RefButton({ reference, onOpen, disabled }: { reference: RecordRef; onOpen: (r: RecordRef) => void; disabled: boolean }) {
  return (
    <button
      className="btn btn-ghost btn-sm kn-ref"
      onClick={() => onOpen(reference)}
      disabled={disabled}
      aria-label={`Open ${refLabel(reference)}`}
      title={`digest ${reference.content_digest}`}
    >
      <code>{refLabel(reference)}</code>
    </button>
  );
}

function HitCard({ hit, onOpen, disabled }: { hit: KnowledgeHitView; onOpen: (r: RecordRef) => void; disabled: boolean }) {
  return (
    <li className="kn-hit" data-kn="hit" data-kind={hit.kind} data-id={hit.reference.logical_id}>
      <div className="ct-row">
        <Chip tone={hit.kind === 'approved_procedure' || hit.kind === 'verified_pattern' ? 'ok' : 'neutral'} kn="hit-kind">
          {kindLabel(hit.kind)}
        </Chip>
        <strong className="kn-title" data-kn="hit-title">{hit.title}</strong>
        <StateBadges active={hit.active} contradictory={hit.contradictory} invalidated={hit.invalidated} />
        <Chip tone={hit.mode === 'historical' ? 'warn' : 'neutral'} kn="hit-mode">
          {hit.mode === 'historical' ? 'Historical — audit only' : 'Current — exact file identity'}
        </Chip>
      </div>
      <SourceBadges source={hit.source} />
      {hit.snippet && (
        <p className="kn-snippet" data-kn="hit-snippet">
          <span className="ct-muted">From the source (untrusted text): </span>
          {hit.snippet}
        </p>
      )}
      <p className="ct-muted" data-kn="hit-why">Why recalled: {hit.why_recalled}</p>
      <div className="ct-row">
        <button
          className="btn btn-secondary btn-sm"
          onClick={() => onOpen(hit.reference)}
          disabled={disabled}
          aria-label={`Open detail of ${refLabel(hit.reference)}`}
        >
          Open detail
        </button>
        <span className="ct-muted">r{hit.reference.revision} · digest {shortDigest(hit.reference.content_digest)}</span>
      </div>
    </li>
  );
}

export function KnowledgeView() {
  const [status, setStatus] = useState<KnowledgeStatus | null>(null);
  const [statusError, setStatusError] = useState('');
  const [tab, setTab] = useState<Tab>('explorer');
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');

  // Explorer
  const [text, setText] = useState('');
  const [mode, setMode] = useState<RecallMode>('historical');
  const [platform, setPlatform] = useState(defaultPlatform);
  const [sourceDraft, setSourceDraft] = useState<TrustedSource>(EMPTY_SOURCE);
  const [kinds, setKinds] = useState<KnowledgeKind[]>([]);
  const [listState, setListState] = useState<ListState>('all');
  const [results, setResults] = useState<Results | null>(null);

  // Detail
  const [detail, setDetail] = useState<KnowledgeDetailView | null>(null);
  const [rejectDialog, setRejectDialog] = useState<{ target: RecordRef; reason: string; boundTo: string } | null>(null);
  const [revokeDialog, setRevokeDialog] = useState<{ approved: RecordRef; boundTo: string } | null>(null);

  // Distiller
  const [sources, setSources] = useState<DistillSource[]>([]);
  const [pasteLabel, setPasteLabel] = useState('');
  const [pasteText, setPasteText] = useState('');
  const [grantedRoots, setGrantedRoots] = useState<string[] | null>(null);
  const [grantError, setGrantError] = useState('');
  const [fileRoot, setFileRoot] = useState('');
  const [filePath, setFilePath] = useState('');
  const [budget, setBudget] = useState({
    max_total_bytes: String(DEFAULT_BUDGET.max_total_bytes),
    max_candidates: String(DEFAULT_BUDGET.max_candidates),
    deadline_ms: String(DEFAULT_BUDGET.deadline_ms),
  });
  const [license, setLicense] = useState('unknown');
  const [topicsText, setTopicsText] = useState('');
  const [distillPreview, setDistillPreview] = useState<{ json: string; request: DistillRequest; hash: string } | null>(null);
  const [distillConfirm, setDistillConfirm] = useState(false);
  const [report, setReport] = useState<DistillReport | null>(null);
  const [runs, setRuns] = useState<DistillRunSummary[] | null>(null);
  const [runsError, setRunsError] = useState('');

  // Export
  const [exportOptions, setExportOptions] = useState<KnowledgeHitView[] | null>(null);
  const [extraExport, setExtraExport] = useState<RecordRef[]>([]);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [includeEvidence, setIncludeEvidence] = useState(false);
  const [exportPreview, setExportPreview] = useState<{ json: string; request: ExportRequest; preview: ExportPreview } | null>(null);
  const [ackPrivate, setAckPrivate] = useState(false);
  const [exportConfirm, setExportConfirm] = useState(false);
  const [bundle, setBundle] = useState<ExportBundle | null>(null);

  const mounted = useRef(false);
  const busyRef = useRef<string | null>(null);
  const grantsLoaded = useRef(false);
  const runsLoaded = useRef(false);
  const exportLoaded = useRef(false);

  const loadStatus = useCallback(async (): Promise<KnowledgeStatus | null> => {
    try {
      const next = await knowledgeApi.status();
      if (mounted.current) {
        setStatus(next);
        setStatusError('');
      }
      return next;
    } catch (e) {
      if (mounted.current) setStatusError(`Could not read the knowledge status: ${errorText(e)}`);
      return null;
    }
  }, []);

  // Mount: status, then the first page of records when initialized. Nothing else.
  useEffect(() => {
    let active = true;
    mounted.current = true;
    void (async () => {
      const first = await loadStatus();
      if (!active || !first?.initialized) return;
      const filter: KnowledgeListFilter = { kinds: [], state: 'all', offset: 0, limit: LIST_LIMIT };
      try {
        const view = await knowledgeApi.list(filter);
        if (active) setResults({ type: 'list', view, filter });
      } catch (e) {
        if (active) setError(`Could not list knowledge records: ${errorText(e)}`);
      }
    })();
    return () => { active = false; mounted.current = false; };
  }, [loadStatus]);

  // The content-free update event refreshes the status only (no content polling).
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<KnowledgeUpdatedPayload>(KNOWLEDGE_UPDATED_EVENT, () => {
      if (disposed || !mounted.current) return;
      void loadStatus();
    }).then(fn => { if (disposed) fn(); else unlisten = fn; }).catch(() => undefined);
    return () => { disposed = true; unlisten?.(); };
  }, [loadStatus]);

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

  const loadRuns = useCallback(async () => {
    try {
      const list = await knowledgeApi.distillRuns();
      if (mounted.current) { setRuns(list); setRunsError(''); }
    } catch (e) {
      if (mounted.current) setRunsError(`Could not read past distillation runs: ${errorText(e)}`);
    }
  }, []);

  const loadGrantedRoots = useCallback(async () => {
    try {
      const info = await tauriApi.getAgentWorkspaceInfo();
      const roots = [info.effective_root, ...info.folders.filter(f => f.exists).map(f => f.root)]
        .map(r => r.trim())
        .filter(Boolean);
      const unique = [...new Set(roots)];
      if (!mounted.current) return;
      setGrantedRoots(unique);
      setGrantError('');
      setFileRoot(current => (current && unique.includes(current) ? current : unique[0] ?? ''));
    } catch (e) {
      if (mounted.current) { setGrantedRoots([]); setGrantError(`Could not read your granted folders: ${errorText(e)}`); }
    }
  }, []);

  const loadExportOptions = useCallback(async () => {
    try {
      const view = await knowledgeApi.list({ kinds: ['verified_pattern', 'approved_procedure'], state: 'active', offset: 0, limit: EXPORT_LIST_LIMIT });
      if (mounted.current) setExportOptions(view.items.filter(item => isExportableKind(item.kind)));
    } catch (e) {
      if (mounted.current) { setExportOptions([]); setError(`Could not list exportable records: ${errorText(e)}`); }
    }
  }, []);

  // Read-only loads happen when their tab is opened (a click), once per mount.
  useEffect(() => {
    if (tab === 'distiller') {
      if (!grantsLoaded.current) {
        grantsLoaded.current = true;
        void loadGrantedRoots();
      }
      if (!runsLoaded.current && status?.initialized) {
        runsLoaded.current = true;
        void loadRuns();
      }
    }
    if (tab === 'export' && !exportLoaded.current && status?.initialized) {
      exportLoaded.current = true;
      void loadExportOptions();
    }
  }, [tab, status, loadGrantedRoots, loadRuns, loadExportOptions]);

  // A confirm dialog is bound to the record that was on screen when it opened.
  const detailKey = detail ? refKey(detail.reference) : null;
  useEffect(() => {
    setRejectDialog(d => (d && d.boundTo !== detailKey ? null : d));
    setRevokeDialog(d => (d && d.boundTo !== detailKey ? null : d));
  }, [detailKey]);

  // ------------------------------------------------------------- derived
  const isBusy = busy !== null;
  const initialized = status?.initialized === true;
  const trusted = completeTrustedSource(sourceDraft);
  const effectiveMode: RecallMode = mode === 'current' && trusted ? 'current' : 'historical';
  const orderedKinds = KIND_FILTERS.map(k => k.kind).filter(k => kinds.includes(k));

  const distillRequest: DistillRequest = {
    sources,
    budget: {
      max_total_bytes: Number(budget.max_total_bytes),
      max_candidates: Number(budget.max_candidates),
      deadline_ms: Number(budget.deadline_ms),
    },
    platform: platform.trim(),
    license: license.trim(),
    topics: parseTopics(topicsText),
  };
  const distillJson = JSON.stringify(distillRequest);
  const distillProblems = distillRequestProblems(distillRequest);
  const distillPreviewCurrent = distillPreview !== null && distillPreview.json === distillJson;

  const exportChoices = mergeExportChoices(exportOptions ?? [], extraExport);
  const exportRequest: ExportRequest = {
    references: exportChoices.filter(c => selected.has(refKey(c.reference))).map(c => c.reference),
    include_evidence_content: includeEvidence,
  };
  const exportJson = JSON.stringify(exportRequest);
  const exportPreviewCurrent = exportPreview !== null && exportPreview.json === exportJson;
  const exportNeedsAck = exportPreview?.preview.contains_private_content === true;

  // ------------------------------------------------------------- actions
  const initialize = () => {
    void run('Initialize knowledge store', async () => {
      const next = await knowledgeApi.initialize({ ui_event_id: newUiEventId() });
      if (mounted.current) setStatus(next);
    });
  };
  const rebuildIndex = () => {
    void run('Rebuild index', async () => {
      const next = await knowledgeApi.rebuildIndex();
      if (mounted.current) {
        setStatus(next);
        setNotice('Index rebuilt from the encrypted catalog.');
      }
    });
  };
  const listRecords = (offset: number) => {
    const filter: KnowledgeListFilter = { kinds: orderedKinds, state: listState, offset, limit: LIST_LIMIT };
    void run('List records', async () => {
      const view = await knowledgeApi.list(filter);
      if (mounted.current) setResults({ type: 'list', view, filter });
    });
  };
  const search = () => {
    const q = text.trim();
    if (!q) return;
    if (scalarLength(q) > MAX_QUERY_SCALARS) {
      setError(`Search text is limited to ${MAX_QUERY_SCALARS} characters.`);
      return;
    }
    const query: KnowledgeQuery = {
      text: q,
      mode: effectiveMode,
      platform: platform.trim(),
      trusted_source: effectiveMode === 'current' ? trusted : null,
      kinds: orderedKinds,
      limit: SEARCH_LIMIT,
    };
    void run('Search', async () => {
      const view = await knowledgeApi.search(query);
      if (mounted.current) setResults({ type: 'search', view, query });
    });
  };
  const openRecord = (reference: RecordRef) => {
    void run('Open record', async () => {
      const next = await knowledgeApi.detail(reference.logical_id, reference.revision);
      if (!mounted.current) return;
      setDetail(next);
      setTab('detail');
    });
  };
  const confirmReject = () => {
    const dialog = rejectDialog;
    if (!dialog) return;
    const reason = dialog.reason.trim();
    if (!reason || scalarLength(reason) > MAX_REASON_SCALARS) return;
    void run('Reject', async () => {
      const next = await knowledgeApi.reject({ target: dialog.target, reason, ui_event_id: newUiEventId() });
      if (!mounted.current) return;
      setRejectDialog(null);
      setDetail(next);
      setNotice('An Invalidation record was appended. The rejected record stays readable for audit.');
      void loadStatus();
    });
  };
  const confirmRevoke = () => {
    const dialog = revokeDialog;
    if (!dialog) return;
    void run('Revoke approval', async () => {
      const next = await knowledgeApi.revokeApproval({ approved: dialog.approved, ui_event_id: newUiEventId() });
      if (!mounted.current) return;
      setRevokeDialog(null);
      setDetail(next);
      setNotice('The approval was revoked by an appended Invalidation record.');
      void loadStatus();
    });
  };
  const addToExport = (reference: RecordRef) => {
    setExtraExport(list => (list.some(r => refKey(r) === refKey(reference)) ? list : [...list, reference]));
    setSelected(s => new Set([...s, refKey(reference)]));
    setTab('export');
  };

  const addPasted = () => {
    if (!pasteLabel.trim() || !pasteText.trim()) return;
    setSources(list => [...list, { kind: 'pasted_text', label: pasteLabel.trim(), text: pasteText }]);
    setPasteLabel('');
    setPasteText('');
  };
  const addLocalFile = () => {
    if (!fileRoot || !filePath.trim()) return;
    setSources(list => [...list, { kind: 'local_file', root: fileRoot, path: filePath.trim() }]);
    setFilePath('');
  };
  const previewDistill = () => {
    if (distillProblems.length > 0) return;
    const snapshot = JSON.parse(distillJson) as DistillRequest;
    void run('Preview distillation', async () => {
      const preview = await knowledgeApi.distillPreview(snapshot);
      if (mounted.current) setDistillPreview({ json: distillJson, request: snapshot, hash: preview.request_sha256 });
    });
  };
  const confirmDistill = () => {
    const preview = distillPreview;
    if (!preview || preview.json !== distillJson) return;
    void run('Run distillation', async () => {
      const result = await knowledgeApi.distill(preview.request, { request_sha256: preview.hash, ui_event_id: newUiEventId() });
      if (!mounted.current) return;
      setReport(result);
      setDistillConfirm(false);
      setDistillPreview(null);
      void loadStatus();
      void loadRuns();
    });
  };

  const previewExport = () => {
    if (exportRequest.references.length === 0 || exportRequest.references.length > MAX_EXPORT_REFS) return;
    const snapshot = JSON.parse(exportJson) as ExportRequest;
    void run('Preview export', async () => {
      const preview = await knowledgeApi.exportPreview(snapshot);
      if (!mounted.current) return;
      setExportPreview({ json: exportJson, request: snapshot, preview });
      setAckPrivate(false);
      setBundle(null);
    });
  };
  const confirmExport = () => {
    const preview = exportPreview;
    if (!preview || preview.json !== exportJson) return;
    if (preview.preview.contains_private_content && !ackPrivate) return;
    void run('Export', async () => {
      const result = await knowledgeApi.export({
        request: preview.request,
        request_sha256: preview.preview.request_sha256,
        ui_event_id: newUiEventId(),
        acknowledged_private: ackPrivate,
      });
      if (!mounted.current) return;
      setBundle(result);
      setExportConfirm(false);
    });
  };

  const dialogKeys = (e: ReactKeyboardEvent<HTMLDivElement>, close: () => void) => {
    if (e.key === 'Escape') close();
  };

  // ------------------------------------------------------------- render pieces
  const header = (
    <div className="main-header">
      <h2>Knowledge</h2>
      <div className="main-header-actions kn-header-actions">
        <button className="btn btn-secondary btn-sm" onClick={() => { void loadStatus(); }} disabled={isBusy}>
          Refresh status
        </button>
      </div>
    </div>
  );

  const statusSection = (
    <section className="settings-section" aria-labelledby="kn-status-title">
      <div className="settings-section-header" id="kn-status-title">Knowledge store (one encrypted vault)</div>
      <div className="settings-section-body">
        {statusError && <div className="ct-alert" role="alert">{statusError}</div>}
        {!status && !statusError && <p className="ct-muted">Reading the knowledge status…</p>}
        {status && (
          <>
            <div className="ct-row" data-kn="status">
              <Chip tone={status.initialized ? 'ok' : 'warn'} kn="initialized">
                {status.initialized ? 'Initialized' : 'Not initialized'}
              </Chip>
              <Chip tone={status.index === 'fresh' ? 'ok' : 'warn'} kn="index-state">{INDEX_LABEL[status.index] ?? status.index}</Chip>
              <span className="ct-muted">{status.catalog_entries} catalog entries</span>
            </div>
            <p className="kn-method" data-kn="method">
              Method: <code>{status.method}</code> — deterministic extraction; no model and no network are used.
            </p>
            <table className="ct-table" data-kn="counts">
              <thead>
                <tr><th scope="col">Kind</th><th scope="col">Records</th><th scope="col">Active</th></tr>
              </thead>
              <tbody>
                {KIND_FILTERS.map(k => (
                  <tr key={k.kind} data-kn="count-row" data-kind={k.kind}>
                    <td>{kindLabel(k.kind)}</td>
                    <td>{status.counts[k.kind] ?? 0}</td>
                    <td>{status.active_counts[k.kind] ?? 0}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            {!status.initialized && (
              <div className="ct-banner" role="note" data-kn="init-banner">
                The knowledge store is not initialized in this vault. Initializing writes its encrypted catalog; it
                imports nothing.{' '}
                <button className="btn btn-primary btn-sm" onClick={initialize} disabled={isBusy}>
                  Initialize knowledge store
                </button>
              </div>
            )}
            {status.initialized && status.index !== 'fresh' && (
              <div className="ct-banner" role="note" data-kn="index-banner">
                The search index is {status.index}: records written since the last rebuild are not searchable yet.
                Search never rebuilds it on its own.{' '}
                <button className="btn btn-secondary btn-sm" onClick={rebuildIndex} disabled={isBusy}>
                  {busy === 'Rebuild index' ? 'Rebuilding…' : 'Rebuild index'}
                </button>
              </div>
            )}
            {status.residuals.length > 0 && (
              <details className="ct-details">
                <summary>Known limits of the knowledge store</summary>
                <ul className="ct-list">{status.residuals.map((r, i) => <li key={i}>{r}</li>)}</ul>
              </details>
            )}
          </>
        )}
      </div>
    </section>
  );

  const explorer = (
    <div role="tabpanel" id="kn-panel-explorer" aria-labelledby="kn-tab-explorer" data-kn="panel-explorer">
      <section className="settings-section" aria-labelledby="kn-search-title">
        <div className="settings-section-header" id="kn-search-title">Search and browse</div>
        <div className="settings-section-body">
          <div className="ct-row">
            <input
              type="text"
              className="kn-input kn-grow"
              placeholder="Search terms (lexical match)…"
              value={text}
              onChange={e => setText(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter') search(); }}
              aria-label="Knowledge search text"
              maxLength={1024}
            />
            <button className="btn btn-primary" onClick={search} disabled={isBusy || !initialized || !text.trim()}>
              Search
            </button>
            <button className="btn btn-secondary" onClick={() => listRecords(0)} disabled={isBusy || !initialized}>
              List records
            </button>
          </div>
          <div className="ct-row">
            <label className="ct-inline-label">
              Recall mode
              <select value={effectiveMode} onChange={e => setMode(e.target.value as RecallMode)} aria-label="Recall mode">
                <option value="historical">Historical (audit only)</option>
                <option value="current" disabled={!trusted}>Current (needs exact file identity)</option>
              </select>
            </label>
            <label className="ct-inline-label">
              Platform
              <input type="text" className="kn-input kn-short" value={platform} onChange={e => setPlatform(e.target.value)} aria-label="Platform" />
            </label>
            <label className="ct-inline-label">
              List state
              <select value={listState} onChange={e => setListState(e.target.value as ListState)} aria-label="List state filter">
                {LIST_STATES.map(s => <option key={s.id} value={s.id}>{s.label}</option>)}
              </select>
            </label>
          </div>
          <p className="ct-muted" data-kn="mode-note">
            Current mode returns only records bound to an exact file identity (source id, version, commit and file
            digest) that you supply. Without it the Explorer searches in historical audit mode: results may be stale,
            revoked or contradictory and are shown for audit only. Matching is lexical, not semantic.
          </p>
          <fieldset className="ct-fieldset" aria-label="Kind filters">
            <legend>Kinds (none ticked = all)</legend>
            <div className="ct-row">
              {KIND_FILTERS.map(k => (
                <label key={k.kind} className="ct-check" data-kn="kind-filter">
                  <input
                    type="checkbox"
                    checked={kinds.includes(k.kind)}
                    onChange={e => {
                      const checked = e.target.checked;
                      setKinds(list => (checked ? [...list, k.kind] : list.filter(x => x !== k.kind)));
                    }}
                    aria-label={`Filter ${k.label}`}
                  />
                  <span>{k.label}</span>
                </label>
              ))}
            </div>
          </fieldset>
          <details className="ct-details">
            <summary>Exact file identity for current mode (optional)</summary>
            <div className="kn-grid">
              {(['source_id', 'source_version', 'source_commit', 'file_digest'] as const).map(field => (
                <label key={field} className="ct-inline-label">
                  {field.replace('_', ' ')}
                  <input
                    type="text"
                    className="kn-input"
                    value={sourceDraft[field]}
                    onChange={e => {
                      const value = e.target.value;
                      setSourceDraft(s => ({ ...s, [field]: value }));
                    }}
                    aria-label={`Trusted ${field.replace('_', ' ')}`}
                  />
                </label>
              ))}
            </div>
          </details>
        </div>
      </section>

      <section className="settings-section" aria-labelledby="kn-results-title" data-kn="results">
        <div className="settings-section-header" id="kn-results-title">
          {results?.type === 'search' ? `Search results (${results.view.hits.length})` : results ? `Records (${results.view.total})` : 'Records'}
        </div>
        <div className="settings-section-body">
          {!initialized && <p className="ct-muted">Initialize the knowledge store to browse records.</p>}
          {results?.type === 'search' && (
            <>
              <p className="ct-muted" data-kn="search-note">{results.view.note}</p>
              <p className="ct-muted" data-kn="normalization">
                Normalization: {results.view.normalization.algorithm} · terms {results.view.normalization.terms.join(', ') || 'none'}
                {results.view.normalization.declared_aliases_used.length ? ` · aliases ${results.view.normalization.declared_aliases_used.join(', ')}` : ''}
              </p>
              {results.view.index !== 'fresh' && (
                <p className="ct-warn-text" data-kn="search-index">{INDEX_LABEL[results.view.index] ?? results.view.index}</p>
              )}
              {results.query.mode === 'historical' && (
                <p className="ct-warn-text" data-kn="historical-label">Historical audit results — not current guidance.</p>
              )}
            </>
          )}
          {results && (results.type === 'search' ? results.view.hits : results.view.items).length === 0 && (
            <p className="ct-muted">No records match.</p>
          )}
          {results && (
            <ul className="kn-hits" data-kn="hits">
              {(results.type === 'search' ? results.view.hits : results.view.items).map(hit => (
                <HitCard key={refKey(hit.reference)} hit={hit} onOpen={openRecord} disabled={isBusy} />
              ))}
            </ul>
          )}
          {results?.type === 'list' && results.view.total > LIST_LIMIT && (
            <div className="ct-row">
              <button
                className="btn btn-secondary btn-sm"
                onClick={() => listRecords(Math.max(0, results.view.offset - LIST_LIMIT))}
                disabled={isBusy || results.view.offset === 0}
              >
                Previous page
              </button>
              <span className="ct-muted">
                {results.view.offset + 1}–{results.view.offset + results.view.items.length} of {results.view.total}
              </span>
              <button
                className="btn btn-secondary btn-sm"
                onClick={() => listRecords(results.view.offset + LIST_LIMIT)}
                disabled={isBusy || results.view.offset + results.view.items.length >= results.view.total}
              >
                Next page
              </button>
            </div>
          )}
        </div>
      </section>
    </div>
  );

  const detailBody = (d: KnowledgeDetailView): ReactNode => {
    const body = d.body;
    switch (body.kind) {
      case 'evidence':
        return (
          <div data-kn="body-evidence">
            <p className="ct-muted">
              Evidence kind: {EVIDENCE_KIND_LABEL[body.evidence_kind] ?? body.evidence_kind} · content SHA-256 <code>{body.content_sha256}</code>
            </p>
            <p className="ct-muted">Content from the source (untrusted text){body.truncated ? ' — truncated at the display bound' : ''}:</p>
            <pre className="ct-pre" data-kn="evidence-content">{body.content}</pre>
          </div>
        );
      case 'candidate':
        return (
          <div data-kn="body-candidate">
            <p className="ct-muted">Statement (extracted from the source; untrusted text):</p>
            <p className="kn-statement" data-kn="statement">{body.statement}</p>
            <p className="ct-muted">Cited evidence ({body.evidence.length}):</p>
            <ul className="ct-list" data-kn="citations">
              {body.evidence.map(r => <li key={refKey(r)}><RefButton reference={r} onOpen={openRecord} disabled={isBusy} /></li>)}
            </ul>
          </div>
        );
      case 'verified_pattern':
        return (
          <div data-kn="body-verified">
            <p className="ct-muted">Statement (untrusted text):</p>
            <p className="kn-statement" data-kn="statement">{body.statement}</p>
            <p className="ct-muted">From candidate: <RefButton reference={body.candidate} onOpen={openRecord} disabled={isBusy} /></p>
            <p className="ct-muted">Check records ({body.checks.length}):</p>
            <ul className="ct-list">
              {body.checks.map(r => <li key={refKey(r)}><RefButton reference={r} onOpen={openRecord} disabled={isBusy} /></li>)}
            </ul>
          </div>
        );
      case 'approved_procedure':
        return (
          <div data-kn="body-approved">
            <p className="ct-muted">Approves pattern: <RefButton reference={body.pattern} onOpen={openRecord} disabled={isBusy} /></p>
            <p className="ct-muted">Explicit UI approval evidence: <RefButton reference={body.approval_evidence} onOpen={openRecord} disabled={isBusy} /></p>
            <p className="ct-muted">Outcome evidence ({body.outcome_evidence.length}):</p>
            <ul className="ct-list">
              {body.outcome_evidence.map(r => <li key={refKey(r)}><RefButton reference={r} onOpen={openRecord} disabled={isBusy} /></li>)}
            </ul>
          </div>
        );
      case 'invalidation':
        return (
          <div data-kn="body-invalidation">
            <p className="ct-muted">Invalidates: <RefButton reference={body.target} onOpen={openRecord} disabled={isBusy} /></p>
            <p className="ct-muted">Reason (untrusted text):</p>
            <p className="kn-statement">{body.reason}</p>
          </div>
        );
      default:
        return <p className="ct-muted">Unrecognised record body.</p>;
    }
  };

  const revokeTarget = (d: KnowledgeDetailView): RecordRef | null =>
    d.kind === 'approved_procedure' ? d.reference : d.verification?.approval ?? null;

  const detailPanel = (
    <div role="tabpanel" id="kn-panel-detail" aria-labelledby="kn-tab-detail" data-kn="panel-detail">
      {!detail ? (
        <div className="empty-state"><p>Open a record from the Explorer to see its detail.</p></div>
      ) : (
        <>
          <section className="settings-section" aria-labelledby="kn-detail-title" data-kn="detail">
            <div className="settings-section-header" id="kn-detail-title">{kindLabel(detail.kind)} {detail.reference.logical_id}</div>
            <div className="settings-section-body">
              <div className="ct-row">
                <Chip tone={detail.kind === 'approved_procedure' || detail.kind === 'verified_pattern' ? 'ok' : 'neutral'} kn="detail-kind">
                  {kindLabel(detail.kind)}
                </Chip>
                <StateBadges active={detail.active} contradictory={detail.contradictory} invalidated={detail.invalidated} />
                <span className="ct-muted" data-kn="detail-ref">
                  revision {detail.reference.revision} · digest <code>{detail.reference.content_digest}</code> · {formatTime(detail.timestamp_ms)}
                </span>
              </div>
              <SourceBadges source={detail.source} />
              <p className="ct-muted">
                Source commit <code>{shortDigest(detail.source.source_commit, 16)}</code>
                {detail.source.file_digest ? <> · file digest <code>{shortDigest(detail.source.file_digest, 16)}</code></> : ' · no file digest'}
                {detail.source.topics.length ? ` · topics ${detail.source.topics.join(', ')}` : ''}
              </p>
              <p className="ct-muted">Recorded by {detail.audit.actor}: {detail.audit.reason}</p>
              {detailBody(detail)}
            </div>
          </section>

          {detail.verification && (
            <section className="settings-section" aria-labelledby="kn-verif-title" data-kn="verification">
              <div className="settings-section-header" id="kn-verif-title">Verification (recorded Stage 4 runs)</div>
              <div className="settings-section-body">
                <div className="ct-row">
                  <span>Repetitions: {detail.verification.repetitions}</span>
                  <Chip tone={detail.verification.approved ? 'ok' : 'neutral'} kn="verification-approved">
                    {detail.verification.approved ? 'Approved for reuse by you' : 'Not approved for reuse'}
                  </Chip>
                  {detail.verification.approval && <RefButton reference={detail.verification.approval} onOpen={openRecord} disabled={isBusy} />}
                </div>
                <table className="ct-table">
                  <thead>
                    <tr><th scope="col">Case</th><th scope="col">Role</th><th scope="col">Exit code</th><th scope="col">Termination</th><th scope="col">Result</th></tr>
                  </thead>
                  <tbody>
                    {detail.verification.checks.map(c => (
                      <tr key={refKey(c.reference)} data-kn="check-row">
                        <td><code>{c.case}</code></td>
                        <td>{c.role}</td>
                        <td data-kn="check-exit">{checkExitLabel(c)}</td>
                        <td>{c.termination.replaceAll('_', ' ')}</td>
                        <td>{c.passed ? 'as expected' : 'not as expected'}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </section>
          )}

          <section className="settings-section" aria-labelledby="kn-history-title">
            <div className="settings-section-header" id="kn-history-title">History and links</div>
            <div className="settings-section-body">
              <p className="ct-muted">Revisions of {detail.reference.logical_id} (oldest first):</p>
              <div className="ct-row" data-kn="history">
                {detail.history.map(r => (
                  <button
                    key={refKey(r)}
                    className={`btn btn-secondary btn-sm ${r.revision === detail.reference.revision ? 'ct-selected' : ''}`}
                    onClick={() => openRecord(r)}
                    disabled={isBusy}
                    aria-label={`Open revision ${r.revision} of ${r.logical_id}`}
                    aria-pressed={r.revision === detail.reference.revision}
                  >
                    r{r.revision}
                  </button>
                ))}
              </div>
              {detail.edges.length === 0 ? <p className="ct-muted">No links.</p> : (
                <table className="ct-table" data-kn="edges">
                  <thead>
                    <tr><th scope="col">Direction</th><th scope="col">Relation</th><th scope="col">Record</th></tr>
                  </thead>
                  <tbody>
                    {detail.edges.map((edge, i) => (
                      <tr key={`${edge.direction}-${edge.relation}-${refKey(edge.target)}-${i}`} data-kn="edge" data-direction={edge.direction}>
                        <td>{edge.direction === 'incoming' ? 'incoming' : 'outgoing'}</td>
                        <td>{EDGE_LABEL[edge.relation] ?? edge.relation}</td>
                        <td><RefButton reference={edge.target} onOpen={openRecord} disabled={isBusy} /></td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          </section>

          <section className="settings-section" aria-labelledby="kn-actions-title" data-kn="actions">
            <div className="settings-section-header" id="kn-actions-title">Actions (offered by the server for this record)</div>
            <div className="settings-section-body">
              {detail.allowed_actions.length === 0 && <p className="ct-muted">No actions are available for this record.</p>}
              <div className="ct-row">
                {detail.allowed_actions.includes('reject') && (
                  <button
                    className="btn btn-danger"
                    onClick={() => setRejectDialog({ target: detail.reference, reason: '', boundTo: refKey(detail.reference) })}
                    disabled={isBusy}
                  >
                    Reject…
                  </button>
                )}
                {detail.allowed_actions.includes('revoke_approval') && (
                  <button
                    className="btn btn-danger"
                    onClick={() => {
                      const target = revokeTarget(detail);
                      if (target) setRevokeDialog({ approved: target, boundTo: refKey(detail.reference) });
                    }}
                    disabled={isBusy || !revokeTarget(detail)}
                  >
                    Revoke approval…
                  </button>
                )}
                {detail.allowed_actions.includes('export') && (
                  <button className="btn btn-secondary" onClick={() => addToExport(detail.reference)} disabled={isBusy}>
                    Add to export
                  </button>
                )}
              </div>
              {detail.residuals.length > 0 && (
                <details className="ct-details">
                  <summary>Known limits of this record</summary>
                  <ul className="ct-list">{detail.residuals.map((r, i) => <li key={i}>{r}</li>)}</ul>
                </details>
              )}
            </div>
          </section>
        </>
      )}
    </div>
  );

  const distiller = (
    <div role="tabpanel" id="kn-panel-distiller" aria-labelledby="kn-tab-distiller" data-kn="panel-distiller">
      <section className="settings-section" aria-labelledby="kn-sources-title">
        <div className="settings-section-header" id="kn-sources-title">Distiller — sources</div>
        <div className="settings-section-body">
          <p className="kn-method" data-kn="distill-method">
            Method: <code>{status?.method ?? 'not reported yet'}</code>. Deterministic extraction of headings,
            docstrings and first lines — no model, no network. A run creates Evidence and Candidate records only;
            nothing is verified or approved by it. Held-out / evaluation material on the exclusion list is skipped and
            reported.
          </p>
          {sources.length === 0 ? <p className="ct-muted">No sources added.</p> : (
            <ul className="ct-list" data-kn="sources">
              {sources.map((s, i) => (
                <li key={i} data-kn="source" data-source-kind={s.kind}>
                  {s.kind === 'pasted_text'
                    ? <>Pasted text <strong>{s.label}</strong> ({scalarLength(s.text)} characters)</>
                    : <>Local file <code>{s.path}</code> in granted folder <code>{s.root}</code></>}{' '}
                  <button
                    className="btn btn-ghost btn-sm"
                    onClick={() => setSources(list => list.filter((_, j) => j !== i))}
                    disabled={isBusy}
                    aria-label={`Remove source ${i + 1}`}
                  >
                    Remove
                  </button>
                </li>
              ))}
            </ul>
          )}
          <fieldset className="ct-fieldset">
            <legend>Add pasted text</legend>
            <div className="kn-stack">
              <input type="text" className="kn-input" placeholder="Label" value={pasteLabel} onChange={e => setPasteLabel(e.target.value)} aria-label="Pasted source label" maxLength={512} />
              <textarea className="kn-textarea" rows={5} value={pasteText} onChange={e => setPasteText(e.target.value)} aria-label="Pasted source text" />
              <div className="ct-row">
                <button className="btn btn-secondary" onClick={addPasted} disabled={isBusy || !pasteLabel.trim() || !pasteText.trim() || sources.length >= DISTILL_LIMITS.sources}>
                  Add pasted text
                </button>
              </div>
            </div>
          </fieldset>
          <fieldset className="ct-fieldset">
            <legend>Add a local file from a granted folder</legend>
            {grantError && <p className="ct-warn-text">{grantError}</p>}
            {grantedRoots && grantedRoots.length === 0 && !grantError && (
              <p className="ct-muted">No granted folders. Grant a folder in Settings first; no grant is created here.</p>
            )}
            <div className="ct-row">
              <label className="ct-inline-label">
                Granted folder
                <select value={fileRoot} onChange={e => setFileRoot(e.target.value)} aria-label="Granted folder" disabled={!grantedRoots?.length}>
                  {(grantedRoots ?? []).map(r => <option key={r} value={r}>{r}</option>)}
                </select>
              </label>
              <input type="text" className="kn-input kn-grow" placeholder="relative/path/inside/folder.md" value={filePath} onChange={e => setFilePath(e.target.value)} aria-label="Relative file path" />
              <button className="btn btn-secondary" onClick={addLocalFile} disabled={isBusy || !fileRoot || !filePath.trim() || sources.length >= DISTILL_LIMITS.sources}>
                Add local file
              </button>
            </div>
            <p className="ct-muted">The folder is re-checked against your grants before hashing and before the run. On Windows local files are reported as unreadable (fd-safe capture is Linux-only); pasted text works everywhere.</p>
          </fieldset>
          <fieldset className="ct-fieldset">
            <legend>Budget and labels</legend>
            <div className="kn-grid">
              <label className="ct-inline-label">
                Max total bytes (≤ {DISTILL_LIMITS.maxTotalBytes})
                <input type="number" className="kn-input" min={1} max={DISTILL_LIMITS.maxTotalBytes} value={budget.max_total_bytes} onChange={e => { const v = e.target.value; setBudget(b => ({ ...b, max_total_bytes: v })); }} aria-label="Max total bytes" />
              </label>
              <label className="ct-inline-label">
                Max candidates (≤ {DISTILL_LIMITS.maxCandidates})
                <input type="number" className="kn-input" min={1} max={DISTILL_LIMITS.maxCandidates} value={budget.max_candidates} onChange={e => { const v = e.target.value; setBudget(b => ({ ...b, max_candidates: v })); }} aria-label="Max candidates" />
              </label>
              <label className="ct-inline-label">
                Deadline ms (≤ {DISTILL_LIMITS.deadlineMs})
                <input type="number" className="kn-input" min={1} max={DISTILL_LIMITS.deadlineMs} value={budget.deadline_ms} onChange={e => { const v = e.target.value; setBudget(b => ({ ...b, deadline_ms: v })); }} aria-label="Deadline ms" />
              </label>
              <label className="ct-inline-label">
                Platform
                <input type="text" className="kn-input" value={platform} onChange={e => setPlatform(e.target.value)} aria-label="Distill platform" />
              </label>
              <label className="ct-inline-label">
                Licence (your declaration)
                <input type="text" className="kn-input" value={license} onChange={e => setLicense(e.target.value)} aria-label="Licence" />
              </label>
              <label className="ct-inline-label">
                Topics (comma separated, ≤ {DISTILL_LIMITS.topics})
                <input type="text" className="kn-input" value={topicsText} onChange={e => setTopicsText(e.target.value)} aria-label="Topics" />
              </label>
            </div>
          </fieldset>
          {distillProblems.length > 0 && (
            <ul className="ct-reasons" data-kn="distill-problems" aria-label="Why Preview is disabled">
              {distillProblems.map(p => <li key={p}>{p}</li>)}
            </ul>
          )}
          <div className="ct-row">
            <button className="btn btn-secondary" onClick={previewDistill} disabled={isBusy || !initialized || distillProblems.length > 0}>
              Preview
            </button>
            <button
              className="btn btn-primary"
              onClick={() => setDistillConfirm(true)}
              disabled={isBusy || !distillPreviewCurrent}
              data-kn="distill-run"
            >
              Run…
            </button>
            {distillPreview && !distillPreviewCurrent && <span className="ct-warn-text" data-kn="distill-preview-stale">The sources changed since the preview; preview again.</span>}
          </div>
          {distillPreview && distillPreviewCurrent && (
            <div className="kn-plan" data-kn="distill-plan">
              <strong>Plan</strong>
              <ul className="ct-list">
                {distillPreview.request.sources.map((s, i) => (
                  <li key={i}>{s.kind === 'pasted_text' ? `Pasted text "${s.label}"` : `Local file ${s.path} in ${s.root}`}</li>
                ))}
              </ul>
              <p className="ct-muted">
                Budget {distillPreview.request.budget.max_total_bytes} bytes · {distillPreview.request.budget.max_candidates} candidates ·{' '}
                {distillPreview.request.budget.deadline_ms} ms · platform {distillPreview.request.platform} · licence {distillPreview.request.license}
                {distillPreview.request.topics.length ? ` · topics ${distillPreview.request.topics.join(', ')}` : ''}
              </p>
              <p className="ct-muted ct-hashes">Request hash <code data-kn="distill-hash">{distillPreview.hash}</code></p>
            </div>
          )}
        </div>
      </section>

      {report && (
        <section className="settings-section" aria-labelledby="kn-report-title" data-kn="report">
          <div className="settings-section-header" id="kn-report-title">Distillation run {shortDigest(report.run_id, 16)}</div>
          <div className="settings-section-body">
            <p className="kn-method" data-kn="report-method">Method: <code>{report.method}</code></p>
            <div className="ct-row">
              <span>{report.evidence.length} evidence record{report.evidence.length === 1 ? '' : 's'}</span>
              <span>{report.candidates.length} candidate{report.candidates.length === 1 ? '' : 's'}</span>
              <span>{report.excluded.length} excluded</span>
              <span className="ct-muted">{report.elapsed_ms} ms</span>
              {report.budget_exhausted && <Chip tone="warn" kn="budget-exhausted">Budget exhausted — the run stopped early</Chip>}
            </div>
            <p className="ct-muted">Candidates are unverified extracts. They become verified patterns only through a Stage 4 verification run, and approved procedures only through your explicit approval.</p>
            {report.candidates.length > 0 && (
              <ol className="kn-candidates" data-kn="candidates">
                {report.candidates.map(c => (
                  <li key={refKey(c.reference)} data-kn="candidate">
                    <div className="kn-statement" data-kn="candidate-statement">{c.statement}</div>
                    <div className="ct-muted" data-kn="citation">
                      Cites <RefButton reference={c.citation.evidence} onOpen={openRecord} disabled={isBusy} /> bytes {c.citation.start}–{c.citation.end}:
                    </div>
                    <blockquote className="kn-excerpt" data-kn="citation-excerpt">{c.citation.excerpt}</blockquote>
                  </li>
                ))}
              </ol>
            )}
            {report.excluded.length > 0 && (
              <div data-kn="excluded">
                <strong>Excluded sources (reported, never silently dropped)</strong>
                <ul className="ct-list">
                  {report.excluded.map((x, i) => (
                    <li key={i} data-kn="excluded-item" data-reason={x.reason}>
                      <strong>{x.label}</strong> — {exclusionLabel(x.reason)}
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {report.possible_contradictions.length > 0 && (
              <div data-kn="contradictions">
                <strong>Possible contradictions (heuristic)</strong>
                <ul className="ct-list">
                  {report.possible_contradictions.map((c, i) => (
                    <li key={i} data-kn="contradiction">
                      <RefButton reference={c.candidate} onOpen={openRecord} disabled={isBusy} /> vs{' '}
                      <RefButton reference={c.existing} onOpen={openRecord} disabled={isBusy} />{' '}
                      <span className="ct-muted">— heuristic rule <code>{c.rule}</code>; a Contradicting link was added and nothing was invalidated.</span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
            <p className="ct-muted">Run summary record: <RefButton reference={report.summary} onOpen={openRecord} disabled={isBusy} /></p>
          </div>
        </section>
      )}

      <section className="settings-section" aria-labelledby="kn-runs-title">
        <div className="settings-section-header" id="kn-runs-title">Past distillation runs</div>
        <div className="settings-section-body">
          {runsError && <p className="ct-warn-text">{runsError}</p>}
          {runs === null && !runsError && <p className="ct-muted">{initialized ? 'Loading…' : 'Initialize the knowledge store first.'}</p>}
          {runs && runs.length === 0 && <p className="ct-muted">No runs recorded.</p>}
          {runs && runs.length > 0 && (
            <table className="ct-table" data-kn="runs">
              <thead>
                <tr><th scope="col">Run</th><th scope="col">When</th><th scope="col">Evidence</th><th scope="col">Candidates</th><th scope="col">Excluded</th></tr>
              </thead>
              <tbody>
                {runs.map(r => (
                  <tr key={r.run_id}>
                    <td><code>{shortDigest(r.run_id, 16)}</code></td>
                    <td>{formatTime(r.timestamp_ms)}</td>
                    <td>{r.evidence}</td>
                    <td>{r.candidates}</td>
                    <td>{r.excluded}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </section>
    </div>
  );

  const exportPanel = (
    <div role="tabpanel" id="kn-panel-export" aria-labelledby="kn-tab-export" data-kn="panel-export">
      <section className="settings-section" aria-labelledby="kn-export-title">
        <div className="settings-section-header" id="kn-export-title">Export verified knowledge</div>
        <div className="settings-section-body">
          <p className="ct-honest" data-kn="training-export">Training export: disabled — separate consent and licence review required.</p>
          <p className="ct-muted">
            Only active, non-contradictory verified patterns and approved procedures can be exported; anything else is
            refused with a reason. The file is offered to you as a download; UnoOne writes nothing outside the vault.
          </p>
          {!initialized && <p className="ct-muted">Initialize the knowledge store first.</p>}
          {initialized && exportOptions === null && <p className="ct-muted">Loading exportable records…</p>}
          {exportChoices.length === 0 && exportOptions !== null && <p className="ct-muted">No verified patterns or approved procedures yet.</p>}
          {exportChoices.length > 0 && (
            <fieldset className="ct-fieldset">
              <legend>Select records (at most {MAX_EXPORT_REFS})</legend>
              {exportChoices.map(c => (
                <label key={refKey(c.reference)} className="ct-check" data-kn="export-choice">
                  <input
                    type="checkbox"
                    checked={selected.has(refKey(c.reference))}
                    onChange={e => {
                      const checked = e.target.checked;
                      setSelected(s => {
                        const next = new Set(s);
                        if (checked) next.add(refKey(c.reference)); else next.delete(refKey(c.reference));
                        return next;
                      });
                    }}
                    aria-label={`Select ${refLabel(c.reference)} for export`}
                  />
                  <span>
                    {kindLabel(c.reference.kind)} — {c.title}
                    {c.hit?.contradictory ? ' (contradictory)' : ''}{c.hit?.invalidated ? ' (invalidated)' : ''}
                  </span>
                </label>
              ))}
            </fieldset>
          )}
          <label className="ct-check">
            <input type="checkbox" checked={includeEvidence} onChange={e => setIncludeEvidence(e.target.checked)} aria-label="Include evidence content" />
            <span>Include evidence content (private source text; needs your acknowledgement)</span>
          </label>
          <div className="ct-row">
            <button
              className="btn btn-secondary"
              onClick={previewExport}
              disabled={isBusy || exportRequest.references.length === 0 || exportRequest.references.length > MAX_EXPORT_REFS}
            >
              Preview export
            </button>
            <button
              className="btn btn-primary"
              onClick={() => setExportConfirm(true)}
              disabled={isBusy || !exportPreviewCurrent || (exportNeedsAck && !ackPrivate) || (exportPreview?.preview.items.length ?? 0) === 0}
              data-kn="export-open"
            >
              Export…
            </button>
            {exportPreview && !exportPreviewCurrent && <span className="ct-warn-text" data-kn="export-preview-stale">The selection changed since the preview; preview again.</span>}
          </div>
          {exportPreview && exportPreviewCurrent && (
            <div className="kn-plan" data-kn="export-preview">
              <strong>Will export ({exportPreview.preview.items.length})</strong>
              <ul className="ct-list" data-kn="export-items">
                {exportPreview.preview.items.map(item => (
                  <li key={refKey(item.reference)} data-kn="export-item">{kindLabel(item.kind)} — {item.title} · licence {item.licence}</li>
                ))}
              </ul>
              {exportPreview.preview.refused.length > 0 && (
                <>
                  <strong>Refused ({exportPreview.preview.refused.length})</strong>
                  <ul className="ct-list" data-kn="export-refused">
                    {exportPreview.preview.refused.map(([ref, reason]) => (
                      <li key={refKey(ref)} data-kn="export-refused-item">{refLabel(ref)} — {reason}</li>
                    ))}
                  </ul>
                </>
              )}
              <p className="ct-muted" data-kn="export-training">Training export (server): {exportPreview.preview.training_export || TRAINING_EXPORT_DISABLED}</p>
              <p className="ct-muted ct-hashes">Request hash <code data-kn="export-hash">{exportPreview.preview.request_sha256}</code></p>
              {exportNeedsAck && (
                <label className="ct-check" data-kn="export-ack">
                  <input type="checkbox" checked={ackPrivate} onChange={e => setAckPrivate(e.target.checked)} aria-label="Acknowledge private content" />
                  <span>I understand this export contains private content from my vault.</span>
                </label>
              )}
            </div>
          )}
          {bundle && (
            <div className="kn-plan" data-kn="export-bundle" role="status">
              <p>
                Export ready: {bundle.items} item{bundle.items === 1 ? '' : 's'}, schema <code>{bundle.schema}</code>, SHA-256{' '}
                <code data-kn="export-sha">{bundle.sha256}</code>.
              </p>
              <a
                className="btn btn-secondary"
                href={`data:application/json;charset=utf-8,${encodeURIComponent(bundle.json)}`}
                download={`unoone-knowledge-export-${bundle.sha256.slice(0, 12)}.json`}
                data-kn="export-download"
              >
                Download export JSON
              </a>
            </div>
          )}
        </div>
      </section>
    </div>
  );

  return (
    <div>
      {header}
      <div className="main-body">
        <div className="knowledge-view settings-view">
          {error && <div className="ct-alert" role="alert">{error}</div>}
          {notice && <div className="ct-notice" role="status">{notice}</div>}
          {statusSection}
          <div className="kn-tabs" role="tablist" aria-label="Knowledge sections">
            {TABS.map(t => (
              <button
                key={t.id}
                id={`kn-tab-${t.id}`}
                role="tab"
                aria-selected={tab === t.id}
                aria-controls={`kn-panel-${t.id}`}
                className={`btn btn-secondary kn-tab ${tab === t.id ? 'ct-selected' : ''}`}
                onClick={() => setTab(t.id)}
              >
                {t.label}
              </button>
            ))}
          </div>
          {tab === 'explorer' && explorer}
          {tab === 'detail' && detailPanel}
          {tab === 'distiller' && distiller}
          {tab === 'export' && exportPanel}
        </div>
      </div>

      {rejectDialog && (
        <div className="ct-dialog-backdrop">
          <div
            className="ct-dialog kn-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="kn-reject-title"
            data-kn="reject-dialog"
            onKeyDown={e => dialogKeys(e, () => setRejectDialog(null))}
          >
            <h3 id="kn-reject-title">Reject this record</h3>
            <p className="ct-muted">
              An Invalidation record is appended; the rejected record stays readable for audit and is no longer
              returned as current knowledge.
            </p>
            <p className="ct-muted ct-hashes">
              Record <code data-kn="reject-ref">{refLabel(rejectDialog.target)}</code><br />
              Digest <code data-kn="reject-digest">{rejectDialog.target.content_digest}</code>
            </p>
            <label className="kn-stack">
              <span className="ct-muted">Reason (required, at most {MAX_REASON_SCALARS} characters)</span>
              <textarea
                className="kn-textarea"
                rows={3}
                value={rejectDialog.reason}
                onChange={e => { const reason = e.target.value; setRejectDialog(d => (d ? { ...d, reason } : d)); }}
                aria-label="Reject reason"
              />
            </label>
            <div className="ct-row">
              <button
                className="btn btn-danger"
                onClick={confirmReject}
                disabled={isBusy || !rejectDialog.reason.trim() || scalarLength(rejectDialog.reason.trim()) > MAX_REASON_SCALARS}
              >
                Confirm reject
              </button>
              <button className="btn btn-ghost" onClick={() => setRejectDialog(null)} disabled={busy === 'Reject'}>Keep the record</button>
            </div>
          </div>
        </div>
      )}

      {revokeDialog && (
        <div className="ct-dialog-backdrop">
          <div
            className="ct-dialog kn-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="kn-revoke-title"
            data-kn="revoke-dialog"
            onKeyDown={e => dialogKeys(e, () => setRevokeDialog(null))}
          >
            <h3 id="kn-revoke-title">Revoke this approval</h3>
            <p className="ct-muted">The approved procedure is invalidated by an appended record; it stays readable for audit.</p>
            <p className="ct-muted ct-hashes">
              Approval <code data-kn="revoke-ref">{refLabel(revokeDialog.approved)}</code><br />
              Digest <code data-kn="revoke-digest">{revokeDialog.approved.content_digest}</code>
            </p>
            <div className="ct-row">
              <button className="btn btn-danger" onClick={confirmRevoke} disabled={isBusy}>Confirm revoke</button>
              <button className="btn btn-ghost" onClick={() => setRevokeDialog(null)} disabled={busy === 'Revoke approval'}>Keep the approval</button>
            </div>
          </div>
        </div>
      )}

      {distillConfirm && distillPreview && (
        <div className="ct-dialog-backdrop">
          <div
            className="ct-dialog kn-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="kn-distill-title"
            data-kn="distill-dialog"
            onKeyDown={e => dialogKeys(e, () => setDistillConfirm(false))}
          >
            <h3 id="kn-distill-title">Run the distiller</h3>
            <p className="ct-muted">
              {distillPreview.request.sources.length} source{distillPreview.request.sources.length === 1 ? '' : 's'} are read within the budget and
              written to the encrypted vault as Evidence and Candidate records. Nothing is verified or approved. The
              search index becomes stale until you rebuild it.
            </p>
            <p className="kn-method">Method: <code>{status?.method ?? 'not reported yet'}</code></p>
            <p className="ct-muted ct-hashes">Request hash <code data-kn="distill-confirm-hash">{distillPreview.hash}</code></p>
            <div className="ct-row">
              <button className="btn btn-primary" onClick={confirmDistill} disabled={isBusy || !distillPreviewCurrent}>
                {busy === 'Run distillation' ? 'Running…' : 'Confirm run'}
              </button>
              <button className="btn btn-ghost" onClick={() => setDistillConfirm(false)} disabled={busy === 'Run distillation'}>Not now</button>
            </div>
          </div>
        </div>
      )}

      {exportConfirm && exportPreview && (
        <div className="ct-dialog-backdrop">
          <div
            className="ct-dialog kn-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="kn-export-confirm-title"
            data-kn="export-dialog"
            onKeyDown={e => dialogKeys(e, () => setExportConfirm(false))}
          >
            <h3 id="kn-export-confirm-title">Export {exportPreview.preview.items.length} record{exportPreview.preview.items.length === 1 ? '' : 's'}</h3>
            <p className="ct-muted">
              The export bundle is returned to this window and offered as a download. It is a knowledge export, not a
              training export (training export is disabled).
              {exportNeedsAck ? ' It contains private content you acknowledged.' : ''}
            </p>
            <p className="ct-muted ct-hashes">Request hash <code data-kn="export-confirm-hash">{exportPreview.preview.request_sha256}</code></p>
            <div className="ct-row">
              <button className="btn btn-primary" onClick={confirmExport} disabled={isBusy || !exportPreviewCurrent || (exportNeedsAck && !ackPrivate)}>
                {busy === 'Export' ? 'Exporting…' : 'Confirm export'}
              </button>
              <button className="btn btn-ghost" onClick={() => setExportConfirm(false)} disabled={busy === 'Export'}>Not now</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
