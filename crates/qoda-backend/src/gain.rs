//! Embedded gain metadata, pure selection/clipping policy and private preferences.
//! No Plex loudness fields, decoder events or native ReplayGain are used here.
use qoda_core::TrackRef;
use qoda_provider::{MusicProvider, PreparedMedia, ProviderError};

mod reader;
use reader::probe;
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
    pub metadata: Option<Metadata>,
}
impl Applied {
    pub fn off() -> Self {
        Self {
            mode: Mode::Off,
            basis: "off",
            db: 0.0,
            limited: false,
            reason: "ReplayGain off",
            metadata: None,
        }
    }
    pub fn filter_graph(&self) -> Option<String> {
        (self.db != 0.0).then(|| {
            format!(
                "volume={:.17e}:precision=double",
                10_f64.powf(self.db / 20.0)
            )
        })
    }
    pub fn filter(&self) -> String {
        self.filter_graph()
            .map_or_else(|| "af=".into(), |graph| format!("af=lavfi=[{graph}]"))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TagSource {
    Flac,
    Id3,
    Ape,
    Mp4,
    Vorbis,
    Opus,
    WavId3,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldStatus {
    #[default]
    Missing,
    Invalid,
    Duplicate,
    Valid,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Metadata {
    pub source: Option<TagSource>,
    pub track_gain: FieldStatus,
    pub track_peak: FieldStatus,
    pub album_gain: FieldStatus,
    pub album_peak: FieldStatus,
}
#[derive(Clone, Debug, Default)]
struct Tags {
    track: Option<f64>,
    track_peak: Option<f64>,
    album: Option<f64>,
    album_peak: Option<f64>,
    metadata: Metadata,
}
impl Tags {
    fn has_fields(&self) -> bool {
        [
            self.metadata.track_gain,
            self.metadata.track_peak,
            self.metadata.album_gain,
            self.metadata.album_peak,
        ]
        .iter()
        .any(|status| *status != FieldStatus::Missing)
    }
}
const FIELD_NAMES: [&str; 4] = [
    "REPLAYGAIN_TRACK_GAIN",
    "REPLAYGAIN_TRACK_PEAK",
    "REPLAYGAIN_ALBUM_GAIN",
    "REPLAYGAIN_ALBUM_PEAK",
];
#[derive(Default)]
struct Fields {
    values: [Option<f64>; 4],
    statuses: [FieldStatus; 4],
    r128: bool,
}
impl Fields {
    fn index(key: &str) -> Option<usize> {
        FIELD_NAMES
            .iter()
            .position(|name| key.eq_ignore_ascii_case(name))
    }
    fn push(&mut self, key: &str, value: Option<&str>) {
        if key
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("R128_"))
        {
            self.r128 = true;
        }
        if let Some(index) = Self::index(key) {
            let parsed = value.and_then(|value| number(value, index % 2 == 0));
            if self.statuses[index] != FieldStatus::Missing {
                self.statuses[index] = FieldStatus::Duplicate;
                self.values[index] = None;
            } else {
                self.statuses[index] = if parsed.is_some() {
                    FieldStatus::Valid
                } else {
                    FieldStatus::Invalid
                };
                self.values[index] = parsed;
            }
        }
    }
    fn finish(self, source: TagSource) -> Tags {
        Tags {
            track: self.values[0],
            track_peak: self.values[1],
            album: self.values[2],
            album_peak: self.values[3],
            metadata: Metadata {
                source: Some(source),
                track_gain: self.statuses[0],
                track_peak: self.statuses[1],
                album_gain: self.statuses[2],
                album_peak: self.statuses[3],
            },
        }
    }
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
fn comment_fields(mut bytes: &[u8]) -> Result<(Fields, &[u8]), &'static str> {
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
    let mut fields = Fields::default();
    for _ in 0..count {
        let len = size(&mut bytes)?;
        let text =
            std::str::from_utf8(take(&mut bytes, len)?).map_err(|_| "Invalid embedded tags")?;
        if let Some((key, value)) = text.split_once('=') {
            fields.push(key, Some(value));
        } else {
            fields.push(text, None);
        }
    }
    Ok((fields, bytes))
}
fn comments(bytes: &[u8]) -> Result<Tags, &'static str> {
    let (fields, bytes) = comment_fields(bytes)?;
    if !bytes.is_empty() {
        return Err("Invalid embedded tags");
    }
    Ok(fields.finish(TagSource::Flac))
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
                metadata: None,
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
            metadata: Some(tags.metadata),
        };
    };
    let ceiling = peak.map(|p| -20.0 * p.log10()).unwrap_or(0.0);
    let db = gain.min(ceiling);
    Applied {
        mode,
        basis,
        db,
        limited: db < gain,
        metadata: Some(tags.metadata),
        reason: if db < gain {
            if peak.is_some() {
                "Gain capped by the matching embedded peak"
            } else {
                "Unknown matching peak; boost capped at unity"
            }
        } else if album && basis == "track" {
            "Album gain unavailable; using embedded track gain"
        } else {
            match tags.metadata.source {
                Some(TagSource::Flac) => "Embedded FLAC ReplayGain; preamp 0 dB",
                Some(TagSource::Id3) => "Embedded ID3 ReplayGain; preamp 0 dB",
                Some(TagSource::Ape) => "Embedded APEv2 ReplayGain; preamp 0 dB",
                Some(TagSource::Mp4) => "Embedded MP4 ReplayGain; preamp 0 dB",
                Some(TagSource::Vorbis) => "Embedded Vorbis ReplayGain; preamp 0 dB",
                Some(TagSource::Opus) => "Embedded Opus ReplayGain; preamp 0 dB",
                Some(TagSource::WavId3) => "Embedded WAV ID3 ReplayGain; preamp 0 dB",
                None => "Embedded ReplayGain; preamp 0 dB",
            }
        },
    }
}
pub struct Prepared {
    pub media: PreparedMedia,
    pub gain: Applied,
    pub source: Option<crate::source_format::Format>,
    pub continuous: Option<crate::continuous::Description>,
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
    let media = provider
        .prepare(request.reference.source, request.reference.key.clone())
        .await
        .map_err(PrepareError::Provider)?;
    describe(media, request).await
}
// Recovery verifies pinned provider identity before reading source/tag bytes.
pub async fn describe(media: PreparedMedia, request: Request) -> Result<Prepared, PrepareError> {
    let Request {
        mode,
        album_session,
        fidelity,
        ..
    } = request;
    let verified = if fidelity {
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
    let source = verified.map(|verified| verified.format);
    let continuous = verified.and_then(|verified| verified.continuous);
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
        continuous,
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
            let dir = crate::cache_files::Directory::open_settings(path)?;
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
        let dir = crate::cache_files::Directory::open_settings(
            self.path.as_ref().ok_or("No audio settings directory")?,
        )?;
        let bytes = serde_json::to_vec(&Preferences { version: 1, mode })
            .map_err(|_| "Audio settings unavailable")?;
        dir.atomic("audio.json", &bytes)?;
        self.mode = mode;
        Ok(())
    }
    pub fn view(&self) -> serde_json::Value {
        serde_json::json!({"mode":self.mode,"applied":null,"message":self.issue.unwrap_or("Changes apply to upcoming preparations; restart/reselect a track to apply now."),"available":self.issue.is_none(),"metadata_scope":"Embedded ReplayGain; unsupported or unavailable tags use unity"})
    }
}

#[cfg(test)]
mod tests {
    use super::reader::tags;
    use super::*;
    use futures_util::StreamExt;
    use qoda_provider::{ByteRange, MediaSource};
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
                    assert_eq!(range.as_ref().map(ByteRange::header), Some("bytes=0-65535"));
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
