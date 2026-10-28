//! Provider metadata and media access. Credentials and upstream URLs never enter UI DTOs.
pub mod artwork;
pub mod playback;
mod plex;
use bytes::Bytes;
use futures_util::stream::BoxStream;
pub use plex::{BrowserPin, Credential, PlexProvider};
use qoda_core::{Counter, ProviderTrackKey, SourceId, TrackRef};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};
pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Library {
    pub id: SourceId,
    pub title: String,
    pub server: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub reference: TrackRef,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: Option<Counter>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogPage {
    pub source: SourceId,
    pub offset: Counter,
    pub total: Option<Counter>,
    pub query: String,
    pub tracks: Vec<Track>,
    pub has_more: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AlbumRef {
    pub source: SourceId,
    pub key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtistRef {
    pub source: SourceId,
    pub key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artist {
    pub reference: ArtistRef,
    pub title: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtistPage {
    pub source: SourceId,
    pub offset: Counter,
    pub query: String,
    pub artists: Vec<Artist>,
    pub has_more: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Album {
    pub reference: AlbumRef,
    pub artist_ref: Option<ArtistRef>,
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    pub track_count: Option<Counter>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlbumPage {
    pub artist: Option<Artist>,
    pub source: SourceId,
    pub offset: Counter,
    pub query: String,
    pub albums: Vec<Album>,
    pub has_more: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlbumTrack {
    pub track: Track,
    pub disc: u32,
    pub number: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlbumSnapshot {
    pub album: Album,
    pub revision: String,
    pub tracks: Vec<AlbumTrack>,
    /// Bounded JPEG data URI; never an upstream URL or credential.
    pub artwork: Option<String>,
    pub artwork_unavailable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaylistRef {
    pub source: SourceId,
    pub key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playlist {
    pub reference: PlaylistRef,
    pub title: String,
    pub smart: bool,
    pub can_edit: bool,
    pub item_count: Counter,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaylistPage {
    pub source: SourceId,
    pub offset: Counter,
    pub playlists: Vec<Playlist>,
    pub has_more: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaylistEntry {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track: Option<Track>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaylistSnapshot {
    pub playlist: Playlist,
    pub revision: String,
    pub entries: Vec<PlaylistEntry>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PlaylistEdit {
    Rename {
        title: String,
    },
    Delete,
    Append {
        track: TrackRef,
    },
    Remove {
        item_id: String,
    },
    Move {
        item_id: String,
        after: Option<String>,
    },
}
#[derive(Clone, Debug)]
pub enum PlaylistChange {
    Create {
        source: SourceId,
        title: String,
        seed: Option<TrackRef>,
    },
    Update {
        reference: PlaylistRef,
        expected_revision: String,
        edit: PlaylistEdit,
    },
}
#[derive(Clone, Debug)]
pub enum PlaylistOutcome {
    Saved(PlaylistSnapshot),
    Deleted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderError {
    Network,
    SignInRequired,
    AccessDenied,
    InvalidReply,
    TooLarge,
    UnknownSource,
    MissingTrack,
    UnsupportedMedia,
    InvalidCredential,
    InvalidRange,
    Unsupported,
    PlaylistChanged,
    UncertainMutation,
    PlaylistDuplicateUnconfirmed,
    ReadOnly,
    InvalidPlaylist,
}
impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Network => "Plex could not be reached. Check the connection and retry.",
            Self::SignInRequired => "Plex sign-in is required.",
            Self::AccessDenied => "Plex denied access. Reconnect or check permissions.",
            Self::InvalidReply => "Plex returned metadata that could not be read.",
            Self::TooLarge => "The server response exceeded the supported page size.",
            Self::UnknownSource => "Choose an available music library.",
            Self::MissingTrack => "This track is no longer available in that library.",
            Self::UnsupportedMedia => "This media version is not supported.",
            Self::InvalidCredential => "The saved sign-in could not be read.",
            Self::InvalidRange => "The requested media range is invalid.",
            Self::Unsupported=>"This provider does not support this action.",
            Self::PlaylistChanged=>"The playlist changed. Refresh and review its contents before editing again.",
            Self::PlaylistDuplicateUnconfirmed=>"Plex did not confirm another copy of this track. Some servers collapse playlist duplicates. Refresh before editing again; use the queue for repeats.",
            Self::UncertainMutation=>"The playlist edit may have reached Plex. Refresh and inspect the result before making another edit; the request was not retried.",
            Self::ReadOnly=>"This playlist is read-only in Qoda. Smart playlists cannot be edited here.",
            Self::InvalidPlaylist=>"The playlist request or complete playlist snapshot is not supported.",
        })
    }
}
impl std::error::Error for ProviderError {}

pub trait MusicProvider: Send + Sync {
    /// Optional fallback for a pinned original version after local unsupported decoding.
    fn prepare_transcoded(
        &self,
        _reference: TrackRef,
        _version: String,
        _position: f64,
    ) -> ProviderFuture<'_, PreparedMedia> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    /// Confirmed nominal playback only. Preparation never calls this operation.
    /// Unsupported by default; implementations must not infer a scrobble.
    fn report_playback(&self, _report: playback::Report) -> ProviderFuture<'_, playback::Outcome> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    fn artists(
        &self,
        _source: SourceId,
        _offset: u64,
        _query: String,
    ) -> ProviderFuture<'_, ArtistPage> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    fn artist_albums(
        &self,
        _reference: ArtistRef,
        _offset: u64,
        _query: String,
    ) -> ProviderFuture<'_, AlbumPage> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    /// A provider must validate JPEG dimensions and cap the result at 256 KiB.
    fn cover(&self, _reference: AlbumRef) -> ProviderFuture<'_, Vec<u8>> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    fn albums(
        &self,
        _source: SourceId,
        _offset: u64,
        _query: String,
    ) -> ProviderFuture<'_, AlbumPage> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    fn album(&self, _reference: AlbumRef) -> ProviderFuture<'_, AlbumSnapshot> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    fn playlists(&self, _source: SourceId, _offset: u64) -> ProviderFuture<'_, PlaylistPage> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    fn playlist(&self, _reference: PlaylistRef) -> ProviderFuture<'_, PlaylistSnapshot> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    fn edit_playlist(&self, _change: PlaylistChange) -> ProviderFuture<'_, PlaylistOutcome> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }

    fn libraries(&self) -> Vec<Library>;
    fn tracks(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> ProviderFuture<'_, CatalogPage>;
    fn prepare(&self, source: SourceId, key: ProviderTrackKey)
    -> ProviderFuture<'_, PreparedMedia>;
}
pub struct PreparedMedia {
    pub track: Track,
    pub version: String,
    pub stream: Box<dyn MediaSource>,
}
pub trait MediaSource: Send + Sync {
    /// Absolute source time corresponding to local decoder time zero.
    fn timeline_offset(&self) -> f64 {
        0.0
    }
    fn transcoded(&self) -> bool {
        false
    }
    /// Retire any provider-owned resources before a playback grant is acknowledged as revoked.
    fn close(&self) -> ProviderFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    fn open(&self, range: Option<ByteRange>, head: bool) -> ProviderFuture<'_, MediaResponse>;
}
pub struct MediaResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub content_range: Option<String>,
    pub etag: Option<String>,
    pub body: BoxStream<'static, Result<Bytes, ProviderError>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteRange(String);
impl ByteRange {
    pub fn header(&self) -> &str {
        &self.0
    }
}
impl TryFrom<&str> for ByteRange {
    type Error = ProviderError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let rest = value
            .strip_prefix("bytes=")
            .ok_or(ProviderError::InvalidRange)?;
        let (a, b) = rest.split_once('-').ok_or(ProviderError::InvalidRange)?;
        if value.len() > 128
            || a.is_empty() && b.is_empty()
            || ![a, b].iter().all(|s| {
                s.is_empty() || s.bytes().all(|c| c.is_ascii_digit()) && s.parse::<u64>().is_ok()
            })
        {
            return Err(ProviderError::InvalidRange);
        }
        if !a.is_empty() && !b.is_empty() && a.parse::<u64>().unwrap() > b.parse::<u64>().unwrap() {
            return Err(ProviderError::InvalidRange);
        }
        if a.is_empty() && b == "0" {
            return Err(ProviderError::InvalidRange);
        }
        Ok(Self(value.to_owned()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_single_bounded_http_ranges_are_accepted() {
        for s in ["bytes=0-", "bytes=10-20", "bytes=-100"] {
            assert!(ByteRange::try_from(s).is_ok());
        }
        for s in [
            "bytes=20-10",
            "bytes=-",
            "bytes=0-1,2-3",
            "bytes=0-\r\nX: secret",
            "bytes=-0",
            "bytes=18446744073709551616-",
        ] {
            assert!(ByteRange::try_from(s).is_err());
        }
    }
}
