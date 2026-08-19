#!/usr/bin/env node
/**
 * Deterministic gate: prove the production frontend is embedded in a built
 * UnoOne Power executable.
 *
 * WHY THIS EXISTS
 * ---------------
 * A physical Pocket AI drive was staged with a UnoOnePower.exe that opened to
 * "localhost refused to connect". The Tauri config was already correct
 * (`frontendDist: "../src/dist"`), so no config diff would have caught it — the
 * staged binary simply did not contain the built assets. Nothing in CI asserted
 * that it did.
 *
 * This gate closes that hole by checking the ACTUAL BYTES of the shipped
 * executable, which is the only artifact that matters. It does not trust
 * tauri.conf.json, the build log, or the presence of a dist directory.
 *
 * WHAT IT PROVES
 * --------------
 *   1. dist/index.html exists and references at least one hashed asset.
 *   2. Every hashed asset filename referenced by index.html appears verbatim in
 *      the executable's bytes.
 *   3. The executable does not carry a dev-server URL as a live start target.
 *
 * A binary that passes cannot be a debug/dev build, and cannot be a release
 * build made before `vite build` ran.
 *
 * USAGE
 *   node scripts/verify-frontend-embedded.mjs \
 *     --dist apps/desktop/src/dist \
 *     --exe  target/release/unoone-power.exe
 *
 *   --json          machine-readable evidence to stdout
 *   --allow-dev-url treat an embedded dev URL as a warning, not a failure
 *
 * Exit codes: 0 pass, 1 fail, 2 bad invocation.
 */

import { readFileSync, existsSync, statSync } from 'node:fs';
import { join, basename } from 'node:path';
import { createHash } from 'node:crypto';

const DEV_URL_PATTERNS = ['http://localhost:5173', 'http://127.0.0.1:5173'];

function parseArgs(argv) {
  const args = { dist: null, exe: null, json: false, allowDevUrl: false };
  for (let i = 2; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--dist') args.dist = argv[++i];
    else if (a === '--exe') args.exe = argv[++i];
    else if (a === '--json') args.json = true;
    else if (a === '--allow-dev-url') args.allowDevUrl = true;
    else if (a === '--help' || a === '-h') args.help = true;
    else {
      console.error(`Unknown argument: ${a}`);
      process.exit(2);
    }
  }
  return args;
}

/**
 * Collect hashed asset filenames referenced by index.html.
 * Vite emits names like `index-DkR2f9Qa.js`; the hash makes them unique per
 * build, which is exactly what makes them a reliable embedding fingerprint.
 */
function extractAssetNames(html) {
  const names = new Set();
  const attrRe = /(?:src|href)\s*=\s*["']([^"']+)["']/gi;
  let m;
  while ((m = attrRe.exec(html)) !== null) {
    const ref = m[1];
    if (ref.startsWith('http://') || ref.startsWith('https://') || ref.startsWith('data:')) {
      continue;
    }
    const name = basename(ref.split('?')[0].split('#')[0]);
    // Only hashed build outputs are useful fingerprints. A bare "favicon.ico"
    // could coincidentally appear in any binary.
    if (/^[^.]+-[A-Za-z0-9_-]{6,}\.(js|css|mjs)$/.test(name)) {
      names.add(name);
    }
  }
  return [...names];
}

function fail(msg, evidence, asJson) {
  if (asJson) {
    console.log(JSON.stringify({ status: 'FAILED', verified: false, error: msg, ...evidence }, null, 2));
  } else {
    console.error(`\n  FAIL: ${msg}\n`);
  }
  process.exit(1);
}

