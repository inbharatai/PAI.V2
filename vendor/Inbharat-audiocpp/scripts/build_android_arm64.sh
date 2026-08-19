#!/usr/bin/env sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
: "${ANDROID_NDK_HOME:?Set ANDROID_NDK_HOME to an installed NDK}"
CMAKE=${CMAKE:-cmake}
PRESET=${PRESET:-android-arm64-cpu}
"$CMAKE" --preset "$PRESET" -S "$ROOT"
"$CMAKE" --build --preset "$PRESET"
echo "Build-only evidence created. Do not label Android device-tested until docs/ANDROID.md gates pass."
