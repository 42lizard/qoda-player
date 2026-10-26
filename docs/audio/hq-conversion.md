# Optional high-quality direct output conversion

In Audio settings, select Fidelity and a direct device, then enable **Allow HQ
conversion when required** and Apply. Apply stays paused; Play/Resume is explicit.
Strict Fidelity remains the default. Existing version-1 output preferences stay
strict; saving writes version 2, including the chosen conversion policy.

The planner first tries exact source rate and adequate precision. If opted in,
it tries precision-preserving alternatives before allowing precision reduction.
Within each phase it prefers the exact rate, then the nearest higher rate, then
lower rates. Output candidates remain 44.1/48/96 kHz, S16_LE or S32_LE stereo.
The current verified-header source scope stays stereo integer 16/24-bit WAV or
native FLAC at 44.1/48/96 kHz. This feature does not add other codec/header readers.
Busy, lost or unidentifiable devices never cause an implicit Desktop fallback.

Converted files use a single libavfilter graph: optional double-precision
ReplayGain, then `aresample` with `resampler=soxr`, `precision=28`, `cheby=1`,
explicit target rate/sample format/valid bits, `dither_method=triangular` and
`async=0`. Exact playback bypasses the conversion filter. The output remains the
selected `hw:` PCM with ALSA plugin resampling disabled. Source and target are
separate descriptors; the submission guard checks the explicitly negotiated
output rate, sample format, valid precision, hardware identity and stereo policy.
There is no automatic change to volume, speed or the ReplayGain clipping policy.
Resampling can create intersample overshoots; it does not add a hidden limiter.

FFmpeg documents precision 28 as SoXR **Very High Quality** and triangular dither
as an available quantization policy. The linked FFmpeg/libmpv build must include
SoXR; filter initialization errors fail playback rather than choosing SWR.
[FFmpeg resampler options](https://ffmpeg.org/ffmpeg-resampler.html).
The graph is validated before load. Per-file `af` values use mpv's outer
length-quoting, so a graph comma cannot split the `loadfile` option map and silently
remove gain/conversion. [mpv escaping and per-file options](https://mpv.io/manual/stable/#escaping-spaces-and-other-special-characters).

## Drivers without channel-map metadata

A separate checkbox, **Assume standard stereo if mapping is unreported**, permits
conventional left/right stereo for a wholly missing/unknown map. It defaults off
and is available only with HQ fallback. Reported incorrect, reversed or partial
maps are still refused. Observed output explicitly distinguishes driver-reported
FL/FR from **stereo assumed; unverified**. No sample-perfect or hardware fidelity
claim is made for converted output or an assumed layout.

This is required by the current built-in speaker driver in addition to rate
conversion. A no-write guarded hardware preflight accepted 44.1 kHz/24-bit source
→ 48 kHz/S16_LE output with the assumption explicitly enabled; strict and
conversion-only attempts refused it. Parameters were installed and closed, but
no audio frames were submitted. This is not a listening or physical-output test.
ALSA's mapping API distinguishes reported positions from missing information;
mpv's ignore-chmap option is limited here to an explicit fixed stereo policy.
[Kernel channel-map API](https://docs.kernel.org/sound/designs/channel-mapping-api.html),
[mpv ALSA output options](https://mpv.io/manual/stable/#audio-output-drivers).

## Gapless scope

An unchanged source format/output plan/gain can be staged through the existing
native queue. Format or conversion-gain changes take the ordinary EOF load path
so the next source can renegotiate and prefer exact playback again. Strict
Fidelity's existing staging behavior is preserved.

mpv drains/resets resampling state per file. Two 44.1→48 kHz files retained the
expected concatenated output-frame count and matched independently converted
reference files at the join. That is not a continuous album-wide resampler: a
sine test found a 35-output-LSB transient relative to an uninterrupted ideal sine
at the file boundary. The same experiment using mpv's internal SoXR resampler
also reset state. Continuous resampler-state handoff is tracked in [#41](https://github.com/42lizard/qoda-player/issues/41);
this increment does not claim that boundary is transparent.

## Validation

The production post-negotiation file pipeline is exercised with generated stereo
WAV fixtures through libmpv `ao=pcm`, never a physical sound device:

- 44.1→48 kHz, 16-bit, 1 kHz tone, including two-file staging and a per-file FFmpeg reference.
- 96→48 kHz, 24→16-bit, 1 kHz tone with −6 dB gain.
- 96→48 kHz, 30 kHz tone to detect alias leakage into the output band.
- 48→48 kHz, silent 24→16-bit conversion to verify nonzero, unbiased TPDF dither.

The measured two-file chain contained exactly 192000 output frames and matched
the per-file reference at the join (0 LSB difference). The 1 kHz/−6 dB case
measured 2903.185 output-LSB RMS; the rejected 30 kHz signal and silent dither
cases each measured approximately 0.498 output-LSB RMS.

Assertions cover output duration, gain within 0.2%, rejected-band RMS below two
16-bit output LSBs, and dither RMS/DC bounds. Pure tests cover strict refusal,
exact preference, precision preference, busy refusal, policy persistence and
incompatible successor refusal. An unavailable-engine test also checks that
filter failure emits no PCM instead of silently using default resampling. QML checks exercise explicit Apply, policy drafts,
and converted/assumed-stereo presentation. Hardware continuity, final ALSA sample
capture, channel order and analog/DAC fidelity remain in issue #27.
