# UnoOne Android Agent Architecture

> **Updated 2026-10-07:** retitled as the Android agent architecture (this file covers only
> `android-app/UnoOneAgent`; the desktop app is described in the repository README). Corrected:
> two registered brain tiers (E2B Lite / E4B Medium) instead of "only E2B" (the app's load paths
> still load E2B); ReAct step budget comes from `ModelProfile.maxAgentSteps` (not `MAX_STEPS=3`); the deleted manifest-signing classes are no
> longer described as present; 16 Gradle modules (adds `:vault`) and the 8-entity Room schema; both
> Gemma `.litertlm` hashes are pinned. Links to removed legacy plan docs were repointed or dropped.

This is the architecture index for the **Android agent** (`android-app/UnoOneAgent`). The desktop
side (Windows Tauri app, harness bridge, vendored harness/audio planes, Dock/Starter) is described
in the [repository README](../README.md). For the deep, module-by-module Android walkthrough see
[`local-architecture.md`](local-architecture.md).

Planning brains: **two Gemma 4 tiers via LiteRT-LM** are registered — **E2B (Lite, default)** and
**E4B (Medium)** (`core/.../model/BrainModel.kt`, `ModelProfile.kt`; both pinned in
`models_manifest.json`). A tier is chosen before a task starts and never switches mid-task. At HEAD
the app's load paths (`UnoOneApplication`, Model Status, `SecureBrowserModelLease`) load E2B;
`ModelTierSelector` (E4B for compound intents when E4B is loaded and ≥ 8,192 MB RAM is available) is
JVM-tested in `:core` but not yet called from `:app`. E2B was loaded on the primary Xiaomi 14
(Android 15 / API 35) on 2026-07-14 — CPU backend (GPU delegate fails on SM8650, safe CPU fallback),
18/18 canonical tool-match at that date; E4B has not yet been tested on a device, and both specs keep
`isDeviceVerified = false` until the full device matrix is committed. Legacy Gemma 3n and
`gemma-local` paths are purged and kept out by `scripts/ci/check_repo_invariants.py`. Every
model-proposed tool call is checked against a **CanonicalToolRegistry** of 42 tools (unknown tools
rejected, required args validated, byte-synced with `packages/tool-contracts` by
`scripts/check_tool_contract_sync.py` in CI) before safety/execution.

**Agentic loop + judge + eval (implemented; control JVM-tested; inference device-time).** After an
LLM-planned observation-producing call, a bounded **ReAct loop** (every step through the safety
pipeline) feeds the tool result back to the model via `planNext`. The step budget is per tier —
`ModelProfile.maxAgentSteps` = 2 for E2B, 4 for E4B, passed as `ReActLoopController.decide(maxSteps)`;
the former `MAX_STEPS = 3` constant is gone. The orchestrator's current call
(`AgentOrchestrator.kt` → `ReActLoopController.decide(stepsExecuted, lastCall, proposal)`) passes no
limit, so `DEFAULT_MAX_STEPS = 2` (the E2B value) applies. A second on-device **safety judge** runs
on a dedicated judge conversation and may only *escalate* the keyword-classified risk
(`SafetyJudgePolicy`, never de-escalates). A **calibration eval harness** (`EvalPromptSet` +
`EvalScorer` + instrumented `BrainEvalHarnessTest`) turns "is Gemma good enough?" into a printed
accuracy number. The control/scoring logic (`ReActLoopController`, `SafetyJudgePolicy`, `EvalScorer`)
is JVM-tested; the LiteRT-LM inference (`planNext`, `judgeSafety`) and the eval runner are
device-time. **The eval harness HAS been run on the physical Xiaomi 14** (18/18 tool-match, 2026-07-
14); the ReAct loop and a standalone judge-verdict benchmark on a physical device are not yet
recorded — no ReAct outcome or judge-verdict number is claimed or committed beyond the eval result.

