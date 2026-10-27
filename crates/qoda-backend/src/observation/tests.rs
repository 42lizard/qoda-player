use super::*;
use qoda_core::{Counter, SourceId};
use qoda_provider::{
    CatalogPage, Library, PreparedMedia, ProviderError, ProviderFuture, Track,
    playback::{Outcome, Report},
};
use std::{path::PathBuf, sync::Mutex};
#[derive(Default)]
struct Reports(Mutex<Vec<Report>>);
impl MusicProvider for Reports {
    fn libraries(&self) -> Vec<Library> {
        vec![]
    }
    fn tracks(&self, _: SourceId, _: u64, _: String) -> ProviderFuture<'_, CatalogPage> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    fn prepare(
        &self,
        _: SourceId,
        _: qoda_core::ProviderTrackKey,
    ) -> ProviderFuture<'_, PreparedMedia> {
        Box::pin(async { Err(ProviderError::Unsupported) })
    }
    fn report_playback(&self, report: Report) -> ProviderFuture<'_, Outcome> {
        Box::pin(async move {
            self.0.lock().unwrap().push(report);
            Ok(Outcome::Accepted)
        })
    }
}
struct Environment {
    root: PathBuf,
    queue: queue::Queue,
    player: player::Player,
    epochs: gapless::Epochs,
    gapless: gapless::Controller,
    proxy: proxy::Proxy,
    confirmed: Option<u64>,
    playing: bool,
    reporting: reporting::Controller,
    archive: archive::Controller,
    provider: Arc<Reports>,
    account: u64,
}
impl Environment {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("qoda-observation-{}", uuid::Uuid::new_v4()));
        let mut queue = queue::Queue::default();
        let selected = queue
            .enqueue(
                Track {
                    reference: qoda_core::TrackRef {
                        source: SourceId::new(),
                        key: "synthetic".to_owned().try_into().unwrap(),
                    },
                    title: "Synthetic".into(),
                    artist: "Fixture".into(),
                    album: "Album".into(),
                    duration_ms: Some(Counter::new(10000)),
                },
                false,
            )
            .unwrap();
        queue.select(selected).unwrap();
        let mut epochs = gapless::Epochs::default();
        epochs.reset().unwrap();
        let mut archive = archive::Controller::start(Ok(root.join("archive.sqlite3")));
        archive.bind("synthetic-account".into()).await.unwrap();
        Self {
            root,
            queue,
            epochs,
            archive,
            player: player::Player::test_output("null").unwrap(),
            gapless: Default::default(),
            proxy: proxy::Proxy::start().await.unwrap(),
            confirmed: None,
            playing: true,
            reporting: reporting::Controller::new(),
            provider: Arc::default(),
            account: 1,
        }
    }
    fn runtime(&mut self) -> Runtime<'_> {
        Runtime {
            queue: &mut self.queue,
            player: &self.player,
            epochs: &mut self.epochs,
            gapless: &mut self.gapless,
            proxy: &self.proxy,
            confirmed: &mut self.confirmed,
            playing: &mut self.playing,
            reporting: &mut self.reporting,
            archive: &mut self.archive,
            scope: Scope {
                account: self.account,
                provider: Some(self.provider.clone()),
                can_raise: true,
            },
        }
    }
    fn audio(&self, position: Option<f64>) -> player::Status {
        player::Status {
            generation: self.epochs.current,
            position_seconds: position,
            entry_ready: true,
            idle: false,
            paused: false,
            ..Default::default()
        }
    }
    async fn settle(&mut self) {
        self.archive.flush().await.unwrap();
        self.archive.page(0).await.unwrap();
        for _ in 0..40 {
            tokio::task::yield_now().await;
        }
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
#[tokio::test]
async fn first_positive_clock_confirms_archive_and_display_before_reporting_the_next_observation() {
    let mut env = Environment::new().await;
    let mut owner = Controller::default();
    let now = Instant::now();
    let first = owner.observe(
        Sample {
            audio: env.audio(Some(1.25)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    assert_eq!(first.presentation.presentation()["known"], true);
    env.settle().await;
    assert_eq!(env.archive.view().total, 1);
    assert!(env.provider.0.lock().unwrap().is_empty());
    owner.observe(
        Sample {
            audio: env.audio(Some(1.3)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    env.settle().await;
    let calls = env.provider.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].position_ms, 1300);
    assert_eq!(calls[0].track, env.archive.view().entries[0].track);
}

#[tokio::test]
async fn stalled_evidence_is_unknown_but_a_confirmed_pause_has_distinct_consumer_rules() {
    let mut env = Environment::new().await;
    let mut owner = Controller::default();
    let now = Instant::now();
    owner.observe(
        Sample {
            audio: env.audio(Some(1.0)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    owner.observe(
        Sample {
            audio: env.audio(Some(1.1)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    env.settle().await;
    assert_eq!(env.provider.0.lock().unwrap().len(), 1);
    let later = now + Duration::from_millis(300);
    let stalled = owner.observe(
        Sample {
            audio: env.audio(Some(1.1)),
            failed: false,
            at: later,
        },
        later,
        &mut env.runtime(),
    );
    assert_eq!(stalled.presentation.presentation()["known"], false);
    env.settle().await;
    assert_eq!(env.archive.view().total, 1);
    assert_eq!(env.provider.0.lock().unwrap().len(), 1);
    let mut paused = env.audio(Some(1.1));
    paused.paused = true;
    env.playing = false;
    let paused = owner.observe(
        Sample {
            audio: paused,
            failed: false,
            at: later,
        },
        later,
        &mut env.runtime(),
    );
    assert_eq!(paused.presentation.presentation()["known"], true);
    assert_eq!(paused.presentation.presentation()["status"], "Paused");
    env.settle().await;
    let calls = env.provider.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].state, qoda_provider::playback::State::Paused);
    assert_eq!(env.archive.view().entries[0].last_position_ms, 1000);
}
#[tokio::test]
async fn missing_duration_can_archive_and_display_without_provider_reporting() {
    let mut env = Environment::new().await;
    let mut owner = Controller::default();
    let now = Instant::now();
    let mut track = env
        .queue
        .visible_track(env.queue.selected().unwrap())
        .unwrap();
    track.duration_ms = None;
    env.queue.update_selected(track, "fixture").unwrap();
    let tick = owner.observe(
        Sample {
            audio: env.audio(Some(1.0)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    owner.observe(
        Sample {
            audio: env.audio(Some(1.1)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    assert_eq!(tick.presentation.presentation()["known"], true);
    env.settle().await;
    assert_eq!(env.archive.view().total, 1);
    assert!(env.provider.0.lock().unwrap().is_empty());
}
#[tokio::test]
async fn stream_failure_hides_current_evidence_and_emits_only_one_interruption() {
    let mut env = Environment::new().await;
    let mut owner = Controller::default();
    let now = Instant::now();
    owner.observe(
        Sample {
            audio: env.audio(Some(1.0)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    owner.observe(
        Sample {
            audio: env.audio(Some(1.1)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    env.settle().await;
    let failed = owner.observe(
        Sample {
            audio: env.audio(Some(1.1)),
            failed: true,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    assert_eq!(failed.presentation.presentation()["known"], false);
    assert!(matches!(failed.event, Some(Event::StreamFailed(_, true))));
    assert!(!env.playing);
    let duplicate = owner.observe(
        Sample {
            audio: env.audio(Some(1.1)),
            failed: true,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    assert!(duplicate.event.is_none());
    env.settle().await;
    assert_eq!(env.archive.view().total, 1);
    assert_eq!(env.provider.0.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn successor_without_a_fresh_clock_cannot_inherit_predecessor_evidence() {
    let mut env = Environment::new().await;
    let mut owner = Controller::default();
    let now = Instant::now();
    owner.observe(
        Sample {
            audio: env.audio(Some(1.0)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    owner.observe(
        Sample {
            audio: env.audio(Some(1.1)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    env.settle().await;
    let first = env.queue.selected().unwrap();
    let mut next = env.queue.visible_track(first).unwrap();
    next.reference.key = "successor".to_owned().try_into().unwrap();
    let next = env.queue.enqueue(next, false).unwrap();
    env.queue.select(next).unwrap();
    env.epochs.reset().unwrap();
    let handoff = owner.observe(
        Sample {
            audio: env.audio(None),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    assert_eq!(handoff.presentation.presentation()["known"], false);
    env.settle().await;
    assert_eq!(env.archive.view().total, 1);
    assert!(
        !env.provider
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|report| String::from(report.track.key.clone()) == "successor")
    );
    let fresh = owner.observe(
        Sample {
            audio: env.audio(Some(0.02)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    assert_eq!(fresh.presentation.presentation()["known"], true);
    owner.observe(
        Sample {
            audio: env.audio(Some(0.03)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    env.settle().await;
    let archive = env.archive.view();
    assert_eq!(archive.total, 2);
    let calls = env.provider.0.lock().unwrap();
    let successor = calls
        .iter()
        .find(|report| String::from(report.track.key.clone()) == "successor")
        .unwrap();
    assert_eq!(successor.position_ms, 30);
    assert!(
        archive
            .entries
            .iter()
            .any(|entry| entry.occurrence == next && entry.track == successor.track)
    );
    assert!(
        archive
            .entries
            .iter()
            .any(|entry| entry.occurrence == first)
    );
}
#[tokio::test]
async fn account_replacement_rejects_old_output_until_a_new_epoch_and_clock() {
    let mut env = Environment::new().await;
    let mut owner = Controller::default();
    let now = Instant::now();
    owner.observe(
        Sample {
            audio: env.audio(Some(1.0)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    env.settle().await;
    env.account = 2;
    env.archive
        .bind("replacement-account".into())
        .await
        .unwrap();
    let old = owner.observe(
        Sample {
            audio: env.audio(Some(1.1)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    assert_eq!(old.presentation.presentation()["known"], false);
    assert!(env.confirmed.is_none());
    env.settle().await;
    assert_eq!(env.archive.view().total, 0);
    let mut late_end = env.audio(Some(1.2));
    late_end.idle = true;
    late_end.natural_eof = true;
    let late_end = owner.observe(
        Sample {
            audio: late_end,
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    assert!(late_end.event.is_none());
    let old_audio = env.audio(Some(1.2));
    env.epochs.reset().unwrap();
    let mismatch = owner.observe(
        Sample {
            audio: old_audio,
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    assert_eq!(mismatch.presentation.presentation()["known"], false);
    let new = owner.observe(
        Sample {
            audio: env.audio(Some(0.05)),
            failed: false,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    assert_eq!(new.presentation.presentation()["known"], true);
    env.settle().await;
    assert_eq!(env.archive.view().total, 1);
}

#[tokio::test]
async fn output_barrier_retains_position_without_confirming_a_stalled_positive_clock() {
    let mut env = Environment::new().await;
    let mut owner = Controller::default();
    let now = Instant::now();
    owner.observe(
        Sample {
            audio: env.audio(Some(1.25)),
            failed: true,
            at: now,
        },
        now,
        &mut env.runtime(),
    );
    assert!(env.confirmed.is_none());
    let later = now + Duration::from_millis(300);
    let sample = Sample {
        audio: env.audio(Some(1.25)),
        failed: false,
        at: later,
    };
    let changed = owner.record_barrier(
        sample,
        later,
        Confirmation {
            queue: &mut env.queue,
            account: env.account,
            generation: env.epochs.current,
            confirmed: &mut env.confirmed,
        },
    );
    assert!(!changed);
    assert!(env.confirmed.is_none());
    assert_eq!(env.queue.snapshot().position_seconds, 1.25);
    let sample = Sample {
        audio: env.audio(Some(1.3)),
        failed: false,
        at: later,
    };
    assert!(owner.record_barrier(
        sample,
        later,
        Confirmation {
            queue: &mut env.queue,
            account: env.account,
            generation: env.epochs.current,
            confirmed: &mut env.confirmed,
        }
    ));
    assert_eq!(env.confirmed, Some(env.epochs.current));
    env.settle().await;
    assert_eq!(env.archive.view().total, 0);
    assert!(env.provider.0.lock().unwrap().is_empty());
}
