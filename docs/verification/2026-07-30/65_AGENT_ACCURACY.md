# 65 — Agent accuracy

**Status: BUILDS_NOT_RUNTIME_TESTED** for the live-model run;
**VERIFIED_WORKING** for the deterministic guard layer (16/16 unit tests).

## Deterministic harness (runs without a model, part of `cargo test`)

- correct tool with valid args → accepted (4 tools × tests)
- wrong/unknown tool → rejected and named (`Unknown tool: X`)
- malformed call → rejected (wrong arg type)
- missing/empty args → rejected before execution; the model is told why
- repeated identical call → circuit-breaker fires at threshold 2
- distinct calls / single repeat / zero threshold → not flagged
- unmeasured confidence is `None`, never 1.0 (regression test)

## Loop hardening

- Tool calls arrive schema-validated from llama-server's tool parser;
  `validate_tool_call` adds product-side arg validation before safety review.
- Repetition circuit breaker with canonical (tool, args) fingerprints.
- Whole-loop 240 s deadline with a truthful timeout response.
- `InvalidToolCall` and `SafetyBlock` steps are visible in the UI.
- No arbitrary shell; no unrestricted browser scripting (see 60).

## Live-model journey (human gate, needs unlocked vault)

The full suite against the bundled Gemma (correct tool, wrong tool rejected,
malformed call, unsafe action, failed verification, repeated loop, model
refusal, timeout, USB disconnect) must run while a human watches and can
confirm observed behavior matches the step trail. BLOCKED_BY_ENVIRONMENT for
an unattended session; nothing here claims live-model accuracy was measured.