**In-app security level (user-selectable, Settings → Security Level).** `AgentOrchestrator`
consults a persisted `SecurityLevel` (STANDARD / RELAXED / OFF) on every validated tool call:
STANDARD runs the judge + enforces the keyword BLOCK tier + requires confirm taps; RELAXED disables
the judge and auto-approves confirmations but keeps the BLOCK tier; OFF (demo) additionally bypasses
the BLOCK tier so every module can be exercised. This is safe because the BLOCK-tier tool names
(`make_payment`, `send_message`, `access_passwords`, `install_app`, `silent_control`) have **no
`ActionExecutor` handlers** — they fall through to the plugin router (a no-op error) — so OFF triggers
no real payment / SMS / credential / install action. STANDARD is the default and the production
posture.

**Innovations #4–#8 (implemented, control JVM-tested, inference device-time-only / partly inactive).**
- **Multimodal vision (#4):** a new `describe_scene` tool (canonical, STRONG_CONFIRM +
  MediaProjection) builds a screen scene from OCR + foreground context via JVM-tested
  `SceneDescriptionBuilder`. The LiteRT-LM `Content.ImageBytes` vision path is wired against the real
  AAR but gated `VISION_MODEL_ENABLED = false` (shipped Gemma models are text-only); it lights up only
  with a vision-capable `.litertlm` artifact. No vision understanding is claimed.
