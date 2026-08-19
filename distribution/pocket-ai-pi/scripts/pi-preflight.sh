#!/usr/bin/env sh
set -eu
fail(){ echo "FAIL: $*" >&2; exit 1; }
[ "$(uname -s)" = "Linux" ] || fail "Pocket AI Pi requires Linux"
case "$(uname -m)" in aarch64|arm64) ;; *) fail "64-bit ARM required; got $(uname -m)";; esac
[ -r /proc/meminfo ] || fail "/proc/meminfo unavailable"
mem_kb=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
[ -n "$mem_kb" ] || fail "cannot determine RAM"
python3 - <<'PY' || exit 1
import platform,sys
parts=[]
for x in platform.release().split('-')[0].split('.')[:2]:
    try: parts.append(int(x))
    except ValueError: parts.append(0)
if tuple(parts)<(6,1):
    print(f"FAIL: Linux kernel >= 6.1 required; got {platform.release()}",file=sys.stderr);sys.exit(1)
PY
command -v python3 >/dev/null 2>&1 || fail "python3 required for package verification"
command -v sha256sum >/dev/null 2>&1 || fail "sha256sum required"
echo "arch=$(uname -m)"
echo "kernel=$(uname -r)"
echo "mem_total_mib=$((mem_kb/1024))"
if [ -r /sys/class/thermal/thermal_zone0/temp ]; then
  t=$(cat /sys/class/thermal/thermal_zone0/temp 2>/dev/null || echo 0)
  echo "cpu_temp_c=$(awk "BEGIN{printf \"%.1f\", $t/1000}")"
fi
if command -v vcgencmd >/dev/null 2>&1; then
  vcgencmd get_throttled 2>/dev/null || true
fi
echo "PASS: Pocket AI Pi preflight"
