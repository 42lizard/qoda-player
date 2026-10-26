# Bounded embedded ReplayGain extraction

Researched 2026-10-26 for #30. Public format specifications and pinned upstream
reader/writer sources only. No copied implementation, server/credential access,
playback or installation. This describes storage framing, not a new loudness
algorithm, reference-level conversion, or Plex gain-unit interpretation.

## Shared value and provenance contract

The four names are `REPLAYGAIN_TRACK_GAIN`, `REPLAYGAIN_TRACK_PEAK`,
`REPLAYGAIN_ALBUM_GAIN`, `REPLAYGAIN_ALBUM_PEAK`. Text gains express dB; peaks are
dimensionless, with 1 representing full scale and possible values above 1.
TXXX descriptions, Vorbis-comment keys and APE text keys carry these names;
case variants and ordinary numeric-format differences occur. The currently
published revised ReplayGain proposal is explicitly under construction; do not
silently adopt its algorithm/reference-level revisions merely to read tags.
[Published metadata conventions](https://wiki.hydrogenaudio.org/index.php?title=ReplayGain_2.0_specification#Metadata_format),
[Xiph ReplayGain field examples](https://wiki.xiph.org/VorbisComment#Replay_Gain).

**Qoda policy recommendation:** retain one complete tag-family/instance as the
provenance of a gain and its matching peak. Never pool an ID3 gain with an APE
peak, MP4 namespaces, different ID3 tags, or different Ogg logical streams. Apply
existing finite-value, range and duplicate-field rejection to each group. A
single invalid field is different from a malformed/truncated container; a partial
scan cannot establish that other relevant fields/duplicates are absent. Declare
whole-tag precedence or reject conflicting families; container specifications do
not establish one universal cross-system precedence. Do not interpret SoundCheck,
RVA2/RVAD/RGAD, BWF loudness, OpusHead gain, R128 fields or reference-loudness tags
as these four values without a separate policy and evidence.

## ID3v2.3

- Header: ten bytes, `ID3`, major/revision, flags, four-byte synchsafe28 body
  length. Each length byte has bit 7 clear. Body includes extended header and
  padding, excludes the ten-byte tag header. Header flags: `0x80` global
  unsynchronization, `0x40` extended header, `0x20` experimental; other bits unused.
- Frames: ten-byte header: four-character `[A-Z0-9]` ID, **ordinary BE32** payload
  length, two flag bytes. Payload length excludes frame header. Padding is zero.
- Extended-header BE32 length **excludes its own four bytes**: normally 6, or 10
  with CRC. Then flags BE16 and padding length BE32; optional CRC is four bytes.
- Global unsynchronization changes stored versus logical offsets. Invert it
  across the tag body before using logical frame lengths. Sparse jumping over a
  transformed payload by its frame length is unsafe without a translation scan.
- Encoding byte 0 means Latin-1; 1 means 16-bit Unicode with BOM. Single-byte
  text terminates with `00`, Unicode with aligned `00 00`. Frame-format flags
  include compression, encryption and one-byte grouping identity.

[ID3v2.3 specification, §§3–5](https://id3.org/id3v2.3.0).

Recommended bounded scope: a globally unsynchronized tag must fit the complete
read/decode budget, otherwise explicit fallback. Do not make an artwork-sized
allocation or pretend sparse offsets remain unchanged. Unsupported compression,
encryption or unknown format flags on a potentially relevant TXXX frame cannot
be treated as proof of absence. Non-TXXX opaque frames may be skipped only after
their framing and enclosing boundary are established.

## ID3v2.4 and TXXX text

- Outer header still ten bytes with synchsafe28 body length. Flags add `0x10`
  footer. Body length excludes header and optional footer; footer begins at
  `tag_start + 10 + body_length`, occupies ten bytes, and mirrors the header with
  magic `3DI`. Padding and footer are mutually exclusive.
- Frame header remains ten bytes, but frame payload length is **synchsafe28** and
  describes stored bytes after compression/encryption/unsynchronization.
- Extended-header synchsafe28 length **includes itself**, minimum six bytes;
  number-of-flag-bytes is 1. Update, CRC and restriction payload lengths are
  respectively 0, 5 and 1. Validate framing before skipping.
- Unsynchronization is frame-local. Header unsync means all frames; frame bit
  `0x02` identifies transformed frames. Do not globally deunsynchronize before
  frame walking. Group byte `0x40`, encryption `0x04`, compression `0x08` and
  four-byte synchsafe data-length indicator `0x01` require explicit handling;
  compression requires that indicator. Stored frame length includes additions.
- Appended tags require footers; SEEK/update/multiple-tag replacement semantics
  exist, so a prefix-only reader must explicitly limit its support claim.

[ID3v2.4 structure, §§3–6](https://id3.org/id3v2.4.0-structure).

TXXX payload is encoding byte, terminated description, then value. Multiple TXXX
frames are allowed, but descriptions must be unique. ID3v2.4 adds encoding 2
(UTF-16BE without BOM) and 3 (UTF-8); encoding 1 is UTF-16 with BOM, with consistent
byte order throughout the frame. Search for UTF-16 terminators only on code-unit
boundaries, decode strictly, and distinguish description from value. Reject
truncated encodings and ambiguous multi-value numeric fields; do not decode by
lossy replacement or split a UTF-16 string at any zero byte.
[TXXX frame definition, §4.2.6](https://id3.org/id3v2.4.0-frames),
[text encodings in the structure specification](https://id3.org/id3v2.4.0-structure).

Recommended first scope: v2.3/v2.4 only, valid known flags and bounded complete
framing; reject unknown revisions/features rather than interpreting v2.2 TXX as
TXXX. Decide explicit UTF-16 compatibility behavior for writers omitting a second
BOM; the normative BOM requirements are not permission to guess arbitrary bytes.

## APEv2 at EOF

WavPack's own 5.8.1 reader checks the final 32-byte `APETAGEX` footer, then the
footer immediately before a verified 128-byte ID3v1 `TAG` trailer; it also has a
separate leading-header fallback. Its descriptor has magic[8], LE32 version
(2000), size, item count, flags, reserved[8]. Size covers items plus footer,
excluding an optional 32-byte header. Footer bit 31 announces that header; bit 29
marks a descriptor as a header. The reader validates matching header
magic/version/size/count when present.
[WavPack 5.8.1 tag reader](https://github.com/dbry/WavPack/blob/5.8.1/src/tags.c#L33-L114),
[descriptor layout](https://github.com/dbry/WavPack/blob/5.8.1/src/wavpack_local.h#L150-L169).

For endpoint `E` immediately after the footer: items start at `E-size`; optional
header starts at `E-size-32`. Validate these subtractions against object length
before requesting bytes. A failed length/validator check is not an empty tag.

Items carry LE32 value length, LE32 flags, NUL-terminated ASCII key, then exactly
the declared value bytes. TagLib's reader interprets flag bit 0 as read-only and
bits 1–2 as type; type 0 is UTF-8 text, whose multiple values are NUL-separated.
Header/footer bit 30 indicates footer absence; bit 29 differs between header and
footer. This is pinned implementation evidence, not an invented requirement for
every other writer.
[TagLib 2.0.2 items](https://github.com/taglib/taglib/blob/v2.0.2/taglib/ape/apeitem.cpp#L215-L237),
[footer flags](https://github.com/taglib/taglib/blob/v2.0.2/taglib/ape/apefooter.cpp#L154-L179).

Recommended policy: bounded key lengths/counts; strict text values for the four
fields; reject multiple numeric values, duplicate case-folded keys, inconsistent
descriptors/reserved flags and payload overruns. Skip binary artwork by checked
value length. Probe only the declared footer positions, not arbitrary matches in
audio. Declare leading tags or extra trailing junk unsupported if not handled.

## MP4/M4A iTunes freeform atoms

Apple defines atom sizes as including headers, normally BE32, with BE64 extended
size when size=1; size=0 extends to EOF. Type alone is insufficient: hierarchy
matters, unknown atoms can be skipped by length, and children cannot escape
parent boundaries. Large `mdat` payloads need not be read to reach a later `moov`.
[Apple atom framing](https://developer.apple.com/documentation/quicktime-file-format/atoms),
[large data atoms](https://developer.apple.com/documentation/quicktime-file-format/movie_data_atom).

TagLib 2.0.2 reads `moov/udta/meta/ilst`; freeform `----` children contain `mean`,
`name`, then `data`. Its writer stores four zero version/flag bytes before UTF-8
`mean` and `name` strings. A text `data` atom has BE32 type=1, BE32 locale, then
UTF-8 bytes: minimum total header+fields is 16 bytes; `mean`/`name` minimum is 12.
The composite key is `----:<mean>:<name>`. It handles QuickTime's alternate
non-full `meta` layout separately, so unconditionally skipping four bytes after
every `meta` header is not universal.
[TagLib freeform parser/writer](https://github.com/taglib/taglib/blob/v2.0.2/taglib/mp4/mp4itemfactory.cpp#L399-L444),
[freeform value writer](https://github.com/taglib/taglib/blob/v2.0.2/taglib/mp4/mp4itemfactory.cpp#L745-L785),
[container path](https://github.com/taglib/taglib/blob/v2.0.2/taglib/mp4/mp4tag.cpp#L67-L88),
[UTF-8 type value](https://github.com/taglib/taglib/blob/v2.0.2/taglib/mp4/mp4atom.h#L35-L42),
[meta layout distinction](https://github.com/taglib/taglib/blob/v2.0.2/taglib/mp4/mp4atom.cpp#L100-L113).

Loudgain's own documentation records the ReplayGain names under
`----:com.apple.iTunes:REPLAYGAIN_*`.
[Writer convention](https://github.com/Moonbase59/loudgain#replaygain-tags).

Recommended scope: exact namespace `com.apple.iTunes`, known scalar text type,
bounded fields, explicit supported `meta` variant, and complete relevant `ilst`
walk. Reject duplicate gain/peak names or multiple numeric `data` children. Do not
search `mdat` bytes for `----`, accept another vendor namespace, or borrow peaks
from another `ilst`. MDTA keys, ID32-embedded ID3 and SoundCheck are separate paths.

## Ogg Vorbis and Opus

Ogg pages start `OggS`, version 0; the fixed header is 27 bytes, followed by
`page_segments` lacing bytes. Total page length is `27 + count + sum(laces)`;
multi-byte fields are little-endian. Serial number is at offset 14, page sequence
at 18, CRC at 22, segment count at 26. Lacing 255 continues a packet; a value
below 255 terminates it, including a zero segment after a multiple of 255.
Packets can span pages. Continuation flags, serial/sequence and CRC (polynomial
`0x04c11db7`, checksum field zero during calculation) establish page ownership.
[RFC 3533, §§5–6](https://www.rfc-editor.org/rfc/rfc3533.html#section-6).

Vorbis comments are the second header packet: prefix `03` + `vorbis`, then LE32
vendor byte length/vendor, LE32 comment count, repeated LE32 byte length/UTF-8
comment, then a set framing bit. Fields use case-insensitive ASCII names followed
by `=`; duplicate names are generally permitted by Vorbis, so rejecting duplicate
ReplayGain scalars is Qoda's stricter policy. Packet size is not inherently small.
[Xiph comment specification](https://xiph.org/vorbis/doc/v-comment.html),
[header signatures, §4.2.1](https://xiph.org/vorbis/doc/Vorbis_I_spec.html).

Opus has `OpusHead` first and `OpusTags` second. The latter uses the comment
length/value design without Vorbis's final framing bit; trailing padding/data
rules differ. RFC 7845 discourages the four REPLAYGAIN fields unless normalization
confusion is excluded. OpusHead output gain and R128 tags have separate semantics.
[RFC 7845, §§3,5.2–5.2.1](https://www.rfc-editor.org/rfc/rfc7845.html#section-5.2).

Recommended first scope: one supported logical audio stream, validated page
CRC/sequence/continuation, bounded comment reassembly and no decoding. Do not
accidentally require Vorbis's framing byte in native FLAC or Opus. A prefix-only
metadata check does not establish whole-file absence of later chained links;
applying first-link gain to all later links is unsafe. A narrow valid-stream
claim should also check an EOF-ending EOS page with the same serial and reject
other BOS/serials encountered while reading headers. This still is not full
validation of every intervening audio page. If the supported single-link boundary
cannot be established within budgets, use explicit fallback.

**Conservative Opus subset, recommended Qoda policy:** explicit `REPLAYGAIN_*`
text may be consumed only if a supported, validated OpusHead declares output
gain exactly zero (signed LE16 at offsets 16–17), complete OpusTags contain no
case-insensitive `R128_*` field at all, the single-stream/link boundary above is
established, and native mpv ReplayGain stays disabled. Reject R128 presence even
when its value is malformed or duplicated; never convert or infer a reference
level. Apply only the literal tagged dB with existing matching-peak rules. Keep
nonzero output gain, unknown head versions/mappings, conflicting tag systems and
incomplete packets unsupported until separately designed and tested.

RFC 7845 does **not** forbid this reader policy: its §5.2.1 discourages writing
ReplayGain fields except where normalization confusion is excluded. Zero header
gain plus no R128 fields removes the declared competing gain paths; the explicit
policy supplies Qoda's narrowly controlled context. This is an implementation
inference and supported subset, not an RFC guarantee about arbitrary encoders or
tag accuracy. Test actual mpv/decoder output with these cases before claiming the
subset qualified. No R128/OpusHead reference conversion belongs in this change.

## WAV/RIFF

RIFF `WAVE` starts with FOURCC, size and form type (12 bytes). Child chunks use
FOURCC + size (8 bytes); sizes exclude that header and the odd-length pad byte.
The next child is at `start + 8 + size + (size & 1)`. A LIST's payload begins with
its four-byte list type; `LIST/INFO` contains child chunks, not arbitrary long
Vorbis-style keys.
[Microsoft RIFF framing](https://learn.microsoft.com/en-us/windows/win32/xaudio2/resource-interchange-file-format--riff-),
[LIST/INFO layout](https://learn.microsoft.com/en-us/windows/win32/multimedia/resource-interchange-file-format-services).

FFmpeg n8.0 recognizes both `ID3 ` and `id3 ` WAV chunks and dispatches their
payloads to its ID3v2 reader. It handles LIST/INFO separately and distinguishes
RIFX, RF64/BW64 and ds64 sizing from ordinary little-endian RIFF.
[Pinned WAV reader](https://github.com/FFmpeg/FFmpeg/blob/n8.0/libavformat/wavdec.c#L528-L576).

Recommended scope: ordinary RIFF/WAVE plus a bounded ID3-chunk payload, walked
through the declared RIFF end including tags after `data`. Skip audio with checked
lengths. Declare RIFX/RF64/BW64 unsupported until their distinct rules exist.
No reviewed primary mapping establishes LIST/INFO text as the four long
ReplayGain keys; do not parse a comment string for gain-looking substrings or
reinterpret BWF loudness as ReplayGain.

## Range budgets and acceptance cases

Retain the existing prepared-source/version and HTTP-range/ETag invariants,
global request/body/time budgets, overflow-checked offsets and explicit fallback.
Container adapters must also bound frames/items/pages/depth and scalar allocation.
Declared large artwork/audio payloads can be skipped only where physical lengths
are reliable. EOF adapters require a trusted consistent total length; unknown
length is unsupported, not absent metadata. An adapter's logical-metadata budget
must not accidentally prohibit a safe sparse jump across a large audio `mdat` or
WAV `data` chunk merely because its file offset exceeds FLAC's metadata-span cap.

Meaningful fixtures should cover actual upstream-authored tags plus fragmented
ranges; UTF-16 terminators/BOMs; v3 versus v4 sizes and unsync; extended headers;
footer/ID3v1 combinations; tail MP4 `moov`; WAV tags after audio; split Ogg packets
and incorrect CRC/serial; duplicate/malformed values; independent-family conflicts;
oversized skipped artwork; unexpected EOF, changing validators and ignored ranges.
Off must still perform no metadata probe. Unity fallback keeps playback available
and names the unsupported/invalid/incomplete reason; successful parsing is not a
physical fidelity qualification.
