//! Optional authenticated progressive audio conversion; never exposes server URLs.
use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use crate::TranscodeCodec as Codec;
#[derive(Clone)]
struct Conversion {
    http: Client,
    source: Source,
    key: String,
    codec: Codec,
    position: f64,
}
struct Session {
    request: Conversion,
    id: String,
    operation: tokio::sync::Mutex<()>,
    stopped: AtomicBool,
}
impl Session {
    fn endpoint(&self, operation: &str) -> Url {
        let mut url = self
            .request
            .source
            .base
            .join(&format!("music/:/transcode/universal/{operation}"))
            .expect("fixed relative endpoint");
        if operation == "stop" {
            url.query_pairs_mut().append_pair("session", &self.id);
            return url;
        }
        let codec = self.request.codec.name();
        let profile = format!(
            "add-transcode-target(type=musicProfile&context=streaming&protocol=http&container={codec}&audioCodec={codec}&replace=true)"
        );
        url.query_pairs_mut().extend_pairs([
            ("path", format!("/library/metadata/{}", self.request.key)),
            ("mediaIndex", "0".into()),
            ("partIndex", "0".into()),
            ("protocol", "http".into()),
            ("offset", self.request.position.to_string()),
            ("fastSeek", "0".into()),
            ("directPlay", "0".into()),
            ("directStream", "0".into()),
            ("directStreamAudio", "0".into()),
            ("audioCodec", codec.into()),
            ("container", codec.into()),
            ("session", self.id.clone()),
            ("X-Plex-Session-Identifier", self.id.clone()),
            ("X-Plex-Platform", "Generic".into()),
            ("X-Plex-Client-Profile-Name", "Generic".into()),
            ("X-Plex-Client-Profile-Extra", profile),
        ]);
        url
    }
    async fn decision(&self) -> Result<(), ProviderError> {
        let response = self
            .request
            .http
            .get(self.endpoint("decision"))
            .headers(self.request.source.headers.clone())
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|_| ProviderError::Network)?;
        if matches!(response.status().as_u16(), 400 | 404 | 406 | 415) {
            return Err(ProviderError::Unsupported);
        }
        let body = text(checked(response).await?).await?;
        let doc = Document::parse(&body).map_err(|_| ProviderError::InvalidReply)?;
        let root = doc.root_element();
        if !root.has_tag_name("MediaContainer") {
            return Err(ProviderError::InvalidReply);
        }
        if root
            .attribute("generalDecisionCode")
            .and_then(|v| v.parse::<u32>().ok())
            .is_none_or(|n| !(1000..2000).contains(&n))
        {
            return Err(ProviderError::Unsupported);
        }
        let media = doc
            .descendants()
            .find(|n| n.has_tag_name("Media"))
            .ok_or(ProviderError::Unsupported)?;
        if media.attribute("audioCodec") != Some(self.request.codec.name())
            || media.attribute("container") != Some(self.request.codec.name())
        {
            return Err(ProviderError::Unsupported);
        }
        Ok(())
    }
    async fn stop(&self) -> Result<(), ProviderError> {
        let _operation = self.operation.lock().await;
        if self.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        let response = self
            .request
            .http
            .get(self.endpoint("stop"))
            .headers(self.request.source.headers.clone())
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map_err(|_| ProviderError::Network)?;
        if !matches!(response.status().as_u16(), 200..=299 | 404) {
            return Err(ProviderError::Network);
        }
        self.stopped.store(true, Ordering::Release);
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if !self.stopped.load(Ordering::Acquire) {
            let http = self.request.http.clone();
            let endpoint = self.endpoint("stop");
            let headers = self.request.source.headers.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = http
                        .get(endpoint)
                        .headers(headers)
                        .timeout(Duration::from_secs(3))
                        .send()
                        .await;
                });
            }
        }
    }
}
struct BodyLease {
    session: Arc<Session>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl Drop for BodyLease {
    fn drop(&mut self) {
        let permit = self.permit.take();
        if !self.session.stopped.load(Ordering::Acquire) {
            let session = self.session.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = session.stop().await;
                    drop(permit);
                });
            }
        }
    }
}
#[derive(Default)]
struct Lifetime {
    closed: bool,
    sessions: Vec<Arc<Session>>,
}
struct Transcoded {
    request: Conversion,
    lifetime: Mutex<Lifetime>,
    readers: Arc<tokio::sync::Semaphore>,
}
impl Transcoded {
    fn session(&self) -> Arc<Session> {
        Arc::new(Session {
            request: self.request.clone(),
            id: uuid::Uuid::new_v4().to_string(),
            operation: Default::default(),
            stopped: AtomicBool::new(false),
        })
    }
}
impl MediaSource for Transcoded {
    fn timeline_offset(&self) -> f64 {
        self.request.position
    }
    fn transcoded(&self) -> bool {
        true
    }
    fn close(&self) -> ProviderFuture<'_, ()> {
        Box::pin(async move {
            let sessions = {
                let mut lifetime = self.lifetime.lock().map_err(|_| ProviderError::Network)?;
                lifetime.closed = true;
                std::mem::take(&mut lifetime.sessions)
            };
            let results =
                futures_util::future::join_all(sessions.iter().map(|session| session.stop())).await;
            results
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .map(|_| ())
        })
    }
    fn open(&self, range: Option<ByteRange>, head: bool) -> ProviderFuture<'_, MediaResponse> {
        Box::pin(async move {
            if self
                .lifetime
                .lock()
                .map_err(|_| ProviderError::Network)?
                .closed
            {
                return Err(ProviderError::Unsupported);
            }
            if head {
                return Ok(MediaResponse {
                    status: 200,
                    content_type: Some(format!("audio/{}", self.request.codec.name())),
                    content_length: None,
                    content_range: None,
                    etag: None,
                    body: Box::pin(futures_util::stream::empty()),
                });
            }
            let permit = self
                .readers
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| ProviderError::Network)?;
            let session = self.session();
            {
                let mut lifetime = self.lifetime.lock().map_err(|_| ProviderError::Network)?;
                if lifetime.closed {
                    return Err(ProviderError::Unsupported);
                }
                lifetime
                    .sessions
                    .retain(|s| !s.stopped.load(Ordering::Acquire));
                if lifetime.sessions.len() >= 8 {
                    return Err(ProviderError::Unsupported);
                }
                lifetime.sessions.push(session.clone());
            }
            let operation = session.operation.lock().await;
            session.decision().await?;
            let mut request = self
                .request
                .http
                .get(session.endpoint(&format!("start.{}", self.request.codec.name())))
                .headers(self.request.source.headers.clone());
            if let Some(range) = range {
                request = request.header(RANGE, range.header());
            }
            let response =
                checked(request.send().await.map_err(|_| ProviderError::Network)?).await?;
            let status = response.status().as_u16();
            if !matches!(status, 200 | 206)
                || self
                    .lifetime
                    .lock()
                    .map_err(|_| ProviderError::Network)?
                    .closed
            {
                return Err(ProviderError::Unsupported);
            }
            let header = |name| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let content_type = header("content-type");
            if content_type
                .as_deref()
                .is_some_and(|v| v.contains("mpegurl") || v.contains("dash") || v.contains("text/"))
            {
                return Err(ProviderError::Unsupported);
            }
            let content_length = response.content_length();
            let content_range = header("content-range");
            drop(operation);
            let stream = response.bytes_stream();
            let body = futures_util::stream::unfold(
                (
                    stream,
                    BodyLease {
                        session,
                        permit: Some(permit),
                    },
                    false,
                ),
                |(mut stream, lease, finished)| async move {
                    if finished {
                        return None;
                    }
                    match stream.next().await {
                        Some(Ok(bytes)) => Some((Ok(bytes), (stream, lease, false))),
                        Some(Err(_)) => {
                            let _ = lease.session.stop().await;
                            Some((Err(ProviderError::Network), (stream, lease, true)))
                        }
                        None => {
                            let _ = lease.session.stop().await;
                            None
                        }
                    }
                },
            );
            Ok(MediaResponse {
                status,
                content_type,
                content_length,
                content_range,
                etag: None,
                body: Box::pin(body),
            })
        })
    }
}
impl PlexProvider {
    pub(super) async fn transcode(
        &self,
        request: TranscodeRequest,
    ) -> Result<PreparedMedia, ProviderError> {
        let TranscodeRequest {
            reference,
            version,
            position,
            targets,
        } = request;
        if !position.is_finite() || !(0.0..=604800.0).contains(&position) || targets.is_empty() {
            return Err(ProviderError::Unsupported);
        }
        let original = self
            .prepare(reference.source, reference.key.clone())
            .await?;
        if original.version != version || original.track.reference != reference {
            return Err(ProviderError::UnsupportedMedia);
        }
        let source = self
            .sources
            .get(&reference.source)
            .ok_or(ProviderError::UnknownSource)?
            .clone();
        for codec in [Codec::Flac, Codec::Mp3]
            .into_iter()
            .filter(|c| targets.contains(c))
        {
            let stream = Transcoded {
                request: Conversion {
                    http: self.http.clone(),
                    source: source.clone(),
                    key: reference.key.clone().into(),
                    codec,
                    position,
                },
                lifetime: Default::default(),
                readers: Arc::new(tokio::sync::Semaphore::new(1)),
            };
            let session = stream.session();
            let result = session.decision().await;
            let cleanup = session.stop().await;
            match result {
                Ok(()) => {
                    cleanup?;
                    return Ok(PreparedMedia {
                        track: original.track,
                        version: original.version,
                        stream: Box::new(stream),
                    });
                }
                Err(ProviderError::Unsupported) => {
                    cleanup?;
                }
                Err(error) => return Err(error),
            }
        }
        Err(ProviderError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use axum::{
        Router,
        extract::State,
        http::{StatusCode, Uri},
    };
    use std::sync::Mutex;
    fn request(reference: TrackRef, version: String, position: f64) -> TranscodeRequest {
        TranscodeRequest {
            reference,
            version,
            position,
            targets: vec![TranscodeCodec::Flac, TranscodeCodec::Mp3],
        }
    }
    #[derive(Clone, Default)]
    struct Fixture {
        requests: Arc<Mutex<Vec<String>>>,
        reject_flac: bool,
        deny: bool,
    }
    async fn serve(state: State<Fixture>, uri: Uri) -> axum::response::Response {
        use axum::response::IntoResponse;
        let binary = uri.path().contains("/start.");
        let mut response = reply(state, uri).await.into_response();
        if binary {
            response
                .headers_mut()
                .insert("content-type", "application/octet-stream".parse().unwrap());
        }
        response
    }
    async fn reply(State(f): State<Fixture>, uri: Uri) -> (StatusCode, String) {
        f.requests.lock().unwrap().push(uri.to_string());
        if f.deny {
            return (StatusCode::FORBIDDEN, String::new());
        }
        if f.reject_flac
            && uri.path().ends_with("/decision")
            && uri.query().is_some_and(|q| q.contains("audioCodec=flac"))
        {
            return (StatusCode::UNSUPPORTED_MEDIA_TYPE, String::new());
        }
        match uri.path() {
            "/library/metadata/42" => (StatusCode::OK, "<MediaContainer><Track ratingKey='42' librarySectionID='1' title='Fixture'><Media id='7'><Part id='8' key='/library/parts/8/file'/></Media></Track></MediaContainer>".into()),
            "/music/:/transcode/universal/decision" => {
                let codec = if uri.query().is_some_and(|q|q.contains("audioCodec=mp3")) { "mp3" } else { "flac" };
                (StatusCode::OK, format!("<MediaContainer generalDecisionCode='1001'><Media audioCodec='{codec}' container='{codec}'><Part container='{codec}'/></Media></MediaContainer>"))
            },
            "/music/:/transcode/universal/start.mp3" => (StatusCode::OK, "ID3fixture".into()),
            "/music/:/transcode/universal/start.flac" => (StatusCode::OK, "fLaCfixture".into()),
            "/music/:/transcode/universal/stop" => (StatusCode::OK, String::new()),
            _ => (StatusCode::NOT_FOUND, String::new()),
        }
    }
    impl Fixture {
        async fn start(
            &self,
        ) -> (
            PlexProvider,
            TrackRef,
            tokio::task::JoinHandle<Result<(), std::io::Error>>,
        ) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
            let server = tokio::spawn(
                axum::serve(
                    listener,
                    Router::new().fallback(serve).with_state(self.clone()),
                )
                .into_future(),
            );
            let source = SourceId::new();
            let provider = PlexProvider {
                http: Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
                timeline_capabilities: Default::default(),
                account_scope: "fixture".into(),
                unavailable_sources: HashSet::new(),
                unavailable_servers: 0,
                sources: HashMap::from([(
                    source,
                    Source {
                        server_id: "fixture".into(),
                        view: Library {
                            id: source,
                            title: "Music".into(),
                            server: "Fixture".into(),
                        },
                        base,
                        headers: headers("fixture", Some("not-a-real-secret")).unwrap(),
                        key: "1".into(),
                    },
                )]),
            };
            let reference = TrackRef {
                source,
                key: "42".to_owned().try_into().unwrap(),
            };
            (provider, reference, server)
        }
    }
    #[tokio::test]
    async fn locally_unsupported_flac_is_skipped_and_source_offset_is_retained() {
        let f = Fixture::default();
        let (provider, reference, server) = f.start().await;
        let media = provider
            .prepare_transcoded(TranscodeRequest {
                reference,
                version: "7:8".into(),
                position: 12.5,
                targets: vec![TranscodeCodec::Mp3],
            })
            .await
            .unwrap();
        assert_eq!(media.stream.timeline_offset(), 12.5);
        let mut response = media.stream.open(None, false).await.unwrap();
        assert_eq!(
            response.body.next().await.unwrap().unwrap().as_ref(),
            b"ID3fixture"
        );
        media.stream.close().await.unwrap();
        let requests = f.requests.lock().unwrap();
        assert!(requests.iter().any(|r| r.contains("start.mp3")
            && r.contains("offset=12.5")
            && r.contains("fastSeek=0")));
        assert!(!requests.iter().any(|r| r.contains("audioCodec=flac")));
        server.abort();
    }
    #[tokio::test]
    async fn no_local_conversion_target_starts_no_server_work() {
        let f = Fixture::default();
        let (provider, reference, server) = f.start().await;
        assert!(matches!(
            provider
                .prepare_transcoded(TranscodeRequest {
                    reference,
                    version: "7:8".into(),
                    position: 0.0,
                    targets: vec![]
                })
                .await,
            Err(ProviderError::Unsupported)
        ));
        assert!(f.requests.lock().unwrap().is_empty());
        server.abort();
    }
    #[tokio::test]
    async fn unavailable_lossless_target_uses_mp3_but_access_denial_does_not_retry() {
        let f = Fixture {
            reject_flac: true,
            ..Default::default()
        };
        let (provider, reference, server) = f.start().await;
        let media = provider
            .prepare_transcoded(request(reference, "7:8".into(), 0.0))
            .await
            .unwrap();
        let mut response = media.stream.open(None, false).await.unwrap();
        assert_eq!(
            response.body.next().await.unwrap().unwrap().as_ref(),
            b"ID3fixture"
        );
        drop(response);
        media.stream.close().await.unwrap();
        assert!(
            f.requests
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.contains("start.mp3"))
        );
        server.abort();
        let f = Fixture {
            deny: true,
            ..Default::default()
        };
        let (provider, reference, server) = f.start().await;
        assert!(matches!(
            provider
                .prepare_transcoded(request(reference, "7:8".into(), 0.0))
                .await,
            Err(ProviderError::AccessDenied)
        ));
        assert_eq!(f.requests.lock().unwrap().len(), 1);
        server.abort();
    }
    #[tokio::test]
    async fn changed_original_version_never_starts_server_conversion() {
        let f = Fixture::default();
        let (provider, reference, server) = f.start().await;
        assert!(matches!(
            provider
                .prepare_transcoded(request(reference, "different-version".into(), 0.0))
                .await,
            Err(ProviderError::UnsupportedMedia)
        ));
        assert!(
            !f.requests
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.contains("transcode"))
        );
        server.abort();
    }
    #[tokio::test]
    async fn dropping_an_abandoned_body_stops_the_session_without_waiting_for_source_close() {
        let f = Fixture::default();
        let (provider, reference, server) = f.start().await;
        let media = provider
            .prepare_transcoded(request(reference, "7:8".into(), 0.0))
            .await
            .unwrap();
        let response = media.stream.open(None, false).await.unwrap();
        drop(response);
        let until = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if f.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.contains("universal/stop"))
                .count()
                >= 2
            {
                break;
            }
            assert!(tokio::time::Instant::now() < until);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        media.stream.close().await.unwrap();
        server.abort();
    }
    #[tokio::test]
    async fn pinned_fallback_prefers_verified_lossless_output_and_closes_its_session() {
        let f = Fixture::default();
        let (provider, reference, server) = f.start().await;
        let media = provider
            .prepare_transcoded(request(reference.clone(), "7:8".into(), 0.0))
            .await
            .unwrap();
        assert_eq!(media.version, "7:8");
        assert_eq!(media.track.reference, reference);
        assert!(media.stream.transcoded());
        let mut response = media.stream.open(None, false).await.unwrap();
        assert_eq!(
            response.body.next().await.unwrap().unwrap().as_ref(),
            b"fLaCfixture"
        );
        media.stream.close().await.unwrap();
        let requests = f.requests.lock().unwrap().clone();
        assert!(
            requests
                .iter()
                .any(|r| r.contains("start.flac") && r.contains("directPlay=0"))
        );
        assert!(requests.iter().any(|r| r.contains("universal/stop")));
        assert!(!requests.iter().any(|r| r.contains("start.mp3")));
        assert!(media.stream.open(None, false).await.is_err());
        server.abort();
    }
}
