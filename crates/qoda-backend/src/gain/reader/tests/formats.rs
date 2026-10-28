use super::*;
use crate::gain::{FieldStatus, Mode, TagSource, select};

const ID3V23: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/replaygain/id3v23.mp3"
));
const ID3V24: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/replaygain/id3v24.mp3"
));
const APE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/replaygain/ape.mp3"
));
const MP4: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/replaygain/gain.m4a"
));
const VORBIS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/replaygain/gain.ogg"
));
const OPUS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/replaygain/gain.opus"
));
const WAVE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/replaygain/gain.wav"
));
const OPUS_BASE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/replaygain/opus-base.opus"
));
const OPUS_R128: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/replaygain/opus-r128.opus"
));

#[tokio::test]
async fn upstream_authored_tags_preserve_family_values_and_field_provenance() {
    for (name, bytes, origin) in [
        ("id3v23", ID3V23, TagSource::Id3),
        ("id3v24", ID3V24, TagSource::Id3),
        ("ape", APE, TagSource::Ape),
        ("mp4", MP4, TagSource::Mp4),
        ("vorbis", VORBIS, TagSource::Vorbis),
        ("opus", OPUS, TagSource::Opus),
        ("wave", WAVE, TagSource::WavId3),
    ] {
        let source = Source::new(bytes.to_vec(), Fault::None);
        let tags = probe(&source)
            .await
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let applied = select(Mode::Track, false, Ok(tags.clone()));
        assert_eq!(applied.db, -6.0, "{name}");
        let metadata = applied.metadata.unwrap();
        assert_eq!(metadata.source, Some(origin), "{name}");
        for status in [
            metadata.track_gain,
            metadata.track_peak,
            metadata.album_gain,
            metadata.album_peak,
        ] {
            assert_eq!(status, FieldStatus::Valid, "{name}");
        }
        assert_eq!(select(Mode::Album, true, Ok(tags)).db, -12.0, "{name}");
        assert_eq!(
            source.ranges.lock().unwrap().len(),
            1,
            "small complete {name} fixture uses its prefix"
        );
    }
}

fn ss(n: usize) -> [u8; 4] {
    [
        (n >> 21) as u8 & 127,
        (n >> 14) as u8 & 127,
        (n >> 7) as u8 & 127,
        n as u8 & 127,
    ]
}
fn frame(version: u8, name: &[u8; 4], payload: &[u8], format: u8) -> Vec<u8> {
    let mut out = name.to_vec();
    out.extend(if version == 3 {
        (payload.len() as u32).to_be_bytes()
    } else {
        ss(payload.len())
    });
    out.extend([0, format]);
    out.extend(payload);
    out
}
fn text(key: &str, value: &str) -> Vec<u8> {
    let mut out = vec![3];
    out.extend(key.as_bytes());
    out.push(0);
    out.extend(value.as_bytes());
    out
}
fn utf16(key: &str, value: &str, be: bool, second_bom: bool) -> Vec<u8> {
    let mut out = vec![1];
    out.extend(if be { [0xfe, 0xff] } else { [0xff, 0xfe] });
    for unit in key.encode_utf16() {
        out.extend(if be {
            unit.to_be_bytes()
        } else {
            unit.to_le_bytes()
        });
    }
    out.extend([0, 0]);
    if second_bom {
        out.extend(if be { [0xfe, 0xff] } else { [0xff, 0xfe] });
    }
    for unit in value.encode_utf16() {
        out.extend(if be {
            unit.to_be_bytes()
        } else {
            unit.to_le_bytes()
        });
    }
    out.extend([0, 0]);
    out
}
fn tag(version: u8, flags: u8, body: &[u8]) -> Vec<u8> {
    let mut out = b"ID3".to_vec();
    out.extend([version, 0, flags]);
    out.extend(ss(body.len()));
    out.extend(body);
    if version == 4 && flags & 16 != 0 {
        let mut footer = out[..10].to_vec();
        footer[..3].copy_from_slice(b"3DI");
        out.extend(footer);
    }
    out
}
fn mpeg(prefix: &[u8], tail: &[u8]) -> Vec<u8> {
    let mut out = prefix.to_vec();
    out.extend(b"\xff\xfb\x90\0");
    out.extend([0; 256]);
    out.extend(tail);
    out
}
fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (index, byte) in bytes.iter().enumerate() {
        out.push(*byte);
        if *byte == 255
            && bytes
                .get(index + 1)
                .is_none_or(|next| *next == 0 || *next >= 0xe0)
        {
            out.push(0);
        }
    }
    out
}
fn ape_item(key: &str, flags: u32, value: &[u8]) -> Vec<u8> {
    let mut out = (value.len() as u32).to_le_bytes().to_vec();
    out.extend(flags.to_le_bytes());
    out.extend(key.as_bytes());
    out.push(0);
    out.extend(value);
    out
}
fn descriptor(size: usize, count: usize, flags: u32) -> Vec<u8> {
    let mut out = b"APETAGEX".to_vec();
    out.extend(2000u32.to_le_bytes());
    out.extend((size as u32).to_le_bytes());
    out.extend((count as u32).to_le_bytes());
    out.extend(flags.to_le_bytes());
    out.extend([0; 8]);
    out
}
fn ape(items: &[Vec<u8>], header: bool) -> Vec<u8> {
    let size = items.iter().map(Vec::len).sum::<usize>() + 32;
    let flag = if header { 0x8000_0000 } else { 0 };
    let mut out = if header {
        descriptor(size, items.len(), flag | 0x2000_0000)
    } else {
        Vec::new()
    };
    for item in items {
        out.extend(item);
    }
    out.extend(descriptor(size, items.len(), flag));
    out
}
async fn parsed(bytes: Vec<u8>) -> Tags {
    probe(&Source::new(bytes, Fault::None)).await.unwrap()
}

