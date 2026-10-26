# Original synthetic ReplayGain fixtures

These files contain only an original 0.25-second, 1 kHz mathematical tone,
48 kHz stereo, licensed under the repository MIT license. They contain no
recorded performance, user library material, credentials or server identifiers.

`manifest.json` records file hashes, byte counts, tool versions and expected tags.
FFmpeg encodes the tone and Mutagen writes tags; no Mutagen implementation is
copied into Qoda. `opus-base.opus` has no ReplayGain tags;
`opus-r128.opus` deliberately conflicts with the qualified Opus policy.

Tests consume the checked-in files and need no network or Mutagen installation.
To regenerate intentionally, provide FFmpeg and Mutagen 1.47.0 in an isolated
Python environment and run `python generate.py` from this directory. Container
identifiers may change across encoder runs; review the regenerated manifest.
The native Opus test captures into a temporary PCM file and never opens speakers.
These fixtures qualify metadata parsing and the named digital capture only;
they make no hardware or general decoder-format fidelity claim.
