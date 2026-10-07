import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { after, test } from 'node:test';
import { selectChatContext } from '../src/lib/chatContext.ts';

// Real unchanged std-only Harness core + production helper, with explicit counted
// test providers. Not native Tauri IPC, real vault/model, or device acceptance.
const directory = mkdtempSync(join(tmpdir(), 'unoone-context-core-'));
after(() => rmSync(directory, { recursive: true, force: true }));
const installedRust = join(homedir(), '.cargo', 'bin', process.platform === 'win32' ? 'rustc.exe' : 'rustc');
const rustc = process.env.RUSTC || (existsSync(installedRust) ? installedRust : 'rustc');
const library = join(directory, 'libunoone_context_core.rlib');
const binary = join(directory, process.platform === 'win32' ? 'context-core.exe' : 'context-core');
const core = fileURLToPath(new URL('../../../../vendor/inbharat-harness/crates/core/src/lib.rs', import.meta.url));
const probe = fileURLToPath(new URL('./context-core-probe.rs', import.meta.url));
let sequence = 0;
function pair(user, assistant) {
  const pairId = `fixture-${++sequence}`;
  const context = { provenance: 'archive', session_id: 'fixture', pair_id: pairId };
  return [
    { id: `${pairId}-u`, role: 'user', content: user, context: { ...context, user_text: user } },
    { id: `${pairId}-a`, role: 'assistant', content: assistant, context: { ...context } },
  ];
}
const fixtures = [
  ['one-pair.tsv', pair('Create the Small Giant knowledge-base plan', 'Small Giant needs a reviewed queue.')],
  ['utf8-pair.tsv', pair('Create the Small Giant knowledge-base plan ' + '漢字🦀'.repeat(3000), '回答🦀'.repeat(3000))],
  ['ten-pairs.tsv', Array.from({ length: 10 }, (_, i) => pair(i === 0 ? 'Create the Small Giant knowledge-base plan' : 'continue', `milestone-${i}: ` + 'x'.repeat(300))).flat()],
  ['default-edge.tsv', [...pair('🦀 Create Small Giant knowledge-base plan', 'OLD_ORPHAN_ASSISTANT'),
    ...Array.from({ length: 3 }, () => pair('please '.repeat(284) + 'continue!!!', 'a'.repeat(6000))).flat()]],
];
for (const [name, messages] of fixtures) {
  const selected = selectChatContext({ userText: 'Continue Small Giant', hasAttachments: false,
    messages, activeTask: null, sessionId: 'new', newTaskId: 'fixture' });
  assert.ok(selected.history.length >= 2 && selected.history.length % 2 === 0);
  assert.ok(selected.history.every(({ content }) => content.trim() && !/[\t\n\r]/u.test(content)));
  if (name === 'default-edge.tsv') {
    assert.equal(selected.history.length, 6);
    assert.ok(!selected.history.some(({ content }) => content.includes('OLD_ORPHAN')));
  }
  writeFileSync(join(directory, name), selected.history.map(({ role, content }) => `${role}\t${content}`).join('\n') + '\n');
}

test('production context helper agrees with actual Core memory contract and preserves selected pairs', () => {
  try {
    execFileSync(rustc, ['--edition=2021', '--crate-name', 'inbharat_harness_core', '--crate-type', 'rlib',
      '--cfg', 'feature="test-providers"', core, '-o', library], { stdio: 'pipe', timeout: 90000 });
    execFileSync(rustc, ['--edition=2021', '--test', '-D', 'warnings', probe,
      '--extern', `inbharat_harness_core=${library}`, '-o', binary], { stdio: 'pipe', timeout: 90000 });
    const result = execFileSync(binary, ['--nocapture', '--test-threads=1'], {
      encoding: 'utf8', env: { ...process.env, CORE_TEST_FIXTURES: directory }, timeout: 90000,
    });
    console.log(result);
    assert.match(result, /test result: ok\. 44 passed; 0 failed/);
  } catch (error) {
    console.error(error.stdout?.toString() ?? '', error.stderr?.toString() ?? '');
    throw error;
  }
});
