// Dependency-free host regression; not a substitute for retained Vitest/Playwright/device suites.
import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { runInNewContext } from 'node:vm'
const source = readFileSync(new URL('../src/dom-adapter.js', import.meta.url), 'utf8')
const bundle = readFileSync(new URL('../dist/unoone-page-agent.js', import.meta.url), 'utf8')
const android = readFileSync(new URL('../../../android-app/UnoOneAgent/securebrowser/src/main/assets/page-agent/unoone-page-agent.js', import.meta.url), 'utf8')
test('packaged DOM adapter matches source and Android assets exactly', () => {
  assert.equal(source, bundle)
  assert.equal(bundle, android)
  assert.ok(!bundle.includes('__UNOONE_PAGE_AGENT_SESSION__'))
})
test('untrusted document receives only DOM methods and no native/session authority', () => {
  let calls = 0
  const window = {}
  runInNewContext(bundle, { window, location: { href: 'https://approved.example/' },
    document: { title: 'untrusted claims', querySelectorAll: () => [] }, fetch: () => { calls++ } })
  assert.deepEqual(Object.keys(window.UnoOneDomAdapter).sort(), ['act', 'observe', 'verify', 'version'])
  assert.equal(window.UnoOneDomAdapter.observe().elements.length, 0)
  assert.equal(window.UnoOnePageAgent, undefined)
  assert.equal(window.__UNOONE_PAGE_AGENT_SESSION__, undefined)
  assert.equal(calls, 0)
  assert.throws(() => window.UnoOneDomAdapter.act({ index: 1, fingerprint: 'forged', action: 'click_element_by_index' }), /STALE_TARGET/)
})
test('invalid navigation arguments refuse dispatch', () => {
  let calls = 0
  const window = { scrollBy: () => calls++, innerHeight: 100, innerWidth: 100 }
  runInNewContext(bundle, { window })
  assert.throws(() => window.UnoOneDomAdapter.act({ action: 'scroll', down: 'yes' }), /INVALID_DIRECTION/)
  assert.throws(() => window.UnoOneDomAdapter.act({ action: 'scroll', down: true, pixels: 6000 }), /INVALID_SCROLL/)
  assert.equal(calls, 0)
})
