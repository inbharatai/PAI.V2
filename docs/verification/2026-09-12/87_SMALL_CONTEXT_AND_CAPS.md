# 87 — Three Structural Blockers to "Autonomous, Uncapped, World-Class": 4K Runtime Context, Budget Walls, and a Denial-Prone Briefing

**Date:** 2026-09-12
**Scope:** `apps/desktop/src-tauri/src/llama.rs`, `harness_bridge.rs`
**Posture:** live-caught during the user-mandated long-coding acceptance.

## 1. Symptom (live)

Through the real chat UI, the agent was asked to build a complete 4-file web
app in its workspace and test it with Playwright. The wire forensics showed
the harness lane was fully engaged (11 tools including `fs.write` and
`process.run` in the request, correct full-access system briefing) — yet the
model answered "I do not have direct access to your computer's filesystem or
the ability to execute shell commands" and pasted all four files into the chat
as code blocks. A direct follow-up probe ("create wire-probe.txt with
fs.write, read it back") worked perfectly — proving transport was fine.

## 2. Root causes

### 2a. The runtime never got the 32K context lane (the big one)

The drive's llama-server spawned with `-c 4096` and **no** `-ctk/-ctv`
flags. Commit 8261261 ("feat(desktop): vision chat, 32K context…") added the
`cache_type_k/v` plumbing and a Model-view **dropdown** for 16K/32K + q8_0 —
but `ModelConfig::default()` (used by both the App auto-boot and the
ModelManager manual start) still meant `context_size: 4096`, K/V `f16`.
Total window: 4096 tokens for system briefing + tool manifest + prior
conversation + tool results + generation. A multi-file coding session cannot
fit; long prompts get context-shifted by the server. The "32K lane" existed
only if the user happened to open Model → advanced settings and select it.

**Fix:** `get_model_config()` is now genuinely host-adaptive — it probes
total physical RAM (GlobalMemoryStatusEx / /proc/meminfo / sysctl) and picks:
≥24 GiB → 32768 ctx with q8_0 K+V (the lane verified live on the target
RTX 5050 laptop class); ≥12 GiB → 16384 + q8_0; else the safe 4096 f16
baseline. The Model dropdown remains as an override.

### 2b. Budget walls sized for demos, not sessions

Full-access budgets were 48 steps / 96 tool calls / **900s** / 2 MiB. One
long generation on the local 12B took 9 minutes alone. User directive:
"there should be no cap." The harness's budgets are structurally
non-bypassable, so they are now set as a **pathological-loop backstop, not a
task cap**: 512 steps / 1024 tool calls / 8 rounds / 64 MiB / 6 hours.

### 2c. The briefing didn't pre-empt the failure mode

The full-access system prompt said "actually use the tools instead of
claiming you cannot" — but the 12B still denied capability and pasted code.
Extended with an explicit autonomous-agent contract: do the whole task
yourself, create every file with fs.write, verify with process.run/fs.read,
never paste code instead of creating files, never claim you cannot access
the filesystem; plus natural-assistant behavior (short plan first for
complex tasks, natural answers for questions/ideas, tools only when needed)
per the user's "understand basic human language, plan, give ideas, execute".

## 3. Regression tests

- `boot_config_matches_the_detected_memory_step` — the adaptive mapping holds
  for whatever RAM class the host reports (CI runners included).
- `detected_ram_is_plausible` — the probe returns sane values on real hosts.
- Existing 110 desktop tests green; clippy `-D warnings` clean.

## 4. Post-fix acceptance (live, on the re-staged drive)

- Spawn flags show `-c 32768 -ctk q8_0 -ctv q8_0`.
- The long-coding task: agent creates all files with fs.write, runs the
  Playwright test with process.run (exit 0), reports results — no chat
  pasting, no denial.
- Result served on localhost and opened in the Browser workspace.