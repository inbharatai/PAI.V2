#!/usr/bin/env sh
set -eu
zig=${ZIG_EXECUTABLE:-$(command -v zig || true)}
: "${zig:?zig not found; install Zig or set ZIG_EXECUTABLE}"
exec "$zig" ar "$@"
