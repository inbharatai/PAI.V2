#!/usr/bin/env sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
UPSTREAM=${1:-$ROOT/../upstream/audio.cpp}
PIN=bb15edd78b56e035967e0eb999a6b28a62337db4
actual=$(git -C "$UPSTREAM" rev-parse HEAD)
[ "$actual" = "$PIN" ] || { echo "wrong audio.cpp pin: $actual" >&2; exit 1; }
[ -z "$(git -C "$UPSTREAM" status --porcelain)" ] || { echo "audio.cpp checkout is modified" >&2; exit 1; }
echo "audio.cpp pin/pristine check OK: $PIN"
