import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  act, mount, baseView, summary, taskSummary, temperatureDiff, durationDiff, readyPreview, deferred, trustedText,
  lib, previewWindowLib, emitRaw, frontendRoot, sha, riskHashOf, hex,
  TASK_ID, OTHER_TASK_ID, H, NARRATIVE, RISKS, PREVIEW_URL,
} from './coding-task-harness.mjs';

// Mounted production React with the official Tauri mockIPC double. These
// tests prove mounted frontend behaviour and the exact IPC arguments the view
// emits. They do NOT exercise native Tauri IPC, the Rust glue, WebView2,
// Windows, a real vault or a real sandbox.

const READS = ['coding_task_capability', 'coding_task_list', 'coding_task_view'];
const MUTATIONS = /coding_task_(apply|revert_applied|review_file|revert_file|resolve_interrupted|resume|confirm_plan|run_gate|start_preview|stop_preview|http_checks|cancel|open)$/;
const FORBIDDEN_CLAIMS = /\b(implemented|done|success(ful(ly)?)?)\b/i;
const chip = (ui, key) => ui.ct(`chip-${key}`)?.textContent;
const fileRow = (ui, path) => ui.q(`[data-ct="file"][data-path="${path}"]`);
const bump = (s, patch = {}) => {
  const v = s.views[TASK_ID];
  s.views[TASK_ID] = { ...v, view_seq: v.view_seq + 1, ...patch };
  return structuredClone(s.views[TASK_ID]);
};
const decidedView = (over = {}) => baseView({
  diff: [
    summary('temperature.py', { base: H.tempBase, next: H.tempNew, decision: { kind: 'accepted' } }),
    summary('duration.py', { base: H.durBase, next: H.durNew, decision: { kind: 'rejected' } }),
  ],
  ...over,
});

test('mount reads only capability, list and view; header shows repo, unverified branch label, objective, capability and status', async t => {
  const ui = await mount(t);
  assert.deepEqual(ui.commandsCalled(), READS, 'on mount the view calls only the three read commands');
  assert.deepEqual(ui.ipc('coding_task_capability'), [{}]);
  assert.deepEqual(ui.ipc('coding_task_list'), [{}]);
  assert.deepEqual(ui.ipc('coding_task_view'), [{ taskId: TASK_ID }], 'camelCase top-level key, as tauri.ts converts');
  assert.equal(ui.ct('repo-path').textContent, '/home/dev/projects/weather-tools');
  assert.equal(ui.ct('repo-label').textContent, 'branch main @ 89abcdef0123 — from .git files — not verified');
  assert.equal(ui.ct('objective').textContent, baseView().objective);
  assert.equal(ui.ct('capability').textContent, 'Isolation: Runtime verified');
  assert.equal(ui.ct('task-status').textContent, 'Awaiting your review');
  assert.match(ui.text(), /Protected test files \(hidden from the assistant\): tests\/helpers\.py, tests\/test_temperature\.py/);
  assert.equal(ui.listeners(), 1, 'exactly one Tauri listener (unoone:coding-task-updated) while mounted');
  await ui.unmount();
  assert.equal(ui.listeners(), 0, 'listener removed on unmount');
});

test('repository label sources: unknown and user labels are never shown as verified', async t => {
  const unknown = baseView({ repository: { ...baseView().repository, branch: null, head_commit: null, label_source: 'unknown' } });
  const ui = await mount(t, { views: { [TASK_ID]: unknown } });
  assert.equal(ui.ct('repo-label').textContent, 'branch unknown');
  await ui.unmount();
  const user = baseView().repository;
  assert.equal(lib.repositoryBadge({ ...user, label_source: 'user_label', branch: 'release' }), 'label release — your label — not verified');
  assert.equal(lib.repositoryBadge({ ...user, head_commit: null }), 'branch main — from .git files — not verified');
});

test('model narrative "all passed" + real gate exit 2: Build Failed (exit 2), Goal not met, no success wording outside the collapsed untrusted notes', async t => {
  const ui = await mount(t);
  assert.equal(chip(ui, 'tool'), 'OK (tool calls only — not a build or goal result)');
  assert.equal(chip(ui, 'build'), 'Failed (exit 2) — compile');
  assert.equal(chip(ui, 'test'), 'Not run');
  assert.equal(chip(ui, 'preview'), 'Not part of this task');
  assert.equal(chip(ui, 'browser'), 'Not verified by UnoOne — HTTP checks only');
  assert.equal(chip(ui, 'goal'), 'Not met (build-ok)');
  assert.equal(chip(ui, 'review'), '2 files awaiting your decision');
  assert.equal(chip(ui, 'apply'), 'Not applied');
  assert.doesNotMatch(ui.ct('outcome').textContent, /passed|complete|success|implemented|\bdone\b/i);
  // Real exit codes and truncation metadata from the gate record.
  assert.equal(ui.ct('exit-code').textContent, 'exit 2');
  assert.equal(ui.ct('output-bytes').textContent, 'stdout 0/0 B · stderr 65536/70000 B (truncated)');
  assert.match(ui.ct('gate').textContent, /python3 -m py_compile temperature\.py duration\.py/);
  assert.match(ui.q('[data-criterion="build-ok"]').textContent, /Failed \(exit 2\)/);
  assert.match(ui.q('[data-criterion="tests-ok"]').textContent, /Not run/);
  assert.match(ui.q('[data-criterion="look"]').textContent, /Manual — your judgement/);
  // The narrative exists only inside the collapsed, labelled untrusted block.
  const notes = ui.ct('assistant-notes');
  assert.equal(notes.tagName, 'DETAILS');
  assert.equal(notes.open, false, 'assistant notes are collapsed by default');
  assert.match(notes.querySelector('summary').textContent, /Assistant notes \(unverified model text\)/);
  assert.ok(notes.textContent.includes(NARRATIVE));
  const trusted = trustedText(ui);
  assert.ok(!trusted.includes('Build complete'), 'narrative never leaks into trusted UI text');
  assert.doesNotMatch(trusted, /all tests passed|explicit_approval/i);
  assert.doesNotMatch(trusted, FORBIDDEN_CLAIMS, 'no Implemented/Done/success wording anywhere outside the untrusted notes');
  assert.match(ui.ct('journal').textContent, /narrative — unverified model text \(see Assistant notes\)/);
  assert.deepEqual(ui.qa('[data-ct="step"]').map(r => [r.dataset.step, r.dataset.state]), [['capture', 'completed'], ['gate-1', 'failed']]);
});

test('outcome chip ladder: the strongest pre-acceptance goal wording is "Checks passed — awaiting your review"', () => {
  const o = baseView().outcome;
  const goals = [
    ['unverified', 'Unverified'],
    [{ unmet: { criteria: ['tests-ok', 'api-ok'] } }, 'Not met (tests-ok, api-ok)'],
    ['checks_passed_pending_review', 'Checks passed — awaiting your review'],
    ['accepted', 'Accepted by you'],
    ['rejected', 'Rejected by you'],
  ];
  const checks = ['not_run', { passed: { gate: 'g1' } }, { failed: { gate: 'g1', command: 'unit', exit: null } }, { error: { gate: 'g1', kind: 'timeout' } }, { stale: { gate: 'g1' } }];
  for (const [goal, label] of goals) {
    for (const build of checks) {
      const badges = lib.outcomeBadges({ ...o, goal_status: goal, build_status: build, test_status: build });
      assert.deepEqual(badges.map(b => b.key), ['tool', 'build', 'test', 'preview', 'browser', 'goal', 'review', 'apply'], 'distinct chips');
      assert.equal(badges.find(b => b.key === 'goal').label, label);
      for (const b of badges) assert.doesNotMatch(b.label, FORBIDDEN_CLAIMS, `${b.key}: ${b.label}`);
    }
  }
  const failedNoExit = lib.outcomeBadges({ ...o, build_status: { failed: { gate: 'g', command: 'compile', exit: null } } });
  assert.equal(failedNoExit[1].label, 'Failed (no exit code) — compile');
  assert.equal(lib.outcomeBadges({ ...o, test_status: { stale: { gate: 'g' } } })[2].label, 'Stale — content changed since the last run');
  assert.equal(lib.outcomeBadges({ ...o, tool_status: { partial: { failed: 3 } } })[0].label, '3 tool calls failed');
});

