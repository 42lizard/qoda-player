# Audio formats and Plex conversion

Qoda attempts original media through the installed mpv/libmpv. Input support follows
that decoder build, including AAC/MP4, MP3, ALAC, FLAC, Vorbis, Opus and integer or
floating-point PCM. File extensions and Plex codec names do not decide whether a
track may play. Embedded references remain disabled: a provider track cannot grant
access to arbitrary external resources.

Desktop playback uses the desktop audio path. Fidelity inspects the decoded input
silently on null output before opening the selected ALSA device. The reported PCM
bits describe decoder representation; they do not invent an encoded integer bit
depth for lossy or float audio. Independently verified WAV/FLAC header precision can
refine an integer container description when it agrees with the decoder.

Strict Fidelity refuses required conversion to the current stereo integer hardware
path. Enable HQ fallback to permit explicit high-quality rate conversion,
float-to-integer/precision conversion and stereo mixing. The processing display
reports the source and target, with SoXR VHQ and TPDF dither when conversion is
needed. An unsupported output route never silently changes to Desktop.

## Plex fallback

A locally unrecognized format or unavailable decoder can trigger one bounded
server-conversion attempt for that native load. An unreadable header can also be
reported by mpv as unknown format; the server may still refuse it. Generic decode
errors, authentication/access failures, network interruptions and output-device
failures do not select transcoding. Failure of converted playback ends the attempt;
there is no format-fallback loop.

Plex conversion is optional. Qoda verifies that the server still exposes the pinned
original version, negotiates a progressive HTTP audio target and prefers FLAC.
If the server explicitly refuses that target, it tries MP3. Servers that cannot
provide either target report that conversion is unsupported; manifests are not
passed directly to mpv with server credentials. The display labels **Plex
transcoded** and the codec actually reported by the decoder, so a requested
lossless target is not itself evidence of lossless delivered output.

Original selected ReplayGain is retained across fallback and applied once. Server
output tags are not treated as original embedded metadata. Queue occurrence and
playback visit are retained, as are pause intent and pinned recovery. Seeking and recovery ask Plex to start a new representation at the requested
source time; the backend maps the decoder-local clock to that source offset only
after positive clock evidence. This uses Plex’s seek contract rather than claiming
byte-exact frame alignment in a transcoded stream. Each media
read has a private server session; abandoned bodies, grant revocation and shutdown
retire those sessions. Cleanup requests have finite deadlines; a disconnected
server can delay its own resource release.

## Continuity and evidence

The existing qualified continuous-conversion profile remains stereo integer
16/24-bit native WAV/FLAC at 44.1/48/96 kHz with verified frame spans and sufficient
track length. Other input can play through the ordinary native path, with explicit
independent-conversion fallback where continuous spans are unavailable. Server
conversion has no new gapless guarantee.

Native tests exercise representative compressed, float, mono, multichannel and
unusual-rate input, plus strict/HQ planning and cancellation. These do not measure
DAC output. Broader codec support does not broaden the named-device physical
qualification claim.

The initial live development probe on 2026-10-28 confirmed an owned Plex server's
progressive FLAC decision/start response and explicit session stop. This is evidence
for that server path, not all Plex versions, permissions or hardware.

Implementation references: [mpv decoder/output properties](https://mpv.io/manual/stable/#properties)
and [Plex transcode targets and lifecycle](https://developer.plex.tv/pms/).
