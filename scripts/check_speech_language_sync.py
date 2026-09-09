#!/usr/bin/env python3
"""Cross-platform speech language table sync check.

The PAI speech language contract has exactly one source of truth:
``packages/speech-contracts/languages.v1.json`` (embedded by the Rust crate
via ``include_str!`` and read here directly). The Android mirror inside
``VoiceLanguage.kt`` (``CANONICAL_ALIASES``) must contain the very same
alias -> canonical mapping. If the two drift, the same user input means
different languages on desktop and Android — a silent cross-platform
contract break, not a cosmetic difference.

This script fails CI when either side changes without the other.

Usage:
    python3 scripts/check_speech_language_sync.py
Exit code 0 = in sync, 1 = drift (with a printed diff).
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
JSON_PATH = ROOT / "packages" / "speech-contracts" / "languages.v1.json"
RUST_CRATE_MARKER = ROOT / "packages" / "speech-contracts" / "src" / "lib.rs"
KOTLIN_PATH = (
    ROOT
    / "android-app"
    / "UnoOneAgent"
    / "voice"
    / "src"
    / "main"
    / "java"
    / "com"
    / "unoone"
    / "agent"
    / "voice"
    / "VoiceLanguage.kt"
)

BEGIN_MARKER = "// canonical-aliases-begin"
END_MARKER = "// canonical-aliases-end"


def json_aliases() -> dict[str, str]:
    table = json.loads(JSON_PATH.read_text(encoding="utf-8"))
    if table.get("schema") != "inbharat.speech.languages.v1":
        raise SystemExit(f"FAIL: unexpected schema in {JSON_PATH}: {table.get('schema')!r}")
    # Keys are compared case-SENSITIVELY: the table deliberately lists case
    # variants of one alias (`as-in` and `as-IN`); they must agree on the
    # target, which the variant check below enforces.
    return {key: value for key, value in table["aliases"].items()}


def kotlin_aliases() -> dict[str, str]:
    source = KOTLIN_PATH.read_text(encoding="utf-8")
    match = re.search(
        re.escape(BEGIN_MARKER) + r"(.*?)" + re.escape(END_MARKER), source, re.DOTALL
    )
    if match is None:
        raise SystemExit(
            f"FAIL: {KOTLIN_PATH} has no '{BEGIN_MARKER} ... {END_MARKER}' block.\n"
            "The Kotlin mirror of languages.v1.json is missing or was moved; "
            "restore the CANONICAL_ALIASES block so the sync check can see it."
        )
    entries = re.findall(r'"([^"]+)"\s+to\s+"([^"]+)"', match.group(1))
    aliases: dict[str, str] = {}
    for key, value in entries:
        if key in aliases:
            raise SystemExit(f"FAIL: duplicate Kotlin alias key '{key}'")
        aliases[key] = value
    return aliases


def main() -> int:
    if not JSON_PATH.is_file():
        raise SystemExit(f"FAIL: language table missing at {JSON_PATH}")
    if not RUST_CRATE_MARKER.is_file():
        raise SystemExit("FAIL: packages/speech-contracts crate is missing its lib.rs")

    expected = json_aliases()
    actual = kotlin_aliases()

    failures: list[str] = []

    only_json = sorted(set(expected) - set(actual))
    only_kotlin = sorted(set(actual) - set(expected))
    if only_json:
        failures.append(f"aliases present in languages.v1.json but missing from VoiceLanguage.kt: {only_json}")
    if only_kotlin:
        failures.append(f"aliases present in VoiceLanguage.kt but missing from languages.v1.json: {only_kotlin}")
    for key in sorted(set(expected) & set(actual)):
        if expected[key] != actual[key]:
            failures.append(
                f"alias '{key}' maps to '{expected[key]}' in JSON but '{actual[key]}' in Kotlin"
            )

    # Structural invariants the Rust tests also enforce; repeated here so a
    # single-sided edit that keeps counts balanced still cannot rot the table.
    for key, target in sorted(expected.items()):
        if target == "auto":
            if key != "auto":
                failures.append(f"only the reserved 'auto' sentinel may map to 'auto' (found '{key}')")
            continue
        # Every alias target must be an idempotent entry: some key equal to the
        # target (ignoring case, since both spellings are listed) maps to itself.
        idempotent = any(
            k.lower() == target.lower() and v == target for k, v in expected.items()
        )
        if not idempotent:
            failures.append(
                f"alias '{key}' targets '{target}' which is not an idempotent table entry"
            )
    # Case variants of one alias (as-in / as-IN) must agree on the target.
    grouped: dict[str, set[str]] = {}
    for key, target in expected.items():
        grouped.setdefault(key.lower(), set()).add(target)
    for lowered, targets in sorted(grouped.items()):
        if len(targets) > 1:
            failures.append(f"case variants of '{lowered}' disagree on targets: {sorted(targets)}")

    if failures:
        print("FAIL: speech language tables are out of sync.")
        for failure in failures:
            print(f"  - {failure}")
        print()
        print("Fix: edit packages/speech-contracts/languages.v1.json and the "
              "CANONICAL_ALIASES block in VoiceLanguage.kt together, in the same commit.")
        return 1

    print(f"PASS: Kotlin mirror == languages.v1.json ({len(expected)} aliases, both directions).")
    return 0


if __name__ == "__main__":
    sys.exit(main())