#[tokio::test]
async fn id3_lengths_unicode_global_and_frame_unsynchronization_are_separate() {
    let mut latin = text("REPLAYGAIN_TRACK_GAIN", "-6 dB");
    latin[0] = 0;
    let mut big_endian = utf16("REPLAYGAIN_TRACK_GAIN", "-6 dB", true, false);
    big_endian[0] = 2;
    big_endian.drain(1..3);
    for (version, payload) in [
        (3, latin),
        (4, big_endian),
        (3, utf16("REPLAYGAIN_TRACK_GAIN", "-6 dB", false, false)),
        (3, utf16("REPLAYGAIN_TRACK_GAIN", "-6 dB", true, true)),
        (4, utf16("REPLAYGAIN_TRACK_GAIN", "-6 dB", false, true)),
        (4, text("REPLAYGAIN_TRACK_GAIN", "-6 dB")),
    ] {
        let normal = frame(version, b"TXXX", &payload, 0);
        let tags = parsed(mpeg(&tag(version, 0, &normal), &[])).await;
        assert_eq!(tags.track, Some(-6.0));
        if version == 3 {
            assert_eq!(
                parsed(mpeg(&tag(3, 0x80, &escape(&normal)), &[]))
                    .await
                    .track,
                Some(-6.0)
            );
        }
    }
    let payload = utf16("REPLAYGAIN_TRACK_GAIN", "-6 dB", false, true);
    for (header_flags, format_flags) in [(0, 3), (0x80, 1), (0x80, 3)] {
        let mut with_length = ss(payload.len()).to_vec();
        with_length.extend(&payload);
        let body = frame(4, b"TXXX", &escape(&with_length), format_flags);
        assert_eq!(
            parsed(mpeg(&tag(4, header_flags, &body), &[])).await.track,
            Some(-6.0)
        );
    }
    let mut big = frame(3, b"APIC", &vec![0x55; 255], 0);
    big.extend(frame(
        3,
        b"TXXX",
        &utf16("REPLAYGAIN_TRACK_GAIN", "-6 dB", false, false),
        0,
    ));
    assert_eq!(parsed(mpeg(&tag(3, 0, &big), &[])).await.track, Some(-6.0));
    let mut grouped = vec![7];
    grouped.extend(ss(payload.len()));
    grouped.extend(payload);
    assert_eq!(
        parsed(mpeg(&tag(4, 0, &frame(4, b"TXXX", &grouped, 0x41)), &[]))
            .await
            .track,
        Some(-6.0)
    );
}

#[tokio::test]
async fn id3_extended_headers_footers_and_scalar_states_remain_explicit() {
    let body = frame(4, b"TXXX", &text("REPLAYGAIN_TRACK_GAIN", "0 dB"), 0);
    let measured = parsed(mpeg(&tag(4, 16, &body), &[])).await;
    assert_eq!(measured.track, Some(0.0));
    assert_eq!(measured.metadata.track_gain, FieldStatus::Valid);
    assert_eq!(parsed(mpeg(&[], &tag(4, 16, &body))).await.track, Some(0.0));
    let mut ext = ss(6).to_vec();
    ext.extend([1, 0]);
    ext.extend(&body);
    assert_eq!(
        parsed(mpeg(&tag(4, 0x40, &ext), &[])).await.track,
        Some(0.0)
    );
    let mut ext = 6u32.to_be_bytes().to_vec();
    ext.extend([0, 0]);
    ext.extend(25u32.to_be_bytes());
    ext.extend(frame(
        3,
        b"TXXX",
        &utf16("REPLAYGAIN_TRACK_GAIN", "0 dB", false, false),
        0,
    ));
    ext.extend([0; 25]);
    assert_eq!(
        parsed(mpeg(&tag(3, 0x40, &ext), &[])).await.track,
        Some(0.0)
    );
    let mut duplicate = body.clone();
    duplicate.extend(frame(4, b"TXXX", &text("replaygain_track_gain", "0"), 0));
    let duplicate = parsed(mpeg(&tag(4, 0, &duplicate), &[])).await;
    assert_eq!(duplicate.track, None);
    assert_eq!(duplicate.metadata.track_gain, FieldStatus::Duplicate);
    let invalid = parsed(mpeg(
        &tag(
            4,
            0,
            &frame(4, b"TXXX", &text("REPLAYGAIN_TRACK_GAIN", "-6\0-12"), 0),
        ),
        &[],
    ))
    .await;
    assert_eq!(invalid.metadata.track_gain, FieldStatus::Invalid);
    assert_eq!(invalid.metadata.track_peak, FieldStatus::Missing);
    let mut odd = utf16("REPLAYGAIN_TRACK_GAIN", "-6 dB", false, false);
    odd.pop();
    assert_eq!(
        parsed(mpeg(&tag(4, 0, &frame(4, b"TXXX", &odd, 0)), &[]))
            .await
            .metadata
            .track_gain,
        FieldStatus::Invalid
    );
}

