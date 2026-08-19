#!/bin/sh
set -eu

if [ "$#" -lt 1 ]; then
  echo "usage: cargo-portable.sh <cargo-subcommand> [args...]" >&2
  exit 64
fi

if [ -n "${CARGO:-}" ]; then
  cargo_bin="$CARGO"
elif command -v cargo >/dev/null 2>&1; then
  cargo_bin="$(command -v cargo)"
else
  echo "cargo not found; install Rust or set CARGO" >&2
  exit 69
fi

subcommand=$1
shift

if command -v cc >/dev/null 2>&1; then
  exec "$cargo_bin" "$subcommand" "$@"
fi

target=x86_64-unknown-linux-musl
rustc_bin="$(dirname "$cargo_bin")/rustc"
rustup_bin="$(dirname "$cargo_bin")/rustup"
"$rustup_bin" target add "$target" >/dev/null
sysroot="$($rustc_bin --print sysroot)"
linker="$sysroot/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
if [ ! -x "$linker" ]; then
  echo "no C linker and rust-lld not found" >&2
  exit 69
fi
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="$linker"
exec "$cargo_bin" "$subcommand" --target "$target" "$@"
