//! Revocable lazy artwork URLs with bounded memory/disk caches and background refresh.
use crate::artwork_store::{Disk, Loaded};
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Response, StatusCode},
    routing::get,
};
use bytes::Bytes;
use qoda_provider::{AlbumRef, MusicProvider, ProviderError};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore, mpsc, watch};
const ENTRIES: usize = 64;
const IMAGE_BYTES: usize = 256 * 1024;
#[derive(Clone)]
struct Grant {
    reference: AlbumRef,
    provider: Arc<dyn MusicProvider>,
    cancel: watch::Sender<bool>,
    epoch: u64,
}
#[derive(Clone)]
struct Hit {
    image: Option<Bytes>,
    stale: bool,
}
struct Cached {
    reference: AlbumRef,
    image: Option<Bytes>,
    until: Instant,
    stale: bool,
}
#[derive(Default)]
struct Data {
    grants: HashMap<String, Grant>,
    cache: VecDeque<Cached>,
    stale: HashSet<AlbumRef>,
    refreshing: HashMap<AlbumRef, String>,
    retry: HashMap<AlbumRef, Instant>,
}
impl Data {
    fn get(&mut self, reference: &AlbumRef) -> Option<Hit> {
        self.cache.retain(|c| c.until > Instant::now());
        let index = self.cache.iter().position(|c| &c.reference == reference)?;
        let entry = self.cache.remove(index)?;
        let result = Hit {
            image: entry.image.clone(),
            stale: entry.stale,
        };
        self.cache.push_back(entry);
        Some(result)
    }
    fn put(&mut self, reference: AlbumRef, image: Option<Bytes>) {
        let ttl = if image.is_some() {
            7 * 24 * 60 * 60
        } else {
            30
        };
        self.cache(reference, image, Duration::from_secs(ttl), false);
    }
    fn cache(&mut self, reference: AlbumRef, image: Option<Bytes>, ttl: Duration, stale: bool) {
        self.cache
            .retain(|c| c.reference != reference && c.until > Instant::now());
        self.cache.push_back(Cached {
            reference,
            image,
            until: Instant::now() + ttl,
            stale,
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
    pub disk: Disk,
    events: Option<mpsc::Sender<crate::Event>>,
}
impl Artwork {
    #[cfg(test)]
    pub async fn start() -> std::io::Result<Self> {
        Self::with_disk(Disk::disabled(), None).await
    }
    pub async fn persistent(events: mpsc::Sender<crate::Event>) -> std::io::Result<Self> {
        Self::with_disk(
            Disk::start(crate::artwork_store::Paths::discover()).await,
            Some(events),
        )
        .await
    }
    async fn with_disk(
        disk: Disk,
        events: Option<mpsc::Sender<crate::Event>>,
    ) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let this = Self {
            data: Default::default(),
            active: Arc::new(Semaphore::new(2)),
            pending: Arc::new(Semaphore::new(32)),
            port: listener.local_addr()?.port(),
            disk,
            events,
        };
        let app = Router::new()
            .route("/art/{id}", get(image))
            .with_state(this.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(this)
    }
    async fn revoke_memory(&self, clear_cache: bool) {
        let mut data = self.data.lock().await;
        for grant in data.grants.values() {
            grant.cancel.send_replace(true);
        }
        data.grants.clear();
        data.stale.clear();
        data.refreshing.clear();
        data.retry.clear();
        if clear_cache {
            data.cache.clear();
        }
    }
    pub async fn revoke(&self, clear_cache: bool) {
        self.revoke_memory(clear_cache).await;
        if clear_cache {
            self.disk.clear().await;
        }
    }
    pub async fn begin_clear(&self) -> Option<u64> {
        self.revoke_memory(true).await;
        self.disk.begin_clear()
    }
    pub async fn publish(
        &self,
        provider: Arc<dyn MusicProvider>,
        references: impl IntoIterator<Item = AlbumRef>,
    ) -> HashMap<String, String> {
        self.revoke(false).await;
        let mut data = self.data.lock().await;
        for reference in references.into_iter().take(100) {
            let id = uuid::Uuid::new_v4().to_string();
            let (cancel, _) = watch::channel(false);
            data.grants.insert(
                id,
                Grant {
                    reference,
                    provider: provider.clone(),
                    cancel,
                    epoch: self.disk.epoch(),
                },
            );
        }
        self.urls_locked(&data)
    }
    fn urls_locked(&self, data: &Data) -> HashMap<String, String> {
        data.grants
            .iter()
            .map(|(id, grant)| {
                (
                    grant.reference.key.clone(),
                    format!("http://127.0.0.1:{}/art/{id}", self.port),
                )
            })
            .collect()
    }
    pub async fn urls(&self) -> HashMap<String, String> {
        let data = self.data.lock().await;
        self.urls_locked(&data)
    }
    pub async fn view(&self) -> serde_json::Value {
        let mut view = serde_json::to_value(self.disk.view()).expect("cache view");
        let data = self.data.lock().await;
        view["refreshing"] = serde_json::json!(data.refreshing.len());
        view["stale"] = serde_json::json!(!data.stale.is_empty());
        view
    }
    fn notify(&self) {
        if let Some(tx) = &self.events {
            let _ = tx.try_send(crate::Event::Artwork);
        }
    }
    async fn cached(&self, grant: &Grant) -> Option<Hit> {
        if let Some(hit) = self.data.lock().await.get(&grant.reference) {
            return Some(hit);
        }
        let Loaded { image, fresh_for } = self.disk.get(grant.epoch, &grant.reference).await?;
        let mut data = self.data.lock().await;
        if *grant.cancel.borrow() {
            return None;
        }
        let stale = fresh_for.is_zero();
        let hit = Hit {
            image: Some(image.clone()),
            stale,
        };
        data.cache(
            grant.reference.clone(),
            Some(image),
            if stale {
                Duration::from_secs(30)
            } else {
                fresh_for
            },
            stale,
        );
        Some(hit)
    }
    async fn refresh(&self, id: &str, grant: &Grant) {
        let mut data = self.data.lock().await;
        if !data.grants.contains_key(id) {
            return;
        }
        data.stale.insert(grant.reference.clone());
        if data.refreshing.contains_key(&grant.reference)
            || data
                .retry
                .get(&grant.reference)
                .is_some_and(|until| *until > Instant::now())
        {
            return;
        }
        data.refreshing.insert(grant.reference.clone(), id.into());
        data.retry.insert(
            grant.reference.clone(),
            Instant::now() + Duration::from_secs(30),
        );
        drop(data);
        let this = self.clone();
        let grant = grant.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let mut cancel = grant.cancel.subscribe();
            let work = async {
                let _permit = this
                    .active
                    .acquire()
                    .await
                    .map_err(|_| ProviderError::Network)?;
                grant.provider.cover(grant.reference.clone()).await
            };
            let result = tokio::select! {result=tokio::time::timeout(Duration::from_secs(25),work)=>result.unwrap_or(Err(ProviderError::Network)),_=cancel.changed()=>Err(ProviderError::Network)};
            if !*cancel.borrow() {
                match result {
                    Ok(bytes) if qoda_provider::artwork::bounded_jpeg(&bytes) => {
                        let image = Bytes::from(bytes);
                        this.disk
                            .put(grant.epoch, &grant.reference, image.clone())
                            .await;
                        this.replace(&id, &grant, Some(image)).await;
                    }
                    Err(ProviderError::AccessDenied) => {
                        this.disk.remove(grant.epoch, &grant.reference).await;
                        this.replace(&id, &grant, None).await;
                    }
                    _ => {}
                }
            }
            let mut data = this.data.lock().await;
            if data.refreshing.get(&grant.reference) == Some(&id) {
                data.refreshing.remove(&grant.reference);
            }
            drop(data);
            this.notify();
        });
        self.notify();
    }
    async fn replace(&self, id: &str, grant: &Grant, image: Option<Bytes>) {
        let mut data = self.data.lock().await;
        if *grant.cancel.borrow() || !data.grants.contains_key(id) {
            return;
        }
        data.put(grant.reference.clone(), image);
        data.stale.remove(&grant.reference);
        data.grants.remove(id);
        grant.cancel.send_replace(true);
        let (cancel, _) = watch::channel(false);
        data.grants.insert(
            uuid::Uuid::new_v4().to_string(),
            Grant {
                cancel,
                ..grant.clone()
            },
        );
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
        if let Some(hit) = art.cached(&grant).await {
            if hit.stale {
                art.refresh(&id, &grant).await;
            }
            return Ok(hit.image);
        }
        let _active = art
            .active
            .acquire()
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if *fetch_cancel.borrow() {
            return Err(StatusCode::NOT_FOUND);
        }
        if let Some(hit) = art.data.lock().await.get(&grant.reference) {
            return Ok(hit.image);
        }
        let image = grant
            .provider
            .cover(grant.reference.clone())
            .await
            .ok()
            .filter(|bytes| {
                bytes.len() <= IMAGE_BYTES && qoda_provider::artwork::bounded_jpeg(bytes)
            })
            .map(Bytes::from);
        let mut data = art.data.lock().await;
        if *fetch_cancel.borrow() {
            return Err(StatusCode::NOT_FOUND);
        }
        data.put(grant.reference.clone(), image.clone());
        drop(data);
        if let Some(image) = &image {
            art.disk
                .put(grant.epoch, &grant.reference, image.clone())
                .await;
        }
        art.notify();
        Ok(image)
    };
    let result = tokio::select! {r=tokio::time::timeout(Duration::from_secs(25),fetch)=>r.map_err(|_|StatusCode::GATEWAY_TIMEOUT)??,_=cancel.changed()=>return Err(StatusCode::NOT_FOUND)};
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
                    4 => Err(ProviderError::Network),
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
    async fn cached_fixture(stale: bool) -> (crate::artwork_store::tests::Fixture, Disk, AlbumRef) {
        let fixture = crate::artwork_store::tests::Fixture::new();
        let reference = reference(1);
        let disk = Disk::start(Ok(fixture.paths.clone())).await;
        disk.put(
            disk.epoch(),
            &reference,
            Bytes::from_static(include_bytes!(
                "../../qoda-provider/tests/fixtures/cover.jpg"
            )),
        )
        .await;
        if stale {
            for file in std::fs::read_dir(&fixture.paths.cache).unwrap() {
                let path = file.unwrap().path();
                if path.extension().is_some_and(|ext| ext == "jpg") {
                    std::fs::File::open(path)
                        .unwrap()
                        .set_times(std::fs::FileTimes::new().set_modified(
                            std::time::SystemTime::now() - Duration::from_secs(8 * 86400),
                        ))
                        .unwrap();
                }
            }
        }
        (fixture, disk, reference)
    }
    #[tokio::test]
    async fn disk_artwork_is_reused_after_restart_without_a_provider_image_request() {
        let (fixture, disk, reference) = cached_fixture(false).await;
        disk.shutdown().await;
        let disk = Disk::start(Ok(fixture.paths.clone())).await;
        let art = Artwork::with_disk(disk.clone(), None).await.unwrap();
        let provider = Arc::new(Fixture::default());
        let url = art.publish(provider.clone(), [reference]).await["1"].clone();
        assert_eq!(reqwest::get(url).await.unwrap().status(), 200);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        disk.shutdown().await;
    }
    #[tokio::test]
    async fn stale_artwork_is_immediate_and_clearing_cancels_a_hung_refresh_without_resurrection() {
        let (_fixture, disk, reference) = cached_fixture(true).await;
        let art = Artwork::with_disk(disk.clone(), None).await.unwrap();
        let provider = Arc::new(Fixture {
            mode: 1,
            ..Default::default()
        });
        let url = art.publish(provider.clone(), [reference]).await["1"].clone();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), reqwest::get(url))
                .await
                .unwrap()
                .unwrap()
                .status(),
            200
        );
        tokio::time::timeout(Duration::from_secs(2), provider.started.notified())
            .await
            .unwrap();
        assert_eq!(art.view().await["stale"], true);
        art.revoke(true).await;
        assert!(art.data.lock().await.cache.is_empty());
        assert_eq!(disk.view().entries, 0);
        tokio::task::yield_now().await;
        assert!(art.data.lock().await.cache.is_empty());
        disk.shutdown().await;
    }
    #[tokio::test]
    async fn refresh_rotates_the_local_url_and_denial_purges_while_network_failure_retains_stale_bytes()
     {
        for mode in [0, 2, 4] {
            let (_fixture, disk, reference) = cached_fixture(true).await;
            let art = Artwork::with_disk(disk.clone(), None).await.unwrap();
            let provider = Arc::new(Fixture {
                mode,
                ..Default::default()
            });
            let old = art.publish(provider.clone(), [reference.clone()]).await["1"].clone();
            assert_eq!(reqwest::get(&old).await.unwrap().status(), 200);
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if provider.calls.load(Ordering::SeqCst) > 0
                        && art.data.lock().await.refreshing.is_empty()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let new = art.urls().await["1"].clone();
            if mode == 4 {
                assert_eq!(old, new);
                assert_eq!(reqwest::get(&new).await.unwrap().status(), 200);
                assert_eq!(disk.view().entries, 1);
            } else {
                assert_ne!(old, new);
                assert_eq!(reqwest::get(&old).await.unwrap().status(), 404);
                assert_eq!(
                    reqwest::get(&new).await.unwrap().status(),
                    if mode == 0 { 200 } else { 404 }
                );
                assert_eq!(disk.view().entries, usize::from(mode == 0));
                if mode == 0 {
                    assert!(
                        !disk
                            .get(disk.epoch(), &reference)
                            .await
                            .unwrap()
                            .fresh_for
                            .is_zero()
                    );
                }
            }
            assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
            disk.shutdown().await;
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
