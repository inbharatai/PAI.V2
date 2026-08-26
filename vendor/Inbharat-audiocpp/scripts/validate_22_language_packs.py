#!/usr/bin/env python3
"""Fail-closed validation for InBharat's 22 scheduled-language pack catalog."""
from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKS = ROOT / "packs"
EXPECTED = {
    "as-IN", "bn-IN", "brx-IN", "doi-IN", "gu-IN", "hi-IN", "kn-IN", "ks-IN",
    "kok-IN", "mai-IN", "ml-IN", "mni-IN", "mr-IN", "ne-IN", "or-IN", "pa-IN",
    "sa-IN", "sat-IN", "sd-IN", "ta-IN", "te-IN", "ur-IN",
}
STATES = {"VERIFIED", "FAILED", "PENDING"}
TASKS = {"stt", "tts", "language_detection", "speech_to_speech", "windows_physical", "android_physical"}
errors: list[str] = []
seen: dict[str, Path] = {}

for path in sorted(PACKS.glob("*/pack.json")):
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:
        errors.append(f"{path}: invalid JSON: {exc}")
        continue
    code = doc.get("language", {}).get("code")
    if code in seen:
        errors.append(f"{path}: duplicate language code {code}; first at {seen[code]}")
    if isinstance(code, str):
        seen[code] = path
    if doc.get("id") != f"pack-{code}":
        errors.append(f"{path}: id/code mismatch")
    if not doc.get("scripts"):
        errors.append(f"{path}: scripts must be non-empty")
    validation = doc.get("validation", {})
    if set(validation) != TASKS:
        errors.append(f"{path}: validation keys must be exactly {sorted(TASKS)}")
    for task, state in validation.items():
        if state not in STATES:
            errors.append(f"{path}: {task} has unknown state {state!r}")
    providers = doc.get("providers", {})
    if not isinstance(providers.get("stt_candidates"), list) or not providers.get("stt_candidates"):
        errors.append(f"{path}: missing STT candidates")
    if not isinstance(providers.get("tts_candidates"), list) or not providers.get("tts_candidates"):
        errors.append(f"{path}: missing TTS candidates")
    # A VERIFIED task needs a separate evidence object. None is allowed to appear by
    # accident: if someone flips a status without evidence, this gate fails.
    verified = [task for task, state in validation.items() if state == "VERIFIED"]
    evidence = doc.get("evidence", {})
    for task in verified:
        item = evidence.get(task)
        if not isinstance(item, dict) or not item.get("report") or not item.get("artifact_sha256"):
            errors.append(f"{path}: VERIFIED {task} lacks report + artifact_sha256 evidence")

actual = set(seen)
missing = EXPECTED - actual
extra = actual - EXPECTED
if missing:
    errors.append(f"missing language packs: {sorted(missing)}")
if extra:
    errors.append(f"unexpected language packs: {sorted(extra)}")
if len(seen) != 22:
    errors.append(f"expected 22 unique packs, found {len(seen)}")

# Verify the native catalog itself and every manifest hash.
catalog = PACKS / "catalog.v1.tsv"
if not catalog.is_file():
    errors.append("missing packs/catalog.v1.tsv")
else:
    catalog_codes: set[str] = set()
    for number, line in enumerate(catalog.read_text(encoding="utf-8").splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) != 4:
            errors.append(f"catalog line {number}: expected 4 tab fields")
            continue
        code, relative, expected_hash, scripts = fields
        catalog_codes.add(code)
        manifest = (PACKS / relative).resolve()
        try:
            manifest.relative_to(PACKS.resolve())
        except ValueError:
            errors.append(f"catalog line {number}: manifest path escapes pack root: {relative}")
            continue
        if not manifest.is_file():
            errors.append(f"catalog line {number}: missing {relative}")
            continue
        actual_hash = hashlib.sha256(manifest.read_bytes()).hexdigest()
        if actual_hash != expected_hash:
            errors.append(f"catalog line {number}: hash mismatch for {relative}")
        if not scripts:
            errors.append(f"catalog line {number}: empty scripts")
    if catalog_codes != EXPECTED:
        errors.append(f"catalog language set mismatch: missing={sorted(EXPECTED-catalog_codes)} extra={sorted(catalog_codes-EXPECTED)}")

if errors:
    for error in errors:
        print("ERROR", error)
    sys.exit(1)
print("PASS language-pack-catalog: 22/22 unique packs; all claims evidence-gated")
