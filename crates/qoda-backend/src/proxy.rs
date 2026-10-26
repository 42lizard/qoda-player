//! Only unguessable, revocable media grants are visible to libmpv.
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
    routing::get,
};
use futures_util::StreamExt;
use qoda_provider::{ByteRange, MediaSource, ProviderError};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{RwLock, watch};
#[derive(Clone)]
struct Grant {
    source: Arc<dyn MediaSource>,
    cancel: watch::Sender<bool>,
    failed: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<ProviderError>>>,
}
impl Grant {
    fn fail(&self, error: ProviderError) {
        let mut failure = self.failure.lock().unwrap_or_else(|e| e.into_inner());
        // Concurrent range requests must not downgrade terminal auth/missing
        // failures to a later generic network failure.
        if failure.is_none() || *failure == Some(ProviderError::Network) {
            *failure = Some(error);
        }
        self.failed.store(true, Ordering::Release);
    }
}
#[derive(Default)]
struct Grants {
    entries: HashMap<String, Grant>,
    current: Option<String>,
}
#[derive(Clone)]
pub struct Proxy {
    grants: Arc<RwLock<Grants>>,
    pub port: u16,
}
impl Proxy {
    pub async fn start() -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let this = Self {
            grants: Default::default(),
            port: listener.local_addr()?.port(),
        };
        let app = Router::new()
            .route("/media/{id}", get(media))
            .with_state(this.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(this)
    }
    pub async fn failed(&self) -> bool {
        let grants = self.grants.read().await;
        grants
            .current
            .as_ref()
            .and_then(|id| grants.entries.get(id))
            .is_some_and(|grant| grant.failed.load(Ordering::Acquire))
    }
    pub async fn failure(&self) -> Option<ProviderError> {
        let grants = self.grants.read().await;
        let grant = grants
            .current
            .as_ref()
            .and_then(|id| grants.entries.get(id))?;
        *grant.failure.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub async fn revoke(&self) {
        let mut grants = self.grants.write().await;
        for grant in grants.entries.values() {
            grant.cancel.send_replace(true);
        }
        grants.entries.clear();
        grants.current = None;
    }
    fn id<'a>(&self, url: &'a str) -> Option<&'a str> {
        url.strip_prefix(&format!("http://127.0.0.1:{}/media/", self.port))
    }
    pub async fn revoke_url(&self, url: &str) {
        let Some(id) = self.id(url) else {
            return;
        };
        let mut grants = self.grants.write().await;
        if let Some(grant) = grants.entries.remove(id) {
            grant.cancel.send_replace(true);
        }
        if grants.current.as_deref() == Some(id) {
            grants.current = None;
        }
    }
    pub async fn promote(&self, url: &str) -> Result<(), &'static str> {
        let id = self.id(url).ok_or("Invalid media grant")?;
        let mut grants = self.grants.write().await;
        if !grants.entries.contains_key(id) {
            return Err("The prepared media grant expired");
        }
        if grants
            .current
            .as_ref()
            .and_then(|id| grants.entries.get(id))
            .is_some_and(|grant| grant.failed.load(Ordering::Acquire))
        {
            return Err("Current stream failed before handoff; its occurrence remains selected.");
        }
        grants.entries.retain(|key, grant| {
            if key == id {
                true
            } else {
                grant.cancel.send_replace(true);
                false
            }
        });
        grants.current = Some(id.to_owned());
        Ok(())
    }
    pub async fn grant(&self, source: Box<dyn MediaSource>) -> String {
        self.revoke().await;
        let url = self
            .grant_next(source)
            .await
            .expect("empty media grant set");
        self.promote(&url).await.expect("new media grant");
        url
    }
    pub async fn grant_next(&self, source: Box<dyn MediaSource>) -> Result<String, &'static str> {
        let mut grants = self.grants.write().await;
        if grants.entries.len() >= 2 {
            return Err("The current/next media grant limit was reached");
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (cancel, _) = watch::channel(false);
        grants.entries.insert(
            id.clone(),
            Grant {
                source: Arc::from(source),
                failed: Arc::new(AtomicBool::new(false)),
                failure: Arc::new(Mutex::new(None)),
                cancel,
            },
        );
        Ok(format!("http://127.0.0.1:{}/media/{id}", self.port))
    }
}
async fn media(
    State(proxy): State<Proxy>,
    Path(id): Path<String>,
    headers: HeaderMap,
    method: axum::http::Method,
) -> Result<Response<Body>, StatusCode> {
    if headers.get("host").and_then(|s| s.to_str().ok())
        != Some(format!("127.0.0.1:{}", proxy.port).as_str())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let grant = proxy
        .grants
        .read()
        .await
        .entries
        .get(&id)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    let mut cancel = grant.cancel.subscribe();
    if *cancel.borrow() {
        return Err(StatusCode::NOT_FOUND);
    }
    let range = headers
        .get("range")
        .map(|s| {
            s.to_str()
                .map_err(|_| StatusCode::BAD_REQUEST)
                .and_then(|s| ByteRange::try_from(s).map_err(|_| StatusCode::RANGE_NOT_SATISFIABLE))
        })
        .transpose()?;
    let response = tokio::select! {r=grant.source.open(range,method==axum::http::Method::HEAD)=>r.map_err(|error|{grant.fail(error);StatusCode::BAD_GATEWAY})?,_=cancel.changed()=>return Err(StatusCode::NOT_FOUND)};
    let mut builder = Response::builder().status(response.status);
    for (k, v) in [
        ("content-type", response.content_type),
        (
            "content-length",
            response.content_length.map(|n| n.to_string()),
        ),
        ("content-range", response.content_range),
        ("etag", response.etag),
    ] {
        if let Some(v) = v {
            builder = builder.header(k, v);
        }
    }
    let upstream = response
        .body
        .inspect(move |chunk| {
            if let Err(error) = chunk {
                grant.fail(*error);
            }
        })
        .boxed();
    let stream = futures_util::stream::unfold(
        (upstream, cancel),
        |(mut upstream, mut cancel)| async move {
            if *cancel.borrow() {
                return None;
            }
            tokio::select! {chunk=upstream.next()=>chunk.map(|c|(c,(upstream,cancel))),_=cancel.changed()=>None}
        },
    );
    builder
        .body(Body::from_stream(stream))
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qoda_provider::{MediaResponse, ProviderFuture};
    struct Fixture;
    impl MediaSource for Fixture {
        fn open(&self, range: Option<ByteRange>, head: bool) -> ProviderFuture<'_, MediaResponse> {
            Box::pin(async move {
                let partial = range.is_some();
                let bytes = if head {
                    bytes::Bytes::new()
                } else {
                    bytes::Bytes::from_static(b"test")
                };
                Ok(MediaResponse {
                    status: if partial { 206 } else { 200 },
                    content_type: Some("audio/flac".into()),
                    content_length: Some(4),
                    content_range: partial.then(|| "bytes 0-3/4".into()),
                    etag: Some("fixture-v1".into()),
                    body: futures_util::stream::once(async move { Ok(bytes) }).boxed(),
                })
            })
        }
    }
    struct Pending {
        started: Arc<tokio::sync::Notify>,
    }
    impl MediaSource for Pending {
        fn open(&self, _: Option<ByteRange>, _: bool) -> ProviderFuture<'_, MediaResponse> {
            Box::pin(async move {
                self.started.notify_one();
                std::future::pending().await
            })
        }
    }
    struct Broken;
    impl MediaSource for Broken {
        fn open(&self, _: Option<ByteRange>, _: bool) -> ProviderFuture<'_, MediaResponse> {
            Box::pin(async { Err(qoda_provider::ProviderError::Network) })
        }
    }
    struct BrokenBody;
    impl MediaSource for BrokenBody {
        fn open(&self, _: Option<ByteRange>, _: bool) -> ProviderFuture<'_, MediaResponse> {
            Box::pin(async {
                Ok(MediaResponse {
                    status: 200,
                    content_type: None,
                    content_length: None,
                    content_range: None,
                    etag: None,
                    body: futures_util::stream::once(async { Err(ProviderError::Network) }).boxed(),
                })
            })
        }
    }
    #[tokio::test]
    async fn body_failure_is_typed_and_concurrent_network_errors_do_not_mask_terminal_errors() {
        let proxy = Proxy::start().await.unwrap();
        let url = proxy.grant(Box::new(BrokenBody)).await;
        if let Ok(response) = reqwest::get(&url).await {
            let _ = response.bytes().await;
        }
        assert_eq!(proxy.failure().await, Some(ProviderError::Network));
        let grant = proxy
            .grants
            .read()
            .await
            .entries
            .values()
            .next()
            .unwrap()
            .clone();
        grant.fail(ProviderError::AccessDenied);
        grant.fail(ProviderError::Network);
        assert_eq!(proxy.failure().await, Some(ProviderError::AccessDenied));
        proxy.revoke().await;
        assert_eq!(proxy.failure().await, None);
    }
    #[tokio::test]
    async fn upstream_failure_is_visible_only_for_its_own_grant() {
        let proxy = Proxy::start().await.unwrap();
        let url = proxy.grant(Box::new(Broken)).await;
        assert_eq!(
            reqwest::Client::new()
                .get(url)
                .send()
                .await
                .unwrap()
                .status(),
            502
        );
        assert!(proxy.failed().await);
        proxy.grant(Box::new(Fixture)).await;
        assert!(!proxy.failed().await);
    }
    #[tokio::test]
    async fn revocation_cancels_an_in_flight_upstream_open() {
        let proxy = Proxy::start().await.unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let url = proxy
            .grant(Box::new(Pending {
                started: started.clone(),
            }))
            .await;
        let request =
            tokio::spawn(async move { reqwest::Client::new().get(url).send().await.unwrap() });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        proxy.revoke().await;
        let response = tokio::time::timeout(std::time::Duration::from_secs(2), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 404);
    }
    #[tokio::test]
    async fn grant_range_head_host_and_revocation() {
        let proxy = Proxy::start().await.unwrap();
        let url = proxy.grant(Box::new(Fixture)).await;
        let http = reqwest::Client::new();
        let r = http
            .get(&url)
            .header("Range", "bytes=0-3")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(r.headers()["content-range"], "bytes 0-3/4");
        assert_eq!(r.bytes().await.unwrap(), "test");
        assert_eq!(http.head(&url).send().await.unwrap().status(), 200);
        assert_eq!(
            http.get(&url)
                .header("Host", "other.example")
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
        assert_eq!(
            http.get(&url)
                .header("Range", "bytes=0-1,2-3")
                .send()
                .await
                .unwrap()
                .status(),
            416
        );
        let new_url = proxy.grant(Box::new(Fixture)).await;
        assert_ne!(url, new_url);
        assert_eq!(http.get(&url).send().await.unwrap().status(), 404);
        proxy.revoke().await;
        assert_eq!(http.get(new_url).send().await.unwrap().status(), 404);
    }
}

