import { useState, useEffect, useCallback, useRef, Component, type ReactNode } from 'react';
import { UnlockScreen } from './components/UnlockScreen';
import { Sidebar, type ViewId } from './components/Sidebar';
import { ChatView } from './components/ChatView';
import { RecordingView } from './components/RecordingView';
import { MemoryExplorer } from './components/MemoryExplorer';
import { VaultView } from './components/VaultView';
import { SettingsView } from './components/SettingsView';
import { HardwareProfile } from './components/HardwareProfile';
import { ModelManager } from './components/ModelManager';
import { BrowserWorkspace } from './components/BrowserWorkspace';
import { CapabilityProfile } from './components/CapabilityProfile';
import { DocumentsView } from './components/DocumentsView';
import { AccessibilityView } from './components/AccessibilityView';
import { tauriApi, type StartupPhase } from './lib/tauri';
import { listen } from '@tauri-apps/api/event';

type AppScreen = 'unlock' | 'main';

interface ErrorBoundaryProps {
  children: ReactNode;
}

interface ErrorBoundaryState {
  hasError: boolean;
  error: Error | null;
}

class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error };
  }

  render() {
    if (this.state.hasError) {
      return (
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', height: '100vh', gap: '16px' }}>
          <h2>Something went wrong</h2>
          <p style={{ color: 'var(--text-muted)', maxWidth: '400px', textAlign: 'center' }}>
            {this.state.error?.message || 'An unexpected error occurred.'}
          </p>
          <button onClick={() => window.location.reload()} style={{ padding: '8px 24px' }}>
            Reload
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

function App() {
  const [screen, setScreen] = useState<AppScreen>('unlock');
  const [currentView, setCurrentView] = useState<ViewId>('chat');
  const [vaultId, setVaultId] = useState<string>('');
  const [vaultRoot, setVaultRoot] = useState<string>('');
  const [autoLockMs, setAutoLockMs] = useState<number>(300000); // default 5 min
  const [bootError, setBootError] = useState('');
  const [startupPhase, setStartupPhase] = useState<StartupPhase>('STARTING');
  const bootstrappedRoot = useRef('');

  const handleUnlock = useCallback((id: string, root: string) => {
    setVaultId(id);
    setVaultRoot(root);
    setScreen('main');
  }, []);

  const handleLock = useCallback(() => {
    void tauriApi.stopModelServer().catch(() => undefined);
    void tauriApi.lockVault().catch(() => undefined);
    bootstrappedRoot.current = '';
    setVaultId('');
    setVaultRoot('');
    setScreen('unlock');
    setCurrentView('chat');
  }, []);

  // Load settings to get auto-lock timer; re-fetch when vaultId changes
  useEffect(() => {
    if (screen !== 'main' || !vaultId) return;
    tauriApi.getSettings(vaultRoot || '').then(settings => {
      if (settings?.auto_lock_minutes) {
        setAutoLockMs(settings.auto_lock_minutes * 60 * 1000);
      }
    }).catch(e => {
      console.error('[App] getSettings failed:', e);
      setBootError(prev => prev || `Settings load failed: ${e instanceof Error ? e.message : String(e)}`);
    });
  }, [screen, vaultId, vaultRoot]);

  // The Pocket AI pen drive owns the runtime and model. After unlock, start
  // only the manifest-verified bundled llama-server. The backend alone moves
  // the state to READY after model identity and health verification.
  useEffect(() => {
    if (screen !== 'main' || !vaultRoot) return;
    if (bootstrappedRoot.current === vaultRoot) return;
    bootstrappedRoot.current = vaultRoot;
    let cancelled = false;
    void (async () => {
      try {
        setBootError('');
        await tauriApi.getHardwareProfile();
        const models = await tauriApi.listModels(vaultRoot);
        const desktopModel = models.find(model =>
          model.available && model.model_type.toLowerCase().includes('12b')
        );
        if (!desktopModel) {
          throw new Error('No manifest-verified Gemma 12B desktop model is available.');
        }
        await tauriApi.detectAcceleration();
        const config = await tauriApi.getModelConfig();
        await tauriApi.startModelServer({
          ...config,
          model_path: desktopModel.path,
          mmproj_path: desktopModel.mmproj_path,
        }, vaultRoot);
        const health = await tauriApi.checkModelHealth();
        if (!health.model_id) {
          throw new Error('The model server responded without a verified model identity.');
        }
      } catch (e) {
        if (cancelled) return;
        await tauriApi.setStartupLimited().catch(() => undefined);
        setBootError(`Limited mode: ${e instanceof Error ? e.message : String(e)}`);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [screen, vaultRoot]);

  useEffect(() => {
    if (screen !== 'main') return;
    let active = true;
    const refresh = () => {
      void tauriApi.getStartupStatus()
        .then(status => {
          if (active) setStartupPhase(status.phase);
        })
        .catch(() => undefined);
    };
    refresh();
    const interval = window.setInterval(refresh, 1000);
    return () => {
      active = false;
      window.clearInterval(interval);
    };
  }, [screen]);

  // Lock immediately when the canonical Pocket AI pen drive is removed.
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<string>('pai-disconnected', () => {
      if (!active) return;
      handleLock();
      setBootError('Pocket AI was disconnected. Inference and recording were stopped and the vault was locked.');
    }).then(fn => { unlisten = fn; });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [handleLock]);

  // Auto-lock on window blur (timer from settings)
  useEffect(() => {
    if (screen !== 'main') return;
    let timer: number | null = null;
    const handleBlur = () => {
      timer = window.setTimeout(() => {
        handleLock();
      }, autoLockMs);
    };
    const handleFocus = () => {
      if (timer) window.clearTimeout(timer);
    };
    window.addEventListener('blur', handleBlur);
    window.addEventListener('focus', handleFocus);
    return () => {
      window.removeEventListener('blur', handleBlur);
      window.removeEventListener('focus', handleFocus);
      if (timer) window.clearTimeout(timer);
    };
  }, [screen, handleLock, autoLockMs]);

  if (screen === 'unlock') {
    return (
      <ErrorBoundary>
        <UnlockScreen onUnlock={handleUnlock} />
      </ErrorBoundary>
    );
  }

  const renderView = () => {
    switch (currentView) {
      case 'chat':
        return <ChatView />;
      case 'recordings':
        return <RecordingView />;
      case 'memory':
        return <MemoryExplorer />;
      case 'vault':
        return <VaultView />;
      case 'model':
        return <ModelManager />;
      case 'browser':
        return <BrowserWorkspace />;
      case 'documents':
        return <DocumentsView />;
      case 'accessibility':
        return <AccessibilityView />;
      case 'capability':
        return <CapabilityProfile />;
      case 'hardware':
        return <HardwareProfile />;
      case 'settings':
        return <SettingsView vaultRoot={vaultRoot} />;
      default:
        return <ChatView />;
    }
  };

  return (
    <ErrorBoundary>
      <div className="app-shell">
        <Sidebar currentView={currentView} onNavigate={setCurrentView} onLock={handleLock} />
        <div className="main-content">
          {startupPhase !== 'READY' && (
            <div style={{ padding: '8px 16px', background: 'var(--surface-secondary)', color: 'var(--text-secondary)', borderBottom: '1px solid var(--border)', fontSize: '12px' }}>
              Pocket AI startup: {startupPhase.replaceAll('_', ' ')}
            </div>
          )}
          {bootError && (
            <div style={{ padding: '12px 16px', background: 'var(--error-bg, #3a1c1c)', color: 'var(--error-text, #ff9e9e)', borderBottom: '1px solid var(--border)', fontSize: '13px' }}>
              ⚠️ {bootError}
            </div>
          )}
          {renderView()}
        </div>
      </div>
    </ErrorBoundary>
  );
}

export default App;
