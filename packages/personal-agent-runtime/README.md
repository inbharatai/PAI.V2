# Personal agent runtime — explicit v2 shared ledger

**Current:** `shared.rs` and Android `SharedLedger.kt` implement the opt-in v2 causal board, integrated with both active UIs and authenticated peer transport. [UNIFIED_SYNC_V2](../../docs/UNIFIED_SYNC_V2.md) is the current protocol/migration contract. Original v1 records remain unchanged; ordinary reads never adopt an identity. No execution authority is imported.

The following documents the **preserved v1 local lane**. Its future-sync limitations are superseded only after explicit both-screen v2 adoption; they remain accurate for unadopted v1 ledgers.

# Local personal identity and task ledger

This crate is an implemented **manual, local-only product lane**, wired to the existing Power vault and the Android `app/personal` Kotlin mirror. It is not a model, scheduler, provider adapter, agent factory or peer-sync implementation. The existing chat and TaskCoordinator remain intact beside the new panel.

## Durable authority

- Each unlocked vault lazily creates one stable public `PersonalAgent`, person UUID and distinct replica UUID when the personal panel is opened. These are encrypted, not stored in plaintext preferences. The ledger is bound to the actual local vault UUID. New installations create independent identities; no automatic identity adoption/merge exists.
- Fixed *local namespace* record ID `7683459b-5738-4d18-a824-b3485869673b`, existing `CONTEXT_SNAPSHOT`/canonical-AAD vault envelope. The fixed ID is NOT the person or replica ID and is not a cross-device record identity. Never copy this raw record or the vault to implement pairing.
- The `inbharat.pai.personal-ledger` v1 body contains the local binding and immutable append-only `mutations`. Each mutation has a consecutive sequence, unique operation ID, predecessor operation ID, original local request, and shared-contract envelopes. It **is** the authoritative pending outbox. There is no best-effort second outbox write or materialized state transaction to lose.
- All mutations load/validate/project/append/validate/write one existing encrypted vault record under the existing native session mutex. Power uses the vault-core journal/write API; Android uses VaultConnection's session-bound writer and existing atomic private-file API. Android honors historical-data quarantine, never writes separate Room task journals and never resets a corrupt envelope based on a metadata listing that skips malformed files.
- Revision and expected-replica compare-and-swap reject stale screens. Retrying an identical operation ID is idempotent; reuse with a different request is rejected. Native commands are main-window only; lock epochs prevent a queued Power edit from crossing sessions. No plaintext cache survives lock in the runtime.

## Contracts and consent

Every Persona, PersonalAgent, TaskSpec and TaskEvent is validated through the existing shared contract boundary on write and read. Task projection uses `fold_task` / `foldTask`; hydration cannot dispatch. The owner used by projection comes from the local authenticated vault binding, not a wire event's newest claimed assignment.

Create, edit, accept-for-review, snooze, cancel, delete and view are real local mutations in both UIs. Accept moves to `READY_FOR_REVIEW`, **not** `IN_PROGRESS` or `VERIFIED`; it records a local human decision but constructs no grant. Editing an already-reviewed draft holds it `BLOCKED` for fresh review. Snooze persists a time and hold state, not an OS notification or scheduler. No completion action is exposed. A hypothetical wire VERIFIED event without actual host verification projects `AWAITING_VERIFICATION`; this lane never manufactures `VerifiedReceipt` from JSON or a model report.

Persona preferences are user-authored corrections with provenance/revision. Clear preferences and delete task append logical tombstones. Old encrypted values remain in the bounded mutation history for later conflict review; **not secure erasure**. Task IDs cannot be resurrected by recreation. The UI states these semantics before confirmation. Persona preferences are not yet injected into existing chat prompts.

No provider-token/password fields, imported grants, side effects, cross-device handoff or fabricated model responses exist. Inbox/calendar have explanatory unavailable UI, not simulated adapter success.

## Bounds and future sync

4 MiB decrypted ledger, 2048 total mutations, 128 task IDs (including deleted), 4096 UTF-8 bytes per editable field, existing 1024-event fold bound. Caps fail without eviction or silent compaction; they are visible in the UI. The entire ledger is validated on each mutation: suitable for this bounded initial lane, not an unlimited transcript store.

Future pairing/sync must explicitly bind person identity, allocate its own distinct local replica and keys, consume the mutation sequence, authenticate provenance, resolve concurrent persona/task ownership, and construct the existing `ReplicaChange` wire contract. Current mutation entries are **not authenticated network ReplicaChange envelopes** and must not be sent as grants. There is no acknowledgement/compaction/peer cursor or conversation synchronization here. Pending mutations are retained rather than falsely marked synced. This format's local vault binding must not be silently rewritten from peer data.

## Tests and integration gates

- `cargo test -p unoone-personal-agent-runtime --locked -j 1 -- --test-threads=1`: actual vault create/write/restart, independent vaults/identities, foreign-key and corrupt-data rejection without reset, encrypted secrets scan, persona/task deletion, idempotency/collision/stale revision, edit/snooze/cancel, causal fold/fork/gap and no model verification.
- `tools/test_android.py`: isolated current Kotlin 2.2.21 compiler plus existing runtime jars, actual private-file vault/crypto persistence tests and pre-atomic-replace fault (state and outbox remain together). `--api` compiles actual new service/Compose UI/nav against existing Android 35/dependency/module classes; not a full Gradle/KSP or device run.
- `examples/ledger_interop.rs` and Kotlin `PersonalLedgerInterop`: synthetic Rust→Kotlin mutation→Rust ledger/contract compatibility. Not a production import/merge operation and not a model fixture.
- Power `personal-agent-mounted.test.mjs`: mounted React plus official mocked IPC, testing user controls and truthful unavailable/consent/error copy. Native persistence is tested separately; mock IPC is not native end-to-end proof.

Parent release gates remain full Tauri/Gradle integration, Android mounted/device journeys, real WebView IPC, crash/disk exhaustion/OS filesystem qualification, reviewed pairing/sync and providers, and persona-to-existing-chat integration. No commits, mobile golden regeneration or licence changes are performed by this slice.
