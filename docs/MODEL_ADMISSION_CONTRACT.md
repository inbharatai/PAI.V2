# Model admission and provisioning policy contract v1

## Status and ownership

`packages/model-admission` (`unoone-model-admission` 0.1.0) implements a **pure Rust policy core**, not a production installer, wizard, model server, cryptographic library or real device qualification. It follows the adopted revised brief §3.4. It does not change either app's current admission behavior. No models, signing keys, real-device support claims, new dependencies, vendor code or licences are introduced. Repository/crate licensing awaits the rights holder's decision; the existing root licence conflict is not resolved here.

Versioned, strict serde DTOs are authoritative for this **new contract**, not replacements already connected to current app/catalog wire formats. JSON uses snake_case field names, SCREAMING_SNAKE_CASE enum values, exact unsigned integer bytes/timestamps, and rejects unknown struct fields. `schema_version` must equal 1 at the validating boundary. `decode` limits a payload to 262,144 bytes. Missing measurements are `{"provenance":"UNKNOWN"}`, not guessed zero. Android/Kotlin parity implementation and its test execution remain pending; `tests/fixtures/admission-v1.json` is the shared synthetic source of truth for future adapters.

## Modules / public integration surface

| Module | Surface | Responsibility |
|---|---|---|
| `dto` | `DeviceProbe`, `CatalogCandidate`, `AdmissionRequest`, `QualificationRecord`, `Decision` | Versioned data, explicit provenance and evidence scope |
| `trust` | `verify_candidate`, `validate_qualification`, `SignatureVerifier` | Verify exact signed payload through host verifier, then validate scope and structure |
| `trust` | `NativePreflight::from_native_report` | Distinct native preflight; never substitutes for qualification |
| `admission` | `evaluate`, `peak_ram`, `storage_reservation`, `rank_choices` | Same deterministic filter for recommendation, pre-download and startup; one general recommendation and at most one alternative |
| `policy` | `NativePolicyGrant::from_local_store`, `check_download_policy`, `DownloadContext::check` | Current local standing consent, automatic acquisition purpose, network/metering/licence/storage/expiry/model rules |
| `policy` | `recovery_candidate`, `oom_recovery_candidate` | Local qualified alternatives inside the same standing policy; OOM requires changed configuration and lower peak memory |
| `router` | `NativeInstalledModel::from_native_inventory`, `route_local` | Prefer hash-verified installed qualified local models; otherwise propose an in-policy acquisition or pause; one lease at a time |
| `lifecycle` | `Provisioner` | Pure state machine; emits `ExternalAction` IO requests and accepts trusted native observations; active identity changes only after a successful scoped smoke report |

The serializable `Decision`, `ProvisioningSnapshot` and policy DTOs are explanations/data, **not execution permits**. Do not accept them back from the renderer to authorize anything. Recompute at every native boundary. `VerifiedCandidate`, `ValidatedQualification`, `NativePreflight`, `NativePolicyGrant`, `NativeInstalledModel`, `Provisioner` and native IO report types intentionally have no `Deserialize` implementation. Compile-fail tests check five principal trust types.

## Native authority and signed evidence

`SignatureVerifier::verify(domain, exact_payload_bytes, attestation)` is an injected **trusted native adapter**. It must implement actual Ed25519 verification using reviewed existing host facilities, pinned trusted keys, key revocation, domain separation, approved origin, catalog lineage and anti-rollback rules. There is no default/allow-all production verifier, no key generation and no copied cryptography. The structural attestation checks are not cryptographic verification. Never implement this trait using a `workflowVerified`, `isDeviceVerified`, HTTP success, manifest qualification label or bridge-JSON boolean.

`verify_candidate` checks the signature before decoding the candidate. `validate_qualification` separately verifies the qualification payload, then binds its scope to **the entire exact candidate** (all artifact hashes/sizes, runtime/backend/driver/OS/API/ABI, context/KV, projector/speech plan, licence, capabilities, revision and expiry). It requires a declared qualification ID, scoped device class/concurrency, valid time interval and six individually evidenced passing checks: integrity, load, memory peak, tool format, capability tasks and thermal soak. A signed failure/unknown outcome is not accepted as qualification. It still trusts the authorized qualification issuer to have performed those tests; this library cannot independently reproduce physical evidence.

