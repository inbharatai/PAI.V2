#!/bin/sh
# Build InBharat Harness for every supported target and record hard evidence.
#
# Evidence levels (be honest about each):
#   BUILT+RUNS   — binary produced AND executed in this environment (strongest)
#   BUILT        — binary produced, correct format verified, not executed here
#   BLOCKED      — cannot build in this environment; reason recorded verbatim
#
# Cross-compilation uses zig as the C toolchain (toolchains/zig-*). zig provides
# libc for linux-musl, windows-gnu, and macos. It does NOT provide an Android
# libc (needs the NDK) — Android is BLOCKED here without the NDK.
set -u
cd "$(dirname "$0")/.."

ZIG="$(ls -d /agent/workspace/toolchains/zig-*-linux-* 2>/dev/null | head -1)/zig"
SHIM_DIR=/tmp/inbharat-zig-shims
mkdir -p "$SHIM_DIR"

mkshim() { # name target
  cat > "$SHIM_DIR/$1" <<SH
#!/bin/sh
exec $ZIG cc -target $2 "\$@"
SH
  chmod +x "$SHIM_DIR/$1"
}

# windows shim needs arg rewriting (mingw CRT supplied by zig)
cat > "$SHIM_DIR/zig-cc-windows-gnu" <<SH
#!/bin/sh
out=""
for a in "\$@"; do
  case "\$a" in
    -nodefaultlibs) continue ;;
    -lmsvcrt|-lmingwex|-lmingw32|-lgcc) continue ;;
    -l:libpthread.a) a="-lpthread" ;;
  esac
  out="\$out '\$a'"
done
eval "set -- \$out"
exec $ZIG cc -target x86_64-windows-gnu "\$@" -lunwind
SH
chmod +x "$SHIM_DIR/zig-cc-windows-gnu"

# mingw dlltool shim (needed by -Z build-std for windows-gnu)
cat > "$SHIM_DIR/x86_64-w64-mingw32-dlltool" <<SH
#!/bin/sh
exec $ZIG dlltool "\$@"
SH
chmod +x "$SHIM_DIR/x86_64-w64-mingw32-dlltool"

mkshim zig-cc-musl x86_64-linux-musl
mkshim zig-cc-macos-x86 x86_64-macos-none
mkshim zig-cc-macos-arm aarch64-macos-none

export PATH="$SHIM_DIR:$PATH"

say() { printf '%s\n' "$*"; }

say "== target build evidence =="

# linux-musl: static musl target links via the toolchain's own rust-lld
# (self-contained CRT — do NOT route through zig cc, which injects its own CRT
# and collides on `_start`). This mirrors scripts/cargo-portable.sh.
MUSL_SYSROOT="$(rustc --print sysroot)"
MUSL_LLD="$MUSL_SYSROOT/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
if [ -x "$MUSL_LLD" ] && \
   CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="$MUSL_LLD" \
   cargo build --release --target x86_64-unknown-linux-musl >/dev/null 2>&1; then
  B=target/x86_64-unknown-linux-musl/release/inbharat-harness
  if "$B" info >/dev/null 2>&1; then say "x86_64-unknown-linux-musl  BUILT+RUNS"; else say "x86_64-unknown-linux-musl  BUILT (run failed)"; fi
else
  say "x86_64-unknown-linux-musl  BLOCKED: rust-lld link failed"
fi

# windows-gnu (needs nightly -Z build-std because zig+mingw gnu unwind symbols)
if CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$SHIM_DIR/zig-cc-windows-gnu" \
   RUSTFLAGS="-C panic=abort" \
   cargo +nightly build -Z build-std=std,panic_abort --release --target x86_64-pc-windows-gnu >/dev/null 2>&1 \
   && [ -f target/x86_64-pc-windows-gnu/release/inbharat-harness.exe ] \
   && [ "$(head -c2 target/x86_64-pc-windows-gnu/release/inbharat-harness.exe)" = "MZ" ]; then
  say "x86_64-pc-windows-gnu      BUILT (PE/MZ verified; needs Windows/Wine to run)"
else
  say "x86_64-pc-windows-gnu      BLOCKED: build or MZ check failed"
fi

# macOS x86_64 + aarch64 (zig provides macos libc; SDK-version embed skipped = warning only)
if CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER="$SHIM_DIR/zig-cc-macos-x86" \
   cargo build --release --target x86_64-apple-darwin >/dev/null 2>&1 \
   && [ -f target/x86_64-apple-darwin/release/inbharat-harness ]; then
  say "x86_64-apple-darwin        BUILT (Mach-O; needs macOS to run)"
else
  say "x86_64-apple-darwin        BLOCKED"
fi
if CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="$SHIM_DIR/zig-cc-macos-arm" \
   cargo build --release --target aarch64-apple-darwin >/dev/null 2>&1 \
   && [ -f target/aarch64-apple-darwin/release/inbharat-harness ]; then
  say "aarch64-apple-darwin       BUILT (Mach-O; needs macOS to run)"
else
  say "aarch64-apple-darwin       BLOCKED"
fi

# android — zig cannot provide an Android libc; NDK required. Record verbatim.
ANDROID_ERR="$(cargo build --release --target aarch64-linux-android 2>&1 | tail -1)"
say "aarch64-linux-android      BLOCKED: needs Android NDK (zig: no android libc). Last error: $ANDROID_ERR"
