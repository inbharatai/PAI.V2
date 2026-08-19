#!/usr/bin/env sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TOOLCHAINS_ROOT=${TOOLCHAINS_ROOT:-}
if [ -n "$TOOLCHAINS_ROOT" ]; then
  CMAKE=${CMAKE:-$TOOLCHAINS_ROOT/cmake-3.30.5-linux-x86_64/bin/cmake}
  NINJA=${NINJA:-$TOOLCHAINS_ROOT/ninja/ninja}
  CC=${CC:-$TOOLCHAINS_ROOT/bin/cc}
  CXX=${CXX:-$TOOLCHAINS_ROOT/bin/c++}
else
  CMAKE=${CMAKE:-$(command -v cmake || true)}
  NINJA=${NINJA:-$(command -v ninja || true)}
  CC=${CC:-$(command -v cc || true)}
  CXX=${CXX:-$(command -v c++ || true)}
fi
: "${CMAKE:?cmake not found; install it or set CMAKE/TOOLCHAINS_ROOT}"
: "${NINJA:?ninja not found; install it or set NINJA/TOOLCHAINS_ROOT}"
: "${CC:?C compiler not found; install it or set CC/TOOLCHAINS_ROOT}"
: "${CXX:?C++ compiler not found; install it or set CXX/TOOLCHAINS_ROOT}"
BUILD=${BUILD:-$ROOT/build/linux-release}
"$CMAKE" -S "$ROOT" -B "$BUILD" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_C_COMPILER="$CC" -DCMAKE_CXX_COMPILER="$CXX" -DCMAKE_MAKE_PROGRAM="$NINJA" \
  -DIBAUDIO_BUILD_TESTS=ON -DIBAUDIO_BUILD_CLI=ON -DIBAUDIO_ENABLE_VULKAN_PROBE=ON
"$CMAKE" --build "$BUILD" --parallel "${JOBS:-2}"
"$CMAKE" -E env CTEST_OUTPUT_ON_FAILURE=1 "$CMAKE" --build "$BUILD" --target test
python3 "$ROOT/scripts/check_abi.py" "$BUILD/libibaudio.so" "$ROOT/abi/ibaudio_symbols_v1.txt"
