//! Artist identities stay scoped to the selected music library.
use super::albums::{key, page_count, page_headers};
use super::*;

fn artist(node: Node<'_, '_>, source: SourceId) -> Result<Artist, ProviderError> {
    if node.attribute("type") != Some("artist") {
        return Err(ProviderError::InvalidReply);
    }
    Ok(Artist {
        reference: ArtistRef {
            source,
            key: key(node.attribute("ratingKey").unwrap_or_default())?.into(),
        },
        title: display_attr(node, "title"),
    })
}
impl PlexProvider {
    pub(super) async fn artist_list(
        &self,
        source: SourceId,
        offset: u64,
        query: String,
    ) -> Result<ArtistPage, ProviderError> {
        let s = self
            .sources
            .get(&source)
            .ok_or(ProviderError::UnknownSource)?;
        if query.len() > 2048 {
            return Err(ProviderError::InvalidReply);
        }
        let mut url = path(&s.base, &["library", "sections", &s.key, "all"]);
        url.query_pairs_mut()
            .append_pair("type", "8")
            .append_pair("sort", "titleSort:asc");
        if !query.is_empty() {
            url.query_pairs_mut().append_pair("title", &query);
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
        let mut seen = std::collections::HashSet::new();
        let artists = root
            .children()
            .filter(|n| n.has_tag_name("Directory"))
            .map(|n| {
                if n.attribute("librarySectionID")
                    .is_some_and(|id| id != s.key)
                {
                    return Err(ProviderError::InvalidReply);
                }
                let artist = artist(n, source)?;
                if !seen.insert(artist.reference.key.clone()) {
                    return Err(ProviderError::InvalidReply);
                }
                Ok(artist)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if artists.len() > 100 {
            return Err(ProviderError::TooLarge);
        }
        let total = page_count(root, offset, artists.len())?;
        Ok(ArtistPage {
            source,
            offset: Counter::new(offset),
            query,
            has_more: offset.saturating_add(artists.len() as u64) < total,
            artists,
        })
    }
    pub(super) async fn albums_by_artist(
        &self,
        reference: ArtistRef,
        offset: u64,
        query: String,
    ) -> Result<AlbumPage, ProviderError> {
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
        let root = doc.root_element();
        let node = root
            .children()
            .find(|n| n.has_tag_name("Directory"))
            .ok_or(ProviderError::InvalidReply)?;
        if root
            .attribute("librarySectionID")
            .is_some_and(|id| id != s.key)
            || node
                .attribute("librarySectionID")
                .or_else(|| root.attribute("librarySectionID"))
                != Some(s.key.as_str())
        {
            return Err(ProviderError::InvalidReply);
        }
        let artist = artist(node, reference.source)?;
        if artist.reference != reference {
            return Err(ProviderError::InvalidReply);
        }
        self.album_list_for(reference.source, offset, query, Some(artist))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        extract::{OriginalUri, State},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
    };
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct Fixture {
        mode: u8,
        paths: Arc<Mutex<Vec<String>>>,
    }
    async fn serve(
        State(f): State<Fixture>,
        OriginalUri(uri): OriginalUri,
        h: HeaderMap,
    ) -> axum::response::Response {
        assert_eq!(h.get("x-plex-token").unwrap(), "fixture-secret");
        assert!(!uri.to_string().contains("fixture-secret"));
        f.paths.lock().unwrap().push(uri.path().to_owned());
        let url = Url::parse(&format!("http://fixture{uri}")).unwrap();
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        let section = if f.mode == 1 { "8" } else { "7" };
        let body = match uri.path() {
            "/library/metadata/10" => format!(
                "<MediaContainer librarySectionID='{section}'><Directory type='artist' ratingKey='10' title='Same &amp; name'/></MediaContainer>"
            ),
            "/library/sections/7/all" => {
                assert_eq!(h.get("x-plex-container-size").unwrap(), "100");
                let offset = if f.mode == 3 {
                    "0"
                } else {
                    h.get("x-plex-container-start").unwrap().to_str().unwrap()
                };
                if q.get("type").map(String::as_str) == Some("8") {
                    assert_eq!(q.get("title").unwrap(), "A & B");
                    format!(
                        "<MediaContainer librarySectionID='{section}' offset='{offset}' size='2' totalSize='103'><Directory type='artist' ratingKey='10' title='Same &amp; name'/><Directory type='artist' ratingKey='11' title='Same &amp; name'/></MediaContainer>"
                    )
                } else {
                    assert_eq!(q.get("type").unwrap(), "9");
                    assert_eq!(q.get("artist.id").unwrap(), "10");
                    assert_eq!(q.get("title").unwrap(), "Live & rare");
                    let parent = if f.mode == 2 { "11" } else { "10" };
                    format!(
                        "<MediaContainer offset='{offset}' size='1' totalSize='102'><Directory type='album' ratingKey='20' parentRatingKey='{parent}' parentTitle='Same &amp; name' title='Album'/></MediaContainer>"
                    )
                }
            }
            _ => return StatusCode::NOT_FOUND.into_response(),
        };
        ([("content-type", "application/xml")], body).into_response()
    }
    async fn fixture(mode: u8) -> (PlexProvider, SourceId, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let app = Router::new().fallback(serve).with_state(Fixture {
            mode,
            paths: Default::default(),
        });
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let source = SourceId::new();
        let provider = PlexProvider {
            timeline_capabilities: Default::default(),
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
                    base,
                    key: "7".into(),
                    headers: headers("fixture", Some("fixture-secret")).unwrap(),
                    server_id: "fixture-server".into(),
                    view: Library {
                        id: source,
                        title: "Music".into(),
                        server: "Fixture".into(),
                    },
                },
            )]),
        };
        (provider, source, task)
    }
    #[tokio::test]
    async fn paged_artist_search_keeps_distinct_ids_for_duplicate_names() {
        let (p, source, task) = fixture(0).await;
        let page = p.artists(source, 100, "A & B".into()).await.unwrap();
        assert_eq!(page.offset.get(), 100);
        assert!(page.has_more);
        assert_eq!(page.artists.len(), 2);
        assert_eq!(page.artists[0].title, page.artists[1].title);
        assert_ne!(page.artists[0].reference, page.artists[1].reference);
        assert!(page.artists.iter().all(|a| a.reference.source == source));
        task.abort();
    }
    #[tokio::test]
    async fn artist_album_filter_is_identity_based_paged_and_title_searchable() {
        let (p, source, task) = fixture(0).await;
        let reference = ArtistRef {
            source,
            key: "10".into(),
        };
        let page = p
            .artist_albums(reference.clone(), 100, "Live & rare".into())
            .await
            .unwrap();
        assert_eq!(page.artist.unwrap().reference, reference);
        assert_eq!(page.albums[0].artist_ref.as_ref(), Some(&reference));
        assert_eq!(page.offset.get(), 100);
        assert!(page.has_more);
        assert!(matches!(
            p.artist_albums(
                ArtistRef {
                    source,
                    key: "//external".into()
                },
                0,
                "".into()
            )
            .await,
            Err(ProviderError::InvalidReply)
        ));
        task.abort();
    }
    #[tokio::test]
    async fn foreign_libraries_wrong_artist_parents_and_bad_offsets_are_rejected() {
        for mode in [1, 2, 3] {
            let (p, source, task) = fixture(mode).await;
            assert!(matches!(
                p.artist_albums(
                    ArtistRef {
                        source,
                        key: "10".into()
                    },
                    100,
                    "Live & rare".into()
                )
                .await,
                Err(ProviderError::InvalidReply)
            ));
            if mode != 2 {
                assert!(matches!(
                    p.artists(source, 100, "A & B".into()).await,
                    Err(ProviderError::InvalidReply)
                ));
            }
            task.abort();
        }
    }
}
