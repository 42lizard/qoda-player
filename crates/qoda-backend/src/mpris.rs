//! MPRIS is a projection and command client of the coordinator, never a player owner.
use crate::{player, queue};
use qoda_core::{OccurrenceId, SessionId};
use serde_json::{Value as Json, json};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};
use zbus::{Connection, ObjectPath, Variant as Value, connection::Builder, fdo};

pub const NAME: &str = "org.mpris.MediaPlayer2.qoda";
const PATH: &str = "/org/mpris/MediaPlayer2";
const IFACE: &str = "org.mpris.MediaPlayer2.Player";
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Play,
    Pause,
    Toggle,
    Stop,
    Next,
    Previous,
    Seek(i64),
    SetPosition(String, i64),
    Shuffle(bool),
    Loop(queue::Repeat),
    Quit,
}
pub struct Request {
    pub action: Action,
    pub deadline: tokio::time::Instant,
    pub reply: oneshot::Sender<Result<(), String>>,
}
impl Request {
    pub fn is_live(&self) -> bool {
        !self.reply.is_closed() && self.deadline > tokio::time::Instant::now()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Track {
    path: String,
    title: String,
    artist: String,
    album: String,
    length: Option<i64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    status: &'static str,
    track: Option<Track>,
    position: i64,
    intent: bool,
    can_play: bool,
    can_pause: bool,
    can_next: bool,
    can_previous: bool,
    can_seek: bool,
    shuffled: bool,
    repeat: queue::Repeat,
    seek_serial: u64,
    generation: u64,
}
impl Default for View {
    fn default() -> Self {
        Self {
            status: "Stopped",
            track: None,
            position: 0,
            intent: false,
            can_play: false,
            can_pause: false,
            can_next: false,
            can_previous: false,
            can_seek: false,
            shuffled: false,
            repeat: queue::Repeat::Off,
            seek_serial: 0,
            generation: 0,
        }
    }
}
pub fn track_id(session: SessionId, occurrence: OccurrenceId) -> String {
    format!(
        "/org/qoda/session/s{}/track/t{}",
        uuid::Uuid::from(session).simple(),
        uuid::Uuid::from(occurrence).simple()
    )
}
fn micros(seconds: f64) -> i64 {
    if !seconds.is_finite() {
        return 0;
    }
    (seconds.clamp(0.0, 1.0e9) * 1.0e6).round() as i64
}
/// A stalled positive timestamp must not indefinitely identify current presentation.
#[derive(Default)]
pub struct Clock {
    generation: Option<u64>,
    position: Option<f64>,
    progress: Option<std::time::Instant>,
}
impl Clock {
    pub fn observe(&mut self, audio: &player::Status, now: std::time::Instant) {
        if self.generation != Some(audio.generation) {
            self.generation = Some(audio.generation);
            self.position = None;
            self.progress = None;
        }
        if let Some(position) = audio.position_seconds.filter(|p| p.is_finite() && *p > 0.0)
            && self.position != Some(position)
        {
            self.position = Some(position);
            self.progress = Some(now);
        }
    }
    pub fn stale(&self, audio: &player::Status, now: std::time::Instant) -> bool {
        !audio.paused
            && (self.generation != Some(audio.generation)
                || self.progress.is_none_or(|progress| {
                    now.saturating_duration_since(progress) > Duration::from_millis(250)
                }))
    }
}
impl View {
    /// Shared nominal presentation for UI surfaces, including when D-Bus is unavailable.
    pub fn presentation(&self) -> Json {
        json!({
            "known": self.track.is_some(),
            "status": self.status,
            "track": self.track.as_ref().map(|track| json!({
                "id": track.path, "title": track.title, "artist": track.artist,
                "album": track.album, "duration_seconds": track.length.map(|length| length as f64 / 1.0e6)
            })),
            "position_seconds": self.track.as_ref().map(|_| self.position as f64 / 1.0e6),
            "can_play": self.can_play, "can_pause": self.can_pause,
            "can_next": self.can_next, "can_previous": self.can_previous,
            "can_seek": self.can_seek
        })
    }
    pub fn capture(
        queue: &queue::Queue,
        audio: &player::Status,
        generation: u64,
        confirmed: Option<u64>,
        connected: bool,
        intent: bool,
        failed: bool,
    ) -> Self {
        let q = queue.snapshot();
        let has_pending = q.entries.iter().any(|row| !row.selected);
        let can_play = connected && (has_pending || q.selected.is_some() && !q.ended);
        let mut view = Self {
            generation,
            intent,
            can_play,
            can_pause: can_play,
            can_next: !q.entries.is_empty(),
            shuffled: q.shuffled,
            repeat: q.repeat,
            ..Default::default()
        };
        // A selected/prepared file and restored last-known position are not current evidence.
        let known = connected
            && confirmed == Some(generation)
            && audio.generation == generation
            && !audio.idle
            && audio.entry_ready
            && audio.error.is_none()
            && !failed
            && !q.ended
            && (audio
                .position_seconds
                .is_some_and(|position| position.is_finite() && position >= 0.0)
                || audio.paused && audio.position_seconds.is_none());
        if known && let Some(row) = q.entries.iter().find(|row| row.selected) {
            view.position = micros(audio.position_seconds.unwrap_or(q.position_seconds));
            view.track = Some(Track {
                path: track_id(q.session, row.id),
                title: row.track.title.clone(),
                artist: row.track.artist.clone(),
                album: row.track.album.clone(),
                length: row
                    .track
                    .duration_ms
                    .and_then(|ms| ms.get().checked_mul(1000))
                    .and_then(|us| i64::try_from(us).ok()),
            });
            view.status = if audio.paused || !intent {
                "Paused"
            } else {
                "Playing"
            };
            view.can_seek = true;
            view.can_previous = q.previous_available;
        }
        view
    }
    fn loop_status(&self) -> &'static str {
        match self.repeat {
            queue::Repeat::Off => "None",
            queue::Repeat::One => "Track",
            queue::Repeat::All => "Playlist",
        }
    }
    fn metadata(&self) -> HashMap<String, Value<'static>> {
        let mut metadata = HashMap::new();
        if let Some(track) = &self.track {
            metadata.insert(
                "mpris:trackid".into(),
                Value::from(
                    ObjectPath::try_from(track.path.clone()).expect("generated occurrence path"),
                ),
            );
            metadata.insert("xesam:title".into(), Value::from(track.title.clone()));
            metadata.insert("xesam:album".into(), Value::from(track.album.clone()));
            metadata.insert(
                "xesam:artist".into(),
                Value::from(vec![track.artist.clone()]),
            );
            if let Some(length) = track.length {
                metadata.insert("mpris:length".into(), Value::from(length));
            }
        }
        metadata
    }
    fn changes(&self, old: &Self) -> HashMap<&'static str, Value<'static>> {
        let mut changed = HashMap::new();
        if self.status != old.status {
            changed.insert("PlaybackStatus", Value::from(self.status));
        }
        if self.track != old.track {
            changed.insert("Metadata", Value::from(self.metadata()));
        }
        for (key, new, previous) in [
            ("CanPlay", self.can_play, old.can_play),
            ("CanPause", self.can_pause, old.can_pause),
            ("CanSeek", self.can_seek, old.can_seek),
            ("CanGoNext", self.can_next, old.can_next),
            ("CanGoPrevious", self.can_previous, old.can_previous),
            ("Shuffle", self.shuffled, old.shuffled),
        ] {
            if new != previous {
                changed.insert(key, Value::from(new));
            }
        }
        if self.repeat != old.repeat {
            changed.insert("LoopStatus", Value::from(self.loop_status()));
        }
        changed
    }
}
/// Resolve against current coordinator state, not an earlier desktop property read.
pub fn translate(
    action: &Action,
    view: &View,
    queue: &queue::Queue,
    instance: &str,
) -> Option<Json> {
    let q = queue.snapshot();
    let mut request = match action {
        Action::Play if view.can_play => json!({"op":"resume"}),
        Action::Pause if view.can_pause => json!({"op":"pause"}),
        Action::Toggle if view.can_pause => json!({"op":if view.intent {"pause"} else {"resume"}}),
        Action::Stop => json!({"op":"stop"}),
        Action::Next if view.can_next => json!({"op":"next"}),
        Action::Previous if view.can_previous => json!({"op":"previous"}),
        Action::Shuffle(enabled) => json!({"op":"set_shuffle","enabled":enabled}),
        Action::Loop(mode) => json!({"op":"set_repeat","mode":mode}),
        Action::Quit => json!({"op":"quit"}),
        Action::Seek(offset) if view.can_seek => {
            let position = (i128::from(view.position) + i128::from(*offset)).max(0);
            if view
                .track
                .as_ref()
                .and_then(|t| t.length)
                .is_some_and(|length| position > i128::from(length))
            {
                json!({"op":"next"})
            } else {
                json!({"op":"seek","seconds":position.min(1_000_000_000_000_000) as f64/1.0e6})
            }
        }
        Action::SetPosition(path, position) if view.can_seek => {
            let track = view.track.as_ref()?;
            if *position < 0
                || *path != track.path
                || track.length.is_some_and(|length| *position > length)
            {
                return None;
            }
            json!({"op":"seek","seconds":(*position as f64/1.0e6).min(1.0e9)})
        }
        _ => return None,
    };
    request["instance"] = json!(instance);
    request["queue_session"] = json!(q.session);
    request["queue_revision"] = json!(q.revision);
    Some(request)
}
#[derive(Clone)]
struct Remote {
    view: Arc<RwLock<View>>,
    tx: mpsc::Sender<crate::Event>,
}
impl Remote {
    fn view(&self) -> View {
        self.view.read().expect("MPRIS projection lock").clone()
    }
    async fn request(&self, action: Action) -> fdo::Result<()> {
        let (reply, rx) = oneshot::channel();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        tokio::time::timeout_at(deadline, async {
            self.tx
                .send(crate::Event::Desktop(Request {
                    action,
                    deadline,
                    reply,
                }))
                .await
                .map_err(|_| fdo::Error::Failed("Qoda is shutting down.".into()))?;
            rx.await
                .map_err(|_| {
                    fdo::Error::Failed("Qoda stopped before acknowledging the command.".into())
                })?
                .map_err(fdo::Error::Failed)
        })
        .await
        .map_err(|_| fdo::Error::Failed("The desktop command timed out.".into()))?
    }
}
struct Root(Remote);
#[zbus::interface(name = "org.mpris.MediaPlayer2", spawn = false)]
impl Root {
    async fn quit(&self) -> fdo::Result<()> {
        self.0.request(Action::Quit).await
    }
    fn raise(&self) {}
    #[zbus(property)]
    fn identity(&self) -> &str {
        "Qoda Player"
    }
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec![]
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        vec![]
    }
}
struct Player(Remote);
#[zbus::interface(name = "org.mpris.MediaPlayer2.Player", spawn = false)]
impl Player {
    async fn play(&self) -> fdo::Result<()> {
        self.0.request(Action::Play).await
    }
    async fn pause(&self) -> fdo::Result<()> {
        self.0.request(Action::Pause).await
    }
    async fn play_pause(&self) -> fdo::Result<()> {
        if !self.0.view().can_pause {
            return Err(fdo::Error::NotSupported(
                "There is no playable queue selection.".into(),
            ));
        }
        self.0.request(Action::Toggle).await
    }
    async fn stop(&self) -> fdo::Result<()> {
        self.0.request(Action::Stop).await
    }
    async fn next(&self) -> fdo::Result<()> {
        self.0.request(Action::Next).await
    }
    async fn previous(&self) -> fdo::Result<()> {
        self.0.request(Action::Previous).await
    }
    async fn seek(&self, offset: i64) -> fdo::Result<()> {
        self.0.request(Action::Seek(offset)).await
    }
    async fn set_position(&self, track_id: ObjectPath<'_>, position: i64) -> fdo::Result<()> {
        self.0
            .request(Action::SetPosition(track_id.to_string(), position))
            .await
    }
    fn open_uri(&self, _uri: &str) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported(
            "Choose music through Qoda's provider browser.".into(),
        ))
    }
    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.0.view().status.into()
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, Value<'static>> {
        self.0.view().metadata()
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.0.view().position
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.0.view().can_play
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.0.view().can_pause
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.0.view().can_seek
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.0.view().can_next
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.0.view().can_previous
    }
    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn loop_status(&self) -> String {
        self.0.view().loop_status().into()
    }
    #[zbus(property)]
    async fn set_loop_status(&self, value: &str) -> fdo::Result<()> {
        let mode = match value {
            "None" => queue::Repeat::Off,
            "Track" => queue::Repeat::One,
            "Playlist" => queue::Repeat::All,
            _ => {
                return Err(fdo::Error::InvalidArgs(
                    "Choose None, Track or Playlist.".into(),
                ));
            }
        };
        self.0.request(Action::Loop(mode)).await
    }
    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.0.view().shuffled
    }
    #[zbus(property)]
    async fn set_shuffle(&self, enabled: bool) -> fdo::Result<()> {
        self.0.request(Action::Shuffle(enabled)).await
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn set_rate(&self, value: f64) -> fdo::Result<()> {
        if value == 1.0 {
            Ok(())
        } else {
            Err(fdo::Error::NotSupported(
                "Qoda supports normal playback speed only.".into(),
            ))
        }
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn set_volume(&self, value: f64) -> fdo::Result<()> {
        if value == 1.0 {
            Ok(())
        } else {
            Err(fdo::Error::NotSupported(
                "Use system output volume; Qoda does not apply player volume.".into(),
            ))
        }
    }
}
pub struct Service {
    connection: Connection,
    shared: Arc<RwLock<View>>,
    updates: watch::Sender<View>,
    task: tokio::task::JoinHandle<()>,
    pending_seek: Option<(String, u64)>,
    seek_serial: u64,
}
impl Service {
    pub async fn start(tx: mpsc::Sender<crate::Event>) -> zbus::Result<Self> {
        Self::build(Builder::session(), tx).await
    }
    async fn build(builder: Builder<'_>, tx: mpsc::Sender<crate::Event>) -> zbus::Result<Self> {
        let initial = View::default();
        let shared = Arc::new(RwLock::new(initial.clone()));
        let remote = Remote {
            view: shared.clone(),
            tx: tx.clone(),
        };
        let connection = builder
            .name(NAME)
            .allow_name_replacements(false)
            .replace_existing_names(false)
            .serve_at(PATH, Root(remote.clone()))
            .serve_at(PATH, Player(remote))
            .build()
            .await?;
        let (updates, mut rx) = watch::channel(initial.clone());
        let bus = connection.clone();
        let task = tokio::spawn(async move {
            let mut old = initial;
            loop {
                tokio::select! {
                    _ = bus.closed() => { let _ = tx.send(crate::Event::DesktopLost).await; break; }
                    changed = rx.changed() => { if changed.is_err() { break; } }
                }
                let next = rx.borrow_and_update().clone();
                let changed = next.changes(&old);
                if !changed.is_empty()
                    && bus
                        .emit_signal(
                            None::<&str>,
                            PATH,
                            "org.freedesktop.DBus.Properties",
                            "PropertiesChanged",
                            &(IFACE, changed, Vec::<String>::new()),
                        )
                        .await
                        .is_err()
                {
                    let _ = tx.send(crate::Event::DesktopLost).await;
                    break;
                }
                if next.seek_serial != old.seek_serial
                    && bus
                        .emit_signal(None::<&str>, PATH, IFACE, "Seeked", &(next.position,))
                        .await
                        .is_err()
                {
                    let _ = tx.send(crate::Event::DesktopLost).await;
                    break;
                }
                old = next;
            }
        });
        Ok(Self {
            connection,
            shared,
            updates,
            task,
            pending_seek: None,
            seek_serial: 0,
        })
    }
    pub fn mark_seek(&mut self, queue: &queue::Queue, generation: u64) {
        let q = queue.snapshot();
        self.pending_seek = q.selected.map(|id| (track_id(q.session, id), generation));
    }
    pub fn publish(&mut self, mut view: View) {
        if let Some((path, generation)) = &self.pending_seek {
            if *generation != view.generation {
                self.pending_seek = None;
            } else if let Some(track) = &view.track {
                if &track.path == path {
                    self.seek_serial = self.seek_serial.saturating_add(1);
                }
                self.pending_seek = None;
            }
        }
        view.seek_serial = self.seek_serial;
        *self.shared.write().expect("MPRIS projection lock") = view.clone();
        self.updates.send_replace(view);
    }
    pub async fn shutdown(self) {
        self.task.abort();
        let _ = self.connection.release_name(NAME).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use qoda_core::{Counter, SourceId, TrackRef};
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncBufReadExt;
    fn fixture() -> (queue::Queue, player::Status) {
        let track = qoda_provider::Track {
            reference: TrackRef {
                source: SourceId::new(),
                key: "1".to_owned().try_into().unwrap(),
            },
            title: "Synthetic track".into(),
            artist: "Fixture".into(),
            album: "Fixture album".into(),
            duration_ms: Some(Counter::new(10_000)),
        };
        let mut queue = queue::Queue::default();
        queue.replace(track).unwrap();
        queue.set_position(2.5).unwrap();
        (
            queue,
            player::Status {
                generation: 7,
                position_seconds: Some(2.5),
                entry_ready: true,
                idle: false,
                paused: false,
                natural_eof: false,
                error: None,
                ..Default::default()
            },
        )
    }
    fn view(queue: &queue::Queue, audio: &player::Status) -> View {
        View::capture(queue, audio, 7, Some(7), true, true, false)
    }
    #[test]
    fn stalled_clock_evidence_expires_and_known_pause_freezes_without_refreshing_progress() {
        let (_, mut audio) = fixture();
        let now = std::time::Instant::now();
        let mut clock = Clock::default();
        assert!(clock.stale(&audio, now));
        clock.observe(&audio, now);
        assert!(!clock.stale(&audio, now));
        let later = now + Duration::from_millis(300);
        clock.observe(&audio, later);
        assert!(clock.stale(&audio, later));
        audio.paused = true;
        assert!(!clock.stale(&audio, later));
        audio.paused = false;
        audio.position_seconds = Some(3.0);
        clock.observe(&audio, later);
        assert!(!clock.stale(&audio, later));
        audio.generation = 8;
        audio.position_seconds = None;
        clock.observe(&audio, later);
        assert!(clock.stale(&audio, later));
    }
    #[test]
    fn metadata_requires_nominal_evidence_and_keeps_known_pause_position_only() {
        let (queue, mut audio) = fixture();
        for (generation, confirmed, connected, failed) in [
            (7, None, true, false),
            (8, Some(7), true, false),
            (7, Some(7), false, false),
            (7, Some(7), true, true),
        ] {
            let v = View::capture(
                &queue, &audio, generation, confirmed, connected, true, failed,
            );
            assert_eq!(v.status, "Stopped");
            assert!(v.metadata().is_empty());
            assert!(!v.can_seek);
        }
        for position in [f64::NAN, f64::INFINITY, -0.1] {
            audio.position_seconds = Some(position);
            assert!(view(&queue, &audio).metadata().is_empty());
        }
        audio.position_seconds = Some(2.5);
        let v = view(&queue, &audio);
        assert_eq!(v.status, "Playing");
        assert!(v.can_seek);
        assert_eq!(v.position, 2_500_000);
        assert!(ObjectPath::try_from(v.track.as_ref().unwrap().path.as_str()).is_ok());
        audio.position_seconds = None;
        assert!(view(&queue, &audio).metadata().is_empty());
        audio.paused = true;
        let paused = view(&queue, &audio);
        assert_eq!(paused.status, "Paused");
        assert_eq!(paused.position, 2_500_000);
        assert!(!paused.metadata().is_empty());
        audio.idle = true;
        assert!(view(&queue, &audio).metadata().is_empty());
    }
    #[test]
    fn occurrence_and_session_paths_fence_duplicate_track_seeks() {
        let (mut queue, audio) = fixture();
        let old = view(&queue, &audio).track.unwrap().path;
        let id = queue.selected().unwrap();
        let track = queue.visible_track(id).unwrap();
        queue.enqueue(track.clone(), false).unwrap();
        queue.advance().unwrap();
        let v = view(&queue, &audio);
        let new = v.track.as_ref().unwrap().path.clone();
        assert_ne!(old, new);
        assert!(translate(&Action::SetPosition(old, 1_000_000), &v, &queue, "fixture").is_none());
        let request = translate(
            &Action::SetPosition(new.clone(), 1_000_000),
            &v,
            &queue,
            "fixture",
        )
        .unwrap();
        assert_eq!(request["seconds"], 1.0);
        queue.replace(track).unwrap();
        assert_ne!(new, view(&queue, &audio).track.unwrap().path);
    }
    #[test]
    fn signed_seek_boundaries_and_latest_queue_fences_are_preserved() {
        let (mut queue, audio) = fixture();
        let v = view(&queue, &audio);
        let path = v.track.as_ref().unwrap().path.clone();
        assert_eq!(
            translate(&Action::Seek(i64::MIN), &v, &queue, "fixture").unwrap()["seconds"],
            0.0
        );
        assert_eq!(
            translate(&Action::Seek(i64::MAX), &v, &queue, "fixture").unwrap()["op"],
            "next"
        );
        for position in [-1, 10_000_001, i64::MAX] {
            assert!(
                translate(
                    &Action::SetPosition(path.clone(), position),
                    &v,
                    &queue,
                    "fixture"
                )
                .is_none()
            );
        }
        queue.set_shuffle(true).unwrap();
        let request = translate(&Action::Next, &v, &queue, "fixture").unwrap();
        let snapshot = queue.snapshot();
        assert_eq!(request["queue_revision"], json!(snapshot.revision));
        assert_eq!(request["queue_session"], json!(snapshot.session));
        assert_eq!(request["instance"], "fixture");
    }
    #[test]
    fn capabilities_and_toggle_follow_current_intent_without_autoplaying_modes() {
        let (queue, audio) = fixture();
        let mut v = view(&queue, &audio);
        assert_eq!(
            translate(&Action::Toggle, &v, &queue, "fixture").unwrap()["op"],
            "pause"
        );
        v.intent = false;
        assert_eq!(
            translate(&Action::Toggle, &v, &queue, "fixture").unwrap()["op"],
            "resume"
        );
        assert_eq!(
            translate(&Action::Loop(queue::Repeat::One), &v, &queue, "fixture").unwrap()["op"],
            "set_repeat"
        );
        let restored = View::capture(&queue, &audio, 7, None, false, false, false);
        assert!(translate(&Action::Play, &restored, &queue, "fixture").is_none());
        assert!(translate(&Action::Seek(1), &restored, &queue, "fixture").is_none());
        assert!(translate(&Action::Previous, &restored, &queue, "fixture").is_none());
    }
    #[test]
    fn ui_presentation_uses_the_same_nominal_identity_and_uncertainty_as_desktop_metadata() {
        let (queue, mut audio) = fixture();
        let known = view(&queue, &audio).presentation();
        assert_eq!(known["known"], true);
        assert_eq!(known["status"], "Playing");
        assert_eq!(known["position_seconds"], audio.position_seconds.unwrap());
        assert_eq!(
            known["track"]["id"],
            view(&queue, &audio).track.unwrap().path
        );
        audio.paused = true;
        assert_eq!(view(&queue, &audio).presentation()["status"], "Paused");
        for uncertain in [
            View::capture(&queue, &audio, 7, None, true, false, false),
            View::capture(&queue, &audio, 8, Some(7), true, false, false),
            View::capture(&queue, &audio, 7, Some(7), true, false, true),
        ] {
            let state = uncertain.presentation();
            assert_eq!(state["known"], false);
            assert!(state["track"].is_null());
            assert!(state["position_seconds"].is_null());
            assert_eq!(state["can_seek"], false);
        }
        audio.entry_ready = false;
        assert_eq!(view(&queue, &audio).presentation()["known"], false);
    }
    #[test]
    fn position_ticks_do_not_emit_properties_changed_but_transport_and_metadata_do() {
        let (queue, mut audio) = fixture();
        let old = view(&queue, &audio);
        audio.position_seconds = Some(3.0);
        let next = view(&queue, &audio);
        assert!(next.changes(&old).is_empty());
        audio.paused = true;
        let paused = view(&queue, &audio);
        assert!(paused.changes(&next).contains_key("PlaybackStatus"));
        audio.idle = true;
        let stopped = view(&queue, &audio);
        assert!(stopped.changes(&paused).contains_key("Metadata"));
        assert_eq!(stopped.position, 0);
    }
    #[test]
    fn expired_or_abandoned_desktop_requests_cannot_gain_late_authority() {
        let (reply, rx) = oneshot::channel();
        let request = Request {
            action: Action::Toggle,
            deadline: tokio::time::Instant::now() + Duration::from_secs(1),
            reply,
        };
        assert!(request.is_live());
        drop(rx);
        assert!(!request.is_live());
        let (reply, _rx) = oneshot::channel();
        let request = Request {
            action: Action::Play,
            deadline: tokio::time::Instant::now() - Duration::from_secs(1),
            reply,
        };
        assert!(!request.is_live());
    }
    struct Bus {
        child: tokio::process::Child,
        directory: std::path::PathBuf,
        address: String,
    }
    impl Bus {
        async fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("qoda-mpris-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&directory).unwrap();
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
            let config = directory.join("bus.conf");
            // Deliberately no servicedir or standard_session_servicedirs: no portal activation.
            std::fs::write(&config,format!("<busconfig><type>session</type><listen>unix:path={}/bus</listen><auth>EXTERNAL</auth><policy context='default'><allow user='{}'/><allow own='*'/><allow send_destination='*'/><allow receive_sender='*'/></policy></busconfig>",directory.display(),unsafe{libc::getuid()})).unwrap();
            let mut child = tokio::process::Command::new("/usr/bin/dbus-daemon")
                .arg("--nofork")
                .arg("--print-address=1")
                .arg(format!("--config-file={}", config.display()))
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let mut line = String::new();
            let mut read = tokio::io::BufReader::new(child.stdout.take().unwrap());
            tokio::time::timeout(Duration::from_secs(3), read.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            assert!(!line.trim().is_empty());
            Self {
                child,
                directory,
                address: line.trim().to_owned(),
            }
        }
    }
    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.child.start_kill();
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    async fn proxy<'a>(connection: &'a Connection, interface: &str) -> zbus::Proxy<'a> {
        zbus::proxy::Builder::new(connection)
            .destination(NAME)
            .path(PATH)
            .interface(interface.to_owned())
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn dbus_methods_properties_signatures_and_confirmed_signals_use_the_coordinator() {
        let bus = Bus::new().await;
        let (tx, mut rx) = mpsc::channel(16);
        let mut service = Service::build(Builder::address(bus.address.as_str()), tx)
            .await
            .unwrap();
        let (queue, audio) = fixture();
        service.publish(view(&queue, &audio));
        let shared = service.shared.clone();
        let updates = service.updates.clone();
        let actions = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = actions.clone();
        let actor = tokio::spawn(async move {
            while let Some(crate::Event::Desktop(request)) = rx.recv().await {
                assert!(request.is_live());
                seen.lock().unwrap().push(request.action.clone());
                let mut v = shared.read().unwrap().clone();
                match request.action {
                    Action::Pause => {
                        v.intent = false;
                        v.status = "Paused"
                    }
                    Action::Toggle => {
                        v.intent = !v.intent;
                        v.status = if v.intent { "Playing" } else { "Paused" }
                    }
                    Action::Shuffle(enabled) => v.shuffled = enabled,
                    Action::Loop(mode) => v.repeat = mode,
                    Action::Seek(offset) => {
                        v.position += offset;
                        v.seek_serial += 1
                    }
                    _ => {}
                }
                *shared.write().unwrap() = v.clone();
                updates.send_replace(v);
                let _ = request.reply.send(Ok(()));
            }
        });
        let client = Builder::address(bus.address.as_str())
            .build()
            .await
            .unwrap();
        let p = proxy(&client, IFACE).await;
        let root = proxy(&client, "org.mpris.MediaPlayer2").await;
        assert_eq!(
            root.get_property::<String>("Identity").await.unwrap(),
            "Qoda Player"
        );
        assert!(!root.get_property::<bool>("CanRaise").await.unwrap());
        assert!(!root.get_property::<bool>("HasTrackList").await.unwrap());
        assert_eq!(
            p.get_property::<String>("PlaybackStatus").await.unwrap(),
            "Playing"
        );
        assert_eq!(p.get_property::<i64>("Position").await.unwrap(), 2_500_000);
        let metadata: HashMap<String, zbus::OwnedVariant> =
            p.get_property("Metadata").await.unwrap();
        assert_eq!(metadata.len(), 5);
        let _: () = p.call("Pause", &()).await.unwrap();
        assert_eq!(
            p.get_property::<String>("PlaybackStatus").await.unwrap(),
            "Paused"
        );
        let _: () = p.call("PlayPause", &()).await.unwrap();
        assert_eq!(
            p.get_property::<String>("PlaybackStatus").await.unwrap(),
            "Playing"
        );
        p.set_property("Shuffle", true).await.unwrap();
        assert!(p.get_property::<bool>("Shuffle").await.unwrap());
        p.set_property("LoopStatus", "Playlist").await.unwrap();
        assert_eq!(
            p.get_property::<String>("LoopStatus").await.unwrap(),
            "Playlist"
        );
        assert!(p.set_property("LoopStatus", "invalid").await.is_err());
        assert!(p.set_property("Volume", 0.5_f64).await.is_err());
        assert!(p.set_property("Rate", 2.0_f64).await.is_err());
        assert_eq!(p.get_property::<f64>("Volume").await.unwrap(), 1.0);
        assert!(
            p.call::<_, _, ()>("OpenUri", &("file:///not-opened",))
                .await
                .is_err()
        );
        let mut signal = p.receive_signal("Seeked").await.unwrap();
        let _: () = p.call("Seek", &(1_000_000_i64,)).await.unwrap();
        let message = tokio::time::timeout(Duration::from_secs(2), signal.next())
            .await
            .unwrap()
            .unwrap();
        let (position,): (i64,) = message.body().deserialize().unwrap();
        assert_eq!(position, 3_500_000);
        let _: () = p.call("Next", &()).await.unwrap();
        let _: () = p.call("Previous", &()).await.unwrap();
        let _: () = p.call("Stop", &()).await.unwrap();
        let _: () = root.call("Quit", &()).await.unwrap();
        assert_eq!(
            &actions.lock().unwrap()[..4],
            &[
                Action::Pause,
                Action::Toggle,
                Action::Shuffle(true),
                Action::Loop(queue::Repeat::All)
            ]
        );
        service.shutdown().await;
        actor.abort();
    }
    #[tokio::test]
    async fn an_existing_bus_owner_is_not_replaced_and_shutdown_releases_the_name() {
        let bus = Bus::new().await;
        let (tx, _rx) = mpsc::channel(4);
        let service = Service::build(Builder::address(bus.address.as_str()), tx.clone())
            .await
            .unwrap();
        let client = Builder::address(bus.address.as_str())
            .build()
            .await
            .unwrap();
        let dbus = zbus::Proxy::new(
            &client,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await
        .unwrap();
        let before: String = dbus.call("GetNameOwner", &(NAME,)).await.unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_secs(2),
                Service::build(Builder::address(bus.address.as_str()), tx)
            )
            .await
            .unwrap()
            .is_err()
        );
        let after: String = dbus.call("GetNameOwner", &(NAME,)).await.unwrap();
        assert_eq!(before, after);
        service.shutdown().await;
        assert!(
            !dbus
                .call::<_, _, bool>("NameHasOwner", &(NAME,))
                .await
                .unwrap()
        );
    }
    #[tokio::test]
    async fn seek_signals_wait_for_a_matching_nominal_clock_and_old_generations_are_cancelled() {
        let bus = Bus::new().await;
        let (tx, _rx) = mpsc::channel(4);
        let mut service = Service::build(Builder::address(bus.address.as_str()), tx)
            .await
            .unwrap();
        let (queue, audio) = fixture();
        service.mark_seek(&queue, 7);
        service.publish(View::capture(&queue, &audio, 7, None, true, true, false));
        assert_eq!(service.seek_serial, 0);
        service.publish(view(&queue, &audio));
        assert_eq!(service.seek_serial, 1);
        service.mark_seek(&queue, 7);
        service.publish(View::capture(&queue, &audio, 8, None, true, false, false));
        assert!(service.pending_seek.is_none());
        service.publish(view(&queue, &audio));
        assert_eq!(service.seek_serial, 1);
        service.shutdown().await;
    }
    #[tokio::test]
    async fn losing_the_private_bus_is_reported_without_affecting_the_player_owner() {
        let mut bus = Bus::new().await;
        let (tx, mut rx) = mpsc::channel(4);
        let service = Service::build(Builder::address(bus.address.as_str()), tx)
            .await
            .unwrap();
        bus.child.kill().await.unwrap();
        service.connection.closed().await;
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap(),
            Some(crate::Event::DesktopLost)
        ));
        service.shutdown().await;
    }
}
