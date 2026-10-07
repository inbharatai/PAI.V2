// Stage 5 managed-preview window (design §6.5). Opened ONLY from a user click
// in the Coding Task view, on the host bridge's capability URL
// (`http://127.0.0.1:<port>/__pai/open?t=<128-bit hex>`), which is
// re-validated here before any window is created.
//
// The window gets NO Tauri capabilities: capabilities/default.json scopes
// every permission to the `main` window, and Tauri 2 exposes no IPC to remote
// origins without a `remote.urls` capability. What it shows is the user's own
// manual observation; UnoOne's automated evidence stays HTTP-level only.
//
// Construction mirrors the live-proven previewWindow.ts / browserWorkspaceWindow.ts
// shape (JS-side WebviewWindow; a Rust-built window never started WebView2,
// defect #40). A second open for a different URL closes and recreates the
// window, because JS cannot navigate another window.
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { validatePreviewUrl } from './codingTask';

export const TASK_PREVIEW_WINDOW_LABEL = 'task-preview';

/** Open (or retarget) the task preview window. Resolves false on any error or invalid URL. */
export async function openTaskPreviewWindow(capabilityUrl: string): Promise<boolean> {
  const checked = validatePreviewUrl(capabilityUrl);
  if (!checked.ok) return false;
  try {
    const existing = await WebviewWindow.getByLabel(TASK_PREVIEW_WINDOW_LABEL).catch(() => null);
    if (existing) {
      await new Promise<void>(resolve => {
        let settled = false;
        let unlisten: (() => void) | undefined;
        const finish = () => {
          if (settled) return;
          settled = true;
          window.clearTimeout(timer);
          unlisten?.();
          resolve();
        };
        const timer = window.setTimeout(finish, 1000);
        void existing.once('tauri://destroyed', finish)
          .then(fn => { if (settled) fn(); else unlisten = fn; })
          .catch(() => undefined);
        existing.close().catch(finish);
      });
    }
    const webview = new WebviewWindow(TASK_PREVIEW_WINDOW_LABEL, {
      url: checked.url,
      width: 1100,
      height: 760,
      title: 'Task Preview — HTTP-level checks only, not browser-verified by UnoOne',
      center: true,
    });
    return await new Promise<boolean>(resolve => {
      void webview.once('tauri://error', () => resolve(false));
      void webview.once('tauri://created', () => {
        void webview.setFocus().catch(() => {});
        resolve(true);
      });
    });
  } catch {
    return false;
  }
}
