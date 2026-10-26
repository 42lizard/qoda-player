use super::*;
use qoda_core::{Counter, SourceId};
use qoda_provider::{CatalogPage, Library, PreparedMedia, ProviderError, ProviderFuture, Track};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

fn key() -> Key {
    Key {
        account: 1,
        session: SessionId::new(),
        occurrence: OccurrenceId::new(),
        generation: 7,
        track: TrackRef {
            source: SourceId::new(),
            key: "42".to_owned().try_into().unwrap(),
        },
    }
}
fn sample(key: Key, position: u64, paused: bool) -> Sample {
    Sample {
        key,
        duration: NonZeroU64::new(10000).unwrap(),
        position: Some(position),
        paused,
    }
}
fn audio() -> player::Status {
    player::Status {
        generation: 7,
        position_seconds: Some(1.25),
        entry_ready: true,
        idle: false,
        paused: false,
        ..Default::default()
    }
}
fn queue() -> queue::Queue {
    let mut queue = queue::Queue::default();
    let track = Track {
        reference: key().track,
        title: "synthetic".into(),
        artist: String::new(),
        album: String::new(),
        duration_ms: Some(Counter::new(10000)),
    };
    let id = queue.enqueue(track, false).unwrap();
    queue.select(id).unwrap();
    queue
}
fn context() -> Context {
    Context {
        account: 1,
        generation: 7,
        confirmed: Some(7),
        failed_or_stale: false,
    }
}

#[test]
fn decoder_selection_restore_prefetch_and_unknown_clocks_do_not_establish_reporting() {
    let q = queue();
    let mut a = audio();
    assert!(Observation::capture(&q, &a, context()).0.is_some());
    let mut c = context();
    c.confirmed = None;
    assert!(Observation::capture(&q, &a, c).0.is_none());
    let mut c = context();
    c.failed_or_stale = true;
    assert!(Observation::capture(&q, &a, c).0.is_none());
    a.generation = 8;
    assert!(Observation::capture(&q, &a, context()).0.is_none());
    a.generation = 7;
    for position in [
        None,
        Some(0.0),
        Some(-1.0),
        Some(f64::NAN),
        Some(f64::INFINITY),
    ] {
        a.position_seconds = position;
        assert!(Observation::capture(&q, &a, context()).0.is_none());
    }
    a = audio();
    a.idle = true;
    a.natural_eof = true;
    assert!(Observation::capture(&q, &a, context()).0.is_none());
    a = audio();
    a.paused = true;
    let sample = Observation::capture(&q, &a, context()).0.unwrap();
    assert!(
        Tracker::default()
            .observe(sample, Instant::now())
            .is_empty()
    );
    let mut unknown = q.visible_track(q.selected().unwrap()).unwrap();
    unknown.duration_ms = None;
    let mut q = queue::Queue::default();
    let id = q.enqueue(unknown, false).unwrap();
    q.select(id).unwrap();
    assert!(Observation::capture(&q, &audio(), context()).0.is_none());
}

#[test]
fn nominal_positions_are_rounded_and_clamped_without_creating_completion_credit() {
    let q = queue();
    let mut a = audio();
    a.position_seconds = Some(1.2346);
    assert_eq!(
        Observation::capture(&q, &a, context()).0.unwrap().position,
        Some(1235)
    );
    a.position_seconds = Some(100000.0);
    assert_eq!(
        Observation::capture(&q, &a, context()).0.unwrap().position,
        Some(10000)
    );
}

