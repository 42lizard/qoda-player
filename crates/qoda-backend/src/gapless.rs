//! One speculative successor, bound to queue occurrence, media version and render epoch.
use crate::{
    player::{Player, Removal, StageResult, Status},
    proxy::Proxy,
    queue::Queue,
};
use qoda_core::{OccurrenceId, SessionId, TrackRef};
use qoda_provider::{MusicProvider, ProviderError, Track};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::mpsc;
#[derive(Default)]
pub struct Epochs {
    pub current: u64,
    serial: u64,
}
impl Epochs {
    pub fn allocate(&mut self) -> Result<u64, &'static str> {
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or("Render epochs exhausted")?;
        Ok(self.serial)
    }
    pub fn reset(&mut self) -> Result<(), &'static str> {
        self.current = self.allocate()?;
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    session: SessionId,
    owner: OccurrenceId,
    next: OccurrenceId,
    owner_gen: u64,
    next_gen: u64,
    reference: TrackRef,
}
pub struct Reply {
    key: Key,
    media: Result<crate::gain::Prepared, crate::gain::PrepareError>,
}
struct Plan {
    key: Key,
    prepared: Option<(Track, String, String)>,
}
#[derive(Serialize)]
pub struct View {
    pub enabled: bool,
    pub phase: &'static str,
    pub message: String,
}
impl Default for View {
    fn default() -> Self {
        Self{enabled:true,phase:"idle",message:"Gapless chaining enabled for compatible audio; device continuity remains unverified.".into()}
    }
}
#[derive(Default)]
pub struct Controller {
    pub view: View,
    plan: Option<Plan>,
    task: Option<tokio::task::JoinHandle<()>>,
    failed: Option<(u64, OccurrenceId)>,
    pub chained: bool,
}
fn successor(queue: &Queue) -> Result<Option<(OccurrenceId, Track, Queue)>, String> {
    let mut next = queue.clone();
    let Some(track) = next.complete(true).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let id = next.selected().ok_or("Missing successor occurrence")?;
    Ok(Some((id, track, next)))
}
impl Controller {
    pub fn invalidate(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.plan = None;
        self.failed = None;
        self.chained = false;
        if self.view.phase != "review" {
            self.view.phase = if self.view.enabled { "idle" } else { "off" };
        }
    }
    pub fn retry(&mut self) {
        self.failed = None;
    }
    pub fn review(&mut self) {
        self.view.phase = "review";
        self.view.message="Playback paused after a buffered transition reset. Earlier presentation is uncertain; review the selection and choose Resume or Play.".into();
    }
    pub fn observed(&mut self, audio: &Status, confirmed: Option<u64>) {
        if self.view.phase == "transition" && confirmed == Some(audio.generation) {
            self.view.phase = "idle";
            self.view.message = "Nominal clock confirmed for the new occurrence.".into();
        }
    }
    fn matches(&self, key: &Key, queue: &Queue, epoch: u64) -> bool {
        let q = queue.snapshot();
        key.owner_gen == epoch
            && key.session == q.session
            && q.selected == Some(key.owner)
            && successor(queue)
                .ok()
                .flatten()
                .is_some_and(|(id, track, _)| id == key.next && track.reference == key.reference)
    }
    pub fn schedule(
        &mut self,
        queue: &Queue,
        epochs: &mut Epochs,
        provider: Arc<dyn MusicProvider>,
        tx: mpsc::Sender<crate::Event>,
        mode: crate::gain::Mode,
        fidelity: bool,
    ) -> Result<(), String> {
        if !self.view.enabled || self.plan.is_some() {
            return Ok(());
        }
        let q = queue.snapshot();
        let Some(owner) = q.selected else {
            return Ok(());
        };
        let Some((next, track, _)) = successor(queue)? else {
            return Ok(());
        };
        if self.failed == Some((epochs.current, next)) {
            return Ok(());
        }
        let key = Key {
            session: q.session,
            owner,
            next,
            owner_gen: epochs.current,
            next_gen: epochs.allocate().map_err(str::to_owned)?,
            reference: track.reference,
        };
        self.plan = Some(Plan {
            key: key.clone(),
            prepared: None,
        });
        self.view.phase = "preparing";
        self.view.message = "Preparing one next occurrence without changing the queue.".into();
        let album_session = queue.album_gain_eligible();
        self.task = Some(tokio::spawn(async move {
            let media = tokio::time::timeout(
                std::time::Duration::from_secs(20),
                crate::gain::prepare(
                    provider.as_ref(),
                    crate::gain::Request {
                        reference: key.reference.clone(),
                        mode,
                        album_session,
                        fidelity,
                    },
                ),
            )
            .await
            .unwrap_or(Err(crate::gain::PrepareError::Provider(
                ProviderError::Network,
            )));
            let _ = tx.send(crate::Event::Gapless(Reply { key, media })).await;
        }));
        Ok(())
    }
    pub fn unavailable_reply(
        &self,
        reply: &Reply,
        queue: &Queue,
        epochs: &Epochs,
    ) -> Option<(OccurrenceId, crate::queue::Unavailable)> {
        if !self.plan.as_ref().is_some_and(|p| p.key == reply.key)
            || !self.matches(&reply.key, queue, epochs.current)
        {
            return None;
        }
        crate::queue::Unavailable::from_provider(reply.media.as_ref().err().copied()?.provider()?)
            .map(|reason| (reply.key.next, reason))
    }
    pub async fn ready(
        &mut self,
        reply: Reply,
        queue: &Queue,
        epochs: &Epochs,
        player: &Player,
        proxy: &Proxy,
    ) -> Result<(), String> {
        if !self.plan.as_ref().is_some_and(|p| p.key == reply.key)
            || !self.matches(&reply.key, queue, epochs.current)
        {
            return Ok(());
        }
        self.task = None;
        let prepared = match reply.media {
            Ok(media) => media,
            Err(error) => {
                self.failed = Some((reply.key.owner_gen, reply.key.next));
                self.plan = None;
                self.view.phase = "fallback";
                self.view.message = format!(
                    "Next-file preparation failed; the current track is unaffected. {error}"
                );
                return Ok(());
            }
        };
        let crate::gain::Prepared {
            media,
            gain,
            source,
        } = prepared;
        let mut candidate = successor(queue)?.ok_or("The natural successor changed")?.2;
        if let Err(error) = candidate.update_selected(media.track.clone(), &media.version) {
            self.failed = Some((reply.key.owner_gen, reply.key.next));
            self.plan = None;
            self.view.phase = "fallback";
            self.view.message = error.to_string();
            return Ok(());
        }
        let url = proxy
            .grant_next(media.stream)
            .await
            .map_err(str::to_owned)?;
        self.plan.as_mut().unwrap().prepared = Some((media.track, media.version, url.clone()));
        match player
            .stage_gain(
                reply.key.owner_gen,
                reply.key.next_gen,
                url.clone(),
                gain,
                source,
            )
            .await
        {
            Ok(StageResult::Ready) => {
                self.view.phase = "staged";
                self.view.message =
                    "One successor is staged. Compatible formats can share the audio output."
                        .into();
                Ok(())
            }
            Ok(StageResult::MissedWindow) => {
                proxy.revoke_url(&url).await;
                self.failed = Some((reply.key.owner_gen, reply.key.next));
                self.plan = None;
                self.view.phase = "fallback";
                self.view.message =
                    "The successor missed its native preload window; normal loading will continue."
                        .into();
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }
    pub async fn handoff(
        &mut self,
        queue: &mut Queue,
        epochs: &mut Epochs,
        audio: &Status,
        proxy: &Proxy,
    ) -> Result<bool, String> {
        let Some(plan) = &self.plan else {
            return Ok(false);
        };
        if audio.generation != plan.key.next_gen || audio.handoff_from != Some(plan.key.owner_gen) {
            return Ok(false);
        }
        if !self.matches(&plan.key, queue, epochs.current) {
            return Err("The native transition no longer matches the queue".into());
        }
        // A failed current grant cannot be consumed as natural completion just
        // because the decoder already selected its prefetched successor.
        if proxy.failed().await {
            return Ok(false);
        }
        let (track, version, url) = plan
            .prepared
            .as_ref()
            .ok_or("The native transition has no prepared binding")?;
        let mut next = successor(queue)?.ok_or("The successor disappeared")?.2;
        next.update_selected(track.clone(), version)
            .map_err(|e| e.to_string())?;
        proxy.promote(url).await.map_err(str::to_owned)?;
        *queue = next;
        epochs.current = plan.key.next_gen;
        self.plan = None;
        self.failed = None;
        self.chained = true;
        self.view.phase = "transition";
        self.view.message =
            "Engine selection advanced; presentation requires a fresh nominal clock.".into();
        Ok(true)
    }
    pub async fn cancel(
        &mut self,
        player: &Player,
        proxy: &Proxy,
    ) -> Result<Option<Status>, String> {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let Some(plan) = &self.plan else {
            return Ok(None);
        };
        let Some((_, _, url)) = &plan.prepared else {
            self.plan = None;
            self.failed = None;
            self.view.phase = if self.view.enabled { "idle" } else { "off" };
            return Ok(None);
        };
        match player
            .unstage(plan.key.owner_gen, plan.key.next_gen)
            .await
            .map_err(str::to_owned)?
        {
            Removal::Untouched => {
                proxy.revoke_url(url).await;
                self.plan = None;
                self.failed = None;
                self.view.phase = if self.view.enabled { "idle" } else { "off" };
                Ok(None)
            }
            Removal::Reset(status) => Ok(Some(status)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Command;
    use futures_util::StreamExt;
    use qoda_core::{Counter, ProviderTrackKey, SourceId};
    use qoda_provider::{
        ByteRange, CatalogPage, Library, MediaResponse, MediaSource, PreparedMedia, ProviderFuture,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    pub(super) struct Audio(pub(super) bytes::Bytes);
    impl MediaSource for Audio {
        fn open(&self, range: Option<ByteRange>, head: bool) -> ProviderFuture<'_, MediaResponse> {
            Box::pin(async move {
                let size = self.0.len();
                let (mut start, mut end) = (0, size - 1);
                if let Some(range) = &range {
                    let (a, b) = range
                        .header()
                        .strip_prefix("bytes=")
                        .unwrap()
                        .split_once('-')
                        .unwrap();
                    if a.is_empty() {
                        start = size.saturating_sub(b.parse::<usize>().unwrap());
                    } else {
                        start = a.parse().unwrap();
                        if !b.is_empty() {
                            end = b.parse::<usize>().unwrap().min(size - 1);
                        }
                    }
                }
                if start >= size || start > end {
                    return Err(ProviderError::InvalidRange);
                }
                let body = if head {
                    bytes::Bytes::new()
                } else {
                    self.0.slice(start..=end)
                };
                Ok(MediaResponse {
                    status: if range.is_some() { 206 } else { 200 },
                    content_type: Some("audio/wav".into()),
                    content_length: Some((end - start + 1) as u64),
                    content_range: range.map(|_| format!("bytes {start}-{end}/{size}")),
                    etag: None,
                    body: futures_util::stream::once(async move { Ok(body) }).boxed(),
                })
            })
        }
    }
    pub(super) struct Provider {
        pub(super) source: SourceId,
        calls: AtomicUsize,
        fail: bool,
    }
    impl Provider {
        pub(super) fn new() -> Arc<Self> {
            Arc::new(Self {
                source: SourceId::new(),
                calls: AtomicUsize::new(0),
                fail: false,
            })
        }
        pub(super) fn track(&self, key: &str) -> Track {
            Track {
                reference: TrackRef {
                    source: self.source,
                    key: key.to_owned().try_into().unwrap(),
                },
                title: key.into(),
                artist: "Synthetic fixture".into(),
                album: "Synthetic".into(),
                duration_ms: Some(Counter::new(1000)),
            }
        }
        pub(super) fn queue(&self) -> Queue {
            let mut q = Queue::default();
            q.replace_many(vec![self.track("a"), self.track("b"), self.track("c")])
                .unwrap();
            q
        }
    }
    pub(super) fn wav() -> bytes::Bytes {
        let pcm = vec![0_u8; 96000];
        let mut data = Vec::new();
        data.extend(b"RIFF");
        data.extend((36 + pcm.len() as u32).to_le_bytes());
        data.extend(b"WAVEfmt ");
        data.extend(16_u32.to_le_bytes());
        data.extend(1_u16.to_le_bytes());
        data.extend(1_u16.to_le_bytes());
        data.extend(48000_u32.to_le_bytes());
        data.extend(96000_u32.to_le_bytes());
        data.extend(2_u16.to_le_bytes());
        data.extend(16_u16.to_le_bytes());
        data.extend(b"data");
        data.extend((pcm.len() as u32).to_le_bytes());
        data.extend(pcm);
        data.into()
    }
    impl MusicProvider for Provider {
        fn libraries(&self) -> Vec<Library> {
            vec![]
        }
        fn tracks(&self, _: SourceId, _: u64, _: String) -> ProviderFuture<'_, CatalogPage> {
            Box::pin(async { Err(ProviderError::Unsupported) })
        }
        fn prepare(
            &self,
            source: SourceId,
            key: ProviderTrackKey,
        ) -> ProviderFuture<'_, PreparedMedia> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if self.fail {
                    return Err(ProviderError::Network);
                }
                if source != self.source {
                    return Err(ProviderError::UnknownSource);
                }
                let key = String::from(key);
                Ok(PreparedMedia {
                    track: self.track(&key),
                    version: format!("fixture:{key}"),
                    stream: Box::new(Audio(wav())),
                })
            })
        }
    }
    pub(super) async fn wait(player: &Player, predicate: impl Fn(&Status) -> bool) -> Status {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let status = player.status();
            if predicate(&status) {
                return status;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "Gapless pipeline timed out: {status:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
    #[tokio::test]
    async fn failed_current_stream_does_not_consume_a_prefetched_successor() {
        struct Interrupted;
        impl MediaSource for Interrupted {
            fn open(&self, _: Option<ByteRange>, _: bool) -> ProviderFuture<'_, MediaResponse> {
                Box::pin(async { Err(ProviderError::Network) })
            }
        }
        let provider = Provider::new();
        let mut queue = provider.queue();
        let before = serde_json::to_value(&queue).unwrap();
        let (next, track, _) = successor(&queue).unwrap().unwrap();
        let proxy = Proxy::start().await.unwrap();
        let current = proxy.grant(Box::new(Interrupted)).await;
        let url = proxy.grant_next(Box::new(Audio(wav()))).await.unwrap();
        assert_eq!(reqwest::get(current).await.unwrap().status(), 502);
        let mut epochs = Epochs::default();
        epochs.reset().unwrap();
        let next_gen = epochs.allocate().unwrap();
        let mut controller = Controller {
            plan: Some(Plan {
                key: Key {
                    session: queue.snapshot().session,
                    owner: queue.selected().unwrap(),
                    next,
                    owner_gen: epochs.current,
                    next_gen,
                    reference: track.reference.clone(),
                },
                prepared: Some((track, "v1".into(), url)),
            }),
            ..Default::default()
        };
        let audio = Status {
            generation: next_gen,
            handoff_from: Some(epochs.current),
            ..Default::default()
        };
        assert!(
            !controller
                .handoff(&mut queue, &mut epochs, &audio, &proxy)
                .await
                .unwrap()
        );
        assert_eq!(serde_json::to_value(&queue).unwrap(), before);
        assert_eq!(epochs.current, 1);
        assert!(proxy.failed().await);
        proxy.revoke().await;
    }
    #[test]
    fn speculative_successor_is_stable_and_does_not_mutate_duplicates_modes_or_priority() {
        let provider = Provider::new();
        let mut q = provider.queue();
        let before = q.snapshot();
        let first = successor(&q).unwrap().unwrap();
        assert_eq!(first.0, q.pending()[0]);
        assert_eq!(q.selected(), before.selected);
        assert_eq!(q.snapshot().revision, before.revision);
        q.set_repeat(crate::queue::Repeat::One).unwrap();
        assert_eq!(successor(&q).unwrap().unwrap().0, q.selected().unwrap());
        q.set_repeat(crate::queue::Repeat::Off).unwrap();
        let urgent = q.enqueue(provider.track("urgent"), true).unwrap();
        assert_eq!(successor(&q).unwrap().unwrap().0, urgent);
        q.set_shuffle(true).unwrap();
        assert_eq!(
            successor(&q).unwrap().unwrap().0,
            successor(&q).unwrap().unwrap().0
        );
        let duplicate = q.enqueue(provider.track("b"), true).unwrap();
        assert_ne!(duplicate, first.0);
        assert_eq!(q.snapshot().membership_count, 5);
    }
    #[test]
    fn reserved_future_epochs_are_never_reused_by_resets_and_do_not_wrap() {
        let mut epochs = Epochs::default();
        epochs.reset().unwrap();
        assert_eq!(epochs.current, 1);
        assert_eq!(epochs.allocate().unwrap(), 2);
        assert_eq!(epochs.current, 1);
        epochs.reset().unwrap();
        assert_eq!(epochs.current, 3);
        assert_eq!(epochs.allocate().unwrap(), 4);
        epochs.serial = u64::MAX;
        assert!(epochs.allocate().is_err());
        assert_eq!(epochs.current, 3);
    }
    #[tokio::test]
    async fn metadata_preparation_failure_does_not_select_or_consume_the_successor_or_retry_forever()
     {
        let provider = Arc::new(Provider {
            source: SourceId::new(),
            calls: AtomicUsize::new(0),
            fail: true,
        });
        let q = provider.queue();
        let before = q.snapshot();
        let mut epochs = Epochs::default();
        epochs.reset().unwrap();
        let mut controller = Controller::default();
        let (tx, mut rx) = mpsc::channel(4);
        controller
            .schedule(
                &q,
                &mut epochs,
                provider.clone(),
                tx.clone(),
                crate::gain::Mode::Off,
                false,
            )
            .unwrap();
        let Some(crate::Event::Gapless(reply)) = rx.recv().await else {
            panic!("No speculative reply");
        };
        // Failure is handled before touching native playback; dummy objects stay idle/null.
        let player = Player::test_output("null").unwrap();
        let proxy = Proxy::start().await.unwrap();
        controller
            .ready(reply, &q, &epochs, &player, &proxy)
            .await
            .unwrap();
        assert_eq!(controller.view.phase, "fallback");
        controller
            .schedule(
                &q,
                &mut epochs,
                provider.clone(),
                tx,
                crate::gain::Mode::Off,
                false,
            )
            .unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(q.snapshot().revision, before.revision);
        assert_eq!(q.selected(), before.selected);
        assert!(player.status().idle);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_http_handoff_commits_the_bound_occurrence_once_without_a_replacement_load() {
        let provider = Provider::new();
        let mut q = provider.queue();
        let first = q.selected().unwrap();
        let next = q.pending()[0];
        let mut epochs = Epochs::default();
        epochs.reset().unwrap();
        let mut controller = Controller::default();
        let proxy = Proxy::start().await.unwrap();
        let player = Player::test_output("null").unwrap();
        let media = provider
            .prepare(provider.source, "a".to_owned().try_into().unwrap())
            .await
            .unwrap();
        q.update_selected(media.track, &media.version).unwrap();
        let first_url = proxy.grant(media.stream).await;
        player.send(Command::Load(epochs.current, first_url.clone(), 0.0));
        player.send(Command::Pause(false));
        let audio = wait(&player, |s| s.position_seconds.is_some_and(|p| p > 0.0)).await;
        let mut presented = None;
        crate::record_clock(&mut q, &audio, epochs.current, &mut presented, false);
        let (tx, mut rx) = mpsc::channel(4);
        controller
            .schedule(
                &q,
                &mut epochs,
                provider.clone(),
                tx,
                crate::gain::Mode::Off,
                false,
            )
            .unwrap();
        let Some(crate::Event::Gapless(reply)) = rx.recv().await else {
            panic!("No future reply");
        };
        controller
            .ready(reply, &q, &epochs, &player, &proxy)
            .await
            .unwrap();
        assert_eq!(controller.view.phase, "staged");
        let audio = wait(&player, |s| s.handoff_from == Some(1)).await;
        assert!(
            controller
                .handoff(&mut q, &mut epochs, &audio, &proxy)
                .await
                .unwrap()
        );
        assert_eq!(q.selected(), Some(next));
        assert_ne!(q.selected(), Some(first));
        assert_eq!(epochs.current, 2);
        assert_eq!(reqwest::get(first_url).await.unwrap().status(), 404);
        assert!(
            !controller
                .handoff(&mut q, &mut epochs, &audio, &proxy)
                .await
                .unwrap()
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        if audio.position_seconds.is_none() {
            assert!(
                q.confirm_presented().unwrap(),
                "The handoff invented a listening observation"
            );
        }
        player.send(Command::Pause(true));
        player.barrier().await.unwrap();
    }
    #[tokio::test]
    async fn only_current_individual_successor_failures_can_mark_an_occurrence() {
        let provider = Provider::new();
        let mut q = provider.queue();
        let mut epochs = Epochs::default();
        epochs.reset().unwrap();
        let mut controller = Controller::default();
        let (tx, mut rx) = mpsc::channel(4);
        controller
            .schedule(&q, &mut epochs, provider, tx, crate::gain::Mode::Off, false)
            .unwrap();
        let Some(crate::Event::Gapless(mut reply)) = rx.recv().await else {
            panic!("No preparation reply")
        };
        reply.media = Err(crate::gain::PrepareError::Provider(ProviderError::Network));
        assert!(controller.unavailable_reply(&reply, &q, &epochs).is_none());
        reply.media = Err(crate::gain::PrepareError::Provider(
            ProviderError::MissingTrack,
        ));
        let (id, reason) = controller.unavailable_reply(&reply, &q, &epochs).unwrap();
        assert_eq!(id, q.pending()[0]);
        assert_eq!(reason, crate::queue::Unavailable::Missing);
        q.remove(id).unwrap();
        assert!(controller.unavailable_reply(&reply, &q, &epochs).is_none());
        controller.invalidate();
        assert!(controller.unavailable_reply(&reply, &q, &epochs).is_none());
    }
    #[tokio::test]
    async fn stale_preparation_and_changed_successor_cannot_promote_an_old_native_binding() {
        let provider = Provider::new();
        let mut q = provider.queue();
        let mut epochs = Epochs::default();
        epochs.reset().unwrap();
        let next = q.pending()[0];
        let mut controller = Controller::default();
        let (tx, mut rx) = mpsc::channel(4);
        controller
            .schedule(
                &q,
                &mut epochs,
                provider.clone(),
                tx,
                crate::gain::Mode::Off,
                false,
            )
            .unwrap();
        let Some(crate::Event::Gapless(reply)) = rx.recv().await else {
            panic!("No reply");
        };
        controller.invalidate();
        let player = Player::test_output("null").unwrap();
        let proxy = Proxy::start().await.unwrap();
        controller
            .ready(reply, &q, &epochs, &player, &proxy)
            .await
            .unwrap();
        assert!(controller.plan.is_none());
        let key = Key {
            session: q.snapshot().session,
            owner: q.selected().unwrap(),
            next,
            owner_gen: 1,
            next_gen: 2,
            reference: provider.track("b").reference,
        };
        controller.plan = Some(Plan {
            key,
            prepared: Some((
                provider.track("b"),
                "fixture:b".into(),
                "not-a-grant".into(),
            )),
        });
        q.remove(next).unwrap();
        let audio = Status {
            generation: 2,
            handoff_from: Some(1),
            ..Default::default()
        };
        assert!(
            controller
                .handoff(&mut q, &mut epochs, &audio, &proxy)
                .await
                .is_err()
        );
        assert_eq!(epochs.current, 1);
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::tests::{Audio, Provider, wait, wav};
    use super::*;
    use crate::player::Command;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn buffered_reset_promotes_observed_selection_then_rejects_the_old_ui_revision_and_remains_paused()
     {
        let provider = Provider::new();
        let mut queue = provider.queue();
        let mut epochs = Epochs::default();
        epochs.reset().unwrap();
        let mut controller = Controller::default();
        let proxy = Proxy::start().await.unwrap();
        let player = Player::test_output("null").unwrap();
        let mut presented = None;
        let media = provider
            .prepare(provider.source, "a".to_owned().try_into().unwrap())
            .await
            .unwrap();
        queue.update_selected(media.track, &media.version).unwrap();
        let url = proxy.grant(media.stream).await;
        player.send(Command::Load(1, url, 0.0));
        player.send(Command::Pause(false));
        let first = wait(&player, |s| s.position_seconds.is_some_and(|p| p > 0.0)).await;
        crate::record_clock(&mut queue, &first, 1, &mut presented, false);
        let (tx, mut rx) = mpsc::channel(4);
        controller
            .schedule(
                &queue,
                &mut epochs,
                provider.clone(),
                tx,
                crate::gain::Mode::Off,
                false,
            )
            .unwrap();
        let Some(crate::Event::Gapless(reply)) = rx.recv().await else {
            panic!("No future");
        };
        controller
            .ready(reply, &queue, &epochs, &player, &proxy)
            .await
            .unwrap();
        let stale = queue.snapshot();
        let next = queue.pending()[0];
        wait(&player, |s| s.generation == 2 && !s.idle).await;
        let Some(before) = controller.cancel(&player, &proxy).await.unwrap() else {
            panic!("Decoded future was falsely acknowledged untouched");
        };
        crate::finish_gapless_reset(
            &mut controller,
            &mut queue,
            &mut epochs,
            &before,
            &mut presented,
            &proxy,
        )
        .await
        .unwrap();
        assert_eq!(queue.selected(), Some(next));
        assert!(queue.check(stale.session, stale.revision).is_err());
        assert!(epochs.current > 2);
        assert!(player.status().idle && player.status().paused);
        assert_eq!(controller.view.phase, "review");
    }
    #[tokio::test]
    async fn an_unobserved_short_native_successor_is_consumed_once_without_inventing_credit_or_repeating_it()
     {
        let provider = Provider::new();
        let mut queue = provider.queue();
        queue.confirm_presented().unwrap();
        let mut epochs = Epochs::default();
        epochs.reset().unwrap();
        let next = queue.pending()[0];
        let last = queue.pending()[1];
        let snapshot = queue.snapshot();
        let proxy = Proxy::start().await.unwrap();
        let _current = proxy.grant(Box::new(Audio(wav()))).await;
        let url = proxy.grant_next(Box::new(Audio(wav()))).await.unwrap();
        let mut controller = Controller {
            plan: Some(Plan {
                key: Key {
                    session: snapshot.session,
                    owner: snapshot.selected.unwrap(),
                    next,
                    owner_gen: 1,
                    next_gen: epochs.allocate().unwrap(),
                    reference: provider.track("b").reference,
                },
                prepared: Some((provider.track("b"), "fixture:b".into(), url)),
            }),
            ..Default::default()
        };
        let eof = Status {
            generation: 2,
            handoff_from: Some(1),
            idle: true,
            natural_eof: true,
            ..Default::default()
        };
        assert!(
            controller
                .handoff(&mut queue, &mut epochs, &eof, &proxy)
                .await
                .unwrap()
        );
        assert_eq!(queue.selected(), Some(next));
        assert!(
            !controller
                .handoff(&mut queue, &mut epochs, &eof, &proxy)
                .await
                .unwrap()
        );
        queue.complete(false).unwrap();
        assert_eq!(queue.selected(), Some(last));
    }
}
