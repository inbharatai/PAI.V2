import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  act, mountChat, archive, archiveHistory, turn, deferred, reply,
  note, explanations,
} from './chat-view-harness.mjs';

// These are mounted production React tests with official mockIPC. They do not
// invoke native IPC, Rust, a model, a real vault, WebView2 or Windows.
const restoredMessages = turns => turns.flatMap(turn => [
  { role: 'user', content: turn.user_message },
  { role: 'assistant', content: turn.assistant_message },
]);
const historyPair = (user, assistant) => [{ role: 'user', content: user }, { role: 'assistant', content: assistant }];
const validSession = id => assert.match(id, /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i);
const buttons = (ui, text) => [...ui.container.querySelectorAll('button')].filter(button => button.textContent.includes(text));
function assertNote(ui, explanation, count) {
  assert.equal(ui.status(), note(explanation, count), 'selected-history UI note matches actual request history');
}

test('lifecycle restores canonical pairs once for display, with exact recall args and listener cleanup', async t => {
  const ui = await mountChat(t);
  assert.deepEqual(ui.ipc('recall_chat_memory'), [{ limit: 30 }]);
  assert.deepEqual(ui.messages(), restoredMessages(archive));
  assert.match(ui.text(), /Restored 3 turns from your vault for display/);
  assert.match(ui.text(), /Archive is not automatic model context/);
  assert.deepEqual(ui.ipc('save_chat_turn'), []);
  assert.deepEqual(ui.ipc('harness_chat'), []);
  assert.equal(ui.dom.window.__TAURI_INTERNALS__.callbacks.size, 2);
  await ui.unmount();
  assert.equal(ui.dom.window.__TAURI_INTERNALS__.callbacks.size, 0);
});

test('StrictMode effect replay restores once and displays the actual admitted count', async t => {
  const ui = await mountChat(t, { strict: true });
  assert.deepEqual(ui.ipc('recall_chat_memory'), [{ limit: 30 }]);
  assert.deepEqual(ui.messages(), restoredMessages(archive));
  assert.match(ui.text(), /Restored 3 turns from your vault for display/);
  await ui.send('hello');
  assert.deepEqual(ui.ipc('harness_chat')[0].conversationHistory, []);
  assert.equal(ui.ipc('save_chat_turn').length, 1);
});

test('exact temporal-framed continuation is selected in mounted UI without claiming date filtering', async t => {
  const ui = await mountChat(t);
  await ui.send('Continue the knowledge-base plan from yesterday');
  assert.deepEqual(ui.ipc('harness_chat')[0].conversationHistory, archiveHistory);
  assert.match(ui.status(), /only one matching archived task/);
  assert.match(ui.status(), /no date filtering performed/);
});

test('standalone greeting sends no archive and preserves displayed pairs and canonical typed save', async t => {
  const ui = await mountChat(t, { harness: () => reply('Hello from explicit IPC double.') });
  await ui.send('  hello  ', { enter: true });
  const [args] = ui.ipc('harness_chat');
  validSession(args.conversationId);
  assert.deepEqual(args, {
    message: 'hello', conversationHistory: [], conversationId: args.conversationId,
    allowWorkspaceGoal: true, images: [], personalMode: false, personalTask: null, personalUserMessage: null,
  });
  assert.deepEqual(ui.ipc('save_chat_turn'), [{
    sessionId: args.conversationId, userMessage: 'hello', assistantMessage: 'Hello from explicit IPC double.',
  }]);
  assert.deepEqual(ui.messages(), [...restoredMessages(archive), ...historyPair('hello', 'Hello from explicit IPC double.')]);
  assertNote(ui, explanations.greeting, 0);
  await ui.click(buttons(ui, 'Activity')[0]);
  assert.match(ui.container.querySelector('.chat-message.assistant:last-of-type')?.textContent ?? ui.text(), /standalone greeting; no prior task history sent/);
  assert.deepEqual(ui.activity, [true, false], 'lock deferral event spans only the run');
});

