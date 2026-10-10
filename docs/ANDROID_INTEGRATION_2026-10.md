# Android integration — October 2026

## Provenance and scope

Integrated selectively from standalone mobile `2eb9b9e38c36b3ce599abc3505713a4c4844e26b` onto PAI base `60a342623ebe0368a3187f4beb54343384911a00`. This is source integration, **not a signed-upgrade or physical-device qualification receipt**. No AnythingLLM source is incorporated.

The source task coordinator, cancellation/ownership gates, unified voice intake, bilingual recognition, native device/Owl/Qwen runtimes, model qualification policies, SkillsV2, unprivileged browser DOM adapter and evaluation corpus are retained. Source native CPU/arm64 pins, NDK 27.2.12479018, CMake 3.22.1 and notices remain authoritative; desktop native dependencies are not substituted. Source unit/instrumentation tests are imported, including the latest bounded-action evidence tests.

All 72 target-only Android paths remain. Vault module, USB identity/unlock UI, target schema, pending writes/tombstones, environment learning, conversation storage, and opt-in boot preference are preserved rather than replaced by standalone equivalents. Boot cannot override database recovery admission or Android microphone restrictions. Settings retain canonical BCP-47 aliases and distinct reply-language versus bilingual-input recognition.

## Data retention and recovery — release blocker

The existing target database remains SQLCipher-encrypted, Room **schema 6**, with migrations 1→2→3→4→5→6, original filename, wrapped-key filename and Keystore alias. User notes, memories, skills, transcripts, pending writes, tombstones and model metadata are not intentionally reset. Existing preferences, model files, SkillsV2 files and task journals are not deleted by integration.

**Standalone plaintext Room schema-2 import is NOT implemented or claimed compatible.** Before key generation or Room open, the provider detects the SQLite plaintext header and stops. It similarly refuses missing original keys, incomplete existing files, orphan database sidecars, unreadable wrapped keys, and failed authenticated opens/migrations. No destructive Room fallback, automatic data deletion, key replacement, or reinitialization is used. MainActivity presents a recovery explanation and closes safely instead of starting automation. The persisted user-enabled preference is not overwritten by this temporary admission failure.

Recovery instructions: preserve the original installation, original key material, main database and sidecars; do **not** uninstall, clear storage, replace keys, or downgrade as a repair. A future plaintext conversion must establish a coherent snapshot including WAL, authenticate schema/version, export every retained record and retry ledger, encrypt into a separate destination, validate counts/content/integrity and migrations, and commit with a tested crash-recoverable switch. Without that implementation and device tests, plaintext users must not be told this is an install-ready upgrade. Lost device-bound Keystore keys may not be recoverable; no recovery guarantee is made.

Target vault-cache expiration/detach semantics remain for clean mirrored records. Local-only rows survive. Dirty *linked* rows now also survive expiration/detach: pending writes are persisted before checking whether the vault is available, included in backlog drains, and excluded from cache deletion. Hydration defers updates/tombstones for records with outstanding local writes. Explicit deletion still tombstones, and privacy expiry of action logs remains the target behavior. User-edited skills are not overwritten merely because their names match built-ins; refresh requires exact pre-upgrade definition matching and preserves enabled state and vault linkage.

These are source/host checks, not proof of on-device SQLCipher migration, crash consistency, detached-edit concurrency, or ABI coexistence. Those remain release gates.

## Canonical tools and safety

All **42 canonical tool schemas** and their human-readable descriptors remain compatible with `packages/tool-contracts/tools.v1.json`; the root tool-sync script is unchanged. Native typed validation, immutable task tool+argument handle, resource ownership, before-effect journal and permission/confirmation gates remain in the execution route.

