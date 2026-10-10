// Mounted native-IPC contract fixtures, NOT native end-to-end/qualification.
import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { mkdtempSync, rmSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const frontend = fileURLToPath(new URL('../', import.meta.url));
const require = createRequire(join(frontend, 'package.json'));
const tools = createRequire(join(resolve(process.env.LOCAL_VAULT_TEST_TOOL_ROOT || '/agent/workspace/test-tools'), 'package.json'));
const { JSDOM } = tools('jsdom');
const React = require('react'); const { act } = React;
const { mockIPC, clearMocks } = require('@tauri-apps/api/mocks');
const dir = mkdtempSync(join(tmpdir(), 'power-setup-ui-'));
process.on('exit', () => rmSync(dir, { recursive: true, force: true }));
const out = join(dir, 'setup.cjs');
const plugins = [{ name: 'react', setup(b) { b.onResolve({ filter: /^react(?:\/.*)?$/ }, ({ path }) => ({ path: require.resolve(path), external: true })); } }];
await require('esbuild').build({ entryPoints: [join(frontend, 'src/components/ModelSetupWizard.tsx')], outfile: out, bundle: true, platform: 'node', format: 'cjs', jsx: 'automatic', plugins });
const { ModelSetupWizard } = require(out);
const hwOut = join(dir, 'hardware.cjs');
await require('esbuild').build({ entryPoints: [join(frontend, 'src/components/HardwareProfile.tsx')], outfile: hwOut, bundle: true, platform: 'node', format: 'cjs', jsx: 'automatic', plugins });
const { HardwareProfile } = require(hwOut);
const unknown = { provenance: 'UNKNOWN' };
const detected = value => ({ provenance: 'DETECTED', value });
const fixture = () => ({schema_version:1,catalog_state:'UNCONFIGURED',eligible_now:false,assets_ready:false,reason:'Publisher catalog is not configured. No model will be downloaded or loaded.',decisions:[],local_models:[],device:{os:detected('linux'),os_version:detected('fixture-os'),abi:detected('x86_64'),total_ram_bytes:detected(8*2**30),available_ram_bytes:detected(2*2**30),gpu_name:detected('Fixture GPU'),total_vram_bytes:detected(8*2**30),available_vram_bytes:unknown,usable_storage_bytes:unknown,backends:[{backend:'cuda',health:detected('DETECTED_ONLY')},{backend:'vulkan',health:unknown}]}});
// Native LocalModelReport shapes for the three honest states (serialized by provisioning.rs).
const report = (over) => ({id:'gemma-4-e4b-it',path:'/private/local-install/MODELS/e4b.gguf',mmproj_path:null,tier:'E4B',present:true,hash_verified:true,weights_bytes:3*2**30,projector_bytes:null,kv_estimate_bytes:2**30,context_tokens:4096,total_ram_bytes:16*2**30,available_ram_bytes:9*2**30,required_available_bytes:5*2**30,state:'UNKNOWN',outcome:'ALLOWED_WITH_LIMITS',label:'Fits this machine — not yet load-tested here (not yet qualified)',reasons:['Fits TOTAL and AVAILABLE RAM estimates'],decision:null,smoke:null,...over});
const threeStates = () => ({...fixture(), local_models:[
  report({id:'gemma-4-12b-it',path:'/p/12b.gguf',tier:'12B',state:'QUALIFIED',outcome:'RECOMMENDED',label:'Qualified (signed record)',reasons:['Signed candidate q v1: core decision Recommended / [Admitted]'],decision:{status:'RECOMMENDED'}}),
  report({state:'WORKS_HERE',label:'Works on this machine (not yet qualified)',reasons:['Native load + inference smoke previously passed on this device'],smoke:{generated_tokens:2,generation_ms:840,captured_at_ms:1}}),
  report({id:'gemma-4-e2b-it',path:'/p/e2b.gguf',tier:'E2B',present:false,hash_verified:false,weights_bytes:null,kv_estimate_bytes:null,required_available_bytes:null,outcome:'UNAVAILABLE',label:'Unknown',reasons:['Model file is not present on disk']}),
]});
async function mount(t, handler, Component = ModelSetupWizard) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'https://fixture.invalid/' });
  const keys = ['window','document','navigator','Event','MouseEvent','HTMLElement','IS_REACT_ACT_ENVIRONMENT'];
  const old = new Map(keys.map(k => [k,Object.getOwnPropertyDescriptor(globalThis,k)]));
  for(const k of keys)Object.defineProperty(globalThis,k,{configurable:true,writable:true,value:k==='IS_REACT_ACT_ENVIRONMENT'?true:dom.window[k]});
  const calls=[];
  mockIPC((command,args)=>{calls.push({command,args});assert.equal(command,'get_model_setup_assessment');return handler();});
  const root=require('react-dom/client').createRoot(dom.window.document.getElementById('root'));
  const flush=async()=>{for(let i=0;i<3;i++)await act(async()=>{await new Promise(r=>setImmediate(r));});};
  t.after(async()=>{await act(async()=>root.unmount());clearMocks();dom.window.close();for(const [k,v] of old){if(v)Object.defineProperty(globalThis,k,v);else delete globalThis[k];}});
  await act(async()=>root.render(React.createElement(Component)));await flush();
  const container=dom.window.document.getElementById('root');
  return {calls,container,text:()=>container.textContent,click:async text=>{const el=[...container.querySelectorAll('button')].find(b=>b.textContent===text);assert.ok(el,text);await act(async()=>el.dispatchEvent(new dom.window.MouseEvent('click',{bubbles:true})));await flush();}};
}
test('shows total AND available native RAM, detected GPU is not runtime support',async t=>{
  const h=await mount(t,fixture);assert.match(h.text(),/8\.0 GiB · detected/);assert.match(h.text(),/2\.0 GiB · detected/);assert.match(h.text(),/CUDA: Detected only — not load-tested/);assert.match(h.text(),/VULKAN: Unknown — not runtime-tested/);assert.match(h.text(),/Unknown — not measured/);assert.match(h.text(),/Compatibility catalog: not configured/);assert.match(h.text(),/No 2B, 4B or 12B option is approved/);assert.doesNotMatch(h.text(),/30 tokens|will run|0\.0 GiB/);assert.ok([...h.container.querySelectorAll('button')].find(b=>b.textContent==='Download unavailable').disabled);assert.equal(h.calls.length,1);
});
test('review/back/pause/resume never grants renderer consent or starts downloads',async t=>{
  const h=await mount(t,fixture);await h.click('Review download requirements');assert.match(h.text(),/Local download policy — not enabled/);assert.match(h.text(),/Nothing has been consented to/);await h.click('Back to device check');await h.click('Not now');assert.match(h.text(),/Setup paused/);await h.click('Resume setup');assert.equal(h.calls.length,2);await h.click('Recheck my device');assert.equal(h.calls.length,3);
});
test('native probe failure and unknown schema never become ready',async t=>{
  const h=await mount(t,()=>{throw new Error('probe blocked');});assert.match(h.text(),/probe blocked/);assert.doesNotMatch(h.text(),/GiB|Fixture GPU/);assert.ok([...h.container.querySelectorAll('button')].find(b=>b.textContent==='Download unavailable').disabled);
});
test('recheck discards stale native measurements rather than caching success',async t=>{
  let n=0;const h=await mount(t,()=>{if(n++===0)return fixture();throw new Error('recheck failed');});await h.click('Recheck my device');assert.match(h.text(),/recheck failed/);assert.doesNotMatch(h.text(),/Fixture GPU|8\.0 GiB/);
});
test('wizard lists the three local states and never calls an unsigned file qualified',async t=>{
  const h=await mount(t,threeStates);
  const badges=[...h.container.querySelectorAll('[data-local-state]')].map(b=>b.getAttribute('data-local-state'));
  assert.deepEqual(badges.slice(0,3),['QUALIFIED','WORKS_HERE','UNKNOWN']);
  assert.match(h.text(),/Qualified \(signed record\)/);assert.match(h.text(),/Works on this machine \(not yet qualified\)/);assert.match(h.text(),/Not present on disk/);
  assert.match(h.text(),/Load allowed/);assert.match(h.text(),/Not loadable/);assert.match(h.text(),/2 tokens in 840 ms \(count\/time, not a speed claim\)/);
  assert.doesNotMatch(h.text(),/tokens\/s|tok\/s|30 tokens/);
  assert.ok([...h.container.querySelectorAll('button')].find(b=>b.textContent==='Download unavailable').disabled,'downloads stay blocked with present files');
});
test('HardwareProfile shows measured facts plus the three states, no backend promotion, no speed guess',async t=>{
  const h=await mount(t,threeStates,HardwareProfile);
  assert.match(h.text(),/Hardware Profile/);assert.match(h.text(),/Total RAM8\.0 GiB · detected/);assert.match(h.text(),/Available RAM now2\.0 GiB · detected/);
  assert.match(h.text(),/linux · detected · fixture-os · detected · x86_64 · detected/);assert.match(h.text(),/Total GPU memory8\.0 GiB · detected/);assert.match(h.text(),/Free installation storageUnknown — not measured/);
  assert.match(h.text(),/CUDA: Detected only — not load-tested/);assert.match(h.text(),/VULKAN: Unknown — not runtime-tested/);
  const badges=[...h.container.querySelectorAll('[data-local-state]')].map(b=>b.getAttribute('data-local-state'));
  assert.deepEqual(badges.slice(0,3),['QUALIFIED','WORKS_HERE','UNKNOWN']);
  assert.match(h.text(),/Model downloads: blocked — publisher catalog not configured/);
  assert.doesNotMatch(h.text(),/tokens\/s|tok\/s|will run|~30/);
  assert.equal(h.calls.length,1);await h.click('Re-measure');assert.equal(h.calls.length,2);
});
test('HardwareProfile with no local files says so without inventing a download',async t=>{
  const h=await mount(t,fixture,HardwareProfile);assert.match(h.text(),/No declared model file is present in the local installation root/);assert.doesNotMatch(h.text(),/Download now|will run/);
});
test('HardwareProfile with a failed native probe shows no facts',async t=>{
  const f=await mount(t,()=>{throw new Error('probe blocked');},HardwareProfile);assert.match(f.text(),/probe blocked/);assert.doesNotMatch(f.text(),/GiB/);
});
test('source boundary: local select/start go through decide_local; smoke precedes publish; previous server retained; downloads blocked',()=>{
  const rust=readFileSync(join(frontend,'../src-tauri/src/llama.rs'),'utf8');
  const start=rust.slice(rust.indexOf('pub async fn start_model_server('),rust.indexOf('async fn finish_model_start('));
  assert.ok(start.indexOf('local_decision(&vault_root, &file, &config).await')<start.indexOf('active_manager.take()'),'local admission decided before the active server is touched');
  assert.ok(start.indexOf('inference_smoke(')<start.indexOf('finish_model_start('),'smoke runs before the server is published');
  assert.match(start,/resolve_replacement\(previous, manager, smoke\.is_ok\(\)\)/);assert.match(start,/Replacement::Rollback/);
  assert.ok(start.indexOf('while local_report.is_none()')>0,'drive asset sweep gate is skipped only in local mode');
  const select=rust.slice(rust.indexOf('pub async fn select_desktop_model('),rust.indexOf('fn select_model_for_memory('));
  assert.match(select,/return select_local_model\(&vault_root\)\.await;/);assert.match(select,/PocketManifest/,'legacy lane keeps the typed manifest requirement');
  const prov=readFileSync(join(frontend,'../src-tauri/src/provisioning.rs'),'utf8');
  assert.match(prov,/pub fn require_shipping_admission\(\) -> Result<\(\), String> \{\s+Err\(BLOCKED\.into\(\)\)/,'downloads remain fail-closed');
  assert.doesNotMatch(prov,/health: Observation::Tested/);
  const probe=readFileSync(join(frontend,'../src-tauri/src/provisioning_probe.rs'),'utf8');assert.doesNotMatch(probe,/Observation::Tested\(BackendHealth/);
  const app=readFileSync(join(frontend,'src/App.tsx'),'utf8');assert.doesNotMatch(app,/No verified local model\/runtime has been provisioned/);
  const manager=readFileSync(join(frontend,'src/components/ModelManager.tsx'),'utf8');assert.match(manager,/if \(!localInstall\) await tauriApi\.stopModelServer\(\);/);assert.doesNotMatch(manager,/disabled=\{localInstall/);
  const main=readFileSync(join(frontend,'../src-tauri/src/main.rs'),'utf8');
  for(const name of ['get_model_setup_assessment','begin_local_model_setup','revoke_local_model_download_policy'])assert.match(main,new RegExp(`${name},`));
  const command=main.slice(main.indexOf('fn begin_local_model_setup('),main.indexOf('fn revoke_local_model_download_policy('));assert.match(command,/require_main\(&window\)/);assert.match(command,/live\.is_none\(\)/);assert.doesNotMatch(command,/approved:\s*bool|url:\s*String/);
});
