# Harness RC2 platform evidence

| Target | Build/check | Runtime/tests | Label |
|---|---:|---:|---|
| Linux x86_64 musl | pass | debug and release, 60/60 each; CLI and C ABI | host-tested |
| Windows x86_64 GNU | cargo check pass | not run | compile-only |
| Android ARM64 | cargo check pass | not run | compile-only |
| macOS | unavailable | not run | pending |

Compile-only is not runtime support. OS sandbox, descendant-process, packaging, and device lifecycle claims require target runners.