#[tokio::test]
async fn malformed_or_unsupported_later_id3_framing_cannot_validate_an_early_gain() {
    let good = frame(4, b"TXXX", &text("REPLAYGAIN_TRACK_GAIN", "-6 dB"), 0);
    let mut later = good.clone();
    later.extend(frame(4, b"bad!", b"x", 0));
    let mut high = tag(4, 0, &good);
    high[6] = 0x80;
    let mut bad_length = good.clone();
    bad_length[7] = 0x80;
    for bytes in [
        mpeg(&tag(4, 0, &later), &[]),
        mpeg(&high, &[]),
        mpeg(&tag(4, 0, &bad_length), &[]),
        mpeg(
            &tag(
                4,
                0,
                &frame(4, b"TXXX", &text("REPLAYGAIN_TRACK_GAIN", "-6"), 4),
            ),
            &[],
        ),
        mpeg(&tag(4, 0, &good), &tag(4, 16, &good)),
    ] {
        assert!(probe(&Source::new(bytes, Fault::None)).await.is_err());
    }
}

#[tokio::test]
async fn ape_footer_header_id3v1_and_family_conflicts_do_not_borrow_peaks() {
    for header in [false, true] {
        let mut tail = ape(
            &[
                ape_item("REPLAYGAIN_TRACK_GAIN", 0, b"-6 dB"),
                ape_item("REPLAYGAIN_TRACK_PEAK", 0, b"1"),
            ],
            header,
        );
        let tags = parsed(mpeg(&[], &tail)).await;
        assert_eq!(tags.track, Some(-6.0));
        assert_eq!(tags.metadata.source, Some(TagSource::Ape));
        let mut id3v1 = b"TAG".to_vec();
        id3v1.extend([0; 125]);
        tail.extend(id3v1);
        assert_eq!(parsed(mpeg(&[], &tail)).await.track, Some(-6.0));
    }
    let prefix = tag(
        4,
        0,
        &frame(4, b"TXXX", &text("REPLAYGAIN_TRACK_GAIN", "6"), 0),
    );
    let tail = ape(&[ape_item("REPLAYGAIN_TRACK_PEAK", 0, b"0.1")], true);
    assert_eq!(
        probe(&Source::new(mpeg(&prefix, &tail), Fault::None))
            .await
            .unwrap_err(),
        CONFLICT
    );
    let duplicate = parsed(mpeg(
        &[],
        &ape(
            &[
                ape_item("REPLAYGAIN_TRACK_GAIN", 0, b"-6"),
                ape_item("replaygain_track_gain", 0, b"-3"),
            ],
            false,
        ),
    ))
    .await;
    assert_eq!(duplicate.metadata.track_gain, FieldStatus::Duplicate);
    assert!(duplicate.track.is_none());
    let binary = parsed(mpeg(
        &[],
        &ape(&[ape_item("REPLAYGAIN_TRACK_GAIN", 2, b"-6")], false),
    ))
    .await;
    assert_eq!(binary.metadata.track_gain, FieldStatus::Invalid);
    let multiple = parsed(mpeg(
        &[],
        &ape(&[ape_item("REPLAYGAIN_TRACK_GAIN", 0, b"-6\0-3")], false),
    ))
    .await;
    assert_eq!(multiple.metadata.track_gain, FieldStatus::Invalid);
}

#[tokio::test]
async fn ape_truncated_footer_or_inconsistent_header_never_establish_absence() {
    let tail = ape(&[ape_item("REPLAYGAIN_TRACK_GAIN", 0, b"-6")], true);
    for missing in 1..=28 {
        let file = mpeg(&[], &tail[..tail.len() - missing]);
        assert!(
            probe(&Source::new(file, Fault::None)).await.is_err(),
            "missing {missing}"
        );
    }
    for offset in [8, 12, 16, 20, 24] {
        let mut bad = tail.clone();
        bad[offset] ^= 1;
        assert!(
            probe(&Source::new(mpeg(&[], &bad), Fault::None))
                .await
                .is_err()
        );
    }
}

