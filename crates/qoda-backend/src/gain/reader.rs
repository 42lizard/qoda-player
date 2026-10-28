//! Bounded container adapters over one pinned provider stream.
use super::{INCOMPLETE, Tags, comments};
use futures_util::StreamExt;
use qoda_provider::{ByteRange, MediaResponse, MediaSource};
mod ape;
mod id3;
mod mp4;
mod ogg;
mod wave;

const PREFIX: usize = 64 * 1024;
const BYTES: usize = 1024 * 1024;
const BLOCKS: usize = 64;
const REQUESTS: usize = 64;
const SPAN: usize = 64 * 1024 * 1024;
const INVALID: &str = "Invalid embedded FLAC metadata";
const RANGE: &str = "Embedded tag range was not confirmed";
const LIMIT: &str = "Embedded tag scan exceeded its read limit";
const CHANGED: &str = "Embedded tag media changed during the scan";
const CONFLICT: &str = "Multiple embedded ReplayGain tag families or instances; unity gain";
const SCALAR: usize = 64 * 1024;

fn candidate(found: &mut Option<Tags>, next: Tags) -> Result<(), &'static str> {
    match found {
        Some(previous) if previous.has_fields() && next.has_fields() => Err(CONFLICT),
        Some(previous) if previous.has_fields() || !next.has_fields() => Ok(()),
        _ => {
            *found = Some(next);
            Ok(())
        }
    }
}

struct Block {
    kind: u8,
    last: bool,
    size: usize,
}
impl Block {
    fn parse(header: &[u8], index: usize) -> Result<Self, &'static str> {
        let kind = header[0] & 127;
        let size =
            usize::from(header[1]) << 16 | usize::from(header[2]) << 8 | usize::from(header[3]);
        if kind > 6 || (index == 0 && (kind != 0 || size != 34)) || (index > 0 && kind == 0) {
            return Err(INVALID);
        }
        Ok(Self {
            kind,
            last: header[0] & 128 != 0,
            size,
        })
    }
}
fn signature(bytes: &[u8]) -> Result<(), &'static str> {
    if bytes.len() < 4 {
        return Err(INCOMPLETE);
    }
    if !bytes.starts_with(b"fLaC") {
        return Err("Embedded tags unavailable for this format");
    }
    Ok(())
}
fn end_offset(pos: usize, size: usize) -> Result<usize, &'static str> {
    pos.checked_add(size)
        .filter(|end| *end <= SPAN)
        .ok_or(LIMIT)
}
fn store_comments(found: &mut Option<Tags>, bytes: &[u8]) -> Result<(), &'static str> {
    if found.is_some() {
        return Err("Duplicate embedded FLAC comment blocks");
    }
    *found = Some(comments(bytes)?);
    Ok(())
}
// Used only when the initial server response ignores Range. Every byte through
// LAST must actually be present; unread artwork never establishes missing tags.
pub(super) fn tags(bytes: &[u8]) -> Result<Tags, &'static str> {
    signature(bytes)?;
    let mut pos = 4;
    let mut found = None;
    for index in 0..BLOCKS {
        let header_end = end_offset(pos, 4)?;
        let header = bytes.get(pos..header_end).ok_or(INCOMPLETE)?;
        let block = Block::parse(header, index)?;
        pos = header_end;
        let end = end_offset(pos, block.size)?;
        let payload = bytes.get(pos..end).ok_or(INCOMPLETE)?;
        if block.kind == 4 {
            store_comments(&mut found, payload)?;
        }
        if block.last {
            return Ok(found.unwrap_or_default());
        }
        pos = end;
    }
    Err(LIMIT)
}

