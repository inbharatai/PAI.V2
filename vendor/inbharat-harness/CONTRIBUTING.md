# Contributing

1. Keep the trusted core small and provider neutral.
2. Add no third-party dependency without a written threat, size, portability, and maintenance justification.
3. Never add ambient filesystem, environment, network, process, credential, or plugin authority.
4. Version durable events, manifests, and ABI changes before release.
5. Add cancellation, budget, replay, failure, and denial tests for every effectful feature.
6. Run `./scripts/check.sh` and include exact output in release evidence.
7. Do not copy source from audited upstream projects; record architectural influences in `docs/SOURCE_LEDGER.md`.

Changes to router rules require confusion-set additions. Changes to filesystem/process policy require traversal/symlink/cancellation tests. C ABI changes require header, manifest, Rust layout, and ownership documentation updates.
