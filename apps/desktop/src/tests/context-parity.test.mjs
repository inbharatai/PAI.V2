import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { after, test } from 'node:test';
import { selectChatContext } from '../src/lib/chatContext.ts';

// Independent process parity: TS selector + Rust production assembly entrypoint.
// Rust is required ONLY by test:context:parity, not the ordinary Node test suite.
const directory = mkdtempSync(join(tmpdir(), 'unoone-context-parity-'));
const binary = join(directory, process.platform === 'win32' ? 'greeting-probe.exe' : 'greeting-probe');
after(() => rmSync(directory, { recursive: true, force: true }));
const installedRust = join(homedir(), '.cargo', 'bin', process.platform === 'win32' ? 'rustc.exe' : 'rustc');
const rustc = process.env.RUSTC || (existsSync(installedRust) ? installedRust : 'rustc');
try {
  execFileSync(rustc, ['--edition=2021', fileURLToPath(new URL('./context-greeting-probe.rs', import.meta.url)), '-o', binary], { stdio: 'pipe' });
} catch (error) {
  rmSync(directory, { recursive: true, force: true });
  throw new Error(`Production greeting parity requires rustc (or RUSTC): ${error.message}`, { cause: error });
}

const stale = [
  { id: 'old-u', role: 'user', content: 'Create Small Giant knowledge-base plan',
    context: { provenance: 'archive', session_id: 'old', pair_id: 'old', user_text: 'Create Small Giant knowledge-base plan' } },
  { id: 'old-a', role: 'assistant', content: 'PARITY_OLD_PLAN_SENTINEL',
    context: { provenance: 'archive', session_id: 'old', pair_id: 'old' } },
];
const vocabulary = ['hi', 'hello', 'hey', 'hi there', 'hello there', 'hey there', 'greetings', 'howdy',
  'namaste', 'namaskar', 'नमस्ते', 'नमस्कार', 'good morning', 'good afternoon', 'good evening',
  'hola', 'bonjour', 'こんにちは', '你好'];
const cases = new Map();
function add(text, expected) { cases.set(text, expected); }
for (const text of vocabulary) {
  add(text, true);
  add(text.toUpperCase(), true);
  add(`\ufeff${text}\u0085`, true);
  add(text.replace(/[!-~]/g, char => String.fromCharCode(char.charCodeAt(0) + 0xfee0)).replace(/ /g, '\u3000'), true);
  add(`${text}, explain photosynthesis`, false);
}
const whitespace = [0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0x85, 0xa0, 0x1680,
  0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a,
  0x2028, 0x2029, 0x202f, 0x205f, 0x3000, 0xfeff];
for (const cp of whitespace) {
  const char = String.fromCodePoint(cp);
  add(`${char}hi${char}${char}there${char}`, true);
}
for (const [start, end] of [[0x21, 0x2f], [0x3a, 0x40], [0x5b, 0x60], [0x7b, 0x7e]]) {
  for (let cp = start; cp <= end; cp++) {
    const char = String.fromCodePoint(cp);
    add(`${char}hello${char}`, true);
    const fullwidth = String.fromCodePoint(cp + 0xfee0);
    add(`${fullwidth}hello${fullwidth}`, true);
  }
}
for (const char of '。！？，、；：…—–·«»“”‘’¿¡') add(`${char}hello${char}`, true);
for (const cp of [0x2600, 0x26ff, 0x2700, 0x27bf, 0x1f300, 0x1f44b, 0x1faff, 0xfe0f, 0x200d]) {
  add(`hello${String.fromCodePoint(cp)}`, true);
}
for (const text of ['Ｈｅｌｌｏ！', 'hello : )', 'hi 👩‍💻', 'hello\u200d\ufe0f']) add(text, true);
// No NFKC compatibility expansion or blanket punctuation/symbol stripping.
for (const text of ['ℌello', 'ℎello', 'ⓗⓔⓛⓛⓞ', '𝐡𝐞𝐥𝐥𝐨', 'hello\u200b', 'hello\u200c',
  'hello\ufe0e', 'hello\u{1f2ff}', 'hello\u{1fb00}', 'hello\u25ff', 'hello\u27c0', 'hi\u001cthere',
  'hello\u180e', 'hello\u2060', 'hi—there', 'helló', 'hello\u0301', 'hi/there', 'hi﹔',
  'hi・', 'hi…there', '', ' ', 'hello, explain photosynthesis',
  'hello\n[attached file: greeting.txt]\nhi', 'hello [1 image(s) attached]']) add(text, false);

for (const [text, expected] of cases) {
  for (const hasAttachments of [false, true]) {
    test(`greeting parity ${JSON.stringify(text)} attachments=${hasAttachments}`, () => {
      const wanted = expected && !hasAttachments;
      const ts = selectChatContext({ userText: text, hasAttachments, messages: stale,
        activeTask: null, sessionId: 'now', newTaskId: 'parity' });
      const [greeting, bytes, retained] = execFileSync(binary, [text, String(hasAttachments)], { encoding: 'utf8' }).trim().split(',');
      assert.equal(ts.reason === 'greeting', wanted, 'TypeScript contract');
      assert.equal(greeting === 'true', wanted, 'Rust production contract');
      assert.equal(ts.reason === 'greeting', greeting === 'true', 'cross-language classification');
      if (wanted) {
        assert.deepEqual(ts.history, []);
        assert.equal(Number(bytes), 0, 'Rust long-term memory disabled');
        assert.equal(retained, 'false', 'Rust stale history excluded');
      } else {
        assert.ok(Number(bytes) > 0, 'nongreeting memory guard unchanged');
        assert.equal(retained, 'true', 'probe demonstrates greeting guard did not apply');
      }
    });
  }
}