#[test]
fn pause_stop_keep_last_qualified_position_and_updates_are_bounded() {
    let mut tracker = Tracker::default();
    let k = key();
    let now = Instant::now();
    let first = tracker.observe(sample(k.clone(), 1250, false), now);
    assert_eq!(first.len(), 1);
    assert!(
        tracker
            .observe(sample(k.clone(), 2000, false), now + Duration::from_secs(1))
            .is_empty()
    );
    let pause = tracker.observe(sample(k.clone(), 9999, true), now + Duration::from_secs(2));
    assert_eq!(pause[0].position_ms, 2000);
    assert_eq!(pause[0].state, State::Paused);
    assert!(
        tracker
            .observe(sample(k.clone(), 9000, true), now + Duration::from_secs(3))
            .is_empty()
    );
    let resume = tracker.observe(sample(k.clone(), 2100, false), now + Duration::from_secs(4));
    assert_eq!(resume[0].session, first[0].session);
    assert_eq!(resume[0].position_ms, 2100);
    assert!(
        tracker
            .observe(
                sample(k.clone(), 3000, false),
                now + Duration::from_secs(13)
            )
            .is_empty()
    );
    assert_eq!(
        tracker
            .observe(sample(k, 3500, false), now + Duration::from_secs(14))
            .len(),
        1
    );
    let stop = tracker.stop(false).unwrap();
    assert_eq!(stop.position_ms, 3500);
    assert_eq!(stop.state, State::Stopped);
    assert!(!stop.continuing);
    assert!(tracker.stop(false).is_none());
}

#[test]
fn handoffs_duplicate_occurrences_and_render_replacements_keep_their_own_attribution() {
    for change in [0, 1, 2, 3] {
        let mut tracker = Tracker::default();
        let old = key();
        let now = Instant::now();
        let first = tracker
            .observe(sample(old.clone(), 9990, false), now)
            .remove(0);
        let mut next = old.clone();
        match change {
            0 => next.occurrence = OccurrenceId::new(),
            1 => next.generation += 1,
            2 => next.track.source = SourceId::new(),
            _ => next.session = SessionId::new(),
        }
        assert!(
            tracker
                .observe(sample(next.clone(), 9990, true), now)
                .is_empty()
        );
        let handoff = tracker.observe(
            sample(next.clone(), 20, false),
            now + Duration::from_millis(1),
        );
        assert_eq!(handoff.len(), 2);
        assert_eq!(handoff[0].session, first.session);
        assert_eq!(handoff[0].track, old.track);
        assert_eq!(handoff[0].position_ms, 9990);
        assert!(handoff[0].continuing);
        assert_eq!(handoff[1].track, next.track);
        assert_eq!(handoff[1].position_ms, 20);
        assert_ne!(handoff[1].session, first.session);
    }
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<Report>>,
    mode: AtomicU8,
    active: AtomicUsize,
}
struct Flight<'a>(&'a AtomicUsize);
impl Drop for Flight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl MusicProvider for Fake {
    fn libraries(&self) -> Vec<Library> {
        Vec::new()
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
            assert_eq!(
                self.active.fetch_add(1, Ordering::SeqCst),
                0,
                "serialized provider worker"
            );
            let _flight = Flight(&self.active);
            self.calls.lock().unwrap().push(report);
            match self.mode.load(Ordering::SeqCst) {
                1 => Err(ProviderError::Network),
                2 => std::future::pending().await,
                3 => Ok(Outcome::ServerTermination),
                _ => Ok(Outcome::Accepted),
            }
        })
    }
}
async fn pump() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}
fn enabled(fake: Arc<Fake>) -> Controller {
    let mut controller = Controller::new();
    controller.bind(1, Some(fake));
    controller
}

#[tokio::test]
async fn default_controller_reports_only_after_binding_and_a_positive_clock() {
    let fake = Arc::new(Fake::default());
    let mut controller = Controller::new();
    let k = key();
    let now = Instant::now();
    controller.observe(Observation(Some(sample(k.clone(), 1250, false))), now);
    assert_eq!(controller.view()["state"], "disconnected");
    controller.bind(1, Some(fake.clone()));
    controller.observe(Observation(None), now);
    controller.observe(Observation(Some(sample(k.clone(), 1250, true))), now);
    pump().await;
    assert!(fake.calls.lock().unwrap().is_empty());
    controller.observe(Observation(Some(sample(k, 1250, false))), now);
    pump().await;
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    assert_eq!(controller.view()["state"], "accepted");
}

