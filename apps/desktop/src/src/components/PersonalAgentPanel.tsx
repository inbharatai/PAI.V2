import { useEffect, useState } from 'react';
import { PeerSyncPanel } from './PeerSyncPanel';
import { ProviderPanel } from './ProviderPanel';
import { GuardianCard, parseGuardianNote, type GuardianDecision } from './GuardianCard';
import { tauriApi, type PersonalAgentView, type PersonalRequest, type PersonalTask } from '../lib/tauri';

export function PersonalAgentPanel() {
  const [view, setView] = useState<PersonalAgentView | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [name, setName] = useState('');
  const [preferences, setPreferences] = useState('');
  const [goal, setGoal] = useState('');
  const [draft, setDraft] = useState('');
  const [editing, setEditing] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [clearPersona, setClearPersona] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  function loaded(v: PersonalAgentView) { setView(v); setName(v.agent.display_name); setPreferences(v.persona.preferences.map(p => p.value).join('\n')); }
  useEffect(() => { let active = true; tauriApi.personalAgentView().then(v => { if (active) loaded(v); }).catch(() => { if (active) setError('Unlock the local vault, then reload. A corrupt or incompatible ledger is retained, never reset.'); }); return () => { active = false; }; }, []);
  async function reload() { setError(''); setBusy(true); try { loaded(await tauriApi.personalAgentView()); } catch { setView(null); setError('Cannot open the personal ledger. Unlock the local vault or resolve its retained data error.'); } finally { setBusy(false); } }
  async function mutate(action: PersonalRequest['action'], taskId: string | null = null, text = '', body = '', until: number | null = null) {
    if (!view || busy) return;
    setBusy(true); setError('');
    try {
      loaded(await tauriApi.personalAgentMutate({ operation_id: crypto.randomUUID(), expected_revision: view.revision, expected_replica_id: view.replica_id, action, task_id: taskId, text, draft: body, snooze_until_ms: until }));
      if (action === 'CREATE' || action === 'EDIT') { setGoal(''); setDraft(''); setEditing(null); }
      setDeleting(null); setClearPersona(false);
    } catch { setError('Change was not confirmed. Reload to reconcile a locked vault, concurrent edit or write failure before retrying. Nothing was executed.'); }
    finally { setBusy(false); }
  }
  const task: PersonalTask | undefined = view?.tasks.find(t => t.spec.task_id === selected);
  return <section style={{ padding: 24, overflow: 'auto', height: '100%' }} aria-label="Personal agent">
    <h1>{view?.agent.display_name || 'Your personal agent'}</h1>
    <p>One identity, with internal specialists only. This local task ledger is manually reviewed; accepting does not execute a task.</p>
    <p>Before explicit adoption each installation keeps its own person and replica. After both screens approve v2 adoption and exchange, this active board reads the merged causal store; replicas and keys stay distinct. Conversation sync and automatic reminders are not connected.</p>
    {error && <p role="alert">{error}</p>}
    <button disabled={busy} onClick={() => void reload()}>Reload ledger</button>
    {view && <fieldset disabled={busy} style={{ border: 0, padding: 0 }}>
      {!!view.archived_mutations && <p>Archived local mutations: {view.archived_mutations}. Original identity/task records remain encrypted and unchanged, not automatically rebound/imported.</p>}
      {view.conflicts?.map((conflict, n) => <details key={n}><summary>Conflict review — retained alternatives (no last-writer-wins)</summary><pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{conflict}</pre></details>)}
      <details><summary>Identity and local persistence</summary><p>Agent: {view.agent.agent_id}<br />Person: {view.agent.person_id}<br />Replica: {view.replica_id}<br />Encrypted pending mutations: {view.pending_mutations}</p></details>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(280px, 1fr))', gap: 24, marginTop: 16 }}>
        <section aria-label="Persona preferences"><h2>Persona</h2>
          <label>Assistant name<input aria-label="Assistant name" value={name} maxLength={4096} onChange={e => setName(e.target.value)} /></label>
          <label>Your response preferences<textarea aria-label="Response preferences" value={preferences} maxLength={4096} onChange={e => setPreferences(e.target.value)} /></label>
          <p>User-authored, private · revision {view.persona.revision} · source {view.persona.provenance.source}. Approved, resolved preferences apply to Personal mode in the existing Chat conversation. Revoked, stale or conflicted preferences are skipped; they never authorize tools.</p>
          <button disabled={!name.trim()} onClick={() => void mutate('PERSONA', null, name, preferences)}>Save persona</button>
          <button onClick={() => setClearPersona(true)}>Clear preferences</button>
          {clearPersona && <div role="alert"><p>Clear current preferences with a tombstone? Encrypted history is retained for future conflict review; this is not secure erasure.</p><button onClick={() => void mutate('CLEAR_PERSONA')}>Confirm clear preferences</button><button onClick={() => setClearPersona(false)}>Keep preferences</button></div>}
          {view.persona.deleted && <p>Preferences cleared (tombstoned).</p>}
        </section>
        <section aria-label="Task editor"><h2>{editing ? 'Edit task' : 'New personal task'}</h2>
          <form onSubmit={e => { e.preventDefault(); void mutate(editing ? 'EDIT' : 'CREATE', editing || crypto.randomUUID(), goal, draft); }}>
            <label>Goal<textarea aria-label="Task goal" value={goal} maxLength={4096} onChange={e => setGoal(e.target.value)} /></label>
            <label>Private draft<textarea aria-label="Private draft" value={draft} maxLength={4096} onChange={e => setDraft(e.target.value)} /></label>
            <button type="submit" disabled={!goal.trim()}>{editing ? 'Save task edit' : 'Create task'}</button>
            {editing && <button type="button" onClick={() => { setEditing(null); setGoal(''); setDraft(''); }}>Discard edit</button>}
          </form>
          <p>No AI responses are simulated. Drafts and task metadata use your encrypted vault, not browser storage or plaintext logs. This initial ledger holds up to 128 task IDs (including deleted), 2048 mutations or 4 MiB, whichever comes first. It never evicts pending data; reviewed compaction is not available yet.</p>
        </section>
      </div>
      <section aria-label="Personal tasks"><h2>Tasks</h2>{!view.tasks.length && <p>No personal tasks yet.</p>}
        {view.tasks.map(t => <article key={t.spec.task_id} style={{ borderBottom: '1px solid var(--border-color)', padding: '16px 0' }}>
          <h3>{t.spec.goal}</h3><p>Declarative owner: {t.owner_replica_id || "local"} · epoch {t.owner_epoch || 1}. No execution on hydration; handoff/receipt claims do not grant authority. Accepted tasks need a separate draft-template review in Personal Chat.</p><p>{t.status}{t.snooze_until_ms ? ` · Snoozed until ${new Date(t.snooze_until_ms).toLocaleString()} (manual reminder only)` : ''}</p>
          <button onClick={() => setSelected(t.spec.task_id)}>View timeline</button>
          {t.status !== 'CANCELLED' && <><button onClick={() => { setEditing(t.spec.task_id); setGoal(t.spec.goal); setDraft(t.draft); }}>Edit</button>
            <button disabled={t.status === 'READY_FOR_REVIEW'} onClick={() => void mutate('ACCEPT', t.spec.task_id)}>Accept for review</button>
            <button onClick={() => void mutate('SNOOZE', t.spec.task_id, '', '', Date.now() + 86400000)}>Snooze 1 day</button>
            <button onClick={() => void mutate('CANCEL', t.spec.task_id)}>Cancel task</button></>}
          <button onClick={() => setDeleting(t.spec.task_id)}>Delete task</button>
          {deleting === t.spec.task_id && <div role="alert"><p>Hide this task with a deletion tombstone? Encrypted history remains; deleted IDs cannot be recreated.</p><button onClick={() => void mutate('DELETE', t.spec.task_id)}>Confirm delete task</button><button onClick={() => setDeleting(null)}>Keep task</button></div>}
        </article>)}
      </section>
      {task && <section aria-label="Task timeline"><h2>Timeline</h2><p>{task.spec.user_visible_policy}</p><p>Remote observed claims (not native completion): {JSON.stringify(task.remote_claims || [])}</p>
        {(() => { const note = parseGuardianNote(task.draft); if (!note) return <p>Draft: {task.draft || '(empty)'}</p>; if (note.kind === 'RECEIPT' && note.body && 'severity' in note.body) return <GuardianCard decision={note.body as GuardianDecision} taskId={task.spec.task_id} onCorrected={() => void reload()} />; return <p>{note.head}</p>; })()}<p>Expected: {task.spec.expected_postcondition}</p><ol>{task.events.map(e => <li key={e.event_id}>{e.transition} · step {e.step} · evidence: {e.evidence_ref || 'none'}<small> · {e.event_id}</small></li>)}</ol><button onClick={() => setSelected(null)}>Close timeline</button></section>}
      <section aria-label="Unavailable domains"><h2>Inbox and calendar</h2><p>Unavailable: no qualified account/provider adapter is claimed. Use Sources/Prepared below for the separate, explicitly authorized Google adapter lane. No account is connected by creating a task. Opening another app is not proof of mail sent or an appointment saved.</p></section>
    </fieldset>}
    {view && <ProviderPanel view={view} onChanged={() => void reload()} />}
    <PeerSyncPanel onChanged={() => void reload()} />
  </section>;
}