Qualification is either:
- `PHYSICAL_DEVICE`: passing record for the exact local device class and configuration; or
- `VALIDATED_GENERIC_PROFILE`: conservative issuer-validated profile **plus a matching fresh local native preflight**.

Preflight reports must come from actual native operations and bind exact candidate, request, probe ID, capture/expiry and measured responsiveness. No preflight alone turns an unqualified catalog entry into supported. Do not fabricate a generic profile to bootstrap a download: absent a qualifying profile/preflight, the ordinary wizard must say not yet qualified. Building a useful generic pre-download native test (using already-installed/bundled resources rather than the missing multi-GB model) is an adapter/evidence task, not implemented here.

The host must create `DeviceProbe` from native observations, not imported/synced/untrusted renderer data. `TESTED/LOAD_VALIDATED` means the exact runtime/backend/driver was successfully exercised, not DLL presence, nvidia-smi detection, platform name or advertised GPU memory. `NativePolicyGrant` must be re-obtained from the current local consent store on each download boundary, including revocation checks. The trait trusts that store; a cached wrapper cannot detect external revocation by itself. Remote/synced JSON must not mint a local approval.

## Probe and candidate contents

`DeviceProbe` records local class/probe/time, OS/version/API/ABI/instructions, total and available RAM, total/available VRAM, unified-memory flag, remaining native/heap budget, low-memory threshold, storage, disk speed, GPU display name, battery, thermal state and exact backend health. Every observed value carries `DETECTED`, `ESTIMATED`, `TESTED` or `UNKNOWN`. Critical current capacity values require detected/tested evidence; estimates may be shown but are not credited as free capacity. Optional disk speed/battery/GPU-name unknowns are not converted into invented measurements. Probe freshness is 60 seconds; a changed OS/runtime/driver or fresh probe ID invalidates mismatched scoped evidence/preflight.

Each `CatalogCandidate` is **one exact configuration**: role, signed identifiers/revisions, immutable source revisions, relative safe artifact paths, exact lowercase SHA-256, separate download/installed sizes, artifact kind/format, runtime version/backend/driver/OS/API/ABI/features, licence revision/notice hash/distribution approval, verified-capability claims, languages/modalities, fixed context/KV format, explicit peak memory components, concurrency ceiling, qualification IDs and expiry. There is no tier inference from `2B`, `4B`, `12B`, filename, weight size or GPU marketing name. Different context, quantization, projector, driver or runtime means a separately reviewed scoped entry/record; the core never invents a linear KV estimate.

## Deterministic admission and ranking

`evaluate` is used unchanged for pre-download suggestion, acquisition, start-load and final activation checks. It accepts an injected time, not system clock IO.

For the exact configured context:

```
peak RAM = weights RAM + projector RAM + vision RAM + KV RAM
         + concurrent speech RAM + runtime RAM
         + per-agent RAM × parallel agents
         + GPU peak bytes only when unified-memory budgeting requires it
required total RAM = peak RAM + OS reserve
required available RAM = peak RAM + max(available reserve, low-memory threshold)
comfortable available RAM = required available RAM + comfortable headroom
storage reservation = sum(download bytes + installed bytes) + disk headroom
```

All additions/multiplications are checked; overflow denies admission. GPU and CPU allocation components must be disjoint in the profile so unified-memory accounting does not double count them. Separate VRAM is **not added to RAM capacity**. Insufficient total RAM/native/heap/VRAM is a permanent misfit; insufficient currently available memory is cleanup/recheck pressure. Native/heap budgets must represent remaining capacity for this additional load, including current resident workloads. Projector/image memory is always budgeted for a multimodal configuration, even when the immediate request is text. Speech and agent overhead are never omitted because a weights file alone fits.