struct Reader<'a> {
    stream: &'a dyn MediaSource,
    prefix: Vec<u8>,
    total: usize,
    etag: Option<String>,
    requests: usize,
    transferred: usize,
    lookahead: bool,
    window: Option<(usize, Vec<u8>)>,
}
// Require the exact interval (clamped only at EOF) and a numeric object length.
// Unknown totals cannot prove that skipped opaque blocks exist.
fn interval(response: &MediaResponse, start: usize, end: usize) -> Result<usize, &'static str> {
    if response.status != 206 {
        return Err(RANGE);
    }
    let value = response.content_range.as_deref().ok_or(RANGE)?;
    let (span, total) = value
        .strip_prefix("bytes ")
        .ok_or(RANGE)?
        .split_once('/')
        .ok_or(RANGE)?;
    let (first, last) = span.split_once('-').ok_or(RANGE)?;
    fn decimal(s: &str) -> Result<usize, &'static str> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(RANGE);
        }
        s.parse().map_err(|_| RANGE)
    }
    let (first, last, total) = (decimal(first)?, decimal(last)?, decimal(total)?);
    if total == 0 || first != start || start >= total || last != end.min(total - 1) {
        return Err(RANGE);
    }
    let length = last - first + 1;
    if response.content_length.is_some_and(|n| n != length as u64) {
        return Err(RANGE);
    }
    Ok(total)
}
async fn body(
    mut response: MediaResponse,
    length: usize,
    transferred: &mut usize,
) -> Result<Vec<u8>, &'static str> {
    if length > BYTES - *transferred {
        return Err(LIMIT);
    }
    let mut bytes = Vec::with_capacity(length);
    while let Some(chunk) = response.body.next().await {
        let chunk = chunk.map_err(|_| "Embedded tag read interrupted")?;
        if chunk.len() > BYTES - *transferred {
            return Err(LIMIT);
        }
        *transferred += chunk.len();
        if chunk.len() > length - bytes.len() {
            return Err(RANGE);
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != length {
        return Err(INCOMPLETE);
    }
    Ok(bytes)
}
impl Reader<'_> {
    async fn get(&mut self, start: usize, size: usize) -> Result<Vec<u8>, &'static str> {
        // File offsets may cross large audio payloads. Container-specific
        // metadata spans retain their own caps; response bytes/requests do not.
        let end = start.checked_add(size).ok_or(LIMIT)?;
        if end > self.total {
            return Err(INCOMPLETE);
        }
        if let Some(bytes) = self.prefix.get(start..end) {
            return Ok(bytes.to_vec());
        }
        if self.lookahead
            && let Some((base, bytes)) = &self.window
            && start >= *base
            && end - base <= bytes.len()
        {
            return Ok(bytes[start - base..end - base].to_vec());
        }
        if size == 0 {
            return Ok(Vec::new());
        }
        if self.requests >= REQUESTS || size > BYTES - self.transferred {
            return Err(LIMIT);
        }
        self.requests += 1;
        // New container walkers revisit adjacent tiny headers. A single bounded
        // window avoids dozens of network round trips for a tail moov/APE tag.
        // Native FLAC retains its exact sparse reads and existing qualification.
        let (first, last) = if self.lookahead && size <= 4096 {
            let first = start / 4096 * 4096;
            let last = first
                .checked_add(4096)
                .unwrap_or(self.total)
                .max(end)
                .min(self.total);
            if last - first <= BYTES - self.transferred {
                (first, last)
            } else {
                (start, end)
            }
        } else {
            (start, end)
        };
        let range = ByteRange::try_from(format!("bytes={first}-{}", last - 1).as_str()).unwrap();
        let response = self
            .stream
            .open(Some(range), false)
            .await
            .map_err(|_| "Embedded tags could not be read")?;
        let total = interval(&response, first, last - 1)?;
        if total != self.total || response.etag != self.etag {
            return Err(CHANGED);
        }
        let bytes = body(response, last - first, &mut self.transferred).await?;
        if self.lookahead {
            let result = bytes[start - first..end - first].to_vec();
            self.window = Some((first, bytes));
            Ok(result)
        } else {
            Ok(bytes)
        }
    }
    async fn scan(&mut self) -> Result<Tags, &'static str> {
        if self.prefix.len() < 4 {
            return Err(INCOMPLETE);
        }
        if self.prefix.starts_with(b"fLaC") {
            return self.flac().await;
        }
        self.lookahead = true;
        if self.prefix.starts_with(b"OggS") {
            return ogg::scan(self).await;
        }
        if [b"RIFF", b"RIFX", b"RF64", b"BW64"]
            .iter()
            .any(|magic| self.prefix.starts_with(*magic))
        {
            return wave::scan(self).await;
        }
        if self.prefix.get(4..8) == Some(b"ftyp") {
            return mp4::scan(self).await;
        }
        if self.prefix.starts_with(b"ID3") || audio_signature(&self.prefix) {
            return id3::scan(self).await;
        }
        Err("Embedded tags unavailable for this format")
    }
    async fn flac(&mut self) -> Result<Tags, &'static str> {
        signature(&self.prefix)?;
        let mut pos = 4;
        let mut found = None;
        for index in 0..BLOCKS {
            let header = self.get(pos, 4).await?;
            let block = Block::parse(&header, index)?;
            pos = end_offset(pos, 4)?;
            let end = end_offset(pos, block.size)?;
            if end > self.total {
                return Err(INCOMPLETE);
            }
            if block.kind == 4 {
                if found.is_some() {
                    return Err("Duplicate embedded FLAC comment blocks");
                }
                store_comments(&mut found, &self.get(pos, block.size).await?)?;
            }
            if block.last {
                return Ok(found.unwrap_or_default());
            }
            pos = end;
        }
        Err(LIMIT)
    }
}
fn audio_signature(bytes: &[u8]) -> bool {
    bytes.get(..4).is_some_and(|head| {
        head[0] == 0xff && head[1] & 0xe0 == 0xe0
            || head == b"wvpk"
            || head == b"MAC "
            || head == b"MPCK"
            || head.starts_with(b"MP+")
    })
}
pub(super) async fn probe(stream: &dyn MediaSource) -> Result<Tags, &'static str> {
    let range = ByteRange::try_from("bytes=0-65535").unwrap();
    let mut response = stream
        .open(Some(range), false)
        .await
        .map_err(|_| "Embedded tags could not be read")?;
    if response.status == 200 {
        let mut bytes = Vec::with_capacity(BYTES);
        while let Some(chunk) = response.body.next().await {
            let chunk = chunk.map_err(|_| "Embedded tag read interrupted")?;
            if chunk.len() > BYTES - bytes.len() {
                return Err(LIMIT);
            }
            bytes.extend_from_slice(&chunk);
            if bytes.starts_with(b"fLaC") {
                let result = tags(&bytes);
                if !matches!(result, Err(INCOMPLETE)) {
                    return result;
                }
            }
        }
        if response
            .content_length
            .is_some_and(|length| length != bytes.len() as u64)
        {
            return Err(INCOMPLETE);
        }
        let total = bytes.len();
        return Reader {
            stream,
            prefix: bytes,
            total,
            etag: response.etag,
            requests: 1,
            transferred: total,
            lookahead: false,
            window: None,
        }
        .scan()
        .await;
    }
    let total = interval(&response, 0, PREFIX - 1)?;
    let etag = response.etag.clone();
    let mut transferred = 0;
    let prefix = body(response, total.min(PREFIX), &mut transferred).await?;
    Reader {
        stream,
        prefix,
        total,
        etag,
        requests: 1,
        transferred,
        lookahead: false,
        window: None,
    }
    .scan()
    .await
}

