# Local peer sync — v2 shared board plus preserved v1 review

**Current:** explicit fingerprint-pinned, both-screen v2 identity adoption and active causal persona/task convergence are implemented. See [UNIFIED_SYNC_V2](../../docs/UNIFIED_SYNC_V2.md) for consent, archived v1 records, selection, conflicts, ownership, claims, ACK recovery and exact limits. TLS/crypto remain unchanged; no new v2 dependencies.

The following describes the **preserved legacy v1 review mode only**. Its statements that adoption/convergence are absent do not apply to v2. Existing v1 pairings are not automatically rebound or upgraded.

# Local peer exchange v1 — opt-in, inert review preview

This is **not the full adopted §3.2 convergence milestone**. It provides real pinned mutual TLS1.3 LAN exchange of selected immutable personal-ledger changes. Foreign histories are retained encrypted and projected for review, not rewritten into the local ledger. No request, task, preference, grant or skill is dispatched/activated by hydration.

## Identity/history stop condition

The existing Rust `Ledger::view` and Kotlin `PersonalLedger.view` require one local linear predecessor chain and local replica IDs on requests/task specs. Independently created vaults have different person, public agent and genesis IDs. There is no reviewed adoption/cross-replica-union API. Changing foreign IDs or replaying foreign requests through local mutation would fabricate history. This package does neither.

Both screens offer exactly:

- **Same person:** requires matching person AND public agent IDs, and distinct replicas/keys. Independent fresh identities fail `IDENTITY_CONFLICT` without changing either ledger.
- **Keep separate for review:** explicit on both screens; receives the selected foreign history without adopting it. Persona and branch conflicts stay visible; original local work remains untouched. Different choices fail before persistence/ACK.

No reviewed resolution/adoption UI exists yet. A same-person pairing does not magically enable cross-replica edits: the local and peer branches still remain separate for review. This is a deliberate conservative boundary, not fake merge or last-writer-wins.

## Pairing and key custody

Opt-in starts at **Show my pairing identity**, not startup. Each device generates its own EC certificate/key. Manually copy the public JSON offer and compare **all 64 lowercase SHA256 fingerprint characters directly against the other screen**. Approve independently on both devices. No first-network-message trust, six-digit secret, CA wildcard or shipping test root. The offer contains version, replica/person/agent IDs and full-certificate fingerprint, never a private key or vault ID.

Rust uses exactly rustls 0.23.43 with ring, TLS1.3-only, mutual CertificateVerify, pinned exact certificate DER, no TLS tickets/resumption/0-RTT. Pin policy replaces public PKI/hostname/expiry policy; rustls's vetted signature verification still proves private-key possession. Certificates are local identities, not public web certificates. Power's PKCS#8 key and pin/selection are encrypted inside the existing unlocked local vault; not browser storage.

Android uses the real platform `SSLContext("TLSv1.3")`/`SSLSocket` and nonexportable AndroidKeyStore EC signing key. Android 10/API29+ is required for this lane; unsupported hosts fail without cleartext downgrade. The protected key alias is bound to the local vault; only its public cert is sent. Hardware-backed/StrongBox custody is NOT claimed on every device. Missing/replaced key, incompatible/tombstoned/corrupt peer state or orphan key after an interrupted first state save fails closed. Recovery/rotation/re-pair is not implemented, and cannot silently reset trust/history.

## Foreground LAN transport

Power main-window IPC `peer_sync_command`/`peer_sync_cancel`; Android `PeerSyncService` and `NativePeerHttps`. No automatic listener/sender. Power explicitly listens on a chosen numeric private-interface IP:port for **one request**, max 30s; Android initiates. Power also has an explicit peer-client action for Power↔Power tests/use. No wildcard/public bind, DNS, HTTP proxy, redirect, cloud account or relay. Rust accepts private/loopback/link-local IPv4 or IPv6; Android manual path is IPv4. Private addressing is not subnet attestation; VPN/firewall/routing remains an OS qualification issue.

Transport is a closed HTTP/1.1 subset inside mTLS: `POST /unoone-peer-v1`, fixed Content-Length, JSON, connection-close. Headers ≤2048 bytes; bodies ≤262144 bytes; no chunking/compression/redirect. Connect ≤3s, socket I/O ≤2s, total session ≤15s, page ≤8 mutations and ~128KiB encoded changes. Screen background/unmount/Stop closes or cancels sessions. Power checks lock epoch/generation at socket read/write and before commit; Android monitors epoch/screen-stop and closes sockets (50ms watchdog), and commits through the original session-bound writer. Bytes already in OS buffers cannot be recalled after stop/lock.