test('named archive continuation, active followup and new-task reset reach actual harness args', async t => {
  let index = 0;
  const answers = ['Selected Small Giant answer.', 'Active followup answer.', 'Invoice task answer.', 'Invoice fix answer.'];
  const ui = await mountChat(t, { harness: () => reply(answers[index++]) });
  const named = 'Continue our Small Giant knowledge-base plan';
  await ui.send(named);
  assert.deepEqual(ui.ipc('harness_chat')[0].conversationHistory, archiveHistory);
  assertNote(ui, explanations.named, 4);
  await ui.send('yes, continue');
  assert.deepEqual(ui.ipc('harness_chat')[1].conversationHistory, [...archiveHistory, ...historyPair(named, answers[0])]);
  assertNote(ui, explanations.active, 6);
  const newTask = 'Build a React invoice dashboard with CSV export';
  await ui.send(newTask);
  assert.deepEqual(ui.ipc('harness_chat')[2].conversationHistory, []);
  assertNote(ui, explanations.newTask, 0);
  await ui.send('fix it');
  assert.deepEqual(ui.ipc('harness_chat')[3].conversationHistory, historyPair(newTask, answers[2]));
  assertNote(ui, explanations.active, 2);
  const ids = ui.ipc('harness_chat').map(args => args.conversationId);
  assert.equal(new Set(ids).size, 1, 'task reset does not reset stable conversation namespace');
  assert.equal(ui.ipc('save_chat_turn').length, 4);
  assert.deepEqual(ui.messages().slice(0, 6), restoredMessages(archive), 'context resets never erase displayed archive');
});

for (const [title, prompt, turns, explanation] of [
  ['unknown name', 'Continue the Atlas migration', archive, explanations.notFound],
  ['generic continuation', 'continue', archive, explanations.notFound],
  ['ambiguous name', 'Continue Small Giant', [...archive, turn('Create the Small Giant knowledge-base plan', 'Another archived Small Giant.', 'other-session')], explanations.ambiguous],
]) {
  test(`${title} sends no arbitrary archive and displays the correct selected-note`, async t => {
    const ui = await mountChat(t, { turns });
    await ui.send(prompt);
    assert.deepEqual(ui.ipc('harness_chat')[0].conversationHistory, []);
    assertNote(ui, explanation, 0);
    assert.deepEqual(ui.messages().slice(0, turns.length * 2), restoredMessages(turns));
  });
}

test('read-only fallback uses identical selected context and explicit camelCase IPC args', async t => {
  const prompt = 'Continue our Small Giant knowledge-base plan';
  const ui = await mountChat(t, {
    harness: () => { throw new Error('mock harness unavailable'); },
    legacy: () => ({ final_text: 'Read-only fallback result.', steps: [], context_note: 'Backend mock context note.' }),
  });
  await ui.click(ui.container.querySelector('input[aria-label="Manual agent tools"]'));
  assert.equal(ui.dom.window.localStorage.getItem('unoone.fullAccess'), 'off');
  await ui.send(prompt);
  const [harness] = ui.ipc('harness_chat');
  assert.equal(harness.allowWorkspaceGoal, false);
  assert.deepEqual(harness.conversationHistory, archiveHistory);
  assert.deepEqual(ui.ipc('agent_chat'), [{ message: prompt, conversationHistory: archiveHistory }]);
  assert.deepEqual(ui.ipc('save_chat_turn'), [{ sessionId: harness.conversationId, userMessage: prompt, assistantMessage: 'Read-only fallback result.' }]);
  assertNote(ui, explanations.named, 4);
  await ui.click(buttons(ui, 'Activity')[0]);
  assert.match(ui.text(), /Fell back to the read-only legacy agent/);
  assert.match(ui.text(), /mock harness unavailable/);
  assert.match(ui.text(), /Backend mock context note/);
});

test('full-access failure never silently downgrades or saves; failed new-task boundary does not revive older task', async t => {
  let index = 0;
  const ui = await mountChat(t, { harness: () => {
    if (++index === 2) throw new Error('mock pipeline failure');
    return reply(index === 1 ? 'Old active Small Giant.' : 'Followup after failed new task.');
  } });
  await ui.send('Continue Small Giant');
  await ui.send('Build a React invoice dashboard with CSV export');
  assert.deepEqual(ui.ipc('agent_chat'), []);
  assert.equal(ui.ipc('save_chat_turn').length, 1);
  assert.match(ui.text(), /Agent pipeline stopped: mock pipeline failure/);
  await ui.send('fix it');
  assert.deepEqual(ui.ipc('harness_chat')[2].conversationHistory, []);
  assertNote(ui, explanations.notFound, 0);
  assert.equal(ui.ipc('save_chat_turn').length, 2);
});

