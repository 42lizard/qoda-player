# ReplayGain implementation and measurement scope

The initial production path implements Off, Track, Album and Auto (default), with preamp and missing-data gain fixed at 0 dB. Select the mode in **Menu → Audio settings**. Settings persist privately in `qoda/audio.json` under the XDG configuration directory. Restoration remains paused. Invalid, unsafe or newer settings are preserved and disable gain processing until repaired/restarted; they are not overwritten with guessed preferences.

## Policy and identity

Auto chooses album gain only for an unshuffled, album-derived session whose membership still belongs to the source-scoped album snapshot. The full original album membership is retained even when playback begins with “Play from here.” Matching display titles do not establish album identity. Foreign content or shuffle switches subsequent preparations to track gain; ordinary playlist/track batches never acquire an album origin from their titles.

Album mode (and eligible Auto) falls back from album gain to track gain, then unity. Track mode (and other Auto sessions) falls back to unity. Measured zero remains a valid tagged gain; absence, invalid values and unavailable metadata are distinct from that zero.

Each valid gain uses only its matching peak in the same verified tag family and instance. Duplicate gain or peak fields are unusable. Gain values must be finite and within -120 to +120 dB; peaks must be finite and positive. A valid matching peak caps requested gain at `-20 * log10(peak)` dB. Unknown peaks allow attenuation but cap boosts at unity. No limiter, scan-based normalization or Plex loudness conversion is applied.

## Preparation and processing

Gain is captured with each preparation, including the single speculative gapless successor. Changing settings affects subsequent preparations; it never sends a current-file gain update. A changed setting first persists successfully, then invalidates speculative preparation through the existing buffered-edit barrier. Invalid values, failed saves and unchanged modes do not cancel that successor. A late/uncertain handoff may pause for review rather than modifying an uncertain current file. Explicit reselect/reprepare, including a seek that rebuilds the stream, can apply the new setting now.

Both current loads and appended successors receive per-file `af` options. Non-unity gain uses `lavfi` volume with double precision and a generated finite coefficient; unity explicitly clears the per-file filter. mpv native ReplayGain stays disabled, application volume stays 100, volume-gain stays 0, speed stays 1 and the mpv resampler's dithering is disabled. Metadata cannot inject filter expressions. The [mpv loadfile contract](https://mpv.io/manual/stable/#command-interface-loadfile) defines these options as file-local and restores previous values after playback.

Effective gain, basis and limiting/fallback reason are attached to the native render generation. The full transport and compact surface distinguish prepared/unconfirmed state from nominal playback. The requested setting can differ from an already prepared file's immutable gain.

## Native FLAC metadata reader