- **Outcome-learned memory (#5):** per-(command-signature, tool) outcomes are stored in Room and
  surfaced to the planner as a hint ("prior avoid: …", "prior worked: …"). `OutcomeMemoryPolicy`
  (signature + token-overlap retrieval + rendering) is JVM-tested; Room I/O in `MemoryModule`; no
  schema migration. The on-device benefit is not yet measured.
- **Streaming (#6):** first-turn LLM planning can stream partial text to the timeline via LiteRT-LM
  `sendMessageAsync` (`Flow<Message>`). `StreamingTextReducer` (robust to cumulative vs delta
  snapshots) is JVM-tested; the `Flow` + UI surfacing are device-time-only, with a fallback to the
  synchronous `plan()` path.
- **Diagnostics self-heal (#7):** a rolling `ToolHealthTracker` flags flaky tools; the brain
  auto-reloads when found down (it self-closes on a 30s timeout). `BrainHealthPolicy` + the tracker are
  JVM-tested; the reload + timeline surfacing are device-time-only.
- **Signed manifest integrity (#8) — removed:** the inactive, blank-keyed bundled-manifest signing
  classes were deleted; `scripts/ci/check_repo_invariants.py` now prohibits `ManifestSigningKey`,
  `ManifestSignatureVerifier`, `ManifestSigner` and a `"manifestSignature"` field from returning.
  Bundled-model integrity rests on the pinned `sha256` + `sizeBytes` in `models_manifest.json`
  (see *Integrity* below); release-catalogue signing is distribution tooling (`scripts/catalog/`).

## 16-module structure

```
:app                Compose UI, ViewModels, FloatingService, permissions, AgentOrchestrator (8-step pipeline), SecurityLevel gate
:core               Result, ToolCall, TimelineStep, RiskLevel, Logger, safety primitives, CanonicalToolRegistry, BrainModel/ModelProfile
:storage            Room DB (8 entities, 8 DAOs, SQLCipher-encrypted cache), migrations
:modelmanager       manifest load, install + integrity (sha256/size), health, detect
:languagepacks      LanguagePackManager, typed catalogue, dependency-aware install/uninstall, pack health
:localbrain         RuleBasedParser, PromptBuilder, GemmaPlanner (LiteRT-LM), UnoOneToolSet, RAGManager, judgeSafety
:voice              SherpaSttEngine, SherpaTtsEngine, KeywordSpotterEngine (wake word), VoiceService, VoiceModule
:agentrouter        tool registry + plugin routing
:safetyguard        SafetyGuard (4-tier risk), input override
:phonecontrol       PhoneControl, CalendarControl, OcrControl, PackageResolver, BlindAidManager
:memory             keyword context, preferences, corrections, patterns
:skills             JSON step storage, trigger matching, CRUD
:observability      Diagnostics (latency/success), crash logs
:accessibilitycontrol  click/type/fill/scroll/swipe/back/home/read_screen/find+click
:securebrowser      BrowserDomainPolicy, SecureWebViewController, PageAgent protocol, BrowserSafetyPolicy, audit
:vault              MobileVaultRepository (encrypted Pocket AI USB vault over SAF: unlock/read/write/tombstone, vault-core-compatible crypto), VaultSyncPlanner
```

## Request lifecycle

`user input (text/voice) → InputSanitizer → Skill trigger check → CommandParser.parseAsync
(RuleBasedParser fast path, GemmaPlanner for complex) → ToolCall → runValidatedToolCall:
system-access check → runtime-permission check → SecurityLevel read → risk classification
(tool + input, max wins) → optional safety judge (STANDARD only; may only escalate) →
block gate (enforced unless OFF) → confirm gate (tap in STANDARD; auto-approved in RELAXED/OFF) →
ActionExecutor.executeTool → Diagnostics + AuditLogger → timeline + spoken response.`

Compound commands and skill steps each run the full pipeline per step — safety is never bypassed.
In OFF/RELAXED the judge and/or the confirm/block gates are relaxed per the user's chosen level, but
the BLOCK-tier tool names still have no executor, so no real sensitive action is possible.

## Dependency-injection note (honest gap)

Hilt is wired at the app level (`@HiltAndroidApp`, `@AndroidEntryPoint` on `MainActivity`) but
`AgentOrchestrator` still constructs its components manually (`MemoryModule`, `OcrControl`,
`AccessibilityControl`, `CommandParser`, `ActionExecutor`, `SafetyPipeline`). This is intentional
deferred work, not a bug: a full Hilt migration is tracked but not yet done, so the codebase is
**not** claiming a clean DI architecture it doesn't have. Single shared instances (`VoiceModule`,
`OcrControl`, and now `AccessibilityControl`) are enforced manually in `AgentOrchestrator`.

## Key cross-cutting decisions

- **Manual tool calling:** `GemmaPlanner` uses `automaticToolCalling = false` — the model proposes,
  the app validates and executes every call.
- **Offline-first:** voice/notes/control/planning are local; the only network path is opt-in
  `web_search` (off by default). See [`SAFETY.md`](SAFETY.md).
- **Integrity:** every file in the bundled `models_manifest.json` carries `sha256` + `sizeBytes`,
  including both Gemma 4 brains (`gemma-4-E2B-it.litertlm`, 2,588,147,712 B; `gemma-4-E4B-it.litertlm`,
  3,659,530,240 B), so `ModelManager.modelHealth` can report **Verified**; an entry without integrity
  metadata reports "Present — integrity metadata incomplete; release blocked". The E2B pin is also
  asserted by `scripts/ci/check_repo_invariants.py`. See [`MODELS.md`](MODELS.md).
- **Play readiness:** permissions, foreground services, and accessibility justification in
  [`play-review/`](play-review/).

## Further reading

- [`local-architecture.md`](local-architecture.md) — detailed module walkthroughs
- [`SPEECH_ARCHITECTURE.md`](SPEECH_ARCHITECTURE.md) and
  [`SPEECH_MODEL_QUALIFICATION.md`](SPEECH_MODEL_QUALIFICATION.md) — speech (Sherpa STT/TTS/KWS on Android)
- [`../android-app/UnoOneAgent/phonecontrol/README.md`](../android-app/UnoOneAgent/phonecontrol/README.md) — phone/OCR/calendar
- [`../packages/tool-contracts/tools.v1.json`](../packages/tool-contracts/tools.v1.json) — canonical tool declarations (CI-synced)