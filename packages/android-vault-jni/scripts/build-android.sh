#!/usr/bin/env bash
# Fail closed: no placeholder .so, no network/lock regeneration during Gradle.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
OUT="${1:?pass generated jniLibs output directory}"
TARGET_DIR="${2:?pass Cargo target directory}"
NDK="${3:?pass Android NDK directory}"
case "$(uname -s)" in Linux) HOST=linux-x86_64 ;; Darwin) HOST=darwin-x86_64 ;; *) echo 'Native vault build requires Linux/macOS NDK host' >&2; exit 1 ;; esac
[[ -f "$NDK/source.properties" ]] || { echo 'NDK missing' >&2; exit 1; }
python3 -c 'import pathlib,sys; s=pathlib.Path(sys.argv[1]).read_text(); assert any(x.strip()=="Pkg.Revision = 27.2.12479018" for x in s.splitlines()), "Expected NDK 27.2.12479018"' "$NDK/source.properties"
BIN="$NDK/toolchains/llvm/prebuilt/$HOST/bin"
[[ -x "$BIN/aarch64-linux-android28-clang" ]] || { echo 'NDK arm64 API28 clang missing' >&2; exit 1; }
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$BIN/aarch64-linux-android28-clang"
export CC_aarch64_linux_android="$CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"
export AR_aarch64_linux_android="$BIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS='-C link-arg=-Wl,-z,max-page-size=16384'
cargo +1.99.0 build --manifest-path "$ROOT/Cargo.toml" --locked --offline -j1 \
  -p unoone-android-vault-jni --release --target aarch64-linux-android --target-dir "$TARGET_DIR"
SO="$TARGET_DIR/aarch64-linux-android/release/libunoone_android_vault_jni.so"
[[ -s "$SO" ]] || { echo 'Native vault shared library missing' >&2; exit 1; }
mkdir -p "$OUT/arm64-v8a"
cp "$SO" "$OUT/arm64-v8a/libunoone_android_vault_jni.so"
