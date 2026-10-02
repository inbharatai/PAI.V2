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
- **P1-D — Universal transcript lane (asked 2026-10-01: "the memory which is universal should
  store every usage or conversation, be it phone or laptop/desktop, as one source").** The vault
  already owns notes/memories/skills; conversations are the missing class. Android gains a
  persisted conversation store (Room, SQLCipher-encrypted at rest like the other caches) and
  mirrors every completed agent turn (user command + agent response) to the vault as
  `TRANSCRIPT {kind:"transcript"}` records — write-through when unlocked, drain backlog when not,
  same as notes. The hydrator reads them back so any host sees the whole history from the one
  source. Desktop already writes `Transcript` records for voice recordings; conversation-history
  transcripts from both hosts land in the same record space. Offline-first unchanged: nothing
  syncs without the vault attached and unlocked, and unsynced turns are never evicted.
- **P2-A — Android auto-launch (asked 2026-10-01: "auto launch in androids just like it does in
  my laptop").** The laptop auto-launches through the dock/tray USB-insert watcher; the honest
  Android equivalent is a `BOOT_COMPLETED`/`LOCKED_BOOT_COMPLETED` receiver that starts the
  voice service, gated on an explicit user-visible toggle (persisted, default off until first
  enable — never hidden autostart). USB attach cannot auto-start an Android app (no autorun
  support in the platform — the same reason the dock mechanism exists on Windows), so boot is
  the reliable plane. On-device behaviour (FGS type + OEM battery whitelisting) is a device
  gate to verify honestly, not a flag to flip from a unit test.
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

## Registered mission directives (2026-10-01, user)

Two standing product directives from the founder, recorded verbatim-intent; every phase below
must be checked against them:

1. **"All AI models should universally use the harness, audiocpp, and the memory as source of
   truth."** Every model, on every host, executes through the one harness (P2 ABI v2 on Android,
   harness bridge on desktop), speaks/listens through the one audiocpp speech plane, and treats
   the shared vault memory as its single source of truth: previous context is READ from it, and
   every current conversation turn is WRITTEN into it (P1-D transcript lane + harness memory
   envelope from P1-E). No host keeps a private memory plane outside the vault.
2. **"UnoOne should work beyond the drive: access the desktop or mobile apps with the
   permission of the user."** The agent must be able to act on the host itself — desktop files,
   desktop apps, and phone apps — never silently: every scope is an explicit, user-visible,
   revocable grant, recorded and audited like every other tool call, through the same
   SafetyGuard/permission/verification pipeline. Android already has app control via the
   user-granted AccessibilityService; the open work is the desktop host plane (P7).

- **P7 — Beyond-the-drive host access (user-granted).** Desktop agent lane grows from the fixed
  workspace root to user-granted host scopes: an explicit in-UI grant flow (pick folders /
  whole-desktop toggle), persisted allowlist, revocation, and audit-ledger entries on every
  access; then a desktop automation plane (window enumeration + input) for driving host apps,
  mirroring the Android accessibility tools under the same permission model. Blocked classes
  (`shell_execute`, `file_delete_system`, registry, raw sockets) stay blocked. Each increment
  ships with focused tests; host-UI acceptance on real desktop apps is a human gate.

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

## Phase status log (implementation log — updated every phase commit)

| Phase | Status | Commit | Gates |
|---|---|---|---|
| P0-A tool contract | CLOSED | `917592d` | contract + both-side drift gates in CI |
| P0-B capability contracts | CLOSED | `e367626` | crate tests + fixtures |
| P1-A Android vault durability | CLOSED | `e3e4d41` | full android gate; golden re-baseline `beceb33` |
| P1-B desktop↔phone vault interop | CLOSED | `058329c` + `49afc32` | desktop cargo tests; android full gate; re-baseline `76e19a2` |
| P1-C MK-style env learning | CLOSED | Android `7138c2d`, desktop (this commit) | contracts 8+10, recorder 10, mirror 20, hydrator 13, factory 7; cargo + full android gate; re-baseline `3f8d530` |
| P1-D universal transcript lane | CLOSED | `b2128ed` | desktop 148 tests + clippy; android full gate; re-baseline `a857481` |
| P2-A Android auto-launch | CLOSED | `4940f99` | policy tests 3/3 + full android gate; re-baseline `76f6dd1` |
| P2 one harness: ABI v2 registration surface | CLOSED | (this commit) | ffi 15 tests (7 v2 registration incl. C-vtable L1 tool loop + hash-chained ledger, 5 ABI-manifest drift gates) + workspace 79 green + clippy clean; on-device Kotlin/JNI loop = device gate |
| P1-E vault honesty hardening (source-review findings) | CLOSED | (this commit) | all four findings fixed: tombstoned-Skill/memory/turn deletion on hydrate (propagateTombstone via getByVaultRecordId), stable vault record ids (PendingWriteDao mint-persist-reuse) + queued tombstone retry, Power harness-memory envelope → Android (MESSAGE/PREFERENCE/CONTEXT_SNAPSHOT → harness_memory, internal index skipped), honest `boundedArguments` (actual args captured within 400-char bound, else false), keep-local hydration (failed push never clobbered) + drain per-op isolation; mirror 23 / hydrator 18 / recorder 13 tests green; CI e2e timeout drift fix (45s→150s, runner perf measured 1.1m→2.9m) |
| P6 docs accuracy pass 1 (stale-docs sweep, finding #8) + APK manifest tracking | CLOSED | (this commit) | drive README's APK hash-compare instruction made real (New-UnoOneManifestV2.ps1 emits `mobile.apk` kind `MOBILE_APP`; usb-manifest gains the kind + kind-check, 16/16 tests, unknown-field tolerant for staged binaries — live re-verified on the drive, `--verify-only` 0 failures); dead `verify-mobile-untouched.{sh,py,ps1}` deleted (pinned a nonexistent tag, always exit 2) and `docs/MOBILE_GOLDEN_BASELINE.md` rewritten to the pointer mechanism; README: Physical-Pocket-AI row re-stated for the 2026-10-01 drive, CI gate state rewritten to the standing job set, Mobile Protection section points at `MOBILE_PROTECTED_TREE` = `bd97bee7…`, usb-manifest count 6→16, prohibition line un-tagged; ARCHITECTURE.md 26→42 tools (sync-checked by CI); STATUS.md demoted to a marked historical snapshot with the correct repo URL; both 2026-10-01 mission directives registered below (harness/audiocpp/memory universality; beyond-the-drive user-granted host access) |
| Stop control (asked 2026-10-01: "we should have an option to stop a ai processing a request") | CLOSED | `5a4f31e` | HarnessRunRegistry (one live run per conversation, register supersedes via `CancelCause::Parent`, finish never cancels) + `harness_stop_run` command + UI Stop buttons + honest "⏹ Stopped" bubble; registry 5 tests, desktop suite green |
| P7 increment 1 — clarify-first + user-granted workspace root + run trail in vault memory (asked 2026-10-01: agent built a website without asking the style; "should have the option to build and use the desktop to store the folder"; "the memory of the drive should have the context and steps and what was done") | CLOSED | (this commit) | L3 prompt gains a clarify-first rule (ONE short message, 2-4 material questions, then wait — small/clear tasks build immediately); workspace root becomes a user grant in Settings (`set_agent_workspace_root`: existing absolute host folder only, never a drive root, never inside the encrypted package; persisted HOST-LOCAL in `%LOCALAPPDATA%\UnoOne\agent-workspace.json` so it cannot follow the drive to another machine; stale grant ignored; every grant/revocation audited as a vault `AuditRecord`); every meaningful full-access run (real tool work, or stopped mid-flight doing it) writes one bounded Project-scope memory record through the SAME vault memory provider the harness used — request, per-step trail (250-step cap + overflow marker, sub-agent steps included via the parent collector), status completed/stopped/failed, output — in the envelope the phone hydrates (P1-E plane). Trivial L0/L1 chat writes no trail. Trail bounds + envelope round-trip tests |
| Universal-agent cycle — P7 increment 2: permissioned multi-folder grants (asked 2026-10-02: agent should reach Desktop/drive folders with approval) | CLOSED | `270db15` | `granted_fs` 8/8 (multi-root longest-prefix routing, `C:\root` vs `C:\rootx` boundary, grant validation refusals); every fs tool routes through the per-grant `RootedFs` fence; Settings grant/revoke UI reuses the P7-increment-1 validations + vault audit per folder; full workspace suite + clippy `-D warnings` + frontend build green |
| Universal-agent cycle — doc.create: real documents through the fence | CLOSED | `7c385a0` | pure-Rust PDF (lopdf, `Tj`-per-line), DOCX (zip OOXML), MD/TXT writers writing through `GrantedFolders`; 5/5 round-trip tests — every writer's output is re-parsed by the EXISTING readers, so a writer bug cannot pass its own reader; contract lockstep (tools.v1.json entry, pin test, truthful briefings); zero new dependencies |
| Universal-agent cycle — web.preview: live website preview, no web server | CLOSED | `460cbc7` | bounded mirror (128 files / 16 MiB / 8 deep, build dirs skipped, symlinks never mirror) into the only asset-protocol-scoped tree; frontend-created capability-less window (defects #40/#41 pattern) + ~1.5 s heartbeat re-stage-and-reload on file-signature change; honest session end when the site folder vanishes; preview 5/5 (a parallel-mirror staging race was live-caught by the suite run and fixed with a per-state mirror root); contract entry 62; chat ◱ Preview affordance; Tauri 2.11.5 asset protocol verified source-level to carry no CSP (the main window's CSP stays intact) |
| Universal-agent cycle — web session honesty (persistence surfaced + vault note + dead-field sweep) | CLOSED | (this commit) | WebView2 cookie persistence across restarts is surfaced, not faked: `browser_session_status` carries honest persistence facts, the Browser Workspace states them in the UI, and ONE deduped encrypted `BROWSER_RESEARCH {kind:"browser_session_note"}` record stores the status + its honest boundary (host-local profile, NOT vault-encrypted, ClearSession clears page storage only — no cookies or credentials stored); browser 44/44 (note round-trip, dedupe, tombstones stick, foreign research records never mistaken); dead fields removed: `BrowserConfig` (discarded at `let _config` — never wired) and `BrowserStateHolder.confirmation_tokens` (never granted, only cleared) + the `HashSet` import; live cookie-persistence verification on the staged drive is part of the staging gate |
| Universal-agent cycle — P6 docs accuracy (README + this log) | CLOSED | (this commit) | README: tool-surface bullet rewritten for the granted-folder fence + doc.create + web.preview; browser-lane bullet gains the honest web-session persistence statement; live-preview bullet added; backend + browser workspace test counts corrected to the real suite (granted_fs 8, doc-writer 5, preview 5, browser 44); this phase log gains the five cycle rows above |
| Universal-agent cycle — pendrive staging + live verification | CLOSED | staged from `abdd510` 2026-10-03 | full transactional staging (`UnoOnePower` `025AA898…`, Dock `A4C0A391…`, Starter `7A6FEB91…`; strict asset verify `545/545`; `--verify-only` valid=true 0 failures) and LIVE acceptance on the drive, all through the real UI: vault unlock + BootGate + chat-memory recall (3 turns); folder grant added in Settings (audited, revocable); ONE full-access model turn ran `doc.create` (real `%PDF-1.4` in the granted folder) + `fs.write` + `web.preview` end-to-end; the preview window served the mirror over `asset://` and its inline script executed; a WebView2 cookie survived a full app restart; the encrypted vault note decrypts to `kind: "browser_session_note"` (deduped — no cookies or credentials stored) |

Physical gates still **unverified** (hardware): on-phone speech/blind-aid, LiteRT vision
qualification, audiocpp device acceptance, phone↔drive↔Power round-trip (notes, memories,
skills, conversations AND env facts), pendrive auto-launch, Android boot auto-launch.