import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  act, mount, lib, codingFixtures, frontendRoot, sha, ref, hit, statusFixture, listFixture, detailFixture,
  LIST_ITEMS, HOSTILE, METHOD, HEX32,
} from './knowledge-harness.mjs';

// Mounted production React with the official Tauri mockIPC double. These
// tests prove mounted frontend behaviour and the exact IPC arguments the
// views emit. They do NOT exercise native Tauri IPC, the Rust glue
// (knowledge_commands.rs), the K1/K2 adapter services, WebView2, Windows, a
// real vault, a real sandbox or a real file download.

const READS = ['knowledge_status', 'knowledge_list'];
const MUTATIONS = /^(knowledge_(initialize|rebuild_index|reject|revoke_approval|export|distill)|task_(propose_candidate|verify_candidate|approve_pattern|revoke_pattern))$/;
const FORBIDDEN_CLAIMS = /\b(implemented|done|success(ful(ly)?)?)\b/i;
const mutations = ui => ui.calls.filter(c => MUTATIONS.test(c.command));
const hitById = (ui, id) => ui.q(`[data-kn="hit"][data-id="${id}"]`);
const { baseView, summary, H } = codingFixtures;
const CODING_READS = ['coding_task_capability', 'coding_task_list', 'coding_task_view'];
const TID = 'a3f09c4d5e6f7a8b9c0d1e2f3a4b5c6d';

const GRANTS = {
  effective_root: '/home/dev/notes',
  user_granted: '/home/dev/notes',
  default_root: '/home/dev/UnoOne',
  folders: [
    { root: '/home/dev/projects/weather-tools', granted_at_ms: 1790000000000, exists: true },
    { root: '/home/dev/gone', granted_at_ms: 1790000000001, exists: false },
  ],
};

test('mount reads only knowledge_status and the first list page; method label, provenance badges, untrusted snippets as text; one listener cleaned at unmount', async t => {
  const ui = await mount(t);
  assert.deepEqual(ui.commandsCalled(), READS, 'on mount the view calls only the two read commands');
  assert.deepEqual(ui.ipc('knowledge_status'), [{}]);
  assert.deepEqual(ui.ipc('knowledge_list'), [{ filter: { kinds: [], state: 'all', offset: 0, limit: 50 } }], 'filter struct under the `filter` parameter');
  assert.match(ui.kn('method').textContent, new RegExp(`Method: ${METHOD.replace(/[;]/g, '\\$&')} — deterministic extraction; no model and no network are used\\.`));
  assert.equal(ui.kn('initialized').textContent, 'Initialized');
  assert.equal(ui.kn('index-state').textContent, 'Index: fresh');
  assert.deepEqual(ui.qa('[data-kn="count-row"]').map(r => r.textContent), ['Evidence33', 'Candidate22', 'Verified pattern11', 'Approved procedure11', 'Invalidation00']);
  assert.equal(ui.kns('hit').length, LIST_ITEMS.length);

  const first = hitById(ui, 'cand-kelvin-offset');
  assert.equal(first.querySelector('[data-kn="badge-source"]').textContent, 'source local:3f2a9c1d0b7e6a55');
  assert.equal(first.querySelector('[data-kn="badge-version"]').textContent, `version ${sha('units-guide.md v1').slice(0, 12)}`);
  assert.equal(first.querySelector('[data-kn="badge-licence"]').textContent, 'licence CC-BY-4.0');
  assert.equal(first.querySelector('[data-kn="badge-privacy"]').textContent, 'privacy private');
  assert.equal(first.querySelector('[data-kn="badge-platform"]').textContent, 'platform linux, windows');
  assert.equal(first.querySelector('[data-kn="hit-mode"]').textContent, 'Historical — audit only');
  assert.equal(first.querySelector('[data-kn="badge-contradictory"]'), null);
  assert.equal(hitById(ui, 'cand-minutes-rounding').querySelector('[data-kn="badge-contradictory"]').textContent, 'Contradictory');
  const old = hitById(ui, 'cand-old-offset');
  assert.equal(old.querySelector('[data-kn="badge-invalidated"]').textContent, 'Invalidated');

  // Source text is untrusted data: shown verbatim, never parsed as markup.
  const hostile = hitById(ui, 'ev-units-guide').querySelector('[data-kn="hit-snippet"]');
  assert.match(hostile.textContent, /From the source \(untrusted text\): Use 273\.15 <img src=x onerror=/);
  assert.equal(ui.container.querySelector('img'), null, 'no element was created from source text');
  assert.equal(globalThis.__pwned, undefined);

  assert.equal(ui.listeners(), 1, 'exactly one Tauri listener (unoone:knowledge-updated) while mounted');
  await ui.wait(350);
  assert.deepEqual(ui.commandsCalled(), READS, 'nothing else runs on its own, even after waiting');
  await ui.unmount();
  assert.equal(ui.listeners(), 0, 'listener removed on unmount');
  assert.deepEqual(mutations(ui), []);
});

test('uninitialized store: only the status read on mount; Initialize is a click that sends UiInitEvent with a fresh 128-bit ui_event_id', async t => {
  const empty = statusFixture({ initialized: false, index: 'missing', catalog_entries: 0, counts: {}, active_counts: {} });
  const ui = await mount(t, {
    status: empty,
    commands: { knowledge_initialize: (_a, s) => { s.status = statusFixture({ catalog_entries: 0 }); return s.status; } },
  });
  assert.deepEqual(ui.commandsCalled(), ['knowledge_status'], 'no list before the store exists');
  assert.equal(ui.kn('initialized').textContent, 'Not initialized');
  assert.match(ui.kn('init-banner').textContent, /not initialized in this vault.*imports nothing/);
  assert.equal(ui.button('Search').disabled, true);
  assert.equal(ui.button('List records').disabled, true);
  assert.deepEqual(ui.ipc('knowledge_initialize'), [], 'never initialized without a click');
  await ui.click(ui.button('Initialize knowledge store'));
  const sent = ui.ipc('knowledge_initialize');
  assert.equal(sent.length, 1);
  assert.deepEqual(Object.keys(sent[0]), ['event']);
  assert.deepEqual(Object.keys(sent[0].event), ['ui_event_id']);
  assert.match(sent[0].event.ui_event_id, HEX32);
  assert.equal(ui.kn('initialized').textContent, 'Initialized');
  assert.equal(ui.kn('init-banner'), null);
});

