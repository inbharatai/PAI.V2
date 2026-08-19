#!/usr/bin/env sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BUILD=${BUILD:-$ROOT/build/linux-release}
OUT=${OUT:-$ROOT/benchmarks}
mkdir -p "$OUT"
"$BUILD/ibaudio" benchmark --iterations "${ITERATIONS:-20}" \
  --output-json "$OUT/reference_cpu.json" --output-csv "$OUT/reference_cpu.csv"
