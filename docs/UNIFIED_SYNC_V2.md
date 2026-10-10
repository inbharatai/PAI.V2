# Explicit shared personal identity and causal board, v2

This is a manual, bounded **implemented** convergence lane, not an exactly-once side-effect system. Power and Android keep distinct encrypted vaults, local replica IDs and TLS keys. Existing chat/model/provider/storage/crypto code is unchanged. No new dependencies were added for v2.

## Consent and migration

1. On each device open local pairing, exchange public offers and compare the **complete pinned certificate fingerprint** against the other screen.
2. On BOTH screens choose **Unify identity and archive old local board (v2)**, explicitly acknowledge archival, and select categories. Approving locally only stores consent: it does NOT change the active identity.
3. Run the existing pinned mutual-TLS exchange (Power Listen; peer Sync). A matching, authenticated version-2 page confirms the other approval. Only then does the receiver adopt the deterministic person/agent from the original offer with the lexically smaller replica UUID. The identity record contains both distinct replicas and approved fingerprints. Old offers remain the pairing identity, not a claim that keys or local vaults have changed.
4. The new shared board starts empty. **All original v1 mutations remain unchanged as an encrypted local archive**, with their original identities, requests and contract IDs. They are neither rewritten, automatically imported nor sent by this mode. The active board displays the archive count. There is deliberately no silent rebinding of current tasks. A user can manually create a new shared task using reviewed text, but no automated archive-import or archive-browser UI is implemented.

`Ledger::decode` / Kotlin `PersonalLedger.decode` explicitly read version 1 and version 2. Version 1 must not contain shared state. Version 2 validates the unchanged embedded v1 archive with the v1 reader, then validates the separate `shared` state. Unknown versions, corruption and binding conflicts fail without reset. V1 reads remain v1; opening a ledger does not migrate it. Existing review-only pairings remain review-only and cannot silently upgrade/reset history. Existing bound-pair upgrade, reset, rotation and recovery require a future explicit migration; ambiguous histories are held rather than reinterpreted.

## Authoritative state and selection

The personal encrypted ledger remains both authoritative outbox AND active projection source. It now retains per-replica `SharedOperation` chains: distinct replica, unique operation UUID, consecutive bounded sequence, previous operation UUID, causal context (two-replica vector), and selected declarative body. Local retry requests stay local and are NOT sent. Original v1 archive, vault IDs, private TLS keys, provider credentials and local grants never enter v2 payloads.

The existing peer store retains pins, fixed selection, received wire strings and ACK cursor. V2 pages reuse the bounded TLS framing, but have explicit `page.version = 2` and choice `UNIFY_ARCHIVE`. Hashes cover exact UTF-8 payload strings; TLS authenticates metadata and content. This is a TLS-protected causal-change protocol, **not** the older `ReplicaChange.ciphertext` portable per-record encryption format. No homemade crypto or weaker TLS was introduced.

Selection has independent persona and shared-task categories. The explicit UI option “all new shared tasks” uses the reserved nil UUID in `task_ids`; v1 does not interpret that ID as a wildcard. Unselected operations send only a causal marker (`body: null`), never skipped prose. Selection is fixed, so old selected dots cannot later change payload. Equal active projections are expected after complete sync when both users approve the same full categories; deliberately asymmetric selection can intentionally yield different content. No automatic background sending or semantic secret detector exists.

TaskSpec requires scope-shaped contract fields, but v2 accepts only **empty capabilities/tools/data/recipients/hosts/operations**, read-and-suggest declarative level and offline-only policy. No native execution scope is exported, reconstructed or delegated from this scaffolding.

## Merge semantics