#[cfg(test)]
mod gapless_tests {
    use super::*;
    use qoda_provider::{MediaResponse, ProviderError, ProviderFuture};
    struct Source(bool);
    impl MediaSource for Source {
        fn open(&self, _: Option<ByteRange>, _: bool) -> ProviderFuture<'_, MediaResponse> {
            Box::pin(async move {
                if self.0 {
                    return Err(ProviderError::Network);
                }
                Ok(MediaResponse {
                    status: 200,
                    content_type: Some("audio/flac".into()),
                    content_length: Some(4),
                    content_range: None,
                    etag: None,
                    body: futures_util::stream::once(async {
                        Ok(bytes::Bytes::from_static(b"test"))
                    })
                    .boxed(),
                })
            })
        }
    }
    #[tokio::test]
    async fn current_and_next_are_bounded_and_future_failures_do_not_stop_current() {
        let proxy = Proxy::start().await.unwrap();
        let current = proxy.grant(Box::new(Source(false))).await;
        let next = proxy.grant_next(Box::new(Source(true))).await.unwrap();
        assert!(proxy.grant_next(Box::new(Source(false))).await.is_err());
        assert_eq!(reqwest::get(&next).await.unwrap().status(), 502);
        assert!(!proxy.failed().await);
        assert_eq!(reqwest::get(&current).await.unwrap().status(), 200);
        proxy.promote(&next).await.unwrap();
        assert!(proxy.failed().await);
        assert_eq!(reqwest::get(current).await.unwrap().status(), 404);
        proxy.revoke().await;
        assert_eq!(reqwest::get(next).await.unwrap().status(), 404);
    }
    #[tokio::test]
    async fn failed_current_grant_cannot_be_discarded_by_successor_promotion() {
        let proxy = Proxy::start().await.unwrap();
        let current = proxy.grant(Box::new(Source(true))).await;
        let next = proxy.grant_next(Box::new(Source(false))).await.unwrap();
        assert_eq!(reqwest::get(&current).await.unwrap().status(), 502);
        assert!(proxy.promote(&next).await.is_err());
        assert!(proxy.failed().await);
        assert_eq!(proxy.failure().await, Some(ProviderError::Network));
        assert_eq!(reqwest::get(current).await.unwrap().status(), 502);
        proxy.revoke().await;
    }
    #[tokio::test]
    async fn cancelling_only_future_keeps_current_access_and_allows_one_replacement() {
        let proxy = Proxy::start().await.unwrap();
        let current = proxy.grant(Box::new(Source(false))).await;
        let future = proxy.grant_next(Box::new(Source(false))).await.unwrap();
        proxy.revoke_url(&future).await;
        assert_eq!(reqwest::get(future).await.unwrap().status(), 404);
        assert_eq!(reqwest::get(current).await.unwrap().status(), 200);
        assert!(proxy.grant_next(Box::new(Source(false))).await.is_ok());
        assert!(!proxy.failed().await);
    }
}
