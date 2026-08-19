# InBharat Harness adversarial hardening audit

Audit target: `0.1.0-rc.1` at `fb86e6500e7d0f977a6708e75c9d183e2e2f8d8f`  
Hardened result: `0.1.0-rc.2` on the local `hardening-audit` branch  
Disposition: no confirmed Critical findings; all confirmed High and Medium findings below are fixed and regression-tested.

## Confirmed findings

| ID | Severity | Affected area | Finding and impact | Resolution and regression evidence |
|---|---|---|---|---|
| H-01 | High | Root `Cargo.toml`; FFI `ffi_boundary` | Release profile used `panic=abort` while the public C ABI promised `catch_unwind` containment. Any internal release panic would terminate the embedding process instead of returning `IB_STATUS_PANIC`. | Release profile now explicitly uses unwind semantics. `panic_is_contained_at_ffi_boundary` runs in debug and release test lanes; the external C consumer is rebuilt in final gates. |
| H-02 | High | `runtime::run_model_loop` | Every new turn initialized model history from only the current prompt. Resume and multi-turn sessions therefore persisted prior facts but silently withheld them from the model, violating the session/context contract. | Model history is now reconstructed from bounded durable user, assistant, and tool-result facts: at most 256 messages and 2 MiB. `resumed_turns_receive_bounded_prior_conversation_history` verifies exact role/content ordering. |
| H-03 | High | `RunOptions`; model retry loop; stream callback | A caller could set billions of recovery attempts, and zero-byte model chunks did not consume the byte budget. Fast failing or zero-byte providers could cause sustained CPU/event denial of service. | Recovery attempts are capped at 8, custom budgets have hard ceilings, every retry checks cancellation/deadline, and streams stop after 16,384 chunks. Regression tests cover unbounded recovery and a 20,000 zero-byte chunk flood. |
| H-04 | Medium | `Router::escalate`; route capability metadata | A redundant escalation cause at L1/L2 advanced to the next level even though the cause required no higher authority. L0 and `show file` route metadata also omitted capabilities actually required at execution. | Redundant escalation now fails, skipped escalation remains forbidden, and route metadata declares model/read capabilities accurately. Unit tests cover redundant causes and L0 capability failure. |
| H-05 | Medium | `ModelRegistry` | Registry preparation accepted any model string for a registered provider and did not bound request collections/bytes. Providers could receive unsupported model names or oversized requests. | Provider catalogues are validated for non-empty unique bounded model IDs; prepare rejects unadvertised models and bounds system text, messages, tools, attachments, and output. Regression tests cover unsupported and oversized requests. |
| H-06 | Medium | `CancellationToken` | Parent traversal was recursive and `Instant + Duration::MAX` could panic. Deep child chains caused stack overflow during lookup or destruction. | Cancellation ancestry is stored as a flat state vector, lookup is iterative, and timeout uses elapsed subtraction. A 10,000-level chain and `Duration::MAX` are regression-tested. |
| H-07 | Medium | `LocalExecutionBroker` | Allowlisted executable names were resolved through ambient host `PATH` at each spawn, and invalid environment keys could reach `Command` and panic. This weakened allowlist identity and exposed a local denial path. | Names are resolved once to canonical executable paths when the broker is created; unresolved entries fail closed. Child environment keys/values are bounded and reject `=`, NUL, and empty names. Security tests cover both paths. |
| H-08 | Medium | `ToolManifest::validate` | Custom tools could register malformed or oversized JSON schemas and zero-duration/unbounded manifests, producing invalid model contracts or resource pressure. | Manifest identifiers, text, schema, output, timeout, and verification fields are bounded; both schemas must parse as JSON objects. Regression tests cover malformed and oversized manifests. |
| H-09 | Low | `SessionStore::resume` | Resume did not independently cap physical event count or require `session.started` as the first event before lifecycle replay. | Resume enforces the one-million event limit and first-event invariant before reconstructing state. Existing corruption/torn-tail/replay gates continue to pass. |
| H-10 | Low | Fast router grammar | Broad “build/implement a complete …” prefixes activated L3 for ordinary explanatory phrases. | Workspace activation now requires concrete software nouns and excludes outline/plan/explanation patterns. Added false-activation regressions preserve zero ordinary L2/L3 activation. |
| H-11 | Low | Dependency-free JSON parser | Existing malformed coverage was narrow. | Added a deterministic 50,000-case arbitrary-byte campaign; every accepted value must canonicalize and parse identically.

## Validation lanes

- Formatting, workspace check, full tests, Clippy with warnings denied, CLI smoke, and benchmark.
- Release-mode workspace tests, including FFI panic containment.
- Windows GNU and Android ARM64 Rust cross-checks.
- External C11 ABI consumer and ABI symbol-manifest comparison.
- Deterministic route, malformed JSON, cancellation, process, session corruption, recovery, capability, and model-stream adversarial tests.

Raw logs are retained as `reports/HARDENING_*` files. Final counts and artifact hashes are recorded in `HARDENING_VALIDATION.md` after the clean release gate.

## Residual risks, not misrepresented as fixed

- `RootedFs` remains an in-process fence with a shared-tree symlink/rename TOCTOU residual; it is not a hostile multi-user kernel sandbox.
- Direct-child cancellation does not own daemonized descendants across every platform.
- Blocking third-party model/tool providers must cooperate with cancellation; process isolation is required for untrusted providers.
- The event chain detects corruption but is not keyed cryptographic authentication or encryption.
- Invalid/stale foreign pointers and concurrent handle destruction remain C-caller undefined behavior.
- No remote authentication, network egress broker, encrypted store, WASM host, or production credential vault is claimed.
