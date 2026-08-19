# Linux ARM64 / Raspberry Pi

The Harness core is platform-neutral Rust. Pocket AI Pi uses the same provider contracts as desktop and Android; Pi-specific model, vault, tools, GPIO, and lifecycle behavior belongs in the product adapter, not the core.

## Supported production target

`aarch64-unknown-linux-gnu` is the primary Raspberry Pi OS 64-bit target. The release gate requires a real AArch64 executable plus the C ABI library; a host-only `cargo check` is not sufficient evidence.

Build with:

```sh
./scripts/build-linux-arm64.sh
```

On an x86_64 build host install the Rust target and an AArch64 GNU cross linker first. On a Pi 4/5 running a 64-bit OS, build natively with the same script.

## Runtime requirements

- 64-bit Linux (`uname -m` must be `aarch64`/`arm64`).
- Provider-driven model execution; the standalone CLI contains no mock/echo model execution path.
- Network capability denied unless the embedding product explicitly grants it.
- Product-specific canonical memory must be supplied through `MemoryProvider`.
- Arbitrary process execution must not be exposed by Pocket AI; only typed allowlisted tools may be registered.
- USB removal must cancel active Harness work before the vault is locked/unmounted.

## Evidence required before release

1. `cargo test -p inbharat-harness-core` passes.
2. `cargo build --locked --release --target aarch64-unknown-linux-gnu` passes.
3. `file` identifies the CLI as AArch64.
4. CLI `info`, L0 route, L1 route, and fail-closed capability tests run on the actual Pi.
5. Pocket AI provider integration passes real model/tool/memory tests on the actual Pi.
