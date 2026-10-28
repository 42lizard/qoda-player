//! Selected preparation and pinned recovery share one owned completion path.
use crate::{
    Event, gain, gapless, output,
    player::{self, Command, Player},
    proxy, queue, recovery,
};
use qoda_core::{OccurrenceId, SessionId};
use qoda_provider::{MusicProvider, ProviderError, Track};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Clone, Copy)]
struct Selection {
    account: u64,
    generation: u64,
    session: SessionId,
    occurrence: OccurrenceId,
}
impl Selection {
    fn capture(cx: &Runtime<'_>) -> Option<Self> {
        Some(Self {
            account: cx.account,
            generation: cx.epochs.current,
            session: cx.queue.durable_stamp().0,
            occurrence: cx.queue.selected()?,
        })
    }
    fn matches(self, cx: &Runtime<'_>) -> bool {
        self.account == cx.account
            && self.generation == cx.epochs.current
            && self.session == cx.queue.durable_stamp().0
            && Some(self.occurrence) == cx.queue.selected()
    }
}
#[derive(Default)]
pub struct Controller {
    pending: Option<JoinHandle<()>>,
    active: Option<Selection>,
    automatic: Option<u64>,
    recovery: recovery::Controller,
    loaded: Option<Selection>,
    loaded_gain: Option<gain::Applied>,
    fallback_attempted: Option<u64>,
}
pub struct Runtime<'a> {
    pub queue: &'a mut queue::Queue,
    pub player: &'a Player,
    pub proxy: &'a proxy::Proxy,
    pub epochs: &'a mut gapless::Epochs,
    pub gapless: &'a mut gapless::Controller,
    pub playing: &'a mut bool,
    pub presented: &'a mut Option<u64>,
    pub state: &'a mut Value,
    pub output: &'a output::Settings,
    pub gain: &'a gain::Settings,
    pub account: u64,
}
#[derive(Default)]
pub struct Next {
    pub track: Option<Track>,
    pub automatic: bool,
}
impl Controller {
    fn cancel_selected(&mut self) {
        if let Some(task) = self.pending.take() {
            task.abort();
        }
        self.active = None;
        self.automatic = None;
        self.loaded = None;
        self.loaded_gain = None;
    }
    /// Account, output and explicit selection invalidation own both lifetimes.
    pub fn cancel(&mut self) {
        self.cancel_selected();
        self.recovery.cancel();
    }
    pub fn cancel_recovery(&mut self) -> bool {
        let cancelled = self.recovery.cancel();
        if cancelled {
            self.cancel_selected();
        }
        cancelled
    }
    pub fn recovering(&self) -> &recovery::View {
        &self.recovery.view
    }
    pub fn preparing(&self) -> bool {
        self.active.is_some() && self.pending.is_some()
    }
    pub async fn start(
        &mut self,
        track: Track,
        automatic: bool,
        provider: Option<Arc<dyn MusicProvider>>,
        tx: mpsc::Sender<Event>,
        cx: &mut Runtime<'_>,
    ) -> Result<(), &'static str> {
        let converted = cx.player.status().transcoded
            && self.loaded.is_some_and(|key| {
                key.account == cx.account
                    && key.session == cx.queue.durable_stamp().0
                    && Some(key.occurrence) == cx.queue.selected()
            });
        let retained_gain = converted.then(|| self.loaded_gain.clone()).flatten();
        self.cancel();
        self.loaded_gain = retained_gain;
        self.fallback_attempted = None;
        let Some(provider) = provider else {
            cx.state["selection"] = json!(track);
            cx.state["audio"] = Value::Null;
            cx.state["status"] = json!("Selection updated locally. Restore Plex sign-in to play.");
            return Ok(());
        };
        cx.gapless.invalidate();
        let next = cx.epochs.allocate()?;
        cx.epochs.current = next;
        cx.player.invalidate_load(next);
        cx.player.send(Command::Pause(true));
        cx.player.send(Command::Stop);
        cx.proxy.revoke().await;
        self.active = Selection::capture(cx);
        self.automatic = (automatic && *cx.playing).then_some(next);
        let request = gain::Request {
            reference: track.reference,
            mode: cx.gain.mode,
            album_session: cx.queue.album_gain_eligible(),
            fidelity: cx
                .output
                .selection
                .as_ref()
                .is_some_and(output::Selection::fidelity),
        };
        let version = cx.queue.selected_version().map(str::to_owned);
        let position = cx.queue.position();
        self.pending = Some(tokio::spawn(async move {
            let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
                if converted {
                    gain::convert(
                        provider.as_ref(),
                        request,
                        version.ok_or(gain::PrepareError::Verification(
                            "Missing pinned converted version",
                        ))?,
                        position,
                    )
                    .await
                } else {
                    gain::prepare(provider.as_ref(), request).await
                }
            })
            .await
            .unwrap_or(Err(gain::PrepareError::Provider(ProviderError::Network)));
            let _ = tx.send(Event::Media(next, result)).await;
        }));
        cx.state["selection"] = Value::Null;
        cx.state["audio"] = Value::Null;
        cx.state["intent"] = json!(if *cx.playing { "playing" } else { "paused" });
        cx.state["status"] = json!("Preparing selected queue entry");
        Ok(())
    }
    pub async fn complete(
        &mut self,
        g: u64,
        r: Result<gain::Prepared, gain::PrepareError>,
        cx: &mut Runtime<'_>,
    ) -> Next {
        if !self
            .active
            .is_some_and(|selection| selection.generation == g && selection.matches(cx))
        {
            return Next::default();
        }
        self.active = None;
        if let Some(task) = self.pending.take() {
            task.abort();
        }
        let mut to_prepare = None;
        let mut automatic_next = false;
        match r {
            Ok(prepared) => {
                let gain::Prepared {
                    media,
                    gain,
                    source,
                    continuous,
                } = prepared;
                if let Err(error) = cx
                    .queue
                    .update_selected(media.track.clone(), &media.version)
                {
                    let reason = version_failure(&error, &media.version);
                    match preparation_failure(
                        cx.queue,
                        reason,
                        self.automatic == Some(g),
                        *cx.playing,
                    ) {
                        Ok(next) => {
                            cx.state["error"] = if next.is_some() {
                                Value::Null
                            } else {
                                json!(error.to_string())
                            };
                            to_prepare = next;
                        }
                        Err(failure) => cx.state["error"] = json!(failure.to_string()),
                    }
                    automatic_next = to_prepare.is_some();
                    *cx.playing = automatic_next;
                    cx.state["intent"] = json!(if *cx.playing { "playing" } else { "paused" });
                } else {
                    cx.state["selection"] = json!(media.track);
                    let transcoded = media.stream.transcoded();
                    let timeline_offset = media.stream.timeline_offset();
                    let gain = if transcoded {
                        self.loaded_gain.clone().unwrap_or(gain)
                    } else {
                        gain
                    };
                    self.loaded = Selection::capture(cx);
                    self.loaded_gain = Some(gain.clone());
                    let url = cx.proxy.grant(media.stream).await;
                    if let Err(error) = cx.player.load_prepared(player::Load {
                        timeline_offset,
                        transcoded,
                        generation: g,
                        url,
                        position: cx.queue.position(),
                        gain,
                        source,
                        description: continuous,
                        chain: cx.gapless.view.enabled,
                        output: cx.output.selection.clone(),
                        playing: *cx.playing,
                    }) {
                        cx.proxy.revoke().await;
                        *cx.playing = false;
                        cx.state["intent"] = json!("paused");
                        cx.state["error"] = json!(error);
                        cx.state["status"] =
                            json!("Playback load refused; review and retry explicitly.");
                    } else {
                        cx.state["status"] = json!("Track selected; playback requested");
                    }
                }
            }
            Err(e) => {
                match preparation_failure(
                    cx.queue,
                    e.provider().and_then(queue::Unavailable::from_provider),
                    self.automatic == Some(g),
                    *cx.playing,
                ) {
                    Ok(next) => {
                        cx.state["error"] = if next.is_some() {
                            Value::Null
                        } else {
                            json!(e.to_string())
                        };
                        to_prepare = next;
                    }
                    Err(error) => cx.state["error"] = json!(error.to_string()),
                }
                automatic_next = to_prepare.is_some();
                *cx.playing = automatic_next;
                cx.state["status"] = json!(if automatic_next {
                    "Skipping unavailable entry"
                } else if e.provider().is_none() {
                    "Playback verification failed; review the message and explicitly retry"
                } else {
                    "Playback preparation failed; review unavailable entries or retry"
                });
                cx.state["intent"] = json!(if *cx.playing { "playing" } else { "paused" });
            }
        }
        self.automatic = None;
        Next {
            track: to_prepare,
            automatic: automatic_next,
        }
    }
    /// One provider fallback per explicit preparation, for a still-current pinned visit.
    pub async fn unsupported(
        &mut self,
        generation: u64,
        playing: bool,
        provider: Option<Arc<dyn MusicProvider>>,
        tx: mpsc::Sender<Event>,
        cx: &mut Runtime<'_>,
    ) -> Result<(), &'static str> {
        if generation != cx.epochs.current || self.fallback_attempted == Some(generation) {
            return Ok(());
        }
        self.fallback_attempted = Some(generation);
        if self.loaded.is_none_or(|key| !key.matches(cx)) {
            self.loaded_gain = cx.player.status().gain;
        }
        let (Some(provider), Some(version), Some(reference)) = (
            provider,
            cx.queue.selected_version().map(str::to_owned),
            cx.queue
                .selected()
                .and_then(|id| cx.queue.visible_track(id).ok())
                .map(|t| t.reference),
        ) else {
            return Ok(());
        };
        let _ = cx.player.barrier().await;
        cx.gapless.invalidate();
        cx.proxy.revoke().await;
        cx.epochs.reset()?;
        let next = cx.epochs.current;
        cx.player.invalidate_load(next);
        *cx.presented = None;
        *cx.playing = playing;
        self.active = Selection::capture(cx);
        self.automatic = None;
        let request = gain::Request {
            reference: reference.clone(),
            mode: cx.gain.mode,
            album_session: cx.queue.album_gain_eligible(),
            fidelity: cx
                .output
                .selection
                .as_ref()
                .is_some_and(output::Selection::fidelity),
        };
        let position = cx.queue.position();
        self.pending = Some(tokio::spawn(async move {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(20),
                gain::convert(provider.as_ref(), request, version, position),
            )
            .await
            .unwrap_or(Err(gain::PrepareError::Provider(ProviderError::Network)));
            let _ = tx.send(Event::Media(next, result)).await;
        }));
        cx.state["audio"] = Value::Null;
        cx.state["error"] = Value::Null;
        cx.state["status"] =
            json!("Local decoder does not support this media; trying Plex server conversion");
        Ok(())
    }
    pub async fn interrupted(
        &mut self,
        error: ProviderError,
        playing: bool,
        provider: Option<Arc<dyn MusicProvider>>,
        tx: mpsc::Sender<Event>,
        cx: &mut Runtime<'_>,
    ) -> Result<(), &'static str> {
        *cx.playing = false;
        let transcoded = cx.player.status().transcoded;
        let retained_gain = self.loaded_gain.clone();
        self.cancel_selected();
        if transcoded {
            self.loaded_gain = retained_gain;
        }
        let _ = cx.player.barrier().await;
        cx.gapless.invalidate();
        cx.proxy.revoke().await;
        cx.epochs.reset()?;
        cx.player.invalidate_load(cx.epochs.current);
        *cx.presented = None;
        cx.state["audio"] = Value::Null;
        cx.state["intent"] = json!("paused");
        if let (Some(provider), Some(key), Some(version), Some(track)) = (
            provider.clone(),
            recovery::Key::capture(cx.queue, cx.account, cx.epochs.current),
            cx.queue.selected_version().map(str::to_owned),
            cx.queue
                .selected()
                .and_then(|id| cx.queue.visible_track(id).ok()),
        ) {
            if let Some(reason) = queue::Unavailable::from_provider(error) {
                let _ = cx
                    .queue
                    .mark_unavailable(cx.queue.selected().unwrap(), reason);
            }
            self.active = Selection::capture(cx);
            self.recovery.begin(
                recovery::Work {
                    position: cx.queue.position(),
                    key,
                    version,
                    playing,
                    transcoded,
                    failure: error,
                    request: gain::Request {
                        reference: track.reference,
                        mode: cx.gain.mode,
                        album_session: cx.queue.album_gain_eligible(),
                        fidelity: cx
                            .output
                            .selection
                            .as_ref()
                            .is_some_and(output::Selection::fidelity),
                    },
                },
                provider,
                tx.clone(),
            );
            cx.state["error"] = if self.recovery.view.phase == "recovering" {
                Value::Null
            } else {
                json!(error.to_string())
            };
            cx.state["status"] = json!(self.recovery.view.message);
        } else {
            cx.state["error"] =
                json!("Stream interrupted without a pinned recovery target; retry explicitly.");
        }
        Ok(())
    }
    pub async fn recovered(&mut self, reply: recovery::Reply, cx: &mut Runtime<'_>) -> Next {
        let generation = reply.key.generation;
        if !self.recovery.owns(reply.key) {
            return Next::default();
        }
        if !reply.key.matches(cx.queue, cx.account, cx.epochs.current) {
            self.cancel_recovery();
            return Next::default();
        }
        match self.recovery.accept(reply) {
            Some((playing, Ok(prepared))) => {
                *cx.playing = playing;
                cx.state["error"] = Value::Null;
                self.complete(generation, Ok(prepared), cx).await
            }
            Some((_, Err(error))) => {
                self.active = None;
                *cx.playing = false;
                cx.state["intent"] = json!("paused");
                cx.state["error"] = json!(error.message());
                cx.state["status"] = json!("Recovery exhausted; Retry or Skip explicitly.");
                if let Some(reason) = error.reason()
                    && let Some(id) = cx.queue.selected()
                {
                    let _ = cx.queue.mark_unavailable(id, reason);
                }
                Next::default()
            }
            None => Next::default(),
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.cancel();
    }
}
pub(crate) fn version_failure(error: &queue::Error, version: &str) -> Option<queue::Unavailable> {
    (*error == queue::Error::Version && !version.is_empty() && version.len() <= 512)
        .then_some(queue::Unavailable::VersionChanged)
}
pub(crate) fn preparation_failure(
    queue: &mut queue::Queue,
    reason: Option<queue::Unavailable>,
    automatic: bool,
    playing: bool,
) -> Result<Option<qoda_provider::Track>, queue::Error> {
    let Some(id) = queue.selected() else {
        return Ok(None);
    };
    if let Some(reason) = reason {
        queue.mark_unavailable(id, reason)?;
        if automatic && playing {
            return queue.advance();
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::tests::{Answer, Fixture, fixture};
    use std::time::Duration;

    struct Environment {
        queue: queue::Queue,
        player: Player,
        proxy: proxy::Proxy,
        epochs: gapless::Epochs,
        gapless: gapless::Controller,
        playing: bool,
        presented: Option<u64>,
        state: Value,
        output: output::Settings,
        gain: gain::Settings,
        account: u64,
    }
    impl Environment {
        async fn new(answers: Vec<Answer>) -> (Self, Arc<Fixture>) {
            let (queue, provider) = fixture(answers);
            let mut output = output::Settings::load(None);
            output.selection = Some(output::Selection::Desktop);
            (
                Self {
                    queue,
                    player: Player::test_output("null").unwrap(),
                    proxy: proxy::Proxy::start().await.unwrap(),
                    epochs: gapless::Epochs::default(),
                    gapless: gapless::Controller::default(),
                    playing: true,
                    presented: None,
                    state: json!({}),
                    output,
                    gain: gain::Settings::load(None),
                    account: 1,
                },
                provider,
            )
        }
        fn runtime(&mut self) -> Runtime<'_> {
            Runtime {
                queue: &mut self.queue,
                player: &self.player,
                proxy: &self.proxy,
                epochs: &mut self.epochs,
                gapless: &mut self.gapless,
                playing: &mut self.playing,
                presented: &mut self.presented,
                state: &mut self.state,
                output: &self.output,
                gain: &self.gain,
                account: self.account,
            }
        }
        async fn command(&mut self, owner: &mut Controller, op: &str) {
            let mut request = json!({"instance":"fixture","op":op,"mode":"desktop"});
            let result = crate::session_commands::Playback {
                queue: &mut self.queue,
                player: &self.player,
                proxy: &self.proxy,
                epochs: &mut self.epochs,
                gapless: &mut self.gapless,
                preparation: owner,
                desired_playing: &mut self.playing,
                presented: &mut self.presented,
            }
            .admit(
                &mut request,
                &mut crate::session_commands::Context {
                    instance: "fixture",
                    account: self.account,
                    connected: true,
                    credential: true,
                    observations: &mut crate::observation::Controller::default(),
                    can_raise: false,
                    state: &mut self.state,
                    catalog: &crate::catalog::Controller::default(),
                    albums: &crate::albums::Controller::default(),
                    playlists: &crate::playlists::Controller::default(),
                    search: &crate::search::Controller::default(),
                    output: &mut self.output,
                    devices: &[],
                    gain: &mut self.gain,
                },
                None,
                None,
            )
            .await
            .unwrap();
            match result {
                crate::session_commands::Admission::Applied(effects) => {
                    assert_eq!(effects.force_save, op == "pause");
                    if op == "resume" {
                        assert!(
                            effects.prepare.is_none(),
                            "Pending preparation must be retained"
                        );
                    }
                }
                _ => panic!("Command was not admitted: {}", self.state),
            }
        }
        fn selected(&self) -> Track {
            self.queue
                .visible_track(self.queue.selected().unwrap())
                .unwrap()
        }
        async fn loaded(&self) {
            let until = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let status = self.player.status();
                if status.generation == self.epochs.current && status.entry_ready {
                    assert!(status.error.is_none(), "{status:?}");
                    return;
                }
                assert!(tokio::time::Instant::now() < until, "{status:?}");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
    async fn media(
        rx: &mut mpsc::Receiver<Event>,
    ) -> (u64, Result<gain::Prepared, gain::PrepareError>) {
        let Event::Media(g, result) = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("Expected selected preparation");
        };
        (g, result)
    }
    struct Converted(Box<dyn qoda_provider::MediaSource>, f64);
    impl qoda_provider::MediaSource for Converted {
        fn timeline_offset(&self) -> f64 {
            self.1
        }
        fn transcoded(&self) -> bool {
            true
        }
        fn open(
            &self,
            range: Option<qoda_provider::ByteRange>,
            head: bool,
        ) -> qoda_provider::ProviderFuture<'_, qoda_provider::MediaResponse> {
            self.0.open(range, head)
        }
    }
    struct Unrecognized;
    impl qoda_provider::MediaSource for Unrecognized {
        fn open(
            &self,
            _: Option<qoda_provider::ByteRange>,
            _: bool,
        ) -> qoda_provider::ProviderFuture<'_, qoda_provider::MediaResponse> {
            use futures_util::StreamExt;
            Box::pin(async {
                Ok(qoda_provider::MediaResponse {
                    status: 200,
                    content_type: Some("application/octet-stream".into()),
                    content_length: Some(23),
                    content_range: None,
                    etag: None,
                    body: futures_util::stream::once(async {
                        Ok(bytes::Bytes::from_static(b"unrecognized audio data"))
                    })
                    .boxed(),
                })
            })
        }
    }
    struct Fallback {
        inner: Arc<Fixture>,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl MusicProvider for Fallback {
        fn libraries(&self) -> Vec<qoda_provider::Library> {
            self.inner.libraries()
        }
        fn tracks(
            &self,
            source: qoda_core::SourceId,
            offset: u64,
            query: String,
        ) -> qoda_provider::ProviderFuture<'_, qoda_provider::CatalogPage> {
            self.inner.tracks(source, offset, query)
        }
        fn prepare(
            &self,
            source: qoda_core::SourceId,
            key: qoda_core::ProviderTrackKey,
        ) -> qoda_provider::ProviderFuture<'_, qoda_provider::PreparedMedia> {
            self.inner.prepare(source, key)
        }
        fn prepare_transcoded(
            &self,
            reference: qoda_core::TrackRef,
            version: String,
            position: f64,
        ) -> qoda_provider::ProviderFuture<'_, qoda_provider::PreparedMedia> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                let mut media = self.inner.prepare(reference.source, reference.key).await?;
                assert_eq!(version, media.version);
                media.stream = Box::new(Converted(media.stream, position));
                Ok(media)
            })
        }
    }
    #[tokio::test]
    #[ignore = "explicit live Plex qualification using the saved sign-in; digital null output only"]
    async fn live_saved_plex_transcode_decodes_and_seeks_on_null_output() {
        use qoda_provider::{Credential, PlexProvider};
        let result = std::process::Command::new("/usr/bin/secret-tool")
            .args([
                "lookup",
                "application",
                "qoda-player",
                "credential",
                "plex",
                "schema",
                "1",
            ])
            .output()
            .unwrap();
        assert!(result.status.success());
        let secret = zeroize::Zeroizing::new(result.stdout);
        let credential: Credential = serde_json::from_slice(&secret).unwrap();
        drop(secret);
        let (provider, _) = PlexProvider::connect(
            credential,
            crate::identities(crate::identities_path().as_deref()).unwrap(),
        )
        .await
        .unwrap();
        let home = std::env::var_os("HOME").unwrap();
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(home).join(".local/state"));
        let db = rusqlite::Connection::open_with_flags(
            state.join("qoda/session.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let saved: String = db
            .query_row("select payload from session where slot=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let saved: Value = serde_json::from_str(&saved).unwrap();
        let queue: queue::Queue = serde_json::from_value(saved["queue"].clone()).unwrap();
        let track = queue.visible_track(queue.selected().unwrap()).unwrap();
        let original = provider
            .prepare(track.reference.source, track.reference.key.clone())
            .await
            .unwrap();
        let media = provider
            .prepare_transcoded(track.reference.clone(), original.version.clone(), 2.0)
            .await
            .unwrap();
        assert_eq!(media.version, original.version);
        let prepared = gain::describe(
            media,
            gain::Request {
                reference: track.reference,
                mode: gain::Mode::Off,
                fidelity: false,
                album_session: false,
            },
        )
        .await
        .unwrap();
        let proxy = proxy::Proxy::start().await.unwrap();
        let player = Player::test_options("null", &[("ao-null-untimed", "no".into())]).unwrap();
        let url = proxy.grant(prepared.media.stream).await;
        player
            .load_prepared(player::Load {
                timeline_offset: 2.0,
                transcoded: true,
                generation: 1,
                url,
                position: 2.0,
                gain: prepared.gain,
                source: prepared.source,
                description: None,
                chain: false,
                output: Some(output::Selection::Desktop),
                playing: true,
            })
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let result = loop {
            let status = player.status();
            if status.error.is_some() {
                break Err(format!("{status:?}"));
            }
            if status.entry_ready && status.position_seconds.is_some_and(|p| p > 2.1) {
                break if status.transcoded
                    && status.decoder_codec.as_deref() == Some("flac")
                    && status.source.is_some()
                {
                    Ok(())
                } else {
                    Err(format!("{status:?}"))
                };
            }
            if tokio::time::Instant::now() >= deadline {
                break Err(format!("{status:?}"));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        player.barrier().await.unwrap();
        proxy.revoke().await;
        assert!(result.is_ok(), "{result:?}");
    }
    #[tokio::test]
    async fn unsupported_native_input_uses_one_pinned_fallback_retains_gain_visit_and_later_pause()
    {
        let (mut env, inner) = Environment::new(vec![
            Answer::Version("v1"),
            Answer::Version("v1"),
            Answer::Version("v1"),
        ])
        .await;
        let provider = Arc::new(Fallback {
            inner,
            calls: Default::default(),
        });
        let mut owner = Controller::default();
        let (tx, mut rx) = mpsc::channel(8);
        let selected = env.queue.selected();
        let visit = serde_json::to_value(&env.queue).unwrap()["visit"].clone();
        owner
            .start(
                env.selected(),
                false,
                Some(provider.clone()),
                tx.clone(),
                &mut env.runtime(),
            )
            .await
            .unwrap();
        let (generation, reply) = media(&mut rx).await;
        let mut prepared = reply.unwrap();
        prepared.media.stream = Box::new(Unrecognized);
        prepared.gain.db = -3.0;
        owner
            .complete(generation, Ok(prepared), &mut env.runtime())
            .await;
        let until = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if env.player.status().error.is_some() {
                break;
            }
            assert!(tokio::time::Instant::now() < until);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(env.player.status().unsupported_format);
        assert_eq!(
            provider.calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "direct playback must precede provider conversion"
        );
        owner
            .unsupported(
                generation,
                true,
                Some(provider.clone()),
                tx.clone(),
                &mut env.runtime(),
            )
            .await
            .unwrap();
        let (fallback, reply) = media(&mut rx).await;
        env.command(&mut owner, "pause").await;
        owner.complete(fallback, reply, &mut env.runtime()).await;
        env.loaded().await;
        let status = env.player.status();
        assert!(status.transcoded && status.paused && !env.playing);
        assert_eq!(status.gain.unwrap().db, -3.0);
        assert_eq!(env.queue.selected(), selected);
        assert_eq!(env.queue.selected_version(), Some("v1"));
        assert_eq!(serde_json::to_value(&env.queue).unwrap()["visit"], visit);
        env.queue.set_position(3.0).unwrap();
        owner
            .start(
                env.selected(),
                false,
                Some(provider.clone()),
                tx.clone(),
                &mut env.runtime(),
            )
            .await
            .unwrap();
        let (seek, reply) = media(&mut rx).await;
        owner.complete(seek, reply, &mut env.runtime()).await;
        env.loaded().await;
        env.command(&mut owner, "resume").await;
        let until = tokio::time::Instant::now() + Duration::from_secs(4);
        loop {
            let status = env.player.status();
            if status.position_seconds.is_some_and(|p| p > 3.1) {
                break;
            }
            assert!(status.error.is_none(), "{status:?}");
            assert!(tokio::time::Instant::now() < until, "{status:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        owner
            .unsupported(
                generation,
                true,
                Some(provider.clone()),
                tx,
                &mut env.runtime(),
            )
            .await
            .unwrap();
        assert_eq!(provider.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn converted_completion_cannot_cross_account_or_output_cancellation() {
        for account in [false, true] {
            let (mut env, inner) =
                Environment::new(vec![Answer::Version("v1"), Answer::Version("v1")]).await;
            let root = std::env::temp_dir()
                .join(format!("qoda-converted-cancel-{}", uuid::Uuid::new_v4()));
            env.output = output::Settings::load(Some(root.join("output.json")));
            env.output.selection = Some(output::Selection::Desktop);
            let provider = Arc::new(Fallback {
                inner,
                calls: Default::default(),
            });
            let mut owner = Controller::default();
            let (tx, mut rx) = mpsc::channel(8);
            owner
                .start(
                    env.selected(),
                    false,
                    Some(provider.clone()),
                    tx.clone(),
                    &mut env.runtime(),
                )
                .await
                .unwrap();
            let (generation, reply) = media(&mut rx).await;
            owner.complete(generation, reply, &mut env.runtime()).await;
            env.loaded().await;
            owner
                .unsupported(generation, true, Some(provider), tx, &mut env.runtime())
                .await
                .unwrap();
            let (fallback, reply) = media(&mut rx).await;
            if account {
                owner.cancel();
                env.account += 1;
                env.epochs.reset().unwrap();
                env.player.invalidate_load(env.epochs.current);
            } else {
                env.output.selection = None;
                env.command(&mut owner, "set_output").await;
            }
            owner.complete(fallback, reply, &mut env.runtime()).await;
            assert!(env.player.status().idle || env.player.status().paused);
            assert!(!env.player.status().transcoded);
            assert!(!owner.preparing());
            env.proxy.revoke().await;
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[tokio::test]
    async fn paused_selected_preparation_loads_once_and_duplicate_failure_cannot_change_it() {
        let (mut env, provider) = Environment::new(vec![Answer::Version("v1")]).await;
        let mut owner = Controller::default();
        let (tx, mut rx) = mpsc::channel(8);
        owner
            .start(
                env.selected(),
                false,
                Some(provider),
                tx,
                &mut env.runtime(),
            )
            .await
            .unwrap();
        let (g, result) = media(&mut rx).await;
        env.command(&mut owner, "pause").await;
        env.command(&mut owner, "resume").await;
        assert!(env.playing && owner.preparing());
        env.command(&mut owner, "pause").await;
        owner.complete(g, result, &mut env.runtime()).await;
        env.loaded().await;
        assert!(env.player.status().paused);
        let selection = env.state["selection"].clone();
        owner
            .complete(
                g,
                Err(gain::PrepareError::Provider(ProviderError::MissingTrack)),
                &mut env.runtime(),
            )
            .await;
        assert!(!env.queue.selected_unavailable());
        assert_eq!(env.state["selection"], selection);
        assert!(env.state["error"].is_null());
        env.proxy.revoke().await;
    }
    #[tokio::test]
    async fn delayed_success_and_failure_cannot_cross_account_selection_or_output_ownership() {
        for invalidation in 0..3 {
            for success in [true, false] {
                let answer = if success {
                    Answer::Version("v1")
                } else {
                    Answer::Error(ProviderError::MissingTrack)
                };
                let (mut env, provider) = Environment::new(vec![answer]).await;
                let mut owner = Controller::default();
                let (tx, mut rx) = mpsc::channel(8);
                owner
                    .start(
                        env.selected(),
                        false,
                        Some(provider),
                        tx,
                        &mut env.runtime(),
                    )
                    .await
                    .unwrap();
                let (g, result) = media(&mut rx).await;
                match invalidation {
                    0 => env.account += 1,
                    1 => {
                        env.queue.replace(env.selected()).unwrap();
                    }
                    _ => {
                        env.epochs.reset().unwrap();
                        env.player.invalidate_load(env.epochs.current);
                    }
                }
                env.playing = false;
                let before = env.state.clone();
                assert!(
                    owner
                        .complete(g, result, &mut env.runtime())
                        .await
                        .track
                        .is_none()
                );
                assert_eq!(env.state, before);
                assert!(!env.queue.selected_unavailable());
                assert!(!env.player.barrier().await.unwrap().entry_ready);
                assert!(!env.playing);
                env.proxy.revoke().await;
            }
        }
    }
    #[tokio::test]
    async fn explicit_or_paused_failure_stops_but_automatic_playing_failure_advances() {
        for automatic in [false, true] {
            for paused in [false, true] {
                let (mut env, provider) =
                    Environment::new(vec![Answer::Error(ProviderError::MissingTrack)]).await;
                env.queue.enqueue(env.selected(), false).unwrap();
                let selected = env.queue.selected();
                let mut owner = Controller::default();
                let (tx, mut rx) = mpsc::channel(8);
                owner
                    .start(
                        env.selected(),
                        automatic,
                        Some(provider),
                        tx,
                        &mut env.runtime(),
                    )
                    .await
                    .unwrap();
                let (g, result) = media(&mut rx).await;
                if paused {
                    env.command(&mut owner, "pause").await;
                }
                let next = owner.complete(g, result, &mut env.runtime()).await;
                let advance = automatic && !paused;
                assert_eq!(next.track.is_some(), advance);
                assert_eq!(next.automatic, advance);
                assert_eq!(env.playing, advance);
                assert_eq!(env.queue.selected() == selected, !advance);
                env.proxy.revoke().await;
            }
        }
    }
    #[tokio::test]
    async fn pinned_recovery_loads_retained_position_without_another_visit_and_pause_cancels_late_reply()
     {
        for cancel in [false, true] {
            let (mut env, provider) = Environment::new(vec![Answer::Version("v1")]).await;
            let selected = env.queue.selected();
            let visit = env.queue.snapshot().previous_available;
            let mut owner = Controller::default();
            let (tx, mut rx) = mpsc::channel(8);
            owner
                .interrupted(
                    ProviderError::Network,
                    false,
                    Some(provider),
                    tx,
                    &mut env.runtime(),
                )
                .await
                .unwrap();
            loop {
                let Event::Recovery(reply) =
                    tokio::time::timeout(Duration::from_secs(4), rx.recv())
                        .await
                        .unwrap()
                        .unwrap()
                else {
                    panic!("Expected recovery");
                };
                let done = reply.result.is_some();
                if done && cancel {
                    env.command(&mut owner, "pause").await;
                    assert_ne!(owner.recovering().phase, "recovering");
                }
                owner.recovered(reply, &mut env.runtime()).await;
                if done {
                    break;
                }
            }
            assert!(!env.playing);
            assert_eq!(env.queue.selected(), selected);
            assert_eq!(env.queue.position(), 1.0);
            assert_eq!(env.queue.snapshot().previous_available, visit);
            if cancel {
                assert!(!env.player.barrier().await.unwrap().entry_ready);
            } else {
                env.loaded().await;
                assert!(env.player.status().paused);
                env.player.send(Command::Pause(false));
                let until = tokio::time::Instant::now() + Duration::from_secs(4);
                loop {
                    let status = env.player.status();
                    if let Some(position) = status.position_seconds {
                        assert!(position >= 0.9);
                        break;
                    }
                    assert!(tokio::time::Instant::now() < until, "{status:?}");
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            env.proxy.revoke().await;
        }
    }
    struct TrackedSource {
        inner: Box<dyn qoda_provider::MediaSource>,
        dropped: Arc<std::sync::atomic::AtomicBool>,
    }
    impl qoda_provider::MediaSource for TrackedSource {
        fn open(
            &self,
            range: Option<qoda_provider::ByteRange>,
            head: bool,
        ) -> qoda_provider::ProviderFuture<'_, qoda_provider::MediaResponse> {
            self.inner.open(range, head)
        }
    }
    impl Drop for TrackedSource {
        fn drop(&mut self) {
            self.dropped
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
    #[tokio::test]
    async fn refused_native_load_releases_prepared_stream_and_stays_paused() {
        let (mut env, provider) = Environment::new(vec![Answer::Version("v1")]).await;
        let mut owner = Controller::default();
        let (tx, mut rx) = mpsc::channel(8);
        owner
            .start(
                env.selected(),
                false,
                Some(provider),
                tx,
                &mut env.runtime(),
            )
            .await
            .unwrap();
        let (g, result) = media(&mut rx).await;
        let mut prepared = result.unwrap();
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        prepared.media.stream = Box::new(TrackedSource {
            inner: prepared.media.stream,
            dropped: dropped.clone(),
        });
        env.player.invalidate_load(g + 1);
        owner.complete(g, Ok(prepared), &mut env.runtime()).await;
        assert!(!env.playing);
        assert!(
            env.state["error"]
                .as_str()
                .unwrap()
                .contains("Stale playback preparation")
        );
        assert!(
            dropped.load(std::sync::atomic::Ordering::Acquire),
            "Refused input retained its stream grant"
        );
    }
    #[tokio::test]
    async fn recovery_refuses_version_substitution_and_retains_exhausted_budget() {
        for version_changed in [true, false] {
            let answers = if version_changed {
                vec![Answer::Version("other-version")]
            } else {
                vec![
                    Answer::Error(ProviderError::Network),
                    Answer::Error(ProviderError::Network),
                ]
            };
            let (mut env, provider) = Environment::new(answers).await;
            let selected = env.queue.selected();
            let mut owner = Controller::default();
            let (tx, mut rx) = mpsc::channel(8);
            owner
                .interrupted(
                    ProviderError::Network,
                    true,
                    Some(provider.clone()),
                    tx.clone(),
                    &mut env.runtime(),
                )
                .await
                .unwrap();
            while owner.recovering().phase == "recovering" {
                let Event::Recovery(reply) =
                    tokio::time::timeout(Duration::from_secs(4), rx.recv())
                        .await
                        .unwrap()
                        .unwrap()
                else {
                    panic!("Expected recovery");
                };
                owner.recovered(reply, &mut env.runtime()).await;
            }
            assert_eq!(owner.recovering().phase, "exhausted");
            assert!(!env.playing);
            assert_eq!(env.queue.selected(), selected);
            assert_eq!(env.queue.selected_unavailable(), version_changed);
            assert!(!env.player.barrier().await.unwrap().entry_ready);
            if !version_changed {
                assert_eq!(owner.recovering().attempts, 2);
                owner
                    .interrupted(
                        ProviderError::Network,
                        true,
                        Some(provider),
                        tx,
                        &mut env.runtime(),
                    )
                    .await
                    .unwrap();
                assert_eq!(owner.recovering().phase, "exhausted");
                assert_eq!(owner.recovering().attempts, 2);
                assert!(rx.try_recv().is_err());
            }
        }
    }
    #[tokio::test]
    async fn admitted_output_change_cancels_selected_preparation_before_delayed_completion() {
        let (mut env, provider) = Environment::new(vec![Answer::Version("v1")]).await;
        let root =
            std::env::temp_dir().join(format!("qoda-prepared-output-{}", uuid::Uuid::new_v4()));
        let mut owner = Controller::default();
        let (tx, mut rx) = mpsc::channel(8);
        owner
            .start(
                env.selected(),
                false,
                Some(provider),
                tx,
                &mut env.runtime(),
            )
            .await
            .unwrap();
        let (g, result) = media(&mut rx).await;
        env.output = output::Settings::load(Some(root.join("output.json")));
        env.output.selection = None;
        env.command(&mut owner, "set_output").await;
        assert!(!owner.preparing());
        assert!(env.epochs.current > g);
        let state = env.state.clone();
        owner.complete(g, result, &mut env.runtime()).await;
        assert_eq!(env.state, state);
        assert!(!env.playing);
        assert!(!env.player.barrier().await.unwrap().entry_ready);
        env.proxy.revoke().await;
        std::fs::remove_dir_all(root).unwrap();
    }
}
