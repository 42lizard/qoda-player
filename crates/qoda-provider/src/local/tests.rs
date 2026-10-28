use super::*;
use futures_util::StreamExt;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("qoda-local-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn wave(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        let samples = 48000_u32 * 4;
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
        std::fs::write(&path, wav).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn local_catalog_and_ranges_preserve_identity_and_refuse_changed_files() {
    let fixture = Fixture::new();
    let path = fixture.wave("Untitled Ä.strange");
    let root = Root::new(fixture.0.clone()).unwrap();
    let provider = Local::new(vec![root.clone()]);
    provider.scan().await.unwrap();
    let page = provider
        .tracks(root.id, 0, "untitled".into())
        .await
        .unwrap();
    assert_eq!(page.tracks.len(), 1);
    assert_eq!(page.tracks[0].title, "Untitled Ä");
    let reference = page.tracks[0].reference.clone();
    let media = provider
        .prepare(reference.source, reference.key.clone())
        .await
        .unwrap();
    let mut response = media
        .stream
        .open(Some(ByteRange::try_from("bytes=0-3").unwrap()), false)
        .await
        .unwrap();
    assert_eq!(response.status, 206);
    assert_eq!(response.content_length, Some(4));
    assert_eq!(
        response.body.next().await.unwrap().unwrap().as_ref(),
        b"RIFF"
    );
    let mut head = media.stream.open(None, true).await.unwrap();
    assert_eq!(head.status, 200);
    assert!(head.body.next().await.is_none());
    let beyond = media
        .stream
        .open(
            Some(ByteRange::try_from("bytes=999999999-").unwrap()),
            false,
        )
        .await
        .unwrap();
    assert_eq!(beyond.status, 416);
    let restarted = Local::new(vec![Root::new(fixture.0.clone()).unwrap()]);
    restarted.scan().await.unwrap();
    let same = restarted.tracks(root.id, 0, String::new()).await.unwrap();
    assert_eq!(same.tracks[0].reference, reference);
    assert_eq!(
        restarted
            .prepare(reference.source, reference.key.clone())
            .await
            .unwrap()
            .version,
        media.version
    );
    let mut active = media.stream.open(None, false).await.unwrap();
    assert!(active.body.next().await.unwrap().is_ok());
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[100] = 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        active.body.next().await,
        Some(Err(ProviderError::LocalChanged))
    ));
    assert!(matches!(
        media.stream.open(None, false).await,
        Err(ProviderError::LocalChanged)
    ));
    provider.scan().await.unwrap();
    assert_ne!(
        provider
            .prepare(reference.source, reference.key)
            .await
            .unwrap()
            .version,
        media.version
    );
    std::fs::remove_file(path).unwrap();
    provider.scan().await.unwrap();
    assert!(
        provider
            .tracks(root.id, 0, String::new())
            .await
            .unwrap()
            .tracks
            .is_empty()
    );
}

#[tokio::test]
async fn symlink_descendants_and_non_audio_files_are_not_catalogued() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let outside_file = outside.wave("outside.wav");
    std::os::unix::fs::symlink(outside_file, fixture.0.join("escape.wav")).unwrap();
    std::os::unix::fs::symlink(&outside.0, fixture.0.join("escape-folder")).unwrap();
    std::fs::write(fixture.0.join("notes.txt"), b"not music").unwrap();
    let root = Root::new(fixture.0.clone()).unwrap();
    let provider = Local::new(vec![root.clone()]);
    provider.scan().await.unwrap();
    assert!(
        provider
            .tracks(root.id, 0, String::new())
            .await
            .unwrap()
            .tracks
            .is_empty()
    );
}

