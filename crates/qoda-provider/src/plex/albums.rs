//! Library-scoped album snapshots and bounded, authenticated cover reads.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

pub(super) fn key(value: &str) -> Result<&str, ProviderError> {
    if value.is_empty() || value.len() > 32 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ProviderError::InvalidReply);
    }
    Ok(value)
}
fn album(node: Node<'_, '_>, source: SourceId) -> Result<Album, ProviderError> {
    if node.attribute("type") != Some("album") {
        return Err(ProviderError::InvalidReply);
    }
    Ok(Album {
        reference: AlbumRef {
            source,
            key: key(node.attribute("ratingKey").unwrap_or_default())?.into(),
        },
        artist_ref: node
            .attribute("parentRatingKey")
            .filter(|id| key(id).is_ok())
            .map(|id| ArtistRef {
                source,
                key: id.into(),
            }),
        title: display_attr(node, "title"),
        artist: display_attr(node, "parentTitle"),
        year: node.attribute("year").and_then(|v| v.parse().ok()),
        track_count: node
            .attribute("leafCount")
            .map(|v| {
                v.parse::<u64>()
                    .map(Counter::new)
                    .map_err(|_| ProviderError::InvalidReply)
            })
            .transpose()?,
    })
}
pub(super) fn page_headers(source: &Source, offset: u64, size: &'static str) -> HeaderMap {
    let mut h = source.headers.clone();
    h.insert(
        "X-Plex-Container-Start",
        HeaderValue::from_str(&offset.to_string()).unwrap(),
    );
    h.insert("X-Plex-Container-Size", HeaderValue::from_static(size));
    h
}
pub(super) fn page_count(
    root: Node<'_, '_>,
    offset: u64,
    count: usize,
) -> Result<u64, ProviderError> {
    if root
        .attribute("offset")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        != offset
    {
        return Err(ProviderError::InvalidReply);
    }
    let total = root
        .attribute("totalSize")
        .or_else(|| root.attribute("size"))
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or(ProviderError::InvalidReply)?;
    if (count == 0 && offset < total) || offset.saturating_add(count as u64) > total {
        return Err(ProviderError::InvalidReply);
    }
    Ok(total)
}
impl PlexProvider {
    pub(super) async fn album_list(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> Result<AlbumPage, ProviderError> {
        self.album_list_for(source, offset, query, None).await
    }
    pub(super) async fn album_list_for(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
        artist: Option<Artist>,
    ) -> Result<AlbumPage, ProviderError> {
        let s = self
            .sources
            .get(&source)
            .ok_or(ProviderError::UnknownSource)?;
        if query.len() > 2048 {
            return Err(ProviderError::InvalidReply);
        }
        let mut url = path(&s.base, &["library", "sections", &s.key, "all"]);
        url.query_pairs_mut()
            .append_pair("type", "9")
            .append_pair("sort", "artist.titleSort:asc,album.titleSort:asc");
        if !query.is_empty() {
            url.query_pairs_mut().append_pair("title", &query);
        }
        if let Some(artist) = &artist {
            url.query_pairs_mut()
                .append_pair("artist.id", &artist.reference.key);
        }
        let body = xml(&self.http, url, page_headers(s, offset, "100")).await?;
        let doc = Document::parse(&body).map_err(|_| ProviderError::InvalidReply)?;
        let root = doc.root_element();
        if root
            .attribute("librarySectionID")
            .is_some_and(|id| id != s.key)
            || root.attribute("totalSize").is_none()
        {
            return Err(ProviderError::InvalidReply);
        }
        let albums = root
            .children()
            .filter(|n| n.has_tag_name("Directory"))
            .map(|n| {
                if n.attribute("librarySectionID")
                    .is_some_and(|id| id != s.key)
                {
                    return Err(ProviderError::InvalidReply);
                }
                let album = album(n, source)?;
                if artist
                    .as_ref()
                    .is_some_and(|artist| album.artist_ref.as_ref() != Some(&artist.reference))
                {
                    return Err(ProviderError::InvalidReply);
                }
                Ok(album)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if albums.len() > 100 {
            return Err(ProviderError::TooLarge);
        }
        let total = page_count(root, offset, albums.len())?;
        let mut seen = std::collections::HashSet::new();
        if albums.iter().any(|a| !seen.insert(&a.reference.key)) {
            return Err(ProviderError::InvalidReply);
        }
        Ok(AlbumPage {
            artist,
            source,
            offset: Counter::new(offset),
            query,
            has_more: offset.saturating_add(albums.len() as u64) < total,
            albums,
        })
    }
    pub(super) async fn album_snapshot(
        &self,
        reference: AlbumRef,
    ) -> Result<AlbumSnapshot, ProviderError> {
        let s = self
            .sources
            .get(&reference.source)
            .ok_or(ProviderError::UnknownSource)?;
        let id = key(&reference.key)?;
        let body = xml(
            &self.http,
            path(&s.base, &["library", "metadata", id]),
            s.headers.clone(),
        )
        .await?;
        let doc = Document::parse(&body).map_err(|_| ProviderError::InvalidReply)?;
        let node = doc
            .root_element()
            .children()
            .find(|n| n.has_tag_name("Directory"))
            .ok_or(ProviderError::InvalidReply)?;
        if doc
            .root_element()
            .attribute("librarySectionID")
            .is_some_and(|id| id != s.key)
        {
            return Err(ProviderError::InvalidReply);
        }
        if node
            .attribute("librarySectionID")
            .or_else(|| doc.root_element().attribute("librarySectionID"))
            != Some(s.key.as_str())
        {
            return Err(ProviderError::InvalidReply);
        }
        let mut album = album(node, reference.source)?;
        if album.reference != reference {
            return Err(ProviderError::InvalidReply);
        }
        if album.track_count.is_some_and(|count| count.get() > 500) {
            return Err(ProviderError::TooLarge);
        }
        let thumb = attr(node, "thumb");
        let body = xml(
            &self.http,
            path(&s.base, &["library", "metadata", id, "children"]),
            page_headers(s, 0, "500"),
        )
        .await?;
        let doc = Document::parse(&body).map_err(|_| ProviderError::InvalidReply)?;
        if doc
            .root_element()
            .attribute("librarySectionID")
            .is_some_and(|id| id != s.key)
            || (doc.root_element().attribute("totalSize").is_none() && album.track_count.is_none())
        {
            return Err(ProviderError::InvalidReply);
        }
        let mut tracks = Vec::new();
        let mut keys = std::collections::HashSet::new();
        for n in doc.root_element().children().filter(|n| n.is_element()) {
            if !n.has_tag_name("Track")
                || n.attribute("parentRatingKey") != Some(id)
                || n.attribute("librarySectionID").is_some_and(|v| v != s.key)
            {
                return Err(ProviderError::InvalidReply);
            }
            let track = parse_track(n, reference.source)?;
            if !keys.insert(String::from(track.reference.key.clone())) {
                return Err(ProviderError::InvalidReply);
            }
            tracks.push(AlbumTrack {
                track,
                disc: n
                    .attribute("parentIndex")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1),
                number: n
                    .attribute("index")
                    .and_then(|v| v.parse().ok())
                    .ok_or(ProviderError::InvalidReply)?,
            });
        }
        if tracks.len() > 500 {
            return Err(ProviderError::TooLarge);
        }
        if page_count(doc.root_element(), 0, tracks.len())? != tracks.len() as u64
            || album
                .track_count
                .is_some_and(|count| tracks.len() as u64 != count.get())
        {
            return Err(ProviderError::InvalidReply);
        }
        album.track_count = Some(Counter::new(tracks.len() as u64));
        tracks.sort_by_key(|t| (t.disc, t.number));
        let encoded =
            serde_json::to_vec(&(&album, &tracks)).map_err(|_| ProviderError::InvalidReply)?;
        let revision = format!("{:x}", Sha256::digest(encoded));
        // Artwork failure never turns a complete, playable album into an error.
        let artwork = if thumb.is_empty() {
            None
        } else {
            self.album_artwork(s, id, &thumb)
                .await
                .ok()
                .map(|bytes| format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)))
        };
        let artwork_unavailable = artwork.is_none();
        Ok(AlbumSnapshot {
            album,
            revision,
            tracks,
            artwork,
            artwork_unavailable,
        })
    }
    pub(super) async fn album_cover(&self, reference: AlbumRef) -> Result<Vec<u8>, ProviderError> {
        let s = self
            .sources
            .get(&reference.source)
            .ok_or(ProviderError::UnknownSource)?;
        let id = key(&reference.key)?;
        let body = xml(
            &self.http,
            path(&s.base, &["library", "metadata", id]),
            s.headers.clone(),
        )
        .await?;
        let doc = Document::parse(&body).map_err(|_| ProviderError::InvalidReply)?;
        let node = doc
            .root_element()
            .children()
            .find(|n| n.has_tag_name("Directory"))
            .ok_or(ProviderError::InvalidReply)?;
        if node
            .attribute("librarySectionID")
            .or_else(|| doc.root_element().attribute("librarySectionID"))
            != Some(s.key.as_str())
            || doc
                .root_element()
                .attribute("librarySectionID")
                .is_some_and(|id| id != s.key)
            || album(node, reference.source)?.reference != reference
        {
            return Err(ProviderError::InvalidReply);
        }
        let thumb = attr(node, "thumb");
        if thumb.is_empty() {
            return Err(ProviderError::MissingTrack);
        }
        self.album_artwork(s, id, &thumb).await
    }
    async fn album_artwork(
        &self,
        s: &Source,
        id: &str,
        thumb: &str,
    ) -> Result<Vec<u8>, ProviderError> {
        // Only an album's own numeric thumb path can be sent to the image transcoder.
        // No arbitrary remote URL, query string, token or alternate endpoint is accepted.
        let prefix = format!("/library/metadata/{id}/thumb");
        if thumb != prefix
            && !thumb
                .strip_prefix(&(prefix + "/"))
                .is_some_and(|suffix| key(suffix).is_ok())
        {
            return Err(ProviderError::InvalidReply);
        }
        let mut url = path(&s.base, &["photo", ":", "transcode"]);
        url.query_pairs_mut()
            .append_pair("url", thumb)
            .append_pair("width", "320")
            .append_pair("height", "320")
            .append_pair("format", "jpg")
            .append_pair("upscale", "0");
        let mut h = s.headers.clone();
        h.insert(ACCEPT, HeaderValue::from_static("image/jpeg"));
        let response = self
            .http
            .get(url)
            .headers(h)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|_| ProviderError::Network)?;
        let mut response = checked(response).await?;
        if response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap_or("").trim())
            != Some("image/jpeg")
        {
            return Err(ProviderError::InvalidReply);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ProviderError::Network)? {
            if bytes.len() + chunk.len() > 256 * 1024 {
                return Err(ProviderError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        if !bounded_jpeg(&bytes) {
            return Err(ProviderError::InvalidReply);
        }
        Ok(bytes)
    }
}
// Bound decoder dimensions as well as transport size. QML handles actual decoding.
fn bounded_jpeg(bytes: &[u8]) -> bool {
    if !bytes.starts_with(&[0xff, 0xd8]) || !bytes.ends_with(&[0xff, 0xd9]) {
        return false;
    }
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xff {
            return false;
        }
        while i < bytes.len() && bytes[i] == 0xff {
            i += 1;
        }
        let Some(&marker) = bytes.get(i) else {
            return false;
        };
        i += 1;
        if marker == 0xda || marker == 0xd9 {
            return false;
        }
        if i + 2 > bytes.len() {
            return false;
        }
        let size = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
        if size < 2 || i + size > bytes.len() {
            return false;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if size < 8 {
                return false;
            }
            let height = u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]);
            let width = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]);
            return (1..=512).contains(&height) && (1..=512).contains(&width);
        }
        i += size;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        extract::{OriginalUri, State},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::get,
    };
    use std::sync::{Arc, Mutex};
    const JPEG: &[u8] = include_bytes!("../../tests/fixtures/cover.jpg");
    #[derive(Clone)]
    struct FixtureState {
        mode: u8,
        requests: Arc<Mutex<Vec<String>>>,
    }
    async fn serve(
        State(f): State<FixtureState>,
        OriginalUri(uri): OriginalUri,
        h: HeaderMap,
    ) -> axum::response::Response {
        assert_eq!(h.get("x-plex-token").unwrap(), "fixture-secret");
        assert!(!uri.to_string().contains("fixture-secret"));
        f.requests.lock().unwrap().push(uri.to_string());
        let url = Url::parse(&format!("http://fixture{uri}")).unwrap();
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        let body = match uri.path() {
            "/library/sections/7/all" => {
                assert_eq!(q.get("type").unwrap(), "9");
                assert_eq!(q.get("title").unwrap(), "Café & live");
                assert_eq!(h.get("x-plex-container-size").unwrap(), "100");
                let offset = h.get("x-plex-container-start").unwrap().to_str().unwrap();
                format!(
                    "<MediaContainer offset='{offset}' size='1' totalSize='102'><Directory type='album' ratingKey='20' title='Album &amp; title' parentTitle='Artist' leafCount='3' year='2024'/></MediaContainer>"
                )
            }
            "/library/metadata/20" => {
                let section = if f.mode == 1 { "8" } else { "7" };
                let thumb = if f.mode == 3 {
                    "https://external.invalid/secret"
                } else {
                    "/library/metadata/20/thumb/123"
                };
                let count = if matches!(f.mode, 7 | 8) {
                    ""
                } else {
                    "leafCount='3'"
                };
                format!(
                    "<MediaContainer librarySectionID='{section}'><Directory type='album' ratingKey='20' title='Album' parentTitle='Artist' {count} thumb='{thumb}'/></MediaContainer>"
                )
            }
            "/library/metadata/20/children" => {
                assert_eq!(h.get("x-plex-container-size").unwrap(), "500");
                let total = if f.mode == 2 {
                    "totalSize='4'"
                } else if f.mode == 8 {
                    ""
                } else {
                    "totalSize='3'"
                };
                let parent = if f.mode == 4 { "21" } else { "20" };
                format!(
                    "<MediaContainer size='3' {total}><Track ratingKey='3' parentRatingKey='{parent}' title='Disc 2' parentIndex='2' index='1'/><Track ratingKey='2' parentRatingKey='20' title='Second' parentIndex='1' index='2'/><Track ratingKey='1' parentRatingKey='20' title='First' parentIndex='1' index='1'/></MediaContainer>"
                )
            }
            "/photo/:/transcode" => {
                assert_eq!(q.get("url").unwrap(), "/library/metadata/20/thumb/123");
                assert_eq!(q.get("width").unwrap(), "320");
                assert_eq!(q.get("format").unwrap(), "jpg");
                if f.mode == 5 {
                    return (StatusCode::FORBIDDEN, "private detail").into_response();
                }
                let data = if f.mode == 6 {
                    vec![0; 256 * 1024 + 1]
                } else {
                    JPEG.to_vec()
                };
                return ([("content-type", "image/jpeg")], data).into_response();
            }
            _ => return StatusCode::NOT_FOUND.into_response(),
        };
        ([("content-type", "application/xml")], body).into_response()
    }
    async fn fixture(
        mode: u8,
    ) -> (
        PlexProvider,
        SourceId,
        Arc<Mutex<Vec<String>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new().fallback(get(serve)).with_state(FixtureState {
            mode,
            requests: requests.clone(),
        });
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let source = SourceId::new();
        let provider = PlexProvider {
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            sources: HashMap::from([(
                source,
                Source {
                    base,
                    headers: headers("fixture", Some("fixture-secret")).unwrap(),
                    key: "7".into(),
                    server_id: "fixture-server".into(),
                    view: Library {
                        id: source,
                        title: "Music".into(),
                        server: "Fixture".into(),
                    },
                },
            )]),
        };
        (provider, source, requests, task)
    }
    #[tokio::test]
    async fn paged_album_title_search_keeps_source_identity_and_escaped_queries() {
        let (p, source, _, task) = fixture(0).await;
        let page = p.albums(source, 100, "Café & live".into()).await.unwrap();
        assert_eq!(page.offset.get(), 100);
        assert!(page.has_more);
        assert_eq!(page.albums[0].title, "Album & title");
        assert_eq!(page.albums[0].reference.source, source);
        assert!(matches!(
            p.albums(SourceId::new(), 0, "".into()).await,
            Err(ProviderError::UnknownSource)
        ));
        task.abort();
    }
    #[tokio::test]
    async fn complete_album_orders_discs_and_delivers_only_bounded_cover_bytes() {
        for mode in [0, 7] {
            let (p, source, requests, task) = fixture(mode).await;
            let view = p
                .album(AlbumRef {
                    source,
                    key: "20".into(),
                })
                .await
                .unwrap();
            assert_eq!(
                view.tracks
                    .iter()
                    .map(|t| t.track.title.as_str())
                    .collect::<Vec<_>>(),
                ["First", "Second", "Disc 2"]
            );
            assert!(
                view.tracks
                    .iter()
                    .all(|t| t.track.reference.source == source)
            );
            assert_eq!(
                view.artwork.unwrap(),
                format!("data:image/jpeg;base64,{}", STANDARD.encode(JPEG))
            );
            assert!(!view.artwork_unavailable);
            assert_eq!(view.revision.len(), 64);
            assert!(
                requests
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|r| !r.contains("fixture-secret"))
            );
            task.abort();
        }
    }
    #[tokio::test]
    async fn cross_library_mixed_parent_and_partial_album_responses_are_rejected() {
        for mode in [1, 2, 4, 8] {
            let (p, source, requests, task) = fixture(mode).await;
            assert!(matches!(
                p.album(AlbumRef {
                    source,
                    key: "20".into()
                })
                .await,
                Err(ProviderError::InvalidReply)
            ));
            assert!(
                !requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|p| p.starts_with("/photo/"))
            );
            task.abort();
        }
    }
    #[tokio::test]
    async fn missing_denied_oversized_or_external_artwork_does_not_break_album_tracks() {
        for mode in [3, 5, 6] {
            let (p, source, requests, task) = fixture(mode).await;
            let view = p
                .album(AlbumRef {
                    source,
                    key: "20".into(),
                })
                .await
                .unwrap();
            assert_eq!(view.tracks.len(), 3);
            assert!(view.artwork.is_none() && view.artwork_unavailable);
            if mode == 3 {
                assert!(
                    !requests
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|p| p.starts_with("/photo/"))
                );
            }
            task.abort();
        }
    }
    #[tokio::test]
    async fn thumbnail_access_is_metadata_only_and_reuses_the_cover_guards() {
        let (p, source, requests, task) = fixture(0).await;
        assert_eq!(
            p.cover(AlbumRef {
                source,
                key: "20".into()
            })
            .await
            .unwrap(),
            JPEG
        );
        assert!(
            !requests
                .lock()
                .unwrap()
                .iter()
                .any(|path| path.contains("children"))
        );
        task.abort();
        for mode in [1, 3, 5, 6] {
            let (p, source, _, task) = fixture(mode).await;
            assert!(
                p.cover(AlbumRef {
                    source,
                    key: "20".into()
                })
                .await
                .is_err()
            );
            task.abort();
        }
    }
    #[test]
    fn malformed_and_unbounded_jpeg_dimensions_are_rejected_before_qml() {
        assert!(bounded_jpeg(JPEG));
        for data in [
            &b"<html>secret</html>"[..],
            &[0xff, 0xd8, 0xff, 0xe0, 0xff, 0xff, 0xff, 0xd9],
            &[
                0xff, 0xd8, 0xff, 0xc0, 0, 8, 8, 0xff, 0xff, 0xff, 0xff, 3, 0xff, 0xd9,
            ],
        ] {
            assert!(!bounded_jpeg(data));
        }
        assert!(key("//external").is_err());
        assert!(key("20?X-Plex-Token=secret").is_err());
    }
}