Status and eligibility are separate:
- `RECOMMENDED`: qualified, admitted now, comfortable headroom, scoped measured responsiveness available.
- `SUPPORTED_WITH_LIMITS`: qualified but limited headroom or unmeasured responsiveness, **or temporarily blocked by memory/storage/thermal pressure**. `eligible_now` distinguishes the latter and must be respected.
- `UNSUPPORTED`: permanent incompatibility, invalid input, memory misfit, licence refusal or arithmetic overflow.
- `NOT_YET_QUALIFIED`: missing/stale/unknown critical evidence, backend not validated, absent/mismatched/expired qualification, or missing required preflight. Pressure without qualification does not imply support.

The chosen context/backend and evidence references accompany the decision. Qualification timings are explicitly scoped to the tested profile, not automatically a benchmark of this device. A fresh matching local preflight may supply this-device measurements. Unknown speed stays unknown; generated-token count and measured duration are retained rather than synthesized TPS. Ranking first filters requested tested capabilities/languages and eligibility, then prefers recommended/comfortable choices and measured first-token/cold-load responsiveness with deterministic ID/version ties. It never favors model parameter count. `rank_choices` is display/planning only; its serialized decision inputs do not grant execution authority.

The router prefers verified installed qualified models and supports later genuinely needed specialists. Conservative v1 serializes all resident model leases; concurrent native orchestration is not claimed. It emits `QueueUntilLeaseReleased`, `LeaseInstalled`, `ProvisionWithinGrant`, `PauseOutsideGrant`, or `NoQualifiedLocalModel`. None is a cloud route or a claim that IO occurred. Vault/non-model functionality must remain accessible if no candidate qualifies.

Storage is deliberately conservative and phase-independent: the same full reservation rule is checked at each boundary, even after acquisition or for an installed route. Adapters must provide available capacity consistently and never count the old working model as reclaimable. Optimizing phase-specific disk reservations is future contract work; do not silently bypass a failure in the UI.

## Standing acquisition policy

A local standing policy controls initial general-model acquisition, a needed specialist, updates and recovery separately. It specifies allowed networks, metering, expiry/not-before, per-transfer and model-store caps, exact licence acceptances (including revision/notice hash), and model/runtime/capability/version rules. Version rules are exact or explicitly user-approved `ANY_SIGNED_COMPATIBLE`; the latter still requires current signed compatible evidence and all other policy checks. Model-store usage includes the previous model and other stored models; the pending reservation must not be counted twice by the host.

A matching existing grant returns `ALLOWED_WITHIN_GRANT` without repeated permission prompts. Outside-grant work returns a specific `PAUSE` reason; the host can offer recheck, free-space guidance or explicit grant expansion. Missing/unknown/metered-disallowed connectivity cannot silently proceed. Acquisition is not inference: a previously acquired/verified model may load offline without a network grant, subject to current local admission/qualification. Native installation receipt and applicable licence/distribution metadata remain mandatory.

OOM recovery uses `oom_recovery_candidate`: different exact configuration and reduced validated RAM/VRAM peak, checked through the same admission and grant. A lower context requires its own signed qualified configuration/request, not an automatic guessed context mutation. Other recovery excludes known failing configurations in the caller. No model download can fall through to cloud inference. Repeated failures should remain local non-sensitive diagnostics, not device fingerprint uploads.

## Provisioning lifecycle / real IO boundary

```
Idle / Failed / Cancelled / Ready
  -> begin + all gates -> PendingExternalIo
  -> real native progress -> Downloading
  -> interrupted / grant change -> Paused -> recheck + resume
  -> exact downloaded byte count -> AwaitingVerification
  -> native exact artifact hash/size observations -> Staged
  -> fresh shared admission -> Loading
  -> actual prompt/tool-format/first-token/thermal smoke succeeds -> Ready
```

`PendingExternalIo` is intentional. The module does not perform HTTP, reserve files, resume Range requests, fsync, rename, call JNI/Tauri, load models or run smoke prompts. `ExternalAction` requests those actions from future native adapters. Download byte completion is not hash verification; hash verification is not model load; model load alone is not a passed smoke test. The adapter must verify a signature through `VerifiedCandidate`, hash **actual staged bytes**, then supply `NativeArtifactMeasurement` values. Never copy expected catalog digests into a fabricated report. Compressed artifacts require the catalog size/hash semantics to correspond to the exact bytes the host verifies; v1 assumes the signed hash identifies the installed artifact.

