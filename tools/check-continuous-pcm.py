#!/usr/bin/env python3
"""Qualify the shared Rust continuous-input core using synthetic PCM only.

Never opens a physical AO, network service, account, user music or desktop config.
This executable bypasses Qoda's queue/ownership/recovery controllers; native player
tests qualify the adapter separately.
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


def compare(root, example, name, rate, bits, split, gains, flac=False, scale=False,
            target=48000, valid_bits=16, container=16, session=False, seconds=2):
    values = fixture(rate * seconds + 113, bits, rate)
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
    if scale:
        container = 32
    graph = ("aformat=sample_fmts=s32" if scale else
             f"aresample={target}:resampler=soxr:precision=28:cheby=1:osf=s{container}:output_sample_bits={valid_bits}:dither_method=triangular:async=0")
    summary = json.loads(run([str(example), *(["--session"] if session else []), str(manifest), str(capture), graph, str(container),str(target)]))
    if session:
        assert summary["nominal_observations"][-1] == 2, (name,summary)
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
         "-i", str(raw), "-af", graph, "-f", f"s{container}le", str(reference)])
    reference_bytes = reference.read_bytes()
    assert len(actual_bytes) == len(reference_bytes), (name, len(actual_bytes), len(reference_bytes))
    code,width = ("h",2) if container == 16 else ("i",4)
    actual = struct.unpack("<" + code * (len(actual_bytes) // width), actual_bytes)
    expected = struct.unpack("<" + code * (len(reference_bytes) // width), reference_bytes)
    boundary = round(split * target / rate) * 2
    errors = [abs(a - b) for a, b in zip(actual, expected)]
    join = errors[max(0, boundary - 1024):boundary + 1024]
    unit = 2 ** (container-valid_bits)
    assert max(errors) <= 2*unit, (name, max(errors)/unit, max(join)/unit)
    return {"case": name, "frames": len(actual) // 2,"valid_bits":valid_bits,"container_bits":container,
            "join_max_error_valid_lsb": max(join)/unit,
            "whole_max_error_valid_lsb": max(errors)/unit, "ring_high_water_bytes": summary["ring_high_water_bytes"]}


def failures(root, example, session=False):
    label = "session" if session else "sequential"
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
    encoded = root / "truncated.flac"
    run(["ffmpeg", "-nostdin", "-v", "error", "-y", "-i", str(path), str(encoded)])
    payload = encoded.read_bytes()
    encoded.write_bytes(payload[:len(payload)//2])
    cases.append(("truncated-real-flac-payload", [{"path": str(encoded), "frames":48000,"rate":48000,"gain":1}]))
    for name, sources in cases:
        manifest = root / f"{label}-{name}.json"
        manifest.write_text(json.dumps(sources))
        result = subprocess.run([str(example), *(["--session"] if session else []), str(manifest), str(root / f"{label}-{name}.pcm"), GRAPH, "16"],
                                capture_output=True, timeout=8)
        assert result.returncode == 1, (name, result.returncode)
        if name == "truncated-real-flac-payload":
            assert b"Decoder frame count mismatch" in result.stderr, result.stderr.decode(errors="replace")
    return [name for name, _ in cases]


def check(example):
    with tempfile.TemporaryDirectory(prefix="qoda-continuous-pcm-") as directory:
        root = Path(directory)
        results = []
        for mode in [False,True]:
            label="session" if mode else "sequential"
            for rate, bits, flac, split, gains in [
                (44100, 16, False, 44117, (1, 1)),
                (44100, 24, True, 44099, (1, .5011872336272722)),
                (96000, 24, False, 95999, (.5, 1)),
                (96000, 16, True, 96007, (1, 1)),
                (48000, 24, True, 48001, (1, .5)),
            ]:
                name = f"{label}-src-{rate}-{bits}-{'flac' if flac else 'wav'}"
                results.append(compare(root, example, name, rate, bits, split, gains, flac, session=mode))
            for bits in [16, 24]:
                for flac in [False, True]:
                    name = f"{label}-scaling-{bits}-{'flac' if flac else 'wav'}"
                    results.append(compare(root, example, name, 48000, bits, 48013, (1, .5), flac, scale=True, session=mode))
            for rate,bits,target,valid,container,flac in [
                (44100,24,48000,24,32,True),(48000,16,44100,16,16,False),
                (48000,24,96000,24,32,True),(96000,24,44100,16,16,False),
                (96000,16,48000,16,32,True),(96000,24,48000,24,32,True),
                (44100,24,96000,16,16,False),(48000,24,44100,24,32,True),
                (44100,24,44100,16,16,False),(48000,24,48000,16,32,True),
                (96000,24,96000,16,16,False),
            ]:
                name=f"{label}-profile-{rate}-{bits}-to-{target}-{valid}-s{container}"
                results.append(compare(root,example,name,rate,bits,rate*2+17,(1,.5),flac,
                                       target=target,valid_bits=valid,container=container,session=mode,seconds=4))
        print(json.dumps({"pcm_comparisons": results, "rejected_faults":{"sequential":failures(root, example),"session":failures(root,example,True)}}, indent=2))
    print("Synthetic files removed. Queue ownership, seek/recovery and physical output remain unqualified.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--example", type=Path, required=True)
    check(parser.parse_args().example.resolve(strict=True))
