// Dependency-free packaging for the production entry, which is already a self-contained IIFE.
// Vite remains the normal upstream build. This path performs no transpilation/minification and
// explicitly refuses imports/exports rather than silently omitting dependencies.
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { Script } from 'node:vm'
import { createHash } from 'node:crypto'
const root = new URL('../', import.meta.url)
const source = await readFile(new URL('src/dom-adapter.js', root), 'utf8')
if (/^\s*(?:import|export)\b/m.test(source)) throw Error('Adapter must remain dependency-free')
new Script(source, { filename: 'dom-adapter.js' })
if (!source.includes('UnoOneDomAdapter') || source.includes('__UNOONE_PAGE_AGENT_SESSION__')) throw Error('Native boundary violation')
await mkdir(new URL('dist/', root), { recursive: true })
await writeFile(new URL('dist/unoone-page-agent.js', root), source)
console.log('Packaged exact source IIFE (not Vite/transpiled), SHA256 ' + createHash('sha256').update(source).digest('hex'))
await import('./copy-to-android.mjs')