fn atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend(kind);
    out.extend(payload);
    out
}
fn freeform(namespace: &str, key: &str, values: &[(u32, &[u8])]) -> Vec<u8> {
    let mut mean = vec![0; 4];
    mean.extend(namespace.as_bytes());
    let mut name = vec![0; 4];
    name.extend(key.as_bytes());
    let mut out = atom(b"mean", &mean);
    out.extend(atom(b"name", &name));
    for (kind, value) in values {
        let mut data = kind.to_be_bytes().to_vec();
        data.extend([0; 4]);
        data.extend(*value);
        out.extend(atom(b"data", &data));
    }
    atom(b"----", &out)
}
fn moov(items: &[u8]) -> Vec<u8> {
    let mut meta = vec![0; 4];
    meta.extend(atom(b"ilst", items));
    atom(b"moov", &atom(b"udta", &atom(b"meta", &meta)))
}
fn ftyp() -> Vec<u8> {
    atom(b"ftyp", b"M4A \0\0\0\0M4A ")
}
#[tokio::test]
async fn mp4_requires_the_right_namespace_hierarchy_and_single_scalar() {
    let gain = freeform("com.apple.iTunes", "replaygain_track_gain", &[(1, b"6 dB")]);
    let other = freeform("another.vendor", "REPLAYGAIN_TRACK_PEAK", &[(1, b"0.1")]);
    let mut items = gain.clone();
    items.extend(other);
    let mut file = ftyp();
    file.extend(moov(&items));
    let tags = parsed(file).await;
    assert_eq!(tags.track_peak, None);
    let applied = select(Mode::Track, false, Ok(tags));
    assert_eq!(applied.db, 0.0);
    assert!(applied.limited);
    let repeated = freeform(
        "com.apple.iTunes",
        "REPLAYGAIN_TRACK_GAIN",
        &[(1, b"-6"), (1, b"-3")],
    );
    let mut file = ftyp();
    file.extend(moov(&repeated));
    assert_eq!(
        parsed(file).await.metadata.track_gain,
        FieldStatus::Duplicate
    );
    let wrong = freeform("com.apple.iTunes", "REPLAYGAIN_TRACK_GAIN", &[(2, b"-6")]);
    let mut file = ftyp();
    file.extend(moov(&wrong));
    assert_eq!(parsed(file).await.metadata.track_gain, FieldStatus::Invalid);
    let mut file = ftyp();
    file.extend(atom(b"mdat", &gain));
    file.extend(moov(&[]));
    assert!(
        parsed(file).await.track.is_none(),
        "audio bytes never establish a freeform tag"
    );
    let mut file = ftyp();
    file.extend(moov(&gain));
    file.extend([0, 0, 0, 7, b'f', b'r', b'e', b'e']);
    assert!(probe(&Source::new(file, Fault::None)).await.is_err());
}

