# 03 — Production vs Research Classification (as audited 2026-08-20)

Deciding method: the real algorithm in each `.cpp` was traced (not the header comments), the ABI exposure checked, and the test behavior read — including whether `tests/innovation/innovation_tests.cpp` compiles against the shipped header. **It does not** (line 110 references nonexistent `IBAUDIO_STATUS_PERMISSION_DENIED`); the green `INNOVATION_BUILD.log` came from a stale build (`ninja: no work to do`). This is the most urgent defect in the tree: claimed test evidence for the innovation lane is illusory.

## Verdicts

| Module | Real algorithm | Verdict | Flag? |
|---|---|---|---|
| `conversation_state` | Hand-coded 5-state FSM, correct transitions | **PRODUCTION-READY** (name/comment/code agree; needs broader transition coverage) | No |
| `codeswitch_detector` | Script-ratio heuristic; any byte ≥128 = "Devanagari"; RMS-as-confidence | USEFUL-HEURISTIC (relabel; fix UTF-8 codepoint handling) | No, but relabel |
| `turn_manager` | Threshold decision tree on caller-supplied features | USEFUL-HEURISTIC (relabel "semantic"→"rule-based"; pin each branch) | No, but relabel |
| `context_aware_output` | Fixed gain/rate rule table; apply() is volume-only | USEFUL-HEURISTIC (document advisory outputs; fix boundary test) | No |
| `environment_adapter` | Percentile noise floor + amplitude gate; sorted-energies "reverb" is not a decay | MISLEADING name/comments over functional DSP | Yes (comments) |
| `prosody_controller` | Linear control→parameter map; text arg unused; compute/apply not on ABI | PLACEHOLDER as shipped (write-only state) | Yes |
| `voice_clone_engine` | 256-bin magnitude histogram "embedding", hardcoded 0.9 confidence, no synthesis | PLACEHOLDER with a real consent gate | Yes |
| `neural_codec` | Scalar RMS quantization + sine synthesis; no NN, no RVQ | **MISLEADING** (name overclaims a specific neural architecture) | Yes |

## Cross-cutting findings

1. **No research flag exists.** All eight modules are unconditional members of `libibaudio` (`CMakeLists.txt:35-42`). Introducing `IBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES` (default OFF) is itself a required change; wrap `neural_codec`, `prosody_controller`, `voice_clone_engine` sources and their header declarations so the stable ABI stops promising a neural codec and a voice cloner it does not have.
2. **Broken test source.** Add `IBAUDIO_STATUS_PERMISSION_DENIED` to the status enum (or change the test to the implemented `INVALID_ARGUMENT`) and force a clean rebuild in CI before any innovation claim is repeated.
3. **Integration bypass.** Innovation wrappers bypass `guarded()`/`set_error`/metrics; codec buffers never join `live_owned_buffers` (runtime release cannot see them); several output structs never set `struct_size/api_version`. Any module promoted out of research must first be wired into the façade machinery.

## What would make each production-honest

- `neural_codec`: rename to `scalar_energy_codec` or remove; a real codec enters via the model registry with a trained RVQ model; add round-trip SNR test; make bitrate formula match the emitted encoding.
- `voice_clone_engine`: add the missing status enum; implement a real embedding + TTS conditioning path or rename to `speaker_enrollment_registry`; honor or delete the hardcoded confidence.
- `prosody_controller`: expose compute/get_params over the ABI or wire into TTS; use the text argument (punctuation→pause) or remove it; fix dead urgency scaling; assert an output in tests.
- `environment_adapter`: rename comments to "energy-gate noise suppression"; fix the sorted-energies reverb estimate; remove the dead first `is_noisy` assignment; implement true spectral subtraction or stop calling it that.
- `codeswitch_detector`: decode UTF-8 codepoints and restrict Devanagari to U+0900–U+097F; drop or rename RMS "confidence"; expose `get_recommended_model` or delete it; add negative tests (French, Arabic).
- `turn_manager`: rename "semantic"→"rule-based"; pin all five branches; document that feature extraction is the caller's job.
- `context_aware_output`: document rate/emphasis/pause as advisory or implement them in the TTS path; fix the −30 dBFS boundary test; add sample-value assertions on apply().
- `conversation_state`: exhaustive transition-table tests; expose or remove the unreachable `agent_has_more` path.
