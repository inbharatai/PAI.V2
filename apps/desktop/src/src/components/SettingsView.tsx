import { useState, useEffect } from 'react';
import { tauriApi } from '../lib/tauri';
import type { AgentWorkspaceInfo, SecurityLevel } from '../lib/tauri';

interface SettingsViewProps {
  vaultRoot: string;
}

export function SettingsView({ vaultRoot }: SettingsViewProps) {
  const [settings, setSettings] = useState({
    language: 'en',
    securityLevel: 'STANDARD',
    theme: 'dark',
    modelPath: '',
    maxTokens: 4096,
    temperature: 0.7,
    autoLockMinutes: 5,
    usbAutoDetect: true,
  });
  const [appVersion, setAppVersion] = useState('v0.1.0');
  const [error, setError] = useState('');
  // P7 — user-granted agent workspace root (full-access lane).
  const [workspace, setWorkspace] = useState<AgentWorkspaceInfo | null>(null);
  const [workspaceInput, setWorkspaceInput] = useState('');
  const [workspaceMsg, setWorkspaceMsg] = useState('');
  const [workspaceBusy, setWorkspaceBusy] = useState(false);
  // P7 — additional granted folders the agent reaches by absolute path.
  const [folderInput, setFolderInput] = useState('');
  const [folderMsg, setFolderMsg] = useState('');

  // Load settings and version from backend on mount
  useEffect(() => {
    async function loadSettings() {
      try {
        const [secLevel, vaultInfo, backendSettings, version, workspaceInfo] = await Promise.all([
          tauriApi.getSecurityLevel(),
          tauriApi.detectVault(),
          tauriApi.getSettings(vaultRoot).catch(() => null),
          tauriApi.getVersion().catch(() => null),
          tauriApi.getAgentWorkspaceInfo().catch(() => null),
        ]);
        setSettings(prev => ({
          ...prev,
          securityLevel: secLevel,
          modelPath: vaultInfo.detected ? vaultInfo.vault_root + '\\MODELS\\gemma4-12b-q4' : prev.modelPath,
          ...(backendSettings ? {
            maxTokens: backendSettings.max_tokens,
            temperature: backendSettings.temperature,
          } : {}),
        }));
        if (version) setAppVersion(version);
        if (workspaceInfo) {
          setWorkspace(workspaceInfo);
          setWorkspaceInput(workspaceInfo.user_granted ?? '');
        }
      } catch (e) {
        setError(`Failed to load settings: ${e instanceof Error ? e.message : String(e)}`);
      }
    }
    loadSettings();
  }, [vaultRoot]);

  // Grant a new workspace root (validated + audited by the backend) or
  // revoke the grant and return to the default.
  const applyWorkspace = async (root: string | null) => {
    setWorkspaceBusy(true);
    setWorkspaceMsg('');
    try {
      const info = await tauriApi.setAgentWorkspaceRoot(root);
      setWorkspace(info);
      setWorkspaceInput(info.user_granted ?? '');
      setWorkspaceMsg(
        root === null
          ? 'Revoked — the agent builds in its default folder again.'
          : 'Granted — the agent now builds and stores files in this folder.',
      );
    } catch (e) {
      setWorkspaceMsg(`Rejected: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setWorkspaceBusy(false);
    }
  };

  // Grant or revoke one ADDITIONAL folder. The agent's fs tools reach these
  // by absolute path, fenced per folder; grants widen the file surface,
  // never the program allowlist. All validated + audited by the backend.
  const applyAddFolder = async () => {
    setWorkspaceBusy(true);
    setFolderMsg('');
    try {
      const info = await tauriApi.addAgentFolder(folderInput.trim());
      setWorkspace(info);
      setFolderInput('');
      setFolderMsg('Granted — the agent can now read and write inside this folder.');
    } catch (e) {
      setFolderMsg(`Rejected: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setWorkspaceBusy(false);
    }
  };

  const applyRemoveFolder = async (root: string) => {
    setWorkspaceBusy(true);
    setFolderMsg('');
    try {
      const info = await tauriApi.removeAgentFolder(root);
      setWorkspace(info);
      setFolderMsg('Revoked — the agent can no longer reach that folder.');
    } catch (e) {
      setFolderMsg(`Rejected: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setWorkspaceBusy(false);
    }
  };

  const handleChange = (key: string, value: string | number | boolean) => {
    setSettings(prev => ({ ...prev, [key]: value }));
    if (key === 'securityLevel') {
      tauriApi.setSecurityLevel(value as SecurityLevel).catch((err) => {
        console.error('[SettingsView] setSecurityLevel failed:', err);
      });
    }
  };

  return (
    <div>
      <div className="main-header">
        <h2>Settings</h2>
      </div>

      <div className="main-body">
        <div className="settings-view">
          {error && (
            <div style={{ padding: '12px 16px', marginBottom: '16px', background: 'var(--error-bg, #3a1c1c)', color: 'var(--error-text, #ff9e9e)', border: '1px solid var(--border)', borderRadius: 'var(--radius-md)', fontSize: '13px' }}>
              ⚠️ {error}
            </div>
          )}
          {/* General */}
          <div className="settings-section">
            <div className="settings-section-header">General</div>
            <div className="settings-section-body">
              <div className="settings-row">
                <div>
                  <div className="settings-row-label">Language</div>
                  <div className="settings-row-desc">Written language for model output</div>
                </div>
                <select
                  value={settings.language}
                  onChange={e => handleChange('language', e.target.value)}
                >
                  <option value="en">English</option>
                  <option value="hi">हिन्दी (Hindi)</option>
                  <option value="bn">বাংলা (Bengali)</option>
                  <option value="ta">தமிழ் (Tamil)</option>
                  <option value="te">తెలుగు (Telugu)</option>
                  <option value="kn">ಕನ್ನಡ (Kannada)</option>
                  <option value="ml">മലയാളം (Malayalam)</option>
                </select>
              </div>
              <div className="settings-row">
                <div>
                  <div className="settings-row-label">Theme</div>
                  <div className="settings-row-desc">Application color theme</div>
                </div>
                <select
                  value={settings.theme}
                  onChange={e => handleChange('theme', e.target.value)}
                >
                  <option value="dark">Dark</option>
                  <option value="light">Light</option>
                  <option value="system">System</option>
                </select>
              </div>
              <div className="settings-row">
                <div>
                  <div className="settings-row-label">Auto-Lock Timer</div>
                  <div className="settings-row-desc">Lock vault after inactivity</div>
                </div>
                <select
                  value={settings.autoLockMinutes}
                  onChange={e => handleChange('autoLockMinutes', Number(e.target.value))}
                >
                  <option value={1}>1 minute</option>
                  <option value={5}>5 minutes</option>
                  <option value={15}>15 minutes</option>
                  <option value={30}>30 minutes</option>
                  <option value={0}>Never</option>
                </select>
              </div>
            </div>
          </div>

          {/* Security */}
          <div className="settings-section">
            <div className="settings-section-header">Security</div>
            <div className="settings-section-body">
              <div className="settings-row">
                <div>
                  <div className="settings-row-label">Security Level</div>
                  <div className="settings-row-desc">Controls how aggressively the safety guard filters model output</div>
                </div>
                <select
                  value={settings.securityLevel}
                  onChange={e => handleChange('securityLevel', e.target.value)}
                >
                  <option value="OFF">OFF — No filtering (not recommended)</option>
                  <option value="STANDARD">STANDARD — Balanced safety</option>
                  <option value="RELAXED">RELAXED — Reduced filtering</option>
                </select>
              </div>
              <div className="settings-row">
                <div>
                  <div className="settings-row-label">USB Auto-Detect</div>
                  <div className="settings-row-desc">Automatically detect when Pocket USB is connected</div>
                </div>
                <label style={{ display: 'flex', alignItems: 'center', cursor: 'pointer' }}>
                  <input
                    type="checkbox"
                    checked={settings.usbAutoDetect}
                    onChange={e => handleChange('usbAutoDetect', e.target.checked)}
                    style={{ width: '16px', height: '16px' }}
                  />
                </label>
              </div>
            </div>
          </div>

          {/* Model */}
          <div className="settings-section">
            <div className="settings-section-header">Model</div>
            <div className="settings-section-body">
              <div className="settings-row">
                <div>
                  <div className="settings-row-label">GGUF Model Path</div>
                  <div className="settings-row-desc">Path to Gemma 4 12B Q4 GGUF model on Pocket USB</div>
                </div>
                <input
                  type="text"
                  value={settings.modelPath}
                  onChange={e => handleChange('modelPath', e.target.value)}
                  style={{ width: '280px' }}
                />
              </div>
              <div className="settings-row">
                <div>
                  <div className="settings-row-label">Max Tokens</div>
                  <div className="settings-row-desc">Maximum tokens per response</div>
                </div>
                <select
                  value={settings.maxTokens}
                  onChange={e => handleChange('maxTokens', Number(e.target.value))}
                >
                  <option value={2048}>2048</option>
                  <option value={4096}>4096</option>
                  <option value={8192}>8192</option>
                  <option value={16384}>16384</option>
                </select>
              </div>
              <div className="settings-row">
                <div>
                  <div className="settings-row-label">Temperature</div>
                  <div className="settings-row-desc">Controls randomness (0 = deterministic, 1 = creative)</div>
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                  <input
                    type="range"
                    min="0"
                    max="1"
                    step="0.1"
                    value={settings.temperature}
                    onChange={e => handleChange('temperature', Number(e.target.value))}
                    style={{ width: '120px' }}
                  />
                  <span style={{ fontFamily: 'var(--font-mono)', fontSize: '13px', width: '32px' }}>
                    {settings.temperature.toFixed(1)}
                  </span>
                </div>
              </div>
            </div>
          </div>

          {/* Agent Workspace (P7 — user-granted) */}
          <div className="settings-section">
            <div className="settings-section-header">Agent Workspace</div>
            <div className="settings-section-body">
              <div className="settings-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: '8px' }}>
                <div>
                  <div className="settings-row-label">Build Folder</div>
                  <div className="settings-row-desc">
                    Where the agent builds and stores its files in full-access mode. Grant a host folder
                    (e.g. the Desktop) to have builds stored there; the grant is this-computer-only,
                    recorded in the vault audit trail, and revocable at any time. The agent can never be
                    granted a folder inside the encrypted Pocket AI drive.
                  </div>
                </div>
                <div style={{ fontSize: '12px', color: 'var(--text-secondary)', fontFamily: 'var(--font-mono)' }}>
                  Effective: {workspace ? workspace.effective_root : '…'}
                  {workspace?.user_granted && ' (user-granted)'}
                </div>
                <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
                  <input
                    type="text"
                    placeholder={workspace?.default_root ? `e.g. C:\\Users\\reetu\\Desktop` : ''}
                    value={workspaceInput}
                    onChange={e => { setWorkspaceInput(e.target.value); setWorkspaceMsg(''); }}
                    style={{ width: '340px' }}
                  />
                  <button
                    disabled={workspaceBusy || !workspaceInput.trim()}
                    onClick={() => void applyWorkspace(workspaceInput.trim())}
                  >
                    Grant
                  </button>
                  <button
                    disabled={workspaceBusy || !workspace?.user_granted}
                    onClick={() => void applyWorkspace(null)}
                  >
                    Revoke
                  </button>
                </div>
                {workspaceMsg && (
                  <div style={{ fontSize: '12px', color: 'var(--text-secondary)' }}>{workspaceMsg}</div>
                )}
              </div>
              <div className="settings-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: '8px' }}>
                <div>
                  <div className="settings-row-label">Additional Granted Folders</div>
                  <div className="settings-row-desc">
                    Extra folders the agent may read and write (reached by absolute path, fenced per
                    folder, audited on every grant and revocation). Grant e.g. the Desktop or a folder
                    on a second drive — never a folder inside the encrypted Pocket AI drive, and never
                    a whole drive.
                  </div>
                </div>
                {(workspace?.folders.length ?? 0) > 0 && (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: '6px' }}>
                    {workspace?.folders.map((folder) => (
                      <div
                        key={folder.root}
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          gap: '8px',
                          fontSize: '12px',
                          fontFamily: 'var(--font-mono)',
                        }}
                      >
                        <span style={{ flex: 1, wordBreak: 'break-all' }}>
                          {folder.root}
                          {!folder.exists && (
                            <span title="the folder no longer exists — the grant is not honored"> ⚠ missing</span>
                          )}
                        </span>
                        <button
                          disabled={workspaceBusy}
                          onClick={() => void applyRemoveFolder(folder.root)}
                        >
                          Revoke
                        </button>
                      </div>
                    ))}
                  </div>
                )}
                <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
                  <input
                    type="text"
                    placeholder="e.g. C:\Users\reetu\Desktop"
                    value={folderInput}
                    onChange={e => { setFolderInput(e.target.value); setFolderMsg(''); }}
                    style={{ width: '340px' }}
                  />
                  <button
                    disabled={workspaceBusy || !folderInput.trim()}
                    onClick={() => void applyAddFolder()}
                  >
                    Grant Folder
                  </button>
                </div>
                {folderMsg && (
                  <div style={{ fontSize: '12px', color: 'var(--text-secondary)' }}>{folderMsg}</div>
                )}
              </div>
            </div>
          </div>

          {/* About */}
          <div className="settings-section">
            <div className="settings-section-header">About</div>
            <div className="settings-section-body">
              <div style={{ fontSize: '13px', color: 'var(--text-secondary)', lineHeight: 1.6 }}>
                <p><strong>UnoOne Power</strong> {appVersion}</p>
                <p>Private AI Desktop Workstation</p>
                <p style={{ marginTop: '8px' }}>Model: Gemma 4 12B Q4_K_M GGUF</p>
                <p>Runtime: llama.cpp</p>
                <p>Encryption: Argon2id + XChaCha20-Poly1305</p>
                <p>Vault: PocketMemoryVault (USB canonical)</p>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}