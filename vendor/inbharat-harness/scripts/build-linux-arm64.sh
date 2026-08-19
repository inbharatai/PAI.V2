#!/usr/bin/env sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TARGET=${INBHARAT_ARM64_TARGET:-aarch64-unknown-linux-gnu}
CARGO_BIN=${CARGO:-cargo}
command -v "$CARGO_BIN" >/dev/null 2>&1 || { echo "cargo not found" >&2; exit 69; }
"$CARGO_BIN" rustc -Vv >/dev/null
"$CARGO_BIN" target list --installed | grep -qx "$TARGET" || {
  echo "Rust target $TARGET is not installed. Run: rustup target add $TARGET" >&2
  exit 70
}
if [ "$TARGET" = "aarch64-unknown-linux-gnu" ] && [ "$(uname -m)" != "aarch64" ]; then
  command -v aarch64-linux-gnu-gcc >/dev/null 2>&1 || {
    echo "aarch64-linux-gnu-gcc is required for cross-linking" >&2; exit 71;
  }
  export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER:-aarch64-linux-gnu-gcc}
fi
cd "$ROOT"
"$CARGO_BIN" fmt --all -- --check
"$CARGO_BIN" test -p inbharat-harness-core --all-features
"$CARGO_BIN" build --locked --release --target "$TARGET" -p inbharat-harness-cli -p inbharat-harness-ffi
CLI="$ROOT/target/$TARGET/release/inbharat-harness"
LIB_SO="$ROOT/target/$TARGET/release/libinbharat_harness.so"
LIB_A="$ROOT/target/$TARGET/release/libinbharat_harness.a"
[ -x "$CLI" ] || { echo "missing ARM64 harness CLI: $CLI" >&2; exit 72; }
file "$CLI" | grep -Eqi 'ARM aarch64|aarch64' || { echo "output is not AArch64" >&2; file "$CLI" >&2; exit 73; }
mkdir -p "$ROOT/dist/linux-arm64"
cp "$CLI" "$ROOT/dist/linux-arm64/inbharat-harness"
[ -f "$LIB_SO" ] && cp "$LIB_SO" "$ROOT/dist/linux-arm64/"
[ -f "$LIB_A" ] && cp "$LIB_A" "$ROOT/dist/linux-arm64/"
sha256sum "$ROOT/dist/linux-arm64/"* > "$ROOT/dist/linux-arm64/SHA256SUMS"
echo "PASS: InBharat Harness Linux ARM64 artifacts are in dist/linux-arm64"
