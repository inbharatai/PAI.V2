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
    # The registry is the default (dependency-free) build's catalog. The audio.cpp adapter
    # build adds the Silero VAD and Qwen3-ASR models at runtime; they are provider-supplied
    # entries documented in MODEL_PROVENANCE.md / MODEL_LICENSES.json, not registry lines.
    # Exclude them from the registry order comparison, but still verify their license entry.
    adapter_only = {"audiocpp-silero-vad-v1", "audiocpp-qwen3-asr-v1"}
    compiled_core = [entry for entry in compiled if entry["id"] not in adapter_only]
    assert [entry["id"] for entry in registry] == [entry["id"] for entry in compiled_core]
    by_id = {entry["id"]: entry for entry in registry}
    license_by_id = {entry["model_id"]: entry for entry in licenses}
    for descriptor in compiled_core:
        entry = by_id[descriptor["id"]]
        license_entry = license_by_id[descriptor["id"]]
        assert descriptor["sha256"] == entry["artifact_sha256"] == license_entry["sha256"]
        assert descriptor["spdx_license"] == entry["spdx_license"] == license_entry["spdx_license"]
        assert descriptor["available"] == entry["available"]
        assert descriptor["streaming_label"] == entry["streaming_label"]
    # Adapter-supplied models must have a license entry with a pinned sha256 (the literal,
    # auditable hash). They are not in the registry, so check the license map directly.
    for descriptor in compiled:
        if descriptor["id"] in adapter_only:
            assert descriptor["id"] in license_by_id, descriptor["id"] + " missing license entry"
            assert len(license_by_id[descriptor["id"]]["sha256"]) == 64
    print("PASS metadata")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