test('assistant-proposed plan shows "confirm to use"; Confirm sends UiPlanConfirmEvent with the displayed revision and view_seq', async t => {
  const ui = await mount(t, {
    commands: {
      coding_task_confirm_plan: (_args, s) => bump(s, { plan: { ...s.views[TASK_ID].plan, confirmed: true } }),
    },
  });
  assert.equal(ui.ct('plan-author').textContent, 'Proposed by assistant — confirm to use');
  assert.deepEqual(ui.ipc('coding_task_confirm_plan'), [], 'never confirmed without a click');
  await ui.click(ui.button('Confirm plan'));
  assert.deepEqual(ui.ipc('coding_task_confirm_plan'), [{ event: { task_id: TASK_ID, view_seq: 12, revision: 2 } }], 'UiPlanConfirmEvent under the `event` parameter, serde snake_case fields');
  assert.equal(ui.ct('plan-author').textContent, 'Proposed by assistant — confirmed by you');
  assert.equal(ui.buttons('Confirm plan').length, 0);
});

test('per-file review: decisions need the diff on screen and echo its hashes; Apply stays disabled until every file is decided; confirm sends the displayed hashes; apply never called without a click', async t => {
  const report = { step_id: 'apply-1', files: ['temperature.py'], worktree: { path: '/data/unoone/task-worktrees/6f1c', dev: 64769, ino: 9911, created_at_ms: 1790000009000 }, post_image: { 'temperature.py': H.tempNew } };
  const ui = await mount(t, {
    commands: {
      coding_task_file_diff: args => (args.path === 'temperature.py' ? temperatureDiff() : durationDiff()),
      coding_task_review_file: ({ event }, s) => {
        const v = s.views[TASK_ID];
        assert.equal(event.view_seq, v.view_seq, 'review echoes the displayed view_seq');
        const diff = v.diff.map(d => (d.path === event.path ? { ...d, decision: event.decision } : d));
        const pending = diff.filter(d => d.decision.kind === 'pending').length;
        return bump(s, {
          diff,
          change_set_sha256: sha(JSON.stringify(diff)),
          outcome: { ...v.outcome, review_status: pending ? { pending: { n: pending } } : { decided: { accepted: 1, rejected: 1 } } },
        });
      },
      coding_task_apply: (_args, s) => {
        bump(s, { status: 'applied', outcome: { ...s.views[TASK_ID].outcome, apply_status: { applied: { files: report.files, worktree: report.worktree.path } } } });
        return report;
      },
    },
  });
  const applyOpen = ui.ct('apply-open');
  assert.equal(applyOpen.disabled, true);
  assert.match(ui.ct('apply-reasons').textContent, /2 files still need your decision\./);
  assert.equal(ui.ct('diff-pane'), null, 'no decision buttons before a diff is shown');
  assert.equal(ui.buttons('Accept file').length, 0);

  await ui.click(ui.button('Show diff of temperature.py'));
  assert.deepEqual(ui.ipc('coding_task_file_diff'), [{ taskId: TASK_ID, path: 'temperature.py' }]);
  const rows = ui.qa('[data-ct="diff-pane"] table.ct-diff tr').map(tr => [...tr.children].map(td => td.textContent));
  assert.deepEqual(rows, [
    ['3', '3', ' ', 'def to_kelvin(celsius):'],
    ['4', '', '-', '    return celsius + 273'],
    ['', '4', '+', '    return celsius + 273.15'],
    ['5', '5', ' ', ''],
  ], 'unified diff with old/new line numbers');
  assert.match(ui.ct('diff-hashes').textContent, new RegExp(`${H.tempBase}.*${H.tempNew}`));
  const hunkBox = ui.q('[data-ct="diff-pane"] input[type="checkbox"]');
  assert.equal(hunkBox.disabled, true, 'per-hunk acceptance disabled (composed hash not exposed)');

  await ui.click(ui.button('Accept file temperature.py'));
  assert.deepEqual(ui.ipc('coding_task_review_file'), [{ event: {
    task_id: TASK_ID, view_seq: 12, path: 'temperature.py', decision: { kind: 'accepted' },
    displayed_base_sha256: H.tempBase, displayed_new_sha256: H.tempNew,
  } }]);
  assert.equal(fileRow(ui, 'temperature.py').dataset.decision, 'accepted');
  assert.equal(ui.ct('apply-open').disabled, true, 'one file still undecided');
  assert.match(ui.ct('apply-reasons').textContent, /1 file still needs your decision\./);

  await ui.click(ui.button('Show diff of duration.py'));
  await ui.click(ui.button('Reject file duration.py'));
  assert.deepEqual(ui.ipc('coding_task_review_file')[1], { event: {
    task_id: TASK_ID, view_seq: 13, path: 'duration.py', decision: { kind: 'rejected' },
    displayed_base_sha256: H.durBase, displayed_new_sha256: H.durNew,
  } });
  assert.equal(ui.ct('apply-open').disabled, false, 'every file decided, one accepted');
  assert.equal(ui.ct('apply-reasons'), null);
  assert.deepEqual(ui.ipc('coding_task_apply'), [], 'apply is never called without the confirm click');

  await ui.click(ui.ct('apply-open'));
  const dialog = ui.ct('apply-dialog');
  assert.equal(dialog.getAttribute('role'), 'dialog');
  assert.match(ui.ct('apply-files').textContent, /temperature\.py — Accepted.*duration\.py — Rejected/);
  const confirm = ui.ct('apply-confirm');
  const shownChangeSet = ui.ct('apply-change-set').textContent;
  const shownRiskHash = ui.ct('apply-risk-hash').textContent;
  assert.equal(shownChangeSet, ui.state.views[TASK_ID].change_set_sha256);
  assert.equal(shownRiskHash, riskHashOf(RISKS));
  const boxes = ui.qa('[data-ct="apply-risk"] input');
  assert.equal(boxes.length, RISKS.length, 'each unresolved risk must be ticked');
  for (const [i, box] of boxes.entries()) {
    assert.equal(confirm.disabled, true, `confirm disabled with ${i} of ${boxes.length} risks ticked`);
    await ui.click(box);
  }
  assert.equal(ui.ct('apply-confirm').disabled, false);
  assert.deepEqual(ui.ipc('coding_task_apply'), [], 'ticking risks does not apply');
  await ui.click(ui.ct('apply-confirm'));
  assert.deepEqual(ui.ipc('coding_task_apply'), [{ event: {
    task_id: TASK_ID, view_seq: 14,
    displayed_change_set_sha256: shownChangeSet,
    displayed_risks_sha256: shownRiskHash,
    acknowledged_risk_ids: RISKS.map(r => r.id),
  } }]);
  assert.equal(ui.ct('apply-dialog'), null);
  assert.match(ui.ct('apply-report').textContent, /Wrote 1 file in the task worktree \/data\/unoone\/task-worktrees\/6f1c .*Your repository was not written\./);
  assert.equal(ui.ct('task-status').textContent, 'Applied to task worktree');
  assert.match(chip(ui, 'apply'), /^Applied 1 file to task worktree \/data\/unoone\/task-worktrees\/6f1c$/);
  assert.equal(ui.ipc('coding_task_apply').length, 1);
});

