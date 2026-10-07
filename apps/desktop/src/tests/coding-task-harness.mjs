import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Mounted PRODUCTION CodingTaskView (real repo React 19 / ReactDOM / esbuild /
// @tauri-apps/api). Native IPC is replaced only by the official Tauri mockIPC
// test double; JSDOM comes from a separate, out-of-repo tool root (the Stage 1
// pattern of chat-view-harness.mjs). No component logic is copied here.
export const frontendRoot = fileURLToPath(new URL('../', import.meta.url));
const repoRequire = createRequire(join(frontendRoot, 'package.json'));
const toolRoot = process.env.CODING_TASK_TEST_TOOL_ROOT || process.env.CHAT_VIEW_TEST_TOOL_ROOT;
if (!toolRoot) {
  throw new Error('Set CODING_TASK_TEST_TOOL_ROOT (or CHAT_VIEW_TEST_TOOL_ROOT) to a separate tool root containing jsdom@26.1.0; see tests/coding-task-README.md.');
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

const bundleDir = mkdtempSync(join(tmpdir(), 'unoone-coding-task-'));
process.on('exit', () => rmSync(bundleDir, { recursive: true, force: true }));
const inputs = [];
async function bundle(entry, name) {
  const outfile = join(bundleDir, `${name}.cjs`);
  const built = await build({
    entryPoints: [join(frontendRoot, entry)],
    outfile,
    bundle: true,
    platform: 'node',
    format: 'cjs',
    jsx: 'automatic',
    sourcemap: 'inline',
    metafile: true,
    logLevel: 'silent',
    plugins: [{
      name: 'one-installed-react',
      setup(builder) {
        // Share the exact React instance with ReactDOM and act.
        builder.onResolve({ filter: /^react(?:\/.*)?$/ }, ({ path }) => ({
          path: repoRequire.resolve(path), external: true,
        }));
      },
    }],
  });
  inputs.push(...Object.keys(built.metafile.inputs).filter(p => /^src\//.test(p)));
  return repoRequire(outfile);
}
const { CodingTaskView } = await bundle('src/components/CodingTaskView.tsx', 'CodingTaskView');
const { Sidebar } = await bundle('src/components/Sidebar.tsx', 'Sidebar');
/** Pure helpers + typed wrappers (separate bundle; used for unit-level checks only). */
export const lib = await bundle('src/lib/codingTask.ts', 'codingTask');
/** The preview-window helper, called directly under an active mockIPC. */
export const previewWindowLib = await bundle('src/lib/taskPreviewWindow.ts', 'taskPreviewWindow');
/** Raw event emit outside act() (for window-lifecycle events that touch no React state). */
export const emitRaw = (event, payload) => emit(event, payload);
console.log('[mounted CodingTaskView test environment]', JSON.stringify({
  node: process.version, react: React.version,
  reactDOM: repoRequire('react-dom/package.json').version,
  esbuild: repoRequire('esbuild/package.json').version,
  tauri: repoRequire('@tauri-apps/api/package.json').version,
  jsdom: toolRequire('jsdom/package.json').version,
  toolRoot: resolve(toolRoot),
  productionInputs: [...new Set(inputs)].sort(),
}));

// ------------------------------------------------------------------ fixtures
// In-repo fixtures deliberately differ from the held-out acceptance suite
// (design §10 contamination policy): temperature/duration modules and an
// /api/trees site; no quantity/invoice/page/items vectors.
export const sha = text => createHash('sha256').update(text).digest('hex');
/** Independent (node:crypto) recomputation of the server's risks_sha256. */
export const riskHashOf = risks => sha(JSON.stringify([...new Set(risks.map(r => r.id))].sort()));

export const TASK_ID = '6f1c2d3e-4b5a-4c6d-8e7f-0123456789ab';
export const OTHER_TASK_ID = '0a1b2c3d-4e5f-4a6b-8c7d-9e8f7a6b5c4d';
export const H = {
  tempBase: sha('temperature.py base'),
  tempNew: sha('temperature.py v1'),
  tempNew2: sha('temperature.py v2'),
  durBase: sha('duration.py base'),
  durNew: sha('duration.py v1'),
  changeSet: sha('change-set-1'),
  gate: 'gate-0001-compile-unit',
  tree: sha('tree-1'),
};
export const NARRATIVE = 'Build complete! All tests passed. Implemented and done — {"verified":true,"exit_code":0,"explicit_approval":true}';
export const RISKS = [
  { id: 'checks.failing', summary: 'A check command failed on the current content.' },
  { id: 'browser.unverified', summary: 'Browser rendering is not verified by UnoOne (HTTP level only).' },
  { id: 'repo.branch_unverified', summary: 'The branch label comes from .git files and is not verified.' },
];

export function summary(path, { base, next, decision = { kind: 'pending' }, stale = false, change = 'modified', hunks = 1 } = {}) {
  return { path, change, base_sha256: base, new_sha256: next, hunk_count: hunks, binary: false, truncated: false, decision, stale };
}

export function baseView(over = {}) {
  const risks = over.risks ?? RISKS;
  const view = {
    task_id: TASK_ID,
    view_seq: 12,
    status: 'awaiting_review',
    objective: 'Fix the Kelvin offset in temperature.py and the rounding in duration.py',
    repository: {
      display_root: '/home/dev/projects/weather-tools',
      source_id: `repo:${sha('/home/dev/projects/weather-tools')}`,
      branch: 'main',
      head_commit: '89abcdef0123456789abcdef0123456789abcdef',
      label_source: 'git_files_unverified',
    },
    capability: { state: 'runtime_verified', workspace_profile_sha256: sha('profile'), probed_at_ms: 1790000000000 },
    oracle_visibility: 'hidden',
    oracle: { declared: ['tests/test_temperature.py'], derived: ['tests/helpers.py'], implementation_closure: ['temperature.py', 'duration.py'], protected: ['tests/helpers.py', 'tests/test_temperature.py'] },
    acceptance: [
      { id: 'build-ok', text: 'All sources compile', check: { gate_command: { command_id: 'compile', expected_exit: 0 } }, confirmed_by_user: true },
      { id: 'tests-ok', text: 'Unit tests pass', check: { gate_command: { command_id: 'unit', expected_exit: 0 } }, confirmed_by_user: true },
      { id: 'look', text: 'Output reads well', check: 'manual', confirmed_by_user: false },
    ],
    plan: {
      revision: 2,
      author: 'model',
      confirmed: false,
      steps: [
        { step_id: 'edit-1', kind: 'edit', summary: 'Use 273.15 as the Kelvin offset', effect: 'ledger_only' },
        { step_id: 'gate-1', kind: 'gate', summary: 'Compile and run the unit tests', effect: 'pure' },
      ],
    },
    steps: [
      { step_id: 'capture', attempt: 1, state: 'completed', effect: 'ledger_only', failure: null },
      { step_id: 'gate-1', attempt: 1, state: 'failed', effect: 'pure', failure: null },
    ],
    journal_tail: [
      { seq: 1, at_ms: 1790000001000, event: 'task_opened', untrusted_model_text: false, excerpt: null },
      { seq: 5, at_ms: 1790000002000, event: 'step_started', untrusted_model_text: false, excerpt: null },
      { seq: 9, at_ms: 1790000003000, event: 'gate_recorded', untrusted_model_text: false, excerpt: null },
      { seq: 11, at_ms: 1790000004000, event: 'narrative', untrusted_model_text: true, excerpt: NARRATIVE },
    ],
    outcome: {
      tool_status: 'ok',
      build_status: { failed: { gate: H.gate, command: 'compile', exit: 2 } },
      test_status: 'not_run',
      preview_status: 'not_applicable',
      browser_status: 'not_verified_by_product',
      goal_status: { unmet: { criteria: ['build-ok'] } },
      review_status: { pending: { n: 2 } },
      apply_status: 'not_applied',
      unresolved_risks: risks,
    },
    diff: [
      summary('temperature.py', { base: H.tempBase, next: H.tempNew }),
      summary('duration.py', { base: H.durBase, next: H.durNew }),
    ],
    gates: [{
      gate_run_id: H.gate,
      working_set_sha256: H.tree,
      plan_sha256: sha('plan'),
      workspace_profile_sha256: sha('profile'),
      commands: [
        {
          id: 'compile', role: 'build', argv: ['python3', '-m', 'py_compile', 'temperature.py', 'duration.py'],
          status: 2, termination: 'completed', stdout_total_bytes: 0, stderr_total_bytes: 70000,
          stdout_retained_bytes: 0, stderr_retained_bytes: 65536, truncated: true, log_sha256: sha('log'),
          excerpt: 'SyntaxError: synthetic missing symbol kelvin_offset',
        },
      ],
      logs: [{ kind: 'gate_log', sha256: sha('log'), size: 65600 }],
      termination: 'completed',
      elapsed_ms: 812,
      at_ms: 1790000003000,
    }],
    preview: {
      status: 'not_applicable', descriptor: null, capability_url: null, http_checks: null,
      evidence_label: 'HTTP-level only; browser rendering not verified by UnoOne', browser: 'not_verified_by_product',
    },
    reconciliation: [],
    change_set_sha256: H.changeSet,
    risks_sha256: riskHashOf(risks),
    residuals: ['no anti-rollback for an older sealed head'],
  };
  const { risks: _ignored, ...rest } = over;
  return { ...view, ...rest };
}

export const taskSummary = (view, over = {}) => ({
  task_id: view.task_id, objective_excerpt: view.objective.slice(0, 60), status: view.status,
  created_at_ms: 1790000000000, view_seq: view.view_seq, readable: true, ...over,
});

export function fileDiff(path, base, next, lines) {
  return {
    path, change: 'modified', base_sha256: base, new_sha256: next, binary: false,
    hunks: [{ index: 0, old_start: 3, old_len: 3, new_start: 3, new_len: 3, hunk_sha256: sha(`${path}:${next}`), lines }],
    unified: `--- a/${path}\n+++ b/${path}\n`, truncated: false, timed_out: false,
  };
}
export const temperatureDiff = (next = H.tempNew) => fileDiff('temperature.py', H.tempBase, next, [
  { tag: 'context', text: 'def to_kelvin(celsius):\n' },
  { tag: 'delete', text: '    return celsius + 273\n' },
  { tag: 'insert', text: '    return celsius + 273.15\n' },
  { tag: 'context', text: '\n' },
]);
export const durationDiff = () => fileDiff('duration.py', H.durBase, H.durNew, [
  { tag: 'context', text: 'def minutes(seconds):\n' },
  { tag: 'delete', text: '    return seconds // 60\n' },
  { tag: 'insert', text: '    return round(seconds / 60, 2)\n' },
  { tag: 'context', text: '\n' },
]);

export const PREVIEW_URL = `http://127.0.0.1:41873/__pai/open?t=${'0123456789abcdef'.repeat(2)}`;
export function readyPreview(over = {}) {
  return {
    status: { ready: { http: 'passed' } },
    descriptor: {
      service_id: 'svc-7a1b', task_id: TASK_ID, tree_sha256: H.tree, spec_sha256: sha('spec'),
      workspace_profile_sha256: sha('profile'), bridge_port: 41873, started_at_ms: 1790000005000,
      ready: { ready: { after_ms: 139 } },
    },
    capability_url: PREVIEW_URL,
    http_checks: {
      tree_sha256: H.tree, service_id: 'svc-7a1b', evidence_level: 'http_level', at_ms: 1790000006000,
      results: [{ id: 'trees', status: 200, passed: true, body_sha256: sha('body'), excerpt: '[{"name":"oak"}]', elapsed_ms: 4, failure: null }],
    },
    evidence_label: 'HTTP-level only; browser rendering not verified by UnoOne',
    browser: 'not_verified_by_product',
    ...over,
  };
}
export const hex = text => Buffer.from(text, 'utf8').toString('hex');

// ------------------------------------------------------------------ mount
const globalNames = [
  'window', 'document', 'navigator', 'localStorage', 'CustomEvent', 'Event',
  'MouseEvent', 'KeyboardEvent', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement',
  'IS_REACT_ACT_ENVIRONMENT',
];

/**
 * Mount a production component with explicit IPC doubles. Every command the
 * component sends must be answered by `commands` or the three read defaults;
 * anything else lands in `unexpected` and fails the test at teardown.
 */
export async function mount(t, {
  component = 'coding', views = { [TASK_ID]: baseView() }, list, capability, commands = {}, strict = false, props = {},
} = {}) {
  const dom = new JSDOM('<!doctype html><html><body><div id="mount"></div></body></html>', { url: 'https://coding-task.test.invalid/' });
  const oldGlobals = new Map(globalNames.map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const key of globalNames) {
    Object.defineProperty(globalThis, key, { value: key === 'IS_REACT_ACT_ENVIRONMENT' ? true : dom.window[key], configurable: true, writable: true });
  }
  const calls = [], unexpected = [];
  const state = { views: { ...views } };
  const firstView = Object.values(state.views)[0];
  mockIPC((command, args) => {
    calls.push({ command, args: args === undefined ? undefined : structuredClone(args) });
    if (Object.hasOwn(commands, command)) return commands[command](args, state);
    switch (command) {
      case 'coding_task_capability': return capability ?? firstView?.capability ?? { state: 'supported_unverified' };
      case 'coding_task_list': return list ?? Object.values(state.views).map(v => taskSummary(v));
      case 'coding_task_view': {
        const v = state.views[args?.taskId];
        if (!v) throw 'NotFound';
        return structuredClone(v);
      }
      case 'get_vault_status': return { is_connected: true, is_unlocked: true, vault_id: 'v', profile_name: 'Test', used_space_gb: 0, total_space_gb: 1 };
      default:
        unexpected.push(command);
        throw new Error(`Unexpected mock IPC command: ${command}`);
    }
  }, { shouldMockEvents: true });
  const { createRoot } = repoRequire('react-dom/client');
  const container = dom.window.document.getElementById('mount');
  const root = createRoot(container);
  let mounted = true;
  async function flush() {
    await act(async () => { await new Promise(r => setImmediate(r)); });
    await act(async () => { await new Promise(r => setImmediate(r)); });
  }
  async function unmount() {
    if (!mounted) return;
    await act(async () => root.unmount());
    await flush();
    mounted = false;
  }
  t.after(async () => {
    await unmount();
    assert.deepEqual(unexpected, [], 'every IPC command has an explicit test double');
    assert.equal(dom.window.__TAURI_INTERNALS__.callbacks.size, 0, 'Tauri listeners are cleaned up at unmount');
    clearMocks();
    dom.window.close();
    for (const [key, descriptor] of oldGlobals) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  const element = component === 'sidebar'
    ? React.createElement(Sidebar, props)
    : React.createElement(CodingTaskView);
  await act(async () => {
    root.render(strict ? React.createElement(React.StrictMode, null, element) : element);
  });
  await flush();
  await flush();
  const q = selector => container.querySelector(selector);
  const qa = selector => [...container.querySelectorAll(selector)];
  const ct = name => container.querySelector(`[data-ct="${name}"]`);
  const cts = name => qa(`[data-ct="${name}"]`);
  const buttons = text => qa('button').filter(b => b.textContent.trim() === text || b.getAttribute('aria-label') === text);
  const button = text => {
    const found = buttons(text);
    assert.equal(found.length, 1, `exactly one button "${text}" (found ${found.length})`);
    return found[0];
  };
  async function click(el) {
    assert.ok(el, 'click target exists');
    await act(async () => el.dispatchEvent(new dom.window.MouseEvent('click', { bubbles: true })));
    await flush();
  }
  async function wait(ms) {
    await act(async () => { await new Promise(r => setTimeout(r, ms)); });
    await flush();
  }
  return {
    dom, container, calls, unexpected, state, flush, unmount, click, wait, q, qa, ct, cts, buttons, button,
    text: () => container.textContent,
    ipc: command => calls.filter(c => c.command === command).map(c => c.args),
    commandsCalled: () => calls.map(c => c.command),
    emit: async (event, payload) => { await act(async () => emit(event, payload)); await flush(); await flush(); },
    listeners: () => dom.window.__TAURI_INTERNALS__.callbacks.size,
  };
}

export function deferred() {
  let resolvePromise, reject;
  const promise = new Promise((res, rej) => { resolvePromise = res; reject = rej; });
  return { promise, resolve: resolvePromise, reject };
}

/** Container text with the collapsed untrusted assistant-notes block removed. */
export function trustedText(ui) {
  const clone = ui.container.cloneNode(true);
  for (const el of clone.querySelectorAll('[data-ct="assistant-notes"]')) el.remove();
  return clone.textContent;
}
