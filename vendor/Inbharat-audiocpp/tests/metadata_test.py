#!/usr/bin/env python3
from __future__ import annotations

import json
import pathlib
import subprocess
import sys


def main() -> int:
    cli = pathlib.Path(sys.argv[1])
    root = pathlib.Path(sys.argv[2])
    compiled = json.loads(subprocess.check_output([str(cli), "models", "--json"], text=True))
    registry = json.loads((root / "models" / "registry.v1.json").read_text())["models"]
    licenses = json.loads((root / "licenses" / "MODEL_LICENSES.json").read_text())["entries"]
    assert [entry["id"] for entry in registry] == [entry["id"] for entry in compiled]
    by_id = {entry["id"]: entry for entry in registry}
    license_by_id = {entry["model_id"]: entry for entry in licenses}
    for descriptor in compiled:
        entry = by_id[descriptor["id"]]
        license_entry = license_by_id[descriptor["id"]]
        assert descriptor["sha256"] == entry["artifact_sha256"] == license_entry["sha256"]
        assert descriptor["spdx_license"] == entry["spdx_license"] == license_entry["spdx_license"]
        assert descriptor["available"] == entry["available"]
        assert descriptor["streaming_label"] == entry["streaming_label"]
    print("PASS metadata")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