- Navigation aliases route through the source controls. Runtime safety for atomic accessibility aliases is at least the source `system_control` tier, even where stable public contract metadata describes a lower baseline.
- Historical opaque node-ID mutation aliases are still recognized contracts but explicitly refuse execution: an old node ID is not current snapshot/package/semantic/review authority. Users must use the grounded native device route. Horizontal legacy scroll similarly requires manual handover. No unsupported alias is reported as a completed action.
- Contact resolution refuses absent or ambiguous recipient matches instead of choosing the first row or treating an unresolved name as a number. `READ_CONTACTS` is declared for this actual, explicitly permission-gated use.
- WhatsApp compatibility actions open a reviewable draft and never press Send. Calendar creation opens a reviewable event form and never claims the event was saved.
- Source deterministic parser outputs are retained; aliases do not force the old parser/model execution path back into production. Legacy candidate/model helper types and descriptor APIs remain present, but source task/runtime authorization is authoritative.
- Environment-learning callbacks survive. Legacy executor success is recorded conservatively as unverified telemetry, never promoted to verified postconditions simply because a call returned success. User skill approval gates remain intact.

## Bounded-action evidence and persisted vocabulary

`ACTION_VERIFIED` remains distinct from wider task success in task summaries, voice reports, Task Board, audit/log labels and latency export. The append-time terminal guard now includes `ACTION_VERIFIED`, so an asynchronous stale RUNNING/CANCELLING summary cannot replace terminal action evidence. Host and instrumentation regressions cover that ordering.

Unknown persisted journal enum values fail closed with `RECOVERY_FAILED`, preserve bytes and block further journalled effects. They are not silently mapped to a successful outcome, reset, or replayed. This is safe refusal, not forward/rollback compatibility: older binaries may reject the new vocabulary. Task-journal format version 2 and latency schema version 1 retain the source vocabulary expansion; consumers must preserve the distinct outcome.

## Browser build provenance

The former privileged generated bundle is replaced with the source `UnoOneDomAdapter` entry. The upstream Vite build and original Vitest/Playwright tests remain available and unchanged.

In this environment, npm registry requests returned HTTP 403, so the **upstream npm/Vite/typecheck/Vitest/Playwright pipeline has not passed**. A dependency-free packaging path is provided because the production entry is already a complete, import-free JavaScript IIFE:

```
node web-runtime/page-agent-unoone/scripts/build-dom-adapter.mjs
node --test web-runtime/page-agent-unoone/tests/adapter-host-smoke.mjs
```

It syntax-checks the source, refuses module imports/exports, copies exact source bytes to both dist and Android assets and prints their hash. This is transparent source packaging, not a claim of Vite output or browser/device qualification. Host tests assert byte identity, unprivileged-only methods, absent page session authority, stale-target rejection and invalid-navigation refusal. Production generated-asset protection must include these final bytes.

## Version, signing and validation

Application ID remains `com.unoone.agent`. Integrated version is **10 / 0.9.0-alpha-integrated**. Original signing configuration is retained; no key is generated or stored in the repository. Deployed certificate/version inventory and the authorized signing key are unknown. Matching package IDs and a higher version code do not establish signed-upgrade compatibility.

Actually run during integration: unchanged tool and speech sync scripts; repository/model/native source invariants; 15 Python invariant/mutation/retention tests (including execution of DAO SQL against SQLite fixtures); device-agent corpus validator; exact-IIFE packaging and three Node host smoke tests. Corpus remains **100 pending, 0 physical passes**. No source annotation count is a passing-test receipt.

Full Gradle unit/lint/assemble/native builds were explicitly deferred for parent-orchestrated resource coordination. Android hardware tests, signed installation upgrades, SQLCipher+Sherpa+LiteRT/native ABI behavior, voice acoustics, Owl/Qwen performance, thermal/battery, browser e2e, vault attachment/detachment and crash/rollback recovery remain pending. Protected-tree/golden rebaseline and cross-platform monorepo checks belong to the parent integration phase; this owner did not edit root contracts, workflows, golden hashes, README, packages or desktop applications.
