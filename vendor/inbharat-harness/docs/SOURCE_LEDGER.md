# Source and Idea Ledger

This implementation is independent. The only inspected inputs were the two project-authored reports outside this Git repository:

- `../reports/HARNESS_SOURCE_AUDIT_DRAFT.md`
- `../reports/TECH_DECISION_DRAFT.md`

They describe architectural concepts observed in a pristine upstream checkout: explicit turn/step events, provider seams, capability-scoped tools, monotonic policy, cancellation ownership, unknown-outcome repair, and a Rust/C-ABI recommendation. No upstream source file was opened while implementing this repository, no upstream source text was copied, and no command wrote into the upstream checkout.

Project-specific contracts—including the anchored router grammar, standard-library JSON value/parser, JSONL envelope, FNV chain, Rust trait signatures, tool manifests, execution broker, job implementation, CLI, tests, examples, and ABI—were authored for InBharat Harness.

If code is later imported from a third party, this ledger must record exact path, revision, license, notice, modifications, and compatibility rationale before merge.