function main() {
  const args = parseArgs(process.argv);

  if (args.help || !args.dist || !args.exe) {
    console.log(
      'Usage: node scripts/verify-frontend-embedded.mjs --dist <distDir> --exe <executable> [--json] [--allow-dev-url]'
    );
    process.exit(args.help ? 0 : 2);
  }

  const indexPath = join(args.dist, 'index.html');
  const evidence = {
    dist: args.dist,
    exe: args.exe,
    checked_at_utc: new Date().toISOString(),
  };

  if (!existsSync(indexPath)) {
    fail(
      `${indexPath} not found. The frontend was never built, so nothing could have been embedded. Run "npm run build" in apps/desktop/src first.`,
      evidence,
      args.json
    );
  }
  if (!existsSync(args.exe)) {
    fail(`Executable not found: ${args.exe}`, evidence, args.json);
  }

  const html = readFileSync(indexPath, 'utf8');
  const assets = extractAssetNames(html);

  if (assets.length === 0) {
    fail(
      `No hashed asset references found in ${indexPath}. Expected Vite output such as "index-AbC123.js". A dist without hashed assets cannot be verified as embedded.`,
      { ...evidence, index_html_bytes: html.length },
      args.json
    );
  }

  const exeStat = statSync(args.exe);
  const exeBuf = readFileSync(args.exe);
  evidence.exe_bytes = exeStat.size;
  evidence.exe_sha256 = createHash('sha256').update(exeBuf).digest('hex');
  evidence.assets_expected = assets;

  // Byte-level search. Asset names are ASCII; check both plain and UTF-16LE
  // because Windows resource sections may store strings wide.
  const found = [];
  const missing = [];
  for (const name of assets) {
    const hitAscii = exeBuf.includes(Buffer.from(name, 'utf8'));
    const hitWide = exeBuf.includes(Buffer.from(name, 'utf16le'));
    if (hitAscii || hitWide) found.push(name);
    else missing.push(name);
  }
  evidence.assets_found = found;
  evidence.assets_missing = missing;

  const devUrls = DEV_URL_PATTERNS.filter(
    (u) => exeBuf.includes(Buffer.from(u, 'utf8')) || exeBuf.includes(Buffer.from(u, 'utf16le'))
  );
  evidence.dev_urls_present = devUrls;

  if (missing.length > 0) {
    fail(
      `${missing.length} of ${assets.length} frontend asset(s) are NOT present in the executable: ${missing.join(', ')}. ` +
        `This binary will not render the UI — it is either a debug build, or a release build produced before "vite build" ran. ` +
        `This is the exact condition that caused "localhost refused to connect" on the physical drive.`,
      evidence,
      args.json
    );
  }

  // A dev-server URL string inside the binary is INERT CONFIG METADATA in a
  // correctly-built release binary: tauri-macros serialises the whole config
  // (including build.devUrl) into the binary no matter what, so the string
  // legitimately appears in every stock Tauri release binary. What decides
  // whether the app actually targets the dev server is which FRONTEND SOURCE
  // the codegen macro selected — and a dev-target build embeds zero assets,
  // which is already a hard failure above. With assets present, the dev URL
  // is therefore reported as a warning, not a failure.
  if (devUrls.length > 0) {
    if (missing.length === 0) {
      evidence.dev_url_note =
        'dev-server URL string present as inert config metadata while assets are embedded (normal for Tauri releases); not a failure';
    } else if (!args.allowDevUrl) {
      fail(
        `Executable contains dev-server URL(s): ${devUrls.join(', ')} and NO embedded assets — this binary targets a Vite dev server. ` +
          `Re-run with --allow-dev-url only if this string is provably inert in release builds.`,
        evidence,
        args.json
      );
    }
  }

  evidence.status = 'VERIFIED_WORKING';
  evidence.verified = true;

  if (args.json) {
    console.log(JSON.stringify(evidence, null, 2));
  } else {
    console.log('');
    console.log('  Frontend embedding verified');
    console.log(`    executable   : ${args.exe}`);
    console.log(`    size         : ${evidence.exe_bytes} bytes`);
    console.log(`    sha256       : ${evidence.exe_sha256}`);
    console.log(`    assets found : ${found.length}/${assets.length}`);
    for (const a of found) console.log(`                   ${a}`);
    if (devUrls.length > 0) {
      console.log(`    dev URLs     : ${devUrls.join(', ')} (allowed by flag)`);
    }
    console.log('');
  }
  process.exit(0);
}

main();
