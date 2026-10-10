# Personal-agent contracts v1

## Scope and status

Implemented foundation for adopted brief §§3.1 and 3.5: **14 typed Rust/Kotlin record contracts, a checked-in JSON schema, shared golden fixtures, bounded codecs, native-authority separation, exact child scope/budget attenuation, and a pure deterministic task-event projection.** This is not peer sync, encryption, account connection, provider integration, a scheduler, model admission, or completed app/UI integration.

- Rust package: `packages/personal-agent-contracts`, crate `unoone-personal-agent-contracts`.
- Android package: `com.unoone.agent.core.personal`, in `core/src/main/java/.../personal`.
- Existing `capability.v1`, `tools.v1`, vault records and Android tool IDs are unchanged. The scope fixture uses the existing `draft_email` ID and a Rust drift assertion checks it against `tools.v1.json`; declaring it is not proof of an email adapter or permission to use it.
- Runtime dependencies are only the workspace's existing serde/serde_json versions. Kotlin uses the already configured serialization library. There is no new cryptography, transport, model, or provider dependency.

## Wire format and compatibility

```json
{
  "schema": "inbharat.pai.personal-agent",
  "version": {"major": 1, "minor": 0},
  "kind": "PERSONAL_AGENT",
  "payload": {
    "agent_id": "agent-1", "person_id": "person-1",
    "profile_revision": 1, "display_name": "UnoOne", "persona_revision": 1,
    "conversation_refs": ["thread-1"], "capability_preferences": ["mail.read"]
  }
}
```

`personal-agent.v1.schema.json` defines the structural JSON format. `tools/generate.py` is the common declaration source for the schema, Rust DTOs, Kotlin DTOs/dispatch and the 14 synthetic examples. Handwritten codecs add semantic invariants that JSON Schema alone does not express. Both language test suites consume the **same files**, not copied Android resources.

Use Rust `decode(&[u8])` / `encode(&Document)` or Kotlin `PersonalCodec.decode(ByteArray|String)` / `encode(PersonalRecord)`. Individual serde/Kotlin DTO deserialization is not a validated native boundary. Every wire record is an inert claim; `Document::may_execute_on_hydration()` / `HydratedDocument.mayExecuteOnHydration` is always false. Writers validate too.

This is a **new v1 namespace**, not a rename of an existing v1 record to v2. Readers and writers round-trip every checked-in v1 record in both directions. There was no prior personal-agent record format to migrate; no fictional v0 converter or destructive old-vault migration is included. Existing capability/vault compatibility readers remain responsible for their own namespaces. Future migration must have explicit source/target versions and new round-trip/rollback fixtures before enabling writes.

Version policy is deliberately closed: unknown schema, major, minor, record kind, enum and object field are rejected without a mutation. Nullable fields are required and explicitly `null`; omission is rejected. The only omitted-field defaults are `READ_AND_SUGGEST` delegation and `OFFLINE_ONLY` network policy. Re-encoding emits those defaults. Duplicate JSON object keys, including escaped duplicate names, are rejected rather than silently taking the last value. JSON key order/whitespace are not significant; encoding is **not** a cryptographic canonicalization algorithm.

### Native input limits

| Limit | Value |
|---|---:|
| UTF-8 encoded envelope | 65,536 bytes |
| Container nesting | 12 |
| Array items / object members | 64 |
| String value | 4,096 UTF-8 bytes |
| Opaque identifier | 1–128 ASCII graphic characters |
| Exact scope token | 1–256 ASCII graphic characters, no `*` |
| JSON integer | 0–9,007,199,254,740,991; no fractional values |
| Task/event steps / tool calls / network calls | 1,024 maximum each |
| Task/grant window or duration | 24 hours maximum; positive |
| Budgeted data bytes | 64 MiB maximum |
| Agent depth / children | 2 maximum each |
| Pure fold input batch | 1,024 events |

These are hard ceilings, not automatically granted budgets. Scope tokens are exact opaque identifiers/ASCII address references; no prefix, wildcard, case-folding, Unicode, email-domain or path expansion is performed. Internationalized addresses/files require a reviewed native mapping to stable opaque resource IDs. User prose remains Unicode. Both codecs reject invalid Unicode/UTF-8. Native ingress must also cap stream/buffer allocation before constructing an input value.

## Record inventory