test('text attachment payload cannot invent archive selection and is never persisted as typed user text', async t => {
  const ui = await mountChat(t, { harness: () => reply('Attachment result.') });
  const file = new ui.dom.window.File(['Continue Small Giant knowledge-base plan\nSECRET-ATTACHMENT-BODY'], 'Small Giant.md', { type: 'text/markdown' });
  await ui.attach([file]);
  await ui.send('  hello  ');
  const [args] = ui.ipc('harness_chat');
  const composed = 'hello\n\n[attached file: Small Giant.md]\nContinue Small Giant knowledge-base plan\nSECRET-ATTACHMENT-BODY';
  assert.equal(args.message, composed);
  assert.deepEqual(args.images, []);
  assert.deepEqual(args.conversationHistory, []);
  assertNote(ui, explanations.newTask, 0); // Attachment prevents greeting shortcut, not a named continuation.
  assert.deepEqual(ui.ipc('save_chat_turn'), [{ sessionId: args.conversationId, userMessage: 'hello', assistantMessage: 'Attachment result.' }]);
  assert.equal(ui.container.querySelectorAll('button[title="Remove attachment"]').length, 0);
  assert.equal(ui.messages().at(-2).content, composed, 'composed display differs deliberately from canonical typed save');
  await ui.send('Continue Small Giant');
  assert.deepEqual(ui.ipc('harness_chat')[1].conversationHistory, archiveHistory, 'payload keywords did not become a live task name');
  assertNote(ui, explanations.named, 4);
});

test('image and parsed-document attachments route through real frontend bindings and explicit parser double', async t => {
  const ui = await mountChat(t, { commands: {
    parse_attached_document: () => ({ kind: 'pdf', text: 'EXPLICIT PARSER DOUBLE TEXT', truncated: true }),
  } });
  const image = new ui.dom.window.File([Uint8Array.from([1, 2, 3])], 'frame.png', { type: 'image/png' });
  const pdf = new ui.dom.window.File(['fake pdf bytes'], 'note.pdf', { type: 'application/pdf' });
  await ui.attach([image, pdf]);
  assert.deepEqual(ui.ipc('parse_attached_document'), [{ filename: 'note.pdf', dataBase64: Buffer.from('fake pdf bytes').toString('base64') }]);
  await ui.send('Summarize this attached frame and document');
  const [args] = ui.ipc('harness_chat');
  assert.deepEqual(args.images, ['data:image/png;base64,AQID']);
  assert.equal(args.message, 'Summarize this attached frame and document\n\n[attached file: note.pdf (pdf) — content truncated]\nEXPLICIT PARSER DOUBLE TEXT\n\n[1 image(s) attached]');
  assert.deepEqual(args.conversationHistory, []);
  assert.equal(ui.ipc('save_chat_turn')[0].userMessage, 'Summarize this attached frame and document');
  assert.equal(ui.container.querySelectorAll('button[title="Remove attachment"], button[title="Remove image"]').length, 0);
});

test('read-only attachment fallback receives composed prompt and the same selected history, canonical save stays typed', async t => {
  const ui = await mountChat(t, { fullAccess: false, harness: () => { throw new Error('mock read-only rollback'); } });
  await ui.attach([new ui.dom.window.File(['UNTRUSTED PER-TURN PAYLOAD'], 'notes.txt', { type: 'text/plain' })]);
  await ui.send('Continue Small Giant');
  const [harness] = ui.ipc('harness_chat');
  assert.equal(harness.allowWorkspaceGoal, false);
  assert.deepEqual(harness.conversationHistory, archiveHistory);
  assert.deepEqual(ui.ipc('agent_chat'), [{ message: 'Continue Small Giant\n\n[attached file: notes.txt]\nUNTRUSTED PER-TURN PAYLOAD', conversationHistory: archiveHistory }]);
  assert.equal(ui.ipc('save_chat_turn')[0].userMessage, 'Continue Small Giant');
  assertNote(ui, explanations.named, 4);
});

