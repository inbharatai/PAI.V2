#!/usr/bin/env sh
# BUILD-ONLY cross-compile of the InBharat Audio Android scaffold (arm64-v8a).
#
# What this script proves: the universal library AND the thin JNI bridge
# (ibaudio_jni) compile against the Android NDK for arm64-v8a. What it does
# NOT prove: anything executed on a device or emulator. Artifacts are
# Android-target binaries that cannot run on the host — evidence tier is
# BUILD-ONLY, and any runtime/physical-device claim requires an actual device
# run (see docs/SPEECH_TEST_EVIDENCE.md tiers).
#
# Usage:
#   scripts/build_android_scaffold.sh [--sanitizers] [--tests]
#     ANDROID_NDK=<path>   NDK root (default: auto-detect 25.1.8937393)
#     BUILD_DIR=<path>     output build dir (default: build/android-arm64)
#   --sanitizers  add ASan+UBSan (NDK clang; supported on Android targets)
#   --tests       also build the release-candidate test executables
#                 (compile-only — they cannot run on the host)
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

SANITIZERS=0
TESTS=0
for arg in "$@"; do
    case "$arg" in
        --sanitizers) SANITIZERS=1 ;;
        --tests) TESTS=1 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

if [ -n "${ANDROID_NDK:-}" ]; then
    NDK="${ANDROID_NDK}"
else
    for candidate in \
        "${ANDROID_HOME:-}/ndk/25.1.8937393" \
        "${LOCALAPPDATA:-}/Android/Sdk/ndk/25.1.8937393" \
        "${USERPROFILE:-}/Android/Sdk/ndk/25.1.8937393" \
        "${HOME:-}/Android/Sdk/ndk/25.1.8937393"; do
        if [ -f "$candidate/build/cmake/android.toolchain.cmake" ]; then
            NDK="$candidate"
            break
        fi
    done
fi
if [ -z "${NDK:-}" ] || [ ! -f "$NDK/build/cmake/android.toolchain.cmake" ]; then
    echo "NDK not found; set ANDROID_NDK to an NDK root containing build/cmake/android.toolchain.cmake" >&2
    exit 1
fi

CMAKE=${CMAKE:-$(command -v cmake || true)}
NINJA=""
if [ -z "$CMAKE" ]; then
    for candidate in \
        "${LOCALAPPDATA:-}/Android/Sdk/cmake/3.22.1/bin/cmake.exe" \
        "${LOCALAPPDATA:-}/Android/Sdk/cmake/3.22.1/bin/cmake"; do
        if [ -f "$candidate" ]; then CMAKE="$candidate"; NINJA="${candidate%cmake*}ninja.exe"; break; fi
    done
fi
: "${CMAKE:?cmake not found; install it or set CMAKE}"

BUILD=${BUILD_DIR:-$ROOT/build/android-arm64}
mkdir -p "$BUILD"

ARGS="-G Ninja -DCMAKE_TOOLCHAIN_FILE=$NDK/build/cmake/android.toolchain.cmake -DANDROID_ABI=arm64-v8a -DANDROID_PLATFORM=android-28 -DIBAUDIO_BUILD_CLI=OFF -DIBAUDIO_BUILD_MCP=OFF -DIBAUDIO_BUILD_ANDROID_JNI=ON -DIBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=OFF -DIBAUDIO_ENABLE_SHERPA_ONNX=OFF -DIBAUDIO_ENABLE_VULKAN_PROBE=OFF"
if [ -n "$NINJA" ] && [ -f "$NINJA" ]; then
    ARGS="$ARGS -DCMAKE_MAKE_PROGRAM=$NINJA"
fi
if [ "$TESTS" -eq 1 ]; then
    ARGS="$ARGS -DIBAUDIO_BUILD_TESTS=ON"
else
    ARGS="$ARGS -DIBAUDIO_BUILD_TESTS=OFF"
fi
if [ "$SANITIZERS" -eq 1 ]; then
    ARGS="$ARGS -DIBAUDIO_ENABLE_SANITIZERS=ON"
fi

# shellcheck disable=SC2086
"$CMAKE" -S "$ROOT" -B "$BUILD" $ARGS
# shellcheck disable=SC2086
"$CMAKE" --build "$BUILD" --parallel "${JOBS:-4}"

echo
echo "BUILD-ONLY PASS: ibaudio + ibaudio_jni compiled for arm64-v8a (NDK $(basename "$NDK"))."
echo "No artifact was executed: Android binaries do not run on this host."
echo "Artifacts: $BUILD"