#[cfg(test)]
mod tests {
    mod formats;
    use super::*;
    use bytes::Bytes;
    use qoda_provider::{ProviderError, ProviderFuture};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    #[derive(Clone, Copy, Default)]
    enum Fault {
        #[default]
        None,
        Ignore,
        IgnoreSeek,
        WrongStart,
        WrongEnd,
        UnknownTotal,
        ChangedTotal,
        ChangedEtag,
        MissingEtag,
        WrongLength,
        Truncated,
        ExtraByte,
        Interrupted,
        Hanging,
        HangingSeek,
    }
    struct Source {
        bytes: Bytes,
        fault: Fault,
        ranges: Mutex<Vec<(usize, usize)>>,
        dropped: Arc<AtomicBool>,
    }
    impl Source {
        fn new(bytes: Vec<u8>, fault: Fault) -> Self {
            Self {
                bytes: bytes.into(),
                fault,
                ranges: Mutex::new(Vec::new()),
                dropped: Arc::new(AtomicBool::new(false)),
            }
        }
    }
    struct Hanging(Arc<AtomicBool>);
    impl futures_util::Stream for Hanging {
        type Item = Result<Bytes, ProviderError>;
        fn poll_next(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            std::task::Poll::Pending
        }
    }
    impl Drop for Hanging {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    impl MediaSource for Source {
        fn open(&self, range: Option<ByteRange>, head: bool) -> ProviderFuture<'_, MediaResponse> {
            Box::pin(async move {
                assert!(!head);
                let range = range.unwrap();
                let (start, end) = range
                    .header()
                    .strip_prefix("bytes=")
                    .unwrap()
                    .split_once('-')
                    .unwrap();
                let (start, end) = (
                    start.parse::<usize>().unwrap(),
                    end.parse::<usize>().unwrap().min(self.bytes.len() - 1),
                );
                self.ranges.lock().unwrap().push((start, end));
                let mut response = MediaResponse {
                    status: 206,
                    content_type: None,
                    content_length: Some((end - start + 1) as u64),
                    content_range: Some(format!("bytes {start}-{end}/{}", self.bytes.len())),
                    etag: Some("\"fixture-v1\"".into()),
                    body: futures_util::stream::iter(
                        self.bytes
                            .slice(start..=end)
                            .chunks(11)
                            .map(Bytes::copy_from_slice)
                            .map(Ok)
                            .collect::<Vec<_>>(),
                    )
                    .boxed(),
                };
                match self.fault {
                    Fault::Ignore | Fault::IgnoreSeek
                        if start != 0 || matches!(self.fault, Fault::Ignore) =>
                    {
                        response.status = 200;
                        response.content_range = None;
                        response.content_length = Some(self.bytes.len() as u64);
                        response.body = futures_util::stream::iter(
                            self.bytes
                                .chunks(997)
                                .map(Bytes::copy_from_slice)
                                .map(Ok)
                                .collect::<Vec<_>>(),
                        )
                        .boxed();
                    }
                    Fault::WrongStart => {
                        response.content_range =
                            Some(format!("bytes {}-{end}/{}", start + 1, self.bytes.len()))
                    }
                    Fault::WrongEnd => {
                        response.content_range =
                            Some(format!("bytes {start}-{}/{}", end - 1, self.bytes.len()))
                    }
                    Fault::UnknownTotal => {
                        response.content_range = Some(format!("bytes {start}-{end}/*"))
                    }
                    Fault::ChangedTotal if start > 0 => {
                        response.content_range =
                            Some(format!("bytes {start}-{end}/{}", self.bytes.len() + 1))
                    }
                    Fault::ChangedEtag if start > 0 => {
                        response.etag = Some("\"fixture-v2\"".into())
                    }
                    Fault::MissingEtag if start > 0 => response.etag = None,
                    Fault::WrongLength => response.content_length = Some(1),
                    Fault::Truncated => {
                        response.body =
                            futures_util::stream::iter([Ok(self.bytes.slice(start..end))]).boxed()
                    }
                    Fault::ExtraByte => {
                        response.body = futures_util::stream::iter([
                            Ok(self.bytes.slice(start..=end)),
                            Ok(Bytes::from_static(b"!")),
                        ])
                        .boxed()
                    }
                    Fault::Interrupted => {
                        response.body =
                            futures_util::stream::iter([Err(ProviderError::Network)]).boxed()
                    }
                    Fault::Hanging | Fault::HangingSeek
                        if start > 0 || matches!(self.fault, Fault::Hanging) =>
                    {
                        response.body = Hanging(self.dropped.clone()).boxed()
                    }
                    _ => (),
                }
                Ok(response)
            })
        }
    }
    fn block(bytes: &mut Vec<u8>, kind: u8, last: bool, payload: &[u8]) {
        let size = payload.len();
        assert!(size < 1 << 24);
        bytes.extend([
            kind | if last { 128 } else { 0 },
            (size >> 16) as u8,
            (size >> 8) as u8,
            size as u8,
        ]);
        bytes.extend(payload);
    }
    fn base() -> Vec<u8> {
        let mut bytes = b"fLaC".to_vec();
        block(&mut bytes, 0, false, &[0; 34]);
        bytes
    }
    fn comment() -> Vec<u8> {
        let mut bytes = 0u32.to_le_bytes().to_vec();
        bytes.extend(2u32.to_le_bytes());
        for s in ["REPLAYGAIN_TRACK_GAIN=-6 dB", "REPLAYGAIN_TRACK_PEAK=0.8"] {
            bytes.extend((s.len() as u32).to_le_bytes());
            bytes.extend(s.as_bytes());
        }
        bytes
    }
    fn picture() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(3u32.to_be_bytes()); // front cover
        bytes.extend(10u32.to_be_bytes());
        bytes.extend(b"image/jpeg");
        bytes.extend(0u32.to_be_bytes()); // empty description
        for value in [512u32, 512, 24, 0] {
            bytes.extend(value.to_be_bytes());
        }
        bytes.extend((2u32 * 1024 * 1024).to_be_bytes());
        bytes.resize(bytes.len() + 2 * 1024 * 1024, 0x55); // opaque synthetic artwork
        bytes
    }
    fn fixture(after: bool) -> Vec<u8> {
        let mut bytes = base();
        if after {
            block(&mut bytes, 4, false, &comment());
        }
        block(&mut bytes, 6, after, &picture());
        if !after {
            block(&mut bytes, 4, true, &comment());
        }
        bytes.extend(b"synthetic audio is never decoded");
        bytes
    }
    #[tokio::test]
    async fn sparse_artwork_before_or_after_comments_does_not_download_the_picture() {
        for after in [false, true] {
            let source = Source::new(fixture(after), Fault::None);
            let tags = probe(&source).await.unwrap();
            assert_eq!(tags.track, Some(-6.0));
            assert_eq!(tags.track_peak, Some(0.8));
            let ranges = source.ranges.lock().unwrap();
            let transferred: usize = ranges.iter().map(|(a, b)| b - a + 1).sum();
            assert!(transferred < PREFIX + 256, "{ranges:?}");
            assert_eq!(ranges.len(), if after { 1 } else { 3 });
        }
    }
    #[tokio::test]
    async fn duplicate_comments_after_unread_artwork_are_rejected() {
        let mut bytes = fixture(false);
        // Prepend an earlier comment without changing the valid later framing.
        let mut early = base();
        block(&mut early, 4, false, &comment());
        early.extend(bytes.drain(42..));
        let source = Source::new(early, Fault::None);
        assert_eq!(
            probe(&source).await.err(),
            Some("Duplicate embedded FLAC comment blocks")
        );
        assert!(source.ranges.lock().unwrap().len() > 1);
    }
    #[tokio::test]
    async fn inconsistent_partial_responses_and_ignored_seeks_cannot_supply_gain() {
        for fault in [
            Fault::IgnoreSeek,
            Fault::WrongStart,
            Fault::WrongEnd,
            Fault::UnknownTotal,
            Fault::ChangedTotal,
            Fault::ChangedEtag,
            Fault::MissingEtag,
            Fault::WrongLength,
            Fault::Truncated,
            Fault::ExtraByte,
            Fault::Interrupted,
        ] {
            let source = Source::new(fixture(false), fault);
            let applied =
                super::super::select(super::super::Mode::Track, false, probe(&source).await);
            assert_eq!(applied.basis, "unity");
            assert_ne!(
                applied.reason,
                "No valid embedded ReplayGain tag; unity gain"
            );
        }
    }
    #[tokio::test]
    async fn ignored_initial_range_accepts_only_complete_bounded_metadata() {
        let mut bytes = base();
        block(&mut bytes, 4, true, &comment());
        assert_eq!(
            probe(&Source::new(bytes, Fault::Ignore))
                .await
                .unwrap()
                .track,
            Some(-6.0)
        );
        let source = Source::new(fixture(false), Fault::Ignore);
        assert!(probe(&source).await.is_err());
        assert_eq!(source.ranges.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn truncation_and_invalid_framing_never_establish_missing_tags() {
        let mut bytes = base();
        block(&mut bytes, 4, true, &comment());
        for len in 1..bytes.len() {
            assert!(
                probe(&Source::new(bytes[..len].to_vec(), Fault::None))
                    .await
                    .is_err(),
                "{len}"
            );
        }
        for (offset, value) in [(4, 4), (7, 33), (42, 127), (42, 0)] {
            let mut invalid = bytes.clone();
            invalid[offset] = value;
            assert!(probe(&Source::new(invalid, Fault::None)).await.is_err());
        }
        // A valid early comment cannot hide a later truncated final picture.
        let mut truncated_tail = fixture(true);
        truncated_tail.truncate(truncated_tail.len() - 64);
        assert!(
            probe(&Source::new(truncated_tail, Fault::None))
                .await
                .is_err()
        );
        let mut missing = base();
        block(&mut missing, 1, true, &[0; 4]);
        assert_eq!(
            probe(&Source::new(missing, Fault::None))
                .await
                .unwrap()
                .track,
            None
        );
    }
    #[tokio::test]
    async fn block_request_and_payload_budgets_bound_the_operation() {
        let mut many = base();
        for i in 0..BLOCKS {
            block(&mut many, 1, i == BLOCKS - 1, &[]);
        }
        assert_eq!(
            probe(&Source::new(many, Fault::None)).await.err(),
            Some(LIMIT)
        );
        let mut many = base();
        for _ in 1..BLOCKS {
            block(&mut many, 1, false, &vec![0; PREFIX + 1]);
        }
        block(&mut many, 4, true, &comment());
        let source = Source::new(many, Fault::None);
        assert_eq!(probe(&source).await.err(), Some(LIMIT));
        assert!(source.ranges.lock().unwrap().len() <= REQUESTS);
        let mut oversized = base();
        block(&mut oversized, 4, true, &vec![0; BYTES]);
        let source = Source::new(oversized, Fault::None);
        assert_eq!(probe(&source).await.err(), Some(LIMIT));
        assert_eq!(source.ranges.lock().unwrap().len(), 1);
        assert_eq!(end_offset(SPAN - 1, 2), Err(LIMIT));
    }
    #[tokio::test(start_paused = true)]
    async fn preparation_timeout_cancels_body_and_off_never_opens_it() {
        use super::super::{Mode, Request, describe};
        use qoda_core::{SourceId, TrackRef};
        use qoda_provider::{PreparedMedia, Track};
        for (mode, fault) in [
            (Mode::Track, Fault::Hanging),
            (Mode::Track, Fault::HangingSeek),
            (Mode::Off, Fault::Hanging),
        ] {
            let source = Source::new(fixture(false), fault);
            let dropped = source.dropped.clone();
            let reference = TrackRef {
                source: SourceId::new(),
                key: "fixture".to_owned().try_into().unwrap(),
            };
            let media = PreparedMedia {
                track: Track {
                    reference: reference.clone(),
                    title: "Synthetic".into(),
                    artist: "Fixture".into(),
                    album: "Fixture".into(),
                    duration_ms: None,
                },
                version: "pinned-fixture".into(),
                stream: Box::new(source),
            };
            let start = tokio::time::Instant::now();
            let prepared = describe(
                media,
                Request {
                    reference,
                    mode,
                    album_session: false,
                    fidelity: false,
                },
            )
            .await
            .unwrap();
            assert_eq!(prepared.media.version, "pinned-fixture");
            if mode == Mode::Off {
                assert_eq!(prepared.gain.as_ref().unwrap().basis, "off");
                assert_eq!(start.elapsed(), std::time::Duration::ZERO);
                assert!(!dropped.load(Ordering::SeqCst));
            } else {
                assert_eq!(prepared.gain.as_ref().unwrap().basis, "unity");
                assert_eq!(
                    prepared.gain.as_ref().unwrap().reason,
                    "Embedded tag read timed out; unity gain"
                );
                assert_eq!(start.elapsed(), std::time::Duration::from_secs(5));
                assert!(dropped.load(Ordering::SeqCst));
            }
        }
    }
}
