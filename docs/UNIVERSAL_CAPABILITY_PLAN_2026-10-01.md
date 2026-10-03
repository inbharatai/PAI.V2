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
| Universal-agent cycle — browser popup shim (user-reported 2026-10-03: "clicking on sign in does nothing" on Gmail) | CLOSED | `e462d79` | root cause via CDP probe `window.open → {opened:false, openerNull:true}`: wry/WebView2 swallow new-window requests, so sign-in buttons that open popups died silently. Fix = idempotent JS shim (marker guard) that routes `window.open` into `location.assign` of the same window and rewrites `target="_blank"` to `_self` at click-capture; injected on session start, re-injected after every navigation, kept alive by a once-only 1 s thread that re-evals after page loads. Browser suite 47/47 (guards its own install, routes `window.open`, rewrites blank targets) |
| Universal-agent cycle — in-chat folder-grant approval (user: "i give you permission" in chat while the model was fenced out) | CLOSED | `e46d734` | `GrantedFolders` gains a `DeniedPathRequest` hook: an outside-fence path asks the HUMAN mid-run — `unoone:folder-grant-request` card in the chat UI, 60 s bounded deny-by-default wait on the tool thread; Grant runs the SAME validation + vault audit + persistence as the Settings lane; the hook re-checks the persisted store on every denial so mid-run grants (its own card, Settings, a sibling agent) resolve without re-asking; Decline marks the folder declined-for-this-session and returns the honest reason to the model. Tauri commands + frontend banner + truthful system-prefix bullet; granted_fs hook tests + 6 approval-lane tests; 233 desktop tests green |
| Universal-agent cycle — prose-dump corrective retry (user: "fix a 12B model — complex multi-step reasoning … can come back as prose instead of actions") | CLOSED | `47db550` | vendor `RunOptions.corrective_retries` (default 0): the nudge fires only when tools were offered, zero tool calls ALL RUN, no streamed calls, and the final text holds a complete fenced code block; ONE bounded nudge then the model's text stands; already-acted and chat-only runs are never corrected; retry consumes no extra step budget. Full-access + sub-agent runs opt in with 1. 4 CI-visible behavior tests (scripted dump→acts, opt-out stands, already-acted exempt, bounded to one) + vendor fenced-block detector test; full desktop suite 237 green; vendored workspace 81 tests green + first-ever `cargo fmt --all` normalization (formatting only) |
| Universal-agent cycle — account-work briefing + docs honesty (user: "access needs connection via browser and cowork no api is required") | CLOSED | `f2c607d` | the full-access system prefix now teaches the persistent-session lane truthfully: sites the user logged into stay logged in across restarts, account work rides that browser session with NO API keys/OAuth apps, the model never asks for or types the user's credentials — a missing login is routed to the user to complete themselves in the Browser window; the sub-agent briefing carries the same boundary with "report to the PARENT" routing; pinned by `briefing_teaches_the_persistent_browser_session_and_credential_boundary`; README tool-surface/browser-lane bullets extended with the in-chat grant card, the popup shim, the corrective retry, and the account-work boundary |
| Universal-agent cycle 2 — pendrive staging + live verification | CLOSED | staged from `f2c607d` 2026-10-03 | full transactional staging (`UnoOnePower` `3C346A80…`, Dock `EB16F3FF…`, Starter `2F93185E…`; prior exes preserved in `RECOVERY/package-backups/20261003-112354`; frontend-embedding gate VERIFIED_WORKING; `--verify-only` valid=true 0 failures) and LIVE acceptance on the staged drive through the real UI: the Gmail sign-in click that previously did nothing navigated the same window to the real `accounts.google.com` sign-in page (shim probe `shim:true, opened:true`); ONE full-access model turn touching the then-ungranted `Desktop\Stanford` opened the in-chat approval card → Grant clicked → `fs.list` completed with the real file names → grant persisted host-locally → the encrypted vault `AuditRecord` (`action: folder_grant`) verified by decrypt probe. The user's original live defect ("i give you permission" → model still fenced out) is closed end-to-end. Desktop CI + Windows Bundle + Mobile Protection all green at `f2c607d` |
| Universal-agent cycle — fs.copy: binary-safe copy, the design-website lane (user: "make sure … it can build design websites with images etc like chatgpt or glm or claude does") | CLOSED | `16d59ed` | root gap: `fs.read` is UTF-8-only, so an agent "copying" a PNG by read+write silently corrupted every binary image — designed websites with real images were impossible. Vendor: `RootedFs::copy_file` (resolve source through the fence, self-copy no-op, byte-bound check, `fs::copy` into the shared `atomic_replace` temp+rename tail now extracted from both write paths — no third duplication), `ExecutionBroker::copy_file` default refusal + local delegation, `CopyFileTool` (`fs.copy`, FileRead+FileWrite, OnSideEffect) + schema + registration; read-only sources copy WRITABLE (Windows attribute cleared with a justified clippy allow; Unix 0o644). Desktop: `GrantedFolders::copy_file` routes BOTH ends independently (a missing grant on either end asks the human exactly like a read/write), same-root on the vendored fence, cross-root resolve+bound+atomic binary write; broker + `desktop_workspace_tools` registration both lanes; contract lockstep (tools.v1.json entry 63, pin test, sync gate OK); briefing teaches the full design lane (CSS/SVG authoring, `fs.copy` for the user's images, the fs.read+fs.write CORRUPT trap, "never claim images are impossible") in parent + sub-agent prefixes; `progress_detail` "Copying X → Y". Tests: vendor 2 new (binary round-trip incl. high-bit bytes/escape probes/directory refusal/no temp residue; self-copy/overwrite/limits/read-only-source) + 1 tool test; desktop granted_fs 2 new (same-root + cross-root + broker round-trips, outside-on-either-end denial), briefing + progress tests |
| Desktop session-safety + model-selection hardening — PR #61 merged, single-branch policy (user: "we need only one branch and git and readme should be accurate with nothing stale") | CLOSED | `a231a97` (merge of `desktop/session-safety-and-model-selection`; branch deleted — `main` is the only branch) | session-scoped host-command consent (`desktop_process.rs`): `process.run` runs only with Settings-enabled session permission (audited `host_commands_enabled`/`host_commands_revoked` vault records), every spawned process owned and swept on lock/revoke/exit (Windows Job Objects kill whole trees, 32-process cap), generation-invalidated leases so a pre-revoke broker can never spawn after the revoke; locking is a full sweep (inference stop, generation-id'd run registry closes the supersede/finish ABA race, pending in-chat grant cards denied, preview + browser windows closed); grant-card admission serialized with vault locking so no card races past a lock. Model selection: manual Stop/Load supersedes BootGate auto-boot, cached boot hash-verifies the declared vision projector, conservative memory admission (`desktop_model_policy`: fail-closed, never credits unmeasured GPU memory, never silently downloads a tier — originally available-RAM-budgeted, recalibrated to a total-RAM budget the same day, see the next row), cancelled loads never read model bytes, `StartingModelChild`/`ModelManager` Drop kills uncommitted inference children mid-shutdown. All 13 PR checks + all 3 main CI workflows green at the merge; README gained the session-consent and model-selection bullets; drive re-staged from the merge build (`UnoOnePower` `A2EFE8DC…`, Dock `66DB49C1…`, Starter `BC0F7880…`) |
| Model-admission recalibration — total-RAM budget (user: "we should have ram budget of what the system holds and should be maximum and also we have the pendrive to accomodate as it was working accurately before") | CLOSED | `efa3368` | live-caught on the staged a231a97 drive: the PR-#61 available-RAM admission refused the flagship 12B on its own verified target machine — the probe measured the exact budget (granted 16K q8_0 KV = 12.75 GiB; even the 4096-token floor needed 12.20 GiB vs 10.4 GiB free, and the shipped 32K lane needs 21.76 GiB — dead on any realistic host, so the drive's only desktop model could never boot). `desktop_model_policy::fits` now budgets against the system's TOTAL RAM (weights ×1.1 + projector + KV + 1 GiB ≤ total; tier minimums kept; NaN/invalid still fail closed) — free-RAM pressure is not a veto, restoring the lane that ran accurately before admission existed. The post-sweep re-admission in `start_model_server` was removed as dead logic (total RAM cannot change during the sweep); selection reason + refusal strings reworded to the total-budget semantics; policy tests rewritten (`total_ram_is_the_budget_even_under_pressure` pins the real flagship numbers: 7.14+12.75 fits 24 GiB, refused at 16) + the selection test asserts pressure-independence. 13 targeted tests green, full workspace 489 green, clippy -D warnings clean, fmt clean |
| Manifest-digest case mismatch — boot the verified host cache again (live-caught on the staged efa3368 drive while running the held fs.copy live turn) | CLOSED | `d4790f0` | with the RAM gate finally admitting the 12B, the boot chain reached `manager_model_for_config` and landed in limited mode: "Model or cache entry does not match a declared desktop model". Root cause: the PR-#61 hardening compared manifest SHA-256 with an exact `==`, but the staged drive manifest stores UPPERCASE digests while the host-cache branch of `read_manifest_model_hash` lowercases the filename digest — so the digest-verified cache copy (the normal fast boot path) could never match and no model started; masked until now because the available-RAM refusal fired earlier in the chain. Fixed case-insensitively like every other identity check (`verify_projection_hash`, `MODEL_IDENTITY_POLICY`). The same bug in `stage_model_to_host_cache`'s streamed-digest comparison was found by the regression test: on any FRESH host (no cache yet) re-staging against the shipped uppercase manifest would fail and force slow drive-path boots forever — fixed too. Regression test stages against an uppercase manifest digest and resolves the cache path back to the declared model, pinning both. Full workspace green (260 in the desktop bin), clippy -D warnings clean, fmt clean |
| web.preview relative references 403 — inject a `<base href>` into mirrored pages (live-caught finishing the fs.copy design-lane verify) | CLOSED | `f0dcfaf` | with the 12B finally booting, the ONE design-lane model turn ran the full lane: `fs.mkdir` + `fs.write` (3079-byte designed page: inline SVG, gradient hero, cards) + `fs.copy` byte-exact (1928 bytes, sha256 `E32D…949CA` matches the source PNG exactly — the UTF-8-corruption defect this cycle closes is proven absent) + `web.preview` (2 file(s), 5007 bytes mirrored, preview window live on `asset.localhost`, "Preview / Stop preview" affordance in chat). But the page's `<img src="assets/logo.png">` 403'd (`naturalWidth: 0`): the preview page URL carries the whole absolute file path as ONE percent-encoded segment, so every relative reference collapses to `asset.localhost/<relative>` and escapes the mirror — no model-authored site with relative subresources could ever render. `stage_mirror` now patches every mirrored `.html`/`.htm` with a `<base href>` at the page's OWN mirror directory (nested pages each get their own), in the exact URL space `convertFileSrc` serves the entry from (encodeURIComponent replicated on the Rust side); the tag sits directly after the real `<head…>` open tag (boundary-matched so a page's own `<header>` can never swallow it), and an author's own `<base>` wins. Honest boundary documented: references inside mirrored CSS subresources still do not resolve; inline `<style>` is unaffected. 8 new tests + 2 existing mirror assertions updated; full workspace green, clippy -D warnings clean, fmt clean |

Physical gates still **unverified** (hardware): on-phone speech/blind-aid, LiteRT vision
qualification, audiocpp device acceptance, phone↔drive↔Power round-trip (notes, memories,
skills, conversations AND env facts), pendrive auto-launch, Android boot auto-launch.