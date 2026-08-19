# Source and Idea Ledger

This implementation is independent. The only inspected inputs were the two project-authored reports outside this Git repository:

- `../reports/HARNESS_SOURCE_AUDIT_DRAFT.md`
- `../reports/TECH_DECISION_DRAFT.md`

They describe architectural concepts observed in a pristine upstream checkout: explicit turn/step events, provider seams, capability-scoped tools, monotonic policy, cancellation ownership, unknown-outcome repair, and a Rust/C-ABI recommendation. No upstream source file was opened while implementing this repository, no upstream source text was copied, and no command wrote into the upstream checkout.

Project-specific contracts—including the anchored router grammar, standard-library JSON value/parser, JSONL envelope, FNV chain, Rust trait signatures, tool manifests, execution broker, job implementation, CLI, tests, examples, and ABI—were authored for InBharat Harness.

If code is later imported from a third party, this ledger must record exact path, revision, license, notice, modifications, and compatibility rationale before merge.

## Pinned baseline evidence

The upstream checkout was pinned at `47f943859bef60e4160492346772ded9b24f765a` (`0.1.0-rc.5` publication work). Its exact pnpm 11.7.0 dependency graph installed successfully and `pnpm run build` passed in the local Linux sandbox.

The complete upstream Vitest run executed 13,512 tests: 13,387 passed, 109 skipped, and 16 failed across seven files. The failures were retained as baseline evidence rather than patched upstream. They are dominated by permission-mode assertions that cannot be reproduced on the sandbox-mounted filesystem, plus related pretty-format output. Raw logs and a concise baseline summary are packaged under `reports/UPSTREAM_*`; they are not release claims for InBharat Harness.

The upstream working tree remained clean. InBharat Harness has no runtime or build dependency on DeepSeek Harness, Cordis, Node, pnpm, or a DeepSeek model.
