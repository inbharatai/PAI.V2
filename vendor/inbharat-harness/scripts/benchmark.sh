#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
"$root/scripts/cargo-portable.sh" build --release -p inbharat-harness-cli
if command -v cc >/dev/null 2>&1; then
  bin="$root/target/release/inbharat-harness"
else
  bin="$root/target/x86_64-unknown-linux-musl/release/inbharat-harness"
fi
"$bin" benchmark --iterations "${ITERATIONS:-100000}" --output "$root/benchmarks/routing-latest.json"