Both must remain running/unlocked. No NSD/mDNS, hotspot setup or Wi-Fi Direct API integration is implemented. A user-established LAN/hotspot may provide the private IP path, but no physical hotspot/Wi-Fi Direct success is claimed. Device firewall, local-network permission, OEM/background and actual Android TLS/Keystore behavior are release gates.

## Data and crash/replay policy

- Existing immutable encrypted personal ledger remains **the only authoritative outbound log**. Export reads validated runtime ledgers. No second outbox, no master/recovery-key transfer, no raw vault/aggregate file copying.
- Approved fixed selection contains persona/name history and explicit existing task IDs. New tasks do not automatically enter selection. Selection includes subsequent edits/tombstones to selected tasks; review their prose before each manual transfer. No semantic secret detector is claimed: users must not put credentials/raw mail in selected free text.
- A page carries version, exact sender identity, agreed identity choice, `after`, and immutable `{sequence, operation_id, predecessor_operation_id, payload?, content_hash}` entries. Skipped selection positions carry only IDs/sequence and an empty-payload hash, so global cursor gaps cannot silently discard selected entries. SHA256 covers exact UTF-8 payload bytes; TLS authenticates all metadata. The exact payload string survives Rust↔Kotlin round-trip.
- Payload contains only shared-contract PersonalAgent, Persona, TaskSpec and TaskEvent envelopes plus bounded draft/reminder/deletion state derived from local immutable requests. Unknown kinds/fields, grants, mail/provider/draft-account records and unsupported versions are rejected. The local Request object, vault binding and runtime authority are never sent. No receipt/provider/skill/conversation/notes sync is claimed for this lane.
- This is a new review-transfer schema, not a counterfeit `ReplicaChange.ciphertext`. `ReplicaChange` per-record ciphertext/key-wrapping integration still needs a reviewed contract. Payload is plaintext only inside an authenticated TLS channel and independently encrypted by each receiving vault at rest; it is not a separately encrypted portable blob.
- New remote operations must continue the peer predecessor chain. Identical retries deduplicate; collision/gap/out-of-order/schema/hash mismatch fails without ACK. Pending local mutations and received tombstones are never evicted.
- Candidate receive/cursor state is one existing-vault encrypted write **before ACK**. Power returns the persisted receive cursor; Android's next user-invoked request advertises only its persisted receive cursor. If reply/ACK is lost, replay is idempotent. No separate best-effort data/cursor write.
- Store bound: 4MiB/2048 received operations; runtime's own outbound cap unchanged. Exceeding a cap fails, does not compact/evict. No large attachment chunk feature.
- Foreign task events are retained and folded using shared conservative task fold (no native receipt injected). A claimed VERIFIED event stays awaiting verification. Per-peer delete tombstones suppress stale resurrection even under old-page replay; history remains encrypted (logical deletion, not secure erasure). Cross-local/foreign branch conflicts are retained separately, not discarded/auto-resolved.
- Revocation is encrypted and blocks subsequent sync. Previously copied records cannot be remotely erased; key rotation and re-pair/recovery remain release blockers.

## Narrow dependency addition (no licence-policy change)

Only new registry packages versus the owner-start lockfile: **rcgen =0.13.2** (`MIT OR Apache-2.0`) and lock-pinned transitive **yasna 0.5.2** (`MIT OR Apache-2.0`). Existing package versions did not move. rustls 0.23.43 (`Apache-2.0 OR ISC OR MIT`), ring 0.17.14 (existing bundled multi-licence notices), sha2/serde/serde_json/uuid/vault/contracts/runtime were already locked. New direct rustls feature enables existing ring/std; no AWS-LC, new Java/Android library or dependency download at runtime. This inventory is not a complete release SBOM/legal clearance. No root/vendor/Harness/Audio licence changes.

## Tests / qualification boundaries

Scoped `cargo test -p unoone-local-peer-sync --locked -j1 -- --test-threads=1`; clippy `--all-targets -D warnings`; fmt. Real independent encrypted temp vaults and loopback TCP/mTLS: spoofed server/client, actual filesystem write fault/no ACK, restart, replay, truncated authenticated stream, bounded HTTP, capture lacking fixture plaintext, cursors/selection, identity conflicts, delete convergence and forbidden grants/provider data. These are synthetic local fixtures, no real model/provider.

`examples/wire_interop.rs emit/check` and `tools/test_android.py` prove Rust→Kotlin→Rust payload/hash interchange and run JVM tests against actual protocol/native framing code. `--api` compiles actual Android Keystore/TLS/service/Compose/nav against Android35 and existing dependency/module jars. JVM does not execute AndroidKeyStore; isolated API compilation is not full Gradle/KSP/lint/APK, emulator/handset or physical TLS evidence. Power IPC is rustfmt-parsed; full native Tauri compilation/real IPC is a parent gate. Mounted React tests use the official mocked Tauri IPC, not fake network evidence.
