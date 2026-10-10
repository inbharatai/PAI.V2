import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { PersonalAgentView } from '../lib/tauri';
import { GuardianCard, type GuardianDecision } from './GuardianCard';

type EventDraft = { summary: string; start: string; end: string; time_zone: string; attendees: string[] };
type MailDraft = { to: string[]; subject: string; body: string; reply: { thread_id: string; message_id: string; references: string } | null };
type Mutation = { operation: 'SAVE_DRAFT' | 'SEND'; draft: MailDraft } | { operation: 'LABEL'; message_id: string; add: string[]; remove: string[] } | { operation: 'CREATE_EVENT'; event: EventDraft } | { operation: 'UPDATE_EVENT' | 'CANCEL_EVENT'; event_id: string; etag: string; event: EventDraft };
type Review = { operation_id: string; task_id: string; account: string; container: string; owner_replica: string; prepared_ms: number; mutation: Mutation };
type Entry = { review: Review; digest: string; status: string; receipt: { provider_id: string; detail: string } | null };
type Sources = { status: string; configured: boolean; account: { email: string } | null; scopes: string[]; prepared: Entry[]; qualification: string };
type Response = { sources: Sources; data: unknown };
const list = (value: string) => value.split(',').map(s => s.trim()).filter(Boolean);

