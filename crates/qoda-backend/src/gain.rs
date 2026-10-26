//! Embedded gain metadata, pure selection/clipping policy and private preferences.
//! No Plex loudness fields, decoder events or native ReplayGain are used here.
use futures_util::StreamExt;
use qoda_core::TrackRef;
use qoda_provider::{ByteRange, MediaSource, MusicProvider, PreparedMedia, ProviderError};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::PathBuf, time::Duration};

const INCOMPLETE: &str = "Embedded tags incomplete or beyond the scan limit";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Off,
    Track,
    Album,
    #[default]
    Auto,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Applied {
    pub mode: Mode,
    pub basis: &'static str,
    pub db: f64,
    pub limited: bool,
    pub reason: &'static str,
}
impl Applied {
    pub fn off() -> Self {
        Self {
            mode: Mode::Off,
            basis: "off",
            db: 0.0,
            limited: false,
            reason: "ReplayGain off",
        }
    }
    pub fn filter(&self) -> String {
        if self.db == 0.0 {
            "af=".into()
        } else {
            format!(
                "af=lavfi=[volume={:.17e}:precision=double]",
                10_f64.powf(self.db / 20.0)
            )
        }
    }
}
#[derive(Clone, Debug, Default)]
struct Tags {
    track: Option<f64>,
    track_peak: Option<f64>,
    album: Option<f64>,
    album_peak: Option<f64>,
}
fn number(value: &str, gain: bool) -> Option<f64> {
    let value = value.trim();
    let value = if gain
        && value.len() >= 2
        && value.as_bytes()[value.len() - 2..].eq_ignore_ascii_case(b"db")
    {
        value[..value.len() - 2].trim()
    } else {
        value
    };
    let n = value.parse::<f64>().ok()?;
    (n.is_finite()
        && if gain {
            (-120.0..=120.0).contains(&n)
        } else {
            n > 0.0
        })
    .then_some(n)
}
// FLAC's metadata framing and Vorbis comment length fields are checked before
// slicing or allocating. A partial prefix never establishes missing tags.
fn tags(bytes: &[u8]) -> Result<Tags, &'static str> {
    if bytes.len() < 4 {
        return Err(INCOMPLETE);
    }
    if !bytes.starts_with(b"fLaC") {
        return Err("Embedded tags unavailable for this format");
    }
    let mut pos = 4usize;
    for _ in 0..1024 {
        let header = bytes.get(pos..pos + 4).ok_or(INCOMPLETE)?;
        let size =
            usize::from(header[1]) << 16 | usize::from(header[2]) << 8 | usize::from(header[3]);
        pos += 4;
        let block = bytes.get(pos..pos + size).ok_or(INCOMPLETE)?;
        if header[0] & 127 == 4 {
            return comments(block);
        }
        if header[0] & 128 != 0 {
            return Ok(Tags::default());
        }
        pos += size;
    }
    Err("Embedded tag scan exceeded its block limit")
}
fn comments(mut bytes: &[u8]) -> Result<Tags, &'static str> {
    fn take<'a>(bytes: &mut &'a [u8], size: usize) -> Result<&'a [u8], &'static str> {
        let value = bytes.get(..size).ok_or("Invalid embedded tags")?;
        *bytes = &bytes[size..];
        Ok(value)
    }
    fn size(bytes: &mut &[u8]) -> Result<usize, &'static str> {
        Ok(u32::from_le_bytes(take(bytes, 4)?.try_into().unwrap()) as usize)
    }
    let vendor = size(&mut bytes)?;
    take(&mut bytes, vendor)?;
    let count = size(&mut bytes)?;
    if count > 16384 {
        return Err("Embedded tag count exceeds limit");
    }
    let mut fields: [Option<f64>; 4] = [None; 4];
    let mut seen = [false; 4];
    for _ in 0..count {
        let len = size(&mut bytes)?;
        let text =
            std::str::from_utf8(take(&mut bytes, len)?).map_err(|_| "Invalid embedded tags")?;
        let Some((key, value)) = text.split_once('=') else {
            continue;
        };
        let index = [
            "REPLAYGAIN_TRACK_GAIN",
            "REPLAYGAIN_TRACK_PEAK",
            "REPLAYGAIN_ALBUM_GAIN",
            "REPLAYGAIN_ALBUM_PEAK",
        ]
        .iter()
        .position(|name| key.eq_ignore_ascii_case(name));
        if let Some(i) = index {
            // Conflicting or duplicated fields never supply an actionable gain/peak.
            fields[i] = if seen[i] {
                None
            } else {
                number(value, i % 2 == 0)
            };
            seen[i] = true;
        }
    }
    if !bytes.is_empty() {
        return Err("Invalid embedded tags");
    }
    Ok(Tags {
        track: fields[0],
        track_peak: fields[1],
        album: fields[2],
        album_peak: fields[3],
    })
}
fn select(mode: Mode, album_session: bool, tags: Result<Tags, &'static str>) -> Applied {
    if mode == Mode::Off {
        return Applied::off();
    }
    let tags = match tags {
        Ok(tags) => tags,
        Err(reason) => {
            return Applied {
                mode,
                basis: "unity",
                db: 0.0,
                limited: false,
                reason,
            };
        }
    };
    let album = mode == Mode::Album || mode == Mode::Auto && album_session;
    let chosen = if album {
        tags.album.map(|v| (v, tags.album_peak, "album"))
    } else {
        None
    }
    .or_else(|| tags.track.map(|v| (v, tags.track_peak, "track")));
    let Some((gain, peak, basis)) = chosen else {
        return Applied {
            mode,
            basis: "unity",
            db: 0.0,
            limited: false,
            reason: "No valid embedded ReplayGain tag; unity gain",
        };
    };
    let ceiling = peak.map(|p| -20.0 * p.log10()).unwrap_or(0.0);
    let db = gain.min(ceiling);
    Applied {
        mode,
        basis,
        db,
        limited: db < gain,
        reason: if db < gain {
            if peak.is_some() {
                "Gain capped by the matching embedded peak"
            } else {
                "Unknown matching peak; boost capped at unity"
            }
        } else if album && basis == "track" {
            "Album gain unavailable; using embedded track gain"
        } else {
            "Embedded FLAC ReplayGain; preamp 0 dB"
        },
    }
}
async fn probe(stream: &dyn MediaSource) -> Result<Tags, &'static str> {
    const LIMIT: usize = 1024 * 1024;
    let range = ByteRange::try_from("bytes=0-1048575").unwrap();
    let mut response = stream
        .open(Some(range), false)
        .await
        .map_err(|_| "Embedded tags could not be read")?;
    if response.status != 200 && response.status != 206 {
        return Err("Embedded tags could not be read");
    }
    if response.status == 206
        && !response
            .content_range
            .as_deref()
            .is_some_and(|s| s.starts_with("bytes 0-"))
    {
        return Err("Embedded tag range was not confirmed");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.body.next().await {
        let chunk = chunk.map_err(|_| "Embedded tag read interrupted")?;
        let size = chunk.len().min(LIMIT - bytes.len());
        bytes.extend_from_slice(&chunk[..size]);
        if bytes.len() >= 4 && !bytes.starts_with(b"fLaC") {
            return tags(&bytes);
        }
        if bytes.len() < 4 {
            continue;
        }
        // Stop as soon as a complete tag block (or final metadata block) exists.
        let result = tags(&bytes);
        if !matches!(result, Err(INCOMPLETE)) {
            return result;
        }
        if bytes.len() == LIMIT {
            break;
        }
    }
    tags(&bytes)
}
pub struct Prepared {
    pub media: PreparedMedia,
    pub gain: Applied,
    pub source: Option<crate::source_format::Format>,
}
#[derive(Clone, Copy, Debug)]
pub enum PrepareError {
    Provider(ProviderError),
    Fidelity(&'static str),
}
impl PrepareError {
    pub fn provider(self) -> Option<ProviderError> {
        match self {
            Self::Provider(error) => Some(error),
            Self::Fidelity(_) => None,
        }
    }
}
impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(error) => write!(f, "{error}"),
            Self::Fidelity(error) => f.write_str(error),
        }
    }
}
pub struct Request {
    pub reference: TrackRef,
    pub mode: Mode,
    pub album_session: bool,
    pub fidelity: bool,
}
pub async fn prepare(
    provider: &dyn MusicProvider,
    request: Request,
) -> Result<Prepared, PrepareError> {
    let Request {
        reference,
        mode,
        album_session,
        fidelity,
    } = request;
    let media = provider
        .prepare(reference.source, reference.key)
        .await
        .map_err(PrepareError::Provider)?;
    let source = if fidelity {
        Some(
            tokio::time::timeout(
                Duration::from_secs(5),
                crate::source_format::probe(media.stream.as_ref()),
            )
            .await
            .unwrap_or(Err("Source verification timed out; fidelity stays paused."))
            .map_err(PrepareError::Fidelity)?,
        )
    } else {
        None
    };
    let gain = if mode == Mode::Off {
        Applied::off()
    } else {
        let tags = tokio::time::timeout(Duration::from_secs(5), probe(media.stream.as_ref()))
            .await
            .unwrap_or(Err("Embedded tag read timed out; unity gain"));
        select(mode, album_session, tags)
    };
    Ok(Prepared {
        media,
        gain,
        source,
    })
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u8,
    mode: Mode,
}
pub struct Settings {
    pub mode: Mode,
    issue: Option<&'static str>,
    path: Option<PathBuf>,
}
impl Settings {
    pub fn load(path: Option<PathBuf>) -> Self {
        let read = || {
            let path = path.as_ref().ok_or("No audio settings directory")?;
            let dir = crate::cache_files::Directory::open(path)?;
            let Some(file) = dir.file("audio.json")? else {
                return Ok(Mode::Auto);
            };
            if file
                .metadata()
                .map_err(|_| "Audio settings unavailable")?
                .len()
                > 4096
            {
                return Err("Audio settings are too large; preserved");
            }
            let prefs: Preferences = serde_json::from_reader(file.take(4097))
                .map_err(|_| "Invalid audio settings preserved")?;
            if prefs.version != 1 {
                return Err("Unsupported audio settings preserved");
            }
            Ok(prefs.mode)
        };
        match read() {
            Ok(mode) => Self {
                mode,
                path,
                issue: None,
            },
            Err(issue) => Self {
                mode: Mode::Off,
                path,
                issue: Some(issue),
            },
        }
    }
    pub fn set(&mut self, mode: Mode) -> Result<(), &'static str> {
        if let Some(issue) = Self::load(self.path.clone()).issue {
            return Err(issue);
        }
        if let Some(issue) = self.issue {
            return Err(issue);
        }
        let dir = crate::cache_files::Directory::open(
            self.path.as_ref().ok_or("No audio settings directory")?,
        )?;
        let bytes = serde_json::to_vec(&Preferences { version: 1, mode })
            .map_err(|_| "Audio settings unavailable")?;
        dir.atomic("audio.json", &bytes)?;
        self.mode = mode;
        Ok(())
    }
    pub fn view(&self) -> serde_json::Value {
        serde_json::json!({"mode":self.mode,"applied":null,"message":self.issue.unwrap_or("Changes apply to upcoming preparations; restart/reselect a track to apply now."),"available":self.issue.is_none(),"metadata_scope":"Embedded FLAC ReplayGain; other formats use unity when tags are unavailable"})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn flac(values: &[&str]) -> Vec<u8> {
        let mut block = Vec::new();
        block.extend(0_u32.to_le_bytes());
        block.extend((values.len() as u32).to_le_bytes());
        for text in values {
            block.extend((text.len() as u32).to_le_bytes());
            block.extend(text.as_bytes());
        }
        let mut bytes = b"fLaC\x00\x00\x00\x22".to_vec();
        bytes.extend([0; 34]);
        let size = block.len() as u32;
        bytes.extend([0x84, (size >> 16) as u8, (size >> 8) as u8, size as u8]);
        bytes.extend(block);
        bytes
    }
    #[test]
    fn zero_is_measured_and_duplicate_or_nonfinite_fields_cannot_supply_gain() {
        let measured = select(
            Mode::Track,
            false,
            tags(&flac(&["replaygain_track_gain=0 dB"])),
        );
        assert_eq!(measured.basis, "track");
        assert_eq!(measured.db, 0.0);
        for value in ["NaN", "inf", "121 dB", "-121 dB", "💥", "not gain"] {
            assert!(number(value, true).is_none());
        }
        for value in ["NaN", "inf", "0", "-1"] {
            assert!(number(value, false).is_none());
        }
        let duplicate = select(
            Mode::Track,
            false,
            tags(&flac(&[
                "REPLAYGAIN_TRACK_GAIN=-6 dB",
                "REPLAYGAIN_TRACK_GAIN=-3 dB",
            ])),
        );
        assert_eq!(duplicate.basis, "unity");
        assert!(
            tags(&flac(&["REPLAYGAIN_TRACK_GAIN=-6 dB"]))
                .unwrap()
                .track
                .is_some()
        );
    }
    #[test]
    fn auto_fallbacks_and_matching_peak_provenance_are_explicit() {
        let values = tags(&flac(&[
            "REPLAYGAIN_TRACK_GAIN=-6 dB",
            "REPLAYGAIN_ALBUM_GAIN=-12 dB",
            "REPLAYGAIN_TRACK_PEAK=0.5",
        ]))
        .unwrap();
        assert_eq!(select(Mode::Auto, false, Ok(values.clone())).db, -6.0);
        assert_eq!(select(Mode::Auto, true, Ok(values.clone())).db, -12.0);
        assert_eq!(select(Mode::Off, true, Ok(values)).basis, "off");
        let missing_album = tags(&flac(&[
            "REPLAYGAIN_TRACK_GAIN=6 dB",
            "REPLAYGAIN_TRACK_PEAK=0.5",
        ]));
        assert_eq!(select(Mode::Album, false, missing_album).db, 6.0);
        let no_album_peak = select(
            Mode::Album,
            true,
            tags(&flac(&[
                "REPLAYGAIN_ALBUM_GAIN=6 dB",
                "REPLAYGAIN_TRACK_PEAK=0.1",
            ])),
        );
        assert_eq!(no_album_peak.db, 0.0);
        assert!(no_album_peak.limited);
        let cap = select(
            Mode::Track,
            false,
            tags(&flac(&[
                "REPLAYGAIN_TRACK_GAIN=12 dB",
                "REPLAYGAIN_TRACK_PEAK=0.5",
            ])),
        );
        assert!((cap.db - 20_f64 * 2_f64.log10()).abs() < 1e-12);
        assert!(cap.limited);
    }
    #[test]
    fn bounded_prefix_parser_rejects_truncation_and_malformed_lengths_without_panicking() {
        let valid = flac(&["REPLAYGAIN_TRACK_GAIN=-6 dB"]);
        for n in 0..valid.len() {
            assert!(tags(&valid[..n]).is_err());
        }
        assert_eq!(tags(&flac(&[])).unwrap().track, None);
        let mut invalid = valid.clone();
        invalid[46..50].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(tags(&invalid).is_err());
        let mut invalid = valid;
        invalid.push(0); // Bytes after the complete tag block are audio, not extra comments.
        assert!(tags(&invalid).is_ok());
    }
    #[test]
    fn preferences_roundtrip_and_unknown_documents_are_preserved() {
        let path =
            std::env::temp_dir().join(format!("qoda-gain-settings-{}", uuid::Uuid::new_v4()));
        let mut settings = Settings::load(Some(path.clone()));
        assert_eq!(settings.mode, Mode::Auto);
        settings.set(Mode::Album).unwrap();
        assert_eq!(Settings::load(Some(path.clone())).mode, Mode::Album);
        let future = b"{\"version\":2,\"mode\":\"auto\"}";
        std::fs::write(path.join("audio.json"), future).unwrap();
        let mut settings = Settings::load(Some(path.clone()));
        assert!(settings.set(Mode::Off).is_err());
        assert_eq!(std::fs::read(path.join("audio.json")).unwrap(), future);
        std::fs::remove_dir_all(path).unwrap();
    }
    #[tokio::test]
    async fn real_flac_tags_flow_through_preparation_and_off_does_not_probe() {
        use qoda_provider::{CatalogPage, Library, MediaResponse, ProviderFuture, Track};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Source {
            bytes: bytes::Bytes,
            reads: Arc<AtomicUsize>,
        }
        impl MediaSource for Source {
            fn open(&self, range: Option<ByteRange>, _: bool) -> ProviderFuture<'_, MediaResponse> {
                Box::pin(async move {
                    assert_eq!(
                        range.as_ref().map(ByteRange::header),
                        Some("bytes=0-1048575")
                    );
                    self.reads.fetch_add(1, Ordering::SeqCst);
                    Ok(MediaResponse {
                        status: 206,
                        content_type: None,
                        content_length: Some(self.bytes.len() as u64),
                        content_range: Some(format!(
                            "bytes 0-{}/{}",
                            self.bytes.len() - 1,
                            self.bytes.len()
                        )),
                        etag: None,
                        body: futures_util::stream::iter([
                            Ok(self.bytes.slice(..2)),
                            Ok(self.bytes.slice(2..)),
                        ])
                        .boxed(),
                    })
                })
            }
        }
        struct Provider {
            track: Track,
            bytes: bytes::Bytes,
            reads: Arc<AtomicUsize>,
        }
        impl MusicProvider for Provider {
            fn libraries(&self) -> Vec<Library> {
                vec![]
            }
            fn tracks(
                &self,
                _: qoda_core::SourceId,
                _: u64,
                _: String,
            ) -> ProviderFuture<'_, CatalogPage> {
                Box::pin(async { Err(ProviderError::Unsupported) })
            }
            fn prepare(
                &self,
                _: qoda_core::SourceId,
                _: qoda_core::ProviderTrackKey,
            ) -> ProviderFuture<'_, PreparedMedia> {
                Box::pin(async {
                    Ok(PreparedMedia {
                        track: self.track.clone(),
                        version: "pinned".into(),
                        stream: Box::new(Source {
                            bytes: self.bytes.clone(),
                            reads: self.reads.clone(),
                        }),
                    })
                })
            }
        }
        let path =
            std::env::temp_dir().join(format!("qoda-flac-tags-{}.flac", uuid::Uuid::new_v4()));
        let result = std::process::Command::new("/usr/bin/ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=48000:cl=stereo",
                "-t",
                "0.1",
                "-metadata",
                "REPLAYGAIN_TRACK_GAIN=3 dB",
                "-metadata",
                "REPLAYGAIN_TRACK_PEAK=0.8",
                "-metadata",
                "REPLAYGAIN_ALBUM_GAIN=-6 dB",
                "-c:a",
                "flac",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(result.status.success());
        let provider = Provider {
            track: Track {
                reference: TrackRef {
                    source: qoda_core::SourceId::new(),
                    key: "fixture".to_owned().try_into().unwrap(),
                },
                title: "Fictional".into(),
                artist: "Fixture".into(),
                album: "Fixture".into(),
                duration_ms: None,
            },
            bytes: std::fs::read(&path).unwrap().into(),
            reads: Arc::new(AtomicUsize::new(0)),
        };
        std::fs::remove_file(path).unwrap();
        let request = |mode, album_session| Request {
            reference: provider.track.reference.clone(),
            mode,
            album_session,
            fidelity: false,
        };
        let album = prepare(&provider, request(Mode::Auto, true)).await.unwrap();
        assert_eq!(album.gain.db, -6.0);
        assert_eq!(album.media.version, "pinned");
        let track = prepare(&provider, request(Mode::Auto, false))
            .await
            .unwrap();
        assert!(track.gain.limited);
        assert!((track.gain.db + 20.0 * 0.8_f64.log10()).abs() < 1e-12);
        let before = provider.reads.load(Ordering::SeqCst);
        assert_eq!(
            prepare(&provider, request(Mode::Off, false))
                .await
                .unwrap()
                .gain
                .basis,
            "off"
        );
        assert_eq!(provider.reads.load(Ordering::SeqCst), before);
    }
    #[tokio::test]
    async fn metadata_reads_are_bounded_even_when_the_source_ignores_range() {
        struct Source;
        impl MediaSource for Source {
            fn open(
                &self,
                _: Option<ByteRange>,
                _: bool,
            ) -> qoda_provider::ProviderFuture<'_, qoda_provider::MediaResponse> {
                Box::pin(async {
                    Ok(qoda_provider::MediaResponse {
                        status: 200,
                        content_type: None,
                        content_length: None,
                        content_range: None,
                        etag: None,
                        body: futures_util::stream::iter([
                            Ok(bytes::Bytes::from_static(b"fLaC\x04\xff\xff\xff")),
                            Ok(bytes::Bytes::from(vec![0; 1024 * 1024])),
                        ])
                        .boxed(),
                    })
                })
            }
        }
        assert!(probe(&Source).await.is_err());
    }
}
