import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { tauriApi, type PersonalAgentView } from '../lib/tauri';

type Offer = { version: number; replica_id: string; person_id: string; agent_id: string; fingerprint: string };
type PeerStatus = { offer: Offer; peer: Offer | null; revoked: boolean; received: number; sent_ack: number; pending: number; message: string; persona_review: string | null; tasks: { task_id: string; goal: string; draft: string; deleted: boolean; status: string; snooze_until_ms: number | null; execute_on_hydration: false; events: { event_id: string; transition: string; step: number; evidence_ref: string | null }[] }[] };
export function PeerSyncPanel({ onChanged }: { onChanged?: () => void } = {}) {
  const generation = useRef(0);
  const [status, setStatus] = useState<PeerStatus | null>(null);
  const [local, setLocal] = useState<PersonalAgentView | null>(null);
  const [offer, setOffer] = useState('');
  const [compared, setCompared] = useState(false);
  const [persona, setPersona] = useState(false);
  const [tasks, setTasks] = useState<string[]>([]);
  const [choice, setChoice] = useState('SAME_PERSON');
  const [archive, setArchive] = useState(false);
  const [sharedTasks, setSharedTasks] = useState(false);
  const [address, setAddress] = useState('');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  useEffect(() => {
    const invalidate = () => { generation.current++; };
    const cancel = () => { if (document.hidden) { generation.current++; void invoke('peer_sync_cancel').catch(() => {}); setBusy(false); setStatus(null); setLocal(null); setOffer(''); } };
    document.addEventListener('visibilitychange', cancel);
    return () => { document.removeEventListener('visibilitychange', cancel); invalidate(); void invoke('peer_sync_cancel').catch(() => {}); };
  }, []);
  async function run(action: string) {
    const ticket = generation.current;
    setBusy(true); setMessage(action === 'listen' ? 'Listening once for 30 seconds on the address you entered. Keep this screen open and unlocked; tap Sync one page on the other device.' : '');
    try {
      const result = await invoke<PeerStatus>('peer_sync_command', { command: { action, offer: action === 'approve' ? JSON.parse(offer) : null, selection: action === 'approve' ? { persona, task_ids: choice === 'UNIFY_ARCHIVE' ? (sharedTasks ? ['00000000-0000-0000-0000-000000000000'] : []) : tasks } : null, choice: action === 'approve' ? choice : null, compared_fingerprint: compared, address } });
      if (ticket === generation.current && !document.hidden) { const localView = await tauriApi.personalAgentView(); if (ticket !== generation.current || document.hidden) return; setStatus(result); setLocal(localView); setMessage(result.message); onChanged?.(); }
    } catch (e) { if (ticket === generation.current) setMessage(typeof e === 'string' ? e : 'Pairing or transfer failed; original data retained. Check unlock, full fingerprints, identity choice, LAN address and peer status.'); }
    finally { if (ticket === generation.current) setBusy(false); }
  }
  return <section aria-label="Local peer pairing" style={{ padding: 24, borderTop: '1px solid var(--border-color)' }}>
    <h2>Local peer sync · explicit shared identity v2</h2>
    <p>Manual LAN/IP only, TLS 1.3 mutual authentication. No cloud, master-key sharing, background sending, grants or execution. Wi-Fi Direct and automatic discovery are not verified.</p>
    <p>Independent installations have different people/agents. <strong>Same person</strong> refuses a mismatch. <strong>Keep separate for review</strong> preserves both identities and histories; it does not merge your task board. Both screens must choose the same option. Unify and archive adopts the person/agent from the offer with the lexically smaller replica ID only after an authenticated exchange confirms both approvals. Replica IDs, vaults and keys remain distinct.</p>
    <button disabled={busy} onClick={() => void run('status')}>Show my pairing identity</button>
    {message && <p role="status">{message}</p>}
    {status && <>
      <p>Compare this entire SHA-256 fingerprint directly on both devices:</p><code style={{ overflowWrap: 'anywhere' }}>{status.offer.fingerprint}</code>
      <label>My manual pairing offer (public identifiers only)<textarea readOnly value={JSON.stringify(status.offer)} rows={5} style={{ width: '100%' }} /></label>
      {!status.peer && <fieldset disabled={busy}>
        <legend>Approve one peer locally</legend>
        <label>Paste the other device’s public pairing offer<textarea aria-label="Peer offer" maxLength={2048} value={offer} onChange={e => { setOffer(e.target.value); setCompared(false); }} /></label>
        <label><input type="checkbox" checked={compared} onChange={e => setCompared(e.target.checked)} />I compared the complete fingerprint against the other screen, not a network message or short code.</label>
        <label>Identity choice<select aria-label="Identity choice" value={choice} onChange={e => setChoice(e.target.value)}><option value="UNIFY_ARCHIVE">Unify identity and archive old local board (v2)</option><option value="SAME_PERSON">Same person (refuse mismatched IDs)</option><option value="KEEP_SEPARATE_REVIEW">Keep separate for review (no adoption/overwrite)</option></select></label>
        {choice === 'UNIFY_ARCHIVE' && <div role="alert"><p>This starts a new shared board. ALL current local persona/tasks remain encrypted in an immutable local archive; they are NOT rebound or automatically sent. No archive import, reset or rollback UI is available. Only new shared edits in selected categories are sent. No side effects execute.</p><label><input type="checkbox" checked={archive} onChange={e => setArchive(e.target.checked)} />I explicitly approve archiving my current board and adopting the deterministic shared person/agent after both screens approve.</label><label><input type="checkbox" checked={sharedTasks} onChange={e => setSharedTasks(e.target.checked)} />Share all new shared tasks, drafts/notes, events, reminders and tombstones</label></div>}
        <p>Select exactly what this device may send. Selection is fixed for this v1 pairing; new tasks are not automatically shared. Check selected prose for secrets/raw mail before approval.</p>
        <label><input type="checkbox" checked={persona} onChange={e => setPersona(e.target.checked)} />Share persona/name and retained preference history</label>
        {choice !== 'UNIFY_ARCHIVE' && local?.tasks.map(t => <label key={t.spec.task_id} style={{ display: 'block' }}><input type="checkbox" checked={tasks.includes(t.spec.task_id)} onChange={e => setTasks(prev => e.target.checked ? [...prev, t.spec.task_id] : prev.filter(i => i !== t.spec.task_id))} />{t.spec.goal} (draft/events/reminder/deletion history)</label>)}
        <button disabled={!compared || !offer || (choice === 'UNIFY_ARCHIVE' && !archive)} onClick={() => void run('approve')}>Approve fingerprint and selected records</button>
      </fieldset>}
      {status.peer && <>
        <p>Peer fingerprint: <code style={{ overflowWrap: 'anywhere' }}>{status.peer.fingerprint}</code></p>
        <p>{status.revoked ? 'REVOKED — no future connections. Previously received copies cannot be remotely erased.' : 'Approved locally; the other screen must also approve you.'}</p>
        <p>Received operations: {status.received} · Acknowledged outbound: {status.sent_ack} · Pending local sequence positions: {status.pending}</p>
        <label>Numeric local IP:port (listen uses this computer’s LAN IP; sync uses peer IP)<input aria-label="LAN address" value={address} maxLength={80} placeholder="192.168.1.12:43123" onChange={e => setAddress(e.target.value)} /></label>
        <button disabled={busy || status.revoked || !address} onClick={() => void run('listen')}>Listen for one page (30 seconds)</button>
        <button disabled={busy || status.revoked || !address} onClick={() => void run('sync')}>Sync one page now</button>
        <button onClick={() => { generation.current++; setBusy(false); void invoke('peer_sync_cancel').catch(() => {}); setMessage('Session cancellation requested. Reload to reconcile persisted pages.'); }}>Stop session</button>
        <button disabled={busy || status.revoked} onClick={() => { if (window.confirm('Revoke this peer? Future sync stops. Existing copies remain on the peer. V1 has no reset/re-pair flow.')) void run('revoke'); }}>Revoke peer</button>
        <p>{status.message}</p>
        {status.persona_review && <p>Persona conflict review: {status.persona_review}</p>}
        <h3>Retained peer tasks · inert conflict review</h3>
        {status.tasks.map(t => <article key={t.task_id}><strong>{t.deleted ? 'Deleted task (tombstone retained)' : t.goal}</strong><p>{t.status} · no execution authority</p>{!t.deleted && <><p>{t.draft}</p>{t.snooze_until_ms && <p>Manual reminder: {new Date(t.snooze_until_ms).toLocaleString()}</p>}<ol>{t.events.map(e => <li key={e.event_id}>{e.transition} · step {e.step} · evidence {e.evidence_ref || "none"}</li>)}</ol></>}</article>)}
      </>}
      <p>At most 8 changes/page, 256 KiB HTTP body, 2048 retained operations and 4 MiB encrypted review state. No eviction. Logical tombstones, not secure erasure. No full conversation, provider account, model or skill sync.</p>
    </>}
  </section>;
}
