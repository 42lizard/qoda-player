//! Read-compare-write-refresh. Plex has no atomic compare-and-swap here;
//! fingerprints catch observed changes, not every possible concurrent race.
use super::*;
use reqwest::Method;
use sha2::{Digest, Sha256};
fn key(value: &str) -> Result<&str, ProviderError> {
    if value.is_empty() || value.len() > 32 || !value.bytes().all(|b| b.is_ascii_digit()) {
        Err(ProviderError::InvalidPlaylist)
    } else {
        Ok(value)
    }
}
fn title(value: &str) -> Result<(), ProviderError> {
    if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        Err(ProviderError::InvalidPlaylist)
    } else {
        Ok(())
    }
}
fn flag(node: Node<'_, '_>, name: &str) -> bool {
    matches!(node.attribute(name), Some("1" | "true"))
}
fn metadata(node: Node<'_, '_>, source: SourceId) -> Result<Playlist, ProviderError> {
    if node.attribute("playlistType") != Some("audio") {
        return Err(ProviderError::InvalidPlaylist);
    }
    let id = key(node.attribute("ratingKey").unwrap_or_default())?.to_owned();
    let smart = flag(node, "smart");
    Ok(Playlist {
        reference: PlaylistRef { source, key: id },
        title: display_attr(node, "title"),
        smart,
        can_edit: matches!(node.attribute("smart"), Some("0" | "false"))
            && !flag(node, "readOnly")
            && !matches!(node.attribute("allowEdit"), Some("0" | "false")),
        item_count: Counter::new(
            node.attribute("leafCount")
                .and_then(|v| v.parse().ok())
                .ok_or(ProviderError::InvalidReply)?,
        ),
    })
}
impl PlexProvider {
    fn playlist_source(&self, source: SourceId) -> Result<&Source, ProviderError> {
        self.sources
            .get(&source)
            .ok_or(ProviderError::UnknownSource)
    }
    pub(super) async fn playlist_list(
        &self,
        source: SourceId,
        offset: u64,
    ) -> Result<PlaylistPage, ProviderError> {
        let s = self.playlist_source(source)?;
        let mut url = path(&s.base, &["playlists"]);
        url.query_pairs_mut().append_pair("playlistType", "audio");
        let mut h = s.headers.clone();
        h.insert(
            "X-Plex-Container-Start",
            HeaderValue::from_str(&offset.to_string()).unwrap(),
        );
        h.insert("X-Plex-Container-Size", HeaderValue::from_static("100"));
        let body = xml(&self.http, url, h).await?;
        let doc = Document::parse(&body).map_err(|_| ProviderError::InvalidReply)?;
        let root = doc.root_element();
        if root
            .attribute("offset")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            != offset
        {
            return Err(ProviderError::InvalidReply);
        }
        let nodes: Vec<_> = root
            .children()
            .filter(|n| n.has_tag_name("Playlist"))
            .collect();
        if nodes.len() > 100 {
            return Err(ProviderError::TooLarge);
        }
        let playlists = nodes
            .iter()
            .filter(|n| n.attribute("playlistType") == Some("audio"))
            .map(|n| metadata(*n, source))
            .collect::<Result<Vec<_>, _>>()?;
        let total = root
            .attribute("totalSize")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(offset.saturating_add(nodes.len() as u64));
        Ok(PlaylistPage {
            source,
            offset: Counter::new(offset),
            has_more: offset.saturating_add(nodes.len() as u64) < total,
            playlists,
        })
    }
    async fn playlist_metadata(
        &self,
        reference: &PlaylistRef,
    ) -> Result<(Playlist, String), ProviderError> {
        let s = self.playlist_source(reference.source)?;
        let body = xml(
            &self.http,
            path(&s.base, &["playlists", key(&reference.key)?]),
            s.headers.clone(),
        )
        .await?;
        let doc = Document::parse(&body).map_err(|_| ProviderError::InvalidReply)?;
        let node = doc
            .descendants()
            .find(|n| n.has_tag_name("Playlist"))
            .ok_or(ProviderError::InvalidReply)?;
        let playlist = metadata(node, reference.source)?;
        if playlist.reference != *reference {
            return Err(ProviderError::InvalidReply);
        }
        Ok((playlist, attr(node, "updatedAt")))
    }
    pub(super) async fn playlist_snapshot(
        &self,
        reference: PlaylistRef,
    ) -> Result<PlaylistSnapshot, ProviderError> {
        let s = self.playlist_source(reference.source)?;
        let (mut playlist, before) = self.playlist_metadata(&reference).await?;
        if playlist.item_count.get() > 500 {
            return Err(ProviderError::TooLarge);
        }
        let mut h = s.headers.clone();
        h.insert("X-Plex-Container-Start", HeaderValue::from_static("0"));
        h.insert("X-Plex-Container-Size", HeaderValue::from_static("500"));
        let body = xml(
            &self.http,
            path(&s.base, &["playlists", key(&reference.key)?, "items"]),
            h,
        )
        .await?;
        let doc = Document::parse(&body).map_err(|_| ProviderError::InvalidReply)?;
        let root = doc.root_element();
        if root
            .attribute("offset")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            != 0
        {
            return Err(ProviderError::InvalidReply);
        }
        let nodes: Vec<_> = root.children().filter(|n| n.is_element()).collect();
        if nodes.len() > 500
            || nodes.len() as u64 != playlist.item_count.get()
            || root
                .attribute("totalSize")
                .and_then(|v| v.parse::<u64>().ok())
                .is_some_and(|total| total != nodes.len() as u64)
        {
            return Err(ProviderError::InvalidPlaylist);
        }
        let mut entries = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for (index, node) in nodes.iter().enumerate() {
            let item = attr(*node, "playlistItemID");
            let valid_id = key(&item).is_ok() && ids.insert(item.clone());
            if !valid_id {
                playlist.can_edit = false;
            }
            let section = node.attribute("librarySectionID");
            let source = section.and_then(|section| {
                self.sources
                    .iter()
                    .find(|(_, library)| library.server_id == s.server_id && library.key == section)
                    .map(|(id, _)| *id)
            });
            let track = if node.has_tag_name("Track") {
                source.and_then(|source| parse_track(*node, source).ok())
            } else {
                None
            };
            entries.push(PlaylistEntry {
                id: if valid_id {
                    item
                } else {
                    format!("row:{index}")
                },
                title: display_attr(*node, "title"),
                artist: display_attr(*node, "grandparentTitle"),
                album: display_attr(*node, "parentTitle"),
                track,
            });
        }
        let (after, updated) = self.playlist_metadata(&reference).await?;
        if before != updated
            || playlist.title != after.title
            || playlist.smart != after.smart
            || playlist.item_count != after.item_count
        {
            return Err(ProviderError::PlaylistChanged);
        }
        playlist.can_edit &= after.can_edit;
        let encoded = serde_json::to_vec(&(&playlist, &updated, &entries))
            .map_err(|_| ProviderError::InvalidReply)?;
        let revision = format!("{:x}", Sha256::digest(encoded));
        Ok(PlaylistSnapshot {
            playlist,
            revision,
            entries,
        })
    }
    fn track_uri(&self, source: &Source, track: &TrackRef) -> Result<String, ProviderError> {
        let library = self.playlist_source(track.source)?;
        if library.server_id != source.server_id
            || !source
                .server_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(ProviderError::InvalidPlaylist);
        }
        let track = String::from(track.key.clone());
        key(&track)?;
        Ok(format!(
            "server://{}/com.plexapp.plugins.library/library/metadata/{track}",
            source.server_id
        ))
    }
    async fn write(
        &self,
        source: &Source,
        method: Method,
        url: Url,
    ) -> Result<Option<String>, ProviderError> {
        let response = self
            .http
            .request(method, url)
            .headers(source.headers.clone())
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|_| ProviderError::UncertainMutation)?;
        match response.status().as_u16() {
            200..=299 => {
                if response.status().as_u16() == 204 {
                    return Ok(None);
                }
                let body = text(response)
                    .await
                    .map_err(|_| ProviderError::UncertainMutation)?;
                Ok(Some(body))
            }
            401 => Err(ProviderError::SignInRequired),
            403 => Err(ProviderError::AccessDenied),
            400 | 404 | 409 => Err(ProviderError::PlaylistChanged),
            _ => Err(ProviderError::UncertainMutation),
        }
    }
    pub(super) async fn playlist_change(
        &self,
        change: PlaylistChange,
    ) -> Result<PlaylistOutcome, ProviderError> {
        match change {
            PlaylistChange::Create {
                source,
                title: label,
                seed,
            } => {
                title(&label)?;
                let s = self.playlist_source(source)?;
                let mut url = path(&s.base, &["playlists"]);
                url.query_pairs_mut()
                    .append_pair("type", "audio")
                    .append_pair("smart", "0")
                    .append_pair("title", &label);
                let expected_seed = seed.clone();
                if let Some(seed) = seed {
                    url.query_pairs_mut()
                        .append_pair("uri", &self.track_uri(s, &seed)?);
                }
                let body = self
                    .write(s, Method::POST, url)
                    .await?
                    .ok_or(ProviderError::UncertainMutation)?;
                let doc = Document::parse(&body).map_err(|_| ProviderError::UncertainMutation)?;
                let node = doc
                    .descendants()
                    .find(|n| n.has_tag_name("Playlist"))
                    .ok_or(ProviderError::UncertainMutation)?;
                let created =
                    metadata(node, source).map_err(|_| ProviderError::UncertainMutation)?;
                if created.title != label || created.smart {
                    return Err(ProviderError::UncertainMutation);
                }
                let result = self
                    .playlist_snapshot(created.reference)
                    .await
                    .map_err(|_| ProviderError::UncertainMutation)?;
                let matches = match expected_seed {
                    Some(seed) => {
                        result.entries.len() == 1
                            && result.entries[0]
                                .track
                                .as_ref()
                                .is_some_and(|track| track.reference == seed)
                    }
                    None => result.entries.is_empty(),
                };
                if !matches {
                    return Err(ProviderError::UncertainMutation);
                }
                Ok(PlaylistOutcome::Saved(result))
            }
            PlaylistChange::Update {
                reference,
                expected_revision,
                edit,
            } => {
                let current = self.playlist_snapshot(reference.clone()).await?;
                if current.revision != expected_revision {
                    return Err(ProviderError::PlaylistChanged);
                }
                if !current.playlist.can_edit {
                    return Err(ProviderError::ReadOnly);
                }
                let s = self.playlist_source(reference.source)?;
                let base = ["playlists", key(&reference.key)?];
                let (method, url) = match &edit {
                    PlaylistEdit::Rename { title: label } => {
                        title(label)?;
                        let mut url = path(&s.base, &base);
                        url.query_pairs_mut().append_pair("title", label);
                        (Method::PUT, url)
                    }
                    PlaylistEdit::Delete => (Method::DELETE, path(&s.base, &base)),
                    PlaylistEdit::Append { track } => {
                        let uri = self.track_uri(s, track)?;
                        let mut url = path(&s.base, &[base[0], base[1], "items"]);
                        url.query_pairs_mut().append_pair("uri", &uri);
                        (Method::PUT, url)
                    }
                    PlaylistEdit::Remove { item_id } => {
                        if !current.entries.iter().any(|e| e.id == *item_id) {
                            return Err(ProviderError::PlaylistChanged);
                        }
                        (
                            Method::DELETE,
                            path(&s.base, &[base[0], base[1], "items", key(item_id)?]),
                        )
                    }
                    PlaylistEdit::Move { item_id, after } => {
                        if !current.entries.iter().any(|e| e.id == *item_id)
                            || after.as_ref().is_some_and(|id| {
                                id == item_id || !current.entries.iter().any(|e| e.id == *id)
                            })
                        {
                            return Err(ProviderError::PlaylistChanged);
                        }
                        let mut url =
                            path(&s.base, &[base[0], base[1], "items", key(item_id)?, "move"]);
                        url.query_pairs_mut()
                            .append_pair("after", after.as_deref().unwrap_or("0"));
                        (Method::PUT, url)
                    }
                };
                // A single write only. Even a successful response is followed by a fresh read.
                self.write(s, method, url).await?;
                if matches!(edit, PlaylistEdit::Delete) {
                    return match self.playlist_metadata(&reference).await {
                        Err(ProviderError::MissingTrack) => Ok(PlaylistOutcome::Deleted),
                        _ => Err(ProviderError::UncertainMutation),
                    };
                }
                let after = self
                    .playlist_snapshot(reference)
                    .await
                    .map_err(|_| ProviderError::UncertainMutation)?;
                let old: Vec<_> = current.entries.iter().map(|e| e.id.as_str()).collect();
                let new: Vec<_> = after.entries.iter().map(|e| e.id.as_str()).collect();
                let verified = match &edit {
                    PlaylistEdit::Rename { title } => after.playlist.title == *title && old == new,
                    PlaylistEdit::Append { track } => {
                        new.len() == old.len() + 1
                            && new[..old.len()] == old
                            && after.entries.last().is_some_and(|e| {
                                e.track.as_ref().is_some_and(|t| t.reference == *track)
                            })
                    }
                    PlaylistEdit::Remove { item_id } => {
                        new == old
                            .into_iter()
                            .filter(|id| *id != item_id)
                            .collect::<Vec<_>>()
                    }
                    PlaylistEdit::Move { item_id, after } => {
                        let mut expected = old
                            .into_iter()
                            .filter(|id| *id != item_id)
                            .collect::<Vec<_>>();
                        let index = after
                            .as_ref()
                            .and_then(|id| expected.iter().position(|old| *old == id))
                            .map(|i| i + 1)
                            .unwrap_or(0);
                        expected.insert(index, item_id);
                        new == expected
                    }
                    PlaylistEdit::Delete => false,
                };
                if !verified {
                    return Err(ProviderError::UncertainMutation);
                }
                Ok(PlaylistOutcome::Saved(after))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        extract::State,
        http::{HeaderMap as AxumHeaders, StatusCode, Uri},
        response::IntoResponse,
    };
    use std::sync::Mutex;
    #[derive(Clone)]
    struct Fixture {
        data: Arc<Mutex<Data>>,
        provider: Arc<PlexProvider>,
        source: SourceId,
    }
    struct Data {
        exists: bool,
        title: String,
        smart: bool,
        rows: Vec<(String, String)>,
        writes: usize,
        fail: bool,
        known: bool,
        deny: bool,
    }
    fn escaped(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
    fn meta(d: &Data) -> String {
        format!(
            "<MediaContainer size='1'><Playlist ratingKey='10' title=\"{}\" playlistType='audio' smart='{}' leafCount='{}' updatedAt='1'/></MediaContainer>",
            escaped(&d.title),
            u8::from(d.smart),
            d.rows.len()
        )
    }
    async fn serve(
        State(data): State<Arc<Mutex<Data>>>,
        method: Method,
        uri: Uri,
        headers: AxumHeaders,
    ) -> impl IntoResponse {
        assert_eq!(headers.get("x-plex-token").unwrap(), "fixture-secret");
        let url = Url::parse(&format!("http://fixture{uri}")).unwrap();
        let params: HashMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert!(!url.as_str().contains("fixture-secret"));
        let mut d = data.lock().unwrap();
        let p = url.path();
        if method == Method::GET {
            if p == "/playlists" {
                let body = if d.exists {
                    meta(&d)
                } else {
                    "<MediaContainer size='0' totalSize='0'/>".into()
                };
                return (StatusCode::OK, body);
            }
            if !d.exists {
                return (StatusCode::NOT_FOUND, String::new());
            }
            if p == "/playlists/10" {
                return (StatusCode::OK, meta(&d));
            }
            if p == "/playlists/10/items" {
                let section = if d.known { "librarySectionID='7'" } else { "" };
                let rows=d.rows.iter().map(|(id,key)|format!("<Track playlistItemID='{id}' ratingKey='{key}' title='Track {key}' grandparentTitle='Fixture' parentTitle='Album' {section}/>")).collect::<String>();
                return (
                    StatusCode::OK,
                    format!(
                        "<MediaContainer offset='0' totalSize='{}'>{rows}</MediaContainer>",
                        d.rows.len()
                    ),
                );
            }
            return (StatusCode::NOT_FOUND, String::new());
        }
        d.writes += 1;
        if d.deny {
            return (StatusCode::FORBIDDEN, String::new());
        }
        if method == Method::POST && p == "/playlists" {
            d.exists = true;
            d.title = params["title"].clone();
            d.rows = params
                .get("uri")
                .map(|uri| vec![("201".into(), uri.rsplit('/').next().unwrap().to_owned())])
                .unwrap_or_default();
        } else if p == "/playlists/10" && method == Method::DELETE {
            d.exists = false;
        } else if p == "/playlists/10" && method == Method::PUT {
            d.title = params["title"].clone();
        } else if p == "/playlists/10/items" && method == Method::PUT {
            let key = params["uri"].rsplit('/').next().unwrap().to_owned();
            let id = (300 + d.writes).to_string();
            d.rows.push((id, key));
        } else if p.ends_with("/move") {
            let id = p.split('/').nth(4).unwrap();
            let index = d.rows.iter().position(|(entry, _)| entry == id).unwrap();
            let row = d.rows.remove(index);
            let after = params.get("after").unwrap();
            let at = d
                .rows
                .iter()
                .position(|(entry, _)| entry == after)
                .map(|i| i + 1)
                .unwrap_or(0);
            d.rows.insert(at, row);
        } else if method == Method::DELETE {
            let id = p.rsplit('/').next().unwrap();
            d.rows.retain(|(entry, _)| entry != id);
        }
        if d.fail {
            return (StatusCode::INTERNAL_SERVER_ERROR, "private failure".into());
        }
        (StatusCode::OK, meta(&d))
    }
    impl Fixture {
        async fn new() -> Self {
            let data = Arc::new(Mutex::new(Data {
                exists: true,
                title: "Fixture".into(),
                smart: false,
                rows: vec![
                    ("101".into(), "1".into()),
                    ("102".into(), "1".into()),
                    ("103".into(), "2".into()),
                ],
                writes: 0,
                fail: false,
                known: true,
                deny: false,
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
            let app = Router::new().fallback(serve).with_state(data.clone());
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            let source = SourceId::new();
            let provider = Arc::new(PlexProvider {
                account_scope: "a".repeat(64),
                unavailable_sources: HashSet::new(),
                unavailable_servers: 0,
                http: Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
                sources: HashMap::from([(
                    source,
                    Source {
                        server_id: "fixture-server".into(),
                        view: Library {
                            id: source,
                            title: "Fixture".into(),
                            server: "Fixture".into(),
                        },
                        base,
                        headers: headers("fixture", Some("fixture-secret")).unwrap(),
                        key: "7".into(),
                    },
                )]),
            });
            Self {
                data,
                provider,
                source,
            }
        }
        async fn snapshot(&self) -> PlaylistSnapshot {
            self.provider
                .playlist(PlaylistRef {
                    source: self.source,
                    key: "10".into(),
                })
                .await
                .unwrap()
        }
        async fn edit(
            &self,
            snapshot: &PlaylistSnapshot,
            edit: PlaylistEdit,
        ) -> Result<PlaylistOutcome, ProviderError> {
            self.provider
                .edit_playlist(PlaylistChange::Update {
                    reference: snapshot.playlist.reference.clone(),
                    expected_revision: snapshot.revision.clone(),
                    edit,
                })
                .await
        }
    }
    #[test]
    fn missing_smart_capability_is_not_assumed_editable() {
        let doc = Document::parse(
            "<Playlist ratingKey='10' title='Unknown' playlistType='audio' leafCount='0'/>",
        )
        .unwrap();
        let playlist = metadata(doc.root_element(), SourceId::new()).unwrap();
        assert!(!playlist.can_edit);
        let doc=Document::parse("<Playlist ratingKey='10' title='Read only' playlistType='audio' smart='0' allowEdit='false' leafCount='0'/>").unwrap();
        assert!(
            !metadata(doc.root_element(), SourceId::new())
                .unwrap()
                .can_edit
        );
    }
    #[tokio::test]
    async fn duplicate_occurrence_crud_is_verified_and_credentials_stay_private() {
        let f = Fixture::new().await;
        let list = f.provider.playlists(f.source, 0).await.unwrap();
        assert_eq!(list.playlists.len(), 1);
        let view = f.snapshot().await;
        assert_eq!(
            view.entries[0].track.as_ref().unwrap().reference,
            view.entries[1].track.as_ref().unwrap().reference
        );
        assert_ne!(view.entries[0].id, view.entries[1].id);
        let PlaylistOutcome::Saved(view) = f
            .edit(
                &view,
                PlaylistEdit::Move {
                    item_id: "102".into(),
                    after: None,
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(view.entries[0].id, "102");
        let PlaylistOutcome::Saved(view) = f
            .edit(
                &view,
                PlaylistEdit::Remove {
                    item_id: "102".into(),
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            view.entries
                .iter()
                .filter(|e| e.track.as_ref().unwrap().reference.key
                    == "1".to_owned().try_into().unwrap())
                .count(),
            1
        );
        let PlaylistOutcome::Saved(view) = f
            .edit(
                &view,
                PlaylistEdit::Rename {
                    title: "Renamed & verified".into(),
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(view.playlist.title, "Renamed & verified");
        let track = view.entries[0].track.as_ref().unwrap().reference.clone();
        let PlaylistOutcome::Saved(view) =
            f.edit(&view, PlaylistEdit::Append { track }).await.unwrap()
        else {
            panic!()
        };
        assert_eq!(view.entries.len(), 3);
        assert!(
            !serde_json::to_string(&view)
                .unwrap()
                .contains("fixture-secret")
        );
        assert!(matches!(
            f.edit(&view, PlaylistEdit::Delete).await,
            Ok(PlaylistOutcome::Deleted)
        ));
        assert!(!f.data.lock().unwrap().exists);
        let PlaylistOutcome::Saved(created) = f
            .provider
            .edit_playlist(PlaylistChange::Create {
                source: f.source,
                title: "New fixture".into(),
                seed: None,
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(created.playlist.title, "New fixture");
        assert!(created.entries.is_empty());
        f.edit(&created, PlaylistEdit::Delete).await.unwrap();
        let seed = TrackRef {
            source: f.source,
            key: "2".to_owned().try_into().unwrap(),
        };
        let PlaylistOutcome::Saved(seeded) = f
            .provider
            .edit_playlist(PlaylistChange::Create {
                source: f.source,
                title: "Seeded fixture".into(),
                seed: Some(seed.clone()),
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(seeded.entries[0].track.as_ref().unwrap().reference, seed);
    }
    #[tokio::test]
    async fn changed_smart_and_inaccessible_sources_are_not_silently_edited_or_substituted() {
        let f = Fixture::new().await;
        let before = f.snapshot().await;
        f.data.lock().unwrap().rows.swap(0, 2);
        assert!(matches!(
            f.edit(&before, PlaylistEdit::Delete).await,
            Err(ProviderError::PlaylistChanged)
        ));
        assert_eq!(f.data.lock().unwrap().writes, 0);
        f.data.lock().unwrap().smart = true;
        let smart = f.snapshot().await;
        assert!(!smart.playlist.can_edit);
        assert!(matches!(
            f.edit(&smart, PlaylistEdit::Delete).await,
            Err(ProviderError::ReadOnly)
        ));
        assert_eq!(f.data.lock().unwrap().writes, 0);
        f.data.lock().unwrap().known = false;
        let unavailable = f.snapshot().await;
        assert!(unavailable.entries.iter().all(|e| e.track.is_none()));
        assert_eq!(unavailable.entries.len(), 3);
    }
    #[tokio::test]
    async fn a_write_applied_before_failure_is_never_automatically_retried() {
        let f = Fixture::new().await;
        let view = f.snapshot().await;
        f.data.lock().unwrap().fail = true;
        let track = view.entries[0].track.as_ref().unwrap().reference.clone();
        assert!(matches!(
            f.edit(&view, PlaylistEdit::Append { track }).await,
            Err(ProviderError::UncertainMutation)
        ));
        assert_eq!(f.data.lock().unwrap().writes, 1);
        assert_eq!(f.data.lock().unwrap().rows.len(), 4);
        f.data.lock().unwrap().fail = false;
        let after = f.snapshot().await;
        assert_eq!(after.entries.len(), 4);
    }
    #[tokio::test]
    async fn denial_is_visible_and_does_not_create_a_retry_loop() {
        let f = Fixture::new().await;
        let view = f.snapshot().await;
        f.data.lock().unwrap().deny = true;
        assert!(matches!(
            f.edit(&view, PlaylistEdit::Delete).await,
            Err(ProviderError::AccessDenied)
        ));
        assert_eq!(f.data.lock().unwrap().writes, 1);
        assert!(f.data.lock().unwrap().exists);
    }
}
