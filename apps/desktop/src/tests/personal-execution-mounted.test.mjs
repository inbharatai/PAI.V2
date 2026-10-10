// Mounted production ChatView + real Tauri binding. IPC is explicitly mocked; not native/model qualification.
import assert from 'node:assert/strict';
import test from 'node:test';
import { mountChat, reply, act } from './chat-view-harness.mjs';
const view = () => ({ revision: 7, replica_id: 'replica-a', agent: { agent_id:'agent-a',person_id:'person-a',display_name:'UnoOne',conversation_refs:[] }, persona:{revision:2,deleted:false,preferences:[],provenance:{source:'USER'}}, tasks:[{spec:{task_id:'task-a',goal:'Prepare a travel draft',deadline_ms:9999999999999,origin_replica_id:'replica-a'},status:'READY_FOR_REVIEW',owner_replica_id:'replica-a',owner_epoch:1,events:[],draft:'',snooze_until_ms:null,remote_claims:[],execute_on_hydration:false}],pending_mutations:7,conflicts:[] });
test('personal is the actual default request lane, with no inherited manual tools or duplicate renderer persistence', async t => {
  const ui = await mountChat(t,{personalMode:true,commands:{personal_agent_view:()=>view()},harness:()=>reply('Native IPC test response')});
  assert.ok(ui.container.querySelector('[aria-label="Personal conversation controls"]'));
  assert.equal(ui.ipc('harness_chat').length,0,'hydration never executes');
  await ui.send('Write a greeting');
  const args=ui.ipc('harness_chat')[0];
  assert.equal(args.personalMode,true); assert.equal(args.personalTask,null);
  assert.equal(ui.ipc('agent_chat').length,0); assert.equal(ui.ipc('save_chat_turn').length,0);
});
test('accepted task still needs a distinct exact template review before a request carries its ID and revision', async t => {
  const ui=await mountChat(t,{personalMode:true,commands:{personal_agent_view:()=>view()}});
  const select=ui.container.querySelector('select[aria-label="Draft template task"]');
  assert.ok(select);
  await act(async()=>{select.value='task-a';select.dispatchEvent(new ui.dom.window.Event('change',{bubbles:true}));});
  assert.equal(ui.ipc('harness_chat').length,0);
  assert.match(ui.text(),/Read only the exact selected local source/);
  const review=[...ui.container.querySelectorAll('input[type=checkbox]')].find(e=>e.parentElement.textContent.includes('I reviewed this draft-only template'));
  await ui.click(review); await ui.send('Run the reviewed draft');
  assert.deepEqual(ui.ipc('harness_chat')[0].personalTask,{task_id:'task-a',expected_revision:7,source:{kind:'NONE'},children:false});
});
test('native revocation fails closed without legacy fallback, save or automatic replay', async t=>{
  const ui=await mountChat(t,{personalMode:true,commands:{personal_agent_view:()=>view()},harness:()=>{throw Error('Personal context changed');}});
  await ui.send('Reply briefly');
  assert.match(ui.text(),/No fallback or automatic retry/);
  assert.equal(ui.ipc('harness_chat').length,1);assert.equal(ui.ipc('agent_chat').length,0);assert.equal(ui.ipc('save_chat_turn').length,0);
});

test('selected source and specialist choice are explicit and selection changes revoke draft review', async t=>{
  const ui=await mountChat(t,{personalMode:true,commands:{personal_agent_view:()=>view()}});
  async function select(label,value) {const el=ui.container.querySelector(`select[aria-label="${label}"]`);await act(async()=>{el.value=value;el.dispatchEvent(new ui.dom.window.Event('change',{bubbles:true}));});}
  await select('Draft template task','task-a');await select('Local source template','SELECTED_FILE');
  const review=[...ui.container.querySelectorAll('input[type=checkbox]')].find(e=>e.parentElement.textContent.includes('I reviewed this draft-only template'));
  await ui.click(review);
  await select('Local source template','NOTES');
  assert.equal(review.checked,false);
  assert.equal(ui.ipc('harness_chat').length,0);
  await ui.send('Conversation rather than unreviewed source');
  assert.equal(ui.ipc('harness_chat')[0].personalTask,null);
});
