//! Provider metadata and media access. Credentials and upstream URLs never enter UI DTOs.
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
        })
    }
}
impl std::error::Error for ProviderError {}

pub trait MusicProvider: Send + Sync {
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
