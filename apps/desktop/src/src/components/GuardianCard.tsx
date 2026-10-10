import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

/** Shape of `unoone_privacy_guardian::Decision` / `Receipt` as serialized by the native side. */
export type GuardianDecision = {
  guardian_version: number;
  severity: 'ALLOW' | 'WARN' | 'BLOCK';
  intent_kind: string;
  fingerprint: string;
  signals: string[];
  explanation: string;
  verification_route: string | null;
  model_note: string | null;
  decided_by?: 'POLICY' | 'HUMAN';
  proceeded?: boolean;
  at_ms?: number;
};

export const GUARDIAN_RECEIPT_PREFIX = 'GUARDIAN RECEIPT v1';
export const GUARDIAN_CORRECTION_PREFIX = 'GUARDIAN CORRECTION v1';

/** Parses a task draft note written by the runtime's guardian API. Returns null for ordinary drafts. */
export function parseGuardianNote(draft: string | undefined | null): { kind: 'RECEIPT' | 'CORRECTION'; head: string; body: GuardianDecision | Record<string, unknown> | null } | null {
  if (!draft) return null;
  const kind = draft.startsWith(GUARDIAN_RECEIPT_PREFIX) ? 'RECEIPT' : draft.startsWith(GUARDIAN_CORRECTION_PREFIX) ? 'CORRECTION' : null;
  if (!kind) return null;
  const newline = draft.indexOf('\n');
  const head = newline === -1 ? draft : draft.slice(0, newline);
  let body: GuardianDecision | Record<string, unknown> | null = null;
  if (newline !== -1) { try { body = JSON.parse(draft.slice(newline + 1)); } catch { body = null; } }
  return { kind, head, body };
}

const SEVERITY_COPY: Record<GuardianDecision['severity'], string> = {
  ALLOW: 'No warning.',
  WARN: 'Check before continuing — your decision is needed.',
  BLOCK: 'Stopped. This action stays unavailable from the assistant.',
};

/** Warning / receipt card. Deterministic host text only; a model note is labelled as not authority.
 * `onAcknowledge` is only offered for WARN and carries the exact fingerprint shown here. */
export function GuardianCard({ decision, taskId, onAcknowledge, onCorrected }: {
  decision: GuardianDecision; taskId?: string; onAcknowledge?: (fingerprint: string) => void; onCorrected?: () => void;
}) {
  const [comment, setComment] = useState('');
  const [reporting, setReporting] = useState(false);
  const [status, setStatus] = useState('');
  async function correct(kind: 'FALSE_ALARM' | 'CONFIRMED_HARMFUL' | 'MISSED_WARNING') {
    if (!taskId) return;
    setStatus('');
    try {
      await invoke('provider_request', { request: { action: 'GUARDIAN_CORRECTION', task_id: taskId, fingerprint: decision.fingerprint, kind, comment } });
      setStatus('Recorded in your encrypted ledger. Corrections never change a grant or silence future checks.');
      setReporting(false); setComment(''); onCorrected?.();
    } catch (e) { setStatus(typeof e === 'string' ? e : 'Correction was not recorded; unlock the vault and retry.'); }
  }
  return <aside role={decision.severity === 'ALLOW' ? 'status' : 'alert'} aria-label="Privacy guardian" style={{ border: '1px solid var(--border-color)', borderLeft: `6px solid ${decision.severity === 'BLOCK' ? '#b00020' : decision.severity === 'WARN' ? '#c77700' : '#2e7d32'}`, padding: 12, marginTop: 12 }}>
    <strong>Privacy guardian · {decision.severity} · {decision.intent_kind.toLowerCase().replace(/_/g, ' ')}</strong>
    <p>{SEVERITY_COPY[decision.severity]}</p>
    <p>{decision.explanation}</p>
    {decision.verification_route && <p><b>Independent check:</b> {decision.verification_route}</p>}
    {decision.model_note && <p><small>{decision.model_note}</small></p>}
    {decision.decided_by && <p><small>Decided by {decision.decided_by === 'HUMAN' ? 'you (explicit review)' : 'local policy'} · {decision.proceeded ? 'action proceeded' : 'action not performed'}</small></p>}
    <details><summary>Signals ({decision.signals.length})</summary><ul>{decision.signals.map(s => <li key={s}>{s}</li>)}</ul><p><small>Fingerprint: {decision.fingerprint}</small></p></details>
    {decision.severity === 'WARN' && onAcknowledge && <button onClick={() => onAcknowledge(decision.fingerprint)}>I checked this independently — proceed once with exactly this destination</button>}
    {taskId && <>
      <button onClick={() => setReporting(r => !r)}>{reporting ? 'Close report' : 'Report / correct this warning'}</button>
      {reporting && <div>
        <label>What happened (optional, secrets are masked)<input aria-label="Guardian correction comment" value={comment} maxLength={500} onChange={e => setComment(e.target.value)} /></label>
        <button onClick={() => void correct('FALSE_ALARM')}>Needless warning</button>
        <button onClick={() => void correct('CONFIRMED_HARMFUL')}>It was harmful</button>
        <button onClick={() => void correct('MISSED_WARNING')}>Something harmful was missed</button>
      </div>}
    </>}
    {status && <p><small>{status}</small></p>}
  </aside>;
}
