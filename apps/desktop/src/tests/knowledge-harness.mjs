import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Mounted PRODUCTION KnowledgeView, CodingTaskView (with the Stage 6 Learning
// panel), MemoryExplorer and Sidebar, built from src/ with the repo's own
// React 19 / ReactDOM / esbuild / @tauri-apps/api. Native IPC is replaced only
// by the official Tauri mockIPC test double; JSDOM comes from a separate,
// out-of-repo tool root (the Stage 1/5 harness pattern). No component logic is
// copied here.
export const frontendRoot = fileURLToPath(new URL('../', import.meta.url));
const repoRequire = createRequire(join(frontendRoot, 'package.json'));
const toolRoot = process.env.KNOWLEDGE_TEST_TOOL_ROOT || process.env.CODING_TASK_TEST_TOOL_ROOT || process.env.CHAT_VIEW_TEST_TOOL_ROOT;
if (!toolRoot) {
  throw new Error('Set KNOWLEDGE_TEST_TOOL_ROOT (or CODING_TASK_TEST_TOOL_ROOT / CHAT_VIEW_TEST_TOOL_ROOT) to a separate tool root containing jsdom@26.1.0; see tests/knowledge-README.md.');
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

const bundleDir = mkdtempSync(join(tmpdir(), 'unoone-knowledge-'));
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
        builder.onResolve({ filter: /^react(?:\/.*)?$/ }, ({ path }) => ({
          path: repoRequire.resolve(path), external: true,
        }));
      },
    }],
  });
  inputs.push(...Object.keys(built.metafile.inputs).filter(p => /^src\//.test(p)));
  return repoRequire(outfile);
}
const { KnowledgeView } = await bundle('src/components/KnowledgeView.tsx', 'KnowledgeView');
const { CodingTaskView } = await bundle('src/components/CodingTaskView.tsx', 'CodingTaskView');
const { MemoryExplorer } = await bundle('src/components/MemoryExplorer.tsx', 'MemoryExplorer');
const { Sidebar } = await bundle('src/components/Sidebar.tsx', 'Sidebar');
/** Typed wrappers + pure helpers (separate bundle; unit-level checks only). */
export const lib = await bundle('src/lib/knowledge.ts', 'knowledge');
console.log('[mounted Knowledge test environment]', JSON.stringify({
  node: process.version, react: React.version,
  reactDOM: repoRequire('react-dom/package.json').version,
  esbuild: repoRequire('esbuild/package.json').version,
  tauri: repoRequire('@tauri-apps/api/package.json').version,
  jsdom: toolRequire('jsdom/package.json').version,
  toolRoot: resolve(toolRoot),
  productionInputs: [...new Set(inputs)].sort(),
}));

/** Stage 5 TaskView fixtures (baseView, summary, H …), reused for the Learning panel. */
process.env.CODING_TASK_TEST_TOOL_ROOT ||= toolRoot;
export const codingFixtures = await import('./coding-task-harness.mjs');

// ------------------------------------------------------------------ fixtures
// Hand-written mirrors of the frozen Stage 6 serde shapes (design §1/§2).
// They deliberately differ from the held-out acceptance material: a
// weather-units guide (Kelvin offset, minute rounding); no quantity / invoice
// / page / items vectors.
export const sha = text => createHash('sha256').update(text).digest('hex');
export const HEX32 = /^[0-9a-f]{32}$/;
export const METHOD = 'extractive-headings-docstrings-v1; deterministic; no model; no network';
export const ref = (logicalId, kind, revision = 1) => ({ logical_id: logicalId, revision, kind, content_digest: sha(`${logicalId}:${revision}`) });
export const badge = (over = {}) => ({
  source_id: 'local:3f2a9c1d0b7e6a55',
  source_version: sha('units-guide.md v1'),
  source_commit: sha('units-guide.md v1'),
  file_digest: sha('units-guide.md v1'),
  license: 'CC-BY-4.0',
  privacy: 'private',
  platforms: ['linux', 'windows'],
  topics: ['weather', 'units'],
  ...over,
});
/** A snippet that would inject markup if it were ever rendered as HTML. */
export const HOSTILE = 'Use 273.15 <img src=x onerror="globalThis.__pwned=1"> for Kelvin';
export function hit(logicalId, kind, over = {}) {
  return {
    reference: ref(logicalId, kind),
    kind,
    title: `Kelvin offset — ${logicalId}`,
    snippet: 'Add 273.15 to a Celsius value to get Kelvin.',
    source: badge(),
    active: true,
    contradictory: false,
    invalidated: false,
    why_recalled: 'lexical terms: kelvin, offset',
    mode: 'historical',
    ...over,
  };
}
export function statusFixture(over = {}) {
  return {
    initialized: true,
    index: 'fresh',
    catalog_entries: 7,
    counts: { evidence: 3, candidate: 2, verified_pattern: 1, approved_procedure: 1, invalidation: 0 },
    active_counts: { evidence: 3, candidate: 2, verified_pattern: 1, approved_procedure: 1, invalidation: 0 },
    method: METHOD,
    residuals: ['extraction is deterministic and shallow (not semantic understanding)'],
    ...over,
  };
}
export const LIST_ITEMS = [
  hit('cand-kelvin-offset', 'candidate'),
  hit('ev-units-guide', 'evidence', { title: 'units-guide.md', snippet: HOSTILE }),
  hit('cand-minutes-rounding', 'candidate', { contradictory: true, title: 'Round minutes to two places' }),
  hit('cand-old-offset', 'candidate', { active: false, invalidated: true, title: 'Add 273 for Kelvin' }),
];
export const listFixture = (items = LIST_ITEMS, over = {}) => ({ items, total: items.length, offset: 0, ...over });

export function detailFixture(kind = 'candidate', over = {}) {
  const reference = over.reference ?? ref(`${kind}-kelvin-offset`, kind, 2);
  const bodies = {
    evidence: { kind: 'evidence', evidence_kind: 'artifact', content: `# Units\n\n${HOSTILE}\n`, truncated: false, content_sha256: sha('content') },
    candidate: { kind: 'candidate', statement: 'Kelvin: add 273.15 to a Celsius value.', evidence: [ref('ev-units-guide', 'evidence')] },
    verified_pattern: { kind: 'verified_pattern', statement: 'Kelvin: add 273.15 to a Celsius value.', candidate: ref('cand-kelvin-offset', 'candidate'), checks: [ref('chk-baseline', 'evidence'), ref('chk-fixed', 'evidence')] },
    approved_procedure: { kind: 'approved_procedure', pattern: ref('vp-kelvin-offset', 'verified_pattern'), outcome_evidence: [ref('run-kelvin', 'evidence')], approval_evidence: ref('ui-approval-kelvin', 'evidence') },
    invalidation: { kind: 'invalidation', target: ref('cand-old-offset', 'candidate'), reason: 'superseded' },
  };
  return {
    reference,
    kind,
    body: bodies[kind],
    source: badge(),
    audit: { actor: 'distiller', reason: 'extractive candidate' },
    timestamp_ms: 1790000000000,
    history: [ref(reference.logical_id, kind, 1), reference],
    edges: [
      { relation: 'supporting', target: ref('ev-units-guide', 'evidence'), direction: 'outgoing' },
      { relation: 'invalidated_by', target: ref('inv-old-offset', 'invalidation'), direction: 'incoming' },
    ],
    active: true,
    invalidated: false,
    contradictory: false,
    verification: null,
    allowed_actions: [],
    residuals: [],
    ...over,
  };
}

// ------------------------------------------------------------------ mount
const globalNames = [
  'window', 'document', 'navigator', 'localStorage', 'CustomEvent', 'Event',
  'MouseEvent', 'KeyboardEvent', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'HTMLTextAreaElement',
  'IS_REACT_ACT_ENVIRONMENT',
];

/**
 * Mount a production component with explicit IPC doubles. Every command the
 * component sends must be answered by `commands` or the component's read
 * defaults; anything else lands in `unexpected` and fails the test at teardown,
 * together with a check that every Tauri listener was removed.
 */
export async function mount(t, {
  component = 'knowledge', props = {}, strict = false, commands = {},
  status = statusFixture(), list = listFixture(), views = {}, capability, memories = [],
} = {}) {
  const dom = new JSDOM('<!doctype html><html><body><div id="mount"></div></body></html>', { url: 'https://knowledge.test.invalid/' });
  const oldGlobals = new Map(globalNames.map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const key of globalNames) {
    Object.defineProperty(globalThis, key, { value: key === 'IS_REACT_ACT_ENVIRONMENT' ? true : dom.window[key], configurable: true, writable: true });
  }
  const calls = [], unexpected = [];
  const state = { status, list, views: { ...views } };
  const reads = {
    knowledge: {
      knowledge_status: () => (typeof state.status === 'function' ? state.status() : structuredClone(state.status)),
      knowledge_list: () => structuredClone(state.list),
    },
    coding: {
      coding_task_capability: () => capability ?? Object.values(state.views)[0]?.capability ?? { state: 'supported_unverified' },
      coding_task_list: () => Object.values(state.views).map(v => ({
        task_id: v.task_id, objective_excerpt: v.objective.slice(0, 60), status: v.status,
        created_at_ms: 1790000000000, view_seq: v.view_seq, readable: true,
      })),
      coding_task_view: args => {
        const v = state.views[args?.taskId];
        if (!v) throw 'NotFound';
        return structuredClone(v);
      },
    },
    memory: {
      detect_vault: () => ({ detected: true, vault_root: '/media/pocket/UNOONE', vault_id: 'vault-1', startup_state: 'ready', validation_failures: [] }),
      search_memories: () => structuredClone(memories),
    },
    sidebar: {
      get_vault_status: () => ({ is_connected: true, is_unlocked: true, vault_id: 'v', profile_name: 'Test', used_space_gb: 0, total_space_gb: 1 }),
    },
  }[component];
  mockIPC((command, args) => {
    calls.push({ command, args: args === undefined ? undefined : structuredClone(args) });
    if (Object.hasOwn(commands, command)) return commands[command](args, state);
    if (reads && Object.hasOwn(reads, command)) return reads[command](args, state);
    unexpected.push(command);
    throw new Error(`Unexpected mock IPC command: ${command}`);
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
  const element = React.createElement({ knowledge: KnowledgeView, coding: CodingTaskView, memory: MemoryExplorer, sidebar: Sidebar }[component], props);
  await act(async () => {
    root.render(strict ? React.createElement(React.StrictMode, null, element) : element);
  });
  await flush();
  await flush();
  const q = selector => container.querySelector(selector);
  const qa = selector => [...container.querySelectorAll(selector)];
  const kn = name => container.querySelector(`[data-kn="${name}"]`);
  const kns = name => qa(`[data-kn="${name}"]`);
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
  /** Set a controlled input/textarea/select value the way a user would. */
  async function type(el, value) {
    assert.ok(el, 'input exists');
    const proto = el.tagName === 'TEXTAREA' ? dom.window.HTMLTextAreaElement.prototype
      : el.tagName === 'SELECT' ? dom.window.HTMLSelectElement.prototype : dom.window.HTMLInputElement.prototype;
    await act(async () => {
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value);
      el.dispatchEvent(new dom.window.Event(el.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true }));
    });
    await flush();
  }
  const byLabel = label => {
    const found = qa('input, textarea, select').filter(el => el.getAttribute('aria-label') === label);
    assert.equal(found.length, 1, `exactly one control labelled "${label}" (found ${found.length})`);
    return found[0];
  };
  async function wait(ms) {
    await act(async () => { await new Promise(r => setTimeout(r, ms)); });
    await flush();
  }
  return {
    dom, container, calls, unexpected, state, flush, unmount, click, type, byLabel, wait, q, qa, kn, kns, ct, cts, buttons, button,
    text: () => container.textContent,
    ipc: command => calls.filter(c => c.command === command).map(c => c.args),
    commandsCalled: () => calls.map(c => c.command),
    emit: async (event, payload) => { await act(async () => emit(event, payload)); await flush(); await flush(); },
    listeners: () => dom.window.__TAURI_INTERNALS__.callbacks.size,
  };
}