The working `ActiveModel` is preserved through failed/interrupted/cancelled downloads, integrity failure, rejected admission, OOM, driver/tool-format/thermal failure and late callbacks. Only accepted native success mutates it. A replacement emits `RetirePrevious` with the previous **activation attempt**, not merely model ID, so an in-place update cannot accidentally retire the new instance. No stop-before-load-success action exists in this core. Memory probes during replacement must include the old resident process; the host cannot release its resources first just to manufacture eligibility. Final checks use the same scoped fresh pre-load capacity observation and native outcome; a long load with expired evidence requires an appropriately revalidated observation rather than a fabricated fresh timestamp.

Attempts increase monotonically within a `Provisioner`; cancelled/stale generation callbacks cannot activate. Durable crash recovery, globally unique store transactions across restarts, file locks, bounded partial cleanup, disk reservations, trusted inventory rehydration and lifecycle persistence are **not implemented**. `ProvisioningSnapshot` deserialization never resumes active state. After restart, native adapters must reconstruct/reverify store state and run load admission/smoke, not trust a persisted READY flag. Cancellation retains the working model and requests native cancellation; no partial artifact becomes active or authorizes vault deletion.

## Existing-source integration map (not changed by this crate)

- `apps/desktop/src-tauri/src/desktop_model_policy.rs` currently uses total RAM only, tier-name inference, tier floors and estimates. Its test explicitly ignores free-RAM pressure. The new core does **not** copy that behavior.
- `main.rs::HardwareProfile/get_hardware_profile` currently rounds GiB values, returns zero on failed memory probes, treats Vulkan library presence as detection and macOS as Metal presence. A future adapter must preserve exact byte values and distinguish detection from tested runtime health.
- `llama.rs::admit_model` currently infers tiers, reads artifact sizes, uses a fallback KV estimate and calls the old total-RAM policy. `select_model_for_memory` tries largest first. `start_model_server` currently stops the previous server before replacement succeeds. Native integration must replace these boundaries coherently, not add a contradictory UI-only card.
- `distribution/catalog/model-artifact.schema.json` and `release-catalog.schema.json` use existing Android-oriented formats/stages and Ed25519 envelopes. They do not contain this richer DTO, GGUF/runtime profile support or full qualification scope. A reviewed signed catalog-format extension/native mapping is pending; a mapping must cryptographically bind the exact new payload, not arbitrarily claim old stage strings satisfy it. Existing origin, signature, licence and checksum enforcement must not be weakened.

## Tests actually run vs pending app semantics

Executed locally using the actual new crate and locked existing dependencies, `-j1`, no workspace/native/Kotlin/Gradle build:

```sh
cargo test -p unoone-model-admission --locked --offline -j1
cargo clippy -p unoone-model-admission --all-targets --locked --offline -j1 -- -D warnings
cargo fmt -p unoone-model-admission -- --check
```

The tests exercise the real policy functions/state machine with explicitly synthetic native/signature adapters and JSON fixtures, including the reported **1,879,048,192-byte KV allocation**, context plus projector/image/speech overhead, pressure vs permanent misfit, unknown/estimated probes, total/available VRAM, unified memory, API/runtime mismatch, storage/peak overflow, signed payload/qualification tampering, failed qualification checks, expired/revoked grants, offline load, active preservation, cancellation/retry/stale callbacks, routing/lease serialization and measured responsiveness ranking. The AnythingLLM scenario is a reported allocation regression fixture, **not a reproduced real AnythingLLM/model/device benchmark**.

Pending: Kotlin mirror execution, real native probes/crypto/catalog bindings, production consent-store adapter, real downloader/atomic model store, actual loader overlap/rollback, durable cancel/resume/crash recovery, Device Check/wizard UI and localization/accessibility, application boundary integration, real model tool/quality/latency/thermal/device tests, production catalog rights/signing and physical Windows/macOS/Android qualification. No production wizard, model installation, clean offline installation journey or newly supported device is claimed by these unit/contract results.
