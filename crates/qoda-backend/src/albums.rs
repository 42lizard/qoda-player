//! Cancellable album browsing; account and request generations fence late reads.
use crate::metadata_cache::{Cache, Info, Page, Query};
use qoda_core::{Counter, SourceId};
use qoda_provider::{
    AlbumPage, AlbumRef, AlbumSnapshot, ArtistPage, ArtistRef, MusicProvider, ProviderError, Track,
};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Default, Serialize)]
pub struct View {
    pub page: Option<AlbumPage>,
    pub artists: Option<ArtistPage>,
    pub covers: std::collections::HashMap<String, String>,
    pub selected: Option<AlbumSnapshot>,
    pub busy: bool,
    pub message: String,
    pub cache: Info,
}
pub struct Reply {
    info: Info,
    auth: u64,
    generation: u64,
    result: Result<Update, ProviderError>,
}
enum Update {
    Page(AlbumPage),
    Artists(ArtistPage),
    Open(AlbumSnapshot),
}
#[derive(Default)]
pub struct Controller {
    pub view: View,
    generation: u64,
    source: Option<SourceId>,
    task: Option<tokio::task::JoinHandle<()>>,
    cache: Cache,
}
impl Controller {
    pub fn with_cache(cache: Cache) -> Self {
        Self {
            cache,
            ..Self::default()
        }
    }
    pub fn invalidate(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.generation = self.generation.saturating_add(1);
        self.view = View::default();
        self.source = None;
    }
    pub fn source_changed(&mut self, source: SourceId) -> bool {
        if self.source.is_some_and(|current| current != source) {
            self.invalidate();
            return true;
        }
        false
    }
    pub fn command(
        &mut self,
        request: &Value,
        provider: Arc<dyn MusicProvider>,
        auth: u64,
        events: mpsc::Sender<crate::Event>,
    ) -> Result<(), String> {
        enum Action {
            Page(SourceId, u64, String, Option<ArtistRef>),
            Artists(SourceId, u64, String),
            Open(AlbumRef),
        }
        let action = match request["op"].as_str() {
            Some("album_list" | "artist_list" | "artist_open") => {
                let artist = if request["op"] == "artist_open" {
                    let reference: ArtistRef = serde_json::from_value(request["reference"].clone())
                        .map_err(|_| "Choose an artist.")?;
                    if !self.known_artist(&reference) {
                        return Err("That artist is no longer displayed. Browse again.".into());
                    }
                    Some(reference)
                } else {
                    None
                };
                let source = match &artist {
                    Some(artist) => artist.source,
                    None => serde_json::from_value(request["source"].clone())
                        .map_err(|_| ProviderError::UnknownSource.to_string())?,
                };
                let offset = serde_json::from_value::<Counter>(request["offset"].clone())
                    .map_err(|_| "Invalid album page.")?
                    .get();
                let query: String = request["query"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(512)
                    .collect();
                if request["op"] == "artist_list" {
                    Action::Artists(source, offset, query)
                } else {
                    Action::Page(source, offset, query, artist)
                }
            }
            Some("album_open") => {
                let reference: AlbumRef = serde_json::from_value(request["reference"].clone())
                    .map_err(|_| "Choose an album.")?;
                if !self
                    .view
                    .page
                    .as_ref()
                    .is_some_and(|p| p.albums.iter().any(|a| a.reference == reference))
                {
                    return Err(
                        "That album is no longer on the displayed page. Browse again.".into(),
                    );
                }
                Action::Open(reference)
            }
            _ => return Err("Unknown album action.".into()),
        };
        let source = match &action {
            Action::Page(source, ..) | Action::Artists(source, ..) => *source,
            Action::Open(reference) => reference.source,
        };
        self.source_changed(source);
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("Album generation exhausted")?;
        self.generation = generation;
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.source = Some(match &action {
            Action::Page(source, ..) | Action::Artists(source, ..) => *source,
            Action::Open(reference) => reference.source,
        });
        self.view.selected = None;
        self.view.busy = true;
        self.view.message = if matches!(action, Action::Artists(..)) {
            "Loading artists…"
        } else {
            "Loading albums…"
        }
        .into();
        if matches!(action, Action::Page(..) | Action::Artists(..)) {
            self.view.page = None;
            self.view.covers.clear();
        }
        if matches!(action, Action::Artists(..)) {
            self.view.artists = None;
        }
        let cache = self.cache.clone();
        self.task = Some(tokio::spawn(async move {
            let query = match &action {
                Action::Page(source, offset, query, artist) => Some(Query::Albums {
                    source: *source,
                    offset: *offset,
                    query: query.clone(),
                    artist: artist.clone(),
                }),
                Action::Artists(source, offset, query) => Some(Query::Artists {
                    source: *source,
                    offset: *offset,
                    query: query.clone(),
                }),
                Action::Open(_) => None,
            };
            if let Some(query) = query {
                cache
                    .read(provider, query, move |result, info| {
                        let events = events.clone();
                        async move {
                            let result = result.and_then(|page| match page {
                                Page::Albums(page) => Ok(Update::Page(page)),
                                Page::Artists(page) => Ok(Update::Artists(page)),
                                _ => Err(ProviderError::InvalidReply),
                            });
                            let _ = events
                                .send(crate::Event::Albums(Reply {
                                    auth,
                                    generation,
                                    result,
                                    info,
                                }))
                                .await;
                        }
                    })
                    .await;
            } else if let Action::Open(reference) = action {
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(45),
                    provider.album(reference),
                )
                .await
                .unwrap_or(Err(ProviderError::Network))
                .map(Update::Open);
                let _ = events
                    .send(crate::Event::Albums(Reply {
                        auth,
                        generation,
                        result,
                        info: Info::default(),
                    }))
                    .await;
            }
        }));
        Ok(())
    }
    pub fn known_artist(&self, reference: &ArtistRef) -> bool {
        self.view
            .artists
            .as_ref()
            .is_some_and(|p| p.artists.iter().any(|a| &a.reference == reference))
            || self.view.page.as_ref().is_some_and(|p| {
                p.artist.as_ref().is_some_and(|a| &a.reference == reference)
                    || p.albums
                        .iter()
                        .any(|a| a.artist_ref.as_ref() == Some(reference))
            })
            || self
                .view
                .selected
                .as_ref()
                .is_some_and(|s| s.album.artist_ref.as_ref() == Some(reference))
    }
    /// Returns true only when a current page needs fresh artwork grants.
    pub fn apply(&mut self, reply: Reply, auth: u64) -> bool {
        if reply.auth != auth || reply.generation != self.generation {
            return false;
        }
        if !reply.info.refreshing {
            self.task = None;
        }
        self.view.busy = false;
        self.view.cache = reply.info;
        let new_page = matches!(&reply.result, Ok(Update::Page(_)));
        match reply.result {
            Ok(Update::Artists(page)) => {
                self.view.message = if page.artists.is_empty() {
                    "No artists found."
                } else {
                    "Choose an artist to browse their albums."
                }
                .into();
                self.view.artists = Some(page);
            }
            Ok(Update::Page(page)) => {
                self.view.message = if page.albums.is_empty() {
                    "No albums found."
                } else {
                    "Choose an album to view its tracks and cover."
                }
                .into();
                self.view.page = Some(page);
            }
            Ok(Update::Open(snapshot)) => {
                self.view.message = if snapshot.artwork_unavailable {
                    "Album loaded. Cover unavailable."
                } else {
                    "Album loaded. Queue copies are independent of the library."
                }
                .into();
                self.view.selected = Some(snapshot);
            }
            Err(error) => {
                self.view.message = error.to_string();
                if matches!(
                    error,
                    ProviderError::AccessDenied | ProviderError::UnknownSource
                ) {
                    self.view.page = None;
                    self.view.artists = None;
                    self.view.selected = None;
                    self.view.covers.clear();
                }
            }
        }
        if self.view.cache.origin != "live" {
            self.view.message = format!("{} {}", self.view.cache.message, self.view.message);
        }
        new_page
    }
    pub fn tracks(&self, request: &Value) -> Result<Vec<Track>, String> {
        let reference: AlbumRef = serde_json::from_value(request["reference"].clone())
            .map_err(|_| "Choose a loaded album.")?;
        let snapshot = self
            .view
            .selected
            .as_ref()
            .filter(|s| {
                s.album.reference == reference
                    && Some(s.revision.as_str()) == request["expected_revision"].as_str()
            })
            .ok_or("This album view changed. Open the album again.")?;
        if self.view.busy {
            return Err("Wait for the album to finish loading.".into());
        }
        let start = if let Some(key) = request["track_key"].as_str() {
            snapshot
                .tracks
                .iter()
                .position(|t| String::from(t.track.reference.key.clone()) == key)
                .ok_or("This album track is no longer displayed.")?
        } else {
            0
        };
        Ok(snapshot.tracks[start..]
            .iter()
            .map(|t| t.track.clone())
            .collect())
    }
    pub fn track(&self, reference: &qoda_core::TrackRef) -> Option<Track> {
        if self.view.busy {
            return None;
        }
        self.view
            .selected
            .as_ref()?
            .tracks
            .iter()
            .find(|t| &t.track.reference == reference)
            .map(|t| t.track.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qoda_provider::{Album, AlbumTrack};
    use serde_json::json;
    fn snapshot() -> AlbumSnapshot {
        let source = SourceId::new();
        AlbumSnapshot {
            album: Album {
                reference: AlbumRef {
                    source,
                    key: "10".into(),
                },
                artist_ref: None,
                title: "Album".into(),
                artist: "Artist".into(),
                year: None,
                track_count: Some(Counter::new(2)),
            },
            revision: "snapshot-a".into(),
            tracks: ["1", "2"]
                .into_iter()
                .map(|key| AlbumTrack {
                    track: Track {
                        reference: qoda_core::TrackRef {
                            source,
                            key: key.to_owned().try_into().unwrap(),
                        },
                        title: key.into(),
                        artist: "Artist".into(),
                        album: "Album".into(),
                        duration_ms: None,
                    },
                    disc: 1,
                    number: key.parse().unwrap(),
                })
                .collect(),
            artwork: None,
            artwork_unavailable: true,
        }
    }
    #[test]
    fn queue_copy_is_fenced_by_loaded_album_revision_and_requested_start_track() {
        let snapshot = snapshot();
        let mut request =
            json!({"reference":snapshot.album.reference,"expected_revision":snapshot.revision});
        let mut c = Controller::default();
        c.view.selected = Some(snapshot);
        assert_eq!(c.tracks(&request).unwrap().len(), 2);
        request["track_key"] = json!("2");
        assert_eq!(c.tracks(&request).unwrap().len(), 1);
        request["track_key"] = json!("missing");
        assert!(c.tracks(&request).is_err());
        request["track_key"] = Value::Null;
        request["expected_revision"] = json!("older");
        assert!(c.tracks(&request).is_err());
        request["expected_revision"] = json!("snapshot-a");
        c.view.busy = true;
        assert!(c.tracks(&request).is_err());
        c.invalidate();
        assert!(c.tracks(&request).is_err());
    }
    fn artist_page(source: SourceId) -> ArtistPage {
        ArtistPage {
            source,
            offset: Counter::new(0),
            query: "".into(),
            has_more: false,
            artists: ["1", "2"]
                .into_iter()
                .map(|key| qoda_provider::Artist {
                    reference: ArtistRef {
                        source,
                        key: key.into(),
                    },
                    title: "Same name".into(),
                })
                .collect(),
        }
    }
    #[test]
    fn artist_navigation_uses_displayed_source_scoped_identity_and_invalidates_on_switch() {
        let source = SourceId::new();
        let mut c = Controller {
            source: Some(source),
            ..Default::default()
        };
        c.view.artists = Some(artist_page(source));
        assert!(c.known_artist(&ArtistRef {
            source,
            key: "1".into()
        }));
        assert!(c.known_artist(&ArtistRef {
            source,
            key: "2".into()
        }));
        assert!(!c.known_artist(&ArtistRef {
            source,
            key: "3".into()
        }));
        assert!(!c.known_artist(&ArtistRef {
            source: SourceId::new(),
            key: "1".into()
        }));
        assert!(c.source_changed(SourceId::new()));
        assert!(!c.known_artist(&ArtistRef {
            source,
            key: "1".into()
        }));
        let snapshot = snapshot();
        let reference = ArtistRef {
            source: snapshot.album.reference.source,
            key: "9".into(),
        };
        c.view.selected = Some(AlbumSnapshot {
            album: qoda_provider::Album {
                artist_ref: Some(reference.clone()),
                ..snapshot.album.clone()
            },
            ..snapshot
        });
        assert!(c.known_artist(&reference));
        c.invalidate();
        assert!(!c.known_artist(&reference));
    }
    #[test]
    fn late_artist_pages_cannot_cross_account_request_or_library_fences() {
        let source = SourceId::new();
        let mut c = Controller {
            generation: 4,
            source: Some(source),
            ..Default::default()
        };
        for (auth, generation) in [(1, 4), (2, 3)] {
            assert!(!c.apply(
                Reply {
                    info: Info::default(),
                    auth,
                    generation,
                    result: Ok(Update::Artists(artist_page(source)))
                },
                2
            ));
            assert!(c.view.artists.is_none());
        }
        c.apply(
            Reply {
                info: Info::default(),
                auth: 2,
                generation: 4,
                result: Ok(Update::Artists(artist_page(source))),
            },
            2,
        );
        assert!(c.view.artists.is_some());
        c.source_changed(SourceId::new());
        c.apply(
            Reply {
                info: Info::default(),
                auth: 2,
                generation: 4,
                result: Ok(Update::Artists(artist_page(source))),
            },
            2,
        );
        assert!(c.view.artists.is_none());
    }
    #[test]
    fn old_account_or_superseded_album_reads_cannot_repopulate_current_view() {
        let mut c = Controller {
            generation: 3,
            ..Controller::default()
        };
        c.apply(
            Reply {
                info: Info::default(),
                auth: 1,
                generation: 3,
                result: Ok(Update::Open(snapshot())),
            },
            2,
        );
        c.apply(
            Reply {
                info: Info::default(),
                auth: 2,
                generation: 2,
                result: Ok(Update::Open(snapshot())),
            },
            2,
        );
        assert!(c.view.selected.is_none());
        c.apply(
            Reply {
                info: Info::default(),
                auth: 2,
                generation: 3,
                result: Ok(Update::Open(snapshot())),
            },
            2,
        );
        assert!(c.view.selected.is_some());
        c.invalidate();
        c.apply(
            Reply {
                info: Info::default(),
                auth: 2,
                generation: 3,
                result: Ok(Update::Open(snapshot())),
            },
            2,
        );
        assert!(c.view.selected.is_none());
        c.source = Some(SourceId::new());
        c.view.busy = true;
        let generation = c.generation;
        c.source_changed(SourceId::new());
        assert!(!c.view.busy && c.source.is_none());
        assert!(c.generation > generation);
    }
}
