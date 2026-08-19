# DeepSeek Harness pinned baseline

- Revision: `47f943859bef60e4160492346772ded9b24f765a`
- Package version: `0.1.0-rc.5`
- Package manager: pnpm 11.7.0
- Dependency install: passed with lockfile frozen and supply-chain policy verification
- Build: `pnpm run build` passed
- Full Vitest run: 13,512 tests discovered; 13,387 passed, 109 skipped, 16 failed across seven files
- Duration: 976.95 seconds

The upstream source was not modified. The retained failures are baseline observations, not InBharat defects. Most failing assertions attempted to provoke permission errors with chmod; the sandbox-mounted filesystem continued to permit access, so the expected EACCES/denial did not occur. Related error pretty-printing also failed in that path. No upstream patch was applied.

Raw evidence: `UPSTREAM_BUILD.log`, `UPSTREAM_TEST_FULL.log`, and `UPSTREAM_ENVIRONMENT.txt`.
