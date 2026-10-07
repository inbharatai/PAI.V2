import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real repo React/ReactDOM/esbuild and Tauri bindings, not copied component logic.
// Only JSDOM lives in an explicitly supplied evidence-only tool root.
export const frontendRoot = fileURLToPath(new URL('../', import.meta.url));
const repoRequire = createRequire(join(frontendRoot, 'package.json'));
const toolRoot = process.env.CHAT_VIEW_TEST_TOOL_ROOT;
if (!toolRoot) {
  throw new Error('Set CHAT_VIEW_TEST_TOOL_ROOT to a separate tool root containing jsdom@26.1.0; see tests/chat-view-README.md.');
}
const toolRequire = createRequire(join(resolve(toolRoot), 'package.json'));
const { JSDOM } = toolRequire('jsdom');
assert.equal(toolRequire('jsdom/package.json').version, '26.1.0', 'test DOM version is pinned');
export const React = repoRequire('react');
assert.match(React.version, /^19\./, 'use the installed React 19');
export const { act } = React;
const { mockIPC, clearMocks } = repoRequire('@tauri-apps/api/mocks');
const { emit } = repoRequire('@tauri-apps/api/event');
const { build } = repoRequire('esbuild');
const bundleDir = mkdtempSync(join(tmpdir(), 'unoone-chat-view-'));
const bundlePath = join(bundleDir, 'ChatView.cjs');
const built = await build({
  entryPoints: [join(frontendRoot, 'src/components/ChatView.tsx')],
  outfile: bundlePath,
  bundle: true,
  platform: 'node',
  format: 'cjs',
  jsx: 'automatic',
  sourcemap: 'inline',
  metafile: true,
  plugins: [{
    name: 'one-installed-react',
    setup(builder) {
      // Bundle the real component, selector and Tauri API, but share the exact
      // same React instance as ReactDOM and act (no synthetic hooks/modules).
      builder.onResolve({ filter: /^react(?:\/.*)?$/ }, ({ path }) => ({
        path: repoRequire.resolve(path), external: true,
      }));
    },
  }],
});
const { ChatView } = repoRequire(bundlePath);
console.log('[mounted ChatView test environment]', JSON.stringify({
  node: process.version, react: React.version,
  reactDOM: repoRequire('react-dom/package.json').version,
  esbuild: repoRequire('esbuild/package.json').version,
  tauri: repoRequire('@tauri-apps/api/package.json').version,
  jsdom: toolRequire('jsdom/package.json').version,
  toolRoot: resolve(toolRoot),
  productionInputs: Object.keys(built.metafile.inputs).filter(path => /ChatView\.tsx|chatContext\.ts|lib\/tauri\.ts/.test(path)),
}));
process.on('exit', () => rmSync(bundleDir, { recursive: true, force: true }));

export function deferred() {
  let resolvePromise, reject;
  const promise = new Promise((resolve, rejectPromise) => {
    resolvePromise = resolve;
    reject = rejectPromise;
  });
  return { promise, resolve: resolvePromise, reject };
}
export const reply = output => ({
  output, route: 'L1', steps: 1, tool_calls: 0, elapsed_ms: 1, context_note: null,
});
export function turn(user_message, assistant_message, session_id = 'archive-small') {
  return { kind: 'chat_turn', schema: 1, session_id, user_message, assistant_message, timestamp: '2026-09-01T12:00:00Z' };
}
export const archive = [
  turn('Create the Small Giant knowledge-base plan', 'Small Giant: distillery, reviewed learning queue, and learning backlog.'),
  turn('yes, continue', 'Small Giant: the distillery now has a review queue.'),
  turn('Plan a garden irrigation controller', 'The garden needs moisture sensors and a watering schedule.', 'archive-garden'),
];
export const archiveHistory = archive.slice(0, 2).flatMap(turn => [
  { role: 'user', content: turn.user_message },
  { role: 'assistant', content: turn.assistant_message },
]);
export const note = (explanation, count) => `Context: ${explanation} (${count} messages). Visible history and encrypted records are unchanged.`;
export const explanations = {
  greeting: 'standalone greeting; no prior task history sent',
  newTask: 'new task boundary; no prior task history sent',
  named: 'explicit named continuation; only one matching archived task is in context',
  active: 'within-task followup; only the active task is in context',
  notFound: 'no clear archived task name match; no archive sent — specify the task name',
  ambiguous: 'ambiguous archived task name; no archive sent — clarify the task/session',
};

const globalNames = [
  'window', 'document', 'navigator', 'localStorage', 'CustomEvent', 'Event',
  'MouseEvent', 'KeyboardEvent', 'HTMLElement', 'HTMLTextAreaElement', 'File',
  'FileReader', 'IS_REACT_ACT_ENVIRONMENT',
];