test('the apply dialog is bound to its view: a commit event closes it; the risk list must match the server risk hash', async t => {
  const ui = await mount(t, { views: { [TASK_ID]: decidedView() } });
  assert.equal(ui.ct('apply-open').disabled, false);
  await ui.click(ui.ct('apply-open'));
  assert.ok(ui.ct('apply-dialog'));
  // Same view_seq: no reload.
  await ui.emit('unoone:coding-task-updated', { task_id: TASK_ID, view_seq: 12 });
  assert.equal(ui.ipc('coding_task_view').length, 1);
  assert.ok(ui.ct('apply-dialog'), 'an unchanged view keeps the dialog');
  bump(ui.state);
  await ui.emit('unoone:coding-task-updated', { task_id: TASK_ID, view_seq: 13 });
  assert.equal(ui.ipc('coding_task_view').length, 2, 'commit event reloads the selected view (no content polling)');
  assert.equal(ui.ct('apply-dialog'), null, 'dialog closed: its hashes belong to an older view');
  assert.match(ui.text(), /The task changed while the apply dialog was open/);
  // Commands that return no TaskView emit view_seq: null; that also reloads.
  await ui.emit('unoone:coding-task-updated', { task_id: TASK_ID, view_seq: null });
  assert.equal(ui.ipc('coding_task_view').length, 3);
  // An event for an unknown task refreshes the list only.
  ui.state.views[OTHER_TASK_ID] = baseView({ task_id: OTHER_TASK_ID });
  await ui.emit('unoone:coding-task-updated', { task_id: OTHER_TASK_ID, view_seq: 1 });
  assert.equal(ui.ipc('coding_task_list').length, 2);
  assert.equal(ui.ipc('coding_task_view').length, 3);
  assert.deepEqual(ui.ipc('coding_task_apply'), []);
  await ui.unmount();

  const mismatched = decidedView({ risks_sha256: sha('a different risk set') });
  const gate = lib.canApply(mismatched, new Set());
  assert.equal(gate.ok, false);
  assert.deepEqual(gate.reasons, ['The displayed risk list does not match the server risk hash.']);
});

test('stale badge resets decisions when content changes (client-detected and server-flagged); the open diff reloads for the new hash', async t => {
  let fileDiffNext = H.tempNew;
  const ui = await mount(t, {
    commands: {
      coding_task_file_diff: args => (args.path === 'temperature.py' ? temperatureDiff(fileDiffNext) : durationDiff()),
      coding_task_review_file: ({ event }, s) => {
        const v = s.views[TASK_ID];
        return bump(s, { diff: v.diff.map(d => (d.path === event.path ? { ...d, decision: event.decision, stale: false } : d)) });
      },
    },
  });
  await ui.click(ui.button('Show diff of temperature.py'));
  await ui.click(ui.button('Accept file temperature.py'));
  await ui.click(ui.button('Show diff of duration.py'));
  await ui.click(ui.button('Reject file duration.py'));
  await ui.click(ui.button('Show diff of temperature.py'));
  assert.equal(ui.ct('apply-open').disabled, false);
  assert.equal(ui.ipc('coding_task_file_diff').length, 3);

  // A repair/copy-out changes temperature.py. Defensive case: the server view
  // still says "accepted" at the NEW hash; the UI must not carry the decision over.
  fileDiffNext = H.tempNew2;
  bump(ui.state, { diff: [
    summary('temperature.py', { base: H.tempBase, next: H.tempNew2, decision: { kind: 'accepted' } }),
    summary('duration.py', { base: H.durBase, next: H.durNew, decision: { kind: 'pending' }, stale: true }),
  ] });
  await ui.emit('unoone:coding-task-updated', { task_id: TASK_ID, view_seq: 15 });
  assert.equal(fileRow(ui, 'temperature.py').dataset.decision, 'pending', 'decision reset on content change');
  assert.match(fileRow(ui, 'temperature.py').textContent, /Undecided.*Stale — content changed, decide again/);
  assert.match(fileRow(ui, 'duration.py').textContent, /Undecided.*Stale — content changed, decide again/, 'server-flagged stale');
  assert.equal(ui.ct('apply-open').disabled, true);
  assert.match(ui.ct('apply-reasons').textContent, /2 files still need your decision\./);
  assert.equal(ui.ipc('coding_task_file_diff').length, 4, 'the open diff reloaded for the new content');
  assert.deepEqual(ui.ipc('coding_task_file_diff')[3], { taskId: TASK_ID, path: 'temperature.py' });
  assert.match(ui.ct('diff-hashes').textContent, new RegExp(H.tempNew2));

  await ui.click(ui.button('Accept file temperature.py'));
  const last = ui.ipc('coding_task_review_file').at(-1).event;
  assert.equal(last.displayed_new_sha256, H.tempNew2, 'the new decision echoes the new displayed hash');
  assert.equal(last.view_seq, 15);
  assert.equal(fileRow(ui, 'temperature.py').dataset.decision, 'accepted');
  assert.doesNotMatch(fileRow(ui, 'temperature.py').textContent, /Stale/);
  assert.deepEqual(ui.ipc('coding_task_apply'), []);

  const v = ui.state.views[TASK_ID];
  assert.deepEqual(lib.staleFiles(v, new Map([['temperature.py', H.tempNew]])), ['temperature.py', 'duration.py'], 'client-detected + server-flagged');
  assert.deepEqual(lib.staleFiles(v, new Map([['temperature.py', H.tempNew2]])), ['duration.py']);
});

test('interrupted steps show explicit options; resolve/resume are never called automatically, only per click', async t => {
  const paused = baseView({
    status: { paused: { reason: 'review_required' } },
    steps: [
      { step_id: 'apply-1', attempt: 1, state: 'interrupted', effect: 'host_write', failure: null },
      { step_id: 'gate-2', attempt: 1, state: 'interrupted', effect: 'pure', failure: null },
    ],
    outcome: { ...baseView().outcome, apply_status: 'interrupted' },
    reconciliation: [
      { step_id: 'apply-1', attempt: 1, effect: 'host_write', observation: { unknown: { per_path: { 'temperature.py': H.tempNew }, identity_ok: true } }, options: ['restore_pre_image', 'mark_manually_resolved', 'abandon'] },
      { step_id: 'gate-2', attempt: 1, effect: 'pure', observation: 'not_applicable', options: ['retry_as_new_attempt', 'abandon'] },
    ],
  });
  const resolveCmd = ({ event }, s) => {
    const v = s.views[TASK_ID];
    return bump(s, { reconciliation: v.reconciliation.map(r => (r.step_id === event.step_id ? { ...r, options: [] } : r)) });
  };
  const ui = await mount(t, {
    views: { [TASK_ID]: paused },
    commands: {
      coding_task_resolve_interrupted: resolveCmd,
      coding_task_resume: (_a, s) => bump(s, { status: 'awaiting_review', reconciliation: [] }),
    },
  });
  await ui.wait(1200);
  assert.deepEqual(ui.commandsCalled(), READS, 'nothing but the three reads runs on its own, even after waiting');
  assert.equal(ui.ct('task-status').textContent, 'Paused — your review of interrupted steps is required');
  const item = step => ui.q(`[data-ct="recon-item"][data-step="${step}"]`);
  assert.match(item('apply-1').textContent, /Worktree state is unknown: 1 path matches neither the pre-image nor the post-image; worktree identity unchanged\./);
  assert.deepEqual([...item('apply-1').querySelectorAll('button')].map(b => b.textContent),
    ['Restore pre-image (worktree write)', 'Mark manually resolved', 'Abandon step']);
  assert.match(item('gate-2').textContent, /never recycled/);
  assert.deepEqual([...item('gate-2').querySelectorAll('button')].map(b => b.textContent), ['Retry as a new attempt', 'Abandon step']);
  const resume = ui.button('Resume task (re-checks admission)');
  assert.equal(resume.disabled, true, 'resume needs every interrupted step resolved');
  assert.equal(ui.ct('apply-open').disabled, true);
  assert.match(ui.ct('apply-reasons').textContent, /Task is paused: resolve the interrupted steps first\./);

  await ui.click([...item('apply-1').querySelectorAll('button')][0]);
  assert.deepEqual(ui.ipc('coding_task_resolve_interrupted'), [{ event: { task_id: TASK_ID, view_seq: 12, step_id: 'apply-1', attempt: 1, resolution: 'restore_pre_image' } }]);
  assert.equal(ui.button('Resume task (re-checks admission)').disabled, true);
  await ui.click([...item('gate-2').querySelectorAll('button')][0]);
  assert.deepEqual(ui.ipc('coding_task_resolve_interrupted')[1], { event: { task_id: TASK_ID, view_seq: 13, step_id: 'gate-2', attempt: 1, resolution: 'retry_as_new_attempt' } });
  assert.deepEqual(ui.ipc('coding_task_resume'), [], 'resolving does not resume');
  assert.equal(ui.button('Resume task (re-checks admission)').disabled, false);
  await ui.click(ui.button('Resume task (re-checks admission)'));
  assert.deepEqual(ui.ipc('coding_task_resume'), [{ event: { task_id: TASK_ID, view_seq: 14 } }]);
  assert.equal(ui.ct('reconciliation'), null);
  assert.deepEqual(ui.ipc('coding_task_apply'), []);
  assert.deepEqual(ui.ipc('coding_task_run_gate'), [], 'no re-run of any step');
});

