//! Revocable lazy artwork URLs. At most two upstream reads and 16 MiB of JPEGs.
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
    routing::get,
};
use bytes::Bytes;
use qoda_provider::{AlbumRef, MusicProvider};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore, watch};

const ENTRIES: usize = 64;
const IMAGE_BYTES: usize = 256 * 1024;
#[derive(Clone)]
struct Grant {
    reference: AlbumRef,
    provider: Arc<dyn MusicProvider>,
    cancel: watch::Sender<bool>,
}
struct Cached {
    reference: AlbumRef,
    image: Option<Bytes>,
    until: Instant,
}
#[derive(Default)]
struct Data {
    grants: HashMap<String, Grant>,
    cache: VecDeque<Cached>,
}
impl Data {
    fn get(&mut self, reference: &AlbumRef) -> Option<Option<Bytes>> {
        self.cache.retain(|c| c.until > Instant::now());
        let index = self.cache.iter().position(|c| &c.reference == reference)?;
        let entry = self.cache.remove(index)?;
        let result = entry.image.clone();
        self.cache.push_back(entry);
        Some(result)
    }
    fn put(&mut self, reference: AlbumRef, image: Option<Bytes>) {
        self.cache
            .retain(|c| c.reference != reference && c.until > Instant::now());
        let ttl = if image.is_some() { 300 } else { 30 };
        self.cache.push_back(Cached {
            reference,
            image,
            until: Instant::now() + Duration::from_secs(ttl),
        });
        while self.cache.len() > ENTRIES {
            self.cache.pop_front();
        }
    }
}
#[derive(Clone)]
pub struct Artwork {
    data: Arc<Mutex<Data>>,
    active: Arc<Semaphore>,
    pending: Arc<Semaphore>,
    port: u16,
}
impl Artwork {
    pub async fn start() -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let this = Self {
            data: Default::default(),
            active: Arc::new(Semaphore::new(2)),
            pending: Arc::new(Semaphore::new(32)),
            port: listener.local_addr()?.port(),
        };
        let app = Router::new()
            .route("/art/{id}", get(image))
            .with_state(this.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(this)
    }
    pub async fn revoke(&self, clear_cache: bool) {
        let mut data = self.data.lock().await;
        for grant in data.grants.values() {
            grant.cancel.send_replace(true);
        }
        data.grants.clear();
        if clear_cache {
            data.cache.clear();
        }
    }
    pub async fn publish(
        &self,
        provider: Arc<dyn MusicProvider>,
        references: impl IntoIterator<Item = AlbumRef>,
    ) -> HashMap<String, String> {
        self.revoke(false).await;
        let mut data = self.data.lock().await;
        let mut urls = HashMap::new();
        for reference in references.into_iter().take(100) {
            let id = uuid::Uuid::new_v4().to_string();
            let (cancel, _) = watch::channel(false);
            urls.insert(
                reference.key.clone(),
                format!("http://127.0.0.1:{}/art/{id}", self.port),
            );
            data.grants.insert(
                id,
                Grant {
                    reference,
                    provider: provider.clone(),
                    cancel,
                },
            );
        }
        urls
    }
}
async fn image(
    State(art): State<Artwork>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response<Body>, StatusCode> {
    if headers.get("host").and_then(|v| v.to_str().ok())
        != Some(format!("127.0.0.1:{}", art.port).as_str())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let _pending = art
        .pending
        .try_acquire()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let grant = art
        .data
        .lock()
        .await
        .grants
        .get(&id)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    let mut cancel = grant.cancel.subscribe();
    if *cancel.borrow() {
        return Err(StatusCode::NOT_FOUND);
    }
    let fetch_cancel = cancel.clone();
    let fetch = async {
        if let Some(image) = art.data.lock().await.get(&grant.reference) {
            return Ok(image);
        }
        let _active = art
            .active
            .acquire()
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if *fetch_cancel.borrow() {
            return Err(StatusCode::NOT_FOUND);
        }
        if let Some(image) = art.data.lock().await.get(&grant.reference) {
            return Ok(image);
        }
        let image = grant
            .provider
            .cover(grant.reference.clone())
            .await
            .ok()
            .filter(|v| !v.is_empty() && v.len() <= IMAGE_BYTES)
            .map(Bytes::from);
        let mut data = art.data.lock().await;
        if *fetch_cancel.borrow() {
            return Err(StatusCode::NOT_FOUND);
        }
        data.put(grant.reference.clone(), image.clone());
        Ok(image)
    };
    let result = tokio::select! {
        r=tokio::time::timeout(Duration::from_secs(25),fetch)=>r.map_err(|_|StatusCode::GATEWAY_TIMEOUT)??,
        _=cancel.changed()=>return Err(StatusCode::NOT_FOUND),
    };
    let bytes = result.ok_or(StatusCode::NOT_FOUND)?;
    Response::builder()
        .header("content-type", "image/jpeg")
        .header("cache-control", "no-store")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qoda_core::{ProviderTrackKey, SourceId};
    use qoda_provider::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Default)]
    struct Fixture {
        calls: AtomicUsize,
        active: AtomicUsize,
        maximum: AtomicUsize,
        started: tokio::sync::Notify,
        mode: u8,
    }
    impl MusicProvider for Fixture {
        fn libraries(&self) -> Vec<Library> {
            vec![]
        }
        fn tracks(&self, _: SourceId, _: u64, _: String) -> ProviderFuture<'_, CatalogPage> {
            Box::pin(async { Err(ProviderError::Unsupported) })
        }
        fn prepare(&self, _: SourceId, _: ProviderTrackKey) -> ProviderFuture<'_, PreparedMedia> {
            Box::pin(async { Err(ProviderError::Unsupported) })
        }
        fn cover(&self, _: AlbumRef) -> ProviderFuture<'_, Vec<u8>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum.fetch_max(active, Ordering::SeqCst);
                self.started.notify_one();
                if self.mode == 1 {
                    return std::future::pending().await;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
                self.active.fetch_sub(1, Ordering::SeqCst);
                match self.mode {
                    2 => Err(ProviderError::AccessDenied),
                    3 => Ok(vec![0; IMAGE_BYTES + 1]),
                    _ => {
                        Ok(include_bytes!("../../qoda-provider/tests/fixtures/cover.jpg").to_vec())
                    }
                }
            })
        }
    }
    fn reference(key: usize) -> AlbumRef {
        AlbumRef {
            source: SourceId::new(),
            key: key.to_string(),
        }
    }
    #[tokio::test]
    async fn lazy_bounded_urls_cache_hits_and_host_and_revocation_guards() {
        let art = Artwork::start().await.unwrap();
        let provider = Arc::new(Fixture::default());
        let reference = reference(1);
        let url = art.publish(provider.clone(), [reference.clone()]).await["1"].clone();
        let http = reqwest::Client::new();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            http.get(&url)
                .header("host", "foreign.invalid")
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
        for _ in 0..2 {
            let response = http.get(&url).send().await.unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["content-type"], "image/jpeg");
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert!(!response.bytes().await.unwrap().is_empty());
        }
        let next = art.publish(provider.clone(), [reference.clone()]).await["1"].clone();
        assert_eq!(http.get(&url).send().await.unwrap().status(), 404);
        assert_eq!(http.get(&next).send().await.unwrap().status(), 200);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        art.revoke(true).await;
        assert!(art.data.lock().await.cache.is_empty());
        assert_eq!(http.get(&next).send().await.unwrap().status(), 404);
        let fresh = art.publish(provider.clone(), [reference]).await["1"].clone();
        assert_eq!(http.get(fresh).send().await.unwrap().status(), 200);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn at_most_two_upstream_images_are_loaded_at_a_time() {
        let art = Artwork::start().await.unwrap();
        let provider = Arc::new(Fixture::default());
        let urls = art.publish(provider.clone(), (0..10).map(reference)).await;
        let tasks = urls
            .into_values()
            .map(|url| tokio::spawn(async move { reqwest::get(url).await.unwrap().status() }))
            .collect::<Vec<_>>();
        for task in tasks {
            assert_eq!(task.await.unwrap(), 200);
        }
        assert_eq!(provider.maximum.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn cancellation_interrupts_pending_reads_without_repopulating_the_cache() {
        let art = Artwork::start().await.unwrap();
        let provider = Arc::new(Fixture {
            mode: 1,
            ..Default::default()
        });
        let url = art.publish(provider.clone(), [reference(1)]).await["1"].clone();
        let task = tokio::spawn(async move { reqwest::get(url).await.unwrap().status() });
        tokio::time::timeout(Duration::from_secs(2), provider.started.notified())
            .await
            .unwrap();
        art.revoke(true).await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap(),
            404
        );
        assert!(art.data.lock().await.cache.is_empty());
    }
    #[tokio::test]
    async fn failed_and_oversized_images_are_negative_cached_without_retaining_bytes() {
        for mode in [2, 3] {
            let art = Artwork::start().await.unwrap();
            let provider = Arc::new(Fixture {
                mode,
                ..Default::default()
            });
            let url = art.publish(provider.clone(), [reference(1)]).await["1"].clone();
            for _ in 0..2 {
                assert_eq!(reqwest::get(&url).await.unwrap().status(), 404);
            }
            assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
            let data = art.data.lock().await;
            assert!(data.cache[0].image.is_none());
            assert!(data.cache[0].until <= Instant::now() + Duration::from_secs(30));
        }
    }
    #[test]
    fn cache_is_lru_and_expiring_and_never_exceeds_its_entry_budget() {
        let mut data = Data::default();
        let first = reference(0);
        data.put(first.clone(), Some(Bytes::from_static(b"jpeg")));
        for i in 1..ENTRIES {
            data.put(reference(i), Some(Bytes::from_static(b"jpeg")));
        }
        assert!(data.get(&first).is_some());
        data.put(reference(ENTRIES), Some(Bytes::from_static(b"jpeg")));
        assert_eq!(data.cache.len(), ENTRIES);
        assert!(data.get(&first).is_some());
        data.cache
            .iter_mut()
            .for_each(|entry| entry.until = Instant::now() - Duration::from_secs(1));
        assert!(data.get(&first).is_none());
        assert!(data.cache.is_empty());
    }
}
