#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
"$root/scripts/cargo-portable.sh" build --workspace
if command -v cc >/dev/null 2>&1; then
  target_dir="$root/target/debug"
else
  target_dir="$root/target/x86_64-unknown-linux-musl/debug"
fi
bin="$target_dir/inbharat-harness"
"$bin" info
"$bin" route "hello"
"$bin" route "read file README.md"
"$bin" route "research and compare two options"
"$bin" route --workspace "build a complete website in the workspace and test it"
"$bin" run-task "hello"
"$bin" run-task "research and compare two bounded options"
"$bin" run-task --workspace --yes "build a complete verified result in the workspace"
rm -rf "$root/target/smoke-work"
mkdir -p "$root/target/smoke-work"
if "$bin" run-task --root "$root/target/smoke-work" "write file denied.txt blocked" >/dev/null 2>&1; then
  echo "secure-default write unexpectedly succeeded" >&2
  exit 1
fi
"$bin" run-task --root "$root/target/smoke-work" --allow-write --yes "write file allowed.txt approved"
[ -f "$root/target/smoke-work/allowed.txt" ]
[ ! -e "$root/target/smoke-work/denied.txt" ]
"$bin" run-task --root "$root/target/smoke-work" --allow-process --allow-program sleep --trusted-process --yes "run command sleep 0"
rm -rf "$root/target/smoke-sessions"
session_output=$("$bin" run-task --session-dir "$root/target/smoke-sessions" "hello session")
session_log=${session_output##*session_log=}
session_id=${session_log##*/}
session_id=${session_id%.jsonl}
"$bin" run-task --session-dir "$root/target/smoke-sessions" --resume "$session_id" "hello resumed"
printf '/quit\n' | "$bin" chat
"$bin" demo-website --output target/smoke-demo --force
[ -f "$root/target/smoke-demo/index.html" ]
[ -f "$root/target/smoke-demo/style.css" ]
"$bin" benchmark --iterations 1000