test('managed preview: URL and "HTTP-level checks only" label, bounded logs with truncation metadata, 1 s cursor polling only while running, window opens only on click', async t => {
  const running = baseView({
    preview: readyPreview(),
    acceptance: [...baseView().acceptance, { id: 'api-ok', text: '/api/trees lists the trees', check: { http: { check_id: 'trees' } }, confirmed_by_user: true }],
    outcome: { ...baseView().outcome, preview_status: { ready: { http: 'passed' } } },
  });
  const chunks = [
    {
      records: [
        { seq: 12, at_ms: 1790000007000, stream: 'stdout', bytes: hex('GET /api/trees 200\n') },
        { seq: 13, at_ms: 1790000007001, stream: 'stderr', bytes: hex('warn: slow disk\n') },
      ],
      next_cursor: 14, truncated_before_cursor: true, first_retained_seq: 12, retained_bytes: 35, cap_bytes: 65536,
      dropped_bytes: 4096, dropped_records: 12, supervisor_dropped_bytes: 512,
    },
    {
      records: [{ seq: 14, at_ms: 1790000008000, stream: 'stdout', bytes: hex('GET / 200\n') }],
      next_cursor: 15, truncated_before_cursor: false, first_retained_seq: 12, retained_bytes: 45, cap_bytes: 65536,
      dropped_bytes: 4096, dropped_records: 12, supervisor_dropped_bytes: 512,
    },
  ];
  const ui = await mount(t, {
    views: { [TASK_ID]: running },
    commands: {
      coding_task_preview_logs: () => chunks.shift() ?? { records: [], next_cursor: 15, truncated_before_cursor: false, first_retained_seq: 12, retained_bytes: 45, cap_bytes: 65536, dropped_bytes: 4096, dropped_records: 12, supervisor_dropped_bytes: 512 },
      coding_task_http_checks: (_a, s) => bump(s),
      coding_task_stop_preview: (_a, s) => bump(s, {
        preview: { ...readyPreview(), status: 'stopped', descriptor: null, capability_url: null },
        outcome: { ...s.views[TASK_ID].outcome, preview_status: 'stopped' },
      }),
      'plugin:window|get_all_windows': () => [],
      'plugin:webview|create_webview_window': () => null,
      'plugin:window|set_focus': () => null,
    },
  });
  assert.deepEqual(ui.commandsCalled(), READS, 'no log poll and no window at mount');
  assert.equal(ui.ct('preview-honesty').textContent, 'HTTP-level checks only — not browser-rendered');
  assert.match(ui.ct('preview').textContent, /Browser rendering: not verified by UnoOne — HTTP checks only/);
  assert.equal(ui.ct('preview-url').textContent, PREVIEW_URL);
  assert.equal(ui.ct('preview-status').textContent, 'Ready after 139 ms');
  assert.equal(ui.ct('http-level').textContent, 'HTTP-level only (http level)');
  assert.match(ui.q('[data-ct="http-result"][data-check="trees"]').textContent, /trees200passed4 ms/);
  assert.match(ui.q('[data-criterion="api-ok"]').textContent, /Passed \(HTTP 200, HTTP level only\)/);
  assert.equal(chip(ui, 'preview'), 'Server ready · HTTP checks passed');

  await ui.wait(1150);
  assert.deepEqual(ui.ipc('coding_task_preview_logs'), [{ taskId: TASK_ID, cursor: 0, limit: 200 }]);
  const meta = ui.ct('log-meta').textContent;
  assert.match(meta, /Server ring: 35 B retained of 64\.0 KiB cap/);
  assert.match(meta, /dropped 4096 bytes \/ 12 records/);
  assert.match(meta, /rate-limited before the host: 512 bytes/);
  assert.match(meta, /first retained record #12/);
  assert.match(meta, /earlier records were evicted before they were shown/);
  assert.equal(ui.q('[data-ct="preview-logs"] pre').textContent, 'GET /api/trees 200\n[stderr] warn: slow disk\n');
  await ui.wait(1050);
  assert.deepEqual(ui.ipc('coding_task_preview_logs')[1], { taskId: TASK_ID, cursor: 14, limit: 200 }, 'cursor advances');
  assert.match(ui.q('[data-ct="preview-logs"] pre').textContent, /GET \/ 200\n$/);

  assert.equal(ui.calls.filter(c => c.command.startsWith('plugin:webview')).length, 0, 'no window before the click');
  await ui.click(ui.button('Open preview'));
  await ui.flush();
  const created = ui.ipc('plugin:webview|create_webview_window');
  assert.equal(created.length, 1);
  assert.equal(created[0].options.label, 'task-preview');
  assert.equal(created[0].options.url, PREVIEW_URL);
  assert.match(created[0].options.title, /HTTP-level checks only/);
  assert.deepEqual(ui.ipc('plugin:window|set_focus'), [{ label: 'task-preview' }]);
  assert.match(ui.text(), /Preview window opened\./);

  await ui.click(ui.button('Run HTTP checks'));
  assert.deepEqual(ui.ipc('coding_task_http_checks'), [{ taskId: TASK_ID }]);
  await ui.click(ui.button('Stop preview'));
  assert.deepEqual(ui.ipc('coding_task_stop_preview'), [{ taskId: TASK_ID }]);
  assert.equal(ui.ct('preview-status').textContent, 'Stopped');
  assert.equal(ui.button('Open preview').disabled, true);
  const polls = ui.ipc('coding_task_preview_logs').length;
  await ui.wait(1300);
  assert.equal(ui.ipc('coding_task_preview_logs').length, polls, 'polling stops when the preview is not running');
});

test('preview start is a click, shows "Starting…", and startup failure is shown with its reason', async t => {
  const pending = deferred();
  const notStarted = baseView({
    preview: { ...readyPreview(), status: 'not_started', descriptor: null, capability_url: null, http_checks: null },
    outcome: { ...baseView().outcome, preview_status: 'not_started' },
  });
  const ui = await mount(t, {
    views: { [TASK_ID]: notStarted },
    commands: { coding_task_start_preview: () => pending.promise },
  });
  assert.equal(ui.ct('preview-status').textContent, 'Not started');
  assert.equal(ui.button('Open preview').disabled, true);
  await ui.click(ui.button('Start preview'));
  assert.deepEqual(ui.ipc('coding_task_start_preview'), [{ event: { task_id: TASK_ID, view_seq: 12 } }]);
  assert.equal(ui.ct('preview-status').textContent, 'Starting…');
  const failedDescriptor = { ...readyPreview().descriptor, bridge_port: 0, ready: { startup_failed: { reason: 'timeout' } } };
  ui.state.views[TASK_ID] = baseView({
    view_seq: 13,
    preview: { ...readyPreview(), status: 'startup_failed', descriptor: failedDescriptor, capability_url: null, http_checks: null },
    outcome: { ...baseView().outcome, preview_status: 'startup_failed' },
  });
  pending.resolve({ ...ui.state.views[TASK_ID].preview });
  await ui.flush();
  await ui.flush();
  assert.equal(ui.ipc('coding_task_view').length, 2, 'view reloaded after start');
  assert.equal(ui.ct('preview-status').textContent, 'Startup failed (timed out waiting for readiness)');
  assert.equal(chip(ui, 'preview'), 'Startup failed');
  assert.equal(ui.ct('preview-url'), null);
  assert.equal(ui.button('Open preview').disabled, true);
});

test('preview URL validation: only http://127.0.0.1:<port>/__pai/open?t=<32 hex>; invalid URLs are rejected and never opened', async t => {
  const token = '0123456789abcdef'.repeat(2);
  const cases = [
    [PREVIEW_URL, true],
    [`http://127.0.0.1:1/__pai/open?t=${token}`, true],
    [`http://127.0.0.1:65535/__pai/open?t=${token}`, true],
    [`http://127.0.0.1:0/__pai/open?t=${token}`, false],
    [`http://127.0.0.1:65536/__pai/open?t=${token}`, false],
    [`http://localhost:41873/__pai/open?t=${token}`, false],
    [`http://127.0.0.2:41873/__pai/open?t=${token}`, false],
    [`https://127.0.0.1:41873/__pai/open?t=${token}`, false],
    [`http://127.0.0.1.evil.test:41873/__pai/open?t=${token}`, false],
    [`http://[::1]:41873/__pai/open?t=${token}`, false],
    [`http://127.0.0.1:41873/__pai/open?t=${token.toUpperCase()}`, false],
    [`http://127.0.0.1:41873/__pai/open?t=${token}&x=1`, false],
    [`http://127.0.0.1:41873/__pai/open?t=${token.slice(1)}`, false],
    [`http://127.0.0.1:41873/other?t=${token}`, false],
    [`http://user@127.0.0.1:41873/__pai/open?t=${token}`, false],
    ['javascript:alert(1)', false],
    ['', false],
    [null, false],
    [42, false],
  ];
  for (const [url, ok] of cases) assert.equal(lib.validatePreviewUrl(url).ok, ok, String(url));

  const bad = `http://localhost:41873/__pai/open?t=${token}`;
  const ui = await mount(t, {
    views: { [TASK_ID]: baseView({ preview: readyPreview({ capability_url: bad }), outcome: { ...baseView().outcome, preview_status: { ready: { http: 'not_run' } } } }) },
    commands: { coding_task_preview_logs: () => ({ records: [], next_cursor: 0, first_retained_seq: 0, dropped_bytes: 0, dropped_records: 0, supervisor_dropped_bytes: 0 }) },
  });
  assert.equal(ui.ct('preview-url'), null);
  assert.match(ui.ct('preview-url-rejected').textContent, /Preview URL rejected .*Only http:\/\/127\.0\.0\.1 capability URLs are opened\./);
  assert.equal(ui.button('Open preview').disabled, true);
  await ui.click(ui.button('Open preview'));
  // The window helper re-validates before creating anything.
  assert.equal(await previewWindowLib.openTaskPreviewWindow(bad), false);
  assert.equal(ui.calls.filter(c => c.command.startsWith('plugin:')).length, 0, 'no window IPC for an invalid URL');
});

test('preview window helper retargets an existing task-preview window (close, then recreate) and cleans its listener', async t => {
  const ui = await mount(t, {
    commands: {
      'plugin:window|get_all_windows': () => ['main', 'task-preview'],
      'plugin:window|close': () => { setTimeout(() => { void emitRaw('tauri://destroyed', null); }, 5); return null; },
      'plugin:webview|create_webview_window': () => null,
      'plugin:window|set_focus': () => null,
    },
  });
  const before = ui.listeners();
  assert.equal(await previewWindowLib.openTaskPreviewWindow(PREVIEW_URL), true);
  assert.deepEqual(ui.ipc('plugin:window|close'), [{ label: 'task-preview' }]);
  assert.equal(ui.ipc('plugin:webview|create_webview_window')[0].options.url, PREVIEW_URL);
  assert.equal(ui.listeners(), before, 'destroyed-listener removed');
});

test('Windows Unsupported: run, preview and apply are disabled with the reason while diff, review and patch export stay usable', async t => {
  const reason = 'no runtime-verified isolation backend on this OS (IsolationUnavailable)';
  const unsupported = { state: 'unsupported', reason };
  const view = decidedView({
    capability: unsupported,
    preview: { ...readyPreview(), status: 'not_started', descriptor: null, capability_url: null, http_checks: null },
    outcome: { ...baseView().outcome, preview_status: 'not_started' },
  });
  const patch = '--- a/temperature.py\n+++ b/temperature.py\n@@ -4 +4 @@\n-    return celsius + 273\n+    return celsius + 273.15\n';
  const ui = await mount(t, {
    capability: unsupported,
    views: { [TASK_ID]: view },
    commands: {
      coding_task_file_diff: args => (args.path === 'temperature.py' ? temperatureDiff() : durationDiff()),
      coding_task_review_file: ({ event }, s) => bump(s, { diff: s.views[TASK_ID].diff.map(d => (d.path === event.path ? { ...d, decision: event.decision } : d)) }),
      coding_task_export_patch: () => patch,
    },
  });
  assert.equal(ui.ct('capability').textContent, 'Isolation: Unsupported');
  assert.equal(ui.ct('capability-detail').textContent, reason);
  assert.match(ui.ct('exec-blocked').textContent, new RegExp(`${reason.replace(/[()]/g, '\\$&')}.*Running checks, the preview and\\s+Apply are disabled`));
  for (const label of ['Run checks', 'Run checks on accepted changes', 'Start preview', 'Open preview', 'Run HTTP checks']) {
    assert.equal(ui.button(label).disabled, true, `${label} disabled`);
    await ui.click(ui.button(label));
  }
  assert.equal(ui.ct('apply-open').disabled, true);
  assert.match(ui.ct('apply-reasons').textContent, new RegExp(`Apply unavailable on this system: ${reason.replace(/[()]/g, '\\$&')}`));
  await ui.click(ui.ct('apply-open'));
  assert.equal(ui.ct('apply-dialog'), null);
  // The diff, the step log and review stay usable (LedgerOnly).
  assert.ok(ui.ct('steps'));
  await ui.click(ui.button('Show diff of temperature.py'));
  assert.equal(ui.qa('[data-ct="diff-pane"] table.ct-diff tr').length, 4, 'diff visible');
  assert.equal(ui.button('Accept file temperature.py').disabled, false);
  await ui.click(ui.button('Reject file temperature.py'));
  assert.equal(ui.ipc('coding_task_review_file').length, 1);
  await ui.click(ui.button('Export patch'));
  assert.deepEqual(ui.ipc('coding_task_export_patch'), [{ taskId: TASK_ID }]);
  assert.equal(ui.q('[data-ct="patch-dialog"] textarea').value, patch);
  await ui.click(ui.button('Close'));
  assert.equal(ui.ct('patch-dialog'), null);
  assert.equal(ui.calls.filter(c => /run_gate|start_preview|http_checks|coding_task_apply|plugin:webview/.test(c.command)).length, 0, 'nothing executable was invoked');
});

test('Run checks sends the gate target, shows progress, and real results replace the chips; errors are shown, never swallowed', async t => {
  const pending = deferred();
  const ui = await mount(t, {
    commands: {
      coding_task_run_gate: args => (args.target === 'current' ? pending.promise : Promise.reject('AdmissionDenied("attestation mismatch")')),
      coding_task_file_diff: () => temperatureDiff(),
      coding_task_review_file: () => Promise.reject('Conflict'),
    },
  });
  await ui.click(ui.button('Run checks'));
  assert.deepEqual(ui.ipc('coding_task_run_gate'), [{ taskId: TASK_ID, target: 'current' }]);
  assert.equal(ui.buttons('Running checks…').length, 1);
  assert.equal(ui.button('Run checks on accepted changes').disabled, true, 'one action at a time');
  const passedGate = { ...baseView().gates[0], gate_run_id: 'gate-0002', commands: [{ ...baseView().gates[0].commands[0], status: 0, stderr_total_bytes: 0, stderr_retained_bytes: 0, truncated: false, excerpt: '' }] };
  ui.state.views[TASK_ID] = baseView({
    view_seq: 13,
    gates: [...baseView().gates, passedGate],
    outcome: { ...baseView().outcome, build_status: { passed: { gate: 'gate-0002' } }, test_status: { passed: { gate: 'gate-0002' } }, goal_status: 'checks_passed_pending_review' },
  });
  pending.resolve(structuredClone(ui.state.views[TASK_ID]));
  await ui.flush();
  assert.equal(chip(ui, 'build'), 'Passed (gate gate-000)');
  assert.equal(chip(ui, 'goal'), 'Checks passed — awaiting your review');
  assert.deepEqual(ui.cts('exit-code').map(e => e.textContent), ['exit 0', 'exit 2'], 'newest gate first, real exit codes');
  assert.match(ui.q('[data-criterion="build-ok"]').textContent, /Passed \(exit 0\)/);

  await ui.click(ui.button('Run checks on accepted changes'));
  assert.deepEqual(ui.ipc('coding_task_run_gate')[1], { taskId: TASK_ID, target: 'accepted_composition' });
  assert.equal(ui.q('[role="alert"]').textContent, 'Run checks on accepted changes failed: AdmissionDenied("attestation mismatch")');
  await ui.click(ui.button('Show diff of temperature.py'));
  await ui.click(ui.button('Accept file temperature.py'));
  assert.equal(ui.q('[role="alert"]').textContent, 'Accept file failed: Conflict');
  assert.equal(fileRow(ui, 'temperature.py').dataset.decision, 'pending', 'a refused decision is not shown as made');
});

test('cancel and revert-applied need a second confirming click; export patch shows copyable text', async t => {
  const applied = decidedView({
    status: 'applied',
    outcome: { ...baseView().outcome, apply_status: { applied: { files: ['temperature.py'], worktree: '/data/unoone/task-worktrees/6f1c' } } },
  });
  const ui = await mount(t, {
    views: { [TASK_ID]: applied },
    commands: {
      coding_task_revert_applied: (_a, s) => {
        bump(s, { status: 'awaiting_review', outcome: { ...s.views[TASK_ID].outcome, apply_status: 'reverted' } });
        return { step_id: 'revert-1', files: ['temperature.py'], worktree: { path: '/data/unoone/task-worktrees/6f1c', dev: 1, ino: 2, created_at_ms: 3 }, post_image: { 'temperature.py': H.tempBase } };
      },
      coding_task_cancel: (_a, s) => { bump(s, { status: 'cancelled' }); return null; },
    },
  });
  assert.equal(ui.ct('apply-open').disabled, true, 'already applied');
  await ui.click(ui.button('Revert applied changes…'));
  assert.deepEqual(ui.ipc('coding_task_revert_applied'), []);
  await ui.click(ui.button('Yes, revert the task worktree'));
  assert.deepEqual(ui.ipc('coding_task_revert_applied'), [{ event: { task_id: TASK_ID, view_seq: 12 } }]);
  assert.equal(chip(ui, 'apply'), 'Reverted');
  await ui.click(ui.button('Cancel task…'));
  assert.deepEqual(ui.ipc('coding_task_cancel'), []);
  await ui.click(ui.button('Keep the task'));
  await ui.click(ui.button('Cancel task…'));
  await ui.click(ui.button('Yes, cancel this task'));
  assert.deepEqual(ui.ipc('coding_task_cancel'), [{ taskId: TASK_ID }]);
  assert.equal(ui.ct('task-status').textContent, 'Cancelled');
  assert.equal(ui.button('Cancel task…').disabled, true);
});

test('task list: an empty vault shows an empty state and reads nothing else', async t => {
  const empty = await mount(t, { views: {} });
  assert.deepEqual(empty.commandsCalled(), ['coding_task_capability', 'coding_task_list']);
  assert.match(empty.text(), /No coding tasks in this vault/);
});

test('task list: several tasks get a labelled selector, the newest readable one opens, and switching loads only that view', async t => {
  const other = baseView({ task_id: OTHER_TASK_ID, objective: 'Window slicing helper', view_seq: 3 });
  const ui = await mount(t, {
    views: { [TASK_ID]: baseView(), [OTHER_TASK_ID]: other },
    list: [taskSummary(baseView(), { created_at_ms: 1790000000000 }), taskSummary(other, { created_at_ms: 1790000100000 }), { task_id: 'unreadable', objective_excerpt: '', status: null, created_at_ms: 1790000200000, view_seq: 0, readable: false }],
  });
  assert.deepEqual(ui.ipc('coding_task_view'), [{ taskId: OTHER_TASK_ID }], 'newest readable task is opened');
  const select = ui.q('select[aria-label="Select coding task"]');
  assert.ok(select);
  assert.equal(select.querySelector('option[value="unreadable"]').disabled, true);
  await act(async () => {
    Object.getOwnPropertyDescriptor(ui.dom.window.HTMLSelectElement.prototype, 'value').set.call(select, TASK_ID);
    select.dispatchEvent(new ui.dom.window.Event('change', { bubbles: true }));
  });
  await ui.flush();
  assert.deepEqual(ui.ipc('coding_task_view')[1], { taskId: TASK_ID });
  assert.equal(ui.ct('objective').textContent, baseView().objective);
  assert.equal(ui.calls.filter(c => MUTATIONS.test(c.command)).length, 0);
});

test('StrictMode mount: still no mutation without a click and every listener is cleaned up', async t => {
  const ui = await mount(t, { strict: true });
  assert.equal(ui.calls.filter(c => MUTATIONS.test(c.command)).length, 0);
  assert.ok(ui.calls.every(c => READS.includes(c.command)));
  assert.equal(ui.listeners(), 1);
  await ui.unmount();
  assert.equal(ui.listeners(), 0);
});

test('accessibility: every button and checkbox has an accessible name, dialogs are labelled, and the scoped 44px target rule exists', async t => {
  const rich = baseView({
    preview: readyPreview(),
    outcome: { ...baseView().outcome, preview_status: { ready: { http: 'passed' } } },
    reconciliation: [{ step_id: 'gate-2', attempt: 1, effect: 'pure', observation: 'not_applicable', options: ['retry_as_new_attempt', 'abandon'] }],
    diff: [
      summary('temperature.py', { base: H.tempBase, next: H.tempNew, decision: { kind: 'accepted' } }),
      summary('duration.py', { base: H.durBase, next: H.durNew, decision: { kind: 'rejected' } }),
    ],
  });
  const ui = await mount(t, {
    views: { [TASK_ID]: rich },
    commands: { coding_task_file_diff: () => temperatureDiff(), coding_task_preview_logs: () => ({ records: [], next_cursor: 0, first_retained_seq: 0, dropped_bytes: 0, dropped_records: 0, supervisor_dropped_bytes: 0 }) },
  });
  await ui.click(ui.button('Show diff of temperature.py'));
  const nameOf = el => (el.getAttribute('aria-label') || el.textContent || '').trim();
  for (const b of ui.qa('button')) assert.ok(nameOf(b).length > 0, `button has a name: ${b.outerHTML.slice(0, 80)}`);
  for (const box of ui.qa('input[type="checkbox"]')) {
    assert.ok(box.getAttribute('aria-label') || box.closest('label')?.textContent.trim(), 'checkbox labelled');
  }
  for (const el of ui.qa('button, input, select')) {
    assert.ok(el.closest('.coding-task-view, .ct-dialog, .ct-header-actions'), 'every control sits under a 44px-scoped container');
  }
  const css = readFileSync(join(frontendRoot, 'src/index.css'), 'utf8');
  const rule = css.slice(css.indexOf('/* ===== Coding Task View (Stage 5) ====='));
  assert.match(rule, /\.coding-task-view \.btn,\n\.ct-dialog \.btn,\n\.ct-header-actions \.btn,[^{]*\{\n  min-height: 44px;\n  min-width: 44px;/);
  assert.match(rule, /\.coding-task-view \.ct-check,\n\.ct-dialog \.ct-check \{[^}]*min-height: 44px;/);
  assert.match(rule, /\.ct-details summary \{[^}]*min-height: 44px;/);
});

test('accessibility: apply and patch dialogs are modal, labelled, keyboard-closable, and every risk checkbox is named', async t => {
  const ui = await mount(t, { views: { [TASK_ID]: decidedView() }, commands: { coding_task_export_patch: () => 'patch text' } });
  await ui.click(ui.ct('apply-open'));
  const dialog = ui.ct('apply-dialog');
  assert.equal(dialog.getAttribute('aria-modal'), 'true');
  assert.equal(ui.dom.window.document.getElementById(dialog.getAttribute('aria-labelledby')).textContent, 'Apply accepted changes to the task worktree');
  const boxes = ui.qa('[data-ct="apply-risk"] input');
  assert.equal(boxes.length, RISKS.length);
  for (const box of boxes) assert.match(box.getAttribute('aria-label'), /^Acknowledge risk /);
  for (const b of ui.qa('.ct-dialog button')) assert.ok(b.textContent.trim(), 'dialog buttons are labelled');
  await act(async () => dialog.dispatchEvent(new ui.dom.window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
  await ui.flush();
  assert.equal(ui.ct('apply-dialog'), null, 'Escape closes the dialog without applying');
  await ui.click(ui.button('Export patch'));
  const patch = ui.ct('patch-dialog');
  assert.equal(patch.getAttribute('aria-modal'), 'true');
  assert.equal(ui.dom.window.document.getElementById(patch.getAttribute('aria-labelledby')).textContent, 'Patch of accepted changes');
  assert.equal(patch.querySelector('textarea').getAttribute('aria-label'), 'Patch text');
  assert.equal(patch.querySelector('textarea').readOnly, true);
  await ui.click(ui.button('Close'));
  assert.deepEqual(ui.ipc('coding_task_apply'), []);
});

test('navigation: the Sidebar offers "Coding Task" and App renders CodingTaskView for it', async t => {
  const navigated = [];
  const ui = await mount(t, {
    component: 'sidebar',
    props: { currentView: 'chat', onNavigate: view => navigated.push(view), onLock: () => {} },
  });
  const item = ui.qa('.nav-item').find(el => el.textContent.trim() === 'Coding Task');
  assert.ok(item, 'nav item present');
  await ui.click(item);
  assert.deepEqual(navigated, ['coding']);
  assert.deepEqual(ui.commandsCalled(), ['get_vault_status']);
  const app = readFileSync(join(frontendRoot, 'src/App.tsx'), 'utf8');
  assert.match(app, /import \{ CodingTaskView \} from '\.\/components\/CodingTaskView';/);
  assert.match(app, /case 'coding':\n\s+return <CodingTaskView \/>;/);
});

test('IPC plumbing: the frozen §8.2 command names, and the camelCase conversion is byte-identical to src/lib/tauri.ts', () => {
  assert.deepEqual(Object.values(lib.CODING_TASK_COMMANDS).sort(), [
    'coding_task_apply', 'coding_task_cancel', 'coding_task_capability', 'coding_task_confirm_plan',
    'coding_task_export_patch', 'coding_task_file_diff', 'coding_task_http_checks', 'coding_task_list',
    'coding_task_open', 'coding_task_preview_logs', 'coding_task_resolve_interrupted', 'coding_task_resume',
    'coding_task_revert_applied', 'coding_task_revert_file', 'coding_task_review_file', 'coding_task_run_gate',
    'coding_task_start_preview', 'coding_task_stop_preview', 'coding_task_view',
  ]);
  assert.equal(lib.CODING_TASK_UPDATED_EVENT, 'unoone:coding-task-updated');
  const extract = (src, start) => {
    const at = src.indexOf(start);
    assert.ok(at >= 0, start);
    return src.slice(at, src.indexOf('\n}\n', at) + 2);
  };
  const tauri = readFileSync(join(frontendRoot, 'src/lib/tauri.ts'), 'utf8');
  const mine = readFileSync(join(frontendRoot, 'src/lib/codingTask.ts'), 'utf8');
  for (const start of ['function snakeToCamelKey(', 'async function invoke<T>(']) {
    assert.equal(extract(mine, start), extract(tauri, start), `${start} identical to tauri.ts`);
  }
});

test('typed wrappers: exact argument shape of every §8.2 command as it reaches IPC', async t => {
  const answers = {
    coding_task_open: () => baseView(), coding_task_file_diff: () => temperatureDiff(), coding_task_confirm_plan: () => baseView(),
    coding_task_run_gate: () => baseView(), coding_task_review_file: () => baseView(), coding_task_revert_file: () => baseView(),
    coding_task_apply: () => ({}), coding_task_revert_applied: () => ({}), coding_task_start_preview: () => readyPreview(),
    coding_task_stop_preview: () => baseView(), coding_task_preview_logs: () => ({}), coding_task_http_checks: () => baseView(),
    coding_task_resolve_interrupted: () => baseView(), coding_task_resume: () => baseView(), coding_task_cancel: () => null,
    coding_task_export_patch: () => '',
  };
  const ui = await mount(t, { commands: answers });
  const api = lib.codingTaskApi;
  const ev = { task_id: TASK_ID, view_seq: 7 };
  const open = {
    root: '/home/dev/projects/weather-tools', files: ['temperature.py'], primary: 'temperature.py', oracle_files: [],
    oracle_visibility: 'hidden', objective: 'o', acceptance: [], gate_plan: { schema: 's' }, preview: null,
    repair: { max_attempts: 3, max_total_gate_ms: 600000 }, allowed_new_prefixes: [],
  };
  const review = { ...ev, path: 'temperature.py', decision: { kind: 'accepted' }, displayed_base_sha256: H.tempBase, displayed_new_sha256: H.tempNew };
  const applyEv = { ...ev, displayed_change_set_sha256: H.changeSet, displayed_risks_sha256: riskHashOf(RISKS), acknowledged_risk_ids: ['checks.failing'] };
  const reconcile = { ...ev, step_id: 'apply-1', attempt: 2, resolution: 'confirm_observation' };
  await api.open(open);
  await api.fileDiff(TASK_ID, 'temperature.py');
  await api.confirmPlan({ ...ev, revision: 3 });
  await api.runGate(TASK_ID, 'accepted_composition');
  await api.reviewFile(review);
  await api.revertFile({ ...ev, path: 'duration.py', displayed_new_sha256: null });
  await api.apply(applyEv);
  await api.revertApplied(ev);
  await api.startPreview(ev);
  await api.stopPreview(TASK_ID);
  await api.previewLogs(TASK_ID, 14, 200);
  await api.httpChecks(TASK_ID);
  await api.resolveInterrupted(reconcile);
  await api.resume(ev);
  await api.cancel(TASK_ID);
  await api.exportPatch(TASK_ID);
  assert.deepEqual(ui.calls.slice(READS.length), [
    { command: 'coding_task_open', args: { request: open } },
    { command: 'coding_task_file_diff', args: { taskId: TASK_ID, path: 'temperature.py' } },
    { command: 'coding_task_confirm_plan', args: { event: { ...ev, revision: 3 } } },
    { command: 'coding_task_run_gate', args: { taskId: TASK_ID, target: 'accepted_composition' } },
    { command: 'coding_task_review_file', args: { event: review } },
    { command: 'coding_task_revert_file', args: { event: { ...ev, path: 'duration.py', displayed_new_sha256: null } } },
    { command: 'coding_task_apply', args: { event: applyEv } },
    { command: 'coding_task_revert_applied', args: { event: ev } },
    { command: 'coding_task_start_preview', args: { event: ev } },
    { command: 'coding_task_stop_preview', args: { taskId: TASK_ID } },
    { command: 'coding_task_preview_logs', args: { taskId: TASK_ID, cursor: 14, limit: 200 } },
    { command: 'coding_task_http_checks', args: { taskId: TASK_ID } },
    { command: 'coding_task_resolve_interrupted', args: { event: reconcile } },
    { command: 'coding_task_resume', args: { event: ev } },
    { command: 'coding_task_cancel', args: { taskId: TASK_ID } },
    { command: 'coding_task_export_patch', args: { taskId: TASK_ID } },
  ]);
});

test('pure helpers: SHA-256 and risk-set hash match node:crypto, decisions decode both serde shapes, canApply matrix, bounded UI log', () => {
  for (const input of ['', 'abc', 'a'.repeat(55), 'a'.repeat(56), 'a'.repeat(64), 'a'.repeat(119), 'ünïcødé ✓ 𝄞', JSON.stringify(RISKS)]) {
    assert.equal(lib.sha256Hex(input), createHash('sha256').update(input, 'utf8').digest('hex'), `sha256(${input.length})`);
  }
  assert.equal(lib.riskSetHash([]), sha('[]'));
  assert.equal(lib.riskSetHash(RISKS), riskHashOf(RISKS));
  assert.equal(lib.riskSetHash([...RISKS].reverse().concat(RISKS[0])), riskHashOf(RISKS), 'order- and duplicate-insensitive (BTreeSet)');
  assert.equal(lib.riskSetHash([{ id: 'b' }, { id: 'a' }]), sha('["a","b"]'));

  assert.deepEqual(['unsupported', 'supported_unverified', 'runtime_verified'].map(state => lib.capabilityInfo(
    state === 'unsupported' ? { state, reason: 'r' } : state === 'runtime_verified' ? { state, workspace_profile_sha256: 'f'.repeat(64), probed_at_ms: 1 } : { state },
  )).map(i => [i.label, i.executionBlocked]), [
    ['Isolation: Unsupported', true], ['Isolation: Supported, unverified', false], ['Isolation: Runtime verified', false],
  ], 'three capability states are distinct; only Unsupported blocks execution');
  assert.equal(lib.capabilityInfo(null).executionBlocked, true, 'unknown capability fails closed');
  assert.deepEqual(lib.normalizeDecision({ kind: 'accepted' }), { kind: 'accepted' });
  assert.deepEqual(lib.normalizeDecision('rejected'), { kind: 'rejected' }, "A's Pass-1 stand-in shape");
  assert.deepEqual(lib.normalizeDecision({ partially_accepted: { hunks: [0, 2], composed_sha256: 'c' } }), { kind: 'partially_accepted', hunks: [0, 2], composed_sha256: 'c' });
  assert.deepEqual(lib.normalizeDecision({ kind: 'approved_by_model' }), { kind: 'pending' }, 'unknown decisions fail closed');
  assert.deepEqual(lib.normalizeDecision(undefined), { kind: 'pending' });

  const ok = decidedView();
  assert.deepEqual(lib.canApply(ok, new Set()), { ok: true, reasons: [] });
  assert.equal(lib.canApply(ok, new Set(['temperature.py'])).ok, false, 'a stale decision blocks apply');
  assert.deepEqual(lib.canApply(decidedView({ diff: [summary('temperature.py', { base: H.tempBase, next: H.tempNew, decision: { kind: 'rejected' } })] }), new Set()).reasons, ['No accepted file: nothing to apply.']);
  assert.match(lib.canApply(decidedView({ status: 'running' }), new Set()).reasons.join(' '), /A run is in progress\./);
  assert.match(lib.canApply(decidedView({ change_set_sha256: '' }), new Set()).reasons.join(' '), /no change-set or risk hash/);
  assert.match(lib.canApply(decidedView({ diff: [] }), new Set()).reasons.join(' '), /No changed files\./);

  assert.equal(lib.decodeLogBytes(hex('héllo\n')), 'héllo\n');
  assert.equal(lib.decodeLogBytes([104, 105]), 'hi', "A's stand-in byte-array shape");
  assert.equal(lib.decodeLogBytes('abc'), '[undecodable log bytes]');
  let state = lib.emptyLogState();
  for (let i = 0; i < 5; i += 1) {
    state = lib.appendLogChunk(state, {
      records: Array.from({ length: 100 }, (_, k) => ({ seq: i * 100 + k, stream: 'stdout', bytes: hex(`line ${i * 100 + k}\n`) })),
      next_cursor: (i + 1) * 100, first_retained_seq: 0, dropped_bytes: 0, dropped_records: 0, supervisor_dropped_bytes: 0,
    });
  }
  assert.equal(state.lines.length, lib.UI_LOG_MAX_RECORDS);
  assert.equal(state.uiDroppedRecords, 500 - lib.UI_LOG_MAX_RECORDS);
  assert.equal(state.lines[0].seq, 100);
  assert.equal(state.cursor, 500);
  const big = lib.appendLogChunk(lib.emptyLogState(), {
    records: [{ seq: 0, stream: 'stdout', bytes: hex('x'.repeat(40000)) }, { seq: 1, stream: 'stdout', bytes: hex('y'.repeat(40000)) }],
    next_cursor: 2, first_retained_seq: 0, dropped_bytes: 0, dropped_records: 0, supervisor_dropped_bytes: 0,
  });
  assert.deepEqual(big.lines.map(l => l.seq), [1], 'character cap keeps the newest records');
  assert.equal(big.uiDroppedRecords, 1);
});
