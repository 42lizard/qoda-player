use super::*;
use qoda_provider::{
    ByteRange, Download, Library, MediaResponse, MediaSource, PreparedMedia, ProviderError,
    ProviderFuture,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
struct Provider {
    track: Track,
    bytes: Vec<u8>,
    opens: Arc<AtomicUsize>,
    pause: bool,
    wrong_length: bool,
    version: &'static str,
    pause_close: Arc<AtomicBool>,
}
struct Stream {
    bytes: Vec<u8>,
    opens: Arc<AtomicUsize>,
    pause: bool,
    wrong_length: bool,
    pause_close: Arc<AtomicBool>,
}
impl MediaSource for Stream {
    fn close(&self) -> ProviderFuture<'_, ()> {
        Box::pin(async move {
            if self.pause_close.load(Ordering::SeqCst) {
                std::future::pending().await
            } else {
                Ok(())
            }
        })
    }

    fn open(&self, _: Option<ByteRange>, _: bool) -> ProviderFuture<'_, MediaResponse> {
        Box::pin(async move {
            self.opens.fetch_add(1, Ordering::SeqCst);
            let first = futures_util::stream::once(std::future::ready(Ok(bytes::Bytes::from(
                self.bytes.clone(),
            ))));
            let body: futures_util::stream::BoxStream<
                'static,
                Result<bytes::Bytes, ProviderError>,
            > = if self.pause {
                Box::pin(first.chain(futures_util::stream::pending()))
            } else {
                Box::pin(first)
            };
            Ok(MediaResponse {
                status: 200,
                content_type: None,
                content_length: Some(self.bytes.len() as u64 + u64::from(self.wrong_length)),
                content_range: None,
                etag: Some("fixture".into()),
                body,
            })
        })
    }
}
impl MusicProvider for Provider {
    fn libraries(&self) -> Vec<Library> {
        vec![Library {
            id: self.track.reference.source,
            title: "Fixture".into(),
            server: "Fixture".into(),
        }]
    }
    fn downloads(&self, source: qoda_core::SourceId) -> bool {
        source == self.track.reference.source
    }
    fn download(
        &self,
        _: qoda_core::SourceId,
        key: qoda_core::ProviderTrackKey,
    ) -> ProviderFuture<'_, Download> {
        Box::pin(async move {
            let mut track = self.track.clone();
            track.reference.key = key;
            Ok(Download {
                album: None,
                artwork: Some(
                    include_bytes!("../../../qoda-provider/tests/fixtures/cover.jpg").to_vec(),
                ),
                expected_length: None,
                extension: "wav".into(),
                media: PreparedMedia {
                    track,
                    version: self.version.into(),
                    stream: Box::new(Stream {
                        bytes: self.bytes.clone(),
                        opens: self.opens.clone(),
                        pause: self.pause,
                        wrong_length: self.wrong_length,
                        pause_close: self.pause_close.clone(),
                    }),
                },
            })
        })
    }
    fn tracks(
        &self,
        _: qoda_core::SourceId,
        _: u64,
        _: String,
    ) -> ProviderFuture<'_, qoda_provider::CatalogPage> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    fn prepare(
        &self,
        _: qoda_core::SourceId,
        _: qoda_core::ProviderTrackKey,
    ) -> ProviderFuture<'_, PreparedMedia> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
}
fn fixture(pause: bool, wrong_length: bool) -> (PathBuf, Root, Arc<Provider>) {
    let path = std::env::temp_dir().join(format!("qoda-download-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(path.join("music")).unwrap();
    let root = Root::new(path.join("music")).unwrap();
    let samples = 48000_u32;
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((36 + samples * 2).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16_u32.to_le_bytes());
    wav.extend(1_u16.to_le_bytes());
    wav.extend(1_u16.to_le_bytes());
    wav.extend(48000_u32.to_le_bytes());
    wav.extend(96000_u32.to_le_bytes());
    wav.extend(2_u16.to_le_bytes());
    wav.extend(16_u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((samples * 2).to_le_bytes());
    wav.resize(44 + samples as usize * 2, 0);
    let provider = Arc::new(Provider {
        track: Track {
            reference: TrackRef {
                source: qoda_core::SourceId::new(),
                key: "1".to_owned().try_into().unwrap(),
            },
            title: "Original Ä".into(),
            artist: "Fixture".into(),
            album: "Fixture".into(),
            duration_ms: None,
        },
        bytes: wav,
        opens: Arc::new(AtomicUsize::new(0)),
        pause,
        wrong_length,
        version: "original-v1",
        pause_close: Arc::new(AtomicBool::new(false)),
    });
    (path, root, provider)
}
async fn until(rx: &mut mpsc::Receiver<Event>, predicate: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        loop {
            if let Some(Event::Downloads(view, _)) = rx.recv().await
                && predicate(&view)
            {
                return view;
            }
        }
    })
    .await
    .expect("download view timed out")
}
fn request(controller: &Controller, op: &str, revision: &str, fields: Value, tracks: Vec<Track>) {
    let mut value = json!({"op":op,"account_epoch":"1","downloads_revision":revision});
    for (k, v) in fields.as_object().unwrap() {
        value[k] = v.clone();
    }
    controller.request(value, tracks).unwrap();
}
#[tokio::test]
async fn completed_originals_are_idempotent_local_music_and_survive_account_and_history_cleanup() {
    let (path, root, provider) = fixture(false, false);
    let (tx, mut rx) = mpsc::channel(64);
    let mut controller = Controller::start(Ok(path.join("state")), tx);
    controller.sync(1, Some(provider.clone()), vec![root.clone()]);
    request(
        &controller,
        "download_destination",
        "0",
        json!({"source":root.id}),
        vec![],
    );
    until(&mut rx, |v| v["destination_revision"] == "1").await;
    request(
        &controller,
        "download_start",
        "1",
        json!({}),
        vec![provider.track.clone()],
    );
    let completed = until(&mut rx, |v| v["jobs"][0]["phase"] == "completed").await;
    assert_eq!(
        completed["jobs"][0]["bytes"],
        provider.bytes.len().to_string()
    );
    let local = qoda_provider::local::Local::new(vec![root.clone()]);
    local.scan().await.unwrap();
    let page = local.tracks(root.id, 0, String::new()).await.unwrap();
    assert_eq!(page.tracks.len(), 1);
    let album = local
        .albums(root.id, 0, String::new())
        .await
        .unwrap()
        .albums
        .remove(0);
    assert!(qoda_provider::artwork::bounded_jpeg(
        &local.cover(album.reference.clone()).await.unwrap()
    ));
    let imported = Store::load(&cache_files::Directory::open(&path.join("state")).unwrap())
        .unwrap()
        .imports
        .remove(0);
    let sidecar = root
        .path
        .join(import::MANAGED)
        .join(qoda_provider::local::cover_sidecar(Path::new(
            &imported.filename,
        )));
    std::fs::remove_file(&sidecar).unwrap();
    let media = local
        .prepare(root.id, page.tracks[0].reference.key.clone())
        .await
        .unwrap();
    let mut response = media.stream.open(None, false).await.unwrap();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.body.next().await {
        bytes.extend(chunk.unwrap())
    }
    assert_eq!(bytes, provider.bytes);
    let proxy = crate::proxy::Proxy::start().await.unwrap();
    let player = crate::player::Player::test_output("null").unwrap();
    let url = proxy.grant(media.stream).await;
    player.send(crate::player::Command::Load(1, url, 0.0));
    player.send(crate::player::Command::Pause(false));
    crate::gapless::tests::wait(&player, |s| s.position_seconds.is_some_and(|p| p > 0.0)).await;
    player.barrier().await.unwrap();
    proxy.revoke().await;

    request(
        &controller,
        "download_start",
        "1",
        json!({}),
        vec![provider.track.clone()],
    );
    until(&mut rx, |v| {
        v["jobs"]
            .as_array()
            .is_some_and(|j| j.len() == 2 && j.iter().all(|j| j["phase"] == "completed"))
    })
    .await;
    assert_eq!(provider.opens.load(Ordering::SeqCst), 1);
    assert!(sidecar.exists());
    controller.sync(2, None, vec![root.clone()]);
    assert!(qoda_provider::artwork::bounded_jpeg(
        &local.cover(album.reference).await.unwrap()
    ));
    let clear = json!({"op":"download_clear","account_epoch":"2","downloads_revision":"1"});
    controller.request(clear.clone(), vec![]).unwrap();
    let cleared = until(&mut rx, |v| v["jobs"] == json!([])).await;
    assert_eq!(cleared["copies"].as_array().unwrap().len(), 1);
    let copy = &cleared["copies"][0];
    let track = local_copy(
        &cleared,
        &json!({"copy_key":copy["key"],"copy_source":copy["local"]["source"]}),
    )
    .unwrap();
    assert_eq!(track.track.reference.source, root.id);
    controller.shutdown().await;
    drop(controller);
    assert_eq!(
        local
            .tracks(root.id, 0, String::new())
            .await
            .unwrap()
            .tracks
            .len(),
        1
    );
    let (tx, mut rx) = mpsc::channel(64);
    let _controller = Controller::start(Ok(path.join("state")), tx);
    until(&mut rx, |v| v["available"] == true).await;
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn incomplete_and_cancelled_downloads_are_not_local_tracks_and_restart_stays_paused() {
    let (path, root, provider) = fixture(true, false);
    let (tx, mut rx) = mpsc::channel(64);
    let mut controller = Controller::start(Ok(path.join("state")), tx);
    controller.sync(1, Some(provider.clone()), vec![root.clone()]);
    request(
        &controller,
        "download_destination",
        "0",
        json!({"source":root.id}),
        vec![],
    );
    until(&mut rx, |v| v["destination_revision"] == "1").await;
    request(
        &controller,
        "download_start",
        "1",
        json!({}),
        vec![provider.track.clone()],
    );
    until(&mut rx, |v| v["jobs"][0]["phase"] == "running").await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    // A stalled download body must leave browsing and native transport usable.
    let proxy = crate::proxy::Proxy::start().await.unwrap();
    let player = crate::player::Player::test_output("null").unwrap();
    let url = proxy
        .grant(Box::new(Stream {
            bytes: provider.bytes.clone(),
            opens: Arc::new(AtomicUsize::new(0)),
            pause: false,
            wrong_length: false,
            pause_close: Arc::new(AtomicBool::new(false)),
        }))
        .await;
    player.send(crate::player::Command::Load(1, url, 0.0));
    player.send(crate::player::Command::Pause(false));
    crate::gapless::tests::wait(&player, |s| s.position_seconds.is_some_and(|p| p > 0.0)).await;
    player.send(crate::player::Command::Pause(true));
    tokio::time::timeout(std::time::Duration::from_secs(2), player.barrier())
        .await
        .unwrap()
        .unwrap();
    proxy.revoke().await;
    let local = qoda_provider::local::Local::new(vec![root.clone()]);
    local.scan().await.unwrap();
    assert!(
        local
            .tracks(root.id, 0, String::new())
            .await
            .unwrap()
            .tracks
            .is_empty()
    );
    controller.sync(2, None, vec![root.clone()]);
    until(&mut rx, |v| v["jobs"][0]["phase"] == "paused").await;
    assert!(
        local
            .tracks(root.id, 0, String::new())
            .await
            .unwrap()
            .tracks
            .is_empty()
    );
    drop(controller);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let (tx, mut rx) = mpsc::channel(64);
    let mut controller = Controller::start(Ok(path.join("state")), tx);
    controller.sync(1, Some(provider.clone()), vec![root.clone()]);
    let paused = until(&mut rx, |v| v["jobs"][0]["phase"] == "paused").await;
    assert_eq!(provider.opens.load(Ordering::SeqCst), 1);
    request(
        &controller,
        "download_cancel",
        "1",
        json!({"job_id":paused["jobs"][0]["id"]}),
        vec![],
    );
    until(&mut rx, |v| v["jobs"][0]["phase"] == "cancelled").await;
    drop(controller);
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn truncated_original_and_unsupported_history_are_preserved_without_publication() {
    let (path, root, provider) = fixture(false, true);
    let (tx, mut rx) = mpsc::channel(64);
    let mut controller = Controller::start(Ok(path.join("state")), tx);
    controller.sync(1, Some(provider.clone()), vec![root.clone()]);
    request(
        &controller,
        "download_destination",
        "0",
        json!({"source":root.id}),
        vec![],
    );
    until(&mut rx, |v| v["destination_revision"] == "1").await;
    request(
        &controller,
        "download_start",
        "1",
        json!({}),
        vec![provider.track.clone()],
    );
    until(&mut rx, |v| v["jobs"][0]["phase"] == "failed").await;
    let local = qoda_provider::local::Local::new(vec![root.clone()]);
    local.scan().await.unwrap();
    assert!(
        local
            .tracks(root.id, 0, String::new())
            .await
            .unwrap()
            .tracks
            .is_empty()
    );
    drop(controller);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let state = path.join("state/state.json");
    let bytes = std::fs::read(&state).unwrap();
    let mut newer: Value = serde_json::from_slice(&bytes).unwrap();
    newer["version"] = json!(99);
    let bytes = serde_json::to_vec(&newer).unwrap();
    std::fs::write(&state, &bytes).unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    let _controller = Controller::start(Ok(path.join("state")), tx);
    until(&mut rx, |v| v["available"] == false).await;
    assert_eq!(std::fs::read(state).unwrap(), bytes);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn restart_reconciles_a_published_original_before_completion_was_checkpointed() {
    let (path, root, provider) = fixture(false, false);
    provider.pause_close.store(true, Ordering::SeqCst);
    let (tx, mut rx) = mpsc::channel(64);
    let mut controller = Controller::start(Ok(path.join("state")), tx);
    controller.sync(1, Some(provider.clone()), vec![root.clone()]);
    request(
        &controller,
        "download_destination",
        "0",
        json!({"source":root.id}),
        vec![],
    );
    until(&mut rx, |v| v["destination_revision"] == "1").await;
    request(
        &controller,
        "download_start",
        "1",
        json!({}),
        vec![provider.track.clone()],
    );
    let running = until(&mut rx, |v| v["jobs"][0]["phase"] == "running").await;
    let local = qoda_provider::local::Local::new(vec![root.clone()]);
    for _ in 0..30 {
        local.scan().await.unwrap();
        if !local
            .tracks(root.id, 0, String::new())
            .await
            .unwrap()
            .tracks
            .is_empty()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    assert_eq!(
        local
            .tracks(root.id, 0, String::new())
            .await
            .unwrap()
            .tracks
            .len(),
        1
    );
    request(
        &controller,
        "download_cancel",
        "1",
        json!({"job_id":running["jobs"][0]["id"]}),
        vec![],
    );
    until(&mut rx, |v| v["jobs"][0]["phase"] == "cancelled").await;
    request(&controller, "download_clear", "1", json!({}), vec![]);
    until(&mut rx, |v| {
        v["message"]
            .as_str()
            .is_some_and(|s| s.contains("Unresolved"))
    })
    .await;
    let directory = cache_files::Directory::open(&path.join("state")).unwrap();
    let retained = Store::load(&directory).unwrap();
    assert_eq!(retained.jobs.len(), 1);
    assert!(retained.imports.is_empty());
    assert_eq!(retained.import_records().len(), 1);
    assert_eq!(
        retained.import_records()[0].bytes,
        provider.bytes.len() as u64
    );
    drop(controller);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let (tx, mut rx) = mpsc::channel(64);
    let mut controller = Controller::start(Ok(path.join("state")), tx);
    controller.sync(0, None, vec![root]);
    let view = until(&mut rx, |v| {
        v["jobs"][0]["phase"] == "completed" && v["jobs"][0]["local"].is_object()
    })
    .await;
    assert!(view["jobs"][0]["local"].is_object());
    assert_eq!(provider.opens.load(Ordering::SeqCst), 1);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn only_one_download_owner_can_write_a_profile_and_shutdown_releases_ownership() {
    let (path, _root, _provider) = fixture(false, false);
    let (tx, mut rx) = mpsc::channel(64);
    let mut first = Controller::start(Ok(path.join("state")), tx);
    until(&mut rx, |v| v["available"] == true).await;
    let (tx, mut rx) = mpsc::channel(64);
    let second = Controller::start(Ok(path.join("state")), tx);
    let unavailable = until(&mut rx, |v| v["available"] == false).await;
    assert!(
        unavailable["message"]
            .as_str()
            .unwrap()
            .contains("another backend")
    );
    drop(second);
    first.shutdown().await;
    drop(first);
    let (tx, mut rx) = mpsc::channel(64);
    let mut next = Controller::start(Ok(path.join("state")), tx);
    until(&mut rx, |v| v["available"] == true).await;
    next.shutdown().await;
    drop(next);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn stale_download_requests_leave_the_journal_and_music_unchanged() {
    let (path, root, provider) = fixture(false, false);
    let (tx, mut rx) = mpsc::channel(64);
    let mut controller = Controller::start(Ok(path.join("state")), tx);
    controller.sync(1, Some(provider.clone()), vec![root.clone()]);
    request(
        &controller,
        "download_destination",
        "0",
        json!({"source":root.id}),
        vec![],
    );
    until(&mut rx, |v| v["destination_revision"] == "1").await;
    let before = std::fs::read(path.join("state/state.json")).unwrap();
    request(
        &controller,
        "download_start",
        "0",
        json!({}),
        vec![provider.track.clone()],
    );
    let rejected = until(&mut rx, |v| {
        v["message"].as_str().is_some_and(|s| s.contains("changed"))
    })
    .await;
    assert_eq!(rejected["jobs"], json!([]));
    assert_eq!(provider.opens.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(path.join("state/state.json")).unwrap(),
        before
    );
    controller.shutdown().await;
    drop(controller);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn a_reply_from_a_cancelled_attempt_cannot_mutate_the_retried_job() {
    let (path, root, provider) = fixture(false, false);
    let directory = cache_files::Directory::open(&path.join("state")).unwrap();
    let owner = Owner::open(&directory).unwrap();
    let mut job = Job::new(provider.track.clone(), root);
    job.phase = Phase::Running;
    let id = job.id.clone();
    let (events, _rx) = mpsc::channel(64);
    let (work, _replies) = mpsc::channel(32);
    let mut actor = Actor {
        _owner: owner,
        directory,
        store: Store {
            jobs: vec![job],
            ..Store::default()
        },
        context: Context {
            generation: 3,
            account: 1,
            provider: Some(provider),
            roots: vec![],
        },
        gate: Arc::new(Mutex::new(3)),
        active: Some(Active {
            id: id.clone(),
            generation: 3,
            task: tokio::spawn(std::future::pending()),
        }),
        events,
        work,
        problem: String::new(),
        writable: true,
        dirty: false,
        checkpoint: std::time::Instant::now(),
    };
    let (reply, ack) = oneshot::channel();
    assert!(!actor.apply((
        2,
        Work::Pin {
            id: id.clone(),
            version: "obsolete".into(),
            reply
        }
    )));
    assert!(ack.await.is_err());
    assert!(!actor.apply((
        2,
        Work::Done {
            id: id.clone(),
            result: Err("obsolete failure".into())
        }
    )));
    assert!(actor.store.jobs[0].phase == Phase::Running);
    assert!(actor.store.jobs[0].version.is_none());
    assert!(actor.active.is_some());
    actor.apply((
        3,
        Work::Done {
            id,
            result: Err("current failure".into()),
        },
    ));
    assert!(actor.store.jobs[0].phase == Phase::Failed);
    assert!(actor.active.is_none());
    drop(actor);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn changed_upstream_versions_require_a_new_explicit_download() {
    let (path, root, provider) = fixture(false, true);
    let (tx, mut rx) = mpsc::channel(64);
    let mut controller = Controller::start(Ok(path.join("state")), tx);
    controller.sync(1, Some(provider.clone()), vec![root.clone()]);
    request(
        &controller,
        "download_destination",
        "0",
        json!({"source":root.id}),
        vec![],
    );
    until(&mut rx, |v| v["destination_revision"] == "1").await;
    request(
        &controller,
        "download_start",
        "1",
        json!({}),
        vec![provider.track.clone()],
    );
    let failed = until(&mut rx, |v| v["jobs"][0]["phase"] == "failed").await;
    let changed = Arc::new(Provider {
        track: provider.track.clone(),
        bytes: provider.bytes.clone(),
        opens: provider.opens.clone(),
        pause: false,
        wrong_length: false,
        version: "original-v2",
        pause_close: provider.pause_close.clone(),
    });
    controller.sync(1, Some(changed.clone()), vec![root.clone()]);
    request(
        &controller,
        "download_retry",
        "1",
        json!({"job_id":failed["jobs"][0]["id"]}),
        vec![],
    );
    let rejected = until(&mut rx, |v| {
        v["jobs"][0]["phase"] == "failed"
            && v["jobs"][0]["message"]
                .as_str()
                .is_some_and(|s| s.contains("changed"))
    })
    .await;
    assert!(rejected["jobs"][0]["local"].is_null());
    assert_eq!(provider.opens.load(Ordering::SeqCst), 1);
    request(
        &controller,
        "download_start",
        "1",
        json!({}),
        vec![changed.track.clone()],
    );
    until(&mut rx, |v| v["jobs"][0]["phase"] == "completed").await;
    assert_eq!(provider.opens.load(Ordering::SeqCst), 2);
    controller.shutdown().await;
    drop(controller);
    std::fs::remove_dir_all(path).unwrap();
}

#[tokio::test]
async fn album_downloads_admit_all_tracks_in_order_and_reject_capacity_without_partial_jobs() {
    let (path, root, provider) = fixture(false, false);
    let (tx, mut rx) = mpsc::channel(64);
    let mut controller = Controller::start(Ok(path.join("state")), tx);
    controller.sync(1, Some(provider.clone()), vec![root.clone()]);
    request(
        &controller,
        "download_destination",
        "0",
        json!({"source":root.id}),
        vec![],
    );
    until(&mut rx, |v| v["destination_revision"] == "1").await;
    let tracks: Vec<_> = (1..=3)
        .map(|n| {
            let mut track = provider.track.clone();
            track.reference.key = n.to_string().try_into().unwrap();
            track
        })
        .collect();
    let fields = json!({"reference":{"source":provider.track.reference.source,"key":"album"},"expected_revision":"album-v1"});
    request(
        &controller,
        "download_start",
        "1",
        fields.clone(),
        tracks.clone(),
    );
    until(&mut rx, |v| {
        v["jobs"]
            .as_array()
            .is_some_and(|jobs| jobs.len() == 3 && jobs.iter().all(|j| j["phase"] == "completed"))
    })
    .await;
    let store = Store::load(&cache_files::Directory::open(&path.join("state")).unwrap()).unwrap();
    assert_eq!(
        store
            .jobs
            .iter()
            .map(|j| j.track.reference.clone())
            .collect::<Vec<_>>(),
        tracks
            .iter()
            .map(|t| t.reference.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(provider.opens.load(Ordering::SeqCst), 3);
    request(
        &controller,
        "download_start",
        "1",
        fields,
        vec![provider.track.clone(); MAX_JOBS],
    );
    let rejected = until(&mut rx, |v| {
        v["message"]
            .as_str()
            .is_some_and(|s| s.contains("200 download jobs"))
    })
    .await;
    assert_eq!(rejected["jobs"].as_array().unwrap().len(), 3);
    assert_eq!(provider.opens.load(Ordering::SeqCst), 3);
    controller.shutdown().await;
    drop(controller);
    std::fs::remove_dir_all(path).unwrap();
}