#[tokio::test]
async fn stale_accounts_are_ignored_and_signout_cancels_inflight_and_pending_reports() {
    let fake = Arc::new(Fake::default());
    fake.mode.store(2, Ordering::SeqCst);
    let mut controller = enabled(fake.clone());
    let k = key();
    let now = Instant::now();
    let mut stale = k.clone();
    stale.account = 0;
    controller.observe(Observation(Some(sample(stale, 9000, false))), now);
    pump().await;
    assert!(fake.calls.lock().unwrap().is_empty());
    controller.observe(Observation(Some(sample(k.clone(), 1250, false))), now);
    pump().await;
    assert_eq!(fake.active.load(Ordering::SeqCst), 1);
    controller.observe(Observation(Some(sample(k, 9000, true))), now);
    controller.bind(2, None);
    pump().await;
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    assert!(controller.tracker.owner.is_none());
    assert_eq!(controller.view()["state"], "disconnected");
    fake.mode.store(0, Ordering::SeqCst);
    controller.bind(2, Some(fake.clone()));
    let mut k = key();
    k.account = 2;
    controller.observe(Observation(Some(sample(k, 20, false))), now);
    pump().await;
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
    assert_eq!(fake.calls.lock().unwrap()[1].position_ms, 20);
}

#[tokio::test]
async fn render_fence_cancels_stale_positions_before_successor_confirmation() {
    let fake = Arc::new(Fake::default());
    fake.mode.store(2, Ordering::SeqCst);
    let mut controller = enabled(fake.clone());
    let k = key();
    let now = Instant::now();
    controller.fence_generation(7);
    controller.observe(Observation(Some(sample(k.clone(), 1250, false))), now);
    pump().await;
    controller.observe(
        Observation(Some(sample(k.clone(), 1350, false))),
        now + Duration::from_secs(10),
    );
    controller.fence_generation(8);
    controller.observe(Observation(None), now);
    pump().await;
    controller.observe(
        Observation(Some(sample(k.clone(), 9000, false))),
        now + Duration::from_secs(20),
    );
    pump().await;
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    fake.mode.store(0, Ordering::SeqCst);
    let mut next = k.clone();
    next.generation = 8;
    controller.observe(Observation(Some(sample(next, 20, false))), now);
    pump().await;
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1].state, State::Stopped);
    assert_eq!(calls[1].position_ms, 1350);
    assert_eq!(calls[2].state, State::Playing);
    assert_eq!(calls[2].position_ms, 20);
}

#[tokio::test(start_paused = true)]
async fn cancellation_and_deadline_also_fence_work_waiting_for_the_serial_lock() {
    let fake = Arc::new(Fake::default());
    let mut controller = enabled(fake.clone());
    let now = Instant::now();
    let serial = controller.serial.clone();
    let guard = serial.lock().await;
    controller.observe(Observation(Some(sample(key(), 1250, false))), now);
    pump().await;
    assert!(fake.calls.lock().unwrap().is_empty());
    controller.bind(2, Some(fake.clone()));
    let mut next = key();
    next.account = 2;
    controller.observe(Observation(Some(sample(next.clone(), 20, false))), now);
    pump().await;
    drop(guard);
    pump().await;
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    assert_eq!(fake.calls.lock().unwrap()[0].position_ms, 20);
    let guard = serial.lock().await;
    controller.observe(
        Observation(Some(sample(next, 30, false))),
        now + Duration::from_secs(10),
    );
    pump().await;
    tokio::time::advance(Duration::from_secs(3)).await;
    pump().await;
    assert_eq!(controller.view()["state"], "unavailable");
    drop(guard);
    pump().await;
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn queued_handoff_keeps_terminal_predecessor_and_successor_positions_in_order() {
    let fake = Arc::new(Fake::default());
    fake.mode.store(2, Ordering::SeqCst);
    let mut controller = enabled(fake.clone());
    let old = key();
    let now = Instant::now();
    controller.observe(Observation(Some(sample(old.clone(), 9000, false))), now);
    pump().await;
    controller.observe(Observation(Some(sample(old.clone(), 9999, true))), now);
    let mut next = key();
    next.track.source = SourceId::new();
    controller.observe(Observation(Some(sample(next.clone(), 20, false))), now);
    fake.mode.store(0, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(3)).await;
    pump().await;
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[1].state, State::Paused);
    assert_eq!(calls[1].position_ms, 9000);
    assert_eq!(calls[2].track, old.track);
    assert_eq!(calls[2].state, State::Stopped);
    assert_eq!(calls[2].position_ms, 9000);
    assert!(calls[2].continuing);
    assert_eq!(calls[3].track, next.track);
    assert_eq!(calls[3].state, State::Playing);
    assert_eq!(calls[3].position_ms, 20);
    assert_ne!(calls[2].session, calls[3].session);
}

