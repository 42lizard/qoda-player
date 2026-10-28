use super::*;

// Metadata contributes only a validated suffix, never a filesystem path.
pub(super) fn original_extension(
    filename: Option<&str>,
    part_path: &str,
    container: Option<&str>,
) -> String {
    fn suffix(path: &str) -> Option<&str> {
        path.rsplit(['/', '\\'])
            .next()
            .and_then(|name| name.rsplit_once('.').map(|(_, extension)| extension))
    }
    [filename.and_then(suffix), suffix(part_path), container]
        .into_iter()
        .flatten()
        .find(|extension| {
            !extension.is_empty()
                && extension.len() <= 12
                && extension.bytes().all(|b| b.is_ascii_alphanumeric())
        })
        .unwrap_or("audio")
        .to_ascii_lowercase()
}
impl PlexProvider {
    pub(super) async fn export_original(
        &self,
        source: SourceId,
        key: ProviderTrackKey,
    ) -> Result<Download, ProviderError> {
        let mut download = self.original(source, key, true).await?;
        let head = download.media.stream.open(None, true).await?;
        if head.status != 200
            || head.content_range.is_some()
            || head
                .content_length
                .zip(download.expected_length)
                .is_some_and(|(a, b)| a != b)
        {
            return Err(ProviderError::UnsupportedMedia);
        }
        download.expected_length = download.expected_length.or(head.content_length);
        download.media.version = format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "{}:{:?}:{:?}",
                    download.media.version, head.etag, download.expected_length
                )
                .as_bytes()
            )
        );
        download.media.stream = Box::new(Original {
            stream: download.media.stream,
            etag: head.etag,
            length: download.expected_length,
        });
        Ok(download)
    }
}
struct Original {
    stream: Box<dyn MediaSource>,
    etag: Option<String>,
    length: Option<u64>,
}
impl MediaSource for Original {
    fn open(&self, range: Option<ByteRange>, head: bool) -> ProviderFuture<'_, MediaResponse> {
        Box::pin(async move {
            if range.is_some() {
                return Err(ProviderError::InvalidRange);
            }
            let response = self.stream.open(None, head).await?;
            if response.status != 200
                || response.content_range.is_some()
                || self
                    .etag
                    .as_ref()
                    .is_some_and(|expected| response.etag.as_ref() != Some(expected))
                || self
                    .length
                    .zip(response.content_length)
                    .is_some_and(|(a, b)| a != b)
            {
                return Err(ProviderError::UnsupportedMedia);
            }
            Ok(response)
        })
    }
}
#[cfg(test)]
mod tests {
    use super::super::*;
    use axum::{
        Router,
        body::Body,
        extract::Query,
        http::{Response, StatusCode},
        routing::get,
    };
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
    #[test]
    fn original_suffix_preserves_safe_file_extensions_without_trusting_paths() {
        for (file, path, container, expected) in [
            (
                Some("/private/music/original.M4A"),
                "/library/parts/1/file",
                Some("mp4"),
                "m4a",
            ),
            (
                Some("C:\\music\\original.WV"),
                "/library/parts/1/file.flac",
                Some("flac"),
                "wv",
            ),
            (
                Some("../../no.unsafe%2f"),
                "/library/parts/1/file.OGG",
                Some("opus"),
                "ogg",
            ),
            (None, "/library/parts/1/file", Some("FLAC"), "flac"),
            (None, "/library/parts/1/file", Some("bad/path"), "audio"),
        ] {
            assert_eq!(super::original_extension(file, path, container), expected);
        }
    }
    #[tokio::test]
    async fn original_download_enforces_permission_identity_complete_responses_and_no_redirects() {
        let foreign_requests = Arc::new(AtomicUsize::new(0));
        let hits = foreign_requests.clone();
        let foreign = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let foreign_url = format!("http://{}/foreign", foreign.local_addr().unwrap());
        let other = tokio::spawn(async move {
            axum::serve(
                foreign,
                Router::new().fallback(get(move || {
                    let hits = hits.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        "not followed"
                    }
                })),
            )
            .await
            .unwrap()
        });
        let mode = Arc::new(AtomicU8::new(0));
        let metadata_mode = mode.clone();
        let part_mode = mode.clone();
        let app=Router::new().route("/library/metadata/1",get(move||{let mode=metadata_mode.clone();async move{
            let value=mode.load(Ordering::SeqCst);
            let size=if value==6{""}else{"size='4'"};
            let extra=if value==4{"<Part id='4' key='/library/parts/4/123/file.flac'/>"}else{""};
            format!("<MediaContainer><Track ratingKey='1' librarySectionID='7' title='Original'><Media id='2' container='flac'><Part id='3' {size} key='/library/parts/3/123/file.flac?download=0'/>{extra}</Media></Track></MediaContainer>")
        }})).route("/library/parts/3/123/file.flac",get(move|Query(query):Query<HashMap<String,String>>,headers:HeaderMap|{
            let mode=part_mode.clone();let foreign_url=foreign_url.clone();async move {
                assert_eq!(query.len(),1);assert_eq!(query.get("download").map(String::as_str),Some("1"));
                assert_eq!(headers.get("X-Plex-Token").unwrap(),"secret");
                let value=mode.load(Ordering::SeqCst);
                let mut response=Response::builder().header("content-type","audio/flac").header("etag",if value==2{"\"v2\""}else{"\"v1\""});
                if value==1{return response.status(StatusCode::FORBIDDEN).body(Body::from("denied")).unwrap()}
                if value==5{return response.status(StatusCode::TEMPORARY_REDIRECT).header("location",foreign_url).body(Body::empty()).unwrap()}
                if value==3{response=response.status(StatusCode::PARTIAL_CONTENT).header("content-range","bytes 0-3/4");}
                if value==6 {return response.body(Body::from_stream(futures_util::stream::once(async{Ok::<_,std::io::Error>(Bytes::from_static(b"fLaC"))}))).unwrap()}
                response.body(Body::from("fLaC")).unwrap()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let id = SourceId::new();
        let provider = PlexProvider {
            timeline_capabilities: Default::default(),
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            account_scope: "a".repeat(64),
            unavailable_sources: HashSet::new(),
            unavailable_servers: 0,
            sources: HashMap::from([(
                id,
                Source {
                    base,
                    headers: headers("fixture", Some("secret")).unwrap(),
                    key: "7".into(),
                    view: Library {
                        id,
                        title: "Music".into(),
                        server: "Fixture".into(),
                    },
                    server_id: "server".into(),
                },
            )]),
        };
        let key = || "1".to_owned().try_into().unwrap();
        let original = provider.download(id, key()).await.unwrap();
        assert_eq!(original.extension, "flac");
        assert_eq!(original.expected_length, Some(4));
        let mut response = original.media.stream.open(None, false).await.unwrap();
        assert_eq!(
            response.body.next().await.unwrap().unwrap().as_ref(),
            b"fLaC"
        );
        mode.store(1, Ordering::SeqCst);
        assert!(matches!(
            original.media.stream.open(None, false).await,
            Err(ProviderError::DownloadDenied)
        ));
        assert!(matches!(
            provider.download(id, key()).await,
            Err(ProviderError::DownloadDenied)
        ));
        mode.store(2, Ordering::SeqCst);
        assert!(matches!(
            original.media.stream.open(None, false).await,
            Err(ProviderError::UnsupportedMedia)
        ));
        mode.store(3, Ordering::SeqCst);
        assert!(matches!(
            original.media.stream.open(None, false).await,
            Err(ProviderError::UnsupportedMedia)
        ));
        mode.store(4, Ordering::SeqCst);
        assert!(matches!(
            provider.download(id, key()).await,
            Err(ProviderError::UnsupportedMedia)
        ));
        mode.store(5, Ordering::SeqCst);
        assert!(matches!(
            provider.download(id, key()).await,
            Err(ProviderError::Network)
        ));
        assert_eq!(foreign_requests.load(Ordering::SeqCst), 0);
        mode.store(6, Ordering::SeqCst);
        let unknown = provider.download(id, key()).await.unwrap();
        assert!(unknown.expected_length.is_none());
        let mut response = unknown.media.stream.open(None, false).await.unwrap();
        assert!(response.content_length.is_none());
        assert_eq!(
            response.body.next().await.unwrap().unwrap().as_ref(),
            b"fLaC"
        );
        server.abort();
        other.abort();
    }
}
