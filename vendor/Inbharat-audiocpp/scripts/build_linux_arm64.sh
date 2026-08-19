#!/usr/bin/env sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
BUILD=${IBAUDIO_ARM64_BUILD_DIR:-$ROOT/build-linux-arm64}
NATIVE=0
case "$(uname -m)" in aarch64|arm64) NATIVE=1;; esac
if [ "$NATIVE" -eq 0 ]; then
  command -v aarch64-linux-gnu-gcc >/dev/null 2>&1 || { echo "aarch64-linux-gnu-gcc missing" >&2; exit 69; }
  command -v aarch64-linux-gnu-g++ >/dev/null 2>&1 || { echo "aarch64-linux-gnu-g++ missing" >&2; exit 69; }
  TOOLCHAIN="-DCMAKE_TOOLCHAIN_FILE=$ROOT/cmake/toolchains/linux-aarch64-gcc.cmake"
else
  TOOLCHAIN=""
fi
# Production build: no fixture/reference ASR/TTS and no research prototypes.
cmake -S "$ROOT" -B "$BUILD" -DCMAKE_BUILD_TYPE=Release \
  -DIBAUDIO_BUILD_TESTS=OFF \
  -DIBAUDIO_ENABLE_TEST_FIXTURE_MODELS=OFF \
  -DIBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=OFF \
  $TOOLCHAIN
cmake --build "$BUILD" --parallel
CLI="$BUILD/ibaudio"
[ -x "$CLI" ] || { echo "missing ibaudio CLI: $CLI" >&2; exit 70; }
file "$CLI" | grep -Eqi 'ARM aarch64|aarch64' || { echo "output is not AArch64" >&2; file "$CLI" >&2; exit 71; }
MODELS=$($CLI models --json)
printf '%s' "$MODELS" | grep -Eqi 'reference-asr|reference-tts|deferred-kws' && {
  echo "production ARM64 binary exposes fixture/deferred model" >&2; exit 72;
} || true
mkdir -p "$ROOT/dist/linux-arm64"
cp "$CLI" "$ROOT/dist/linux-arm64/ibaudio"
for candidate in "$BUILD/libinbharat_audio.so" "$BUILD/libinbharat_audio.a"; do
  [ -f "$candidate" ] && cp "$candidate" "$ROOT/dist/linux-arm64/"
done
sha256sum "$ROOT/dist/linux-arm64/"* > "$ROOT/dist/linux-arm64/SHA256SUMS"
echo "PASS: InBharat Audio Linux ARM64 production artifacts are in dist/linux-arm64"
