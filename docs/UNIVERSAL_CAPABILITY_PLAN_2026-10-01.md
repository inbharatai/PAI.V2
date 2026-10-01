# Universal Capability Layer — Execution Plan (2026-10-01)

Baseline: `f66009b` (verified identical to remote main). All verdicts below were verified
against production source by six parallel code-audit passes on 2026-10-01; every claim cites
the file that proves it. This plan extends `docs/UNIVERSALITY_ROADMAP_2026-09-17.md` (U1–U4)
with the mission's capability/memory/vision/audio/environment-learning scope.

## Verified baseline verdicts (gap check vs. the mission brief)

| # | Suspected gap | Verdict | Evidence |
|---|---|---|---|
| 1 | Android Blind Aid independent of Gemma | **TRUE — keep as-is** | MediaPipe EfficientDet-Lite2 asset, every-3rd-frame sampling, two-pass center crop, 3-frame confirmation, tones/haptics; orchestrator *unloads* Gemma on activation (`BlindAidManager.kt`, `AgentOrchestrator.kt:540-553`) |
| 2 | Power `objects: Vec::new()` | **TRUE** | `accessibility.rs:357-361`; no desktop detector exists; UI copy at `AccessibilityView.tsx:663` overstates |
| 3 | Android vision gate false; engine config text-only | **TRUE** | `VISION_MODEL_ENABLED=false` (`AgentOrchestrator.kt:118`); `EngineConfig(modelPath, backend)` lacks vision backend/`maxNumImages` (`GemmaPlanner.kt:332`); `Content.ImageBytes` path wired but dead |
| 4 | Detach/TTL can destroy pending writes + Skills | **TRUE — 10 provable data-loss bugs** | `clearOnVaultDisconnect` deletes unsynced rows (`VaultCacheLifecycle.kt:49-54`); TTL ignores sync state; Skills Room-only, wiped; memory deletions never tombstone |
| 5 | Memory payloads/indices divergent; both directions | **TRUE** | Android `{kind,...}` vs Power `VaultMemoryEnvelope{schema,harness_id,...}` mutually unparseable; Android has **no vault read path in production** |
| 6 | Harness FFI can't register Android providers | **TRUE** | ABI v1 registers nothing; `HarnessBuilder::local` in FFI contradicts the documented `local_embedded` embedding constructor; FFI is production-dead; desktop proves the Rust-trait embedding works (`harness_bridge.rs`) |
| 7 | audiocpp Android = scaffold, Sherpa is production | **TRUE** | `ANDROID.md` self-declares scaffold/build-only; JNI links reference provider + fail-closed sherpa seam; production app uses k2-fsa Sherpa AAR |
| 8 | Stale docs | **TRUE** | `STATUS.md` globally stale (2026-07-23); `ARCHITECTURE.md` says 26 tools vs 42; `verify-mobile-untouched.sh` pins a nonexistent tag |
| 9 | Env learning bounded? | **PARTIAL — bounded on Android, absent on desktop, no epistemic split anywhere** | `OutcomeMemoryPolicy`, `SkillLearningPolicy` (3-use threshold, disabled suggestions, never auto-enable); desktop has zero learning surface; no hypothesis/observation distinction exists |

## Registered model profiles at HEAD (no invention)

- **Android (LiteRT-LM 0.13.1)**: `gemma-4-e2b` (2.59 GB `.litertlm`, sha256-pinned), `gemma-4-e4b` (3.66 GB). Text-only engines as shipped.
- **Desktop Power (llama.cpp)**: `gemma-4-12b` Q4_K_M (~7.14 GB GGUF + mmproj GGUF, manifest-hash-verified). Policy tiers E2B/E4B/TwelveB by host class (`model_policy.rs`).
- **No other LLM is registered.** Speech models (Sherpa ASR/TTS/VAD ×9, Qwen3-ASR/OmniVoice via audiocpp CLI) and the blind-aid EfficientDet-Lite2 asset (not manifest-registered) are non-LLM.

## Phase plan (reviewable commits on `feat/universal-capability-layer`)