The reader accepts native FLAC metadata framing and Vorbis comments, as defined by [RFC 9639](https://www.rfc-editor.org/rfc/rfc9639.html#section-8.6). It reads through the prepared provider-neutral `MediaSource`; credentials and authenticated upstream URLs never enter QML or subprocess arguments.

The reader requests a 64 KiB prefix, then walks metadata headers through the final
block. Large picture, padding and application payloads are skipped using their
validated lengths; only comment bodies and missing headers require further reads.
The first block must be a 34-byte STREAMINFO; repeated STREAMINFO, reserved types,
duplicate comment blocks, invalid lengths and malformed UTF-8 are rejected. A
usable early comment is not accepted until the remaining metadata framing has
been checked. Payload contents of skipped opaque blocks are not decoded/validated.

A scan allows at most 64 blocks, 64 requests, 64 MiB of logical metadata span and
1 MiB of response-body bytes, under the existing five-second tag-read timeout
inside the twenty-second preparation budget. Oversized body chunks cause immediate
fallback rather than being retained or silently truncated. Prefix and comment
storage are bounded by that byte budget; no artwork-sized allocation is made.
Off does not open a tag probe. No decoder, audio device or playback report is
started for metadata extraction.

Sparse reads require HTTP 206 with an exact Content-Range interval (clamped only
at EOF), numeric consistent object length, matching Content-Length when supplied,
and an exact body length. ETags must remain consistent when supplied, including
not disappearing mid-scan. All requests use the same prepared MediaSource/version;
this is not a cryptographic content guarantee for servers that supply no validator.
A server ignoring the initial Range can still supply complete metadata within the
byte budget. An ignored nonzero Range is rejected; the reader never downloads and
discards a cover to simulate seeking.

## Other embedded containers

The same scalar policy and immutable per-file gain path now accept these layouts:

| Container | Qualified metadata path | Explicit exclusions |
| --- | --- | --- |
| MPEG/ADTS and recognized APE-family audio | ID3v2.3/v2.4 TXXX, prefix or one v2.4 footer-delimited appended tag; APEv2 footer at EOF or before ID3v1 | ID3v2.2, encrypted/compressed frames, SEEK/update chains, extended-header CRC/restrictions; conflicting relevant ID3/APE families |
| MP4/M4A | `moov/[udta/]meta/ilst/----`, `mean=com.apple.iTunes`, scalar UTF-8 `data` | `mdta` and other handlers, competing relevant metadata instances, SoundCheck |
| Ogg Vorbis | Complete comment packet with framing byte, checked identification/setup signatures and metadata-page CRCs | Chained/multiplexed streams, ambiguous EOF ownership |
| Ogg Opus | Version 1, mono/stereo mapping 0, zero OpusHead output gain, complete OpusTags with no `R128_` fields | Nonzero header gain, R128 normalization, other mappings/versions or chained/multiplexed streams |
| WAV | Ordinary little-endian RIFF/WAVE with one `ID3 `/`id3 ` chunk, including after audio | RF64/BW64/RIFX, repeated ID3 chunks; INFO/audio text is never searched for gain |

Only the four named ReplayGain scalars are interpreted, case-insensitively. No
reference-level conversion, R128 conversion, BWF loudness or Plex loudness-unit
inference is made. The Opus subset is a deliberate Qoda policy, not a universal
Opus compatibility claim. Native Opus decoding still applies its codec-defined
header gain when present; Qoda declines additional gain for that layout.

ID3 handles Latin-1, BOM-qualified UTF-16 and v2.4 UTF-8/UTF-16BE. Global v2.3
unsynchronization requires the complete tag within the byte budget; v2.4
unsynchronization is decoded per frame. Basic extended headers, grouping and
v2.4 data-length indicators are checked. Malformed late framing invalidates an
early gain. Duplicate/multiple scalar values are unusable; a gain cannot borrow a
peak from another family or instance. Recognizably truncated APE footers fail
rather than establishing absence. Arbitrary unrecognizable tail bytes cannot
establish a missing footer's former contents.

New adapters retain the 64 KiB prefix, 1 MiB transfer, 64-request and five-second
limits. Small sparse header reads reuse one bounded lookahead window; native FLAC
retains its exact sparse-read behavior. Logical metadata spans are capped at
64 MiB; file offsets may pass larger audio payloads to reach tail metadata.
ID3 frames, MP4 atoms and WAV chunks have bounded walkers (1024 each); APE and
comment field counts are bounded at 16384; scalar reads are capped at 64 KiB.
Ogg reassembles at most 64 header pages and requires an unambiguous CRC-valid,
same-serial EOS page ending at EOF within its 64 KiB tail window. A further
bounded walk checks every page header, serial, sequence and continuation, skipping
opaque audio payloads; at most 1024 pages are walked within the same 64-request
and five-second limits. Long or sparse Ogg files can therefore fall back when
that budget is exhausted. Audio payload CRCs are not checked by this metadata
reader. Unsupported/ambiguous layouts fall back explicitly rather than guessing.

Non-FLAC servers ignoring the initial Range must supply the complete bounded
object before tail metadata or absence is accepted. Sparse responses require the
same exact intervals, lengths and validator consistency described above. A read
cancellation drops the pending body. Off still performs no metadata reads.
Partial, interrupted, over-budget and unsupported scans use unity with a static
reason, not a missing-tag claim. Successfully completed scans attach typed source
and per-field `missing`, `invalid`, `duplicate` or `valid` diagnostics to the
render snapshot; tag text and authenticated URLs are not exposed.

See the [primary-source format research](../research/embedded-replaygain.md) and
[original synthetic fixtures](../../tests/fixtures/replaygain/README.md). Plex gain
and loudness fields remain excluded until their units and applicability are
verified in #16.

## Recorded player-side qualification

The production native-player test covers this matrix:

| Dimension | Cases |
| --- | --- |
| Source containers | WAV, FLAC |
| Source precision/layout | Integer 16-bit and 24-bit, stereo |
| Matched source/output rate | 44.1, 48, 96 kHz |
| First-file gain | -6 dB |
| Successor gain | Unity, -12 dB, approximately +6.0206 dB (2x) |
| Capture boundary | mpv `ao=pcm`, signed 32-bit stereo, dithering disabled |
| Acceptance | Exact output sample count and reference order; unity successor samples must match exactly, non-unity gain permits at most one output LSB from double-precision scaling rounded to the signed 32-bit reference |

All 36 compatible transition cases passed locally with mpv 0.41.0 and FFmpeg runtime n9.0.2. FLAC capture fixtures deliberately contain conflicting +12 dB tags, so a native ReplayGain double application would fail the reference comparison. Separate tests read actual FFmpeg-authored FLAC tags through fragmented MediaSource Range responses, check fallback/clipping policy and prove that Off does not probe. Existing gain-off exact WAV/FLAC concatenation tests remain in place.

Checked-in fixtures additionally exercise actual FFmpeg-encoded, Mutagen-tagged
ID3v2.3/v2.4, APEv2, MP4, Vorbis, Opus and WAV metadata. Parser fixtures cover
malformed lengths/late framing, duplicates, invalid scalars, conflicting families,
large skipped artwork/audio, strict Range/ETag checks and cancellation. Native
Opus PCM capture compares tagged-Off output exactly with the untagged baseline;
Track -6 dB permits at most one signed-32-bit output LSB from the reference.
Nonzero header gain and R128 conflicts add no Qoda gain. These checks do not
qualify general chained/multiplexed Ogg or every decoder/container combination.

These are synthetic digital captures at a named PCM-file boundary, for the listed coefficients and profiles. They do not qualify PipeWire routing, direct ALSA negotiation, real-time device continuity, real Plex playback, DAC conversion or analog output. Direct fidelity output is implemented within its documented software scope; final ALSA PCM/profile qualification and broader physical presentation claims from #27 remain unverified and excluded by the accepted first-release boundary. The player does not display a hardware bit-perfect label from these tests.

## Persistence and checks

Session schema/payload 5 retains validated album-origin membership and adds durable visit identity for the [listening archive](../listening-archive.md). Opening schemas 1–4 creates a consistent pre-migration backup before transactional rewrite; old sessions restore without inventing an album origin or archive evidence. Restore accepts supported backups 1–5. Future schemas/payloads are preserved. Gain settings are separate from the session, sign-in and caches; disconnecting does not erase general preferences.

Run `./tools/check.sh` for Clippy, workspace tests and QML input checks. After `cargo build -p qoda-backend --locked`, `python tools/check-surfaces.py` checks real-backend mode changes, invalid-mode rejection, private persistence, UI controls and paused restart with an empty isolated profile. No credentials, Plex requests or real music are needed. `python tools/check-layout.py` checks the production full-window layout offscreen; physical/native input claims from #18 remain unverified and excluded by the accepted first-release boundary.
