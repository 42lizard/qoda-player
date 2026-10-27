//! Listening-session command admission owns validation before cancellation,
//! native reset ordering and desktop acknowledgement. Queue owns membership,
//! visits and navigation; Player/gapless retain native output implementation.
use crate::{
    albums, catalog, gain, gapless, mpris, output,
    player::{self, Command, Player},
    playlists, prepared_playback, proxy, queue, record_clock, search,
};
use qoda_core::OccurrenceId;
use serde_json::{Value, json};
pub(crate) fn navigation_request(
    queue: &queue::Queue,
    request: &Value,
) -> Result<(u64, OccurrenceId), String> {
    check_queue(queue, request).map_err(|error| error.to_string())?;
    let index = serde_json::from_value::<qoda_core::Counter>(request["index"].clone())
        .map_err(|_| "Choose an entry from the current Previous path.")?
        .get();
    let occurrence = serde_json::from_value::<OccurrenceId>(request["occurrence"].clone())
        .map_err(|_| "Choose an entry from the current Previous path.")?;
    queue
        .navigation_target(index, occurrence)
        .map_err(|error| error.to_string())?;
    Ok((index, occurrence))
}

pub(crate) fn unavailable_request(
    queue: &queue::Queue,
    request: &Value,
) -> Result<Option<OccurrenceId>, String> {
    check_queue(queue, request).map_err(|error| error.to_string())?;
    if request["op"] == "skip_unavailable" {
        if queue.selected_unavailable() {
            return Ok(None);
        }
    } else if request["op"] == "retry_unavailable"
        && let Ok(id) = serde_json::from_value::<OccurrenceId>(request["occurrence"].clone())
        && queue.is_unavailable(id)
    {
        return Ok(Some(id));
    }
    Err("That unavailable entry changed. Review the current session.".into())
}
pub(crate) fn reconnect_request(
    queue: &queue::Queue,
    request: &Value,
    account: u64,
    credential: bool,
) -> Result<(), String> {
    check_queue(queue, request).map_err(|e| e.to_string())?;
    if !credential {
        return Err("Restore sign-in or connect Plex before reconnecting.".into());
    }
    if request["account_epoch"].as_str() != Some(account.to_string().as_str()) {
        return Err("The account changed. Review the connection before retrying.".into());
    }
    Ok(())
}

pub(crate) fn check_queue(queue: &queue::Queue, request: &Value) -> Result<(), queue::Error> {
    let session = serde_json::from_value(request["queue_session"].clone())
        .map_err(|_| queue::Error::Conflict)?;
    let revision = serde_json::from_value(request["queue_revision"].clone())
        .map_err(|_| queue::Error::Conflict)?;
    queue.check(session, revision)
}
pub(crate) async fn finish_gapless_reset(
    gapless: &mut gapless::Controller,
    queue: &mut queue::Queue,
    epochs: &mut gapless::Epochs,
    before: &player::Status,
    presented: &mut Option<u64>,
    proxy: &proxy::Proxy,
) -> Result<(), String> {
    let stream_failed = proxy.failed().await;
    let promoted = gapless.handoff(queue, epochs, before, proxy).await;
    record_clock(queue, before, epochs.current, presented, stream_failed);
    gapless.invalidate();
    proxy.revoke().await;
    epochs.reset().map_err(str::to_owned)?;
    gapless.review();
    promoted.map(|_| ())
}

#[derive(Default)]
pub struct Effects {
    pub prepare: Option<qoda_provider::Track>,
    pub automatic: bool,
    pub force_save: bool,
    pub seeked: bool,
}
pub enum Admission {
    Forward,
    Applied(Effects),
    Rejected,
}
// The same resolved operation validates a bounded trial and mutates the live
// Queue. Neither caller nor a second string switch chooses the Queue operation.
enum Edit {
    Replace {
        tracks: Vec<qoda_provider::Track>,
        album: Option<(qoda_provider::AlbumRef, Vec<qoda_core::TrackRef>)>,
        batch: bool,
    },
    Enqueue {
        tracks: Vec<qoda_provider::Track>,
        next: bool,
        batch: bool,
    },
    Select(OccurrenceId),
    Retry(OccurrenceId),
    Next {
        skip: bool,
    },
    Replay,
    Previous(Option<f64>),
    Revisit(u64, OccurrenceId),
    Move(OccurrenceId, Option<OccurrenceId>),
    Remove(OccurrenceId),
    ClearUpcoming,
    StopClear,
    Shuffle(bool),
    Repeat(queue::Repeat),
    Seek(f64),
}
impl Edit {
    fn apply(
        &self,
        queue: &mut queue::Queue,
    ) -> Result<Option<qoda_provider::Track>, queue::Error> {
        match self {
            Self::Replace {
                tracks,
                album: Some((reference, order)),
                ..
            } => queue.replace_album(tracks.clone(), reference.clone(), order.clone()),
            Self::Replace { tracks, .. } => match tracks.as_slice() {
                [track] => queue
                    .replace(track.clone())
                    .and_then(|id| queue.visible_track(id))
                    .map(Some),
                _ => queue.replace_many(tracks.clone()),
            },
            Self::Enqueue { tracks, next, .. } => {
                if *next {
                    queue.enqueue_next_many(tracks.clone())?;
                } else {
                    queue.enqueue_many(tracks.clone())?;
                }
                Ok(None)
            }
            Self::Select(id) => queue.select(*id).map(Some),
            Self::Retry(id) => queue.retry_unavailable(*id).map(Some),
            Self::Next { .. } => queue.advance(),
            Self::Replay => queue.replay(),
            Self::Previous(position) => {
                if let Some(position) = position {
                    queue.set_position(*position)?;
                }
                queue.previous()
            }
            Self::Revisit(index, id) => queue.revisit(*index, *id).map(Some),
            Self::Move(id, before) => queue.move_before(*id, *before).map(|()| None),
            Self::Remove(id) => {
                if queue.remove(*id)? {
                    queue
                        .selected()
                        .map(|id| queue.visible_track(id))
                        .transpose()
                } else {
                    Ok(None)
                }
            }
            Self::ClearUpcoming => queue.clear_upcoming().map(|()| None),
            Self::StopClear => {
                *queue = queue.empty_session();
                Ok(None)
            }
            Self::Shuffle(enabled) => queue.set_shuffle(*enabled).map(|()| None),
            Self::Repeat(mode) => queue.set_repeat(*mode).map(|()| None),
            Self::Seek(position) => queue.seek_to(*position).map(|()| None),
        }
    }
}

