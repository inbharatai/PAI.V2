#!/usr/bin/env sh
set -eu
ROOT=${POCKET_AI_ROOT:-$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)}
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
"$SCRIPT_DIR/pi-preflight.sh"
python3 "$SCRIPT_DIR/verify-package.py" "$ROOT"
VAULT_ID="$ROOT/VAULT/identity/vault.id"
[ -s "$VAULT_ID" ] || { echo "FAIL: canonical vault identity missing" >&2; exit 10; }
RUNTIME_CFG="$ROOT/CONFIG/pocket-ai-pi.runtime.v1.json"
[ -s "$RUNTIME_CFG" ] || { echo "FAIL: runtime config missing" >&2; exit 11; }
eval "$(python3 - "$RUNTIME_CFG" <<'PY'
import json,shlex,sys
m=json.load(open(sys.argv[1],encoding='utf-8'))
if m.get('schema')!='inbharat.pocket_ai_pi.runtime.v1':raise SystemExit('bad runtime config schema')
for key,var in [('model_relative_path','MODEL_REL'),('model_sha256','MODEL_SHA256'),('model_id','MODEL_ID'),('default_backend','DEFAULT_BACKEND')]:
    v=str(m.get(key,''));
    if not v:raise SystemExit(f'missing {key}')
    print(f'{var}={shlex.quote(v)}')
PY
)"
MODEL="$ROOT/$MODEL_REL"
case "$(realpath "$MODEL")" in "$ROOT"/*) ;; *) echo "FAIL: model escaped Pocket AI root" >&2; exit 12;; esac
[ -f "$MODEL" ] || { echo "FAIL: model missing" >&2; exit 12; }
ACTUAL=$(sha256sum "$MODEL" | awk '{print $1}')
[ "$ACTUAL" = "$MODEL_SHA256" ] || { echo "FAIL: model SHA-256 mismatch" >&2; exit 13; }
BACKEND=${POCKET_AI_BACKEND:-$DEFAULT_BACKEND}
case "$BACKEND" in
  CPU) LLAMA="$ROOT/RUNTIMES/LINUX-ARM64/LLAMA/CPU/llama-server"; NGL=0;;
  VULKAN) LLAMA="$ROOT/RUNTIMES/LINUX-ARM64/LLAMA/VULKAN/llama-server"; NGL=${POCKET_AI_GPU_LAYERS:--1};;
  *) echo "FAIL: backend must be CPU or VULKAN" >&2; exit 14;;
esac
[ -x "$LLAMA" ] || { echo "FAIL: verified llama-server missing for $BACKEND" >&2; exit 14; }
PORT=$(python3 - <<'PY'
import socket
s=socket.socket();s.bind(('127.0.0.1',0));print(s.getsockname()[1]);s.close()
PY
)
STATE_DIR=${XDG_RUNTIME_DIR:-/tmp}/pocket-ai-$$;mkdir -p "$STATE_DIR";chmod 700 "$STATE_DIR"
cleanup(){
  if [ -n "${USB_WATCH_PID:-}" ]; then kill "$USB_WATCH_PID" 2>/dev/null || true; wait "$USB_WATCH_PID" 2>/dev/null || true; fi
  if [ -n "${LLAMA_PID:-}" ]; then kill "$LLAMA_PID" 2>/dev/null || true; wait "$LLAMA_PID" 2>/dev/null || true; fi
  rm -rf "$STATE_DIR"
}
trap cleanup EXIT INT TERM HUP
"$LLAMA" -m "$MODEL" --host 127.0.0.1 --port "$PORT" -ngl "$NGL" --reasoning off >"$STATE_DIR/llama.log" 2>&1 & LLAMA_PID=$!
python3 "$SCRIPT_DIR/runtime-health.py" --port "$PORT" --expected-model "$MODEL_ID" --timeout "${POCKET_AI_MODEL_START_TIMEOUT:-240}"
HARNESS="$ROOT/RUNTIMES/LINUX-ARM64/HARNESS/inbharat-harness"
IBAUDIO="$ROOT/RUNTIMES/LINUX-ARM64/AUDIO/ibaudio"
AUDIOCPP="$ROOT/RUNTIMES/LINUX-ARM64/AUDIOCPP/audiocpp_cli"
for x in "$HARNESS" "$IBAUDIO" "$AUDIOCPP"; do [ -x "$x" ] || { echo "FAIL: verified runtime missing: $x" >&2; exit 15; }; done
"$HARNESS" info >/dev/null
"$HARNESS" route "hello" | grep -q 'L0' || { echo "FAIL: Harness L0 route smoke failed" >&2; exit 15; }
"$IBAUDIO" info --json >/dev/null
"$AUDIOCPP" --list-loaders >/dev/null
POCKET_AI_SUPERVISOR_PID=$$ "$SCRIPT_DIR/watch-usb.sh" "$ROOT" & USB_WATCH_PID=$!
printf '{"schema":"inbharat.pocket_ai_pi.runtime_state.v1","llama_pid":%s,"llama_port":%s,"model_id":"%s","backend":"%s"}\n' "$LLAMA_PID" "$PORT" "$MODEL_ID" "$BACKEND" > "$STATE_DIR/runtime.json"
echo "PASS: Pocket AI Pi local runtime verified and running on 127.0.0.1:$PORT"
echo "state=$STATE_DIR/runtime.json"
while kill -0 "$LLAMA_PID" 2>/dev/null; do sleep 2; done
echo "FAIL: llama-server exited" >&2; exit 16
