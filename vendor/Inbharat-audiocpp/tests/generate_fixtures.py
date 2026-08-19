#!/usr/bin/env python3
"""Generate reviewed, synthetic deterministic WAV fixtures (no external assets)."""
from __future__ import annotations

import hashlib
import json
import math
import pathlib
import struct
import wave

ROOT = pathlib.Path(__file__).resolve().parent
FIXTURES = ROOT / "fixtures"
EXPECTED = ROOT / "expected"
FIXTURES.mkdir(parents=True, exist_ok=True)
EXPECTED.mkdir(parents=True, exist_ok=True)


def pcm16(value: float) -> bytes:
    value = max(-1.0, min(1.0, value))
    integer = -32768 if value <= -1.0 else round(value * 32767.0)
    return struct.pack("<h", integer)


def write_wav(path: pathlib.Path, rate: int, channels: int, frames: list[tuple[float, ...]]) -> None:
    with wave.open(str(path), "wb") as output:
        output.setnchannels(channels)
        output.setsampwidth(2)
        output.setframerate(rate)
        output.writeframes(b"".join(pcm16(sample) for frame in frames for sample in frame))


# 250 ms silence + 1 s 440 Hz sine + 250 ms silence.
frames_16k: list[tuple[float, ...]] = []
for index in range(24_000):
    value = 0.0
    if 4_000 <= index < 20_000:
        value = 0.25 * math.sin(2.0 * math.pi * 440.0 * (index - 4_000) / 16_000.0)
    frames_16k.append((value,))
write_wav(FIXTURES / "speech_440hz_16k_mono.wav", 16_000, 1, frames_16k)

# Stereo/channel-conversion fixture: opposite amplitudes intentionally do not cancel fully.
frames_48k: list[tuple[float, ...]] = []
for index in range(24_000):
    left = 0.3 * math.sin(2.0 * math.pi * 300.0 * index / 48_000.0)
    right = 0.1 * math.sin(2.0 * math.pi * 300.0 * index / 48_000.0)
    frames_48k.append((left, right))
write_wav(FIXTURES / "stereo_48k.wav", 48_000, 2, frames_48k)

write_wav(FIXTURES / "silence_16k.wav", 16_000, 1, [(0.0,)] * 8_000)

# Small malformed corpus. Every file is generated locally and expected to be rejected.
(FIXTURES / "malformed_empty.wav").write_bytes(b"")
(FIXTURES / "malformed_riff.wav").write_bytes(b"RIFF\xff\xff\xff\xffWAVEfmt ")
(FIXTURES / "malformed_truncated_data.wav").write_bytes(
    b"RIFF" + struct.pack("<I", 100) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, 16000, 32000, 2, 16)
    + b"data" + struct.pack("<I", 64) + b"\x00\x00"
)
(FIXTURES / "malformed_bad_align.wav").write_bytes(
    b"RIFF" + struct.pack("<I", 38) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 2, 16000, 64000, 3, 16)
    + b"data" + struct.pack("<I", 2) + b"\x00\x00"
)

hashes = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(FIXTURES.glob("*.wav"))}
(EXPECTED / "fixture_sha256.json").write_text(json.dumps(hashes, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print(json.dumps(hashes, indent=2, sort_keys=True))