test('Explorer search: historical audit mode by default with an honest note; current mode needs a complete exact file identity; exact KnowledgeQuery shapes', async t => {
  const searchView = {
    hits: [hit('vp-kelvin-offset', 'verified_pattern')],
    normalization: { algorithm: 'nfkc-lower-v1', terms: ['kelvin', 'offset'], declared_aliases_used: [] },
    index: 'fresh',
    note: 'lexical postings, not semantic; historical results are audit-only',
  };
  const ui = await mount(t, { commands: { knowledge_search: ({ query }) => ({ ...searchView, hits: searchView.hits.map(h => ({ ...h, mode: query.mode })) }) } });
  assert.match(ui.kn('mode-note').textContent, /Without it the Explorer searches in historical audit mode: results may be stale,\s+revoked or contradictory and are shown for audit only\. Matching is lexical, not semantic\./);
  const mode = ui.byLabel('Recall mode');
  assert.equal(mode.value, 'historical');
  assert.equal(mode.querySelector('option[value="current"]').disabled, true, 'current mode unavailable without an identity');
  assert.equal(ui.button('Search').disabled, true, 'empty search is not sent');

  await ui.type(ui.byLabel('Knowledge search text'), '  kelvin offset ');
  await ui.click(ui.byLabel('Filter Verified'));
  await ui.click(ui.byLabel('Filter Approved'));
  await ui.click(ui.button('Search'));
  assert.deepEqual(ui.ipc('knowledge_search'), [{ query: {
    text: 'kelvin offset', mode: 'historical', platform: 'linux', trusted_source: null,
    kinds: ['verified_pattern', 'approved_procedure'], limit: 16,
  } }], 'KnowledgeQuery under the `query` parameter, serde snake_case fields');
  assert.equal(ui.kn('search-note').textContent, searchView.note);
  assert.equal(ui.kn('normalization').textContent, 'Normalization: nfkc-lower-v1 · terms kelvin, offset');
  assert.equal(ui.kn('historical-label').textContent, 'Historical audit results — not current guidance.');
  assert.equal(ui.kn('hit-mode').textContent, 'Historical — audit only');

  // Forcing "current" without an identity still searches historically.
  await ui.type(mode, 'current');
  assert.equal(ui.byLabel('Recall mode').value, 'historical');
  await ui.click(ui.button('Search'));
  assert.equal(ui.ipc('knowledge_search')[1].query.mode, 'historical');
  assert.equal(ui.ipc('knowledge_search')[1].query.trusted_source, null);

  // A partial identity is not enough; a complete one enables current mode.
  await ui.type(ui.byLabel('Trusted source id'), 'repo:weather-tools');
  await ui.type(ui.byLabel('Trusted source version'), 'inbharat.pai.selection.v1');
  await ui.type(ui.byLabel('Trusted source commit'), sha('manifest'));
  assert.equal(ui.byLabel('Recall mode').querySelector('option[value="current"]').disabled, true);
  await ui.type(ui.byLabel('Trusted file digest'), ` ${sha('temperature.py')} `);
  assert.equal(ui.byLabel('Recall mode').querySelector('option[value="current"]').disabled, false);
  await ui.type(ui.byLabel('Recall mode'), 'current');
  await ui.type(ui.byLabel('Platform'), 'windows');
  await ui.click(ui.button('Search'));
  assert.deepEqual(ui.ipc('knowledge_search')[2], { query: {
    text: 'kelvin offset', mode: 'current', platform: 'windows',
    trusted_source: { source_id: 'repo:weather-tools', source_version: 'inbharat.pai.selection.v1', source_commit: sha('manifest'), file_digest: sha('temperature.py') },
    kinds: ['verified_pattern', 'approved_procedure'], limit: 16,
  } });
  assert.equal(ui.kn('historical-label'), null);
  assert.equal(ui.kn('hit-mode').textContent, 'Current — exact file identity');
  assert.deepEqual(mutations(ui), []);
});

test('Explorer list: kind filters and list state reach KnowledgeListFilter; pagination sends the next offset', async t => {
  const ui = await mount(t, { list: listFixture(LIST_ITEMS, { total: 120 }) });
  await ui.click(ui.byLabel('Filter Invalidated'));
  await ui.click(ui.byLabel('Filter Candidate'));
  await ui.type(ui.byLabel('List state filter'), 'invalidated');
  await ui.click(ui.button('List records'));
  assert.deepEqual(ui.ipc('knowledge_list')[1], { filter: { kinds: ['candidate', 'invalidation'], state: 'invalidated', offset: 0, limit: 50 } }, 'kinds in display order');
  assert.equal(ui.button('Previous page').disabled, true);
  await ui.click(ui.button('Next page'));
  assert.deepEqual(ui.ipc('knowledge_list')[2], { filter: { kinds: ['candidate', 'invalidation'], state: 'invalidated', offset: 50, limit: 50 } });
  assert.deepEqual(mutations(ui), []);
});

