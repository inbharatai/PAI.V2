// The agent's live website preview window (web.preview) — created through
// the SAME proven JS path as the browser workspace. Live-caught 2026-09-15
// (defect #40): a window built from Rust never started its WebView2 content
// process, so the backend only EMITS 'unoone:ensure-preview-window' with the
// staged mirror entry path and this code opens the window on the
// asset-protocol URL. The preview window carries NO Tauri capabilities
// (capabilities/default.json scopes every permission to the main window),
// so scripts inside the previewed page can never touch the IPC surface.
//
// A later event for a DIFFERENT entry retargets the preview: JS cannot
// navigate another window, so the old window is closed (waiting for its
// 'tauri://destroyed' event, bounded) and a fresh one opens on the new URL.
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { convertFileSrc } from '@tauri-apps/api/core';

export const PREVIEW_WINDOW_LABEL = 'agent-preview';

/** Create (or retarget) the live preview window. Resolves false on error. */
export async function ensurePreviewWindow(entryPath: string): Promise<boolean> {
  try {
    const url = convertFileSrc(entryPath);
    const existing = await WebviewWindow.getByLabel(PREVIEW_WINDOW_LABEL).catch(() => null);
    if (existing) {
      // Retarget = close + recreate (JS has no cross-window navigation).
      await new Promise<void>(resolve => {
        const timer = window.setTimeout(resolve, 1000);
        existing.once('tauri://destroyed', () => {
          window.clearTimeout(timer);
          resolve();
        });
        existing.close().catch(() => resolve());
      });
    }
    // Same shape as the proven browser-workspace construction; do not
    // deviate from it.
    const webview = new WebviewWindow(PREVIEW_WINDOW_LABEL, {
      url,
      width: 1100,
      height: 760,
      title: 'Website Preview',
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