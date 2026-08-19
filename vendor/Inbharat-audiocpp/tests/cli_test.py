#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import json
import pathlib
import subprocess
import sys
import tempfile


def run(cli: str, *args: str, expect: int = 0) -> subprocess.CompletedProcess[str]:
    result = subprocess.run([cli, *args], text=True, capture_output=True, check=False)
    if result.returncode != expect:
        raise AssertionError(f"command failed ({result.returncode}): {args}\nstdout={result.stdout}\nstderr={result.stderr}")
    return result


def main() -> int:
    cli = sys.argv[1]
    tests = pathlib.Path(sys.argv[2])
    speech = tests / "fixtures" / "speech_440hz_16k_mono.wav"
    malformed = tests / "fixtures" / "malformed_riff.wav"

    info = json.loads(run(cli, "info", "--json").stdout)
    assert info["schema"] == "inbharat.ibaudio.diagnostics.v1"
    assert info["selected_backend"] == "cpu"
    models = json.loads(run(cli, "models", "--json").stdout)
    assert len(models) == 4 and models[3]["available"] is False

    audio_cpp = json.loads(run(cli, "audio-cpp-status", "--json").stdout)
    assert audio_cpp["schema"] == "inbharat.ibaudio.audio_cpp_status.v1"
    # The adapter is intentionally fail-closed until a real model-family path
    # has passed parity, licensing, cancellation, memory and language gates.
    assert audio_cpp["inference_ready"] is False
    assert audio_cpp["upstream_source"] == "https://github.com/0xShug0/audio.cpp"

    offline = json.loads(run(cli, "asr", "--input", str(speech), "--json").stdout)
    streamed = json.loads(run(cli, "asr", "--input", str(speech), "--stream", "--json").stdout)
    assert offline["transcript"] == streamed["transcript"]
    assert offline["transcript"] == (tests / "expected" / "asr_speech.txt").read_text().strip()

    vad = json.loads(run(cli, "vad", "--input", str(speech), "--json").stdout)
    assert len(vad["segments"]) == 1
    assert vad["segments"][0]["start_frame"] == 3840
    run(cli, "asr", "--input", str(malformed), expect=2)

    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        tts_path = root / "tts.wav"
        tts = json.loads(run(cli, "tts", "--text", "namaste", "--output", str(tts_path), "--json").stdout)
        assert tts["frames"] == 9360
        assert hashlib.sha256(tts_path.read_bytes()).hexdigest() == "e03d0963e9c1012d0738b291d85349461c342004f9aac48266fb3692cd022af6"

        json_path = root / "benchmark.json"
        csv_path = root / "benchmark.csv"
        benchmark = json.loads(run(cli, "benchmark", "--iterations", "2", "--output-json", str(json_path),
                                   "--output-csv", str(csv_path)).stdout)
        assert benchmark["schema"] == "inbharat.ibaudio.benchmark.v1"
        assert len(benchmark["results"]) == 3
        assert json.loads(json_path.read_text())["backend"] == "cpu"
        lines = csv_path.read_text().splitlines()
        assert lines[0] == "schema,runtime_version,backend,operation,iterations,mean_ms"
        assert len(lines) == 4
    print("PASS cli")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
