#!/usr/bin/env python3
"""Qualify the isolated Rust continuous-input experiment using synthetic PCM only.

Never opens a physical AO, network service, account, user music or desktop config.
The example is not integrated into Qoda's queue/ownership/recovery controllers.
"""
import argparse
import json
import math
from pathlib import Path
import struct
import subprocess
import tempfile
import wave

GRAPH = "aresample=48000:resampler=soxr:precision=28:cheby=1:osf=s16:output_sample_bits=16:dither_method=triangular:async=0"


def run(command, **kwargs):
    result = subprocess.run(command, capture_output=True, timeout=40, **kwargs)
    if result.returncode:
        raise RuntimeError("Synthetic command failed: " + (result.stdout + result.stderr).decode(errors="replace")[-1500:])
    return result.stdout


def write_wave(path, values, rate, bits):
    with wave.open(str(path), "wb") as output:
        output.setnchannels(2)
        output.setsampwidth(bits // 8)
        output.setframerate(rate)
        output.writeframes(b"".join(value.to_bytes(bits // 8, "little", signed=True)
                                   for pair in values for value in pair))


def fixture(frames, bits, rate):
    amplitude = (2 ** (bits - 1) - 1) * .27
    return [(round(amplitude * math.sin(2 * math.pi * 997 * i / rate)),
             round(amplitude * math.cos(2 * math.pi * 1511 * i / rate)))
            for i in range(frames)]


def compare(root, example, name, rate, bits, split, gains, flac=False, scale=False):
    values = fixture(rate * 2 + 113, bits, rate)
    sources, joined = [], []
    offsets = [0, split, len(values)]
    for index in range(2):
        segment = values[offsets[index]:offsets[index + 1]]
        path = root / f"{name}-{index}.wav"
        write_wave(path, segment, rate, bits)
        if flac:
            encoded = path.with_suffix(".flac")
            run(["ffmpeg", "-nostdin", "-v", "error", "-y", "-i", str(path), str(encoded)])
            path = encoded
        sources.append({"path": str(path), "frames": len(segment), "rate": rate, "gain": gains[index]})
        joined.extend(value / 2 ** (bits - 1) * gains[index] for pair in segment for value in pair)
    manifest = root / f"{name}.json"
    manifest.write_text(json.dumps(sources))
    capture = root / f"{name}.pcm"
    graph = "aformat=sample_fmts=s32" if scale else GRAPH
    summary = json.loads(run([str(example), str(manifest), str(capture), graph, "32" if scale else "16"]))
    assert summary["source_frame_boundaries"] == offsets[1:]
    assert 0 < summary["ring_high_water_bytes"] <= 65536
    actual_bytes = capture.read_bytes()
    if scale:
        actual = struct.unpack("<" + "i" * (len(actual_bytes) // 4), actual_bytes)
        expected = [round(value * 2 ** 31) for value in joined]
        assert actual == tuple(expected), (name, max(abs(a - b) for a, b in zip(actual, expected)))
        return {"case": name, "frames": len(actual) // 2, "max_error_s32_lsb": 0,
                "ring_high_water_bytes": summary["ring_high_water_bytes"]}
    raw = root / f"{name}.f64"
    raw.write_bytes(b"".join(struct.pack("<d", value) for value in joined))
    reference = root / f"{name}-reference.pcm"
    run(["ffmpeg", "-nostdin", "-v", "error", "-y", "-f", "f64le", "-ar", str(rate), "-ac", "2",
         "-i", str(raw), "-af", graph, "-f", "s16le", str(reference)])
    reference_bytes = reference.read_bytes()
    assert len(actual_bytes) == len(reference_bytes), (name, len(actual_bytes), len(reference_bytes))
    actual = struct.unpack("<" + "h" * (len(actual_bytes) // 2), actual_bytes)
    expected = struct.unpack("<" + "h" * (len(reference_bytes) // 2), reference_bytes)
    boundary = round(split * 48000 / rate) * 2
    errors = [abs(a - b) for a, b in zip(actual, expected)]
    join = errors[max(0, boundary - 1024):boundary + 1024]
    assert max(errors) <= 2, (name, max(errors), max(join))
    return {"case": name, "frames": len(actual) // 2, "join_max_error_s16_lsb": max(join),
            "whole_max_error_s16_lsb": max(errors), "ring_high_water_bytes": summary["ring_high_water_bytes"]}


def failures(root, example):
    values = fixture(48000, 16, 48000)
    path = root / "failure-input.wav"
    write_wave(path, values, 48000, 16)
    cases = [
        ("truncated-count", [{"path": str(path), "frames": 48001, "rate": 48000, "gain": 1}]),
        ("excess-count", [{"path": str(path), "frames": 47999, "rate": 48000, "gain": 1}]),
        ("wrong-decoded-rate", [{"path": str(path), "frames": 48000, "rate": 44100, "gain": 1}]),
        ("non-finite-gain", [{"path": str(path), "frames": 48000, "rate": 48000, "gain": "NaN"}]),
        ("changed-rate", [{"path": str(path), "frames": 48000, "rate": 48000, "gain": 1},
                          {"path": str(path), "frames": 48000, "rate": 44100, "gain": 1}]),
    ]
    for name, sources in cases:
        manifest = root / f"{name}.json"
        manifest.write_text(json.dumps(sources))
        result = subprocess.run([str(example), str(manifest), str(root / f"{name}.pcm"), GRAPH, "16"],
                                capture_output=True, timeout=8)
        assert result.returncode == 1, (name, result.returncode)
    return [name for name, _ in cases]


def check(example):
    with tempfile.TemporaryDirectory(prefix="qoda-continuous-pcm-") as directory:
        root = Path(directory)
        results = []
        for rate, bits, flac, split, gains in [
            (44100, 16, False, 44117, (1, 1)),
            (44100, 24, True, 44099, (1, .5011872336272722)),
            (96000, 24, False, 95999, (.5, 1)),
            (96000, 16, True, 96007, (1, 1)),
            (48000, 24, True, 48001, (1, .5)),
        ]:
            name = f"src-{rate}-{bits}-{'flac' if flac else 'wav'}"
            results.append(compare(root, example, name, rate, bits, split, gains, flac))
        for bits in [16, 24]:
            for flac in [False, True]:
                name = f"scaling-{bits}-{'flac' if flac else 'wav'}"
                results.append(compare(root, example, name, 48000, bits, 48013, (1, .5), flac, scale=True))
        print(json.dumps({"pcm_comparisons": results, "rejected_faults": failures(root, example)}, indent=2))
    print("Synthetic files removed. Queue ownership, seek/recovery and physical output remain unqualified.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--example", type=Path, required=True)
    check(parser.parse_args().example.resolve(strict=True))
