# Bounded ReplayGain container expansion

Research for #30, 2026-10-26. This is a proposed implementation scope, not a claim
of complete metadata support. No production/tests changes, dependencies, user
credentials, Plex requests or playback were used. The existing Off/no-gain-probe
behavior and gain/peak policy remain authoritative. **Strict direct ALSA source
support stays WAV/native FLAC, stereo integer 16/24-bit, 44.1/48/96 kHz.** Reading
another tag container does not qualify its codec or extend that output scope.

## Native FLAC: first priority

RFC 9639 defines metadata before audio: a first STREAMINFO block, then four-byte
block headers containing last-block/type and a 24-bit big-endian payload length.
Vorbis-comment payloads use little-endian lengths, UTF-8 fields and case-insensitive
ASCII names. Only one Vorbis-comment block is permitted; multiple pictures are
allowed. Type 127 is forbidden. The comment layout differs from Ogg Vorbis's
framed comment packet. [RFC 9639 §§8.1, 8.6, 8.8](https://www.rfc-editor.org/rfc/rfc9639.html#section-8).

Recommendation: walk header offsets and range-read only the comment body, skipping
picture/padding/application payloads instead of downloading them. Continue through
the metadata last-block marker to detect a second comment block; do not declare
success/missing tags from a prefix before unread metadata. Validate arithmetic,
STREAMINFO framing, field count/lengths and UTF-8 before selecting gain. Reject
truncation, forbidden types and duplicate comment blocks explicitly.

For nonzero sparse offsets, require a verified 206/Content-Range matching the
requested interval. HTTP permits ignoring Range and returning 200; that is not a
random-access result. Stop rather than download/discard a large cover to emulate
seeking. Retain validator consistency when supplied; provider media IDs alone
are not a content hash. [HTTP range semantics](https://www.rfc-editor.org/rfc/rfc9110.html#section-14.2).

Suggested initial limits (Qoda policy, not format maxima): five seconds total,
64 requests/blocks, 64 MiB addressable metadata span, at most 1 MiB transferred
and retained metadata, and small bounded individual text values. Count response
bytes and framing requests against the same operation budget. Coalesce adjacent
small reads; reject servers/ranges that cannot satisfy the budget. Covers beyond
the old prefix become readable without claiming arbitrary-size comments support.

## MP3 ID3v2 TXXX: second priority

ID3v2.3 uses a ten-byte tag header with a 28-bit synchsafe tag length; each
frame has a ten-byte header but a normal big-endian 32-bit body size. Text encoding
0 is Latin-1; encoding 1 is BOM-marked 16-bit Unicode. TXXX holds a terminated
description followed by its value. Global unsynchronisation inserts zero bytes
and must be reversed before interpreting the logical frame stream; blindly
walking encoded offsets as unmodified frame sizes is unsafe.
[ID3v2.3 §§3–5](https://id3.org/id3v2.3.0).

ID3v2.4 frame lengths are synchsafe and describe transformed on-wire payloads.
It adds UTF-16BE without BOM (2) and UTF-8 (3), retains Latin-1 (0) and UTF-16
with BOM (1), and has frame unsynchronisation/data-length-indicator flags.
Unsynchronisation is reversed before decryption/decompression. Extended-header
size rules differ from v2.3; a footer is possible.
[ID3v2.4 structure §§3, 4, 6](https://id3.org/id3v2.4.0-structure).
TXXX still separates description from value; general v2.4 text frames can hold
multiple null-separated strings. [ID3v2.4 frames §4.2](https://id3.org/id3v2.4.0-frames).

Picard documents the four gain/peak names as TXXX descriptions, Vorbis/APEv2 keys,
and iTunes freeform names. This is a tagger interoperability mapping, not a claim
that ID3 itself standardizes ReplayGain semantics.
[Picard tag mapping](https://picard-docs.musicbrainz.org/en/latest/appendices/tag_mapping.html#replaygain-track-gain).
Xiph's ReplayGain examples express gain in dB and peak as a separate scalar,
including values above one. [Xiph field convention](https://wiki.xiph.org/VorbisComment#Replay_Gain).

Recommended first ID3 subset: one leading v2.3.0 or v2.4.0 tag, zero tag/format
flags, ordinary ten-byte frames, the encodings valid for that version, and exactly
one scalar per recognized TXXX description. Match the four existing ASCII names
case-insensitively; preserve duplicate occurrences before any normalization.
Scan UTF-16 terminators on code-unit boundaries; validate BOM/endianness and
strings instead of searching arbitrary `00 00` byte windows. Keep the existing
numeric bounds, measured-zero distinction and matching-peak clipping policy.

Apply the same deadline/byte/request/span caps as FLAC, plus a frame-count cap.
Skip bounded APIC/other opaque frames by validated size. Check through the tag end
for duplicates and validate padding within budget; an unread tail is incomplete,
not proof of absence. Initial explicit fallbacks: v2.2, appended/update/multiple
tags, footer-only discovery, tag-wide or frame unsynchronisation, extended headers,
compression, encryption, grouping, data-length indicators, invalid/reserved flags,
malformed sizes/encoding, multi-value gains or over-budget tags. Do not silently
interpret RVA2/RVAD, MP3Gain undo metadata or an APE tail as the same TXXX policy.
These exclusions deliberately trade coverage for a small auditable first parser.

## Ogg Vorbis: separate packet-parser increment

Ogg pages have segment lacing, stream serial/sequence and continuation flags.
A packet can span pages; lacing values of 255 continue a packet, including a final
zero lacing value for exact multiples. The page CRC covers header (checksum field
zeroed) and body using polynomial `0x04c11db7`.
[RFC 3533 §§5–6](https://www.rfc-editor.org/rfc/rfc3533.html#section-6).
Vorbis uses identification/comment/setup header packets; the comment packet is
second, uses length-prefixed UTF-8 vectors and has a required framing bit.
[Xiph comment format](https://xiph.org/vorbis/doc/v-comment.html),
[Vorbis header/bitpacking specification](https://xiph.org/vorbis/doc/Vorbis_I_spec.html).

A future bounded reader should reconstruct only the selected Vorbis header stream,
check CRC/serial/sequence/continuation and signatures, and cap page count, packet
bytes, field count and time. Reuse the scalar-field policy after container framing,
not a byte search for REPLAYGAIN strings. Do not treat a page as a complete packet
or reuse the FLAC payload parser's end rule unchanged. Initially defer multiplexed,
chained, corrupted and oversized header streams. Supporting only first-stream
tags does not establish correct gain across later chained streams; avoiding that
claim is preferable to scanning all audio merely to prove no chain exists.

## Opus: defer gain semantics, not just parsing

RFC 7845's signed 16-bit OpusHead output gain is Q7.8 dB: divide by 256 before
forming the amplitude factor. Decoders should apply it by default. R128 track/
album tags are additional signed integer Q7.8 gains relative to the R128 reference
and header gain; each must be unique and in -32768..32767, with no whitespace.
The RFC discourages mixing REPLAYGAIN gain/peak tags into this scheme.
[RFC 7845 §§5.1, 5.2.1](https://www.rfc-editor.org/rfc/rfc7845.html#section-5.2.1).

Recommendation: keep Opus/R128 unsupported in the new metadata increment.
`replaygain=no` does not prove the selected decoder ignores codec header gain.
Before expansion, qualify nonzero OpusHead gain, Off, R128/256 units, target
reference, track/album fallback and actual decoder output together. Do not add
header gain twice, treat an R128 integer as decimal dB, infer a peak from gain,
or apply a blanket reference offset without a documented policy. Codec-defined
header gain and optional Qoda normalization need separate user-visible semantics.

## WAV, MP4 and APE

| Container | Verified evidence | Initial recommendation |
| --- | --- | --- |
| WAV | FFmpeg accepts `ID3 ` and `id3 ` chunks and passes them to its ID3 reader. RIFF uses sized chunks with word padding. | Follow MP3 only as an explicit finite RIFF/WAVE ID3 wrapper; cap chunk count/span and skip audio data by size. Defer RF64/W64/RIFX and RIFF INFO gain interpretation. |
| MP4 | Picard maps gain/peak to `----:com.apple.iTunes:REPLAYGAIN_*`; Apple documents structured metadata atoms. | Defer atom walking, tail moov discovery and freeform parsing. Do not equate Sound Check/iTunNORM with ReplayGain. |
| APE | Picard maps the four names to APEv2. FFmpeg's reader locates and validates an APETAGEX footer. | Defer tail access, mixed ID3/APE precedence and repeated tags until separately bounded/tested. |

[FFmpeg WAV source, pinned n8.0](https://github.com/FFmpeg/FFmpeg/blob/140fd653aed8cad774f991ba083e2d01e86420c7/libavformat/wavdec.c#L565-L575),
[Microsoft RIFF](https://learn.microsoft.com/en-us/windows/win32/xaudio2/resource-interchange-file-format--riff-),
[Apple metadata atoms](https://developer.apple.com/documentation/quicktime-file-format/metadata_atoms_and_types),
[Picard mappings](https://picard-docs.musicbrainz.org/en/latest/appendices/tag_mapping.html),
[FFmpeg APE reader](https://github.com/FFmpeg/FFmpeg/blob/140fd653aed8cad774f991ba083e2d01e86420c7/libavformat/apetag.c).

## Rust library assessment: no dependency selected

Lofty **0.25.4**, reviewed at crate VCS commit
`618731cee7aac784f5f08643bd5d8d005f3c01d6`, offers `Read + Seek` probing and an MIT
license option. Its FLAC block reader can seek over unrequested payloads, and
ParseOptions disables properties/cover art. These are useful interfaces, not an
end-to-end network/memory bound.
[Manifest/license](https://github.com/Serial-ATA/lofty-rs/blob/618731cee7aac784f5f08643bd5d8d005f3c01d6/lofty/Cargo.toml),
[Probe](https://github.com/Serial-ATA/lofty-rs/blob/618731cee7aac784f5f08643bd5d8d005f3c01d6/lofty/src/probe.rs#L275),
[FLAC block skipping](https://github.com/Serial-ATA/lofty-rs/blob/618731cee7aac784f5f08643bd5d8d005f3c01d6/lofty/src/flac/block.rs#L32-L66),
[ParseOptions](https://docs.rs/lofty/0.25.4/src/lofty/config/parse_options.rs.html).

Its thread-local allocation limit defaults to 16 MiB **per item**, not total parse
memory/I/O. More importantly, its ID3 frame read loop inserts into a FrameList and
replaces duplicates, rather than preserving all raw duplicate evidence for Qoda.
The reviewed loop does this independently of Strict mode.
[Global allocation options](https://github.com/Serial-ATA/lofty-rs/blob/618731cee7aac784f5f08643bd5d8d005f3c01d6/lofty/src/config/global_options.rs#L24-L90),
[duplicate replacement](https://github.com/Serial-ATA/lofty-rs/blob/618731cee7aac784f5f08643bd5d8d005f3c01d6/lofty/src/id3/v2/read.rs#L153-L184).

Therefore it is not established as a bounded drop-in for this contract. Adoption
would need a deadline/request/byte/seek-limited Read+Seek adapter, aggregate memory
accounting and a way to retain duplicate evidence. A timeout around a blocking
parser is not itself cancellation of that parser. Keep the first FLAC/plain-ID3
subset small and async-range-aware instead of assuming a large dependency solves
these boundaries. This research added no dependency and copied no parser code.

## Required evidence for implementation

Use synthetic fixtures/fake range sources: large pictures before/after comments;
second comment block; hostile block/frame lengths; ignored/wrong/truncated ranges;
request/byte/span/time exhaustion; all supported ID3 encodings, aligned UTF-16
terminators, duplicate/mixed-case fields, measured zero and matching peaks;
unsupported flags and partial tag tails; Off with zero gain-related opens. Verify
the existing gain/PCM matrix and strict ALSA source refusals remain unchanged.
Report supported/absent/unreadable/unsupported distinctly; unknown metadata keeps
its explicit unity fallback and does not make an otherwise playable track fail.