#[tokio::test(start_paused = true)]
async fn timeout_is_bounded_and_periodic_work_coalesces_without_retry() {
    let fake = Arc::new(Fake::default());
    fake.mode.store(2, Ordering::SeqCst);
    let mut controller = enabled(fake.clone());
    let k = key();
    let now = Instant::now();
    controller.observe(Observation(Some(sample(k.clone(), 1250, false))), now);
    pump().await;
    for i in 1..50 {
        controller.observe(
            Observation(Some(sample(k.clone(), 1250 + i, false))),
            now + Duration::from_secs(i * 10),
        );
    }
    assert_eq!(
        controller
            .dispatch
            .as_ref()
            .unwrap()
            .mailbox
            .lock()
            .unwrap()
            .reports
            .len(),
        1
    );
    tokio::time::advance(Duration::from_secs(3)).await;
    pump().await;
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
    assert_eq!(fake.calls.lock().unwrap()[1].position_ms, 1299);
    tokio::time::advance(Duration::from_secs(3)).await;
    pump().await;
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    assert_eq!(controller.view()["state"], "unavailable");
}

#[tokio::test]
async fn overflow_suspends_reporting_and_discards_owned_work_without_touching_the_queue() {
    let fake = Arc::new(Fake::default());
    fake.mode.store(2, Ordering::SeqCst);
    let mut controller = enabled(fake.clone());
    let k = key();
    let now = Instant::now();
    controller.observe(Observation(Some(sample(k.clone(), 1250, false))), now);
    pump().await;
    for i in 1..=10 {
        controller.observe(
            Observation(Some(sample(k.clone(), 1250, i % 2 == 1))),
            now + Duration::from_secs(i),
        );
    }
    pump().await;
    assert_eq!(controller.view()["state"], "suspended");
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_eof_unknown_clock_and_unacknowledged_stop_never_fabricate_terminal_reports() {
    let fake = Arc::new(Fake::default());
    let mut controller = enabled(fake.clone());
    let now = Instant::now();
    controller.observe(Observation(Some(sample(key(), 1250, false))), now);
    pump().await;
    controller.observe(Observation(None), now);
    let mut a = audio();
    a.idle = true;
    a.natural_eof = true;
    controller.observe_stop(&a);
    controller.request_stop();
    // A still-playing sample before native acknowledgement cannot cancel stop.
    controller.observe(
        Observation(Some(sample(
            controller.tracker.owner.as_ref().unwrap().key.clone(),
            1300,
            false,
        ))),
        now,
    );
    controller.observe_stop(&a);
    pump().await;
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    a.natural_eof = false;
    a.generation = 8;
    controller.observe_stop(&a);
    pump().await;
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    a.generation = 7;
    controller.observe_stop(&a);
    pump().await;
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].state, State::Stopped);
    assert_eq!(calls[1].position_ms, 1300);
}

#[tokio::test]
async fn server_termination_and_errors_are_separate_sanitized_reporting_status() {
    for (mode, status) in [(1, "unavailable"), (3, "server_termination")] {
        let fake = Arc::new(Fake::default());
        fake.mode.store(mode, Ordering::SeqCst);
        let mut controller = enabled(fake.clone());
        let k = key();
        let now = Instant::now();
        controller.observe(Observation(Some(sample(k.clone(), 1250, false))), now);
        pump().await;
        assert_eq!(controller.view(), serde_json::json!({"state":status}));
        if mode == 3 {
            controller.observe(
                Observation(Some(sample(k, 1300, false))),
                now + Duration::from_secs(10),
            );
            pump().await;
            assert_eq!(fake.calls.lock().unwrap().len(), 1);
        }
    }
}
