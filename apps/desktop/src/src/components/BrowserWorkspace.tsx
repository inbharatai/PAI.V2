import { useEffect, useState } from 'react';
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { tauriApi, type BrowserAction } from '../lib/tauri';

const WEBVIEW_LABEL = 'browser-workspace';

export function BrowserWorkspace() {
  const [url, setUrl] = useState('https://example.com');
  const [sessionActive, setSessionActive] = useState(false);
  const [error, setError] = useState('');
  const [lastResult, setLastResult] = useState('');
  const [selector, setSelector] = useState('body');
  const [formSelector, setFormSelector] = useState('input[name="q"]');
  const [formValue, setFormValue] = useState('UnoOne');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    return () => {
      void stopSession();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const setOpError = (e: unknown) => {
    const message = e instanceof Error ? e.message : String(e);
    setError(message);
  };

  const injectBridge = async () => {
    try {
      const bridgeScript = await tauriApi.getBrowserBridgeScript();
      await tauriApi.browserEval(WEBVIEW_LABEL, bridgeScript);
    } catch (e) {
      // Bridge injection failures are non-fatal; each action also re-injects the bridge.
      console.warn('Bridge re-injection failed:', e);
    }
  };

  const startSession = async () => {
    setError('');
    setLastResult('');
    try {
      const bind = await tauriApi.startBrowserSession(undefined, WEBVIEW_LABEL);
      if (!bind.success) {
        throw new Error(bind.error || 'Backend refused to bind a browser session');
      }
      const webview = new WebviewWindow(WEBVIEW_LABEL, {
        url: 'about:blank',
        width: 1280,
        height: 800,
        title: 'Browser Workspace',
      });
      webview.once('tauri://error', e => {
        setOpError(e instanceof Error ? e : new Error(String(e)));
      });
      webview.once('tauri://created', () => {
        setSessionActive(true);
        void injectBridge();
      });
    } catch (e) {
      setOpError(e);
    }
  };

  const stopSession = async () => {
    setError('');
    try {
      await tauriApi.stopBrowserSession();
      const webview = await WebviewWindow.getByLabel(WEBVIEW_LABEL);
      if (webview) {
        await webview.close();
      }
    } catch (e) {
      setOpError(e);
    } finally {
      setSessionActive(false);
    }
  };

  /**
   * Execute a typed action. The backend runs it against the live webview and
   * reports what actually happened. Actions flagged CONFIRMATION_REQUIRED
   * (form submit, upload, download, account change) prompt the user and retry
   * with explicit consent.
   */
  const runAction = async (action: BrowserAction) => {
    setError('');
    setBusy(true);
    try {
      let result = await tauriApi.executeBrowserAction(action, false);
      const riskyRefusal =
        !result.success && (result.error || '').toUpperCase().includes('CONFIRMATION_REQUIRED');
      if (riskyRefusal) {
        const consent = window.confirm(
          `${result.error}\n\nProceed with this potentially state-changing action?`
        );
        if (!consent) {
          setLastResult(`REFUSED BY USER\n${result.error}`);
          return;
        }
        result = await tauriApi.executeBrowserAction(action, true);
      }
      const lines = [
        result.success ? `OK (verified=${result.verified})` : `FAILED: ${result.error}`,
        result.current_url ? `url: ${result.current_url}` : '',
        result.current_title ? `title: ${result.current_title}` : '',
        result.screenshot_path
          ? `screenshot: ${result.screenshot_path}\nsha256: ${result.screenshot_sha256}`
          : '',
        typeof result.data === 'object' && result.data !== null
          ? JSON.stringify(result.data, null, 2)
          : '',
      ].filter(Boolean);
      setLastResult(lines.join('\n'));
      if (!result.success) {
        setError(result.error || 'Action failed');
      }
    } catch (e) {
      setOpError(e);
    } finally {
      setBusy(false);
    }
  };

  const navigate = async () => {
    await runAction({ type: 'Navigate', url });
  };

  const extractText = async () => {
    await runAction({ type: 'ExtractElementText', selector });
  };

  const extractPageText = async () => {
    await runAction({ type: 'ExtractPageText' });
  };

  const getPageInfo = async () => {
    await runAction({ type: 'GetPageInfo' });
  };

  const scrollDown = async () => {
    await runAction({ type: 'Scroll', direction: 'Down', amount: 400 });
  };

  const clickSelector = async () => {
    await runAction({ type: 'Click', selector });
  };

  const fillForm = async () => {
    await runAction({
      type: 'FillForm',
      fields: [{ selector: formSelector, value: formValue }],
    });
  };

  const screenshot = async () => {
    await runAction({ type: 'Screenshot' });
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      <div className="main-header">
        <h2>Browser Workspace</h2>
        <div className="main-header-actions">
          <span className={`hw-badge ${sessionActive ? 'available' : 'unavailable'}`}>
            {sessionActive ? 'Session Active' : 'Session Inactive'}
          </span>
        </div>
      </div>

      <div
        className="main-body"
        style={{ display: 'flex', flexDirection: 'column', gap: '16px', padding: '16px' }}
      >
        {error && (
          <div
            style={{
              padding: '8px 12px',
              background: 'rgba(239,68,68,0.1)',
              border: '1px solid rgba(239,68,68,0.3)',
              borderRadius: 'var(--radius-sm)',
              fontSize: '13px',
              color: 'var(--danger)',
            }}
          >
            {error}
          </div>
        )}

        <div style={{ display: 'flex', gap: '8px', alignItems: 'center' }}>
          <input
            type="text"
            value={url}
            onChange={e => setUrl(e.target.value)}
            placeholder="https://example.com"
            style={{ flex: 1 }}
            disabled={busy}
          />
          <button className="btn btn-primary" onClick={navigate} disabled={busy}>
            Navigate
          </button>
          {!sessionActive ? (
            <button className="btn btn-success" onClick={startSession} disabled={busy}>
              Start Session
            </button>
          ) : (
            <button className="btn btn-danger" onClick={stopSession} disabled={busy}>
              Stop Session
            </button>
          )}
          <button
            className="btn btn-secondary"
            onClick={() => void runAction({ type: 'Back' })}
            disabled={!sessionActive || busy}
          >
            Back
          </button>
          <button
            className="btn btn-secondary"
            onClick={() => void runAction({ type: 'Reload' })}
            disabled={!sessionActive || busy}
          >
            Reload
          </button>
        </div>

        <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
          <button className="btn btn-secondary btn-sm" onClick={extractText} disabled={busy}>
            Extract Element Text
          </button>
          <button className="btn btn-secondary btn-sm" onClick={extractPageText} disabled={busy}>
            Extract Page Text
          </button>
          <button className="btn btn-secondary btn-sm" onClick={getPageInfo} disabled={busy}>
            Page Info
          </button>
          <button className="btn btn-secondary btn-sm" onClick={scrollDown} disabled={busy}>
            Scroll Down
          </button>
          <button className="btn btn-secondary btn-sm" onClick={clickSelector} disabled={busy}>
            Click Selector
          </button>
          <button className="btn btn-secondary btn-sm" onClick={fillForm} disabled={busy}>
            Fill Form
          </button>
          <button className="btn btn-secondary btn-sm" onClick={screenshot} disabled={busy}>
            Screenshot (real PNG)
          </button>
        </div>

        <div style={{ display: 'flex', gap: '12px', flexWrap: 'wrap' }}>
          <div className="input-group" style={{ flex: 1, minWidth: '200px' }}>
            <label>Selector</label>
            <input
              type="text"
              value={selector}
              onChange={e => setSelector(e.target.value)}
              disabled={busy}
            />
          </div>
          <div className="input-group" style={{ flex: 1, minWidth: '200px' }}>
            <label>Form selector</label>
            <input
              type="text"
              value={formSelector}
              onChange={e => setFormSelector(e.target.value)}
              disabled={busy}
            />
          </div>
          <div className="input-group" style={{ flex: 1, minWidth: '200px' }}>
            <label>Form value</label>
            <input
              type="text"
              value={formValue}
              onChange={e => setFormValue(e.target.value)}
              disabled={busy}
            />
          </div>
        </div>

        <div
          style={{
            flex: 1,
            minHeight: '120px',
            padding: '12px',
            background: 'var(--bg-secondary)',
            border: '1px solid var(--border)',
            borderRadius: 'var(--radius-md)',
            fontFamily: 'var(--font-mono)',
            fontSize: '12px',
            overflow: 'auto',
            whiteSpace: 'pre-wrap',
            color: 'var(--text-secondary)',
          }}
        >
          {lastResult || 'Action results will appear here…'}
        </div>

        <div
          style={{
            padding: '12px',
            background: 'var(--bg-tertiary)',
            borderRadius: 'var(--radius-md)',
            fontSize: '12px',
            color: 'var(--text-muted)',
          }}
        >
          <p><strong>Controlled browser:</strong> typed actions only — there is no arbitrary-script execution.</p>
          <p><strong>Verification:</strong> results come from the live page; bad selectors fail; risky elements (submit/upload/download) require explicit confirmation.</p>
        </div>
      </div>
    </div>
  );
}
