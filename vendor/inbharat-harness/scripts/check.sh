#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
if [ -n "${CARGO:-}" ]; then
  cargo_bin=$CARGO
elif command -v cargo >/dev/null 2>&1; then
  cargo_bin=$(command -v cargo)
else
  echo "cargo not found; install Rust or set CARGO" >&2
  exit 69
fi
"$cargo_bin" fmt --all --check
"$root/scripts/cargo-portable.sh" check --workspace --all-targets
"$root/scripts/cargo-portable.sh" test --workspace --all-features
"$root/scripts/cargo-portable.sh" clippy --workspace --all-targets -- -D warnings
"$root/scripts/smoke.sh"
