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
assert.equal(tools('jsdom/package.json').version, '26.1.0');
const React = require('react'); const { act } = React;
const { mockIPC, clearMocks } = require('@tauri-apps/api/mocks');
const dir = mkdtempSync(join(tmpdir(), 'local-vault-ui-'));
process.on('exit', () => rmSync(dir, { recursive: true, force: true }));
const out = join(dir, 'unlock.cjs');
await require('esbuild').build({ entryPoints: [join(frontend, 'src/components/UnlockScreen.tsx')], outfile: out, bundle: true, platform: 'node', format: 'cjs', jsx: 'automatic', plugins: [{ name: 'react', setup(b) { b.onResolve({ filter: /^react(?:\/.*)?$/ }, ({ path }) => ({ path: require.resolve(path), external: true })); } }] });
const { UnlockScreen } = require(out);
const local = state => ({ detected: true, storage_kind: 'local', vault_root: '/private/local-install', install_root: '/private/local-install', vault_id: state === 'locked' ? 'own-id' : '', local_vault_state: state, assets_ready: false, validation_failures: [] });
async function mount(t, commands) {
  const dom = new JSDOM('<div id="root"></div>', { url: 'https://local-vault.test.invalid/' });
  const keys = ['window', 'document', 'navigator', 'Event', 'MouseEvent', 'HTMLElement', 'IS_REACT_ACT_ENVIRONMENT'];
  const old = new Map(keys.map(k => [k, Object.getOwnPropertyDescriptor(globalThis, k)]));
  for (const k of keys) Object.defineProperty(globalThis, k, { configurable: true, writable: true, value: k === 'IS_REACT_ACT_ENVIRONMENT' ? true : dom.window[k] });
  const calls = [], unlocked = [], unexpected = [];
  mockIPC((command, args) => {
    calls.push({ command, args });
    if (command === 'plugin:event|listen') return calls.length;
    if (command === 'plugin:event|unlisten') return;
    if (!commands[command]) { unexpected.push(command); throw new Error(`Unexpected ${command}`); }
    return commands[command](args);
  });
  const root = require('react-dom/client').createRoot(dom.window.document.getElementById('root'));
  const flush = async () => { for (let i=0;i<3;i++) await act(async () => { await new Promise(r => setImmediate(r)); }); };
  t.after(async () => { await act(async () => root.unmount()); clearMocks(); dom.window.close(); for (const [k,v] of old) { if(v) Object.defineProperty(globalThis,k,v); else delete globalThis[k]; } assert.deepEqual(unexpected,[]); });
  await act(async () => root.render(React.createElement(UnlockScreen, { onUnlock: (...args) => unlocked.push(args) })));
  await flush();
  const container=dom.window.document.getElementById('root');
  return { calls, unlocked, container, text:()=>container.textContent,
    click: async text => { const el=[...container.querySelectorAll('button')].find(b=>b.textContent===text); assert.ok(el, text); await act(async()=>el.dispatchEvent(new dom.window.MouseEvent('click',{bubbles:true}))); await flush(); },
    fill: async (id, value) => { const el=container.querySelector(`#${id}`); assert.ok(el); await act(async()=> { Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype,'value').set.call(el,value); el.dispatchEvent(new dom.window.Event('input',{bubbles:true})); }); await flush(); },
    submit: async () => { await act(async()=>container.querySelector('form').dispatchEvent(new dom.window.Event('submit',{bubbles:true,cancelable:true}))); await flush(); },
  };
}
test('ordinary first run is explicit local creation, no dev bypass, download or drive wait', async t => {
  const h=await mount(t,{detect_vault:()=>local('new'),setup_vault: args=>{ assert.equal(args.vaultRoot,'/private/local-install'); assert.equal(args.password,'my-password'); return {success:true,vault_id:'new-id',recovery_key:'private recovery words'}; },unlock_vault:()=>({success:true,vault_id:'new-id'})});
  assert.match(h.text(),/Create your local vault/); assert.doesNotMatch(h.text(),/Waiting for.*drive/);
  await h.submit(); assert.match(h.text(),/enter it identically twice/); assert.equal(h.calls.filter(c=>c.command==='setup_vault').length,0);
  await h.fill('vault-password','my-password'); await h.fill('vault-confirm','my-password'); await h.submit();
  assert.match(h.text(),/Save your recovery key/); assert.deepEqual(h.unlocked,[]);
  assert.equal(h.calls.filter(c=>c.command==='unlock_vault').length,0);
  await h.click("I've Saved My Recovery Key"); assert.deepEqual(h.unlocked,[['new-id','/private/local-install','local']]);
  assert.doesNotMatch(h.text(),/private recovery words/);
  assert.equal(window.localStorage.length,0);
});
test('wrong password stays locked; recovery has a distinct explicit IPC and no reset claim',async t=>{
  const h=await mount(t,{detect_vault:()=>local('locked'),unlock_vault:()=>({success:false,error:'Wrong password'}),recover_local_vault:args=>{assert.equal(args.recoveryPhrase,'test recovery words');return {success:true,vault_id:'own-id'};}});
  await h.fill('vault-password','wrong'); await h.submit(); assert.match(h.text(),/Wrong password/); assert.deepEqual(h.unlocked,[]);
  await h.click('Use recovery phrase'); assert.match(h.text(),/does not reset your password/); await h.fill('local-recovery','test recovery words'); await h.submit(); assert.equal(h.unlocked.length,1);
  assert.equal(h.container.querySelector('#local-recovery').value,'');
});
test('interrupted create only resumes on explicit password verification, never invokes setup',async t=>{
  let resumed=false; const h=await mount(t,{detect_vault:()=>local(resumed?'locked':'interrupted'),resume_local_vault:args=>{assert.equal(args.password,'original');resumed=true;}});
  assert.match(h.text(),/Recover interrupted creation/); await h.fill('vault-password','original');await h.submit();
  assert.match(h.text(),/Creation recovered/);assert.deepEqual(h.unlocked,[]);assert.equal(h.container.querySelector('#vault-password').value,'');
});
test('malformed storage never offers replacement creation',async t=>{
  const h=await mount(t,{detect_vault:()=>{throw new Error('Malformed local vault identity');}});
  assert.match(h.text(),/Existing files have not been replaced/);assert.equal(h.container.querySelectorAll('form').length,0);assert.deepEqual(h.unlocked,[]);
});
test('backup is user-triggered and accurately labels same-device and restore limits',async t=>{
  const h=await mount(t,{detect_vault:()=>local('locked'),backup_local_vault:()=>'/private/local-backups/unique-id'});
  assert.equal(h.calls.filter(c=>c.command==='backup_local_vault').length,0);await h.click('Create encrypted backup');
  assert.match(h.text(),/not protection against disk loss/);assert.match(h.text(),/Restore\/import is not automated/);
});
test('older explicit legacy detect contract still unlocks without local recovery or import',async t=>{
  const h=await mount(t,{detect_vault:()=>({detected:true,vault_root:'/legacy/UNOONE',vault_id:'legacy-id',validation_failures:[]}),unlock_vault:()=>({success:true,vault_id:'legacy-id'})});
  assert.match(h.text(),/Explicit legacy drive compatibility mode/);assert.doesNotMatch(h.text(),/Create encrypted backup/);
  await h.fill('vault-password','existing');await h.submit();assert.deepEqual(h.unlocked,[['legacy-id','/legacy/UNOONE',undefined]]);
});
test('local automatic boot waits for unlock, runs the same native chain, and disconnect listener is mode-bound',()=>{
  const src=readFileSync(join(frontend,'src/App.tsx'),'utf8');
  // Local mode never boots pre-unlock (no preUnlockRoot) and no longer short-circuits
  // into a fixed "AI unavailable" error: the backend decides per present file.
  assert.doesNotMatch(src,/No verified local model\/runtime has been provisioned/);
  assert.ok(src.indexOf('await tauriApi.selectDesktopModel(bootRoot)') < src.indexOf('await tauriApi.startModelServer('));
  assert.match(src,/if \(!active \|\| storageKind === 'local'\) return;\s+handleLock\(\)/);
  assert.match(src,/if \(info.storage_kind !== 'local'\) setPreUnlockRoot/);
});
