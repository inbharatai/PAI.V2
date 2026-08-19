import { useState, useCallback, useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { tauriApi, type VaultInfo } from '../lib/tauri';

interface UnlockScreenProps {
  onUnlock: (vaultId: string, vaultRoot: string) => void;
}

export function UnlockScreen({ onUnlock }: UnlockScreenProps) {
  const [mode, setMode] = useState<'detect' | 'unlock' | 'setup'>('detect');
  const [vaultInfo, setVaultInfo] = useState<VaultInfo | null>(null);
  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [profileName, setProfileName] = useState('');
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);
  const [setupStep, setSetupStep] = useState(0);
  const [recoveryKey, setRecoveryKey] = useState('');
  const scanInFlight = useRef(false);

  const handleDetect = useCallback(async () => {
    if (scanInFlight.current) return;
    scanInFlight.current = true;
    setLoading(true);
    setError('');
    try {
      const info = await tauriApi.detectVault();
      setVaultInfo(info);
      if (info.detected) {
        setMode('unlock');
      } else if (info.startup_state === 'PAI_INVALID') {
        const details = info.validation_failures
          .slice(0, 3)
          .map(failure => `${failure.code}: ${failure.message}${failure.path ? ` (${failure.path})` : ''}`)
          .join('\n');
        setError(`Pocket AI was found but rejected because validation failed.\n${details}`);
      } else {
        setError('');
      }
    } catch (e) {
      setError(`Detection failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setLoading(false);
      scanInFlight.current = false;
    }
  }, []);

  useEffect(() => {
    let active = true;
    const unlisteners: UnlistenFn[] = [];

    // Prototype dev bypass: if the binary was built with the dev-bypass cargo
    // feature AND UNOONE_DEV_BYPASS=1 is set, attempt auto-unlock first. In
    // release binaries this command doesn't exist, so the catch falls through
    // to the normal interactive flow silently.
    void (async () => {
      try {
        const result = await tauriApi.devBypassUnlock();
        if (active && result.success) {
          // The Rust state already holds the root; fetch it for the frontend
          // (App.tsx model bootstrap keys off vaultRoot).
          const info = await tauriApi.detectVault().catch(() => null);
          onUnlock(result.vault_id, info?.vault_root || '');
          return;
        }
      } catch {
        // Command not available (release build) — normal flow.
      }
      void handleDetect();
    })();

    const interval = window.setInterval(() => {
      if (active && mode === 'detect') void handleDetect();
    }, 3000);

    void listen('pai-rescan-requested', () => {
      if (active) void handleDetect();
    }).then(unlisten => unlisteners.push(unlisten));
    void listen<string>('pai-disconnected', () => {
      if (!active) return;
      setVaultInfo(null);
      setMode('detect');
      setPassword('');
      setError('Pocket AI was disconnected. The vault has been locked.');
    }).then(unlisten => unlisteners.push(unlisten));

    return () => {
      active = false;
      window.clearInterval(interval);
      unlisteners.forEach(unlisten => unlisten());
    };
  }, [handleDetect, mode, onUnlock]);

  const handleUnlock = useCallback(async () => {
    if (!password) {
      setError('Enter your password');
      return;
    }
    setLoading(true);
    setError('');
    try {
      const result = await tauriApi.unlockVault(password, vaultInfo?.vault_root || '');
      if (result.success) {
        onUnlock(result.vault_id, vaultInfo?.vault_root || '');
      } else {
        setError(result.error || 'Incorrect password');
      }
    } catch (e) {
      setError(`Unlock failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setLoading(false);
    }
  }, [password, vaultInfo, onUnlock]);

  const handleSetup = useCallback(async () => {
    if (!password) {
      setError('Enter a password');
      return;
    }
    if (password !== confirmPassword) {
      setError('Passwords do not match');
      return;
    }
    if (password.length < 8) {
      setError('Password must be at least 8 characters');
      return;
    }
    setLoading(true);
    setError('');
    try {
      const result = await tauriApi.setupVault(password, profileName || null, vaultInfo?.vault_root || '');
      if (result.success) {
        setRecoveryKey(result.recovery_key);
        setSetupStep(2);
      } else {
        setError(result.error || 'Setup failed');
      }
    } catch (e) {
      setError(`Setup failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setLoading(false);
    }
  }, [password, confirmPassword, profileName, vaultInfo]);

  // After a successful setup the vault exists on disk but is still locked in
  // the Rust state (setup_vault only writes the header). Entering the app
  // without unlocking left DesktopVaultState empty ("No Pocket USB") and broke
  // every vault-backed feature. Unlock with the password the user just chose
  // before declaring the session unlocked.
  const handleFinishSetup = useCallback(async () => {
    setLoading(true);
    setError('');
    try {
      const result = await tauriApi.unlockVault(password, vaultInfo?.vault_root || '');
      if (result.success) {
        onUnlock(result.vault_id, vaultInfo?.vault_root || '');
      } else {
        setError(result.error || 'Vault created, but the automatic unlock failed');
      }
    } catch (e) {
      setError(`Vault created, but the automatic unlock failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setLoading(false);
    }
  }, [password, vaultInfo, onUnlock]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') {
      if (mode === 'unlock') handleUnlock();
      else if (mode === 'setup' && setupStep === 1) handleSetup();
    }
  };

  // Step 0: Detect USB
  if (mode === 'detect') {
    return (
      <div className="unlock-screen">
        <div className="unlock-card">
          <div className="unlock-logo">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M21 2H3a2 2 0 0 0-2 2v4a2 2 0 0 0 2 2h1" />
              <path d="M21 2v16a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V2" />
              <path d="M21 2h-2" />
              <circle cx="12" cy="12" r="2" />
              <path d="M12 14v2" />
            </svg>
            <h1>UnoOne Power</h1>
            <p>Private AI Desktop Workstation</p>
          </div>

          <div className="unlock-actions">
            <button className="btn btn-primary" onClick={handleDetect} disabled={loading}>
              {loading ? <span className="spinner" /> : (
                <>
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <path d="M21 12a9 9 0 0 1-9 9m9-9a9 9 0 0 0-9-9m9 9H3m0 0a9 9 0 0 1 9-9m-9 9a9 9 0 0 0 9 9" />
                  </svg>
                  Rescan
                </>
              )}
            </button>
          </div>

          {error && <div className="unlock-error">{error}</div>}

          <div className="unlock-status disconnected">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
              <line x1="12" y1="9" x2="12" y2="13" />
              <line x1="12" y1="17" x2="12.01" y2="17" />
            </svg>
            {loading ? 'Validating Pocket AI…' : 'Waiting for the Pocket AI pen drive…'}
          </div>
        </div>
      </div>
    );
  }

  // Setup mode
  if (mode === 'setup') {
    return (
      <div className="setup-screen">
        <div className="setup-card">
          <div className="unlock-logo">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
              <path d="M7 11V7a5 5 0 0 1 10 0v4" />
            </svg>
            <h1>Create Vault</h1>
            <p>Set up a new encrypted vault on your Pocket USB</p>
          </div>

          <div className="setup-steps">
            <div className={`setup-step ${setupStep >= 0 ? 'active' : ''}`} />
            <div className={`setup-step ${setupStep >= 1 ? 'active' : ''}`} />
            <div className={`setup-step ${setupStep >= 2 ? 'active' : ''}`} />
          </div>

          {setupStep === 0 && (
            <>
              <div className="input-group">
                <label>Profile Name (optional)</label>
                <input
                  type="text"
                  placeholder="e.g., Personal, Work"
                  value={profileName}
                  onChange={e => setProfileName(e.target.value)}
                />
              </div>
              <div className="unlock-actions">
                <button className="btn btn-primary" onClick={() => setSetupStep(1)}>
                  Continue
                </button>
                <button className="btn btn-ghost" onClick={() => setMode('detect')}>
                  Back
                </button>
              </div>
            </>
          )}

          {setupStep === 1 && (
            <>
              <div className="input-group">
                <label>Password</label>
                <input
                  type="password"
                  placeholder="At least 8 characters"
                  value={password}
                  onChange={e => { setPassword(e.target.value); setError(''); }}
                  onKeyDown={handleKeyDown}
                  autoFocus
                />
              </div>
              <div className="input-group">
                <label>Confirm Password</label>
                <input
                  type="password"
                  placeholder="Re-enter your password"
                  value={confirmPassword}
                  onChange={e => { setConfirmPassword(e.target.value); setError(''); }}
                  onKeyDown={handleKeyDown}
                />
              </div>
              {error && <div className="unlock-error">{error}</div>}
              <div className="unlock-actions">
                <button className="btn btn-primary" onClick={handleSetup} disabled={loading}>
                  {loading ? <span className="spinner" /> : 'Create Vault'}
                </button>
                <button className="btn btn-ghost" onClick={() => setSetupStep(0)}>
                  Back
                </button>
              </div>
            </>
          )}

          {setupStep === 2 && (
            <>
              <p style={{ color: 'var(--text-secondary)', fontSize: '13px', textAlign: 'center' }}>
                Write down this recovery key and store it safely. You will need it if you forget your password.
              </p>
              <div className="recovery-key-box">{recoveryKey}</div>
              <button className="btn btn-secondary" style={{ marginTop: '8px', width: '100%' }} onClick={() => navigator.clipboard.writeText(recoveryKey)}>
                Copy to Clipboard
              </button>
              {error && <div className="unlock-error">{error}</div>}
              <div className="unlock-actions">
                <button className="btn btn-primary" onClick={handleFinishSetup} disabled={loading}>
                  {loading ? <span className="spinner" /> : "I've Saved My Recovery Key"}
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    );
  }

  // Unlock mode
  return (
    <div className="unlock-screen">
      <div className="unlock-card">
        <div className="unlock-logo">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
            <path d="M7 11V7a5 5 0 0 1 10 0v4" />
          </svg>
          <h1>Unlock Vault</h1>
          <p>Enter your password to access your Pocket</p>
        </div>

        {vaultInfo?.detected && (
          <div className="unlock-status connected">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
              <polyline points="22 4 12 14.01 9 11.01" />
            </svg>
            Pocket USB detected — {vaultInfo.vault_root}
          </div>
        )}

        <div className="unlock-form">
          <div className="input-group">
            <label>Password</label>
            <input
              type="password"
              placeholder="Enter your vault password"
              value={password}
              onChange={e => { setPassword(e.target.value); setError(''); }}
              onKeyDown={handleKeyDown}
              autoFocus
            />
          </div>

          {error && <div className="unlock-error">{error}</div>}

          <div className="unlock-actions">
            <button className="btn btn-primary" onClick={handleUnlock} disabled={loading}>
              {loading ? <span className="spinner" /> : 'Unlock'}
            </button>
            <button className="btn btn-secondary" onClick={handleDetect}>
              Rescan
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