test('cooperative stop has pinned DOM control, exact namespace, no fallback/save/reuse of cancelled output', async t => {
  const pending = deferred();
  let index = 0;
  const ui = await mountChat(t, { fullAccess: false, harness: () => ++index === 1 ? pending.promise : reply('Followup after cancelled turn.') });
  await ui.send('Continue Small Giant');
  const [args] = ui.ipc('harness_chat');
  assert.equal(ui.textarea().disabled, true);
  assert.equal(ui.sendButton().disabled, true);
  const pinnedStop = buttons(ui, 'Stop').find(button => !button.closest('.chat-messages'));
  assert.ok(pinnedStop, 'stop is outside scrolling transcript');
  await ui.click(pinnedStop);
  assert.deepEqual(ui.ipc('harness_stop_run'), [{ conversationId: args.conversationId }]);
  assert.equal(pinnedStop.disabled, true);
  await ui.click(pinnedStop);
  assert.equal(ui.ipc('harness_stop_run').length, 1, 'disabled control does not dispatch duplicate stop');
  await ui.settle(pending, new Error('cancelled:chat: user'), true);
  assert.match(ui.messages().at(-1).content, /Stopped — you stopped it/);
  assert.deepEqual(ui.ipc('save_chat_turn'), []);
  assert.deepEqual(ui.ipc('agent_chat'), []);
  assert.deepEqual(ui.activity, [true, false]);
  await ui.send('yes, continue');
  assert.deepEqual(ui.ipc('harness_chat')[1].conversationHistory, archiveHistory, 'only selected archive is reused, never cancelled pair');
  assertNote(ui, explanations.active, 4);
  assert.equal(ui.ipc('save_chat_turn').length, 1);
  assert.ok(!JSON.stringify(ui.ipc('harness_chat')[1]).includes('Stopped'));
});

for (const [title, recall] of [
  ['empty archive', () => ({ turns: [] })],
  ['locked/unavailable restore error', () => { throw new Error('vault locked (explicit restore double)'); }],
]) {
  test(`${title} leaves usable session-only chat, no restored badge, and whitespace is a no-op`, async t => {
    const ui = await mountChat(t, { recall });
    assert.ok(ui.container.querySelector('.empty-state'));
    assert.equal(ui.textarea().disabled, false);
    assert.doesNotMatch(ui.text(), /Restored \d+ turn/);
    await ui.type('   ');
    assert.equal(ui.sendButton().disabled, true);
    await ui.click(ui.sendButton());
    await act(async () => ui.textarea().dispatchEvent(new ui.dom.window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })));
    assert.deepEqual(ui.ipc('harness_chat'), []);
    assert.deepEqual(ui.ipc('save_chat_turn'), []);
    await ui.send('hello');
    assertNote(ui, explanations.greeting, 0);
    assert.deepEqual(ui.messages(), historyPair('hello', 'Mounted mock answer.'));
  });
}

for (const liveFinished of [false, true]) {
  test(`delayed restore after live ${liveFinished ? 'completion' : 'send in flight'} neither clobbers chat nor claims a false restored count`, async t => {
    const restore = deferred(), model = deferred();
    const ui = await mountChat(t, { recall: () => restore.promise, harness: () => model.promise });
    await ui.send('Build a React invoice dashboard with CSV export');
    if (liveFinished) await ui.settle(model, reply('Live invoice answer.'));
    const before = ui.messages();
    await ui.settle(restore, { turns: archive });
    assert.deepEqual(ui.messages(), before, 'late archive must not replace live messages');
    assert.doesNotMatch(ui.text(), /Restored \d+ turn/, 'ignored restore must not claim records were displayed');
    assert.doesNotMatch(ui.text(), /Small Giant/);
    assertNote(ui, explanations.newTask, 0);
    if (!liveFinished) await ui.settle(model, reply('Live invoice answer.'));
    assert.deepEqual(ui.messages(), historyPair('Build a React invoice dashboard with CSV export', 'Live invoice answer.'));
    assert.equal(ui.ipc('save_chat_turn').length, 1);
  });
}

test('restore/live-send updates queued in one act retain only live chat and an honest restore count', async t => {
  const restore = deferred(), model = deferred();
  const ui = await mountChat(t, { recall: () => restore.promise, harness: () => model.promise });
  await ui.type('Build the Aurora invoice dashboard');
  await act(async () => {
    ui.sendButton().dispatchEvent(new ui.dom.window.MouseEvent('click', { bubbles: true }));
    restore.resolve({ turns: archive });
  });
  await ui.flush();
  assert.equal(ui.messages()[0].content, 'Build the Aurora invoice dashboard');
  assert.doesNotMatch(ui.text(), /Small Giant|Restored \d+ turn/);
  assert.deepEqual(ui.ipc('harness_chat')[0].conversationHistory, []);
  await ui.settle(model, reply('Aurora invoice answer.'));
});