- Every replica chain and operation ID is immutable. Replay is idempotent. Gaps, rollback clocks, cycles/transitive-clock violations, changed operations and task-creation/identity/deadline/assignment collisions hold the whole candidate without ACK or overwriting accepted history.
- Persona/name and task goal/draft-notes use causal multi-value heads. Concurrent branches remain retained and explicitly visible in the active board conflict review. They do not choose a timestamp or last-writer winner. A later explicit user correction observing both heads resolves the editable values; old alternatives remain in history.
- Events and draft/reminder changes are append-only. Concurrent reminder changes produce a review hold; task deadlines and creation identity remain immutable. Task deletion is monotonic: any tombstone suppresses stale edits/replay/recreation. Persona clear is also monotonic in v2; a subsequent new profile after clear needs an explicit future migration. Logical deletion is not secure erasure.
- Task creation writes an explicit `SharedAssignment { owner_replica_id, epoch: 1 }`. Only the creator can be initial owner; assignment cannot be overwritten by later editable records. Handoff records remain declarative inert claims and **do not** advance the epoch or transfer execution ownership. Cross-owner event chains remain conservative review holds.
- Hydration/execution is ALWAYS disabled here, including for connected known owners. Unknown, disconnected or ambiguous ownership cannot execute. There is no scheduler, native grant issuance, dispatcher or exactly-once claim.
- TaskReceipt and Handoff envelopes may be retained as observed peer claims. Original `source`/`outcome` remain unchanged. UIs label these **remote observed claims, not native completion**; they are never converted to `VerifiedReceipt`. TaskEvent wire VERIFIED does not establish local verification. No provider/native receipt producer is added by this lane.

## Persistence, interruptions and ACKs

Native hosts serialize through the existing vault/session boundary. On receive they validate the pinned peer/choice, merge into a candidate runtime ledger, persist that ledger, and only then save the peer cursor and ACK. There are two existing encrypted writes, **not** an invented cross-record transaction:

- Failure before runtime commit leaves the prior accepted state.
- Failure after runtime commit but before peer cursor save leaves already-merged data with an old ACK; retry repeats identical dots and safely repairs the cursor.
- Lost or partial ACK causes retransmission, never eviction.
- A collision never silently replaces an accepted dot.

The actual Rust test uses real independent encrypted temp vaults and real loopback mTLS, injects an interruption between the two writes, retries with partial ACK, restarts, and compares active projections. Existing filesystem-fault/no-ACK and TLS spoof/revocation tests remain in the v1 regression suite. OS power-cut guarantees remain those of the existing vault writer.

## Bounds and limits

4 MiB decrypted personal ledger; at most 2048 archived v1 mutations plus 2048 shared causal positions across both replicas; 128 shared task IDs including tombstones; 4096 UTF-8 bytes/editable field; 1024 events/task. The peer history remains 4 MiB/2048 received operations, 8 changes/page and bounded HTTP body. Caps fail without compaction or eviction. Archive bytes count against the same 4 MiB ledger cap.

Physical Android TLS/Keystore, OEM lifecycle/network behavior and actual device filesystem crash tests remain pending. Android service/Compose API compilation and JVM protocol/store tests are not device proof. Full coordinated Gradle and full Tauri/native packaging remain parent gates. There is still no multi-peer sync, automatic discovery, conversation history, standalone note entity, provider-account sync, notifications, key rotation, re-pair/reset UI, archive-import UI, executable ownership transfer or chat-prompt persona injection.

## Narrow runnable gates

- `cargo test -p unoone-personal-agent-runtime -p unoone-local-peer-sync --locked -j1 -- --test-threads=1`
- `cargo clippy -p unoone-personal-agent-runtime -p unoone-local-peer-sync --all-targets --locked -j1 -- -D warnings`
- `packages/local-peer-sync/examples/shared_interop.rs emit/check` plus `tools/test_android.py`: shared Rust fixtures → Kotlin active projection comparison → Kotlin reviewed edits/reminder → Rust import/projection equality. Legacy wire/ledger fixtures remain regression gates.
- Mounted `peer-sync-mounted.test.mjs` and `personal-agent-mounted.test.mjs`: explicit separate archive confirmation, selection and active conflicts/claims via official mocked IPC; not native networking evidence.
