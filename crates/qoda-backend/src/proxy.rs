//! Only unguessable, revocable media grants are visible to libmpv.
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
    routing::get,
};
use futures_util::StreamExt;
use qoda_provider::{ByteRange, MediaSource};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{RwLock, watch};
#[derive(Clone)]
struct Grant {
    source: Arc<dyn MediaSource>,
    cancel: watch::Sender<bool>,
}
#[derive(Clone)]
pub struct Proxy {
    grants: Arc<RwLock<HashMap<String, Grant>>>,
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
    pub async fn revoke(&self) {
        let mut g = self.grants.write().await;
        for grant in g.values() {
            grant.cancel.send_replace(true);
        }
        g.clear();
    }
    pub async fn grant(&self, source: Box<dyn MediaSource>) -> String {
        self.revoke().await;
        let id = uuid::Uuid::new_v4().to_string();
        let (cancel, _) = watch::channel(false);
        self.grants.write().await.insert(
            id.clone(),
            Grant {
                source: Arc::from(source),
                cancel,
            },
        );
        format!("http://127.0.0.1:{}/media/{id}", self.port)
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
    let response = tokio::select! {r=grant.source.open(range,method==axum::http::Method::HEAD)=>r.map_err(|_|StatusCode::BAD_GATEWAY)?,_=cancel.changed()=>return Err(StatusCode::NOT_FOUND)};
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
    let upstream = response.body;
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
