# Explicit fidelity path and precision-aware ReplayGain

Qoda defaults to desktop PipeWire playback and offers an explicitly selected direct ALSA fidelity path that refuses unsupported conversion, fixes app volume at unity, and allows selected ReplayGain processing. Because measured mpv integer-output gain uses coarse coefficient steps, ReplayGain uses the qualified high-precision per-file filter path with native gain disabled; gain-off playback bypasses that filter. Configuration intent and verified output are separate states: the contract names the ALSA PCM handoff as its first qualification boundary and makes no unmeasured DAC claim.

The canonical defaults, fallback rules, gain provenance, gapless conditions, and qualification gate are recorded in [Choose verifiable fidelity and ReplayGain guarantees](https://github.com/42lizard/qoda-player/issues/8). No real-device qualification has yet been completed.

## Opt-in HQ conversion amendment (2026-10-26)

Fidelity retains strict exact-format playback by default. The user may explicitly
allow high-quality conversion when exact rate/precision is unavailable. That
policy still prefers an exact supported format, otherwise plans a supported
hardware rate/precision and uses SoXR VHQ (28-bit precision) with TPDF dithering
for integer output. ReplayGain precedes conversion. Rate and precision changes
are shown explicitly and are never labelled sample-perfect. ALSA remains a
direct, guarded hardware path with plugin resampling disabled; the guard checks
the planned output separately from the decoded source. Busy/lost devices never
cause an automatic Desktop fallback or a quality-policy change.

A separate opt-in permits conventional stereo for wholly unreported channel maps.
It does not accept reported reversed/incorrect maps and never claims the assumed
layout is verified. This is needed for the development laptop's driver; merely
allowing resampling does not relax channel-map verification.

## Decoder support amendment (2026-10-28)

The user expanded input support to every audio format decoded by installed mpv,
with optional provider transcoding when local decoding is unsupported. The old
WAV/FLAC header profile now qualifies continuous frame spans only. Silent decoder
inspection supplies PCM rate, representation and channel facts before Fidelity
opens hardware. Strict output still refuses required conversion; HQ fallback also
allows float/integer conversion and stereo mixing, reported explicitly.

A provider-produced representation retains the original media-version and visit
identity. Original selected ReplayGain is retained and applied once; server output
is not assumed to retain original embedded tags. Progressive Plex conversion
prefers FLAC and can use MP3 after an explicit server target refusal. The display
names transcoding and the actual decoded codec. No additional physical output or
unqualified gapless guarantees follow from broader input decoding. See
[format support](../audio/formats.md).
