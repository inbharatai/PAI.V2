# Wrapper Overhead — provider dispatch vs direct reference call

Question: does routing inference through the provider vtable (universal-core refactor, commit c863cc9) add meaningful latency over calling the reference engines directly?

## Method

50 iterations of `ibaudio benchmark` on the same host, Release build, CPU backend.

- **Wrapped (current, provider-dispatched):** this tree at `bef2424`, `build/linux-release/ibaudio benchmark`.
- **Direct (pre-provider):** worktree at `2f1dff9` (immediately before the provider dispatch), same benchmark.

## Measured (mean ms, lower is better)

| Operation | Direct (pre-provider) | Wrapped (provider) | Delta |
|---|---:|---:|---:|
| ASR | 0.2442 | 0.2387 | −2.3% |
| VAD  | 0.1328 | 0.1283 | −3.4% |
| TTS  | 0.4832 | 0.4814 | −0.4% |

## Conclusion

The provider indirection imposes **no measurable regression** — all three deltas are negative and within run-to-run noise at these sub-millisecond magnitudes. This satisfies the design goal that the universal wrapper add nearly negligible latency over the raw engine. (These are deterministic-reference timings, not neural inference claims.)

## Regression budget

Any future provider must be re-measured against this harness. A wrapper regression is investigated when the wrapped mean exceeds the direct mean by **>5%** on the same host and build. The audio.cpp provider, when implemented, is measured the same way against upstream's own CLI.

## Note

The direct baseline was built at `2f1dff9`, which predates the research-gating split — its ABI is the 94-symbol superset. The benchmarked operations (ASR/VAD/TTS on the reference engines) are identical in both; only the dispatch path differs.