test('delayed restore before live send legitimately reports displayed count and named archive remains selectable', async t => {
  const restore = deferred();
  const ui = await mountChat(t, { recall: () => restore.promise });
  assert.doesNotMatch(ui.text(), /Restored/);
  await ui.settle(restore, { turns: archive.slice(0, 1) });
  assert.deepEqual(ui.messages(), restoredMessages(archive.slice(0, 1)));
  assert.match(ui.text(), /Restored 1 turn from your vault for display/);
  await ui.send('Continue Small Giant');
  assert.deepEqual(ui.ipc('harness_chat')[0].conversationHistory, archiveHistory.slice(0, 2));
  assertNote(ui, explanations.named, 2);
});

test('streamed tokens are namespace filtered, not persisted as a separate turn, and scrolling releases follow lock', async t => {
  const pending = deferred();
  const ui = await mountChat(t, { turns: [], harness: () => pending.promise });
  await ui.send('Build a status dashboard');
  const id = ui.ipc('harness_chat')[0].conversationId;
  await ui.emit('chat-token', { conversation_id: 'other-session', delta: 'WRONG SESSION' });
  assert.doesNotMatch(ui.text(), /WRONG SESSION/);
  await ui.emit('chat-token', { conversation_id: id, delta: 'Partial streamed content.' });
  assert.match(ui.text(), /Partial streamed content/);
  assert.deepEqual(ui.ipc('save_chat_turn'), []);
  const transcript = ui.container.querySelector('.chat-messages');
  Object.defineProperties(transcript, { scrollHeight: { value: 1000 }, clientHeight: { value: 100 }, scrollTop: { value: 0, writable: true } });
  await act(async () => transcript.dispatchEvent(new ui.dom.window.Event('scroll', { bubbles: true })));
  const scrolled = ui.scrollCalls.length;
  await ui.emit('chat-token', { conversation_id: id, delta: ' More streamed text.' });
  assert.equal(ui.scrollCalls.length, scrolled, 'scroll spy sees no forced follow after user scrolls away');
  await ui.settle(pending, reply('Authoritative completed answer.'));
  assert.deepEqual(ui.messages(), historyPair('Build a status dashboard', 'Authoritative completed answer.'));
  assert.equal(ui.ipc('save_chat_turn')[0].assistantMessage, 'Authoritative completed answer.');
  assert.equal(ui.textarea().disabled, false);
  assert.deepEqual(ui.activity, [true, false]);
});

test('default-budget Unicode archive clipping emits only nonempty complete pairs to IPC', async t => {
  const followup = 'please '.repeat(284) + 'continue!!!';
  const turns = [turn('🦀 Create Small Giant knowledge-base plan', 'OLD_ORPHAN_ASSISTANT'),
    ...Array.from({ length: 3 }, () => turn(followup, 'a'.repeat(6000)))];
  const ui = await mountChat(t, { turns });
  await ui.send('Continue Small Giant');
  const history = ui.ipc('harness_chat')[0].conversationHistory;
  assert.equal(history.length, 6);
  assert.deepEqual(history, restoredMessages(turns.slice(1)));
  assert.ok(history.every(entry => entry.content.trim().length > 0));
  assert.ok(!history.some(entry => entry.content.includes('OLD_ORPHAN')));
  assert.match(ui.status(), /History truncated/);
  assert.equal(ui.messages().length, 10, 'displayed archive remains unchanged plus completed live pair');
});

test('cross-panel image bridge reaches pending payload without replacing already typed text', async t => {
  const ui = await mountChat(t, { turns: [] });
  await ui.type('My typed accessibility question');
  const frame = 'data:image/png;base64,AQID';
  await act(async () => ui.dom.window.dispatchEvent(new ui.dom.window.CustomEvent('unoone:ask-in-chat', { detail: { text: 'Suggested question', imageDataUrl: frame } })));
  assert.equal(ui.textarea().value, 'My typed accessibility question');
  assert.equal(ui.container.querySelector('img[alt="attachment 1"]').src, frame);
  await ui.click(ui.sendButton());
  assert.equal(ui.ipc('harness_chat')[0].message, 'My typed accessibility question\n\n[1 image(s) attached]');
  assert.deepEqual(ui.ipc('harness_chat')[0].images, [frame]);
  assert.equal(ui.ipc('save_chat_turn')[0].userMessage, 'My typed accessibility question');
});
