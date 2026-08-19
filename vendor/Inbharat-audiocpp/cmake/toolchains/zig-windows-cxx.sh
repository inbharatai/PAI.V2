#!/usr/bin/env sh
set -eu
zig=${ZIG_EXECUTABLE:-$(command -v zig || true)}
: "${zig:?zig not found; install Zig or set ZIG_EXECUTABLE}"
exec "$zig" c++ -target x86_64-windows-gnu "$@"
