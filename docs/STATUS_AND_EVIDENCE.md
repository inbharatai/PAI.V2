# UnoOne Pocket AI — status and verification evidence

The detailed status table, CI gates and dated test evidence, moved out of the [README](../README.md) on 2026-10-07. Every claim carries its date; older evidence must be read with its date.

## Current Status

| Component | Status |
|-----------|--------|
| Physical Pocket AI | **INTEGRITY-VERIFIED PROTOTYPE, RELOADED 2026-10-04** from `main` `e3b64f4` after the original stick reported Windows media errors (Event 51/153) that formatting did not cure. Reloaded complete from git and hash-verified keepers: `SOURCE` (byte-exact `git archive 52a2ec0`), three desktop GGUF tiers with per-tier mmproj, both mobile `.litertlm` models, the Android APK, runtimes, the speech plane, and a **fresh vault** (past chat/memory deliberately not carried — owner decision). The manifest was regenerated off-drive against byte-identical keepers; `Start UnoOne.exe --verify-only` on the reloaded drive: `valid=true`, 0 failures; owner-verified unlock and a working session. Known defect: the app can crash (stack overflow on a spawned thread) when a mid-sweep device-removal IO error fires — hardware-conditional, not reproducible on a healthy filesystem. **The drive does not yet carry the 2026-10-07 build** — restage with [`scripts/Stage-PocketAiDrive.ps1`](../README.md#update-a-pocket-ai-drive). Earlier live acceptance on the pre-reload drive (three cycles, 2026-09/10) covered unlock, BootGate model load, chat-memory recall, audited folder grants and the in-chat grant card, `doc.create`/`fs.write`/`fs.copy`/`web.preview`, browser-session persistence across restarts, and the Gmail popup shim; dated evidence: `docs/verification/2026-09-14/` + `2026-09-15/` |
| Desktop frontend embedding | **VERIFIED** — the historic "localhost refused to connect" drive is fixed (`tauri/custom-protocol` default feature; without it `generate_context!` embeds zero assets). A byte-level gate runs in the Windows bundle CI and passed on the staged drive binary |
| Mobile app (Android) | V2 agent pipeline + Pocket AI USB auto-open. Compiles, lints, tests, and `assembleDebug` passes in Android CI; **cross-platform vault contract proven bidirectionally in CI** (Kotlin↔Rust Argon2id + AES-GCM record layer and XChaCha20 master-key wrap — `packages/vault-core/test-vectors/`, `VaultCryptoCrossPlatformTest`). Physical phone round trip with the drive pending |
| Desktop frontend (React) | BUILDS — Vite build and oxlint in CI; real Tauri API calls, no mock data. Six local Node test suites (context selector, TS/Rust greeting parity, real-core boundary, mounted Chat, Coding Task and Knowledge views) pass locally but are **not yet wired into CI** |
| Desktop backend (Rust) | BUILDS AND TESTS — `cargo fmt --check`, `cargo check`, `cargo test --workspace` and `cargo clippy --workspace -D warnings` green on **windows-latest, ubuntu-latest and macos-latest** (Desktop CI on `05baa79`, 2026-10-07). Suites cover vault-core (Wave-1 regressions + cross-platform vectors), recording policy, text-util, usb-manifest, document migration, browser policy and workspace, chat memory, the granted-folder fence and in-chat grant approval, the corrective retry, document writers, live preview, the desktop glue, and the product adapter (180 library tests including the 2026-10-07 work). The adapter's 50 sandbox tests need Linux isolation; CI runners without bubblewrap skip them with a visible `::warning::` annotation, and they run for real on Linux hosts with bubblewrap |
| Desktop USB/runtime resilience | **VERIFIED 2026-10-05** — Power is copied into a SHA-256-addressed `%LOCALAPPDATA%\UnoOne\PowerCache\<digest>` directory and launched from that verified host copy. Automatic repair retries are bounded at 45/90/180/360/720/900 seconds, recovery checkpoints use `.partial`, and manifest generation excludes incomplete `.partial` artifacts. This protects the app and copied package; it does not claim to repair failing USB hardware |
| Windows Dock / Starter | **INTEGRITY-VERIFIED ON DRIVE** — manifest-declared, hash-verified, native `--verify-only` exits 0; transactional staging with automatic rollback proven live |
| Vault encryption (`packages/vault-core`) | IMPLEMENTED AND CORRECTNESS-HARDENED — Argon2id (256 MiB / t=3 / p=4) + AES-256-GCM for new records (legacy XChaCha20-Poly1305 stays readable, identified by nonce length) + HKDF-SHA-256 + BIP-39 recovery + write-ahead journal; transactional first-use setup refuses re-initialisation and preserves packaged `vault.id` bytes. Per-vault random salts on both the password and recovery paths. The KDF parameters are pinned as a cross-platform contract with the Kotlin `encrypted-vault` package. **Wave 1** fixed four release blockers: newest-committed-generation header slot selection, authenticated record metadata re-verified on every read, real journal transactions with fsync-and-verify before promotion, and canonical UUID v4 record IDs |
| Model inference | Bundled llama.cpp only; direct runtime test verified (real answer, 127.0.0.1-only, clean stop) — see `docs/verification/2026-07-30/59_DIRECT_GEMMA.md` |
| Offline voice | VERIFIED pipeline — bundled Piper synth → bundled Whisper transcribe round trip is verbatim; see `docs/verification/2026-07-30/62_OFFLINE_VOICE.md`. The Windows bundle CI also runs real Whisper/Piper inference on pinned assets |
| InBharat Audio speech plane | ACCEPTANCE-GATED + DEPLOYED 2026-08-26 — `vendor/Inbharat-audiocpp/` (audio.cpp @ `26dcb5c4`); Qwen3-ASR-0.6B Q8_0 + omnivoice Q8_0 GGUF at `SPEECH/models/`; 30/30 hash-bound acceptance gate green. The production voice path goes through a `SpeechRouter` (`speech.rs`) with the explicit `InbharatAudioThenLegacy` policy; both routes hash-verify their CLIs/models and report buffered-final semantics. Language tags are canonicalized through `packages/speech-contracts` and byte-sync-checked against the Android mirror in CI; Assamese routes only to IndicConformer and fails closed when absent. **Live speech matrix on the staged drive 2026-09-15: 19/19 pass** (`docs/verification/2026-09-15/116_SPEECH_LANGUAGE_MATRIX.md`) |
| Recording | IMPLEMENTED WITH ENFORCED PRIVACY — `unoone-recording-policy` makes retention decisions exhaustive; TRANSCRIPT_ONLY/SUMMARY_ONLY retain no audio; temp WAV deleted and verify-checked; zero samples reports an error. **SUMMARY_ONLY is disabled in the UI** until a summariser exists |
| Browser workspace | IMPLEMENTED AS TYPED, VERIFIED ACTIONS — no arbitrary script execution; scheme allowlist; JSON-literal escaping; submit/upload/download require explicit confirmation; real PNG screenshots with SHA-256; the popup shim is live-verified on Gmail. Live-page acceptance journeys are human-gated |
| Text handling (Indic scripts) | HARDENED — `packages/text-util` provides grapheme-cluster-safe truncation (eight byte-offset slicing sites that panicked on Devanagari/Bengali text were fixed) |
| Plaintext elimination (Wave 3) | SHIPPED — migration core (PR #11) and read path (PR #15). The 2026-10-04 reload started from a fresh vault; `migrate_plaintext_documents_to_vault` applies to older drives that still hold legacy plaintext documents (run in a human session, backup first) |
| Document parsing | IMPLEMENTED — PDF (lopdf), DOCX/XLSX/PPTX (zip+quick-xml), TXT/MD/CSV/HTML; TF-IDF keyword search (explicitly not "semantic") |
| Browser redirect policy | IMPLEMENTED — `unoone-browser-policy` verdicts (same-origin/registrable → reached, HTTPS→HTTP → failure, cross-origin → surfaced `verified=false`, never a silent success); live WebView acceptance human-gated |
| Android vault repository | SHIPPED — unlock/read/write/tombstone against the Rust vault, proven bidirectionally by cross-platform vectors and a Rust-generated synthetic-vault fixture; integrated into the app flows (PRs #20, #21: the drive vault is the canonical notes/memories store). Remaining: the physical phone ↔ drive ↔ desktop round trip |
| Accessibility (OCR, Blind View) | IMPLEMENTED — OCR/description via Gemma mmproj; confidence honestly `Option<f32>` (unmeasured, never fabricated) |
| Security (vault writes) | IMPLEMENTED — `vault_write_record` writes encrypted records; recording and document content encrypted end-to-end |
| Chat context accuracy | IMPLEMENTED AND TESTED (2026-10-07) — selector, greeting parity, real-core and mounted Chat tests pass locally; desktop Rust in CI. Not yet live-verified on the staged drive |
| Knowledge, verified learning, coding workspace | IMPLEMENTED AND TESTED (2026-10-07) — each part independently reviewed with all findings fixed; adapter suites pass with the real Linux sandbox; CI green on three OSes. **On Windows:** knowledge search, review and export and pasted-text distillation work; verified learning, local-file distillation and coding tasks (view-only) need a Linux host. Not yet staged on the drive or run on Windows hardware |
| macOS | Rust workspace compiles and unit-tests on `macos-latest` in CI; **no macOS app bundle; not tested on Mac hardware** |

Timestamped verification packages in the repo run through
`docs/verification/2026-09-16/` (docs 105–118). Later results — the
2026-10-02…05 desktop work and the 2026-10-07 knowledge/coding change set — are
recorded in this README, the commit messages, and their CI runs; they have no
separate evidence documents in the repo. `docs/verification/2026-07-30/`
(incl. `72_RELEASE_MATRIX.csv`) is a dated historical snapshot; older evidence
must be read with its dates.

### CI gate state

`main` is kept green across all gates; each workflow's own run is the
evidence, not this paragraph:

- **Desktop CI** — mobile protection, frontend build (Vite), speech
  language-table sync (`scripts/check_speech_language_sync.py`), tool-contract
  sync (`scripts/check_tool_contract_sync.py`), InBharat Audio Linux ctest +
  ABI invariant job, Rust `fmt`/`check`/`test`/`clippy` on **windows-latest,
  ubuntu-latest and macos-latest**, secret scan, artifact scan. Linux sandbox
  tests are skipped with a visible warning when the runner lacks bubblewrap.
- **Mobile Protection** (own workflow, every push) — the committed tree-hash
  pointer (`scripts/MOBILE_PROTECTED_TREE`) must equal
  `git rev-parse HEAD:android-app/UnoOneAgent`.
- **Android CI** — repository invariants, speech/tool contract sync, Page
  Agent typecheck/tests/bundle and Playwright e2e, lint, unit tests, debug APK
  assembly, on the `android-app/` and contract paths.
- **Distribution CI** — distribution API typecheck and policy tests,
  Cloudflare Worker bundle, installer PWA typecheck/tests/build, and catalogue
  signing round-trip plus invalid-payload and tamper-rejection proofs.
- **Pocket AI Windows Bundle** — strict manifest and recording-retention
  tests, builds Power/Dock/Starter together in release mode, gates frontend
  embedding, uploads the portable bundle with SHA-256 sums (14-day artifact),
  and smoke-tests real Whisper/Piper inference on pinned voice assets.

Not yet in CI: the desktop frontend Node test suites (`npm run test:*` in
`apps/desktop/src`).

## Latest verified results

### Knowledge and coding workspace (2026-10-07, `31c7d1d` + `05baa79`)

| Gate | Result |
|---|---|
| Desktop CI on `05baa79` | Rust `fmt`/`check`/`test`/`clippy` green on windows-latest, ubuntu-latest, macos-latest; frontend build, audio ctest, contract syncs, secret and artifact scans green |
| Pocket AI Windows Bundle on `05baa79` | Release build of Power/Dock/Starter on real Windows (MSVC), frontend-embedding gate and voice smoke test green |
| Product adapter, real Linux sandbox (Linux x86_64, bubblewrap 0.10) | 179 passed, 1 ignored helper; 50 sandbox tests ran for real (strict mode, parallel) |
| Windows-style CRLF checkout (simulated) | Adapter 179 passed, contracts 13, desktop glue 10 + 15 |
| Frontend (Node 24) | Build and lint pass; context 53, parity 458, real-core 44, mounted Chat 23, Coding Task 25, Knowledge 24 |
| Clippy `-D warnings` | Adapter and contracts clean for the Linux, Windows and macOS targets |
| Not verified | Running the new screens on Windows hardware, real Tauri IPC/WebView2, the local model with these changes, and the 40-task held-out acceptance suite |

### Desktop resilience and autonomous-agent verification (2026-10-05)

Exercised on the target Windows laptop, not inferred from build output:

| Gate | Result |
|---|---|
| GPU identity | NVIDIA GeForce RTX 5050 Laptop GPU, **8,151 MiB** total VRAM reported by `nvidia-smi`; the earlier 4 GB WMI value was not used |
| Inference topology | llama-server starts with `--parallel 1`, a 16,384-token session context, full available GPU offload, and q8_0 KV cache |
| Throughput | 128-token live completion measured **9.75 tokens/s**, up from approximately **6.1 tokens/s** with four competing slots |
| Autonomous tool turn | Live request completed in **6.23 s** with `finish_reason: tool_calls`, selected `fs.list`, and returned valid structured arguments |
| Long-running generations | SSE activity refreshes a 180 s idle timeout; a distinct 12-minute hard ceiling remains |
| USB-safe launch | Starter/Dock launch the digest-verified host-cached Power executable and pass the original package root explicitly |
| Recovered local package | Desktop copy passes `Start UnoOne.exe --verify-only` with `valid=true` and zero failures |
| Regression gates (pre-2026-10-07 code) | Desktop suite 275/275, harness-bridge subset 43/43, adapter suite 27/27, strict clippy, and release build passed |

The USB device itself still produced Windows Event 51/153 and UASPStor 129
transport resets under load. These changes make UnoOne safer and more
recoverable around that failure; they do not relabel a hardware fault as an
application defect.

### Speech + Android hardening cycle (2026-09-07)

Host (Windows 11, MSVC, Rust 1.93) and emulator (AVD "Medium Phone", API
36.1, x86_64) evidence:

| Gate | Result |
|---|---|
| Android host JVM unit tests (touched modules: app, modelmanager, voice, languagepacks, safetyguard, safety) | BUILD SUCCESSFUL |
| Language-pack install, all 7 baseline packs (en/hi/bn/ta/te/kn/ml), size + SHA-256 verified on-device | `LanguagePackInstallTest` green (5 Indic TTS pins re-pinned after an upstream re-upload; E4B size corrected) |
| Assamese refusal (planned pack, no qualified models) | Install returns Failure, state stays not-installed |
| Real ONNX TTS synthesis (en + hi) + STT engine loads | `SpeechEngineFunctionalTest` — `ttsFailures=0 sttFailures=0` |
| Indic TTS→ASR round trip | `IndicSpeechRoundTripTest` green (hi) |
| Uninstall/retain shared ASR + repair | `LanguagePackRepairRetainTest` green |
| No-cloud speech fallback refusal | `SpeechNoCloudFallbackTest` 4/4 green |
| Headless on-device agent logic batch | 41/41 green |
| Secure browser asset, guarded form fill, local-page read, DOCX/PDF round trips | 8/8 green |
| Local-brain (Gemma) device qualification | Skipped via `assumeTrue` — no `.litertlm` model on the emulator |

Older speech evidence tiers are in
[`docs/SPEECH_TEST_EVIDENCE.md`](SPEECH_TEST_EVIDENCE.md).

### Physical Pocket AI release verification (2026-07-29)

| Gate | Result |
|---|---|
| Physical package | `D:\UNOONE`, exFAT, label `UNOONE`, version `0.5.0-alpha` |
| Strict on-drive verifier | Pass — 545/545 declared assets, exit `0` |
| Native Starter verifier | Pass — `Start UnoOne.exe --verify-only`, exit `0` |
| Windows applications | Power, Dock, and Starter built together and SHA-256 verified |
| Recovery | Every replaced path was staged transactionally |

The manifest provides complete path, size, and SHA-256 integrity checking; it
is not cryptographically signed, and the Windows executables are unsigned. See
[`docs/52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md`](52_POCKET_AI_PHYSICAL_RELEASE_2026-07-29.md).
(The drive was later reloaded on 2026-10-04 — see Current Status.)

### Android physical evidence (Xiaomi 14 `7f8cafef`, Android 15, July 2026)

| Gate | Result |
|---|---|
| Connected-device instrumentation, July 17 | `OK (55 tests)` in 206.218 seconds |
| No-network subset, July 17 | `OK (20 tests)` with Wi-Fi and mobile data disabled |
| PDF and DOCX Android round trips | `OK (2 tests)`; exact values persisted and original bytes unchanged |
| Phone actions | WhatsApp Business, Gmail, and Calendar opened with foreground-package verification; drafts remain reviewable |
| Read Screen | Read actual Accessibility Settings content and spoke the result |
| Page Agent physical WebView | Read rendered page text and executed guarded text entry; complex public-site completion not yet qualified |
| Hindi speech and Blind Aid | Hindi STT/TTS initialized; COCO labels detected and spoken in Hindi, with no stale narration after stop |
| Master disable | Blocked commands and speech, survived process restart, did not replay the old request |
| Crash/ANR/OOM scan | No UnoOne crash, ANR, OOM, or low-memory kill in the final inspected run |

Details: [Connected-device validation](DEVICE_VALIDATION_2026-07-17.md)
and [Device verification](../DEVICE_VERIFICATION.md). The Xiaomi 14 is the only
Android device with recorded evidence.

## Mobile Protection

The Android app is protected by a committed tree-hash pointer plus exact-file
hash verification. `scripts/MOBILE_PROTECTED_TREE` holds the expected tree hash
of `android-app/UnoOneAgent` (currently `bd97bee7…`), and CI fails any push
where `git rev-parse HEAD:android-app/UnoOneAgent` differs from the pointer.
`scripts/MOBILE_GOLDEN_HASHES.txt` carries the per-file blob hashes so the
protected tree can be audited file-by-file. Re-baselining is an ordinary
reviewable commit via `scripts/regen-mobile-golden-hashes.sh` — run it after
committing the Android changes (the script pins the committed tree), then
commit the two baseline files together.

```bash
# Local check (same comparison CI makes)
test "$(cat scripts/MOBILE_PROTECTED_TREE)" = "$(git rev-parse HEAD:android-app/UnoOneAgent)" && echo PASS
```

See [docs/MOBILE_GOLDEN_BASELINE.md](MOBILE_GOLDEN_BASELINE.md) for the
full protection policy.
