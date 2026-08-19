#!/usr/bin/env python3
"""Generate a deterministic file-level SPDX 2.3 SBOM for the source release."""
from __future__ import annotations

import datetime as dt
import hashlib
import json
import pathlib
import re
import sys
import uuid

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "reports" / "SOURCE_SBOM.spdx.json"
EXCLUDED_TOP = {".git", "build", "out"}


def included(path: pathlib.Path) -> bool:
    relative = path.relative_to(ROOT)
    if relative.parts[0] in EXCLUDED_TOP:
        return False
    if relative == pathlib.Path("reports/SOURCE_SBOM.spdx.json"):
        return False
    return path.is_file() and not path.is_symlink()


def spdx_id(relative: str) -> str:
    return "SPDXRef-File-" + re.sub(r"[^A-Za-z0-9.-]", "-", relative)


def main() -> int:
    files = sorted((path for path in ROOT.rglob("*") if included(path)), key=lambda p: p.as_posix())
    entries = []
    sha1_values = []
    relationships = []
    for path in files:
        content = path.read_bytes()
        relative = path.relative_to(ROOT).as_posix()
        sha1 = hashlib.sha1(content).hexdigest()
        sha256 = hashlib.sha256(content).hexdigest()
        sha1_values.append(sha1)
        identifier = spdx_id(relative)
        entries.append({
            "fileName": "./" + relative,
            "SPDXID": identifier,
            "checksums": [
                {"algorithm": "SHA1", "checksumValue": sha1},
                {"algorithm": "SHA256", "checksumValue": sha256},
            ],
            "licenseConcluded": "Apache-2.0",
            "licenseInfoInFiles": ["Apache-2.0"],
            "copyrightText": "NOASSERTION",
        })
        relationships.append({
            "spdxElementId": "SPDXRef-Package-InBharat-Audio",
            "relationshipType": "CONTAINS",
            "relatedSpdxElement": identifier,
        })
    verification = hashlib.sha1("".join(sorted(sha1_values)).encode("ascii")).hexdigest()
    created = dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    document = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": "InBharat-Audio-0.1.0-rc2-source",
        "documentNamespace": "urn:uuid:" + str(uuid.UUID("159a8cb1-37d0-4e5f-9075-e744b6832711")),
        "creationInfo": {"created": created, "creators": ["Tool: scripts/generate_sbom.py"]},
        "packages": [{
            "name": "InBharat Audio",
            "SPDXID": "SPDXRef-Package-InBharat-Audio",
            "versionInfo": "0.1.0-rc2",
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": True,
            "packageVerificationCode": {"packageVerificationCodeValue": verification},
            "licenseConcluded": "Apache-2.0",
            "licenseDeclared": "Apache-2.0",
            "copyrightText": "NOASSERTION",
            "comment": "Default source release contains no copied audio.cpp source or external model assets."
        }],
        "files": entries,
        "relationships": [
            {"spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES",
             "relatedSpdxElement": "SPDXRef-Package-InBharat-Audio"},
            *relationships,
        ],
    }
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {OUTPUT} with {len(entries)} files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
