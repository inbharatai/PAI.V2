#!/usr/bin/env sh
set -eu
ROOT=${1:-${POCKET_AI_ROOT:-}}
[ -n "$ROOT" ] || { echo "usage: watch-usb.sh /absolute/pocket-ai-root" >&2; exit 2; }
ROOT=$(realpath "$ROOT")
IDENTITY="$ROOT/VAULT/identity/vault.id"
[ -s "$IDENTITY" ] || { echo "vault identity missing: $IDENTITY" >&2; exit 3; }
while :; do
  if [ ! -d "$ROOT" ] || [ ! -s "$IDENTITY" ]; then
    echo "Pocket AI removed; requesting runtime shutdown" >&2
    [ -n "${POCKET_AI_SUPERVISOR_PID:-}" ] && kill -TERM "$POCKET_AI_SUPERVISOR_PID" 2>/dev/null || true
    exit 0
  fi
  sleep 1
done