#[tokio::test]
async fn embedded_metadata_drives_album_order_and_artist_browse() {
    let fixture = Fixture::new();
    let seed = fixture.wave("seed.wav");
    for (file, title, disc, number) in [
        ("second.flac", "Second", "2", "1"),
        ("first.flac", "First", "1", "10"),
    ] {
        let status = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&seed)
            .args([
                "-metadata",
                &format!("title={title}"),
                "-metadata",
                "artist=Artist Ä",
                "-metadata",
                "album=Album Ω",
                "-metadata",
                "album_artist=Album Artist",
                "-metadata",
                "date=2024",
                "-metadata",
                &format!("disc={disc}"),
                "-metadata",
                &format!("track={number}"),
                "-c:a",
                "flac",
            ])
            .arg(fixture.0.join(file))
            .status()
            .unwrap();
        assert!(status.success());
    }
    std::fs::remove_file(seed).unwrap();
    let root = Root::new(fixture.0.clone()).unwrap();
    let provider = Local::new(vec![root.clone()]);
    assert!(provider.scan().await.unwrap());
    assert!(!provider.scan().await.unwrap());
    let artists = provider
        .artists(root.id, 0, "album artist".into())
        .await
        .unwrap();
    assert_eq!(artists.artists.len(), 1);
    let albums = provider
        .artist_albums(artists.artists[0].reference.clone(), 0, String::new())
        .await
        .unwrap();
    assert_eq!(albums.albums.len(), 1);
    assert_eq!(albums.albums[0].title, "Album Ω");
    let album = provider
        .album(albums.albums[0].reference.clone())
        .await
        .unwrap();
    assert_eq!(album.tracks.len(), 2);
    assert_eq!(album.tracks[0].track.title, "First");
    assert_eq!(album.tracks[1].track.title, "Second");
    assert_eq!(album.tracks[0].disc, 1);
    assert_eq!(album.tracks[0].number, 10);
    assert_eq!(album.album.year, Some(2024));
    assert!(album.tracks[0].track.duration_ms.unwrap().get() >= 3900);
}

#[tokio::test]
async fn router_keeps_local_access_without_a_remote_account_and_does_not_transcode_on_plex() {
    use crate::router::Router;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Remote {
        source: SourceId,
        calls: AtomicUsize,
    }
    impl MusicProvider for Remote {
        fn libraries(&self) -> Vec<Library> {
            vec![Library {
                id: self.source,
                title: "Remote".into(),
                server: "Plex fixture".into(),
            }]
        }
        fn tracks(
            &self,
            source: SourceId,
            offset: u64,
            query: String,
        ) -> ProviderFuture<'_, CatalogPage> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(CatalogPage {
                    source,
                    offset: Counter::new(offset),
                    total: Some(Counter::ZERO),
                    query,
                    tracks: vec![],
                    has_more: false,
                })
            })
        }
        fn prepare(&self, _: SourceId, _: ProviderTrackKey) -> ProviderFuture<'_, PreparedMedia> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ProviderError::MissingTrack) })
        }
        fn prepare_transcoded(&self, _: TranscodeRequest) -> ProviderFuture<'_, PreparedMedia> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ProviderError::Unsupported) })
        }
    }
    let fixture = Fixture::new();
    fixture.wave("local.wav");
    let root = Root::new(fixture.0.clone()).unwrap();
    let local = Arc::new(Local::new(vec![root.clone()]));
    local.scan().await.unwrap();
    let remote = Arc::new(Remote {
        source: SourceId::new(),
        calls: AtomicUsize::new(0),
    });
    let router = Router::new(local.clone(), Some(remote.clone()));
    let track = router
        .tracks(root.id, 0, String::new())
        .await
        .unwrap()
        .tracks
        .remove(0);
    assert!(!router.cache_catalog(root.id));
    assert!(router.cache_catalog(remote.source));
    assert!(matches!(
        router
            .prepare_transcoded(TranscodeRequest {
                reference: track.reference.clone(),
                version: "pinned".into(),
                position: 0.0,
                targets: vec![TranscodeCodec::Flac]
            })
            .await,
        Err(ProviderError::Unsupported)
    ));
    assert_eq!(remote.calls.load(Ordering::SeqCst), 0);
    router
        .tracks(remote.source, 0, String::new())
        .await
        .unwrap();
    assert_eq!(remote.calls.load(Ordering::SeqCst), 1);
    let offline = Router::new(local, None);
    assert!(offline.prepare(root.id, track.reference.key).await.is_ok());
    assert!(matches!(
        offline.tracks(remote.source, 0, String::new()).await,
        Err(ProviderError::SignInRequired)
    ));
}