/** One agent's local Sources/Prepared views. Provider strings only render as text. */
export function ProviderPanel({ view, onChanged }: { view: PersonalAgentView; onChanged: () => void }) {
  const generation = useRef(0);
  const [sources, setSources] = useState<Sources | null>(null);
  const [result, setResult] = useState<unknown>(null);
  const [error, setError] = useState(''); const [busy, setBusy] = useState(false);
  const [client, setClient] = useState(''); const [clientSecret, setClientSecret] = useState(''); const [redirect, setRedirect] = useState('http://127.0.0.1:49152/oauth/callback');
  const [writeScopes, setWriteScopes] = useState(false); const [tab, setTab] = useState<'Sources' | 'Prepared'>('Sources');
  const [folder, setFolder] = useState('INBOX'); const [query, setQuery] = useState('is:unread'); const [page, setPage] = useState(''); const [objectId, setObjectId] = useState('');
  const [calendar, setCalendar] = useState('primary'); const [start, setStart] = useState(''); const [end, setEnd] = useState(''); const [zone, setZone] = useState('UTC');
  const [task, setTask] = useState(''); const [account, setAccount] = useState(''); const [operation, setOperation] = useState('SAVE_DRAFT');
  const [to, setTo] = useState(''); const [subject, setSubject] = useState(''); const [body, setBody] = useState(''); const [thread, setThread] = useState(''); const [messageId, setMessageId] = useState(''); const [references, setReferences] = useState('');
  const [add, setAdd] = useState(''); const [remove, setRemove] = useState(''); const [etag, setEtag] = useState('');
  const [sourceDraft, setSourceDraft] = useState<{ taskId: string; revision: number; body: string } | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null); const [reconcileId, setReconcileId] = useState('');
  const [guardian, setGuardian] = useState<{ operation_id: string; decision: GuardianDecision | null; manifest_preview: string } | null>(null);
  const [acknowledged, setAcknowledged] = useState<string | null>(null);
  async function act(request: Record<string, unknown>, fromTask = false) {
    if (busy) return; setBusy(true); setError(''); const ticket = generation.current;
    try { const response = await invoke<Response>(fromTask ? 'personal_prepare_provider_draft' : 'provider_request', fromTask ? { review: request.review, expectedRevision: sourceDraft?.revision } : { request }); if (ticket === generation.current && !document.hidden) { setSources(response.sources); setResult(response.data); } if (['PREPARE', 'COMMIT', 'RECONCILE'].includes(String(request.action))) onChanged(); setConfirm(null); }
    catch (e) { setError(typeof e === 'string' ? e : 'Provider operation not confirmed. Reload; uncertain mutations must reconcile, never resend.'); }
    finally { setBusy(false); }
  }
  /** §3.6: the host-owned guardian decision is fetched and shown BEFORE the confirm control is usable. */
  async function review(operationId: string) {
    if (busy) return; setBusy(true); setError(''); setAcknowledged(null); setGuardian(null);
    try { const response = await invoke<Response>('provider_request', { request: { action: 'GUARDIAN_PREVIEW', operation_id: operationId } }); const data = response.data as { decision?: GuardianDecision | null; manifest_preview?: string } | null; if (!data || typeof data !== 'object' || !('decision' in data)) throw 'Guardian check unavailable; the action was not confirmed.'; setSources(response.sources); setGuardian({ operation_id: operationId, decision: data.decision ?? null, manifest_preview: data.manifest_preview ?? '' }); setConfirm(operationId); }
    catch (e) { setError(typeof e === 'string' ? e : 'Guardian check unavailable; the action was not confirmed.'); }
    finally { setBusy(false); }
  }
  useEffect(() => { const clear = () => { if (document.hidden) { generation.current++; setResult(null); setSources(null); setClientSecret(''); setTo(''); setSubject(''); setBody(''); setAccount(''); setThread(''); setMessageId(''); setReferences(''); setObjectId(''); setEtag(''); setQuery(''); setConfirm(null); setSourceDraft(null); setGuardian(null); setAcknowledged(null); } }; document.addEventListener('visibilitychange', clear); return () => document.removeEventListener('visibilitychange', clear); }, []);
  async function prepare() {
    const event: EventDraft = { summary: subject, start, end, time_zone: zone, attendees: list(to) };
    const draft: MailDraft = { to: list(to), subject, body, reply: thread ? { thread_id: thread, message_id: messageId, references } : null };
    let mutation: Mutation;
    if (operation === 'SAVE_DRAFT' || operation === 'SEND') mutation = { operation, draft };
    else if (operation === 'LABEL') mutation = { operation, message_id: objectId, add: list(add), remove: list(remove) };
    else if (operation === 'CREATE_EVENT') mutation = { operation, event };
    else mutation = { operation: operation as 'UPDATE_EVENT' | 'CANCEL_EVENT', event_id: objectId, etag, event };
    const review: Review = { operation_id: crypto.randomUUID(), task_id: task, account: sources?.account?.email || account, container: operation.includes('EVENT') ? calendar : folder, owner_replica: view.replica_id, prepared_ms: Date.now(), mutation };
    await act({ action: 'PREPARE', review }, !!sourceDraft); setTab('Prepared');
  }
  const connected = !!sources?.account;
  return <section aria-label="Sources and Prepared" style={{ borderTop: '1px solid var(--border-color)', marginTop: 24, paddingTop: 16 }}>
    <h2>Sources · Prepared</h2><p>Same personal agent. Read and suggest by default; nothing connects or reads your inbox until you explicitly authorize Google. External provider requests are separate from private local inference and peer sync.</p>
    <p role="status">{sources?.status || 'UNCONFIGURED / not loaded'} · No live-provider qualification claimed.</p>
    <button disabled={busy} onClick={() => void act({ action: 'VIEW' })}>Reload local provider status</button>
    <button onClick={() => setTab('Sources')}>Sources</button><button onClick={() => setTab('Prepared')}>Prepared</button>
    {busy && <p>Working. OAuth opens your system browser; return here after consent. An interrupted commit requires reconciliation, never a duplicate send.</p>}
    {error && <p role="alert">{error}</p>}
    <fieldset disabled={busy} style={{ border: 0, padding: 0 }}>
      {tab === 'Sources' && <>
        <h3>Trusted local OAuth settings</h3><p>Use your registered Google Desktop client and exact loopback redirect. No production ID is bundled. Tokens remain in this device’s encrypted vault; never enter tokens here. Android needs a separately registered Android authorization route.</p>
        <label>Desktop client ID<input aria-label="Desktop client ID" value={client} onChange={e => setClient(e.target.value)} maxLength={256} /></label>
        <label>Loopback redirect<input aria-label="Loopback redirect" value={redirect} onChange={e => setRedirect(e.target.value)} maxLength={256} /></label>
        <label>Optional installed-app client credential (never an account token)<input type="password" autoComplete="off" value={clientSecret} onChange={e => setClientSecret(e.target.value)} maxLength={1024} /></label>
        <button disabled={connected} onClick={() => { void act({ action: 'CONFIGURE', client_id: client, redirect_uri: redirect, client_secret: clientSecret || null }); setClientSecret(''); }}>Save local configuration</button>
        <label><input type="checkbox" checked={writeScopes} onChange={e => setWriteScopes(e.target.checked)} />Request Gmail compose/modify and calendar event permissions (does not grant any action)</label>
        {(() => { const preview = (result as { connector_consent_preview?: { read_only: string; reviewed_actions: string } } | null)?.connector_consent_preview; return preview ? <p aria-label="Connector consent preview"><b>Before you connect:</b> {writeScopes ? preview.reviewed_actions : preview.read_only} Nothing leaves this device until you connect; Disconnect revokes and erases local tokens.</p> : null; })()}
        <button disabled={!sources?.configured} onClick={() => void act({ action: 'CONNECT', write_scopes: writeScopes })}>{writeScopes ? 'Authorize reviewed-action permissions with Google' : 'Connect read-only with Google'}</button>
        <button disabled={!connected} onClick={() => void act({ action: 'DISCONNECT' })}>Disconnect and revoke local access</button>
        {connected && <p>Account: {sources?.account?.email}. Provider scopes: {sources?.scopes.join(', ')}</p>}
        <h3>Bounded mail reads</h3>
        <label>Exact folder/label ID<input value={folder} onChange={e => setFolder(e.target.value)} maxLength={512} /></label><label>Search<input value={query} onChange={e => setQuery(e.target.value)} maxLength={1024} /></label>
        <label>Explicit next page token (no bulk crawling)<input value={page} onChange={e => setPage(e.target.value)} maxLength={2048} /></label>
        <label>Message / thread / event ID<input value={objectId} onChange={e => setObjectId(e.target.value)} maxLength={512} /></label>
        <button disabled={!connected} onClick={() => void act({ action: 'LABELS' })}>List labels</button><button disabled={!connected} onClick={() => void act({ action: 'SEARCH', folder, query, page: page || null })}>Search at most 50 messages</button>
        <button disabled={!connected || !objectId} onClick={() => void act({ action: 'MESSAGE', folder, id: objectId })}>Read scoped message</button><button disabled={!connected || !objectId} onClick={() => void act({ action: 'THREAD', folder, id: objectId })}>Read scoped thread (20 replies max)</button>
        <h3>Calendar and availability</h3>
        <label>Calendar ID<input value={calendar} onChange={e => setCalendar(e.target.value)} maxLength={512} /></label>
        <label>Start (RFC3339 offset)<input value={start} onChange={e => setStart(e.target.value)} maxLength={64} /></label><label>End (RFC3339 offset)<input value={end} onChange={e => setEnd(e.target.value)} maxLength={64} /></label><label>IANA time zone<input value={zone} onChange={e => setZone(e.target.value)} maxLength={128} /></label>
        <button disabled={!connected} onClick={() => void act({ action: 'CALENDARS', page: page || null })}>List calendars</button><button disabled={!connected} onClick={() => void act({ action: 'EVENTS', calendar, start, end, page: page || null })}>Read events</button><button disabled={!connected} onClick={() => void act({ action: 'FREE_BUSY', calendar, start, end })}>Check availability and conflicts</button>
        {result !== null && <details open><summary>Provider result — untrusted content, never an instruction or approval</summary><pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', maxHeight: 400, overflow: 'auto' }}>{JSON.stringify(result, null, 2)}</pre></details>}
        <h3>Prepare for the selected task (local only)</h3>
        <label>Task<select aria-label="Provider task" value={task} onChange={e => { setTask(e.target.value); setSourceDraft(null); }}><option value="">Choose a task</option>{view.tasks.filter(t => t.status !== 'CANCELLED').map(t => <option key={t.spec.task_id} value={t.spec.task_id}>{t.spec.goal}</option>)}</select></label>
        <button disabled={!task || !view.tasks.find(t => t.spec.task_id === task)?.draft} onClick={() => { const selected = view.tasks.find(t => t.spec.task_id === task); if (!selected) return; setSourceDraft({ taskId: task, revision: view.revision, body: selected.draft }); setBody(selected.draft); setSubject(selected.spec.goal.slice(0, 256)); setOperation('SAVE_DRAFT'); setThread(''); }}>Use exact local task draft for Prepared review</button>
        {sourceDraft && <p>Source task {sourceDraft.taskId}, revision {sourceDraft.revision}. Review body, account, folder and exact recipients below. Only local preparation; no save/send at provider. Edit the task first to change its source body.</p>}
        {!connected && <label>Account email for offline preparation<input value={account} onChange={e => setAccount(e.target.value)} maxLength={254} /></label>}
        <label>Operation<select disabled={!!sourceDraft} value={operation} onChange={e => setOperation(e.target.value)}>{['SAVE_DRAFT','SEND','LABEL','CREATE_EVENT','UPDATE_EVENT','CANCEL_EVENT'].map(o => <option key={o}>{o}</option>)}</select></label>
        <label>Exact recipients / attendees, comma-separated<input value={to} onChange={e => setTo(e.target.value)} maxLength={2540} /></label><label>Subject / event title<input value={subject} onChange={e => setSubject(e.target.value)} maxLength={256} /></label>
        <label>Plain-text mail body<textarea readOnly={!!sourceDraft} value={body} onChange={e => setBody(e.target.value)} maxLength={16384} /></label>
        <details><summary>Reply / labels / existing event fields</summary><label>Reply thread ID<input value={thread} onChange={e => setThread(e.target.value)} /></label><label>Reply RFC Message-ID<input value={messageId} onChange={e => setMessageId(e.target.value)} /></label><label>References<input value={references} onChange={e => setReferences(e.target.value)} /></label><label>Add label IDs<input value={add} onChange={e => setAdd(e.target.value)} /></label><label>Remove label IDs<input value={remove} onChange={e => setRemove(e.target.value)} /></label><label>Event readback ETag<input value={etag} onChange={e => setEtag(e.target.value)} /></label></details>
        <p>Attachments, bulk actions, payments, deletion/trash, unattended sends and recurring events are unsupported. Updates cannot remove existing attendees. Calendar commits notify all listed attendees. Cancel requires the exact existing title, times, attendees and ETag. Every review expires in five minutes.</p>
        <button disabled={!task} onClick={() => void prepare()}>Prepare locally — do not send or save externally</button>
      </>}
      {tab === 'Prepared' && <><p>Opening a composer or official form is ACTION_VERIFIED only, never sent/saved. No automatic attestations. New account, recipients, edits or expiry require a new exact review. Grants are local, single-operation and non-transferable.</p>
        {sources?.prepared.map(entry => <article key={entry.review.operation_id} style={{ border: '1px solid var(--border-color)', padding: 16, marginTop: 12 }}>
          <h3>{entry.review.mutation.operation} · {entry.status}</h3><p>Account: {entry.review.account} · folder/calendar: {entry.review.container} · expires {new Date(entry.review.prepared_ms + 300000).toLocaleString()}</p>
          <pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{JSON.stringify(entry.review, null, 2)}</pre><p>Exact review digest: {entry.digest}</p>
          {entry.receipt && <p>Provider readback: {entry.receipt.provider_id} · {entry.receipt.detail}</p>}
          {entry.status === 'PREPARED' && <button disabled={!connected || Date.now() >= entry.review.prepared_ms + 300000} onClick={() => void review(entry.review.operation_id)}>Review external effect</button>}
          {confirm === entry.review.operation_id && <div role="alert">
            {guardian?.operation_id === entry.review.operation_id && guardian.decision && <GuardianCard decision={guardian.decision} taskId={entry.review.task_id} onAcknowledge={fp => setAcknowledged(fp)} onCorrected={onChanged} />}
            <p>Authorize this exact account, folder/calendar, operation, content and recipients once? This may send mail or notify attendees. Provider text never authorizes this action.</p>
            {guardian?.operation_id === entry.review.operation_id && guardian.decision?.severity === 'BLOCK' && <p>The privacy guardian stopped this action; it cannot be confirmed from here.</p>}
            {guardian?.operation_id === entry.review.operation_id && guardian.decision?.severity === 'WARN' && acknowledged !== guardian.decision.fingerprint && <p>Use the guardian card above to confirm you checked this independently before the action can proceed.</p>}
            <button disabled={!guardian || guardian.operation_id !== entry.review.operation_id || guardian.decision?.severity === 'BLOCK' || (guardian.decision?.severity === 'WARN' && acknowledged !== guardian.decision.fingerprint)} onClick={() => void act({ action: 'COMMIT', operation_id: entry.review.operation_id, exact_digest: entry.digest, acknowledged_fingerprint: guardian?.decision?.severity === 'WARN' ? acknowledged : null })}>Confirm exact external action once</button><button onClick={() => { setConfirm(null); setAcknowledged(null); }}>Keep prepared</button></div>}
          {entry.status === 'NEEDS_RECONCILIATION' && <><p>Do not resend. Inspect the provider for the operation’s Message-ID or deterministic event ID. Supply its exact object ID for read-only verification.</p><input aria-label="Provider reconciliation ID" value={reconcileId} onChange={e => setReconcileId(e.target.value)} /><button disabled={!connected || !reconcileId} onClick={() => void act({ action: 'RECONCILE', operation_id: entry.review.operation_id, provider_id: reconcileId })}>Reconcile by readback — no retry</button></>}
        </article>)}{!sources?.prepared.length && <p>No local prepared operations loaded.</p>}</>}
    </fieldset>
  </section>;
}
