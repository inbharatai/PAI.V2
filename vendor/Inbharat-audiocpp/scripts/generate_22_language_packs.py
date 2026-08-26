#!/usr/bin/env python3
"""Generate InBharat's 22 scheduled-language pack catalog.

This script is deliberately data-only and standard-library-only. It does NOT download
models or turn publisher coverage claims into VERIFIED status. Every pack starts with
PENDING task evidence until the same-language benchmark/device gates pass.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKS = ROOT / "packs"

# code, name, primary/accepted scripts, STT candidates, TTS candidates
LANGUAGES = [
    ("as-IN", "Assamese", ["Beng"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indicf5"]),
    ("bn-IN", "Bengali", ["Beng"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indicf5"]),
    ("brx-IN", "Bodo", ["Deva"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indic-parler-tts"]),
    ("doi-IN", "Dogri", ["Deva"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indic-parler-tts"]),
    ("gu-IN", "Gujarati", ["Gujr"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indicf5"]),
    ("hi-IN", "Hindi", ["Deva", "Latn"], ["ai4bharat-indicconformer-600m", "audiocpp-qwen3-asr"], ["ai4bharat-indicf5"]),
    ("kn-IN", "Kannada", ["Knda"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indicf5"]),
    ("ks-IN", "Kashmiri", ["Arab", "Deva"], ["ai4bharat-indicconformer-600m"], ["gaash-bolbosh" ]),
    ("kok-IN", "Konkani", ["Deva", "Latn", "Knda"], ["ai4bharat-indicconformer-600m"], ["springlab-indic-mio-provisional"]),
    ("mai-IN", "Maithili", ["Deva"], ["ai4bharat-indicconformer-600m"], ["springlab-indic-mio-provisional"]),
    ("ml-IN", "Malayalam", ["Mlym"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indicf5"]),
    ("mni-IN", "Manipuri", ["Mtei", "Beng"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indic-parler-tts"]),
    ("mr-IN", "Marathi", ["Deva"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indicf5"]),
    ("ne-IN", "Nepali", ["Deva"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indic-parler-tts"]),
    ("or-IN", "Odia", ["Orya"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indicf5"]),
    ("pa-IN", "Punjabi", ["Guru"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indicf5"]),
    ("sa-IN", "Sanskrit", ["Deva"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indic-parler-tts"]),
    ("sat-IN", "Santali", ["Olck", "Deva", "Beng", "Orya"], ["ai4bharat-indicconformer-600m"], ["springlab-indic-mio-provisional"]),
    ("sd-IN", "Sindhi", ["Arab", "Deva"], ["ai4bharat-indicconformer-600m"], ["springlab-indic-mio-provisional"]),
    ("ta-IN", "Tamil", ["Taml"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indicf5"]),
    ("te-IN", "Telugu", ["Telu"], ["ai4bharat-indicconformer-600m"], ["ai4bharat-indicf5"]),
    ("ur-IN", "Urdu", ["Arab"], ["ai4bharat-indicconformer-600m"], ["springlab-indic-mio-provisional"]),
]

VALIDATION_TEMPLATE = {
    "stt": "PENDING",
    "tts": "PENDING",
    "language_detection": "PENDING",
    "speech_to_speech": "PENDING",
    "windows_physical": "PENDING",
    "android_physical": "PENDING",
}

for code, name, scripts, stt, tts in LANGUAGES:
    directory = PACKS / code
    directory.mkdir(parents=True, exist_ok=True)
    pack = {
        "schema": "inbharat.language-pack.v1",
        "id": f"pack-{code}",
        "language": {"code": code, "name": name, "scheduled_indian_language": True},
        "scripts": scripts,
        "romanized_input": {
            "accepted": "Latn" in scripts or code in {"hi-IN", "kok-IN"},
            "classification_policy": "acoustic_or_user-confirmed",
            "note": "Latin script alone never proves which Indian language was spoken.",
        },
        "normalization": {
            "indian_numbers": True,
            "currency": True,
            "dates_times": "PENDING_TESTS",
            "proper_name_lexicon": "user-approved-local",
        },
        "providers": {
            "stt_candidates": stt,
            "tts_candidates": tts,
            "remote_fallback": "disabled-by-default",
        },
        "deployment": {
            "windows": "candidate",
            "android_arm64": "candidate",
            "model_loading": "lazy-hot-swappable",
            "memory_eviction": "bounded-lru",
        },
        "validation": dict(VALIDATION_TEMPLATE),
        "claim_policy": "A candidate provider is not support. Advertise a task only after same-language benchmark evidence passes.",
    }
    (directory / "pack.json").write_text(json.dumps(pack, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

# Native dependency-free catalog: code<TAB>relative manifest path<TAB>sha256<TAB>scripts.
# The C++ pack registry verifies each manifest against this hash before activation.
rows = ["# inbharat.language-pack-catalog.v1\tlanguage\tmanifest\tsha256\tscripts"]
for code, _name, scripts, _stt, _tts in LANGUAGES:
    manifest = PACKS / code / "pack.json"
    digest = hashlib.sha256(manifest.read_bytes()).hexdigest()
    rows.append(f"{code}\t{code}/pack.json\t{digest}\t{','.join(scripts)}")
(PACKS / "catalog.v1.tsv").write_text("\n".join(rows) + "\n", encoding="utf-8")

print(f"generated {len(LANGUAGES)} scheduled-language packs under {PACKS}")
print(f"wrote hash catalog {PACKS / 'catalog.v1.tsv'}")
