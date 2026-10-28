//! Bounded recovery of one pinned current occurrence. Never selects/advances it.
use crate::{gain, queue};
use qoda_core::{Counter, OccurrenceId, SessionId};
use qoda_provider::{MusicProvider, ProviderError};
use serde::Serialize;
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle};

const ATTEMPTS: u8 = 2;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub account: u64,
    pub generation: u64,
    session: SessionId,
    revision: Counter,
    occurrence: OccurrenceId,
}
impl Key {
    pub fn capture(queue: &queue::Queue, account: u64, generation: u64) -> Option<Self> {
        let (session, revision, _) = queue.durable_stamp();
        Some(Self {
            account,
            generation,
            session,
            revision,
            occurrence: queue.selected()?,
        })
    }
    pub fn matches(self, queue: &queue::Queue, account: u64, generation: u64) -> bool {
        self.account == account
            && self.generation == generation
            && queue.check(self.session, self.revision).is_ok()
            && queue.selected() == Some(self.occurrence)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Provider(ProviderError),
    Version,
    Verification(&'static str),
}
impl Failure {
    pub fn reason(self) -> Option<queue::Unavailable> {
        match self {
            Self::Provider(error) => queue::Unavailable::from_provider(error),
            Self::Version => Some(queue::Unavailable::VersionChanged),
            Self::Verification(_) => None,
        }
    }
    pub fn message(self) -> String {
        match self {
            Self::Provider(error) => error.to_string(),
            Self::Version => "Pinned media version changed; recovery refused substitution.".into(),
            Self::Verification(message) => message.into(),
        }
    }
}
pub struct Reply {
    pub key: Key,
    pub attempts: u8,
    pub result: Option<Result<gain::Prepared, Failure>>,
}
#[derive(Serialize)]
pub struct View {
    pub phase: &'static str,
    pub attempts: u8,
    pub limit: u8,
    pub message: String,
}
impl Default for View {
    fn default() -> Self {
        Self {
            phase: "idle",
            attempts: 0,
            limit: ATTEMPTS,
            message: String::new(),
        }
    }
}
#[derive(Default)]
pub struct Controller {
    pub view: View,
    active: Option<(Key, bool)>,
    owner: Option<(u64, SessionId, OccurrenceId)>,
    task: Option<JoinHandle<()>>,
}
impl Controller {
    pub fn owns(&self, key: Key) -> bool {
        self.active.is_some_and(|(active, _)| active == key)
    }
    pub fn cancel(&mut self) -> bool {
        let pending = self.active.take().is_some();
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.owner = None;
        self.view = View::default();
        pending
    }
    pub fn begin(
        &mut self,
        work: Work,
        provider: Arc<dyn MusicProvider>,
        tx: mpsc::Sender<crate::Event>,
    ) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let owner = (work.key.account, work.key.session, work.key.occurrence);
        if self.owner != Some(owner) {
            self.view.attempts = 0;
        }
        self.owner = Some(owner);
        self.active = None;
        if work.failure != ProviderError::Network || self.view.attempts >= ATTEMPTS {
            self.view.phase = "exhausted";
            self.view.message = format!(
                "{} Automatic recovery stopped. Retry or Skip explicitly.",
                work.failure
            );
            return;
        }
        self.view.phase = "recovering";
        self.view.message = "Stream interrupted; recovering the pinned selection. Playback is paused during recovery.".into();
        self.active = Some((work.key, work.playing));
        let used = self.view.attempts;
        self.task = Some(tokio::spawn(async move {
            let key = work.key;
            let result = tokio::time::timeout(
                Duration::from_secs(15),
                recover(provider.as_ref(), &work, used, &tx),
            )
            .await;
            if result.is_err() {
                let _ = tx
                    .send(crate::Event::Recovery(Reply {
                        key,
                        attempts: ATTEMPTS,
                        result: Some(Err(Failure::Provider(ProviderError::Network))),
                    }))
                    .await;
            }
        }));
    }
    pub fn accept(&mut self, reply: Reply) -> Option<(bool, Result<gain::Prepared, Failure>)> {
        let (key, playing) = self.active?;
        if reply.key != key {
            return None;
        }
        self.view.attempts = self.view.attempts.max(reply.attempts).min(ATTEMPTS);
        let result = reply.result?;
        self.active = None;
        self.task = None;
        self.view.phase = if result.is_ok() { "ready" } else { "exhausted" };
        self.view.message = match &result {
            Ok(_) => "Pinned stream refreshed at retained nominal position.".into(),
            Err(error) => format!(
                "{} Recovery stopped. Retry or Skip explicitly.",
                error.message()
            ),
        };
        Some((playing, result))
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.cancel();
    }
}
pub struct Work {
    pub position: f64,
    pub transcoded: bool,
    pub key: Key,
    pub request: gain::Request,
    pub version: String,
    pub playing: bool,
    pub failure: ProviderError,
}
async fn recover(
    provider: &dyn MusicProvider,
    work: &Work,
    used: u8,
    tx: &mpsc::Sender<crate::Event>,
) {
    for attempt in used + 1..=ATTEMPTS {
        tokio::time::sleep(Duration::from_millis(if attempt == 1 { 500 } else { 1500 })).await;
        if tx
            .send(crate::Event::Recovery(Reply {
                key: work.key,
                attempts: attempt,
                result: None,
            }))
            .await
            .is_err()
        {
            return;
        }
        let result = tokio::time::timeout(Duration::from_secs(6), async {
            let request = gain::Request {
                reference: work.request.reference.clone(),
                mode: work.request.mode,
                album_session: work.request.album_session,
                fidelity: work.request.fidelity,
            };
            if work.transcoded {
                return gain::convert(provider, request, work.version.clone(), work.position)
                    .await
                    .map_err(|error| match error {
                        gain::PrepareError::Provider(error) => Failure::Provider(error),
                        gain::PrepareError::Verification(error) => Failure::Verification(error),
                    });
            }
            let media = provider
                .prepare(
                    work.request.reference.source,
                    work.request.reference.key.clone(),
                )
                .await
                .map_err(Failure::Provider)?;
            if work.version.is_empty()
                || media.version != work.version
                || media.track.reference != work.request.reference
            {
                return Err(Failure::Version);
            }
            gain::describe(media, request)
                .await
                .map_err(|error| match error {
                    gain::PrepareError::Provider(error) => Failure::Provider(error),
                    gain::PrepareError::Verification(error) => Failure::Verification(error),
                })
        })
        .await
        .unwrap_or(Err(Failure::Provider(ProviderError::Network)));
        if matches!(result, Err(Failure::Provider(ProviderError::Network))) && attempt < ATTEMPTS {
            continue;
        }
        let _ = tx
            .send(crate::Event::Recovery(Reply {
                key: work.key,
                attempts: attempt,
                result: Some(result),
            }))
            .await;
        return;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use qoda_core::{SourceId, TrackRef};
    use qoda_provider::{CatalogPage, Library, PreparedMedia, ProviderFuture, Track};
    use std::{collections::VecDeque, sync::Mutex};
    pub(crate) enum Answer {
        Error(ProviderError),
        Version(&'static str),
        Pending,
    }
    pub(crate) struct Fixture {
        track: Track,
        answers: Mutex<VecDeque<Answer>>,
    }
    struct Silent {
        seekable: bool,
    }
    impl qoda_provider::MediaSource for Silent {
        fn open(
            &self,
            range: Option<qoda_provider::ByteRange>,
            head: bool,
        ) -> ProviderFuture<'_, qoda_provider::MediaResponse> {
            use futures_util::StreamExt;
            Box::pin(async move {
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
                let total = wav.len();
                let mut status = 200;
                let mut content_range = None;
                if self.seekable
                    && let Some(range) = range
                {
                    let (a, b) = range
                        .header()
                        .strip_prefix("bytes=")
                        .unwrap()
                        .split_once('-')
                        .unwrap();
                    let start = if a.is_empty() {
                        total.saturating_sub(b.parse().unwrap())
                    } else {
                        a.parse::<usize>().unwrap()
                    };
                    let end = if a.is_empty() || b.is_empty() {
                        total - 1
                    } else {
                        b.parse::<usize>().unwrap().min(total - 1)
                    };
                    if start >= total {
                        status = 416;
                        content_range = Some(format!("bytes */{total}"));
                        wav.clear();
                    } else {
                        status = 206;
                        content_range = Some(format!("bytes {start}-{end}/{total}"));
                        wav = wav[start..=end].to_vec();
                    }
                }
                let length = wav.len() as u64;
                if head {
                    wav.clear();
                }
                Ok(qoda_provider::MediaResponse {
                    status,
                    content_type: Some("audio/wav".into()),
                    content_length: Some(length),
                    content_range,
                    etag: None,
                    body: futures_util::stream::once(async move { Ok(bytes::Bytes::from(wav)) })
                        .boxed(),
                })
            })
        }
    }
    impl MusicProvider for Fixture {
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
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected retry");
            Box::pin(async move {
                match answer {
                    Answer::Error(error) => Err(error),
                    Answer::Pending => std::future::pending().await,
                    Answer::Version(version) => Ok(PreparedMedia {
                        track: self.track.clone(),
                        version: version.into(),
                        stream: Box::new(Silent { seekable: true }),
                    }),
                }
            })
        }
    }
    pub(crate) fn fixture(answers: Vec<Answer>) -> (queue::Queue, Arc<Fixture>) {
        let track = Track {
            reference: TrackRef {
                source: SourceId::new(),
                key: "fixture".to_owned().try_into().unwrap(),
            },
            title: "Synthetic silence".into(),
            artist: "Fixture".into(),
            album: "Fixture".into(),
            duration_ms: None,
        };
        let mut queue = queue::Queue::default();
        queue.replace(track.clone()).unwrap();
        queue.update_selected(track.clone(), "v1").unwrap();
        queue.confirm_presented().unwrap();
        queue.set_position(1.0).unwrap();
        (
            queue,
            Arc::new(Fixture {
                track,
                answers: Mutex::new(answers.into()),
            }),
        )
    }
    fn work(queue: &queue::Queue, generation: u64, playing: bool) -> Work {
        Work {
            position: queue.position(),
            transcoded: false,
            key: Key::capture(queue, 4, generation).unwrap(),
            version: queue.selected_version().unwrap().into(),
            playing,
            failure: ProviderError::Network,
            request: gain::Request {
                reference: queue
                    .visible_track(queue.selected().unwrap())
                    .unwrap()
                    .reference,
                mode: gain::Mode::Off,
                album_session: false,
                fidelity: false,
            },
        }
    }
    async fn finish(
        controller: &mut Controller,
        rx: &mut mpsc::Receiver<crate::Event>,
    ) -> (bool, Result<gain::Prepared, Failure>) {
        while let Some(crate::Event::Recovery(reply)) = rx.recv().await {
            if let Some(result) = controller.accept(reply) {
                return result;
            }
        }
        panic!("missing recovery completion")
    }
    #[tokio::test(start_paused = true)]
    async fn transient_failure_recovers_exact_version_with_budget_and_preserved_pause_intent() {
        let (queue, provider) = fixture(vec![
            Answer::Error(ProviderError::Network),
            Answer::Version("v1"),
        ]);
        let before = serde_json::to_value(&queue).unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        let mut controller = Controller::default();
        controller.begin(work(&queue, 8, false), provider.clone(), tx.clone());
        let (playing, prepared) = finish(&mut controller, &mut rx).await;
        assert!(!playing);
        assert_eq!(prepared.unwrap().media.version, "v1");
        assert_eq!(controller.view.attempts, 2);
        assert_eq!(serde_json::to_value(&queue).unwrap(), before);
        // Another immediate stream failure on this visit does not reset budget.
        controller.begin(work(&queue, 9, true), provider.clone(), tx);
        assert_eq!(controller.view.phase, "exhausted");
        assert!(provider.answers.lock().unwrap().is_empty());
    }
    #[tokio::test(start_paused = true)]
    async fn version_substitution_and_non_network_errors_are_terminal_without_advance() {
        for (answer, expected) in [
            (Answer::Version("v2"), Failure::Version),
            (
                Answer::Error(ProviderError::MissingTrack),
                Failure::Provider(ProviderError::MissingTrack),
            ),
            (
                Answer::Error(ProviderError::AccessDenied),
                Failure::Provider(ProviderError::AccessDenied),
            ),
        ] {
            let (queue, provider) = fixture(vec![answer]);
            let before = serde_json::to_value(&queue).unwrap();
            let (tx, mut rx) = mpsc::channel(16);
            let mut controller = Controller::default();
            let mut request = work(&queue, 1, true);
            request.request.fidelity = expected == Failure::Version;
            controller.begin(request, provider, tx);
            let (_, result) = finish(&mut controller, &mut rx).await;
            assert!(matches!(result, Err(error) if error == expected));
            assert_eq!(controller.view.phase, "exhausted");
            assert_eq!(controller.view.attempts, 1);
            assert_eq!(serde_json::to_value(&queue).unwrap(), before);
        }
    }
    #[tokio::test(start_paused = true)]
    async fn hung_preparations_are_bounded_and_cancelled_responses_cannot_replace_new_work() {
        let (queue, provider) = fixture(vec![Answer::Pending, Answer::Pending]);
        let (tx, mut rx) = mpsc::channel(16);
        let mut controller = Controller::default();
        let started = tokio::time::Instant::now();
        controller.begin(work(&queue, 1, true), provider.clone(), tx.clone());
        let (_, result) = finish(&mut controller, &mut rx).await;
        assert!(matches!(
            result,
            Err(Failure::Provider(ProviderError::Network))
        ));
        assert!(started.elapsed() <= Duration::from_secs(15));
        controller.cancel();
        let old = work(&queue, 2, true).key;
        controller.begin(work(&queue, 2, true), provider.clone(), tx.clone());
        assert!(controller.cancel());
        controller.begin(work(&queue, 3, false), provider, tx);
        assert!(!controller.owns(old));
        assert!(
            controller
                .accept(Reply {
                    key: old,
                    attempts: 2,
                    result: Some(Err(Failure::Version))
                })
                .is_none()
        );
        assert_eq!(controller.view.phase, "recovering");
        controller.cancel();
    }
    #[test]
    fn terminal_stream_failures_do_not_start_provider_requests() {
        for failure in [
            ProviderError::AccessDenied,
            ProviderError::SignInRequired,
            ProviderError::MissingTrack,
            ProviderError::UnsupportedMedia,
            ProviderError::InvalidReply,
        ] {
            let (queue, provider) = fixture(vec![]);
            let (tx, _) = mpsc::channel(16);
            let mut controller = Controller::default();
            let mut request = work(&queue, 1, true);
            request.failure = failure;
            controller.begin(request, provider, tx);
            assert_eq!(controller.view.phase, "exhausted");
            assert_eq!(controller.view.attempts, 0);
            assert_eq!(queue.position(), 1.0);
        }
    }
    #[test]
    fn responses_require_exact_account_session_revision_occurrence_and_generation() {
        let (mut queue, _) = fixture(vec![]);
        let key = work(&queue, 2, true).key;
        assert!(key.matches(&queue, 4, 2));
        assert!(!key.matches(&queue, 5, 2));
        assert!(!key.matches(&queue, 4, 3));
        queue.set_position(2.0).unwrap();
        assert!(key.matches(&queue, 4, 2));
        queue.set_repeat(crate::queue::Repeat::All).unwrap();
        assert!(!key.matches(&queue, 4, 2));
        let (other, _) = fixture(vec![]);
        assert!(!key.matches(&other, 4, 2));
    }
    #[tokio::test]
    async fn a_source_ignoring_resume_seeks_stays_paused_instead_of_restarting_from_zero() {
        let proxy = crate::proxy::Proxy::start().await.unwrap();
        let url = proxy.grant(Box::new(Silent { seekable: false })).await;
        let player = crate::player::Player::test_output("null").unwrap();
        player
            .load_prepared(crate::player::Load {
                timeline_offset: 0.0,
                transcoded: false,
                generation: 1,
                url,
                position: 1.0,
                gain: gain::Applied::off(),
                source: None,
                description: None,
                chain: false,
                output: Some(crate::output::Selection::Desktop),
                playing: true,
            })
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(7);
        loop {
            let audio = player.status();
            assert!(audio.paused, "Unverified resume unpaused: {audio:?}");
            assert!(audio.position_seconds.is_none());
            if let Some(error) = audio.error {
                assert!(error.contains("Resume position"));
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "{audio:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!proxy.failed().await);
        proxy.revoke().await;
    }
}
