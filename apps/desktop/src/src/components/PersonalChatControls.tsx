import { invoke } from '@tauri-apps/api/core';
import { useEffect, useState } from 'react';
import { tauriApi, type PersonalAgentView } from '../lib/tauri';
import { PersonalAgentPanel } from './PersonalAgentPanel';
export type ReviewedSource = { kind: 'NONE' } | { kind: 'NOTES'; record_ids: string[]; query: string } | { kind: 'SELECTED_FILE'; path: string };
export interface ReviewedDraft { task_id: string; expected_revision: number; source?: ReviewedSource; children?: boolean }

/** Local interaction only: hydration never calls Harness or dispatches a task. */
export function PersonalChatControls({ busy, onDraft }: { busy: boolean; onDraft: (draft: ReviewedDraft | null) => void }) {
  const [view, setView] = useState<PersonalAgentView | null>(null);
  const [taskId, setTaskId] = useState('');
  const [reviewed, setReviewed] = useState(false);
  const [error, setError] = useState('');
  const [codingBusy, setCodingBusy] = useState(false);
  const [codingResult, setCodingResult] = useState('');
  const [children, setChildren] = useState(false);
  const [sourceKind, setSourceKind] = useState('NONE');
  const [selection, setSelection] = useState('');
  const [query, setQuery] = useState('');
  const invalidate = () => { setReviewed(false); onDraft(null); };
  const source: ReviewedSource = sourceKind === 'NOTES' ? { kind: 'NOTES', record_ids: selection.split(',').map(s => s.trim()).filter(Boolean), query } : sourceKind === 'SELECTED_FILE' ? { kind: 'SELECTED_FILE', path: selection } : { kind: 'NONE' };
  async function reload() {
    onDraft(null); setReviewed(false); setTaskId('');
    try { setView(await tauriApi.personalAgentView()); setError(''); }
    catch { setView(null); setError('Unlock your vault and reload. No personal request can run while locked.'); }
  }
  useEffect(() => { let current = true; tauriApi.personalAgentView().then(v => { if (current) setView(v); }).catch(() => { if (current) setError('Unlock your vault and reload.'); }); return () => { current = false; }; }, []);
  const task = view?.tasks.find(t => t.spec.task_id === taskId);
  return <section aria-label="Personal conversation controls" style={{ padding: 12 }}>
    <p>{view?.agent.display_name ?? 'Personal agent'} · approved style preferences apply to this conversation. Native policy takes priority. Conversation has no tool, dataset, network or child grant.</p>
    {error && <p role="alert">{error}</p>}
    <details><summary>Persona, tasks and connected sources</summary><PersonalAgentPanel /></details>
    <button disabled={busy || codingBusy} onClick={() => void reload()}>Reload reviewed tasks</button>
    <label>Draft template task<select aria-label="Draft template task" disabled={busy || codingBusy} value={taskId} onChange={e => { setTaskId(e.target.value); setReviewed(false); onDraft(null); }}>
      <option value="">Conversation only</option>
      {view?.tasks.filter(t => t.status === 'READY_FOR_REVIEW' && t.owner_replica_id === view.replica_id && !t.snooze_until_ms).map(t => <option key={t.spec.task_id} value={t.spec.task_id}>{t.spec.goal}</option>)}
    </select></label>
    {task && <div><p>Reviewed goal: {task.spec.goal}</p><p>One local-model draft, at most 60 seconds and 4096 output bytes. Read only the exact selected local source below; no changes, network or send. Draft facts are unverified. Sending this turn runs this exact goal, not additional chat instructions. A stopped/interrupted task never restarts automatically.</p>
      <label><input type="checkbox" checked={children} disabled={busy || codingBusy} onChange={e => { setChildren(e.target.checked); invalidate(); }} />Use two temporary specialists: selected-source summary and independent draft. Shared 60-second deadline, two child calls plus one final response, no tools/process/network. Older one-call tasks must be recreated to expand their budget.</label>
      <label>Local source template<select aria-label="Local source template" disabled={busy || codingBusy} value={sourceKind} onChange={e => { setSourceKind(e.target.value); setSelection(''); setQuery(''); invalidate(); }}><option value="NONE">Draft without sources</option><option value="NOTES">Search selected encrypted notes/documents</option><option value="SELECTED_FILE">Read one file in an existing folder grant</option></select></label>
      {sourceKind !== 'NONE' && <label>{sourceKind === 'NOTES' ? 'Exact encrypted record UUIDs (1–8, comma separated)' : 'Exact absolute file path'}<input aria-label="Exact source selection" value={selection} disabled={busy || codingBusy} onChange={e => { setSelection(e.target.value); invalidate(); }} /></label>}
      {sourceKind === 'NOTES' && <label>Search selected records<input aria-label="Selected notes query" value={query} maxLength={256} disabled={busy || codingBusy} onChange={e => { setQuery(e.target.value); invalidate(); }} /></label>}
      {sourceKind === 'SELECTED_FILE' && <div><p>Isolated coding check: one selected Python file, 20-second gate, no edits/apply/network. Linux requires a runtime-verified backend; Windows/macOS remain blocked until a real isolated backend exists.</p><button disabled={busy || codingBusy || !reviewed || !selection.endsWith('.py')} onClick={async () => { if (!view) return; setCodingBusy(true); onDraft(null); try { const result = await invoke('personal_coding_check', { taskId, expectedRevision: view.revision, path: selection }); setCodingResult(JSON.stringify(result)); } catch(e) { setError(String(e)); } finally { setCodingBusy(false); setReviewed(false); } }}>Run reviewed isolated Python check instead of model draft</button><button disabled={!codingBusy} onClick={() => void invoke('harness_stop_run', { conversationId: `personal-coding:${taskId}` })}>Stop this coding check</button><pre>{codingResult}</pre></div>}
      <p>Source input is limited to 8192 bytes. Unselected records are not decrypted. File reads recheck existing native folder grants; this review creates no new folder grant.</p>
      <label><input type="checkbox" disabled={busy || codingBusy} checked={reviewed} onChange={e => { setReviewed(e.target.checked); onDraft(e.target.checked && view ? { task_id: taskId, expected_revision: view.revision, source, children } : null); }} />I reviewed this draft-only template; run it on my next Send</label>
    </div>}
    <small>Selected local sources are read natively. Scoped child/coding dispatch has separate limits. Manual agent tools remain a separate explicit mode. Provider sends always use their separate exact prepared-review gate.</small>
  </section>;
}
