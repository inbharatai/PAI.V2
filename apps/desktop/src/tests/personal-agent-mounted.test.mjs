import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const frontend = fileURLToPath(new URL('../', import.meta.url));
const require = createRequire(join(frontend, 'package.json'));
const tools = createRequire(join(resolve(process.env.PERSONAL_TEST_TOOL_ROOT || '/agent/workspace/test-tools'), 'package.json'));
const { JSDOM } = tools('jsdom');
const React = require('react'); const { act } = React;
const { mockIPC, clearMocks } = require('@tauri-apps/api/mocks');
const dir = mkdtempSync(join(tmpdir(), 'personal-ui-'));
process.on('exit', () => rmSync(dir, { recursive: true, force: true }));
const out = join(dir, 'personal.cjs');
await require('esbuild').build({ entryPoints: [join(frontend, 'src/components/PersonalAgentPanel.tsx')], outfile: out, bundle: true, platform: 'node', format: 'cjs', jsx: 'automatic', plugins: [{ name: 'react', setup(b) { b.onResolve({ filter: /^react(?:\/.*)?$/ }, ({ path }) => ({ path: require.resolve(path), external: true })); } }] });
const { PersonalAgentPanel } = require(out);
const base = () => ({ revision: 2, replica_id: 'replica-a', agent: { agent_id: 'agent-a', person_id: 'person-a', display_name: 'UnoOne', conversation_refs: [] }, persona: { revision: 1, deleted: false, preferences: [], provenance: { source: 'USER', actor_id: 'person-a', replica_id: 'replica-a' } }, tasks: [], pending_mutations: 2, sync_status: 'LOCAL_ONLY_NOT_PAIRED' });
const task = () => ({ spec: { task_id: 'task-a', goal: 'Review itinerary', deadline_ms: 1791629205000, user_visible_policy: 'Manual review; never executes', expected_postcondition: 'Manual review only', origin_replica_id: 'replica-a' }, events: [{event_id:'event-a',operation_id:'operation-a',predecessor_event_id:null,transition:'PLANNED',step:0,evidence_ref:null}], draft:'Original private draft', snooze_until_ms:null,deleted:false,status:'PLANNED',execute_on_hydration:false });
async function mount(t, view = base(), mutate = () => view) {
  const dom = new JSDOM('<div id="root"></div>', { url:'https://personal.invalid/' });
  const keys=['window','document','navigator','Event','MouseEvent','HTMLElement','IS_REACT_ACT_ENVIRONMENT'];
  const old=new Map(keys.map(k=>[k,Object.getOwnPropertyDescriptor(globalThis,k)]));
  for (const k of keys) Object.defineProperty(globalThis,k,{configurable:true,writable:true,value:k==='IS_REACT_ACT_ENVIRONMENT'?true:dom.window[k]});
  const calls=[]; const unexpected=[];
  mockIPC((command,args)=>{ calls.push({command,args}); if(command==='peer_sync_cancel') return null; if(command==='personal_agent_view') { if(view instanceof Error) throw view; return view; } if(command==='personal_agent_mutate') return mutate(args.request); unexpected.push(command); throw Error('Unexpected IPC'); });
  const root=require('react-dom/client').createRoot(document.getElementById('root'));
  const flush=async()=>{for(let i=0;i<3;i++) await act(async()=>{await new Promise(r=>setImmediate(r));});};
  t.after(async()=>{await act(async()=>root.unmount());clearMocks();dom.window.close();for(const[k,v]of old){if(v)Object.defineProperty(globalThis,k,v);else delete globalThis[k];}assert.deepEqual(unexpected,[]);});
  await act(async()=>root.render(React.createElement(PersonalAgentPanel))); await flush();
  const container=document.getElementById('root');
  return {calls,container,text:()=>container.textContent,
    click:async(text)=>{const b=[...container.querySelectorAll('button')].find(b=>b.textContent===text);assert.ok(b,text);assert.equal(b.disabled,false,text);await act(async()=>b.dispatchEvent(new dom.window.MouseEvent('click',{bubbles:true})));await flush();},
    fill:async(label,value)=>{const el=container.querySelector(`[aria-label="${label}"]`);assert.ok(el);const proto=el.tagName==='TEXTAREA'?dom.window.HTMLTextAreaElement.prototype:dom.window.HTMLInputElement.prototype;await act(async()=>{Object.getOwnPropertyDescriptor(proto,'value').set.call(el,value);el.dispatchEvent(new dom.window.Event('input',{bubbles:true}));});await flush();},
    submit:async()=>{await act(async()=>container.querySelector('form').dispatchEvent(new dom.window.Event('submit',{bubbles:true,cancelable:true})));await flush();},
  };
}
test('one mounted identity, provenance, private local limits and unavailable real domains',async t=>{
  const h=await mount(t);assert.match(h.text(),/UnoOne/);assert.match(h.text(),/source USER/);assert.match(h.text(),/active board reads the merged causal store/);assert.match(h.text(),/Conversation sync.*not connected/);assert.match(h.text(),/Unavailable.*no qualified account\/provider adapter/);assert.equal(h.calls.length,1);assert.equal(window.localStorage.length,0);
});
test('mounted user task creation submits actual bounded native contract input, not AI output',async t=>{
  let captured;const v=base();const h=await mount(t,v,r=>{captured=r;return {...v,revision:3,tasks:[{...task(),spec:{...task().spec,task_id:r.task_id,goal:r.text},draft:r.draft}]};});
  await h.fill('Task goal','Plan my trip');await h.fill('Private draft','Private notes');await h.submit();
  assert.equal(captured.action,'CREATE');assert.equal(captured.text,'Plan my trip');assert.equal(captured.draft,'Private notes');assert.equal(captured.expected_revision,2);assert.match(captured.operation_id,/^[0-9a-f-]{36}$/);assert.match(captured.task_id,/^[0-9a-f-]{36}$/);assert.match(h.text(),/Plan my trip/);assert.equal(window.localStorage.length,0);assert.equal(h.container.querySelector('[aria-label="Task goal"]').value,'');
});
test('mounted persona save and clear require explicit confirmation and preserve provenance',async t=>{
  const v=base();let last;const h=await mount(t,v,r=>{last=r;return {...v,revision:3,persona:{...v.persona,deleted:r.action==='CLEAR_PERSONA'}};});
  await h.fill('Assistant name','My UnoOne');await h.fill('Response preferences','Brief answers');await h.click('Save persona');assert.equal(last.action,'PERSONA');assert.equal(last.draft,'Brief answers');
  await h.click('Clear preferences');assert.equal(last.action,'PERSONA');assert.match(h.text(),/not secure erasure/);await h.click('Confirm clear preferences');assert.equal(last.action,'CLEAR_PERSONA');assert.match(h.text(),/Preferences cleared/);
});
test('mounted edit, timeline, accept, snooze, cancel and deletion use same task ID without execution',async t=>{
  const v={...base(),tasks:[task()]};const requests=[];const h=await mount(t,v,r=>{requests.push(r);return v;});
  await h.click('View timeline');assert.match(h.text(),/Original private draft/);assert.match(h.text(),/evidence: none/);
  await h.click('Edit');await h.fill('Task goal','Corrected itinerary');await h.submit();assert.equal(requests.at(-1).action,'EDIT');assert.equal(requests.at(-1).text,'Corrected itinerary');
  await h.click('Accept for review');await h.click('Snooze 1 day');assert.ok(requests.at(-1).snooze_until_ms>Date.now());await h.click('Cancel task');
  const count=requests.length;await h.click('Delete task');assert.equal(requests.length,count);await h.click('Confirm delete task');
  assert.deepEqual(requests.map(r=>r.action),['EDIT','ACCEPT','SNOOZE','CANCEL','DELETE']);assert.ok(requests.every(r=>r.task_id==='task-a'));assert.ok(h.calls.every(c=>c.command.startsWith('personal_agent_')));
});
test('locked ledger and failed writes never claim success or clear unconfirmed draft',async t=>{
  const h=await mount(t,base(),()=>{throw Error('write failed')});await h.fill('Task goal','Keep my unsaved goal');await h.submit();assert.match(h.text(),/not confirmed/);assert.equal(h.container.querySelector('[aria-label="Task goal"]').value,'Keep my unsaved goal');assert.match(h.text(),/Nothing was executed/);
});
test('initial native read failure offers reload, no fake identity or replacement task controls',async t=>{
  const h=await mount(t,Error('locked'));assert.match(h.text(),/Unlock the local vault/);assert.equal(h.container.querySelector('form'),null);assert.equal(h.calls.length,1);
});

test('active board displays merged peer task, retained conflict alternatives and claims without completion',async t=>{
 const v={...base(),sync_status:'SHARED_V2_MANUAL_ONLY',archived_mutations:4,conflicts:['TASK_CONFLICT: local draft / remote draft'],tasks:[{...task(),owner_replica_id:'replica-b',owner_epoch:1,remote_claims:[{source:'NATIVE',outcome:'ACTION_VERIFIED',observed_effect:'remote composer claim'}],status:'AWAITING_VERIFICATION'}]};
 const h=await mount(t,v);assert.match(h.text(),/Archived local mutations: 4/);assert.match(h.text(),/local draft \/ remote draft/);assert.match(h.text(),/Declarative owner: replica-b/);await h.click('View timeline');assert.match(h.text(),/Remote observed claims \(not native completion\)/);assert.match(h.text(),/remote composer claim/);assert.match(h.text(),/AWAITING_VERIFICATION/);
});
