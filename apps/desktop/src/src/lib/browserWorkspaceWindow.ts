// The agent lane's browser window — created through the SAME proven JS path
// the BrowserWorkspace UI uses. Live-caught 2026-09-15 (defect #40): a
// window built from Rust — even on the main thread via run_on_main_thread —
// produced a window shell whose WebView2 content process never started
// (no CDP target, eval callbacks never fired), while this exact JS
// WebviewWindow construction works every time. So when the agent needs the
// browser workspace and none exists, the backend emits
// 'unoone:ensure-browser-workspace' and this code opens the window.
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';

export const BROWSER_WORKSPACE_LABEL = 'browser-workspace';

/** Create (or focus) the browser-workspace window. Resolves false on error. */
export async function ensureBrowserWorkspaceWindow(): Promise<boolean> {
  try {
    const existing = await WebviewWindow.getByLabel(BROWSER_WORKSPACE_LABEL).catch(() => null);
    if (existing) {
      await existing.setFocus().catch(() => {});
      return true;
    }
    // Same options as BrowserWorkspace.openSession — this exact shape is the
    // live-verified path; do not deviate from it.
    const webview = new WebviewWindow(BROWSER_WORKSPACE_LABEL, {
      url: 'about:blank',
      width: 1280,
      height: 800,
      title: 'Browser Workspace',
      center: true,
    });
    return await new Promise<boolean>(resolve => {
      webview.once('tauri://error', () => resolve(false));
      webview.once('tauri://created', () => {
        void webview.setFocus().catch(() => {});
        resolve(true);
      });
    });
  } catch {
    return false;
  }
}