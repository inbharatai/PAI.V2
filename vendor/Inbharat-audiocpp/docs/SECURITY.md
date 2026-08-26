# Security

InBharat Audio is a local-first speech runtime. This document records the security posture and the hardening invariants enforced in the tree. It describes what the code actually does — audited in `docs/audit/02_CURRENT_FEATURE_MATRIX.md` — not aspirations.

## Supply chain

- **Dependency-free core.** The default build links no external crates/libraries, no audio.cpp, no model assets, no Python. New dependencies require explicit approval.
- **No runtime dynamic loading.** Providers are compile-time built-ins or explicitly-enabled adapters; there is no `dlopen` of arbitrary modules (the optional Vulkan probe only `dlopen`s the system Vulkan loader and never enumerates devices).
- **Pinned upstream.** audio.cpp is pinned to `bb15edd7` (release-0.6) and the adapter refuses a modified or wrong-pin checkout at configure time. The pristine upstream is never modified.

## Path and artifact policy (fail-closed)

- Strict path policy is **on by default** and fails closed when no `allowed_model_root` is configured.
- External model artifacts require a regular-file path inside the allowed root, an optional 64-hex expected SHA-256, and a successful integrity comparison before load.
- Artifact size is capped (4 GiB local RC policy). A documented residual TOCTOU on canonicalized paths is recorded in `docs/SECURITY_PATH_POLICY.md`.

## Memory and thread safety

- Every C entry point runs through the `guarded()` exception firewall; C++ exceptions never cross the ABI.
- Opaque handles, explicit ownership, single-flight session discipline, cooperative cancellation, and bounded stream queues (4096-event absolute ceiling) are enforced and covered by the lifecycle/concurrency/cancellation/stress test lanes.
- Release, ASan+UBSan, and static TSan gates run clean (**15/15** in the current tree, recorded in `reports/`).

## Privacy model

The runtime understands privacy classes (`ephemeral`, `transcript-only`, `audio-and-transcript`, `no-persistence`) but does **not** own a host's vault or retention policy. It emits lifecycle events; the host decides persistence. Remote providers (e.g. Sarvam) are `audio-and-transcript` by definition (audio leaves the device), are compile-time gated behind `IBAUDIO_REMOTE_PROVIDERS=OFF`, and are never selected when a deployment disallows remote — there is no silent cloud fallback.

## What is explicitly NOT done

- No telemetry, no network beacons, no environment-variable credential scraping.
- No fake or stub inference presented as real. Placeholder modules are compile-time gated and labeled.
- No unbounded subprocesses, no `system()`/`popen`/raw socket calls in the core or tools.

## Reporting

Security review is local and adversarial per the project's independent-verification rule. External distribution requires the publication plan (license scan, secret scan, history cleanup) and the user's explicit approval — no remote, no push by default.
