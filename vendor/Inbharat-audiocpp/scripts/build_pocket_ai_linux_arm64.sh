#!/usr/bin/env sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SOURCE=${IBAUDIO_AUDIO_CPP_SOURCE_DIR:-}
[ -n "$SOURCE" ] || { echo "IBAUDIO_AUDIO_CPP_SOURCE_DIR must point to the exact reviewed audio.cpp checkout" >&2; exit 64; }
[ -d "$SOURCE/.git" ] || { echo "audio.cpp source is not a Git checkout: $SOURCE" >&2; exit 65; }
BUILD=${IBAUDIO_PAI_ARM64_BUILD_DIR:-$ROOT/build-pocket-ai-linux-arm64}
NATIVE=0
case "$(uname -m)" in aarch64|arm64) NATIVE=1;; esac
TOOLCHAIN=""
if [ "$NATIVE" -eq 0 ]; then
  command -v aarch64-linux-gnu-gcc >/dev/null 2>&1 || { echo "aarch64-linux-gnu-gcc missing" >&2; exit 69; }
  command -v aarch64-linux-gnu-g++ >/dev/null 2>&1 || { echo "aarch64-linux-gnu-g++ missing" >&2; exit 69; }
  TOOLCHAIN="-DCMAKE_TOOLCHAIN_FILE=$ROOT/cmake/toolchains/linux-aarch64-gcc.cmake"
fi
cmake -S "$ROOT" -B "$BUILD" -DCMAKE_BUILD_TYPE=Release \
  -DIBAUDIO_BUILD_TESTS=OFF \
  -DIBAUDIO_ENABLE_TEST_FIXTURE_MODELS=OFF \
  -DIBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=OFF \
  -DIBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=ON \
  -DIBAUDIO_AUDIO_CPP_SOURCE_DIR="$SOURCE" \
  $TOOLCHAIN
cmake --build "$BUILD" --parallel
CLI="$BUILD/ibaudio"
[ -x "$CLI" ] || { echo "missing ibaudio CLI: $CLI" >&2; exit 70; }
file "$CLI" | grep -Eqi 'ARM aarch64|aarch64' || { echo "output is not AArch64" >&2; file "$CLI" >&2; exit 71; }
STATUS=$($CLI audio-cpp-status --json)
printf '%s\n' "$STATUS"
printf '%s' "$STATUS" | grep -q '"adapter_compiled":true' || { echo "reviewed audio.cpp provenance adapter was not compiled" >&2; exit 72; }
MODELS=$($CLI models --json)
printf '%s' "$MODELS" | grep -Eqi 'reference-asr|reference-tts|deferred-kws' && { echo "production binary exposes fixture/deferred engines" >&2; exit 73; } || true
mkdir -p "$ROOT/dist/pocket-ai-linux-arm64"
cp "$CLI" "$ROOT/dist/pocket-ai-linux-arm64/ibaudio"
for candidate in "$BUILD/libinbharat_audio.so" "$BUILD/libinbharat_audio.a"; do [ -f "$candidate" ] && cp "$candidate" "$ROOT/dist/pocket-ai-linux-arm64/"; done
sha256sum "$ROOT/dist/pocket-ai-linux-arm64/"* > "$ROOT/dist/pocket-ai-linux-arm64/SHA256SUMS"
echo "PASS: Pocket AI Linux ARM64 InBharat Audio provenance build complete"
echo "NOTE: this does not itself prove ASR/TTS. Run the real audio.cpp physical acceptance next."
