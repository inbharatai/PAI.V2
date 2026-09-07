# web-runtime — UnoOne Page Agent runtime

`page-agent-unoone` is the TypeScript/JS runtime the Android secure browser
injects into approved pages (`UnoOnePageAgentRuntime` entry symbol). The
WebView loads it from the APK asset `page-agent/unoone-page-agent.js`, which
lives in `android-app/UnoOneAgent/securebrowser/src/main/assets/page-agent/`.

## Why both the source and the bundle are vendored

The TS source tree (`src/`, `tests/`, config, scripts) has been in the repo
since the original self-contained import (commit 28c35c0). What the repository
lacked was the **built bundle**: the 2026-08-26 pendrive build shipped an APK
that contained the asset, but the repo had neither `dist/unoone-page-agent.js`
(swept up by the `**/dist/` ignore rule) nor the copy at the APK asset path
above — so a repository rebuild could not reproduce the shipped
secure-browser behaviour, and the instrumented gate
(`SecureBrowserPolicyHeadlessTest.pageAgentRuntimeAssetIsPackagedAndByteAuthentic`)
failed with `FileNotFoundException` on an emulator. Fixed by vendoring the
bundle in both places:

- **`dist/unoone-page-agent.js`** — the byte-authentic bundle
  (196,197 bytes, SHA-256 `d798e06e95e3cbab1f71aac4498d428bde76ec1eec6e9c13b99852a6b2cf6369`,
  the same pins the instrumented test asserts). Committed via an explicit
  `.gitignore` exception to the `**/dist/` rule, and copied to the APK asset
  path above. `unoone-page-agent.js.map` stays untracked (build byproduct).
- **the TS source, tests, and config** — already tracked; they make the
  bundle regenerable rather than a black-box blob. `node_modules/` and
  `test-results/` are not tracked.

## Rebuild

```bash
cd web-runtime/page-agent-unoone
npm install
npm run bundle:android   # vite build + scripts/copy-to-android.mjs
```

`copy-to-android.mjs` resolves the repository root from this file's location
(`web-runtime/page-agent-unoone/../../..`) and copies the fresh
`dist/unoone-page-agent.js` into
`android-app/UnoOneAgent/securebrowser/src/main/assets/page-agent/`.

If the bundle is rebuilt and the bytes change, the pinned size and SHA-256 in
`SecureBrowserPolicyHeadlessTest` and in the mobile golden hashes must be
updated in the same commit — the byte pins are the point of the gate.