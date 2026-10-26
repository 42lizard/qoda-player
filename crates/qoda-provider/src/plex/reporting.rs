//! Current documented POST contract; live dispatch remains gated by #16.
use super::*;
use playback::{Outcome, Report, State};
use reqwest::Response;

pub(super) const LIVE_QUALIFIED: bool = true; // VALIDATION BRANCH ONLY
const LIMIT: usize = 64 * 1024;
const CAPABILITY_TTL: Duration = Duration::from_secs(300);
#[derive(Clone)]
pub(super) struct Capability {
    endpoint: Url,
    metadata: Url,
    validated: std::time::Instant,
}

async fn small_body(response: Response) -> Result<String, ProviderError> {
    use futures_util::StreamExt;
    if response.status().as_u16() != 200 {
        checked(response).await?;
        return Err(ProviderError::InvalidReply);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ProviderError::Network)?;
        if bytes.len().saturating_add(chunk.len()) > LIMIT {
            return Err(ProviderError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| ProviderError::InvalidReply)
}

fn feature(base: &Url, provider: Node<'_, '_>, name: &str) -> Result<Url, ProviderError> {
    let mut nodes = provider
        .children()
        .filter(|node| node.has_tag_name("Feature") && node.attribute("type") == Some(name));
    let node = nodes.next().ok_or(ProviderError::Unsupported)?;
    if nodes.next().is_some() {
        return Err(ProviderError::InvalidReply);
    }
    let url = relative(
        base,
        node.attribute("key").ok_or(ProviderError::InvalidReply)?,
    )?;
    if url.query().is_some() || url.path() == "/" {
        return Err(ProviderError::InvalidReply);
    }
    Ok(url)
}

fn capabilities(base: &Url, body: &str) -> Result<(Url, Url), ProviderError> {
    let doc = Document::parse(body).map_err(|_| ProviderError::InvalidReply)?;
    let root = doc.root_element();
    if !root.has_tag_name("MediaContainer") {
        return Err(ProviderError::InvalidReply);
    }
    let mut providers = root.children().filter(|node| {
        node.has_tag_name("MediaProvider")
            && node.attribute("identifier") == Some("com.plexapp.plugins.library")
    });
    let provider = providers.next().ok_or(ProviderError::Unsupported)?;
    if providers.next().is_some() {
        return Err(ProviderError::InvalidReply);
    }
    Ok((
        feature(base, provider, "timeline")?,
        feature(base, provider, "metadata")?,
    ))
}

impl PlexProvider {
    async fn timeline_capability(
        &self,
        id: SourceId,
        source: &Source,
    ) -> Result<Capability, ProviderError> {
        let cached = {
            let cache = self
                .timeline_capabilities
                .lock()
                .map_err(|_| ProviderError::Network)?;
            cache
                .get(&id)
                .filter(|capability| capability.validated.elapsed() < CAPABILITY_TTL)
                .cloned()
        };
        if let Some(cached) = cached {
            eprintln!(
                "QODA_VALIDATION_DEBUG {}",
                serde_json::json!({"event":"capability_cache_hit"})
            );
            return Ok(cached);
        }
        let discovery = self
            .http
            .get(path(&source.base, &["media", "providers"]))
            .headers(source.headers.clone())
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map_err(|_| ProviderError::Network)?;
        let (endpoint, metadata) = capabilities(&source.base, &small_body(discovery).await?)?;
        let capability = Capability {
            endpoint,
            metadata,
            validated: std::time::Instant::now(),
        };
        self.timeline_capabilities
            .lock()
            .map_err(|_| ProviderError::Network)?
            .insert(id, capability.clone());
        Ok(capability)
    }
    pub(super) async fn timeline(&self, report: Report) -> Result<Outcome, ProviderError> {
        let source = self
            .sources
            .get(&report.track.source)
            .ok_or(ProviderError::UnknownSource)?;
        let rating_key = String::from(report.track.key);
        if rating_key.is_empty()
            || rating_key.len() > 32
            || !rating_key.bytes().all(|b| b.is_ascii_digit())
            || report.session.is_nil()
            || report.position_ms > report.duration_ms.get()
            || report.continuing && report.state != State::Stopped
        {
            return Err(ProviderError::InvalidReply);
        }
        // Account/provider-scoped discovery avoids its round trip for every
        // transition. The worker still bounds a cache miss plus POST to 3 seconds.
        let Capability {
            endpoint,
            mut metadata,
            ..
        } = self
            .timeline_capability(report.track.source, source)
            .await?;
        metadata
            .path_segments_mut()
            .map_err(|_| ProviderError::InvalidReply)?
            .pop_if_empty()
            .push(&rating_key);
        let mut h = source.headers.clone();
        h.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let mut session = HeaderValue::from_str(&report.session.to_string())
            .map_err(|_| ProviderError::InvalidReply)?;
        session.set_sensitive(true);
        h.insert("X-Plex-Session-Identifier", session);
        let state = match report.state {
            State::Playing => "playing",
            State::Paused => "paused",
            State::Stopped => "stopped",
        };
        let mut query = vec![
            ("key", metadata.path().to_owned()),
            ("ratingKey", rating_key),
            ("state", state.to_owned()),
            ("time", report.position_ms.to_string()),
            ("duration", report.duration_ms.get().to_string()),
        ];
        if report.continuing {
            query.push(("continuing", "1".into()));
        }
        eprintln!(
            "QODA_VALIDATION_TIMELINE {}",
            serde_json::json!({"state":state,"position_ms":report.position_ms,"duration_ms":report.duration_ms.get(),"continuing":report.continuing,"method":"POST"})
        );
        let response = self
            .http
            .post(endpoint)
            .headers(h)
            .query(&query)
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map_err(|_| ProviderError::Network)?;
        if matches!(response.status().as_u16(), 404 | 410) {
            self.timeline_capabilities
                .lock()
                .map_err(|_| ProviderError::Network)?
                .remove(&report.track.source);
        }
        let body = small_body(response).await?;
        let value: serde_json::Value =
            serde_json::from_str(&body).map_err(|_| ProviderError::InvalidReply)?;
        let container = value
            .get("MediaContainer")
            .and_then(serde_json::Value::as_object)
            .ok_or(ProviderError::InvalidReply)?;
        eprintln!(
            "QODA_VALIDATION_RESULT {}",
            serde_json::json!({"http_status":200,"termination_present":container.contains_key("terminationCode") || container.contains_key("terminationText")})
        );
        Ok(
            if container
                .get("terminationCode")
                .is_some_and(|v| !v.is_null())
                || container
                    .get("terminationText")
                    .is_some_and(|v| !v.is_null())
            {
                Outcome::ServerTermination
            } else {
                Outcome::Accepted
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        extract::{Request, State as AxumState},
        routing::any,
    };
    use std::{
        num::NonZeroU64,
        sync::{Arc, Mutex},
    };
    const DISCOVERY: &str = "<MediaContainer><MediaProvider identifier='other'><Feature type='timeline' key='/wrong'/></MediaProvider><MediaProvider identifier='com.plexapp.plugins.library'><Feature type='timeline' key='/custom/timeline'/><Feature type='metadata' key='/custom/metadata'/></MediaProvider></MediaContainer>";
    #[derive(Clone)]
    struct Fixture {
        requests: Arc<Mutex<Vec<(String, String, HeaderMap)>>>,
        reply: &'static str,
        status: Arc<std::sync::atomic::AtomicU16>,
    }
    async fn receive(
        AxumState(f): AxumState<Fixture>,
        request: Request,
    ) -> (axum::http::StatusCode, String) {
        f.requests.lock().unwrap().push((
            request.method().to_string(),
            request.uri().to_string(),
            request.headers().clone(),
        ));
        if request.uri().path() == "/media/providers" {
            (axum::http::StatusCode::OK, DISCOVERY.into())
        } else {
            (
                axum::http::StatusCode::from_u16(
                    f.status.load(std::sync::atomic::Ordering::SeqCst),
                )
                .unwrap(),
                f.reply.into(),
            )
        }
    }
    async fn fixture(
        reply: &'static str,
    ) -> (PlexProvider, SourceId, Fixture, tokio::task::JoinHandle<()>) {
        let f = Fixture {
            requests: Arc::default(),
            reply,
            status: Arc::new(std::sync::atomic::AtomicU16::new(200)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let router = Router::new().fallback(any(receive)).with_state(f.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let id = SourceId::new();
        let source = Source {
            server_id: "fixture".into(),
            view: Library {
                id,
                title: "fixture".into(),
                server: "fixture".into(),
            },
            base,
            headers: headers("fake-client", Some("fake-token")).unwrap(),
            key: "1".into(),
        };
        let provider = PlexProvider {
            timeline_capabilities: Default::default(),
            account_scope: "fixture".into(),
            unavailable_sources: HashSet::new(),
            unavailable_servers: 0,
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            sources: HashMap::from([(id, source)]),
        };
        (provider, id, f, task)
    }
    fn report(source: SourceId) -> Report {
        Report {
            track: TrackRef {
                source,
                key: "42".to_owned().try_into().unwrap(),
            },
            session: uuid::Uuid::new_v4(),
            state: State::Playing,
            position_ms: 1250,
            duration_ms: NonZeroU64::new(10000).unwrap(),
            continuing: false,
        }
    }
    #[tokio::test]
    async fn scoped_post_uses_discovered_paths_and_headers_without_server_queue_or_scrobble() {
        let (p, id, f, task) = fixture(r#"{"MediaContainer":{}}"#).await;
        let mut r = report(id);
        let session = r.session;
        for state in [State::Playing, State::Paused, State::Stopped] {
            r.state = state;
            r.continuing = state == State::Stopped;
            assert_eq!(p.timeline(r.clone()).await.unwrap(), Outcome::Accepted);
        }
        let requests = f.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        for (index, (method, uri, h)) in requests.iter().enumerate() {
            assert_eq!(h["X-Plex-Token"], "fake-token");
            assert_eq!(h["X-Plex-Client-Identifier"], "fake-client");
            if index == 0 {
                assert_eq!(method, "GET");
                assert_eq!(uri, "/media/providers");
                continue;
            }
            assert_eq!(method, "POST");
            assert_eq!(h["X-Plex-Session-Identifier"], session.to_string());
            let url = Url::parse(&format!("http://fixture{uri}")).unwrap();
            assert_eq!(url.path(), "/custom/timeline");
            let query: HashMap<_, _> = url.query_pairs().collect();
            assert_eq!(query["key"], "/custom/metadata/42");
            assert_eq!(query["ratingKey"], "42");
            assert_eq!(query["time"], "1250");
            assert_eq!(query["duration"], "10000");
            assert_eq!(query["state"], ["playing", "paused", "stopped"][index - 1]);
            assert!(!query.contains_key("playQueueItemID"));
            assert!(!uri.contains("fake-token"));
            assert_eq!(query.contains_key("continuing"), index == 3);
        }
        task.abort();
    }
    #[tokio::test]
    async fn public_live_gate_never_contacts_even_an_available_fixture() {
        let (p, id, f, task) = fixture(r#"{"MediaContainer":{}}"#).await;
        assert_eq!(
            p.report_playback(report(id)).await,
            Err(ProviderError::Unsupported)
        );
        assert!(f.requests.lock().unwrap().is_empty());
        task.abort();
    }
    #[tokio::test]
    async fn expired_capabilities_are_rediscovered_without_sliding_freshness() {
        let (p, id, f, task) = fixture(r#"{"MediaContainer":{}}"#).await;
        p.timeline(report(id)).await.unwrap();
        let validated = p.timeline_capabilities.lock().unwrap()[&id].validated;
        p.timeline(report(id)).await.unwrap();
        assert_eq!(
            p.timeline_capabilities.lock().unwrap()[&id].validated,
            validated
        );
        assert_eq!(f.requests.lock().unwrap().len(), 3);
        p.timeline_capabilities
            .lock()
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .validated -= CAPABILITY_TTL;
        p.timeline(report(id)).await.unwrap();
        let requests = f.requests.lock().unwrap();
        assert_eq!(requests.len(), 5);
        assert_eq!(
            requests
                .iter()
                .filter(|(method, _, _)| method == "GET")
                .count(),
            2
        );
        task.abort();
    }
    #[tokio::test]
    async fn source_and_provider_boundaries_do_not_reuse_other_discovery() {
        let (mut p, id, f, task) = fixture(r#"{"MediaContainer":{}}"#).await;
        let second = SourceId::new();
        let mut source = p.sources[&id].clone();
        source.view.id = second;
        p.sources.insert(second, source);
        p.timeline(report(id)).await.unwrap();
        p.timeline(report(second)).await.unwrap();
        assert_eq!(f.requests.lock().unwrap().len(), 4);
        let other = PlexProvider {
            timeline_capabilities: Default::default(),
            account_scope: "different-account".into(),
            http: p.http.clone(),
            sources: p.sources.clone(),
            unavailable_sources: HashSet::new(),
            unavailable_servers: 0,
        };
        other.timeline(report(id)).await.unwrap();
        let requests = f.requests.lock().unwrap();
        assert_eq!(requests.len(), 6);
        assert_eq!(
            requests
                .iter()
                .filter(|(method, _, _)| method == "GET")
                .count(),
            3
        );
        task.abort();
    }
    #[tokio::test]
    async fn retired_route_invalidates_discovery_without_retrying_the_failed_post() {
        for status in [404, 410] {
            let (p, id, f, task) = fixture(r#"{"MediaContainer":{}}"#).await;
            p.timeline(report(id)).await.unwrap();
            f.status.store(status, std::sync::atomic::Ordering::SeqCst);
            assert!(p.timeline(report(id)).await.is_err());
            assert!(p.timeline_capabilities.lock().unwrap().is_empty());
            assert_eq!(f.requests.lock().unwrap().len(), 3);
            f.status.store(200, std::sync::atomic::Ordering::SeqCst);
            p.timeline(report(id)).await.unwrap();
            assert_eq!(f.requests.lock().unwrap().len(), 5);
            task.abort();
        }
    }
    #[tokio::test]
    async fn response_and_identity_errors_are_bounded_and_sanitized() {
        for (reply, expected) in [
            (
                r#"{"MediaContainer":{"terminationCode":1000,"terminationText":"private"}}"#,
                Ok(Outcome::ServerTermination),
            ),
            ("private-invalid-response", Err(ProviderError::InvalidReply)),
        ] {
            let (p, id, _, task) = fixture(reply).await;
            assert_eq!(p.timeline(report(id)).await, expected);
            task.abort();
        }
        let (p, id, f, task) = fixture("{}").await;
        assert_eq!(
            p.timeline(report(SourceId::new())).await,
            Err(ProviderError::UnknownSource)
        );
        let mut r = report(id);
        r.position_ms = 10001;
        assert_eq!(p.timeline(r).await, Err(ProviderError::InvalidReply));
        let mut r = report(id);
        r.session = uuid::Uuid::nil();
        assert_eq!(p.timeline(r).await, Err(ProviderError::InvalidReply));
        let mut r = report(id);
        r.continuing = true;
        assert_eq!(p.timeline(r).await, Err(ProviderError::InvalidReply));
        let mut r = report(id);
        r.track.key = "42?secret=private".to_owned().try_into().unwrap();
        assert_eq!(p.timeline(r).await, Err(ProviderError::InvalidReply));
        assert!(f.requests.lock().unwrap().is_empty());
        task.abort();
    }
    #[test]
    fn missing_ambiguous_cross_origin_and_query_capabilities_fail_closed() {
        let base = Url::parse("https://fixture/").unwrap();
        assert!(capabilities(&base, DISCOVERY).is_ok());
        for body in ["<MediaContainer/>", "<html/>"] {
            assert!(capabilities(&base, body).is_err());
        }
        for key in [
            "//elsewhere/timeline",
            "https://elsewhere/timeline",
            "/timeline?X-Plex-Token=secret",
            "/timeline#fragment",
            "/",
        ] {
            assert!(capabilities(&base, &DISCOVERY.replace("/custom/timeline", key)).is_err());
        }
        let duplicate = DISCOVERY.replace(
            "<Feature type='timeline' key='/custom/timeline'/>",
            "<Feature type='timeline' key='/one'/><Feature type='timeline' key='/two'/>",
        );
        assert_eq!(
            capabilities(&base, &duplicate),
            Err(ProviderError::InvalidReply)
        );
    }
    #[tokio::test]
    async fn oversized_reply_cannot_allocate_an_unbounded_reporting_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(|| async { "x".repeat(LIMIT + 1) }),
            )
            .await
            .unwrap();
        });
        assert_eq!(
            small_body(Client::new().get(url).send().await.unwrap()).await,
            Err(ProviderError::TooLarge)
        );
        task.abort();
    }
    #[tokio::test]
    async fn unsuccessful_responses_never_expose_server_text_or_follow_redirects() {
        use axum::http::StatusCode;
        for (status, error) in [
            (302, ProviderError::Network),
            (401, ProviderError::SignInRequired),
            (403, ProviderError::AccessDenied),
            (500, ProviderError::Network),
            (204, ProviderError::InvalidReply),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let calls = count.clone();
            let task = tokio::spawn(async move {
                axum::serve(
                    listener,
                    Router::new().fallback(move || {
                        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        async move {
                            (
                                StatusCode::from_u16(status).unwrap(),
                                [("location", "/elsewhere")],
                                "private server text",
                            )
                        }
                    }),
                )
                .await
                .unwrap();
            });
            let http = Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            assert_eq!(
                small_body(http.get(url).send().await.unwrap()).await,
                Err(error)
            );
            assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert!(!error.to_string().contains("private"));
            task.abort();
        }
    }
}