test('Detail: body, citations, history, incoming/outgoing edges; only server allowed_actions; Reject needs a reason and a confirm that sends the displayed ref', async t => {
  const cand = detailFixture('candidate', { allowed_actions: ['reject'] });
  const rejected = {
    ...cand, active: false, invalidated: true, allowed_actions: [],
    edges: [...cand.edges, { relation: 'invalidated_by', target: ref('inv-new', 'invalidation'), direction: 'incoming' }],
  };
  const ui = await mount(t, {
    commands: {
      knowledge_detail: ({ logicalId, revision }) => ({ ...cand, history: cand.history, reference: revision === 1 ? cand.history[0] : cand.reference, _asked: logicalId }),
      knowledge_reject: () => rejected,
    },
  });
  await ui.click(ui.button('Open detail of Candidate cand-kelvin-offset r1'));
  assert.deepEqual(ui.ipc('knowledge_detail'), [{ logicalId: 'cand-kelvin-offset', revision: 1 }], 'camelCase logicalId, Option<u32> revision');
  assert.equal(ui.q('#kn-tab-detail').getAttribute('aria-selected'), 'true');
  // Re-open the head revision from the history row.
  await ui.click(ui.button('Open revision 2 of candidate-kelvin-offset'));
  assert.deepEqual(ui.ipc('knowledge_detail')[1], { logicalId: 'candidate-kelvin-offset', revision: 2 });
  assert.equal(ui.kn('statement').textContent, 'Kelvin: add 273.15 to a Celsius value.');
  assert.deepEqual(ui.qa('[data-kn="citations"] button').map(b => b.textContent), ['Evidence ev-units-guide r1']);
  assert.deepEqual(ui.qa('[data-kn="history"] button').map(b => [b.textContent, b.getAttribute('aria-pressed')]), [['r1', 'false'], ['r2', 'true']]);
  assert.deepEqual(ui.qa('[data-kn="edge"]').map(r => [r.dataset.direction, r.children[1].textContent, r.children[2].textContent]), [
    ['outgoing', 'supporting', 'Evidence ev-units-guide r1'],
    ['incoming', 'invalidated by', 'Invalidation inv-old-offset r1'],
  ]);
  // Exactly the server's actions.
  assert.equal(ui.buttons('Reject…').length, 1);
  assert.equal(ui.buttons('Revoke approval…').length, 0);
  assert.equal(ui.buttons('Add to export').length, 0);

  await ui.click(ui.button('Reject…'));
  const dialog = ui.kn('reject-dialog');
  assert.equal(dialog.getAttribute('aria-modal'), 'true');
  assert.equal(ui.kn('reject-ref').textContent, 'Candidate candidate-kelvin-offset r2');
  assert.equal(ui.kn('reject-digest').textContent, cand.reference.content_digest);
  assert.equal(ui.button('Confirm reject').disabled, true, 'a reason is required');
  await ui.click(ui.button('Confirm reject'));
  assert.deepEqual(ui.ipc('knowledge_reject'), []);
  // Escape closes without rejecting.
  await ui.type(ui.byLabel('Reject reason'), 'x');
  await act(async () => dialog.dispatchEvent(new ui.dom.window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
  await ui.flush();
  assert.equal(ui.kn('reject-dialog'), null);
  assert.deepEqual(ui.ipc('knowledge_reject'), []);

  await ui.click(ui.button('Reject…'));
  await ui.type(ui.byLabel('Reject reason'), '  Superseded by the 273.15 guide  ');
  assert.deepEqual(ui.ipc('knowledge_reject'), [], 'typing never rejects');
  await ui.click(ui.button('Confirm reject'));
  const sent = ui.ipc('knowledge_reject');
  assert.equal(sent.length, 1);
  assert.deepEqual(sent[0].event.target, cand.reference, 'the displayed reference is sent');
  assert.equal(sent[0].event.reason, 'Superseded by the 273.15 guide');
  assert.match(sent[0].event.ui_event_id, HEX32);
  assert.deepEqual(Object.keys(sent[0].event).sort(), ['reason', 'target', 'ui_event_id']);
  assert.equal(ui.kn('reject-dialog'), null);
  assert.equal(ui.kn('badge-invalidated').textContent, 'Invalidated');
  assert.equal(ui.buttons('Reject…').length, 0, 'the refreshed server actions no longer offer reject');
  assert.match(ui.text(), /An Invalidation record was appended\. The rejected record stays readable for audit\./);
  assert.equal(ui.ipc('knowledge_status').length, 2, 'status re-read after the change');
});

test('Detail of an approved procedure: verification summary with real exit codes; Revoke confirm sends the displayed approval; Add to export only because the server allows it', async t => {
  const approved = detailFixture('approved_procedure', {
    allowed_actions: ['revoke_approval', 'export'],
    verification: {
      checks: [
        { reference: ref('chk-b', 'evidence'), case: 'kelvin-positive', role: 'baseline', status: 1, termination: 'completed', passed: true },
        { reference: ref('chk-f', 'evidence'), case: 'kelvin-positive', role: 'fixed', status: 0, termination: 'completed', passed: true },
        { reference: ref('chk-t', 'evidence'), case: 'minutes-regression', role: 'fixed', status: null, termination: 'output_limit', passed: false },
      ],
      repetitions: 2, approved: true, approval: ref('ui-approval-kelvin', 'evidence'),
    },
  });
  const ui = await mount(t, {
    list: listFixture([hit('ap-kelvin-offset', 'approved_procedure')]),
    commands: {
      knowledge_detail: () => approved,
      knowledge_revoke_approval: () => ({ ...approved, active: false, invalidated: true, allowed_actions: [], verification: { ...approved.verification, approved: false } }),
    },
  });
  await ui.click(ui.button('Open detail of Approved procedure ap-kelvin-offset r1'));
  assert.deepEqual(ui.qa('[data-kn="check-exit"]').map(c => c.textContent), ['exit 1', 'exit 0', 'no exit code (output limit)'], 'real exit codes, never inferred');
  assert.equal(ui.kn('verification-approved').textContent, 'Approved for reuse by you');
  assert.match(ui.kn('verification').textContent, /Repetitions: 2/);

  await ui.click(ui.button('Add to export'));
  assert.equal(ui.q('#kn-tab-export').getAttribute('aria-selected'), 'true');
  assert.deepEqual(ui.ipc('knowledge_list')[1], { filter: { kinds: ['verified_pattern', 'approved_procedure'], state: 'active', offset: 0, limit: 100 } });
  const box = ui.byLabel('Select Approved procedure approved_procedure-kelvin-offset r2 for export');
  assert.equal(box.checked, true, 'the added record is pre-selected');

  await ui.click(ui.button('Detail'));
  await ui.click(ui.button('Revoke approval…'));
  assert.equal(ui.kn('revoke-ref').textContent, 'Approved procedure approved_procedure-kelvin-offset r2');
  assert.deepEqual(ui.ipc('knowledge_revoke_approval'), []);
  await ui.click(ui.button('Confirm revoke'));
  const sent = ui.ipc('knowledge_revoke_approval');
  assert.equal(sent.length, 1);
  assert.deepEqual(sent[0].event.approved, approved.reference);
  assert.match(sent[0].event.ui_event_id, HEX32);
  assert.deepEqual(Object.keys(sent[0].event).sort(), ['approved', 'ui_event_id']);
  assert.equal(ui.kn('verification-approved').textContent, 'Not approved for reuse');
  assert.equal(ui.buttons('Revoke approval…').length, 0);
});

test('Distiller: method always labelled; pasted text + local file from a granted folder; budget bounds; Preview shows sources + request hash; Run confirm sends the shown hash; exclusions, citations and heuristic contradictions shown; explicit index rebuild', async t => {
  const report = {
    run_id: sha('run-1').slice(0, 32),
    method: METHOD,
    evidence: [ref('ev-units-guide', 'evidence')],
    candidates: [{
      reference: ref('cand-kelvin', 'candidate'),
      statement: 'Kelvin: Add 273.15 to a Celsius value.',
      citation: { evidence: ref('ev-units-guide', 'evidence'), start: 10, end: 52, excerpt: HOSTILE },
    }],
    excluded: [
      { label: 'heldout-suite/check.md', reason: 'held_out_name' },
      { label: 'eval-copy', reason: 'held_out_hash' },
      { label: 'units-guide (2)', reason: 'duplicate' },
      { label: 'docs/units.md', reason: 'unreadable' },
    ],
    possible_contradictions: [{ candidate: ref('cand-kelvin', 'candidate'), existing: ref('cand-old-offset', 'candidate'), rule: 'negation-or-number-v1' }],
    budget_exhausted: true,
    elapsed_ms: 41,
    summary: ref('obs-run-1', 'evidence'),
  };
  const ui = await mount(t, {
    commands: {
      get_agent_workspace_info: () => GRANTS,
      knowledge_distill_runs: () => [],
      knowledge_distill_preview: ({ request }) => ({ request_sha256: sha(JSON.stringify(request)) }),
      knowledge_distill: (_a, s) => { s.status = statusFixture({ index: 'stale' }); return report; },
      knowledge_rebuild_index: (_a, s) => { s.status = statusFixture(); return s.status; },
    },
  });
  await ui.click(ui.button('Distiller'));
  assert.deepEqual(ui.commandsCalled().slice(READS.length), ['get_agent_workspace_info', 'knowledge_distill_runs'], 'tab open reads grants and past runs only');
  assert.match(ui.kn('distill-method').textContent, /Method: extractive-headings-docstrings-v1; deterministic; no model; no network\. Deterministic extraction/);
  assert.match(ui.kn('distill-method').textContent, /creates Evidence and Candidate records only;\s+nothing is verified or approved by it/);
  assert.match(ui.kn('distill-problems').textContent, /Add at least one source\./);
  assert.equal(ui.button('Preview').disabled, true);

  const text = '# Units\n\n## Kelvin\nAdd 273.15 to a Celsius value to get Kelvin.\n';
  await ui.type(ui.byLabel('Pasted source label'), 'units-guide');
  await ui.type(ui.byLabel('Pasted source text'), text);
  await ui.click(ui.button('Add pasted text'));
  const folder = ui.byLabel('Granted folder');
  assert.deepEqual([...folder.options].map(o => o.value), ['/home/dev/notes', '/home/dev/projects/weather-tools'], 'only existing granted folders');
  await ui.type(folder, '/home/dev/projects/weather-tools');
  await ui.type(ui.byLabel('Relative file path'), ' docs/units.md ');
  await ui.click(ui.button('Add local file'));
  assert.deepEqual(ui.qa('[data-kn="source"]').map(s => s.dataset.sourceKind), ['pasted_text', 'local_file']);

  await ui.type(ui.byLabel('Max candidates'), '65');
  assert.match(ui.kn('distill-problems').textContent, /Candidate budget must be 1–64\./);
  assert.equal(ui.button('Preview').disabled, true);
  await ui.type(ui.byLabel('Max candidates'), '16');
  await ui.type(ui.byLabel('Licence'), 'CC-BY-4.0');
  await ui.type(ui.byLabel('Topics'), 'weather, units');
  assert.equal(ui.kn('distill-problems'), null);
  assert.equal(ui.kn('distill-run').disabled, true, 'Run needs a preview first');

  await ui.click(ui.button('Preview'));
  const expected = {
    sources: [
      { kind: 'pasted_text', label: 'units-guide', text },
      { kind: 'local_file', root: '/home/dev/projects/weather-tools', path: 'docs/units.md' },
    ],
    budget: { max_total_bytes: 262144, max_candidates: 16, deadline_ms: 10000 },
    platform: 'linux',
    license: 'CC-BY-4.0',
    topics: ['weather', 'units'],
  };
  assert.deepEqual(ui.ipc('knowledge_distill_preview'), [{ request: expected }], 'DistillRequest under `request`, tagged sources');
  const shownHash = ui.kn('distill-hash').textContent;
  assert.equal(shownHash, sha(JSON.stringify(expected)));
  assert.match(ui.kn('distill-plan').textContent, /Pasted text "units-guide".*Local file docs\/units\.md in \/home\/dev\/projects\/weather-tools/);

  // Editing after the preview invalidates it until the request is the same again.
  await ui.type(ui.byLabel('Topics'), 'weather');
  assert.ok(ui.kn('distill-preview-stale'));
  assert.equal(ui.kn('distill-run').disabled, true);
  await ui.type(ui.byLabel('Topics'), 'weather, units');
  assert.equal(ui.kn('distill-preview-stale'), null);

  assert.deepEqual(ui.ipc('knowledge_distill'), [], 'never distilled without the confirm click');
  await ui.click(ui.kn('distill-run'));
  assert.equal(ui.kn('distill-confirm-hash').textContent, shownHash);
  assert.deepEqual(ui.ipc('knowledge_distill'), []);
  await ui.click(ui.button('Confirm run'));
  const sent = ui.ipc('knowledge_distill');
  assert.equal(sent.length, 1);
  assert.deepEqual(sent[0].request, expected);
  assert.equal(sent[0].event.request_sha256, shownHash, 'the displayed request hash is sent');
  assert.match(sent[0].event.ui_event_id, HEX32);
  assert.deepEqual(Object.keys(sent[0]).sort(), ['event', 'request']);

  assert.equal(ui.kn('report-method').textContent, `Method: ${METHOD}`);
  assert.equal(ui.kn('budget-exhausted').textContent, 'Budget exhausted — the run stopped early');
  assert.equal(ui.kn('candidate-statement').textContent, 'Kelvin: Add 273.15 to a Celsius value.');
  assert.match(ui.kn('citation').textContent, /Cites Evidence ev-units-guide r1 bytes 10–52:/);
  assert.equal(ui.kn('citation-excerpt').textContent, HOSTILE, 'excerpt shown verbatim as text');
  assert.equal(ui.container.querySelector('img'), null);
  assert.deepEqual(ui.qa('[data-kn="excluded-item"]').map(li => [li.dataset.reason, li.textContent]), [
    ['held_out_name', 'heldout-suite/check.md — held-out / evaluation material (path name rule)'],
    ['held_out_hash', 'eval-copy — held-out / evaluation material (content hash on the exclusion list)'],
    ['duplicate', 'units-guide (2) — duplicate of another source'],
    ['unreadable', 'docs/units.md — unreadable (on this platform local files cannot be captured fd-safely)'],
  ]);
  assert.match(ui.kn('contradictions').textContent, /Possible contradictions \(heuristic\)/);
  assert.match(ui.kn('contradiction').textContent, /heuristic rule negation-or-number-v1; a Contradicting link was added and nothing was invalidated\./);

  // The index is stale after the run; rebuilding is explicit.
  assert.equal(ui.kn('index-state').textContent, 'Index: stale — rebuild to search new records');
  assert.match(ui.kn('index-banner').textContent, /Search never rebuilds it on its own\./);
  assert.deepEqual(ui.ipc('knowledge_rebuild_index'), []);
  await ui.click(ui.button('Rebuild index'));
  assert.deepEqual(ui.ipc('knowledge_rebuild_index'), [{}]);
  assert.equal(ui.kn('index-banner'), null);
  assert.equal(ui.ipc('knowledge_distill').length, 1);
});

test('Export: training export disabled; only verified/approved kinds offered; preview shows refusals; private content needs acknowledgement; consent confirm sends the previewed request and hash; result offered as a download', async t => {
  const vp = hit('vp-kelvin-offset', 'verified_pattern');
  const ap = hit('ap-minutes', 'approved_procedure', { contradictory: true, title: 'Round minutes' });
  const ev = hit('ev-units-guide', 'evidence');
  const preview = {
    request_sha256: sha('export-request'),
    items: [{ reference: vp.reference, kind: 'verified_pattern', title: vp.title, licence: 'CC-BY-4.0' }],
    refused: [[ap.reference, 'contradictory: not exportable']],
    contains_private_content: true,
    training_export: 'disabled: separate consent and licence review required',
  };
  const exported = { schema: 'inbharat.pai.knowledge-export.v1', json: '{"schema":"inbharat.pai.knowledge-export.v1","items":[1]}', sha256: sha('bundle'), items: 1 };
  const ui = await mount(t, {
    list: listFixture([vp, ap, ev]),
    commands: { knowledge_export_preview: () => preview, knowledge_export: () => exported },
  });
  await ui.click(ui.button('Export'));
  assert.equal(ui.kn('training-export').textContent, 'Training export: disabled — separate consent and licence review required.');
  assert.deepEqual(ui.qa('[data-kn="export-choice"]').map(l => l.textContent), [
    `Verified pattern — ${vp.title}`,
    'Approved procedure — Round minutes (contradictory)',
  ], 'evidence is never offered for export');
  assert.equal(ui.button('Preview export').disabled, true, 'nothing selected');
  await ui.click(ui.byLabel(`Select Verified pattern vp-kelvin-offset r1 for export`));
  await ui.click(ui.byLabel(`Select Approved procedure ap-minutes r1 for export`));
  await ui.click(ui.byLabel('Include evidence content'));
  await ui.click(ui.button('Preview export'));
  const request = { references: [vp.reference, ap.reference], include_evidence_content: true };
  assert.deepEqual(ui.ipc('knowledge_export_preview'), [{ request }], 'ExportRequest under `request`');
  assert.deepEqual(ui.qa('[data-kn="export-refused-item"]').map(li => li.textContent), ['Approved procedure ap-minutes r1 — contradictory: not exportable']);
  assert.equal(ui.kn('export-hash').textContent, preview.request_sha256);
  assert.equal(ui.kn('export-training').textContent, 'Training export (server): disabled: separate consent and licence review required');
  assert.equal(ui.kn('export-open').disabled, true, 'private content needs the acknowledgement');
  await ui.click(ui.kn('export-open'));
  assert.equal(ui.kn('export-dialog'), null);
  await ui.click(ui.byLabel('Acknowledge private content'));
  assert.equal(ui.kn('export-open').disabled, false);

  // A changed selection invalidates the preview.
  await ui.click(ui.byLabel(`Select Approved procedure ap-minutes r1 for export`));
  assert.ok(ui.kn('export-preview-stale'));
  assert.equal(ui.kn('export-open').disabled, true);
  await ui.click(ui.byLabel(`Select Approved procedure ap-minutes r1 for export`));
  assert.equal(ui.kn('export-open').disabled, false);

  assert.deepEqual(ui.ipc('knowledge_export'), [], 'never exported without the consent confirm');
  await ui.click(ui.kn('export-open'));
  assert.equal(ui.kn('export-confirm-hash').textContent, preview.request_sha256);
  await ui.click(ui.button('Confirm export'));
  const sent = ui.ipc('knowledge_export');
  assert.equal(sent.length, 1);
  assert.deepEqual(sent[0], { event: { request, request_sha256: preview.request_sha256, ui_event_id: sent[0].event.ui_event_id, acknowledged_private: true } });
  assert.match(sent[0].event.ui_event_id, HEX32);
  const link = ui.kn('export-download');
  assert.equal(link.getAttribute('download'), `unoone-knowledge-export-${exported.sha256.slice(0, 12)}.json`);
  const href = link.getAttribute('href');
  assert.ok(href.startsWith('data:application/json;charset=utf-8,'));
  assert.equal(decodeURIComponent(href.slice('data:application/json;charset=utf-8,'.length)), exported.json);
  assert.equal(ui.kn('export-sha').textContent, exported.sha256);
});

test('Export without private content: acknowledgement not required and sent as false', async t => {
  const vp = hit('vp-kelvin-offset', 'verified_pattern');
  const preview = {
    request_sha256: sha('export-2'), items: [{ reference: vp.reference, kind: 'verified_pattern', title: vp.title, licence: 'unknown' }],
    refused: [], contains_private_content: false, training_export: 'disabled: separate consent and licence review required',
  };
  const ui = await mount(t, {
    list: listFixture([vp]),
    commands: { knowledge_export_preview: () => preview, knowledge_export: () => ({ schema: 's', json: '{}', sha256: sha('b2'), items: 1 }) },
  });
  await ui.click(ui.button('Export'));
  await ui.click(ui.byLabel('Select Verified pattern vp-kelvin-offset r1 for export'));
  await ui.click(ui.button('Preview export'));
  assert.equal(ui.kn('export-ack'), null);
  await ui.click(ui.kn('export-open'));
  await ui.click(ui.button('Confirm export'));
  assert.equal(ui.ipc('knowledge_export')[0].event.acknowledged_private, false);
  assert.equal(ui.ipc('knowledge_export')[0].event.request.include_evidence_content, false);
});

// ------------------------------------------------------------------ Learning panel

const passingView = (over = {}) => baseView({
  task_id: TID,
  view_seq: 21,
  diff: [
    summary('temperature.py', { base: H.tempBase, next: H.tempNew, decision: { kind: 'accepted' } }),
    summary('duration.py', { base: H.durBase, next: H.durNew, decision: { kind: 'accepted' } }),
  ],
  outcome: {
    ...baseView().outcome,
    build_status: { passed: { gate: 'gate-0002' } },
    test_status: { passed: { gate: 'gate-0002' } },
    goal_status: 'checks_passed_pending_review',
    review_status: { decided: { accepted: 2, rejected: 0 } },
  },
  ...over,
});
const RECIPE = {
  schema: 'inbharat.pai.verification-recipe.v1',
  repetitions: 2,
  limits: { cpu_seconds: 2, memory_bytes: 134217728, processes: 16, timeout_ms: 2000, output_bytes: 65536 },
  cases: [
    { name: 'kelvin-positive', kind: 'positive', argv: ['/usr/bin/python3', '-I', '/work/tests/check_temperature.py'], expected_status: 0, expected_stdout: 'ok\n', expected_stderr: '', expected_files: { '/tmp/out.txt': sha('out') } },
    { name: 'kelvin-negative', kind: 'negative', argv: ['/usr/bin/python3', '-I', '/work/tests/check_temperature.py', '--reject-negative'], expected_status: 3, expected_stdout: '', expected_stderr: 'rejected\n', expected_files: {} },
    { name: 'minutes-regression', kind: 'regression', argv: ['/usr/bin/python3', '-I', '/work/tests/check_duration.py'], expected_status: 0, expected_stdout: 'ok\n', expected_stderr: '', expected_files: {} },
  ],
  oracle_files: ['tests/check_duration.py', 'tests/check_temperature.py'],
  implementation_files: ['duration.py', 'temperature.py'],
};
const CANDIDATE = ref('cand-task-kelvin', 'candidate');
const PATTERN = ref('vp-task-kelvin', 'verified_pattern');
const RUN = ref('run-task-kelvin', 'evidence');
const APPROVED = ref('ap-task-kelvin', 'approved_procedure');
const VERIFIED = {
  state: 'verified', pattern: PATTERN, procedure_run: RUN, run_sha256: sha('run-record'), policy_sha256: sha('policy'),
  checks: [
    { reference: ref('chk-1', 'evidence'), case: 'kelvin-positive', role: 'baseline', status: 1, termination: 'completed', passed: true },
    { reference: ref('chk-2', 'evidence'), case: 'kelvin-positive', role: 'fixed', status: 0, termination: 'completed', passed: true },
    { reference: ref('chk-3', 'evidence'), case: 'kelvin-negative', role: 'fixed', status: 3, termination: 'completed', passed: true },
  ],
  approved: null,
  residuals: ['Stage 4 verification shapes are limited to PythonScript oracles'],
};

test('Learning panel: no learning IPC on mount; every step is a click that echoes the displayed view_seq, change set, recipe hash and run/policy hashes; real check exit codes', async t => {
  const patterns = {
    index: 'fresh',
    hits: [hit('vp-earlier-kelvin', 'verified_pattern', { mode: 'current', why_recalled: 'exact trusted source/platform/file fence' })],
    note: 'lexical match on exact file identity',
  };
  const ui = await mount(t, {
    component: 'coding',
    views: { [TID]: passingView() },
    commands: {
      task_relevant_patterns: () => patterns,
      task_propose_candidate: () => ({ candidate: CANDIDATE, evidence: [ref('obs-gate', 'evidence')], verification: { kind: 'supported', cases: 3 } }),
      task_verification_preview: () => ({ candidate: CANDIDATE, recipe: RECIPE, recipe_sha256: sha('recipe'), support: { kind: 'supported', cases: 3 } }),
      task_verify_candidate: () => VERIFIED,
      task_approve_pattern: () => ({ ...VERIFIED, approved: APPROVED }),
      task_revoke_pattern: () => ({ ...VERIFIED, approved: null, residuals: ['approval revoked'] }),
    },
  });
  await ui.wait(350);
  assert.deepEqual(ui.commandsCalled(), CODING_READS, 'the Learning panel calls nothing on its own');
  const panel = ui.ct('learning');
  assert.ok(panel);
  assert.equal(ui.ct('lp-save-reasons'), null);
  assert.equal(ui.ct('lp-save').disabled, false, 'build + tests passed and every file accepted');

  await ui.click(ui.button('Find relevant patterns'));
  assert.deepEqual(ui.ipc('task_relevant_patterns'), [{ taskId: TID, limit: 8 }]);
  assert.equal(ui.ct('lp-patterns-note').textContent, 'lexical match on exact file identity');
  assert.match(ui.ct('lp-pattern').textContent, /Verified pattern Kelvin offset — vp-earlier-kelvin — source local:3f2a9c1d0b7e6a55 · version [0-9a-f]{12} · licence CC-BY-4\.0/);

  await ui.click(ui.ct('lp-save'));
  const proposed = ui.ipc('task_propose_candidate');
  assert.equal(proposed.length, 1);
  assert.deepEqual(proposed[0], { taskId: TID, event: { view_seq: 21, change_set_sha256: H.changeSet, ui_event_id: proposed[0].event.ui_event_id } }, 'the displayed view_seq and change set are sent');
  assert.match(proposed[0].event.ui_event_id, HEX32);
  assert.equal(ui.ct('lp-candidate').textContent, 'Candidate cand-task-kelvin r1');
  assert.equal(ui.ct('lp-support').textContent, 'Verification supported: 3 cases.');

  await ui.click(ui.button('Verification preview'));
  assert.deepEqual(ui.ipc('task_verification_preview'), [{ taskId: TID, candidate: CANDIDATE }]);
  assert.deepEqual(ui.qa('[data-ct="lp-case"]').map(r => [...r.children].map(td => td.textContent)), [
    ['kelvin-positive', 'positive', '/usr/bin/python3 -I /work/tests/check_temperature.py', 'exit 0'],
    ['kelvin-negative', 'negative', '/usr/bin/python3 -I /work/tests/check_temperature.py --reject-negative', 'exit 3'],
    ['minutes-regression', 'regression', '/usr/bin/python3 -I /work/tests/check_duration.py', 'exit 0'],
  ]);
  const recipeHash = ui.ct('lp-recipe-hash').textContent;
  assert.equal(recipeHash, sha('recipe'));

  assert.deepEqual(ui.ipc('task_verify_candidate'), []);
  await ui.click(ui.button('Verify…'));
  assert.equal(ui.ct('lp-verify-hash').textContent, recipeHash);
  assert.deepEqual(ui.ipc('task_verify_candidate'), [], 'opening the dialog does not verify');
  await ui.click(ui.button('Confirm verify'));
  const verified = ui.ipc('task_verify_candidate');
  assert.equal(verified.length, 1);
  assert.deepEqual(verified[0], { taskId: TID, event: { candidate: CANDIDATE, recipe_sha256: recipeHash, ui_event_id: verified[0].event.ui_event_id } });
  assert.equal(ui.ct('lp-state').textContent, 'Verified by a sandboxed Stage 4 run');
  assert.deepEqual(ui.qa('[data-ct="lp-check-exit"]').map(c => c.textContent), ['exit 1', 'exit 0', 'exit 3'], 'real exit codes, baseline failing');
  assert.equal(ui.ct('lp-revoke-open').disabled, true, 'nothing approved yet');

  const runHash = ui.ct('lp-run-hash').textContent;
  const policyHash = ui.ct('lp-policy-hash').textContent;
  assert.deepEqual(ui.ipc('task_approve_pattern'), []);
  await ui.click(ui.button('Approve for reuse…'));
  assert.equal(ui.ct('lp-approve-run').textContent, runHash);
  assert.equal(ui.ct('lp-approve-policy').textContent, policyHash);
  await ui.click(ui.button('Confirm approval'));
  const approved = ui.ipc('task_approve_pattern');
  assert.equal(approved.length, 1);
  assert.deepEqual(approved[0], { taskId: TID, event: {
    pattern: PATTERN, procedure_run: RUN, displayed_run_sha256: runHash, displayed_policy_sha256: policyHash,
    ui_event_id: approved[0].event.ui_event_id,
  } });
  assert.equal(ui.ct('lp-approved').textContent, 'Approved for reuse by you');
  assert.equal(ui.ct('lp-approve-open').disabled, true, 'already approved');

  await ui.click(ui.button('Revoke approval…'));
  assert.equal(ui.ct('lp-revoke-ref').textContent, 'Approved procedure ap-task-kelvin r1');
  await ui.click(ui.button('Confirm revoke'));
  const revoked = ui.ipc('task_revoke_pattern');
  assert.deepEqual(revoked, [{ taskId: TID, event: { approved: APPROVED, ui_event_id: revoked[0].event.ui_event_id } }]);
  assert.equal(ui.ct('lp-approved'), null);

  const ids = [proposed, verified, approved, revoked].map(c => c[0].event.ui_event_id);
  assert.equal(new Set(ids).size, ids.length, 'every UI event has its own id');
  assert.doesNotMatch(panel.textContent, FORBIDDEN_CLAIMS, 'no Implemented/Done/success wording in the panel');
  assert.deepEqual(ui.calls.filter(c => /coding_task_(apply|review_file|run_gate|confirm_plan)/.test(c.command)), [], 'learning never touches task state');
});

test('Learning panel: Save as candidate stays disabled (with reasons) until build and tests passed and every file is accepted; a stale index returns no patterns', async t => {
  const ui = await mount(t, {
    component: 'coding',
    views: { [TID]: baseView({ task_id: TID }) },
    commands: { task_relevant_patterns: () => ({ index: 'stale', hits: [], note: 'lexical match on exact file identity' }) },
  });
  const save = ui.ct('lp-save');
  assert.equal(save.disabled, true);
  assert.deepEqual(ui.qa('[data-ct="lp-save-reasons"] li').map(li => li.textContent), [
    'The build has not passed on the current content.',
    'The tests have not passed on the current content.',
    '2 changed files are not accepted by you.',
  ]);
  await ui.click(save);
  assert.deepEqual(ui.ipc('task_propose_candidate'), []);
  await ui.click(ui.button('Find relevant patterns'));
  assert.equal(ui.ct('lp-index').textContent, 'Index: stale — rebuild to search new records');
  assert.match(ui.ct('lp-patterns').textContent, /no patterns are returned until you rebuild it in Knowledge\./);
  assert.match(ui.ct('lp-patterns').textContent, /No verified pattern matches the exact files of this task\./);
  assert.deepEqual(mutations(ui), []);
});

test('Windows Unsupported: Verify and Approve are disabled with the server reason (and the capability reason); nothing is verified', async t => {
  const reason = 'verification requires the Linux sandbox; unavailable on this platform';
  const capReason = 'no runtime-verified isolation backend on this OS (IsolationUnavailable)';
  const unsupported = { state: 'unsupported', reason: capReason };
  const ui = await mount(t, {
    component: 'coding',
    capability: unsupported,
    views: { [TID]: passingView({ capability: unsupported }) },
    commands: {
      task_propose_candidate: () => ({ candidate: CANDIDATE, evidence: [], verification: { kind: 'unsupported', reason } }),
      task_verification_preview: () => ({ candidate: CANDIDATE, recipe: RECIPE, recipe_sha256: sha('recipe'), support: { kind: 'unsupported', reason } }),
    },
  });
  assert.equal(ui.ct('lp-save').disabled, false, 'saving a candidate is a vault write and works on every platform');
  await ui.click(ui.ct('lp-save'));
  assert.equal(ui.ct('lp-support').textContent, `Verification unsupported: ${reason}`);
  await ui.click(ui.button('Verification preview'));
  const verify = ui.button('Verify…');
  assert.equal(verify.disabled, true);
  await ui.click(verify);
  assert.equal(ui.ct('lp-verify-dialog'), null);
  assert.deepEqual(ui.qa('[data-ct="lp-blocked"] li').map(li => li.textContent), [
    reason,
    `Sandboxed execution is unavailable on this system: ${capReason}`,
  ]);
  assert.deepEqual(ui.ipc('task_verify_candidate'), []);
  assert.deepEqual(ui.ipc('task_approve_pattern'), []);
  assert.equal(ui.buttons('Approve for reuse…').length, 0, 'no verification result, no approve control');
});

test('Learning errors are shown in the panel, never swallowed or turned into a result', async t => {
  const ui = await mount(t, {
    component: 'coding',
    views: { [TID]: passingView() },
    commands: { task_propose_candidate: () => Promise.reject('not_ready') },
  });
  await ui.click(ui.ct('lp-save'));
  assert.equal(ui.ct('lp-error').textContent, 'Save as candidate failed: not_ready');
  assert.equal(ui.ct('lp-proposal'), null);
});

// ------------------------------------------------------------------ MemoryExplorer

test('MemoryExplorer: the search box is live — debounced typing issues search_memories with the typed query; empty means wildcard', async t => {
  const memories = [{ id: 'm1', memory_type: 'Knowledge', title: 'Kelvin offset', preview: 'Add 273.15', relevance: 1, created_at: '2026-10-01T00:00:00Z' }];
  const ui = await mount(t, { component: 'memory', memories });
  assert.deepEqual(ui.commandsCalled(), ['detect_vault', 'search_memories']);
  const base = { memory_types: [], limit: 50, min_relevance: 0 };
  assert.deepEqual(ui.ipc('search_memories'), [{ query: { query: '*', ...base }, vaultRoot: '/media/pocket/UNOONE' }]);
  assert.equal(ui.qa('.memory-card').length, 1, 'existing card look preserved');
  assert.equal(ui.q('.memory-card-title').textContent, 'Kelvin offset');

  const input = ui.byLabel('Search memories');
  await ui.type(input, 'k');
  await ui.type(input, 'ke');
  await ui.type(input, '  kelvin offset ');
  assert.equal(ui.ipc('search_memories').length, 1, 'nothing is sent while typing');
  await ui.wait(400);
  assert.deepEqual(ui.ipc('search_memories').slice(1), [{ query: { query: 'kelvin offset', ...base }, vaultRoot: '/media/pocket/UNOONE' }], 'one debounced search with the typed (trimmed) query');
  assert.equal(ui.ipc('detect_vault').length, 2);

  await ui.type(input, '');
  await ui.wait(400);
  assert.deepEqual(ui.ipc('search_memories').at(-1), { query: { query: '*', ...base }, vaultRoot: '/media/pocket/UNOONE' }, 'clearing the box returns to the wildcard view');
  assert.equal(ui.ipc('search_memories').length, 3);
});

test('MemoryExplorer: a slow older search never overwrites the newer result', async t => {
  let release;
  const slow = new Promise(r => { release = r; });
  let n = 0;
  const ui = await mount(t, {
    component: 'memory',
    commands: {
      search_memories: ({ query }) => {
        n += 1;
        if (query.query === 'kel') return slow.then(() => [{ id: 'old', memory_type: 'Note', title: 'old result', preview: '', relevance: 1, created_at: '' }]);
        return [{ id: `r${n}`, memory_type: 'Note', title: `result for ${query.query}`, preview: '', relevance: 1, created_at: '' }];
      },
    },
  });
  const input = ui.byLabel('Search memories');
  await ui.type(input, 'kel');
  await ui.wait(350);
  await ui.type(input, 'kelvin');
  await ui.wait(350);
  assert.equal(ui.q('.memory-card-title').textContent, 'result for kelvin');
  release();
  await ui.flush();
  await ui.flush();
  assert.equal(ui.q('.memory-card-title').textContent, 'result for kelvin', 'the late older answer is discarded');
});

// ------------------------------------------------------------------ navigation, plumbing, a11y

test('navigation: the Sidebar offers "Knowledge" and App renders KnowledgeView for it; Coding Task stays', async t => {
  const navigated = [];
  const ui = await mount(t, { component: 'sidebar', props: { currentView: 'chat', onNavigate: view => navigated.push(view), onLock: () => {} } });
  const labels = ui.qa('.nav-item').map(el => el.textContent.trim());
  assert.ok(labels.includes('Coding Task'));
  assert.equal(labels.indexOf('Knowledge'), labels.indexOf('Coding Task') + 1, 'Knowledge sits right after Coding Task');
  await ui.click(ui.qa('.nav-item').find(el => el.textContent.trim() === 'Knowledge'));
  assert.deepEqual(navigated, ['knowledge']);
  assert.deepEqual(ui.commandsCalled(), ['get_vault_status']);
  const app = readFileSync(join(frontendRoot, 'src/App.tsx'), 'utf8');
  assert.match(app, /import \{ KnowledgeView \} from '\.\/components\/KnowledgeView';/);
  assert.match(app, /case 'knowledge':\n\s+return <KnowledgeView \/>;/);
  assert.match(app, /case 'coding':\n\s+return <CodingTaskView \/>;/);
});

test('IPC plumbing: the frozen §3.1 command names and event; the camelCase conversion is byte-identical to src/lib/tauri.ts', () => {
  assert.deepEqual(Object.values(lib.KNOWLEDGE_COMMANDS), [
    'knowledge_status', 'knowledge_initialize', 'knowledge_rebuild_index', 'knowledge_search', 'knowledge_list',
    'knowledge_detail', 'knowledge_reject', 'knowledge_revoke_approval', 'knowledge_export_preview', 'knowledge_export',
    'knowledge_distill_preview', 'knowledge_distill', 'knowledge_distill_runs', 'task_relevant_patterns',
    'task_propose_candidate', 'task_verification_preview', 'task_verify_candidate', 'task_approve_pattern', 'task_revoke_pattern',
  ]);
  assert.equal(lib.KNOWLEDGE_UPDATED_EVENT, 'unoone:knowledge-updated');
  assert.equal(lib.DISTILL_METHOD, METHOD);
  const extract = (src, start) => {
    const at = src.indexOf(start);
    assert.ok(at >= 0, start);
    return src.slice(at, src.indexOf('\n}\n', at) + 2);
  };
  const tauri = readFileSync(join(frontendRoot, 'src/lib/tauri.ts'), 'utf8');
  const mine = readFileSync(join(frontendRoot, 'src/lib/knowledge.ts'), 'utf8');
  for (const start of ['function snakeToCamelKey(', 'async function invoke<T>(']) {
    assert.equal(extract(mine, start), extract(tauri, start), `${start} identical to tauri.ts`);
  }
  // The Rust glue declares the same names (source scan; the glue's own std-only tests pin its signatures).
  const glue = readFileSync(join(frontendRoot, '../src-tauri/src/knowledge_commands.rs'), 'utf8');
  for (const name of Object.values(lib.KNOWLEDGE_COMMANDS)) assert.match(glue, new RegExp(`pub\\(crate\\) async fn ${name}\\(`), name);
  assert.match(glue, /pub\(crate\) const UPDATED_EVENT: &str = "unoone:knowledge-updated";/);
});

test('typed wrappers: exact argument shape of every §3.1 command as it reaches IPC', async t => {
  const answers = Object.fromEntries(Object.values(lib.KNOWLEDGE_COMMANDS).map(c => [c, () => ({})]));
  const ui = await mount(t, { commands: { ...answers, knowledge_status: () => statusFixture(), knowledge_list: () => listFixture() } });
  const api = lib.knowledgeApi;
  const r = ref('vp-kelvin-offset', 'verified_pattern');
  const query = { text: 'kelvin', mode: 'historical', platform: 'linux', trusted_source: null, kinds: [], limit: 16 };
  const filter = { kinds: ['candidate'], state: 'active', offset: 0, limit: 10 };
  const exportRequest = { references: [r], include_evidence_content: false };
  const distill = { sources: [{ kind: 'pasted_text', label: 'l', text: 't' }], budget: { max_total_bytes: 1, max_candidates: 1, deadline_ms: 1 }, platform: 'linux', license: 'unknown', topics: [] };
  const id = 'f'.repeat(32);
  const before = ui.calls.length;
  await api.status();
  await api.initialize({ ui_event_id: id });
  await api.rebuildIndex();
  await api.search(query);
  await api.list(filter);
  await api.detail('vp-kelvin-offset', null);
  await api.reject({ target: r, reason: 'x', ui_event_id: id });
  await api.revokeApproval({ approved: r, ui_event_id: id });
  await api.exportPreview(exportRequest);
  await api.export({ request: exportRequest, request_sha256: sha('p'), ui_event_id: id, acknowledged_private: false });
  await api.distillPreview(distill);
  await api.distill(distill, { request_sha256: sha('d'), ui_event_id: id });
  await api.distillRuns();
  await api.relevantPatterns(TID, 8);
  await api.proposeCandidate(TID, { view_seq: 3, change_set_sha256: sha('c'), ui_event_id: id });
  await api.verificationPreview(TID, r);
  await api.verifyCandidate(TID, { candidate: r, recipe_sha256: sha('r'), ui_event_id: id });
  await api.approvePattern(TID, { pattern: r, procedure_run: r, displayed_run_sha256: sha('run'), displayed_policy_sha256: sha('pol'), ui_event_id: id });
  await api.revokePattern(TID, { approved: r, ui_event_id: id });
  assert.deepEqual(ui.calls.slice(before), [
    { command: 'knowledge_status', args: {} },
    { command: 'knowledge_initialize', args: { event: { ui_event_id: id } } },
    { command: 'knowledge_rebuild_index', args: {} },
    { command: 'knowledge_search', args: { query } },
    { command: 'knowledge_list', args: { filter } },
    { command: 'knowledge_detail', args: { logicalId: 'vp-kelvin-offset', revision: null } },
    { command: 'knowledge_reject', args: { event: { target: r, reason: 'x', ui_event_id: id } } },
    { command: 'knowledge_revoke_approval', args: { event: { approved: r, ui_event_id: id } } },
    { command: 'knowledge_export_preview', args: { request: exportRequest } },
    { command: 'knowledge_export', args: { event: { request: exportRequest, request_sha256: sha('p'), ui_event_id: id, acknowledged_private: false } } },
    { command: 'knowledge_distill_preview', args: { request: distill } },
    { command: 'knowledge_distill', args: { request: distill, event: { request_sha256: sha('d'), ui_event_id: id } } },
    { command: 'knowledge_distill_runs', args: {} },
    { command: 'task_relevant_patterns', args: { taskId: TID, limit: 8 } },
    { command: 'task_propose_candidate', args: { taskId: TID, event: { view_seq: 3, change_set_sha256: sha('c'), ui_event_id: id } } },
    { command: 'task_verification_preview', args: { taskId: TID, candidate: r } },
    { command: 'task_verify_candidate', args: { taskId: TID, event: { candidate: r, recipe_sha256: sha('r'), ui_event_id: id } } },
    { command: 'task_approve_pattern', args: { taskId: TID, event: { pattern: r, procedure_run: r, displayed_run_sha256: sha('run'), displayed_policy_sha256: sha('pol'), ui_event_id: id } } },
    { command: 'task_revoke_pattern', args: { taskId: TID, event: { approved: r, ui_event_id: id } } },
  ]);
});

test('pure helpers: ui_event_id format, distill bounds, exportable kinds, propose gate, exclusion labels', () => {
  const ids = new Set(Array.from({ length: 64 }, () => lib.newUiEventId()));
  assert.equal(ids.size, 64);
  for (const id of ids) assert.match(id, HEX32);
  const ok = { sources: [{ kind: 'pasted_text', label: 'l', text: 't' }], budget: { max_total_bytes: 1048576, max_candidates: 64, deadline_ms: 30000 }, platform: 'linux', license: 'unknown', topics: [] };
  assert.deepEqual(lib.distillRequestProblems(ok), []);
  const bad = lib.distillRequestProblems({ ...ok, sources: [], budget: { max_total_bytes: 1048577, max_candidates: 0, deadline_ms: 30001 }, license: '', topics: Array(9).fill('t') });
  assert.deepEqual(bad, [
    'Add at least one source.', 'Byte budget must be 1–1048576.', 'Candidate budget must be 1–64.', 'Deadline must be 1–30000 ms.',
    'Licence is required (at most 256 characters; "unknown" is allowed).', 'At most 8 topics.',
  ]);
  assert.match(lib.distillRequestProblems({ ...ok, sources: Array(33).fill(ok.sources[0]) }).join(' '), /At most 32 sources per run\./);
  assert.match(lib.distillRequestProblems({ ...ok, sources: [{ kind: 'pasted_text', label: 'x'.repeat(129), text: 't' }] }).join(' '), /label of 1–128/);
  assert.deepEqual(['evidence', 'candidate', 'verified_pattern', 'approved_procedure', 'invalidation'].map(lib.isExportableKind), [false, false, true, true, false]);
  assert.equal(lib.completeTrustedSource({ source_id: 'a', source_version: 'b', source_commit: 'c', file_digest: ' ' }), null);
  assert.deepEqual(lib.completeTrustedSource({ source_id: ' a', source_version: 'b ', source_commit: 'c', file_digest: 'd' }), { source_id: 'a', source_version: 'b', source_commit: 'c', file_digest: 'd' });
  assert.equal(lib.exclusionLabel('held_out_hash'), 'held-out / evaluation material (content hash on the exclusion list)');
  assert.equal(lib.exclusionLabel('future_reason'), 'future_reason', 'unknown reasons are shown verbatim, never dropped');
  assert.equal(lib.checkExitLabel({ status: 0, termination: 'completed' }), 'exit 0');
  assert.equal(lib.checkExitLabel({ status: null, termination: 'runner_failure' }), 'no exit code (runner failure)');
  const v = baseView({ task_id: TID });
  assert.equal(lib.canProposeCandidate(v, new Set()).ok, false);
  const pass = passingView();
  assert.deepEqual(lib.canProposeCandidate(pass, new Set()), { ok: true, reasons: [] });
  assert.deepEqual(lib.canProposeCandidate(pass, new Set(['duration.py'])).reasons, ['1 changed file is not accepted by you.'], 'a stale decision blocks saving');
  assert.deepEqual(lib.canProposeCandidate({ ...pass, change_set_sha256: '' }, new Set()).reasons, ['The task view carries no change-set hash.']);
  assert.deepEqual(lib.canProposeCandidate({ ...pass, diff: [] }, new Set()).reasons, ['No changed files.']);
});

test('errors are shown, never swallowed: a locked vault status', async t => {
  const ui = await mount(t, {
    status: () => { throw 'locked'; },
    commands: {},
  });
  assert.deepEqual(ui.commandsCalled(), ['knowledge_status']);
  assert.equal(ui.q('[role="alert"]').textContent, 'Could not read the knowledge status: locked');
  assert.equal(ui.kn('method'), null, 'no status fabricated');
});

test('errors are shown, never swallowed: unreadable grants/runs and a refused distill preview', async t => {
  const ui2 = await mount(t, {
    commands: {
      get_agent_workspace_info: () => Promise.reject('workspace unavailable'),
      knowledge_distill_runs: () => Promise.reject('locked'),
      knowledge_distill_preview: () => Promise.reject('invalid'),
    },
  });
  await ui2.click(ui2.button('Distiller'));
  assert.match(ui2.text(), /Could not read your granted folders: workspace unavailable/);
  assert.match(ui2.text(), /Could not read past distillation runs: locked/);
  await ui2.type(ui2.byLabel('Pasted source label'), 'guide');
  await ui2.type(ui2.byLabel('Pasted source text'), 'text');
  await ui2.click(ui2.button('Add pasted text'));
  await ui2.click(ui2.button('Preview'));
  assert.equal(ui2.q('[role="alert"]').textContent, 'Preview distillation failed: invalid');
  assert.equal(ui2.kn('distill-plan'), null);
  assert.equal(ui2.kn('distill-run').disabled, true);
});

test('the content-free update event re-reads the status only', async t => {
  const ui = await mount(t);
  await ui.emit('unoone:knowledge-updated', { command: 'knowledge_distill', task_id: null });
  assert.deepEqual(ui.commandsCalled(), [...READS, 'knowledge_status']);
  assert.deepEqual(mutations(ui), []);
});

test('StrictMode mount sends no mutation and cleans every listener', async t => {
  const strict = await mount(t, { strict: true });
  assert.ok(strict.calls.every(c => READS.includes(c.command)));
  assert.equal(strict.listeners(), 1);
  await strict.unmount();
  assert.equal(strict.listeners(), 0);
});

test('accessibility: every control is named and sits under a 44px-scoped container; dialogs are modal and labelled; the appended Stage 6 CSS keeps the 44px rule and leaves Stage 5 intact', async t => {
  const approved = detailFixture('approved_procedure', { allowed_actions: ['reject', 'revoke_approval', 'export'] });
  const ui = await mount(t, {
    commands: {
      knowledge_detail: () => approved,
      get_agent_workspace_info: () => GRANTS,
      knowledge_distill_runs: () => [],
    },
  });
  const check = () => {
    const nameOf = el => (el.getAttribute('aria-label') || el.textContent || '').trim();
    for (const b of ui.qa('button')) assert.ok(nameOf(b).length > 0, `button has a name: ${b.outerHTML.slice(0, 80)}`);
    for (const el of ui.qa('input, select, textarea')) {
      assert.ok(el.getAttribute('aria-label') || el.closest('label')?.textContent.trim(), `control labelled: ${el.outerHTML.slice(0, 80)}`);
    }
    for (const el of ui.qa('button, input, select, textarea')) {
      assert.ok(el.closest('.knowledge-view, .kn-dialog, .kn-header-actions'), `under a 44px-scoped container: ${el.outerHTML.slice(0, 80)}`);
    }
  };
  check();
  await ui.click(ui.button('Distiller'));
  check();
  await ui.click(ui.button('Export'));
  check();
  await ui.click(ui.button('Explorer'));
  await ui.click(ui.button('Open detail of Candidate cand-kelvin-offset r1'));
  check();
  assert.deepEqual(ui.qa('[role="tab"]').map(t => [t.textContent, t.getAttribute('aria-selected')]), [['Explorer', 'false'], ['Detail', 'true'], ['Distiller', 'false'], ['Export', 'false']]);
  await ui.click(ui.button('Reject…'));
  const dialog = ui.kn('reject-dialog');
  assert.equal(dialog.getAttribute('aria-modal'), 'true');
  assert.equal(ui.dom.window.document.getElementById(dialog.getAttribute('aria-labelledby')).textContent, 'Reject this record');
  check();
  await ui.click(ui.button('Keep the record'));
  await ui.click(ui.button('Revoke approval…'));
  assert.equal(ui.dom.window.document.getElementById(ui.kn('revoke-dialog').getAttribute('aria-labelledby')).textContent, 'Revoke this approval');
  await ui.click(ui.button('Keep the approval'));
  assert.deepEqual(mutations(ui), []);

  const css = readFileSync(join(frontendRoot, 'src/index.css'), 'utf8');
  const stage5 = css.indexOf('/* ===== Coding Task View (Stage 5) =====');
  const stage6 = css.indexOf('/* ===== Knowledge View + Coding Task Learning panel (Stage 6) =====');
  assert.ok(stage5 > 0 && stage6 > stage5, 'Stage 6 rules are appended after Stage 5');
  const rule = css.slice(stage6);
  assert.match(rule, /\.knowledge-view \.btn,\n\.kn-dialog \.btn,\n\.kn-header-actions \.btn,\n\.knowledge-view select,\n\.knowledge-view \.kn-input \{\n  min-height: 44px;\n  min-width: 44px;/);
  assert.match(rule, /\.knowledge-view \.ct-check \{[^}]*min-height: 44px;/);
});
