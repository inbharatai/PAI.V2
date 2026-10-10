# Power model provisioning: bounded adapter phase

## Shipping behavior

`ModelSetupWizard` (Model Manager for local installs, and Hardware Profile) calls native `get_model_setup_assessment`. This works before model acquisition. It reports detected total **and currently available** physical RAM, OS/ABI/selected-volume free space, and optional NVIDIA name/total/free VRAM (MiB converted to bytes). Failed probes are UNKNOWN. NVIDIA output is DETECTED_ONLY; CPU/Metal/Vulkan load health is UNKNOWN. No GPU utility/library/OS flag supplies runtime qualification. No speed or 12B recommendation is guessed.

The catalog is **UNCONFIGURED**. No production trust key, approved exact signed candidate payload or matching physical/generic qualification record was supplied. Existing distribution manifests/pinned asset hashes are not promoted to qualification. Download, local selection and local server startup fail closed with the same native reason before changing an active process. The UI offers recheck, policy requirements, back, pause and resume; it cannot authorize or download. A local vault remains usable. `assets_ready` is never changed by this adapter.

This is not a completed installer or inference qualification. Explicit legacy drive loading retains its existing policy/behavior; its old total-RAM/stop-before-replacement path was not silently changed into a new qualification claim.

## Actual adapters

- `provisioning.rs`: real `ring` Ed25519 verification of exact payload bytes. App-owned key/domain/exact-payload digest pins constrain approved lineage and rollback; removing a pin/key revokes it. No renderer/env/web key configuration. Shipping pin set is empty. Actual private local policy persistence uses revoke-first, fsync, same-directory atomic publication, bounded strict JSON, fresh approval equality and expiry checks. A policy DTO alone is not authority. Approval writes are not exposed through IPC; the current build cannot draft a valid policy from a missing catalog. Main-window/unlocked-local-vault IPC exposes explicit revocation and a fail-closed start entry, not an `approved` bool.
- `provisioning_probe.rs`: native bounded/timed hardware commands; no device fingerprint upload. Selected root comes from the existing local installer helper. Selected-volume free space is detected on Unix using bounded `df`; Windows volume adapter, process budgets, thermal/battery/disk-speed and runtime load validation remain UNKNOWN.
- `provisioning_download.rs`: actual async reqwest transfer. HTTPS/443 and exact app-owned host allowlist; checked/pinned DNS answers; private/special IP rejection; no ambient proxy; zero redirects; identity encoding; bounded expected bytes and SHA-256. Strong ETag + exact Content-Range/If-Range validates resume. A changed/ignored range is rejected, never appended. Transfers without strong ETags can complete but interrupted partials cannot be automatically resumed. Per-chunk cancellation and fsync; fresh guard at request/chunk/publication. Real selected-volume reservation check plus native core policy gate. In-process mutex + crash-released OS file lock serialize writers. Hash-named immutable bundle directories stage **all** signed artifacts, requiring weights and an explicit runtime executable. No extraction is supported (installed/download byte sizes must match). One same-volume rename publishes staging; old models/manifests/activation state are never touched. A staging receipt is NOT Ready or load authority.
- The reusable native guard calls the shared core evaluator and rereads local consent at every boundary. It deliberately reports network/metering UNKNOWN until a reviewed native adapter exists; it cannot silently accept renderer network assertions. Thus even a caller possessing fixture-like catalog data cannot use this guard to authorize a real production transfer today.

Native probe/network/qualification gaps fail closed. No fallback to cloud inference.

## Remaining integration gates

1. Publisher supplies reviewed key/payload-domain protocol, approved release lineage/pins, signed runtime+model+projector+speech bundle records, actual qualification records, approved HTTPS origins and exact licence notices. Test keys/bytes MUST NOT be shipped as catalog entries.
2. Implement catalog loading against those approved inputs, device-class/process-budget/backend native validation, platform network/metering and Windows selected-volume probes; expose a native-generated bounded policy draft with authenticated main-window user confirmation and session/lock cancellation. No imported/synced policy becomes authority.
3. Connect downloader progress/cancel/resume to the wizard only after these gates. Add narrowly scoped cleanup/retry for invalid partials; do not silently delete working files. Current HTTP range mismatch/hash failures preserve partials for explicit recovery.
4. Implement a supported existing llama loader transaction keeping the prior process/activation until replacement passes actual prompt/tool-format/first-token/thermal smoke, with fresh admission. The current adapter does not start the loader. No receipt may be marked ready from file presence or successful transfer.
5. Full locked Tauri native compile/IPC/WebView tests and Linux/macOS/Windows physical qualification remain required. OS advisory file locking uses Rust >=1.89 (`File::try_lock`); current lightweight tests use rustc 1.99. Windows directory flush/sudden-power-loss durability is not claimed.

## Validation and limits

Tests compile the actual production modules in a small external Cargo harness, single job; no Tauri/full workspace/Android build. It uses the repository-locked package versions. Real socket tests use a loopback HTTP **compile-time test-only** transport exception; production rejects HTTP and private addresses. Tests cover signatures, domain/payload mismatch, local consent expiry/revocation, bytes/hash/range/redirect/encoding, disk reservation, file locks, cancellation/restart, symlink/hardlink refusal and atomic bundle preservation. Existing core vectors cover OOM labels and policy/qualification arithmetic; this is not physical OOM or model smoke evidence. Mounted React tests use official mocked IPC; they are not native end-to-end proof.

No model downloads, public-network live smoke, production keys/records, dependency version bumps, licence changes or commits were made in this phase.