| Record | Meaning / constraints |
|---|---|
| `Persona` | Stable person ID, revision, user-authored preferences, observed/approved/corrected distinction, provenance, sensitivity, correction revision and deletion. Deleted records carry no preferences. Approval/correction requires a USER provenance **claim**; authenticated ownership must still be established by the host. |
| `PersonalAgent` | One stable public `agent_id` tied to `person_id`, profile/persona revisions, thread references and capability preferences. It has no device instance ID. Phone and Power use the same agent ID; each vault/host has a distinct replica ID. Child IDs never become public personas. |
| `TaskSpec` | Stable task/agent/person IDs, idempotency key, goal, origin/target replica, capability/tool/data/recipient/host scopes, host requirements, deadline/budget, postcondition and visible policy. Store encrypted; never put the goal into metadata-only journals. |
| `TaskEvent` | Immutable event and operation IDs, causal predecessor, task, origin and assigned replica, step/deadline, transition, evidence and external object reference. No timestamp decides precedence. |
| `AgentSpec` | Parent task, private child ID, purpose/template version, local-qualified-only model rule, scopes, bounded budgets/depth, expiry, stop generation and native verifier ID. It is a proposal, not a runnable continuation. |
| `TaskReceipt` | Wire **claim** identifying task, operation, attempt, actor/replica, before/after evidence, dispatch intent, effect, outcome, reason/source, timestamp and external object ID. Source=NATIVE on wire is still a claim. |
| `Handoff` | Pending encrypted declarative goal, receipt operation references, target/requirements/expiry and approval claim. No tools, click replay, process command, payment/send continuation or authorization-token field. Receiving approval does not activate permission. |
| `ReplicaChange` | Person/replica, positive local sequence, operation/record IDs, record kind/revision/predecessor, ciphertext **or** tombstone, content hash syntax and provenance claims. Local grants are explicitly forbidden as replicated record kinds. No hash/authentication verification or sequence persistence is performed here. |
| `MailAccount` | Account/person/provider identity, address and low-sensitivity display metadata only. No token, password or protected-store handle. Provider enums identify references, not qualified adapter support. |
| `MessageRef` | Account/message/thread/folder references, not inbox contents. |
| `Draft` | Task/account, optional same-account reply reference, exact recipients, subject/body, attachment references, revision and approval claim. Never an automatic send. |
| `CalendarRef` | Account/calendar identity, name and time zone. |
| `EventRef` | Calendar/event ID, increasing start/end, consistent time zone and attendees. Adapter must additionally validate real IANA zone, recurrence, time ambiguity, saved provider values and permissions. |
| `CapabilityGrant` | Device-local grant **description/request**, with exact scopes, budgets, validity, generation and revocation. Serializing/deserializing this DTO does not create runtime authority. It must not enter the replicated log. |

No raw credential fields exist. Unknown `password`/`access_token` properties are rejected. Free-form prose is not a secret detector: callers must prevent accidental credentials in drafts/goals/provenance text, authorize replicated metadata, encrypt sensitive payloads and never log them. Entire inboxes/attachments are not implicitly replicated.

## Authority and truthful verification

### Child attenuation

Native code obtains a `LocalGrant` via Rust `authority::approve_locally` or Kotlin `LocalGrant.approveLocally` **only after consulting actual local human consent/approved policy storage**. These constructors cannot prove that a caller collected consent; they are the trusted integration seam, not JSON entry points. `LocalGrant` has no serializer/deserializer and no model/peer conversion. Kotlin snapshots mutable collections so later DTO edits cannot widen an approved grant.

`attenuate_child` / `attenuateChild` computes:

```
requested scopes ∩ live local parent grant ∩ native host capability manifest
```

Each dimension is intersected independently: tools, capability IDs, exact data resource and operations, recipients, hosts, operations, delegation level and network policy. The result is sorted/deduplicated deterministically. Budgets take minima; expiry cannot exceed the parent, duration cannot exceed remaining time, and an offline-only result gets zero network calls. Wrong host, expired/revoked parent, stopped generation, missing granted host and excess depth are rejected. Unknown/empty scopes are never replaced with broad defaults.

The host manifest and consent reference must come from native trusted state, **not another parsed proposal**. This function does not spawn or reserve resources. A scheduler must atomically allocate remaining budgets across siblings, enforce actual parent/child depth, bind the real parent task, and recheck revocation, ownership, recipient/operation authorization and stop generation before every dispatch. Caller-supplied “remaining budget” must reflect real accounting. Long-lived standing grants require explicit renewal beyond this v1 24-hour validity ceiling.

### Native evidence

All deserialized receipts are claims, even when `outcome=VERIFIED`. A model may describe completion; it cannot create a `VerifiedReceipt` through deserialization. This non-serializable runtime wrapper is made only by `authority::verify_native` / `VerifiedReceipt.verifyNative`, after native `NativeObservation` is supplied. Checks bind task, operation, host and observed evidence; provider mutations require a reconciled nonempty external object ID that matches the receipt. Host integrations must supply actual independently observed evidence, never convert model JSON into `NativeObservation`.