/// Borrowed runtime ownership keeps transition sequencing inside this module.
/// Catalog/account lifetimes remain with the coordinator, not with the command.
pub struct Playback<'a> {
    pub queue: &'a mut queue::Queue,
    pub player: &'a Player,
    pub proxy: &'a proxy::Proxy,
    pub epochs: &'a mut gapless::Epochs,
    pub gapless: &'a mut gapless::Controller,
    pub preparation: &'a mut prepared_playback::Controller,
    pub desired_playing: &'a mut bool,
    pub presented: &'a mut Option<u64>,
}
pub struct Context<'a> {
    pub instance: &'a str,
    pub account: u64,
    pub connected: bool,
    pub credential: bool,
    pub observations: &'a mut crate::observation::Controller,
    pub can_raise: bool,
    pub state: &'a mut Value,
    pub catalog: &'a catalog::Controller,
    pub albums: &'a albums::Controller,
    pub playlists: &'a playlists::Controller,
    pub search: &'a search::Controller,
    pub output: &'a mut output::Settings,
    pub devices: &'a [output::Device],
    pub gain: &'a mut gain::Settings,
}
pub struct Desktop(mpris::Request);
impl Desktop {
    pub fn new(request: mpris::Request) -> Self {
        Self(request)
    }
    fn valid(&self, owner: Option<&mpris::Service>) -> bool {
        self.0.is_live()
            && owner.is_some_and(|service| service.id() == self.0.owner && service.is_live())
    }
    pub fn complete(self, state: &Value) {
        let result = state["error"]
            .as_str()
            .map(|s| Err(s.to_owned()))
            .unwrap_or(Ok(()));
        let _ = self.0.reply.send(result);
    }
}
impl Playback<'_> {
    pub async fn admit(
        &mut self,
        request: &mut Value,
        context: &mut Context<'_>,
        desktop: Option<&Desktop>,
        owner: Option<&mpris::Service>,
    ) -> Result<Admission, &'static str> {
        if request["instance"] != context.instance {
            return Ok(Admission::Rejected);
        }
        context.state["error"] = Value::Null;
        if desktop.is_some_and(|command| !command.valid(owner)) {
            context.state["error"] =
                json!("The desktop command expired or its connection was replaced.");
            return Ok(Admission::Rejected);
        }
        if let Err(error) = self.validate(request, context) {
            context.state["error"] = json!(error);
            return Ok(Admission::Rejected);
        }
        // Save policy before cancelling speculative output; failed and unchanged
        // preferences must leave the staged successor alone.
        match request["op"].as_str().unwrap_or("") {
            "set_output" => match context.output.request(request, context.devices) {
                Ok(selection)
                    if context.output.selection.as_ref() == Some(&selection)
                        && self.player.status().output_selection.as_ref() == Some(&selection) =>
                {
                    return Ok(Admission::Rejected);
                }
                Ok(selection) => {
                    if let Err(error) = context.output.set(selection) {
                        context.state["error"] = json!(error);
                        return Ok(Admission::Rejected);
                    }
                }
                Err(error) => {
                    context.state["error"] = json!(error);
                    return Ok(Admission::Rejected);
                }
            },
            "set_replaygain" => match serde_json::from_value::<gain::Mode>(request["mode"].clone())
            {
                Ok(mode) if mode == context.gain.mode => return Ok(Admission::Rejected),
                Ok(mode) => {
                    if let Err(error) = context.gain.set(mode) {
                        context.state["error"] = json!(error);
                        return Ok(Admission::Rejected);
                    }
                }
                Err(_) => {
                    context.state["error"] = json!("Choose Off, Track, Album or Auto ReplayGain.");
                    return Ok(Admission::Rejected);
                }
            },
            _ => {}
        }
        let effect = Effect::of(request["op"].as_str().unwrap_or(""));
        if (effect != Effect::None || request["op"] == "pause")
            && self.preparation.cancel_recovery()
        {
            *self.desired_playing = false;
            self.player.send(Command::Pause(true));
            self.player.send(Command::Stop);
            self.epochs.reset()?;
            self.player.invalidate_load(self.epochs.current);
            self.gapless.invalidate();
            self.proxy.revoke().await;
            *self.presented = None;
            context.state["audio"] = Value::Null;
            context.state["intent"] = json!("paused");
            context.state["status"] = json!("Recovery cancelled; resume explicitly.");
        }
        if effect != Effect::None {
            let cancelled = self.gapless.cancel(self.player, self.proxy).await;
            let reset = match cancelled {
                Ok(Some(before)) => Some(Ok(before)),
                Ok(None)
                    if self.gapless.chained
                        && effect.changes_current(request, self.queue)
                        && *self.presented != Some(self.epochs.current) =>
                {
                    Some(self.player.barrier().await)
                }
                Ok(None) => None,
                Err(_) => Some(self.player.barrier().await),
            };
            if let Some(reset) = reset {
                *self.desired_playing = false;
                let result = match reset {
                    Ok(before) => {
                        finish_gapless_reset(
                            self.gapless,
                            self.queue,
                            self.epochs,
                            &before,
                            self.presented,
                            self.proxy,
                        )
                        .await
                    }
                    Err(error) => {
                        self.gapless.invalidate();
                        self.proxy.revoke().await;
                        self.epochs.reset()?;
                        self.player.invalidate_load(self.epochs.current);
                        self.player.send(Command::Pause(true));
                        self.gapless.review();
                        Err(error.to_owned())
                    }
                };
                context.state["audio"] = Value::Null;
                context.state["intent"] = json!("paused");
                context.state["status"] =
                    json!("Buffered transition reset; playback paused for review.");
                if let Err(error) = result {
                    context.state["error"] = json!(error);
                    return Ok(Admission::Rejected);
                }
            }
        }
        if let Some(command) = desktop {
            if !command.valid(owner) {
                context.state["error"] = json!(
                    "The desktop command expired or its connection closed before queue mutation."
                );
                return Ok(Admission::Rejected);
            }
            let view = context
                .observations
                .project(crate::observation::Presentation {
                    queue: self.queue,
                    player: self.player,
                    proxy: self.proxy,
                    account: context.account,
                    generation: self.epochs.current,
                    confirmed: self.presented,
                    connected: context.connected,
                    playing: *self.desired_playing,
                    can_raise: context.can_raise,
                })
                .await;
            let Some(mut translated) =
                mpris::translate(&command.0.action, &view, self.queue, context.instance)
            else {
                return Ok(Admission::Rejected);
            };
            // A reset changes playback intent; Toggle must not turn the original
            // request to pause into a fresh request to resume after that reset.
            if request["op"] == "pause" {
                translated["op"] = json!("pause");
            }
            *request = translated;
        }
        // Resolve again after a reset: a socket keeps its revision, while a
        // desktop command uses the translated nominal view.
        let edit = match self.validate(request, context) {
            Ok(edit) => edit,
            Err(error) => {
                context.state["error"] = json!(error);
                return Ok(Admission::Rejected);
            }
        };
        // No await between this final fence and the admitted Queue mutation.
        if desktop.is_some_and(|command| !command.valid(owner)) {
            context.state["error"] = json!(
                "The desktop command expired or its connection closed before queue mutation."
            );
            return Ok(Admission::Rejected);
        }
        if request["op"] == "set_output" {
            self.change_output(context.output, context.state).await?;
            return Ok(Admission::Applied(Effects::default()));
        }
        if let Some(edit) = edit {
            self.apply(edit, context).await
        } else {
            Ok(Admission::Forward)
        }
    }
    fn validate(&self, request: &mut Value, context: &Context<'_>) -> Result<Option<Edit>, String> {
        let op = request["op"].as_str().unwrap_or("").to_owned();
        let effect = Effect::of(&op);
        if effect.requires_queue()
            || request.get("queue_session").is_some()
                && (effect != Effect::None || matches!(op.as_str(), "pause" | "resume"))
        {
            check_queue(self.queue, request).map_err(|e| e.to_string())?;
        }
        if matches!(op.as_str(), "retry_stream" | "skip_stream") {
            if self.preparation.recovering().phase != "exhausted"
                || check_queue(self.queue, request).is_err()
                || serde_json::from_value::<OccurrenceId>(request["occurrence"].clone()).ok()
                    != self.queue.selected()
            {
                return Err("Recovery selection changed; review the current queue.".into());
            }
            request["op"] = json!(if op == "retry_stream" { "play" } else { "next" });
            // Retry uses the retained selection; the coordinator historically
            // maps it to Play and therefore still requires the displayed track.
            return self.validate(request, context);
        }
        let edit = match op.as_str() {
            "retry_unavailable" | "skip_unavailable" => {
                let id = unavailable_request(self.queue, request)?;
                if !context.connected {
                    return Err(
                        "Restore Plex sign-in before retrying or continuing playback.".into(),
                    );
                }
                Some(if let Some(id) = id {
                    Edit::Retry(id)
                } else {
                    Edit::Next { skip: true }
                })
            }
            "navigation_select" => {
                let (index, id) = navigation_request(self.queue, request)?;
                Some(Edit::Revisit(index, id))
            }
            "reconnect" => {
                reconnect_request(self.queue, request, context.account, context.credential)?;
                None
            }
            "queue_remove" => Some(Edit::Remove(occurrence(request, "occurrence")?)),
            "queue_move" => Some(Edit::Move(
                occurrence(request, "occurrence")?,
                serde_json::from_value(request["before"].clone())
                    .map_err(|_| queue::Error::Missing.to_string())?,
            )),
            "queue_play" => {
                if !context.connected {
                    return Err("Restore Plex sign-in before playback.".into());
                }
                Some(Edit::Select(occurrence(request, "occurrence")?))
            }
            "clear_upcoming" => Some(Edit::ClearUpcoming),
            "stop_clear" => Some(Edit::StopClear),
            "next" => Some(Edit::Next { skip: false }),
            "replay" => {
                if !context.connected {
                    return Err("Restore Plex sign-in before playback.".into());
                }
                Some(Edit::Replay)
            }
            "previous" => {
                let audio = self.player.status();
                Some(Edit::Previous(
                    (audio.generation == self.epochs.current)
                        .then_some(audio.position_seconds)
                        .flatten(),
                ))
            }
            "seek" => Some(Edit::Seek(
                request["seconds"]
                    .as_f64()
                    .ok_or("Choose a valid playback position.")?,
            )),
            "set_shuffle" => Some(Edit::Shuffle(
                request["enabled"]
                    .as_bool()
                    .ok_or("Choose shuffle on or off.")?,
            )),
            "set_repeat" => Some(Edit::Repeat(
                serde_json::from_value(request["mode"].clone())
                    .map_err(|_| "Choose Off, One or All repeat.")?,
            )),
            "set_gapless" if request["enabled"].as_bool().is_none() => {
                return Err("Choose gapless on or off.".into());
            }
            "play" | "enqueue" | "play_next" => {
                if !context.connected {
                    return Err("Connect Plex before adding or playing tracks.".into());
                }
                let track = context
                    .catalog
                    .track(context.albums, request)
                    .map_err(str::to_owned)?;
                Some(if op == "play" {
                    Edit::Replace {
                        tracks: vec![track],
                        album: None,
                        batch: false,
                    }
                } else {
                    Edit::Enqueue {
                        tracks: vec![track],
                        next: op == "play_next",
                        batch: false,
                    }
                })
            }
            "search_play" | "search_next" | "search_enqueue" => {
                if !context.connected {
                    return Err("Connect Plex before using search results.".into());
                }
                let track = context
                    .search
                    .resolve(request, context.account)
                    .map_err(str::to_owned)?;
                Some(if op == "search_play" {
                    Edit::Replace {
                        tracks: vec![track],
                        album: None,
                        batch: false,
                    }
                } else {
                    Edit::Enqueue {
                        tracks: vec![track],
                        next: op == "search_next",
                        batch: false,
                    }
                })
            }
            "play_selected" | "enqueue_selected" | "play_next_selected" => {
                let tracks = context
                    .catalog
                    .selected_tracks(request, context.account, context.connected)
                    .map_err(str::to_owned)?;
                Some(if op == "play_selected" {
                    Edit::Replace {
                        tracks,
                        album: None,
                        batch: true,
                    }
                } else {
                    Edit::Enqueue {
                        tracks,
                        next: op == "play_next_selected",
                        batch: true,
                    }
                })
            }
            "album_play" | "album_enqueue" => {
                if !context.connected {
                    return Err("Connect Plex before queueing an album.".into());
                }
                let tracks = context.albums.tracks(request)?;
                Some(if op == "album_play" {
                    let album = context
                        .albums
                        .view
                        .selected
                        .as_ref()
                        .ok_or("Missing album snapshot")?;
                    Edit::Replace {
                        tracks,
                        album: Some((
                            album.album.reference.clone(),
                            album
                                .tracks
                                .iter()
                                .map(|entry| entry.track.reference.clone())
                                .collect(),
                        )),
                        batch: false,
                    }
                } else {
                    Edit::Enqueue {
                        tracks,
                        next: false,
                        batch: false,
                    }
                })
            }
            "playlist_play" | "playlist_enqueue" => {
                let tracks = playlist_tracks(context.playlists, request)?;
                if op == "playlist_play" && !context.connected {
                    return Err("Restore Plex sign-in before playback.".into());
                }
                Some(if op == "playlist_play" {
                    Edit::Replace {
                        tracks,
                        album: None,
                        batch: false,
                    }
                } else {
                    Edit::Enqueue {
                        tracks,
                        next: false,
                        batch: false,
                    }
                })
            }
            _ => None,
        };
        if let Some(edit) = &edit {
            edit.apply(&mut self.queue.clone())
                .map_err(|e| e.to_string())?;
        }
        Ok(edit)
    }
    async fn apply(
        &mut self,
        edit: Edit,
        context: &mut Context<'_>,
    ) -> Result<Admission, &'static str> {
        let selected = self.queue.selected();
        let prepare = match edit.apply(self.queue) {
            Ok(track) => track,
            Err(error) => {
                context.state["error"] = json!(error.to_string());
                return Ok(Admission::Rejected);
            }
        };
        let mut effects = Effects {
            prepare,
            ..Default::default()
        };
        match &edit {
            Edit::Replace { tracks, batch, .. } => {
                *self.desired_playing = true;
                if *batch {
                    context.state["status"] =
                        json!(format!("Applied selection of {} tracks.", tracks.len()));
                }
            }
            Edit::Enqueue { tracks, batch, .. } if *batch => {
                context.state["status"] =
                    json!(format!("Applied selection of {} tracks.", tracks.len()))
            }
            Edit::Select(_) | Edit::Retry(_) => *self.desired_playing = true,
            Edit::Next { skip: true } => {
                effects.automatic = true;
                *self.desired_playing = effects.prepare.is_some();
                if !*self.desired_playing {
                    context.state["status"] = json!(
                        "No playable entries remain. Open Unavailable to retry or remove an entry."
                    );
                }
            }
            Edit::Next { skip: false } | Edit::Replay => {
                if effects.prepare.is_none() {
                    *self.desired_playing = false;
                    context.state["status"] = json!("End of queue");
                } else if matches!(edit, Edit::Replay) {
                    *self.desired_playing = true;
                }
            }
            Edit::Remove(id) if Some(*id) == selected && effects.prepare.is_none() => {
                *self.desired_playing = false
            }
            Edit::StopClear => *self.desired_playing = false,
            Edit::Seek(_) => {
                effects.force_save = true;
                effects.seeked = true;
                if context.connected {
                    effects.prepare = self
                        .queue
                        .selected()
                        .and_then(|id| self.queue.visible_track(id).ok());
                }
            }
            _ => {}
        }
        if matches!(edit, Edit::StopClear)
            || matches!(edit, Edit::Remove(_)) && self.queue.selected().is_none()
            || matches!(edit, Edit::Next { .. }) && effects.prepare.is_none()
        {
            self.preparation.cancel();
            self.epochs.reset()?;
            self.gapless.invalidate();
            self.player.send(Command::Pause(true));
            self.player.send(Command::Stop);
            self.proxy.revoke().await;
            context.state["selection"] = Value::Null;
            context.state["audio"] = Value::Null;
        }
        context.state["intent"] = json!(if *self.desired_playing {
            "playing"
        } else {
            "paused"
        });
        Ok(Admission::Applied(effects))
    }
    async fn change_output(
        &mut self,
        settings: &output::Settings,
        state: &mut Value,
    ) -> Result<(), &'static str> {
        *self.desired_playing = false;
        self.preparation.cancel();
        match self.player.barrier().await {
            Ok(before) => {
                record_clock(
                    self.queue,
                    &before,
                    self.epochs.current,
                    self.presented,
                    self.proxy.failed().await,
                );
            }
            Err(error) => state["error"] = json!(error),
        }
        self.epochs.reset()?;
        self.player.invalidate_load(self.epochs.current);
        self.gapless.invalidate();
        self.proxy.revoke().await;
        *self.presented = None;
        if let Err(error) = self.player.configure(settings.selection.clone()).await {
            state["error"] = json!(error);
        }
        state["audio"] = Value::Null;
        state["intent"] = json!("paused");
        state["status"] = json!("Output changed; position retained. Choose Resume explicitly.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gapless::tests::{Provider, wait};
    use qoda_provider::MusicProvider;
    struct Fixture {
        queue: queue::Queue,
        player: Player,
        proxy: proxy::Proxy,
        epochs: gapless::Epochs,
        gapless: gapless::Controller,
        preparation: prepared_playback::Controller,
        playing: bool,
        presented: Option<u64>,
        state: Value,
        catalog: catalog::Controller,
        observations: crate::observation::Controller,
    }
    impl Fixture {
        async fn staged() -> Self {
            let (mut fixture, reply) = Self::preparing().await;
            fixture
                .gapless
                .ready(
                    reply,
                    &fixture.queue,
                    &fixture.epochs,
                    &fixture.player,
                    &fixture.proxy,
                )
                .await
                .unwrap();
            fixture
        }
        async fn preparing() -> (Self, gapless::Reply) {
            let provider = Provider::new();
            let mut queue = provider.queue();
            let player = Player::test_output("null").unwrap();
            let proxy = proxy::Proxy::start().await.unwrap();
            let media = provider
                .prepare(provider.source, "a".to_owned().try_into().unwrap())
                .await
                .unwrap();
            queue.update_selected(media.track, &media.version).unwrap();
            queue.confirm_presented().unwrap();
            player.send(Command::Load(1, proxy.grant(media.stream).await, 0.0));
            wait(&player, |s| s.entry_ready).await;
            let mut epochs = gapless::Epochs::default();
            epochs.reset().unwrap();
            let mut gapless = gapless::Controller::default();
            let (tx, mut rx) = tokio::sync::mpsc::channel(4);
            gapless
                .schedule(
                    &queue,
                    &mut epochs,
                    provider.clone(),
                    tx,
                    gain::Mode::Off,
                    false,
                )
                .unwrap();
            let Some(crate::Event::Gapless(reply)) = rx.recv().await else {
                panic!("No successor")
            };
            (
                Self {
                    queue,
                    player,
                    proxy,
                    epochs,
                    gapless,
                    preparation: prepared_playback::Controller::default(),
                    playing: true,
                    presented: Some(1),
                    state: json!({"intent":"playing"}),
                    catalog: catalog::Controller::default(),
                    observations: crate::observation::Controller::default(),
                },
                reply,
            )
        }
        fn request(&self, op: &str) -> Value {
            let q = self.queue.snapshot();
            json!({"instance":"fixture","op":op,"queue_session":q.session,"queue_revision":q.revision})
        }
        async fn admit(&mut self, request: &mut Value) -> bool {
            self.configured(request, &mut output::Settings::load(None), None, None)
                .await
        }
        async fn configured(
            &mut self,
            request: &mut Value,
            output: &mut output::Settings,
            desktop: Option<&Desktop>,
            owner: Option<&mpris::Service>,
        ) -> bool {
            let mut gain = gain::Settings::load(None);
            Playback {
                queue: &mut self.queue,
                player: &self.player,
                proxy: &self.proxy,
                epochs: &mut self.epochs,
                gapless: &mut self.gapless,
                preparation: &mut self.preparation,
                desired_playing: &mut self.playing,
                presented: &mut self.presented,
            }
            .admit(
                request,
                &mut Context {
                    instance: "fixture",
                    account: 1,
                    connected: true,
                    credential: true,
                    observations: &mut self.observations,
                    can_raise: false,
                    state: &mut self.state,
                    catalog: &self.catalog,
                    albums: &albums::Controller::default(),
                    playlists: &playlists::Controller::default(),
                    search: &search::Controller::default(),
                    output,
                    devices: &[],
                    gain: &mut gain,
                },
                desktop,
                owner,
            )
            .await
            .map(|admission| !matches!(admission, Admission::Rejected))
            .unwrap()
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn committed_successor_reset_promotes_observed_selection_and_rejects_stale_edit() {
        let mut f = Fixture::staged().await;
        let next = f.queue.pending()[0];
        let mut request = f.request("queue_remove");
        request["occurrence"] = json!(next);
        f.player.send(Command::Pause(false));
        wait(&f.player, |s| s.generation == 2 && !s.idle).await;
        assert!(!f.admit(&mut request).await);
        assert_eq!(f.queue.selected(), Some(next));
        assert!(f.epochs.current > 2);
        assert!(f.player.status().idle && f.player.status().paused);
        assert_eq!(f.gapless.view.phase, "review");
        assert_eq!(f.state["intent"], "paused");
        assert!(!f.playing);
    }
    #[tokio::test]
    async fn stale_instance_revision_and_invalid_payload_leave_future_output_untouched() {
        let mut f = Fixture::staged().await;
        let before = f.queue.snapshot();
        for mut request in [
            {
                let mut r = f.request("queue_remove");
                r["instance"] = json!("replaced");
                r
            },
            {
                let mut r = f.request("clear_upcoming");
                r["queue_revision"] = json!("18446744073709551615");
                r
            },
            {
                let mut r = f.request("clear_upcoming");
                r.as_object_mut().unwrap().remove("queue_session");
                r
            },
            {
                let mut r = f.request("seek");
                r["seconds"] = json!(-1);
                r
            },
            {
                let mut r = f.request("queue_move");
                r["occurrence"] = json!(f.queue.selected());
                r["before"] = Value::Null;
                r
            },
            {
                let mut r = f.request("set_gapless");
                r["enabled"] = json!("yes");
                r
            },
        ] {
            assert!(!f.admit(&mut request).await);
            assert_eq!(f.gapless.view.phase, "staged");
            assert_eq!(f.epochs.current, 1);
            assert_eq!(f.queue.snapshot().revision, before.revision);
            assert!(f.playing);
        }
    }
    #[tokio::test]
    async fn stale_catalog_selection_is_rejected_before_staged_audio_is_cancelled() {
        let mut fixture = Fixture::staged().await;
        let page = catalog::tests::page();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        fixture
            .catalog
            .begin(
                &json!({"source":page.source,"offset":"0","query":"fixture"}),
                std::sync::Arc::new(catalog::tests::Provider(page.clone())).into(),
                tx,
            )
            .unwrap();
        let Some(crate::Event::Page(reply)) = rx.recv().await else {
            panic!("No catalog page");
        };
        fixture.catalog.accept(reply, &mut fixture.state).unwrap();
        fixture.catalog.publish(&mut fixture.state);
        let mut request = fixture.request("play_selected");
        request["account_epoch"] = json!("1");
        request["catalog_revision"] = fixture.state["catalog"]["revision"].clone();
        request["tracks"] = json!([page.tracks[0].reference]);
        fixture.catalog.invalidate().unwrap();
        let before = fixture.queue.snapshot();
        assert!(!fixture.admit(&mut request).await);
        assert_eq!(fixture.gapless.view.phase, "staged");
        assert_eq!(fixture.epochs.current, 1);
        assert_eq!(fixture.queue.snapshot().revision, before.revision);
        assert!(fixture.playing);
    }
    #[tokio::test]
    async fn untouched_successor_edit_preserves_the_current_epoch_and_intent() {
        let mut f = Fixture::staged().await;
        let selected = f.queue.selected();
        let mut request = f.request("clear_upcoming");
        assert!(f.admit(&mut request).await);
        assert_eq!(f.gapless.view.phase, "idle");
        assert_eq!(f.queue.selected(), selected);
        assert_eq!(f.epochs.current, 1);
        assert!(f.playing);
        assert!(f.player.status().paused);
    }
    #[tokio::test]
    async fn native_barrier_failure_refuses_the_command_and_leaves_review_paused() {
        let mut f = Fixture::staged().await;
        let before = f.queue.snapshot();
        f.player.send(Command::Quit);
        let mut request = f.request("set_gapless");
        request["enabled"] = json!(false);
        assert!(!f.admit(&mut request).await);
        assert_eq!(f.gapless.view.phase, "review");
        assert_eq!(f.state["intent"], "paused");
        assert!(f.epochs.current > 1);
        assert!(!f.playing);
        assert!(f.state["error"].is_string());
        assert_eq!(f.queue.snapshot().revision, before.revision);
    }
    #[tokio::test]
    async fn cancelled_preparation_reply_cannot_restage_a_successor() {
        let (mut f, reply) = Fixture::preparing().await;
        let selected = f.queue.selected();
        let mut request = f.request("clear_upcoming");
        assert!(f.admit(&mut request).await);
        f.gapless
            .ready(reply, &f.queue, &f.epochs, &f.player, &f.proxy)
            .await
            .unwrap();
        assert_eq!(f.gapless.view.phase, "idle");
        assert_eq!(f.queue.selected(), selected);
        assert_eq!(f.epochs.current, 1);
    }
    #[tokio::test]
    async fn output_change_cancels_preparation_and_invalidates_old_render_ownership() {
        let mut f = Fixture::staged().await;
        let path =
            std::env::temp_dir().join(format!("qoda-command-output-{}", uuid::Uuid::new_v4()));
        let mut output = output::Settings::load(Some(path.clone()));
        output.selection = None;
        f.queue.set_position(0.25).unwrap();
        let selected = f.queue.selected();
        let mut request = f.request("set_output");
        request["mode"] = json!("desktop");
        assert!(f.configured(&mut request, &mut output, None, None).await);
        assert!(!f.preparation.preparing());
        assert!(f.epochs.current > 1);
        assert!(f.presented.is_none());
        assert!(!f.playing);
        assert_eq!(f.queue.selected(), selected);
        assert_eq!(f.state["intent"], "paused");
        assert!(f.player.status().paused);
        assert_eq!(output.selection, Some(output::Selection::Desktop));
        assert_eq!(
            f.player.load_prepared(player::Load {
                generation: 1,
                url: "not-opened:stale-fixture".into(),
                position: 0.0,
                gain: gain::Applied::off(),
                source: None,
                description: None,
                chain: false,
                output: None,
                playing: true,
            }),
            Err("Stale playback preparation rejected")
        );
        assert!(f.player.status().paused);

        std::fs::remove_dir_all(path).unwrap();
    }
    #[tokio::test]
    async fn expired_and_closed_desktop_commands_leave_the_staged_successor_untouched() {
        let mut f = Fixture::staged().await;
        let bus = crate::mpris::tests::Bus::new().await;
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let owner =
            mpris::Service::build(zbus::connection::Builder::address(bus.address.as_str()), tx)
                .await
                .unwrap();
        for closed in [false, true] {
            let (reply, receiver) = tokio::sync::oneshot::channel();
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
            let command = Desktop::new(mpris::Request {
                owner: owner.id(),
                action: mpris::Action::Loop(queue::Repeat::All),
                deadline: if closed {
                    deadline
                } else {
                    deadline - std::time::Duration::from_secs(4)
                },
                reply,
            });
            if closed {
                drop(receiver);
            }
            let mut request = f.request("set_repeat");
            request["mode"] = json!("all");
            assert!(
                !f.configured(
                    &mut request,
                    &mut output::Settings::load(None),
                    Some(&command),
                    Some(&owner)
                )
                .await
            );
            assert_eq!(f.gapless.view.phase, "staged");
            assert_eq!(f.epochs.current, 1);
            assert!(f.playing);
        }
        let (reply, receiver) = tokio::sync::oneshot::channel();
        let command = Desktop::new(mpris::Request {
            owner: owner.id(),
            action: mpris::Action::Loop(queue::Repeat::All),
            deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(3),
            reply,
        });
        let mut request = f.request("set_repeat");
        request["mode"] = json!("all");
        assert!(
            f.configured(
                &mut request,
                &mut output::Settings::load(None),
                Some(&command),
                Some(&owner)
            )
            .await
        );
        assert_eq!(request["op"], "set_repeat");
        command.complete(&f.state);
        assert!(receiver.await.unwrap().is_ok());
        owner.shutdown().await;
    }
    #[tokio::test]
    async fn desktop_expiry_and_reply_closure_during_native_reset_prevent_queue_mutation() {
        let bus = crate::mpris::tests::Bus::new().await;
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let owner =
            mpris::Service::build(zbus::connection::Builder::address(bus.address.as_str()), tx)
                .await
                .unwrap();
        for close in [true, false] {
            let mut f = Fixture::staged().await;
            f.player.send(Command::Pause(false));
            wait(&f.player, |s| s.generation == 2 && !s.idle).await;
            let (reply, receiver) = tokio::sync::oneshot::channel();
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(200);
            let command = Desktop::new(mpris::Request {
                owner: owner.id(),
                action: mpris::Action::Loop(queue::Repeat::All),
                deadline,
                reply,
            });
            assert!(command.valid(Some(&owner)));
            let mut request = f.request("set_repeat");
            request["mode"] = json!("all");
            let mut output = output::Settings::load(None);
            let accepted = {
                let admission =
                    f.configured(&mut request, &mut output, Some(&command), Some(&owner));
                tokio::pin!(admission);
                assert!(matches!(
                    futures_util::poll!(admission.as_mut()),
                    std::task::Poll::Pending
                ));
                if close {
                    drop(receiver);
                } else {
                    tokio::time::sleep_until(deadline).await;
                }
                admission.await
            };
            assert!(!accepted);
            assert_eq!(f.queue.snapshot().repeat, queue::Repeat::Off);
            assert_eq!(f.state["intent"], "paused");
            assert!(!f.playing);
        }
        owner.shutdown().await;
    }
    #[tokio::test]
    async fn invalid_command_does_not_cancel_the_staged_successor() {
        let mut f = Fixture::staged().await;
        let mut request = f.request("queue_remove");
        request["occurrence"] = json!(OccurrenceId::new());
        let before = f.queue.snapshot();
        assert!(!f.admit(&mut request).await);
        assert_eq!(f.gapless.view.phase, "staged");
        assert_eq!(f.epochs.current, 1);
        assert_eq!(f.queue.snapshot().revision, before.revision);
        assert!(f.playing);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    None,
    Current,
    QueueCurrent,
    Edit,
    Preference,
    Remove,
}
impl Effect {
    fn of(op: &str) -> Self {
        match op {
            "play" | "seek" | "stop" | "disconnect" | "forget_signin" | "quit" | "set_output" => {
                Self::Current
            }
            "play_selected" | "album_play" | "playlist_play" | "queue_play" | "replay"
            | "previous" | "navigation_select" | "retry_unavailable" | "skip_unavailable"
            | "next" | "stop_clear" | "reconnect" | "search_play" => Self::QueueCurrent,
            "queue_remove" => Self::Remove,
            "clear_upcoming" | "queue_move" | "enqueue" | "play_next" | "enqueue_selected"
            | "play_next_selected" | "set_shuffle" | "set_repeat" | "album_enqueue"
            | "playlist_enqueue" | "search_next" | "search_enqueue" => Self::Edit,
            "set_gapless" | "set_replaygain" => Self::Preference,
            _ => Self::None,
        }
    }
    fn requires_queue(self) -> bool {
        matches!(self, Self::QueueCurrent | Self::Edit | Self::Remove)
    }
    fn changes_current(self, request: &Value, queue: &queue::Queue) -> bool {
        matches!(self, Self::Current | Self::QueueCurrent)
            || self == Self::Remove
                && serde_json::from_value::<OccurrenceId>(request["occurrence"].clone()).ok()
                    == queue.selected()
    }
}
fn occurrence(request: &Value, key: &str) -> Result<OccurrenceId, String> {
    serde_json::from_value(request[key].clone()).map_err(|_| queue::Error::Missing.to_string())
}
pub(crate) fn playlist_tracks(
    controller: &playlists::Controller,
    request: &Value,
) -> Result<Vec<qoda_provider::Track>, String> {
    let reference =
        serde_json::from_value::<qoda_provider::PlaylistRef>(request["reference"].clone())
            .map_err(|_| "Choose a loaded playlist.")?;
    let snapshot = controller.checked_snapshot(
        &reference,
        request["expected_revision"].as_str().unwrap_or_default(),
    )?;
    let start = if let Some(id) = request["item_id"].as_str() {
        snapshot
            .entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or("This playlist entry changed. Refresh first.")?
    } else {
        0
    };
    snapshot.entries[start..].iter().map(|entry| entry.track.clone().ok_or_else(|| "Some playlist source references could not be resolved; the snapshot was not partially queued.".to_owned())).collect()
}
