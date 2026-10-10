import { useState, useCallback, useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { tauriApi, type VaultInfo } from '../lib/tauri';

interface UnlockScreenProps {
  onUnlock: (vaultId: string, vaultRoot: string, storageKind?: VaultInfo['storage_kind']) => void;
}

/** Interactive production lifecycle. No dev bypass, media scan, download or migration. */
export function UnlockScreen({ onUnlock }: UnlockScreenProps) {
  const [info, setInfo] = useState<VaultInfo | null>(null);
  const [password, setPassword] = useState('');
  const [confirmation, setConfirmation] = useState('');
  const [phrase, setPhrase] = useState('');
  const [recoveryKey, setRecoveryKey] = useState('');
  const [recovering, setRecovering] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [loading, setLoading] = useState(false);
  const busy = useRef(false);
  const active = useRef(true);
  const infoRef = useRef(info);
  infoRef.current = info;

  const detect = useCallback(async () => {
    if (busy.current) return;
    busy.current = true; setLoading(true); setError('');
    try {
      const next = await tauriApi.detectVault();
      if (!active.current) return;
      setInfo(next);
      if (!next.detected) {
        setError(next.validation_failures.map(f => `${f.code}: ${f.message}`).join('\n') || 'The explicitly selected legacy vault is unavailable. Restart without --vault-root to use local storage.');
      }
    } catch (e) {
      if (active.current) { setInfo(null); setError(`Cannot open vault storage: ${String(e)}. Existing files have not been replaced.`); }
    } finally { busy.current = false; if (active.current) setLoading(false); }
  }, []);

  useEffect(() => {
    active.current = true;
    void detect();
    const unlisteners: UnlistenFn[] = [];
    const attach = (promise: Promise<UnlistenFn>) => { void promise.then(fn => { if (active.current) unlisteners.push(fn); else fn(); }); };
    attach(listen('pai-rescan-requested', () => { if (!infoRef.current || infoRef.current.storage_kind !== 'local') void detect(); }));
    attach(listen('pai-disconnected', () => {
      if (!active.current || infoRef.current?.storage_kind === 'local') return;
      setInfo(null); setPassword(''); setPhrase(''); setRecoveryKey('');
      setError('Legacy drive disconnected. The vault was locked.');
    }));
    return () => { active.current = false; unlisteners.forEach(fn => fn()); };
  }, [detect]);

  const local = info?.storage_kind === 'local';
  const fresh = local && info.local_vault_state === 'new';
  const interrupted = local && info.local_vault_state === 'interrupted';
  const finish = (id: string) => {
    setPassword(''); setConfirmation(''); setPhrase(''); setRecoveryKey('');
    if (active.current && info) onUnlock(id, info.vault_root, info.storage_kind);
  };
  const submit = async () => {
    if (busy.current || !info?.detected) return;
    setError(''); setNotice('');
    if (fresh && !recoveryKey && (password.length < 8 || password !== confirmation)) {
      setError('Choose a password of at least 8 characters and enter it identically twice.'); return;
    }
    busy.current = true; setLoading(true);
    try {
      if (interrupted) {
        await tauriApi.resumeLocalVault(password);
        setPassword('');
        busy.current = false;
        await detect();
        setNotice('Creation recovered. Unlock with the original password. If the recovery key was never displayed, keep your password safe; no new recovery key was issued.');
      } else if (fresh && !recoveryKey) {
        const result = await tauriApi.setupVault(password, null, info.vault_root);
        if (!result.success) throw new Error(result.error || 'Creation failed');
        if (active.current) { setRecoveryKey(result.recovery_key); setConfirmation(''); }
      } else {
        const result = recovering
          ? await tauriApi.recoverLocalVault(phrase)
          : await tauriApi.unlockVault(password, info.vault_root);
        if (!result.success) throw new Error(result.error || 'Unlock failed');
        finish(result.vault_id);
      }
    } catch (e) { if (active.current) setError(String(e)); }
    finally {
      busy.current = false;
      if (active.current) { setLoading(false); setPhrase(''); }
    }
  };
  const backup = async () => {
    if (busy.current) return;
    busy.current = true; setLoading(true); setError('');
    try { const path = await tauriApi.backupLocalVault(); if (active.current) setNotice(`Encrypted backup saved to ${path}. This is a same-device safety copy, not protection against disk loss. Copy it to your own secure backup storage. Restore/import is not automated; never run this backup as a second replica.`); }
    catch (e) { if (active.current) setError(`Backup failed: ${String(e)}. Partial backup files, if any, were retained and are not marked complete.`); }
    finally { busy.current = false; if (active.current) setLoading(false); }
  };

  return <div className="unlock-screen"><div className="unlock-card">
    <div className="unlock-logo">
      <h1>{recoveryKey ? 'Save your recovery key' : interrupted ? 'Recover interrupted creation' : fresh ? 'Create your local vault' : recovering ? 'Recover your local vault' : 'Unlock Vault'}</h1>
      <p>{local ? 'Private encrypted storage on this computer. No pendrive or account required.' : 'UnoOne Power — private local storage'}</p>
    </div>
    {info?.detected && <div className="unlock-status connected">{local ? 'Local installation' : 'Explicit legacy drive compatibility mode'} — {info.install_root || info.vault_root}</div>}
    {local && <p>Models and runtimes are not installed or qualified by vault setup. Local AI remains unavailable until verified provisioning is supported. Legacy import is separate and is not performed here.</p>}
    {interrupted && <p>An earlier creation was interrupted. Enter its original password to verify and finish it. An incomplete or malformed attempt will stay untouched for manual recovery; do not delete it or create over it.</p>}
    {recoveryKey ? <>
      <p>Write these words down privately. They unlock this vault. They are not saved in plaintext and cannot be shown again. Do not share them.</p>
      <div className="recovery-key-box">{recoveryKey}</div>
      <button className="btn btn-primary" disabled={loading} onClick={() => void submit()}>I've Saved My Recovery Key</button>
    </> : info?.detected && <form onSubmit={e => { e.preventDefault(); void submit(); }}>
      {recovering ? <div className="input-group">
        <label htmlFor="local-recovery">Recovery phrase (24 words)</label>
        <input id="local-recovery" type="password" autoComplete="off" value={phrase} onChange={e => setPhrase(e.target.value)} />
        <p>Recovery opens this session; it does not reset your password or create a new vault.</p>
      </div> : <div className="input-group">
        <label htmlFor="vault-password">{fresh ? 'New password' : 'Password'}</label>
        <input id="vault-password" type="password" autoComplete={fresh ? 'new-password' : 'current-password'} value={password} onChange={e => setPassword(e.target.value)} />
      </div>}
      {fresh && <div className="input-group"><label htmlFor="vault-confirm">Confirm password</label><input id="vault-confirm" type="password" autoComplete="new-password" value={confirmation} onChange={e => setConfirmation(e.target.value)} /></div>}
      <div className="unlock-actions"><button type="submit" className="btn btn-primary" disabled={loading}>{loading ? 'Working…' : interrupted ? 'Verify and finish creation' : fresh ? 'Create Vault' : recovering ? 'Unlock with recovery phrase' : 'Unlock'}</button></div>
    </form>}
    {local && !fresh && !interrupted && <div className="unlock-actions">
      <button className="btn btn-secondary" disabled={loading} onClick={() => { setRecovering(!recovering); setPassword(''); setPhrase(''); setError(''); }}>{recovering ? 'Use password instead' : 'Use recovery phrase'}</button>
      <button className="btn btn-secondary" disabled={loading} onClick={() => void backup()}>Create encrypted backup</button>
    </div>}
    {!recoveryKey && <button className="btn btn-ghost" disabled={loading} onClick={() => void detect()}>Retry detection</button>}
    {error && <div className="unlock-error" role="alert">{error}</div>}
    {notice && <p role="status">{notice}</p>}
  </div></div>;
}
