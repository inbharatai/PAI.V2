#!/usr/bin/env sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cp "$ROOT/tests/expected/fixture_sha256.json" "$TMP/before.json"
python3 "$ROOT/tests/generate_fixtures.py" >/dev/null
python3 -c 'import pathlib,sys; a=pathlib.Path(sys.argv[1]).read_bytes(); b=pathlib.Path(sys.argv[2]).read_bytes(); raise SystemExit(0 if a == b else 1)' \
  "$TMP/before.json" "$ROOT/tests/expected/fixture_sha256.json"
echo "synthetic fixture regeneration is deterministic"
