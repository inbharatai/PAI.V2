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
BUILD=${BUILD:-$ROOT/build/linux-asan-ubsan}
"$CMAKE" -S "$ROOT" -B "$BUILD" -G Ninja \
  -DCMAKE_BUILD_TYPE=Debug -DCMAKE_C_COMPILER="$CC" -DCMAKE_CXX_COMPILER="$CXX" \
  -DCMAKE_MAKE_PROGRAM="$NINJA" -DIBAUDIO_ENABLE_SANITIZERS=ON \
  -DIBAUDIO_BUILD_TESTS=ON -DIBAUDIO_BUILD_CLI=ON
"$CMAKE" --build "$BUILD" --parallel "${JOBS:-2}"
ASAN_OPTIONS=detect_leaks=1:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 \
  "$CMAKE" -E env CTEST_OUTPUT_ON_FAILURE=1 "$CMAKE" --build "$BUILD" --target test
