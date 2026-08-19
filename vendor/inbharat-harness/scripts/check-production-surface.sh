#!/usr/bin/env sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
# This is a source gate; the authoritative binary gate is cargo build without
# --features test-providers followed by nm/strings checks on the release artifact.
grep -q 'default = \[\]' "$ROOT/crates/core/Cargo.toml"
grep -q 'test-providers = \[\]' "$ROOT/crates/core/Cargo.toml"
python3 - "$ROOT/crates/core/src/lib.rs" <<'PY'
from pathlib import Path
import sys
s=Path(sys.argv[1]).read_text()
line='pub use providers::{EchoModelProvider, MockModelProvider, MockStep};'
pos=s.find(line)
if pos < 0:
    raise SystemExit('FAIL: synthetic provider export gate missing')
prefix=s[max(0,pos-100):pos]
if '#[cfg(any(test, feature = \"test-providers\"))]' not in prefix:
    raise SystemExit('FAIL: synthetic providers are exported without test-provider cfg')
PY
echo 'PASS: Harness production source surface excludes synthetic model providers by default'