`OPEN_COMPOSER` and `OPEN_EVENT_FORM` can never produce a verified-goal wrapper; the wire validator also rejects a `VERIFIED` receipt for those dispatch intents. UI launches can be `ACTION_VERIFIED`, not evidence that email was sent or an event saved. An unverified/claimed remote receipt stays untrusted after hydration; authenticated provenance and native revalidation are a separate integration task.

Exact wire outcomes: `VERIFIED`, `ACTION_VERIFIED`, `RESPONDED`, `UNVERIFIED`, `NEEDS_USER`, `FAILED`, `CANCELLED`. Recommended meanings are complete explicit goal, bounded observed action, response only, not independently verified, needs a human decision, failed, stopped. Consumers must not collapse action-only/response-only into goal completion.

## Pure task projection

`fold::fold_task` / `foldTask` is a small reusable normalizer, not a sync engine:

1. Validate bounded events and require one task ID.
2. Deduplicate identical immutable event/operation pairs. Reusing either identity with different content is `CONFLICT`, independent of arrival order.
3. Require separately resolved authenticated ownership. Missing, mismatched or changing ownership yields `WAITING_FOR_OWNER`; there is no automatic failover.
4. Require one PLANNED causal root, known predecessors and a single monotonic valid transition chain. Forks, cycles, illegal transitions or changed deadlines remain conflicts; gaps are `MISSING_PREDECESSOR`. No timestamp last-writer-wins.
5. A final wire `VERIFIED` event projects to `AWAITING_VERIFICATION` unless a matching native `VerifiedReceipt` wrapper exists for task/operation/owner/evidence/external object.
6. Return sorted event IDs and status. Execution-on-hydration is **always false**, even for fully native-verified history.

Wire transitions: `PLANNED`, `WAITING_FOR_ACCESS`, `DRAFTED`, `READY_FOR_REVIEW`, `IN_PROGRESS`, `AWAITING_VERIFICATION`, `VERIFIED`, `BLOCKED`, `CANCELLED`. Projection-only states: empty, owner unresolved, missing predecessor, conflict. Root→in-progress→awaiting-verification→verified is supported, as are draft/review/access branches and explicit block/cancel transitions. Completed/cancelled history is terminal. This conservative v1 does not merge concurrent draft edits or implement an ownership-transfer protocol: a real handoff requires a separately authenticated epoch/assignment design before projecting cross-owner continuation as one valid chain.

The projection does not guarantee exactly-once effects, reconcile provider retries, verify hashes/signatures, persist cursors/outboxes, or negotiate peers. Those remain explicit later phases. A hydrated task is never enqueued by this package.

## Reproducible targeted tests

From repo root, after `. /agent/workspace/toolchains/env.sh` in the integration environment:

```sh
CARGO_BUILD_JOBS=1 cargo test -p unoone-personal-agent-contracts --locked -j 1
cargo fmt -p unoone-personal-agent-contracts --check
cargo clippy -p unoone-personal-agent-contracts --all-targets --locked -j 1 -- -D warnings
python3 packages/personal-agent-contracts/tools/test_kotlin.py --jars target/personal-kotlin-jars
```

The isolated Kotlin runner performs no downloads and does not invoke Gradle/Android. Supply the existing Kotlin 2.2.21 compiler + serialization compiler plugin, serialization JSON/core JVM 1.8.0, JUnit 4.13.2, Hamcrest 1.3, and compiler runtime jars. The integration environment's ignored `target/personal-kotlin-jars` cache was bootstrapped from Maven Central for this run. Compiler runtime jars include stdlib/script-runtime/daemon 2.2.21, reflect 1.6.10, coroutines-core 1.8.0 and annotations 13.0; none were added as product dependencies. Compilation uses a 768 MiB JVM and tests 512 MiB.

At source freeze: **11 Rust tests passed; 11 isolated Kotlin/JUnit tests passed**. Both suites round-trip all 14 shared golden records and reject all 35 shared semantic/structural negative mutations, plus hard size/depth/duplicate-key/UTF-8 cases. Tests cover inert hydration, read-only defaults, scope intersection, bounded expiry/budgets, native-verification separation, provider-ID mismatch, causal permutation/dedup, conflicts, missing predecessors, unknown ownership and no action from a verified history. Rust checks the unchanged `draft_email` tool ID; Kotlin tests immutable local-grant snapshots. Targeted Rust formatting and clippy are separate recorded gates.

To regenerate DTOs/schema/examples, run `python3 packages/personal-agent-contracts/tools/generate.py`, then `cargo fmt -p unoone-personal-agent-contracts`, and rerun both suites. Review generated schema/API/fixture changes; this is not a reason to replace a golden expectation blindly.

Full Android Gradle, full Rust workspace, physical devices, actual mail/calendar accounts, local pairing/transport, encrypted persistence and UI flows were **not** tested by this contract task. No claim of provider qualification or complete peersync follows from these unit tests.
