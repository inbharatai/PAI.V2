# InBharat Harness 0.1.0-rc.2 hardening validation

Validated: 2026-08-17T18:55:24Z  
Baseline head: `fb86e6500e7d0f977a6708e75c9d183e2e2f8d8f`  
Source-fix commit: `41d710a`  
Result: PASS within the documented local boundary.

## Clean gates

- Rust formatting: pass.
- Workspace check, all targets: pass.
- Debug workspace tests: 60 passed, 0 failed, 0 ignored.
- Clippy, all targets, warnings denied: pass.
- Release workspace tests: 60 passed, 0 failed, including C ABI panic containment under unwind semantics.
- Release workspace build: pass.
- CLI smoke for info, route, L0/L1/L2/L3, permissions, session resume, process, and website demo: pass.
- External C11 consumer linked against the static library and ran: pass.
- Windows GNU and Android ARM64 Rust cross-checks: pass as compile-only evidence.

## Adversarial coverage added

- 50,000 deterministic arbitrary-byte JSON cases with canonical round-trip for accepted inputs.
- 20,000 zero-byte model-chunk flood rejected at the 16,384-chunk ceiling.
- Unbounded recovery/budget requests rejected before execution.
- 10,000-level cancellation ancestry and `Duration::MAX` wait without stack/instant overflow.
- Prior-turn user/assistant history reconstructed exactly and bounded.
- Unsupported provider models and oversized requests rejected.
- Redundant escalation, missing L0 model capability, false L3 explanatory phrases, invalid tool schemas, unresolved programs, and invalid environment entries rejected.
- Existing traversal, symlink escape, subprocess flood/deadline, cancellation, session corruption, torn tail, recovery, approval, capability, job/subagent, and C ABI tests remain passing.

## Routing benchmark

- Iterations: 100,000.
- Ordinary benchmark prompts: 600.
- False L2/L3 activations: 0.
- p50: 1,474 ns; p95: 1,844 ns; max: 54,086 ns.
- Session churn: 10,000 sessions in 45,550,783 ns.

Timing is container-specific. Correctness and authority gates, not nanoseconds, are the release criteria.

## Artifacts

Hashes and sizes are in `HARDENING_ARTIFACTS.sha256` and `HARDENING_ARTIFACT_SIZES.txt`. Release binary growth relative to RC1 is expected because the C ABI now retains unwind support instead of aborting on panic.

## Residual boundary

No claim is made for kernel-grade hostile-code isolation, descendant process groups, remote authentication, encrypted or cryptographically authenticated sessions, uncooperative provider preemption, or safe use of invalid/stale C pointers. See `HARDENING_AUDIT.md` and `SECURITY.md`.
