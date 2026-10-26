#!/usr/bin/env python3
"""Compare synthetic playlist/EDL resampling with uninterrupted conversion.

PCM-file output only; no backend, credentials, network or hardware sound device.
Default exits nonzero when the native playlist differs at the join. --expect-reset
instead verifies the currently documented reset while checking the continuous
control. EDL is an architecture experiment, not a production queue replacement.
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


def run(command):
    result = subprocess.run(command, capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError("Synthetic command failed: " + (result.stdout + result.stderr)[-2000:])


def check(expect_reset=False):
    with tempfile.TemporaryDirectory(prefix="qoda-resampler-boundary-") as temporary:
        root = Path(temporary)
        part = b"".join(struct.pack("<hh", sample, sample) for sample in (
            round(32767 * .25 * math.sin(2 * math.pi * 1000 * i / 44100))
            for i in range(88200)))
        paths = []
        for name, pcm in [("first", part), ("second", part), ("whole", part * 2)]:
            path = root / (name + ".wav")
            with wave.open(str(path), "wb") as output:
                output.setnchannels(2); output.setsampwidth(2); output.setframerate(44100)
                output.writeframes(pcm)
            paths.append(path)
        reference = root / "reference.pcm"
        run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(paths[2]),
             "-af", GRAPH, "-f", "s16le", "-c:a", "pcm_s16le", str(reference)])
        expected = list(struct.iter_unpack("<hh", reference.read_bytes()))
        results = {}
        edl = "edl://" + ";".join("%" + str(len(str(path).encode())) + "%" + str(path)
                                  for path in paths[:2]) + ";"
        for mode, inputs in [("single", [str(paths[2])]),
                             ("playlist", list(map(str, paths[:2]))), ("edl_restricted", [edl]), ("edl", [edl])]:
            capture = root / (mode + ".pcm")
            command = ["mpv", "--no-config", "--load-scripts=no", "--ytdl=no",
                 ("--access-references=yes" if mode == "edl" else "--access-references=no"), "--vo=null", "--vid=no", "--ao=pcm",
                 "--ao-pcm-waveheader=no", "--ao-pcm-file=" + str(capture),
                 "--gapless-audio=yes", "--volume=100", "--replaygain=no", "--speed=1",
                 "--audio-format=s16", "--audio-samplerate=48000", "--audio-channels=stereo",
                 "--af=lavfi=[" + GRAPH + "]", "--", *inputs]
            result = subprocess.run(command, capture_output=True, text=True, timeout=30)
            if result.returncode:
                results[mode] = {"error": (result.stdout + result.stderr)[-500:]}
                continue
            actual = list(struct.iter_unpack("<hh", capture.read_bytes()))
            assert len(actual) == len(expected) == 192000, (mode, len(actual), len(expected))
            errors = [abs(actual[index][channel] - expected[index][channel])
                      for index in range(96000 - 512, 96000 + 512) for channel in range(2)]
            results[mode] = {"frames": len(actual), "join_max_error_lsb": max(errors),
                             "join_rms_error_lsb": round(math.sqrt(sum(x*x for x in errors) / len(errors)), 6)}
        print(json.dumps(results, indent=2))
        assert results["single"]["join_max_error_lsb"] <= 2, "Uninterrupted control failed"
        assert results["edl"]["join_max_error_lsb"] <= 2, "EDL experiment did not preserve this format's state"
        if expect_reset:
            assert results["playlist"]["join_max_error_lsb"] > 2, "Documented reset no longer reproduced"
        else:
            assert results["playlist"]["join_max_error_lsb"] <= 2, "Native playlist resets conversion state (#41)"
    print("Synthetic fixtures removed; no hardware sound, credentials, network or desktop changes.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--expect-reset", action="store_true")
    check(parser.parse_args().expect_reset)