- **P0-A — Tool contract (U1 delivered).** `packages/tool-contracts/tools.v1.json` (canonical id, JSON
  arg/result schema, platform availability, permission, risk class, timeout, verification, audit) +
  Rust crate (`include_str!`, invariants) + `scripts/check_tool_contract_sync.py` that parses the REAL
  Kotlin production tables (`CanonicalToolRegistry`/`SafetyGuard`/`ToolPermissionRegistry`) and fails
  on drift + CI wiring in both workflows + a Rust test pinning the desktop harness-bridge registry to
  the contract. Android tool names stay pinned (trained datasets); cross-platform concepts get aliases.
- **P0-B — Capability contracts.** `packages/capability-contracts`: versioned JSON schemas + Rust types +
  tests for `PerceptionObservation`, `EnvironmentObservation` (epistemic status:
  observation/hypothesis/correction/verified_fact + provenance), `ProcedureOutcome` (preconditions,
  postconditions, verification evidence, risk class, promotion status), audio backend status, and
  device capability manifest. Kotlin mirrors land with their first Android consumer (P1/P5).
- **P1-A — Android durability fixes (data-loss bugs 1–4, 9).** Detach/TTL never delete unsynced rows;
  skills + `skill_usage` exempt from cache-clear/eviction; memory deletions tombstone through
  `VaultMirror`; truthful "encrypted pending on this host" state. Focused tests first (reproduce, then fix).
  Golden-hash re-baseline committed together.
- **P1-B — Cross-platform memory/skill interop.** Shared envelope (read-compat): desktop
  `PaiVaultMemoryProvider`/`documents.rs` parse Android-authored `{kind:...}` envelopes (fix
  `title = record_id`, tombstone-blind listing); Android gains a production vault read path for
  Power-authored memories/skills; skills mirrored to the vault as `DOCUMENT {kind:"skill"}` records in
  the drain backlog; revision/tombstone/dupe semantics preserved.
- **P1-C — MK-style environment learning, bounded.** `EnvironmentObservation` + `ProcedureOutcome`
  records with provenance, produced by Android's existing outcome pipeline (extended, still
  device-local telemetry + vault-mirrored user-facing facts) and by desktop harness runs; explicit
  promotion gate: bounded args + repeatable success + verified postconditions + low-risk class +
  no contradictory evidence + explicit user/policy approval. Hypotheses can never execute.
- **P2 — One harness.** Desktop already proves Rust-trait embedding. Android adoption = U1 contract +
  a narrow Kotlin boundary (JNI to the harness core over an extended size-tagged ABI v2 that can
  register model providers/tools/memory/permission/audit — the current ABI v1 registers nothing).
  Scope honestly: contract + ABI-v2 surface + registration tests this cycle; on-device loop = device gate.
- **P3 — Desktop detector parity + vision honesty.** Desktop path executing the SAME
  `efficientdet_lite2_int8.tflite` asset via a pinned local runtime; feed `PerceptionObservation`;
  replace `objects: Vec::new()` with real boxes/labels; fix overstating UI copy; per-artifact
  Android vision qualification (replace the global flag) only after device verification.
- **P4 — Audio status contract.** Shared audio-capability/status contract mirroring the speech-language
  pattern (backend identity, readiness, streaming class, offline flag); Sherpa stays the honest
  production Android backend until audiocpp passes its own device gates.
- **P5 — Device adapter registry.** Host adapters only for what PAI can actually discover/control
  (vault drive, camera, mic/audio-out, host compute); unknown hardware = truthful read-only identity,
  no guessed control; environment memory records device facts with verification state.
- **P6 — Evidence + docs.** Matrix runs, acceptance doc 119+, README/ARCHITECTURE/STATUS corrections,
  upgrade/rollback steps preserving vault data.

## Host coverage (asked 2026-10-01: "also for google phones")

- **Google Pixel (and every Android handset): already covered.** UnoOneAgent
  is a generic Android app — the tool contract, vault mirror, durability
  fixes and all lanes apply to any modern Android device (Xiaomi 14 was the
  validation handset). No separate platform exists to add; Pixel needs only
  the same on-phone physical gate as any other phone.

## Physical-device gates (cannot be honest without hardware)

- On-phone speech matrix + blind-aid lanes (no device attached 2026-10-01; adb present).
- LiteRT vision qualification on E2B/E4B artifacts (needs a real handset + image-capable artifact).
- audiocpp Android device acceptance (its own eight gates, unchanged).
- Phone ↔ drive ↔ Power round-trip (needs the physical drive + phone).

Everything else is built and tested in-repo with the gates above marked **unverified**.