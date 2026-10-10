import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Same real React/esbuild + official mockIPC + external pinned JSDOM pattern
// as knowledge-harness.mjs. Run sequentially (global DOM).
const frontend = fileURLToPath(new URL('../', import.meta.url));
const require = createRequire(join(frontend, 'package.json'));
const toolRoot = process.env.READINESS_TEST_TOOL_ROOT || process.env.KNOWLEDGE_TEST_TOOL_ROOT;
if (!toolRoot) throw new Error('Set READINESS_TEST_TOOL_ROOT to an external tool root containing jsdom@26.1.0 (see existing knowledge-README.md).');
const tools = createRequire(join(resolve(toolRoot), 'package.json'));
const { JSDOM } = tools('jsdom');
assert.equal(tools('jsdom/package.json').version, '26.1.0');
const React = require('react');
const { act } = React;
const { build } = require('esbuild');
const { mockIPC, clearMocks } = require('@tauri-apps/api/mocks');
const dir = mkdtempSync(join(tmpdir(), 'unoone-readiness-'));
process.on('exit', () => rmSync(dir, { recursive: true, force: true }));
async function bundle(name) {
  const outfile = join(dir, `${name}.cjs`);
  await build({ entryPoints: [join(frontend, `src/components/${name}.tsx`)], outfile, bundle: true, platform: 'node', format: 'cjs', jsx: 'automatic', plugins: [{ name: 'real-react', setup(builder) {
    builder.onResolve({ filter: /^react(?:\/.*)?$/ }, ({ path }) => ({ path: require.resolve(path), external: true }));
  } }] });
  return require(outfile)[name];
}
const ModelManager = await bundle('ModelManager');
const DocumentsView = await bundle('DocumentsView');
const model = (path, available = true) => ({ path, name: path, available, quantization: 'manifest-verified', context_length: 4096, context_verified: true, file_size_gb: 2 });
const config = { model_path: '/one.gguf', context_size: 4096, batch_size: 512, threads: 4, gpu_layers: -1, temperature: 0.5, top_p: 0.9, top_k: 40, repeat_penalty: 1.1, max_tokens: 1024 };
async function mount(t, commands = {}, Component = ModelManager) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'https://readiness.test.invalid/' });
  const keys = ['window', 'document', 'navigator', 'Event', 'MouseEvent', 'HTMLElement', 'IS_REACT_ACT_ENVIRONMENT'];
  const old = new Map(keys.map(k => [k, Object.getOwnPropertyDescriptor(globalThis, k)]));
  for (const key of keys) Object.defineProperty(globalThis, key, { configurable: true, writable: true, value: key === 'IS_REACT_ACT_ENVIRONMENT' ? true : dom.window[key] });
  const calls = [], unexpected = [];
  const defaults = {
    detect_vault: () => ({ detected: true, vault_root: '/existing/vault' }),
    list_models: () => [model('/one.gguf'), model('/two.gguf', false)],
    detect_acceleration: () => ['CPU'], get_model_status: () => 'LOADED',
    get_model_config: () => ({ ...config }), get_security_level: () => 'STANDARD',
    model_cache_status: () => ({ staged: false, cached_path: null, sha256: 'expected' }),
    get_context_budget: () => ({ native_context: 4096, granted_context: 4096, reasons: [], kv_estimate_bytes: null }),
  };
  mockIPC((command, args) => {
    calls.push({ command, args });
    const run = commands[command] || defaults[command];
    if (!run) { unexpected.push(command); throw new Error(`Unexpected IPC ${command}`); }
    return run(args);
  });
  const root = require('react-dom/client').createRoot(dom.window.document.getElementById('root'));
  const flush = async () => { for (let i = 0; i < 3; i++) await act(async () => { await new Promise(resolve => setImmediate(resolve)); }); };
  t.after(async () => {
    await act(async () => root.unmount()); clearMocks(); dom.window.close();
    for (const [key, value] of old) { if (value) Object.defineProperty(globalThis, key, value); else delete globalThis[key]; }
    assert.deepEqual(unexpected, []);
  });
  await act(async () => root.render(React.createElement(Component)));
  await flush();
  const container = dom.window.document.getElementById('root');
  return { container, calls, flush, text: () => container.textContent,
    button: text => [...container.querySelectorAll('button')].find(el => el.textContent === text),
    click: async el => { assert.ok(el); await act(async () => el.dispatchEvent(new dom.window.MouseEvent('click', { bubbles: true }))); await flush(); },
  };
}

test('initial readiness performs reads only; selecting a missing asset is not loading it', async t => {
  const h = await mount(t);
  assert.match(h.text(), /Selected model loaded/);
  assert.doesNotMatch(h.text(), /manifest-verified/);
  await h.click([...h.container.querySelectorAll('.recording-item')][1]);
  assert.match(h.text(), /Missing on disk/);
  assert.match(h.text(), /Another or unidentified model loaded/);
  await h.click(h.button('Refresh observations'));
  assert.match(h.text(), /Selection: \/two.gguf/);
  assert.ok(h.calls.every(c => ['detect_vault', 'list_models', 'detect_acceleration', 'get_model_status', 'get_model_config', 'get_security_level', 'model_cache_status', 'get_context_budget'].includes(c.command)));
  for (const anchor of h.container.querySelectorAll('a')) assert.ok(h.container.querySelector(anchor.getAttribute('href')));
});
test('Check Health does not replace or hide configuration and controls', async t => {
  const h = await mount(t, { check_model_health: () => ({ model_id: 'real-response-shape' }) });
  await h.click(h.button('Check Health'));
  assert.match(h.text(), /Health response \(not workflow qualification\)/);
  assert.ok(h.button('Load Model'));
  assert.ok(h.container.querySelector('#model-configuration'));
});
test('a cache staging response for a previous selection cannot label the new selection verified', async t => {
  let resolveStage;
  const h = await mount(t, { stage_model_cache: () => new Promise(resolve => { resolveStage = resolve; }) });
  await h.click(h.button('Stage to host cache'));
  await h.click([...h.container.querySelectorAll('.recording-item')][1]);
  await act(async () => resolveStage({ staged: true, cached_path: '/cached/one.gguf', size_bytes: 1 }));
  await h.flush();
  assert.match(h.text(), /Selection: \/two.gguf/);
  assert.doesNotMatch(h.text(), /Verified cache marker present/);
});
test('cache probe failures remain unknown and errors do not hide advanced controls', async t => {
  const h = await mount(t, { model_cache_status: () => { throw new Error('no manifest'); }, check_model_health: () => { throw new Error('offline'); } });
  assert.match(h.text(), /Unknown \/ cache probe unavailable/);
  await h.click(h.button('Check Health'));
  assert.ok(h.button('Load Model'));
  assert.ok(h.container.querySelector('#model-configuration'));
});
test('source metadata renders as text, not markup or a verified-content badge', async t => {
  const id = '<img src=x onerror=alert(1)>';
  const h = await mount(t, { list_documents: () => [{ id, title: 'Existing source', document_type: 'PDF', file_size_bytes: 30, source_platform: 'ANDROID', word_count: 0, page_count: null }] }, DocumentsView);
  assert.match(h.text(), /Recorded words: 0/);
  assert.ok(h.text().includes(id));
  assert.equal(h.container.querySelectorAll('img').length, 0);
  assert.match(h.text(), /Metadata is not proof/);
  assert.match(h.text(), /no OCR fallback/);
  assert.deepEqual(h.calls.map(c => c.command), ['detect_vault', 'list_documents']);
});