#[tokio::test]
async fn mp4_extended_and_eof_sizes_stay_within_the_parent() {
    let gain = freeform("com.apple.iTunes", "REPLAYGAIN_TRACK_GAIN", &[(1, b"-6")]);
    let normal = moov(&gain);
    let mut extended = 1u32.to_be_bytes().to_vec();
    extended.extend(b"moov");
    extended.extend((normal.len() as u64 + 8).to_be_bytes());
    extended.extend(&normal[8..]);
    let mut file = ftyp();
    file.extend(&extended);
    assert_eq!(parsed(file).await.track, Some(-6.0));
    let mut eof = normal.clone();
    eof[..4].fill(0);
    let mut file = ftyp();
    file.extend(eof);
    assert_eq!(parsed(file).await.track, Some(-6.0));
    extended[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
    let mut file = ftyp();
    file.extend(extended);
    assert!(probe(&Source::new(file, Fault::None)).await.is_err());
    let mut escape = normal;
    escape[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    let mut file = ftyp();
    file.extend(escape);
    assert!(probe(&Source::new(file, Fault::None)).await.is_err());
}

struct Sparse {
    total: usize,
    regions: Vec<(usize, Vec<u8>)>,
    ranges: Mutex<Vec<(usize, usize)>>,
}
impl Sparse {
    fn new(total: usize, regions: Vec<(usize, Vec<u8>)>) -> Self {
        Self {
            total,
            regions,
            ranges: Mutex::default(),
        }
    }
}
impl MediaSource for Sparse {
    fn open(&self, range: Option<ByteRange>, head: bool) -> ProviderFuture<'_, MediaResponse> {
        Box::pin(async move {
            assert!(!head);
            let raw = range.unwrap();
            let (start, end) = raw
                .header()
                .strip_prefix("bytes=")
                .unwrap()
                .split_once('-')
                .unwrap();
            let start: usize = start.parse().unwrap();
            let end = end.parse::<usize>().unwrap().min(self.total - 1);
            self.ranges.lock().unwrap().push((start, end));
            let mut bytes = vec![0; end - start + 1];
            for (offset, value) in &self.regions {
                let first = start.max(*offset);
                let last = (end + 1).min(offset + value.len());
                if first < last {
                    bytes[first - start..last - start]
                        .copy_from_slice(&value[first - offset..last - offset]);
                }
            }
            Ok(MediaResponse {
                status: 206,
                content_type: None,
                content_length: Some(bytes.len() as u64),
                content_range: Some(format!("bytes {start}-{end}/{}", self.total)),
                etag: Some("fixture-v1".into()),
                body: futures_util::stream::once(async move { Ok(Bytes::from(bytes)) }).boxed(),
            })
        })
    }
}
fn transferred(source: &Sparse) -> usize {
    source
        .ranges
        .lock()
        .unwrap()
        .iter()
        .map(|(a, b)| b - a + 1)
        .sum()
}

#[tokio::test]
async fn sparse_id3_artwork_padding_and_ape_cover_do_not_need_artwork_sized_reads() {
    let gain = frame(4, b"TXXX", &text("REPLAYGAIN_TRACK_GAIN", "-6"), 0);
    let huge = 2 * 1024 * 1024;
    for artwork in [true, false] {
        let body_len = if artwork {
            10 + huge + gain.len()
        } else {
            gain.len() + huge
        };
        let mut prefix = b"ID3\x04\0\0".to_vec();
        prefix.extend(ss(body_len));
        let mut regions = vec![(0, prefix)];
        if artwork {
            let mut h = b"APIC".to_vec();
            h.extend(ss(huge));
            h.extend([0, 0]);
            regions.push((10, h));
            regions.push((20 + huge, gain.clone()));
        } else {
            regions.push((10, gain.clone()));
        }
        regions.push((10 + body_len, b"\xff\xfb\x90\0".to_vec()));
        let source = Sparse::new(10 + body_len + 260, regions);
        assert_eq!(probe(&source).await.unwrap().track, Some(-6.0));
        assert!(transferred(&source) < PREFIX + 12 * 1024);
    }
    let audio = 260;
    let key = b"Cover Art (Front)\0";
    let mut art = (huge as u32).to_le_bytes().to_vec();
    art.extend(2u32.to_le_bytes());
    art.extend(key);
    let gain = ape_item("REPLAYGAIN_TRACK_GAIN", 0, b"-6");
    let size = art.len() + huge + gain.len() + 32;
    let total = audio + 32 + size;
    let source = Sparse::new(
        total,
        vec![
            (0, b"\xff\xfb\x90\0".to_vec()),
            (audio, descriptor(size, 2, 0xa000_0000)),
            (audio + 32, art.clone()),
            (audio + 32 + art.len() + huge, gain),
            (total - 32, descriptor(size, 2, 0x8000_0000)),
        ],
    );
    assert_eq!(probe(&source).await.unwrap().track, Some(-6.0));
    assert!(transferred(&source) < PREFIX + 12 * 1024);
}

#[tokio::test]
async fn large_audio_offsets_do_not_consume_the_metadata_span_budget() {
    let ftyp = ftyp();
    let audio = 80 * 1024 * 1024;
    let mut mdat = ((audio + 8) as u32).to_be_bytes().to_vec();
    mdat.extend(b"mdat");
    let tail = moov(&freeform(
        "com.apple.iTunes",
        "REPLAYGAIN_TRACK_GAIN",
        &[(1, b"-6")],
    ));
    let offset = ftyp.len() + audio + 8;
    let source = Sparse::new(
        offset + tail.len(),
        vec![(0, ftyp.clone()), (ftyp.len(), mdat), (offset, tail)],
    );
    assert_eq!(probe(&source).await.unwrap().track, Some(-6.0));
    assert!(transferred(&source) < PREFIX + 12 * 1024);
    assert!(
        source.ranges.lock().unwrap().len() < 12,
        "tail metadata shares bounded lookahead windows"
    );
    let id3 = tag(
        4,
        0,
        &frame(4, b"TXXX", &text("REPLAYGAIN_TRACK_GAIN", "-6"), 0),
    );
    let offset = 12 + 8 + audio;
    let total = offset + 8 + id3.len() + (id3.len() & 1);
    let mut header = b"RIFF".to_vec();
    header.extend(((total - 8) as u32).to_le_bytes());
    header.extend(b"WAVEdata");
    header.extend((audio as u32).to_le_bytes());
    let mut chunk = b"id3 ".to_vec();
    chunk.extend((id3.len() as u32).to_le_bytes());
    chunk.extend(id3);
    let source = Sparse::new(total, vec![(0, header), (offset, chunk)]);
    let parsed = probe(&source).await.unwrap();
    assert_eq!(parsed.track, Some(-6.0));
    assert_eq!(parsed.metadata.source, Some(TagSource::WavId3));
    assert!(transferred(&source) < PREFIX + 12 * 1024);
}

#[tokio::test]
async fn incomplete_ranges_and_ignored_tail_seeks_never_supply_a_known_early_gain() {
    let mut body = frame(4, b"TXXX", &text("REPLAYGAIN_TRACK_GAIN", "-6"), 0);
    body.extend(frame(4, b"APIC", &vec![0x55; PREFIX + 1], 0));
    let bytes = mpeg(&tag(4, 0, &body), &[]);
    for fault in [
        Fault::IgnoreSeek,
        Fault::ChangedEtag,
        Fault::MissingEtag,
        Fault::ChangedTotal,
        Fault::WrongLength,
        Fault::Truncated,
        Fault::ExtraByte,
        Fault::Interrupted,
    ] {
        assert!(probe(&Source::new(bytes.clone(), fault)).await.is_err());
    }
    assert_eq!(
        probe(&Source::new(bytes.clone(), Fault::Ignore))
            .await
            .unwrap()
            .track,
        Some(-6.0)
    );
    let mut large = bytes;
    large.resize(2 * 1024 * 1024, 0x55);
    assert!(
        probe(&Source::new(large, Fault::Ignore)).await.is_err(),
        "an incomplete scan cannot rule out tail tags"
    );
}

fn comments_packet(values: &[&str]) -> Vec<u8> {
    let mut out = 4u32.to_le_bytes().to_vec();
    out.extend(b"test");
    out.extend((values.len() as u32).to_le_bytes());
    for value in values {
        out.extend((value.len() as u32).to_le_bytes());
        out.extend(value.as_bytes());
    }
    out
}
fn ogg_page(serial: u32, sequence: u32, flags: u8, laces: &[u8], body: &[u8]) -> Vec<u8> {
    assert_eq!(
        laces.iter().map(|n| usize::from(*n)).sum::<usize>(),
        body.len()
    );
    let mut out = b"OggS".to_vec();
    out.extend([0, flags]);
    out.extend([0; 8]);
    out.extend(serial.to_le_bytes());
    out.extend(sequence.to_le_bytes());
    out.extend([0; 4]);
    out.push(laces.len() as u8);
    out.extend(laces);
    out.extend(body);
    let checksum = super::super::ogg::crc(&out);
    out[22..26].copy_from_slice(&checksum.to_le_bytes());
    out
}
fn packet_page(serial: u32, sequence: u32, flags: u8, packet: &[u8]) -> Vec<u8> {
    let mut laces = vec![255; packet.len() / 255];
    laces.push((packet.len() % 255) as u8);
    ogg_page(serial, sequence, flags, &laces, packet)
}
fn opus_head(gain: i16) -> Vec<u8> {
    let mut out = b"OpusHead".to_vec();
    out.extend([1, 2]);
    out.extend(312u16.to_le_bytes());
    out.extend(48000u32.to_le_bytes());
    out.extend(gain.to_le_bytes());
    out.push(0);
    out
}
fn opus_file(gain: i16, values: &[&str], split: bool, audio_pages: u32) -> Vec<u8> {
    let mut out = packet_page(7, 0, 2, &opus_head(gain));
    let mut tags = b"OpusTags".to_vec();
    tags.extend(comments_packet(values));
    let mut sequence = 1;
    if split {
        assert!(tags.len() > 255);
        out.extend(ogg_page(7, sequence, 0, &[255], &tags[..255]));
        sequence += 1;
        out.extend(packet_page(7, sequence, 1, &tags[255..]));
    } else {
        out.extend(packet_page(7, sequence, 0, &tags));
    }
    for index in 0..audio_pages {
        sequence += 1;
        out.extend(packet_page(
            7,
            sequence,
            if index + 1 == audio_pages { 4 } else { 0 },
            b"synthetic opaque audio",
        ));
    }
    out
}
fn rewrite_serial(bytes: &mut [u8], serial: u32) {
    let mut pos = 0;
    while pos < bytes.len() {
        let count = usize::from(bytes[pos + 26]);
        let size = 27
            + count
            + bytes[pos + 27..pos + 27 + count]
                .iter()
                .map(|n| usize::from(*n))
                .sum::<usize>();
        bytes[pos + 14..pos + 18].copy_from_slice(&serial.to_le_bytes());
        let crc = super::super::ogg::crc(&bytes[pos..pos + size]);
        bytes[pos + 22..pos + 26].copy_from_slice(&crc.to_le_bytes());
        pos += size;
    }
}
#[tokio::test]
async fn ogg_packet_continuation_crc_and_many_small_pages_are_checked() {
    let large = format!("COMMENT={}", "x".repeat(600));
    let values = [
        "REPLAYGAIN_TRACK_GAIN=-6",
        "REPLAYGAIN_TRACK_PEAK=1",
        large.as_str(),
    ];
    let bytes = opus_file(0, &values, true, 24);
    assert_eq!(
        parsed(bytes.clone()).await.track,
        Some(-6.0),
        "many real page boundaries do not exhaust false footer candidates"
    );
    let mut corrupt = bytes.clone();
    corrupt[30] ^= 1;
    assert!(probe(&Source::new(corrupt, Fault::None)).await.is_err());
    let mut wrong_continuation = bytes;
    let first = 27 + usize::from(wrong_continuation[26]) + 19;
    let second_count = usize::from(wrong_continuation[first + 26]);
    let second = first + 27 + second_count + 255;
    wrong_continuation[second + 5] &= !1;
    let size = 27
        + usize::from(wrong_continuation[second + 26])
        + wrong_continuation
            [second + 27..second + 27 + usize::from(wrong_continuation[second + 26])]
            .iter()
            .map(|n| usize::from(*n))
            .sum::<usize>();
    let crc = super::super::ogg::crc(&wrong_continuation[second..second + size]);
    wrong_continuation[second + 22..second + 26].copy_from_slice(&crc.to_le_bytes());
    assert!(
        probe(&Source::new(wrong_continuation, Fault::None))
            .await
            .is_err()
    );
}
#[tokio::test]
async fn opus_competing_gain_and_valid_chained_streams_do_not_inherit_first_link_gain() {
    for gain in [256, -256] {
        assert!(
            probe(&Source::new(
                opus_file(gain, &["REPLAYGAIN_TRACK_GAIN=-6"], false, 1),
                Fault::None
            ))
            .await
            .unwrap_err()
            .contains("Opus")
        );
    }
    for r128 in [
        "R128_TRACK_GAIN=0",
        "r128_ALBUM_GAIN=bad",
        "R128_TRACK_GAIN",
    ] {
        assert!(
            probe(&Source::new(
                opus_file(0, &["REPLAYGAIN_TRACK_GAIN=-6", r128], false, 1),
                Fault::None
            ))
            .await
            .unwrap_err()
            .contains("Opus")
        );
    }
    let mut file = OPUS.to_vec();
    let mut next = OPUS.to_vec();
    rewrite_serial(&mut next, 0xfedc_ba98);
    file.extend(next);
    assert!(
        probe(&Source::new(file, Fault::None))
            .await
            .unwrap_err()
            .contains("Chained")
    );
    let mut junk = OPUS.to_vec();
    junk.extend(b"trailing junk");
    assert!(probe(&Source::new(junk, Fault::None)).await.is_err());
    let ordinary = opus_file(0, &["REPLAYGAIN_TRACK_GAIN=-6"], false, 1);
    let tail = packet_page(7, 2, 4, b"synthetic opaque audio");
    let mut multiplexed = ordinary[..ordinary.len() - tail.len()].to_vec();
    multiplexed.extend(packet_page(8, 0, 2, &opus_head(0)));
    multiplexed.extend(packet_page(8, 1, 4, b"other stream"));
    multiplexed.extend(tail);
    assert!(
        probe(&Source::new(multiplexed, Fault::None))
            .await
            .unwrap_err()
            .contains("multiplexed")
    );
    assert!(
        probe(&Source::new(opus_file(0, &[], false, 1024), Fault::None))
            .await
            .is_err()
    );
}
fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut body = b"WAVE".to_vec();
    for (kind, payload) in chunks {
        body.extend(*kind);
        body.extend((payload.len() as u32).to_le_bytes());
        body.extend(payload);
        if payload.len() % 2 != 0 {
            body.push(0);
        }
    }
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}
#[tokio::test]
async fn wave_chunk_boundaries_not_audio_or_info_strings_supply_gain() {
    let tags = tag(
        4,
        0,
        &frame(4, b"TXXX", &text("REPLAYGAIN_TRACK_GAIN", "-6"), 0),
    );
    let data = tag(
        4,
        0,
        &frame(4, b"TXXX", &text("REPLAYGAIN_TRACK_GAIN", "12"), 0),
    );
    let file = riff(&[(b"data", data), (b"id3 ", tags.clone())]);
    assert_eq!(parsed(file).await.track, Some(-6.0));
    let repeated = riff(&[(b"id3 ", tags.clone()), (b"ID3 ", tags)]);
    assert!(probe(&Source::new(repeated, Fault::None)).await.is_err());
    let info = riff(&[(b"LIST", b"INFOREPLAYGAIN_TRACK_GAIN=-6".to_vec())]);
    assert!(parsed(info).await.track.is_none());
    let mut unsupported = WAVE.to_vec();
    unsupported[..4].copy_from_slice(b"RF64");
    assert!(probe(&Source::new(unsupported, Fault::None)).await.is_err());
}
#[tokio::test]
async fn canceling_an_eof_probe_drops_its_pending_body() {
    let mut file = ID3V24.to_vec();
    file.resize(128 * 1024, 0x55);
    let source = Arc::new(Source::new(file, Fault::HangingSeek));
    let worker = source.clone();
    let task = tokio::spawn(async move { probe(worker.as_ref()).await });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while source.ranges.lock().unwrap().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    assert!(source.dropped.load(Ordering::SeqCst));
}
struct CaptureDirectory(std::path::PathBuf);
impl Drop for CaptureDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn capture(bytes: &[u8], gain: crate::gain::Applied) -> Vec<i32> {
    let path = std::env::temp_dir().join(format!("qoda-codec-gain-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    let directory = CaptureDirectory(path);
    let input = directory.0.join("synthetic.opus");
    let output = directory.0.join("capture.pcm");
    std::fs::write(&input, bytes).unwrap();
    let player = crate::player::Player::test_options(
        "pcm",
        &[
            ("ao-pcm-file", output.to_str().unwrap().into()),
            ("ao-pcm-waveheader", "no".into()),
            ("audio-format", "s32".into()),
            ("audio-samplerate", "48000".into()),
            ("audio-channels", "stereo".into()),
        ],
    )
    .unwrap();
    player.send(crate::player::Command::LoadGain(
        1,
        input.to_str().unwrap().into(),
        0.0,
        gain,
    ));
    player.send(crate::player::Command::Pause(false));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let state = player.status();
            assert!(state.error.is_none(), "{:?}", state.error);
            if state.idle && state.natural_eof {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    drop(player);
    let bytes = std::fs::read(output).unwrap();
    assert_eq!(bytes.len() % 8, 0);
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|sample| i32::from_le_bytes(*sample))
        .collect()
}
async fn described_gain(bytes: &[u8]) -> crate::gain::Applied {
    use qoda_core::{Counter, SourceId, TrackRef};
    let reference = TrackRef {
        source: SourceId::new(),
        key: "synthetic-opus".to_owned().try_into().unwrap(),
    };
    let media = qoda_provider::PreparedMedia {
        track: qoda_provider::Track {
            reference: reference.clone(),
            title: "Synthetic tone".into(),
            artist: String::new(),
            album: String::new(),
            duration_ms: Some(Counter::new(250)),
        },
        version: "fixture-original".into(),
        stream: Box::new(Source::new(bytes.to_vec(), Fault::None)),
    };
    crate::gain::describe(
        media,
        crate::gain::Request {
            reference,
            mode: Mode::Track,
            album_session: false,
            fidelity: false,
        },
    )
    .await
    .unwrap()
    .gain
    .unwrap()
}
#[tokio::test]
async fn native_opus_literal_gain_is_single_applied_and_unsupported_normalizers_stay_unity() {
    let baseline = capture(OPUS_BASE, crate::gain::Applied::off()).await;
    assert!(!baseline.is_empty());
    assert!(baseline.iter().any(|sample| *sample != 0));
    let tagged_off = capture(OPUS, crate::gain::Applied::off()).await;
    assert_eq!(
        tagged_off, baseline,
        "native ReplayGain must remain disabled for Opus metadata"
    );
    let gain = described_gain(OPUS).await;
    assert_eq!(gain.db, -6.0);
    assert_eq!(gain.metadata.unwrap().source, Some(TagSource::Opus));
    let output = capture(OPUS, gain).await;
    assert_eq!(output.len(), baseline.len());
    let coefficient = 10.0_f64.powf(-6.0 / 20.0);
    for (actual, original) in output.iter().zip(&baseline) {
        assert!(
            (i64::from(*actual) - (f64::from(*original) * coefficient).round() as i64)
                .unsigned_abs()
                <= 1
        );
    }
    let mut headed = OPUS.to_vec();
    let body = 27 + usize::from(headed[26]);
    headed[body + 16..body + 18].copy_from_slice(&256i16.to_le_bytes());
    let size = body
        + headed[27..body]
            .iter()
            .map(|lace| usize::from(*lace))
            .sum::<usize>();
    let crc = super::super::ogg::crc(&headed[..size]);
    headed[22..26].copy_from_slice(&crc.to_le_bytes());
    for bytes in [&headed[..], OPUS_R128] {
        let gain = described_gain(bytes).await;
        assert_eq!(gain.db, 0.0);
        assert_eq!(gain.basis, "unity");
        assert!(gain.metadata.is_none());
        assert!(gain.reason.contains("Opus"));
        assert_eq!(
            capture(bytes, gain).await,
            capture(bytes, crate::gain::Applied::off()).await,
            "unsupported normalization must not acquire an extra application gain"
        );
    }
}