export async function mountChat(t, { turns = archive, recall, harness, legacy, commands = {}, fullAccess = true, strict = false } = {}) {
  const dom = new JSDOM('<!doctype html><html><body><div id="mount"></div></body></html>', { url: 'https://chat-view.test.invalid/' });
  const oldGlobals = new Map(globalNames.map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const key of globalNames) {
    Object.defineProperty(globalThis, key, { value: key === 'IS_REACT_ACT_ENVIRONMENT' ? true : dom.window[key], configurable: true, writable: true });
  }
  dom.window.HTMLElement.prototype.scrollIntoView = function (options) {
    scrollCalls.push({ element: this, options });
  }; // JSDOM lacks layout/scrollIntoView; this is a spy, not layout evidence.
  dom.window.localStorage.setItem('unoone.fullAccess', fullAccess ? 'on' : 'off');
  dom.window.localStorage.setItem('unoone.autoSpeak', 'off');
  const calls = [], unexpected = [], scrollCalls = [], activity = [];
  dom.window.addEventListener('unoone:agent-activity', event => activity.push(event.detail.active));
  mockIPC((command, args) => {
    calls.push({ command, args: args === undefined ? undefined : structuredClone(args) });
    if (Object.hasOwn(commands, command)) return commands[command](args);
    switch (command) {
      case 'check_model_health': return { status: 'ok' }; // Explicit health double; no model claim.
      case 'detect_vault': return { detected: false, vault_root: '', validation_failures: [] };
      case 'get_workspace_root': return '/mock/granted-workspace';
      case 'recall_chat_memory': return recall ? recall(args) : { turns };
      case 'harness_chat': return harness ? harness(args) : reply('Mounted mock answer.');
      case 'agent_chat': return legacy ? legacy(args) : { final_text: 'Legacy mock answer.', steps: [], context_note: null };
      case 'save_chat_turn': return 'mock-message-record';
      case 'harness_stop_run': return true;
      default:
        unexpected.push(command);
        throw new Error(`Unexpected mock IPC command: ${command}`);
    }
  }, { shouldMockEvents: true });
  // Import ReactDOM only AFTER a DOM exists so its event system sees browser APIs.
  const { createRoot } = repoRequire('react-dom/client');
  const container = dom.window.document.getElementById('mount');
  const root = createRoot(container);
  let mounted = true;
  async function flush() { await act(async () => { await new Promise(resolve => setImmediate(resolve)); }); }
  async function unmount() {
    if (!mounted) return;
    await act(async () => root.unmount());
    await flush();
    mounted = false;
  }
  t.after(async () => {
    await unmount();
    assert.deepEqual(unexpected, [], 'all commands must have explicit test doubles');
    assert.equal(dom.window.__TAURI_INTERNALS__.callbacks.size, 0, 'Tauri listeners clean up at unmount');
    assert.equal(calls.filter(call => /lock_vault|unlock_vault|vault_write_record|delete.*record/.test(call.command)).length, 0, 'ChatView does not change vault lock/canonical record policy');
    clearMocks();
    dom.window.close();
    for (const [key, descriptor] of oldGlobals) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  await act(async () => {
    root.render(strict ? React.createElement(React.StrictMode, null, React.createElement(ChatView)) : React.createElement(ChatView));
  });
  await flush();
  const textarea = () => container.querySelector('textarea.chat-input');
  const sendButton = () => container.querySelector('.chat-input-row .btn-primary');
  async function type(text) {
    const input = textarea();
    assert.ok(input && !input.disabled, 'composer is available');
    await act(async () => {
      // Native setter avoids React's per-element value tracker; dispatch a real DOM event.
      Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(input, text);
      input.dispatchEvent(new dom.window.Event('input', { bubbles: true }));
    });
    assert.equal(input.value, text);
  }
  async function click(element) {
    assert.ok(element, 'click target exists');
    await act(async () => element.dispatchEvent(new dom.window.MouseEvent('click', { bubbles: true })));
    await flush();
  }
  async function send(text, { enter = false } = {}) {
    await type(text);
    assert.equal(sendButton().disabled, false, 'nonempty send is enabled');
    if (enter) {
      await act(async () => textarea().dispatchEvent(new dom.window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })));
      await flush();
    } else await click(sendButton());
  }
  async function attach(files) {
    const input = container.querySelector('input[type="file"]');
    // JSDOM has no OS file chooser/DataTransfer. Supply real JSDOM Files and
    // dispatch the same change event; production FileReader/parsers still run.
    Object.defineProperty(input, 'files', { configurable: true, value: files });
    await act(async () => input.dispatchEvent(new dom.window.Event('change', { bubbles: true })));
    const deadline = Date.now() + 2000;
    const prior = container.querySelectorAll('button[title="Remove attachment"], button[title="Remove image"]').length;
    do {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)); });
      if (container.querySelectorAll('button[title="Remove attachment"], button[title="Remove image"]').length >= prior + files.length) return;
    } while (Date.now() < deadline);
    throw new Error('Attachment processing did not finish');
  }
  const messages = () => [...container.querySelectorAll('.chat-message')].map(element => ({
    role: element.classList.contains('user') ? 'user' : 'assistant',
    content: element.querySelector('.chat-bubble > div').textContent,
  }));
  return {
    dom, container, calls, scrollCalls, activity, textarea, sendButton,
    flush, unmount, type, click, send, attach, messages,
    text: () => container.textContent,
    status: () => container.querySelector('[role="status"]')?.textContent ?? '',
    ipc: command => calls.filter(call => call.command === command).map(call => call.args),
    emit: async (event, payload) => { await act(async () => emit(event, payload)); await flush(); },
    settle: async (pending, value, rejected = false) => { await act(async () => rejected ? pending.reject(value) : pending.resolve(value)); await flush(); },
  };
}
