//! Private nominal observations, independent of Previous and provider reports.
use crate::{player, queue};
use qoda_core::{OccurrenceId, SessionId, TrackRef};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::oneshot;
mod store;
#[cfg(test)]
mod tests;
type Result<T> = std::result::Result<T, &'static str>;
const LIMIT: usize = 10000;
const PAGE: usize = 20;
const INTERVAL: Duration = Duration::from_secs(5);
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub visit: uuid::Uuid,
    pub session: SessionId,
    pub occurrence: OccurrenceId,
    pub track: TrackRef,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub first_observed_ms: u64,
    pub last_observed_ms: u64,
    pub last_position_ms: u64,
    pub max_position_ms: u64,
}
impl Entry {
    fn valid(&self) -> bool {
        !self.visit.is_nil()
            && [&self.title, &self.artist, &self.album]
                .iter()
                .all(|s| s.len() <= 1024)
            && String::from(self.track.key.clone()).len() <= 512
            && self.first_observed_ms <= self.last_observed_ms
            && self.last_position_ms <= self.max_position_ms
            && self.max_position_ms <= 1_000_000_000_000
    }
    fn same_visit(&self, other: &Self) -> bool {
        self.visit == other.visit
            && self.session == other.session
            && self.occurrence == other.occurrence
            && self.track == other.track
    }
    fn capture(
        queue: &queue::Queue,
        audio: &player::Status,
        generation: u64,
        confirmed: Option<u64>,
        failed_or_stale: bool,
        wall: u64,
    ) -> Option<Self> {
        if failed_or_stale
            || confirmed != Some(generation)
            || audio.generation != generation
            || audio.paused
            || audio.idle
            || !audio.entry_ready
            || audio.error.is_some()
            || queue.ended()
        {
            return None;
        }
        let position = audio
            .position_seconds
            .filter(|p| p.is_finite() && *p > 0.0 && *p <= 1.0e9)?;
        let occurrence = queue.selected()?;
        let track = queue.visible_track(occurrence).ok()?;
        let position = (position * 1000.0).round() as u64;
        let position = track
            .duration_ms
            .map_or(position, |duration| position.min(duration.get()));
        let entry = Self {
            visit: queue.visit(),
            session: queue.durable_stamp().0,
            occurrence,
            track: track.reference,
            title: bounded(track.title),
            artist: bounded(track.artist),
            album: bounded(track.album),
            first_observed_ms: wall,
            last_observed_ms: wall,
            last_position_ms: position,
            max_position_ms: position,
        };
        entry.valid().then_some(entry)
    }
}
// Display snapshots are intentionally bounded; enduring track identity is intact.
fn bounded(mut text: String) -> String {
    if text.len() > 1024 {
        let mut end = 1024;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct View {
    pub available: bool,
    pub cleanup_pending: bool,
    pub incomplete: bool,
    pub retention_days: u16,
    pub total: usize,
    pub offset: usize,
    pub entries: Vec<Entry>,
    pub has_next: bool,
    pub message: String,
    pub export_path: Option<String>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            available: false,
            cleanup_pending: false,
            incomplete: false,
            retention_days: 90,
            total: 0,
            offset: 0,
            entries: Vec::new(),
            has_next: false,
            message: "Opening listening archive…".into(),
            export_path: None,
        }
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(u128::from(u64::MAX)) as u64)
}
enum Operation {
    Observe(Entry),
    Bind(String),
    Clear(bool),
    Retention(u16),
    Page(usize),
    Export(bool),
    Flush,
}
struct Request {
    operation: Operation,
    done: Option<oneshot::Sender<Result<()>>>,
}
pub struct Controller {
    tx: mpsc::SyncSender<Request>,
    view: Arc<Mutex<View>>,
    active: bool,
    last: Option<(uuid::Uuid, u64, Instant)>,
    privacy_blocked: bool,
    dropped: bool,
}
impl Controller {
    pub fn start(path: Result<PathBuf>) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Request>(8);
        let view = Arc::new(Mutex::new(View::default()));
        let shared = view.clone();
        std::thread::spawn(move || {
            let opened = path.and_then(|path| store::Store::open(&path, now_ms()));
            let (mut store, error) = match opened {
                Ok(store) => (Some(store), None),
                Err(error) => (None, Some(error)),
            };
            if let Some(store) = store.as_mut() {
                match store.view(0) {
                    Ok(view) => *shared.lock().unwrap() = view,
                    Err(error) => shared.lock().unwrap().message = error.into(),
                }
            } else {
                shared.lock().unwrap().message = error.unwrap().into();
            }
            let mut offset = 0;
            let mut exported = None;
            let mut incomplete = false;
            while let Ok(request) = rx.recv() {
                let result: Result<()> = (|| {
                    let store = store.as_mut().ok_or(error.unwrap_or(
                        "Listening archive unavailable; restart after repairing storage.",
                    ))?;
                    match request.operation {
                        Operation::Observe(entry) => {
                            if let Err(error) = store.observe(entry, now_ms()) {
                                incomplete = true;
                                return Err(error);
                            }
                        }
                        Operation::Bind(account) => {
                            if store.bind(&account)? {
                                incomplete = false;
                            }
                            offset = 0;
                            exported = None;
                        }
                        Operation::Clear(unbind) => {
                            store.clear(unbind)?;
                            incomplete = false;
                            offset = 0;
                            exported = None;
                        }
                        Operation::Retention(days) => {
                            store.retention(days, now_ms())?;
                            if days == 0 {
                                incomplete = false;
                            }
                            offset = 0;
                        }
                        Operation::Page(page) => offset = page,
                        Operation::Export(dropped) => {
                            exported = Some(store.export(now_ms(), incomplete || dropped)?)
                        }
                        Operation::Flush if incomplete => {
                            return Err(
                                "Some nominal archive observations were not saved in this run; retained entries are incomplete.",
                            );
                        }
                        Operation::Flush => {}
                    }
                    let mut view = store.view(offset)?;
                    view.export_path = exported.clone();
                    view.incomplete = incomplete;
                    if incomplete {
                        view.message.push_str(" Some observations were not saved in this run; retained entries are incomplete.");
                    }
                    *shared.lock().unwrap() = view;
                    Ok(())
                })();
                if let Err(error) = result {
                    let mut view = shared.lock().unwrap();
                    view.message = error.into();
                    view.incomplete = incomplete;
                }
                if let Some(done) = request.done {
                    let _ = done.send(result);
                }
            }
        });
        Self {
            tx,
            view,
            active: false,
            last: None,
            privacy_blocked: false,
            dropped: false,
        }
    }
    pub fn view(&self) -> View {
        let mut view = self.view.lock().unwrap().clone();
        if self.privacy_blocked {
            view.entries.clear();
            view.total = 0;
            view.has_next = false;
            view.export_path = None;
            view.available = false;
            view.cleanup_pending = true;
            view.message="Archive account cleanup/binding is incomplete. Old account data is hidden; repair storage and reconnect or retry Disconnect.".into();
        }
        if self.dropped {
            view.incomplete = true;
            view.message.push_str(
                " Some observations were dropped in this run; retained entries are incomplete.",
            );
        }
        if !self.active && view.available {
            view.message.push_str(" Connect Plex to record visits.");
        }
        view
    }
    pub fn cleanup_pending(&self) -> bool {
        self.privacy_blocked
    }
    async fn command(&mut self, operation: Operation) -> Result<()> {
        let (done, reply) = oneshot::channel();
        self.tx
            .try_send(Request {
                operation,
                done: Some(done),
            })
            .map_err(|_| "Listening archive is busy; retry the action.")?;
        tokio::time::timeout(Duration::from_secs(3), reply)
            .await
            .map_err(
                |_| "Listening archive action is still pending; check its status before retrying.",
            )?
            .map_err(|_| "Listening archive worker stopped.")?
    }
    pub async fn bind(&mut self, account: String) -> Result<()> {
        self.active = false;
        self.last = None;
        self.privacy_blocked = true;
        self.command(Operation::Bind(account)).await?;
        self.active = true;
        self.privacy_blocked = false;
        Ok(())
    }
    pub async fn clear(&mut self, unbind: bool) -> Result<()> {
        self.last = None;
        if unbind {
            self.active = false;
            self.privacy_blocked = true;
        }
        self.command(Operation::Clear(unbind)).await?;
        self.privacy_blocked = false;
        self.dropped = false;
        Ok(())
    }
    pub async fn retention(&mut self, days: u16) -> Result<()> {
        self.last = None;
        self.command(Operation::Retention(days)).await?;
        if days == 0 {
            self.dropped = false;
        }
        Ok(())
    }
    pub async fn page(&mut self, offset: usize) -> Result<()> {
        self.command(Operation::Page(offset)).await
    }
    pub fn export(&mut self) -> Result<()> {
        self.tx
            .try_send(Request {
                operation: Operation::Export(self.dropped),
                done: None,
            })
            .map_err(|_| "Listening archive is busy; retry export.")?;
        self.view.lock().unwrap().message = "Exporting a private archive copy…".into();
        Ok(())
    }
    pub async fn flush(&mut self) -> Result<()> {
        self.command(Operation::Flush).await?;
        if self.dropped {
            return Err("Some nominal archive observations were dropped in this run.");
        }
        Ok(())
    }
    pub fn observe(
        &mut self,
        queue: &queue::Queue,
        audio: &player::Status,
        generation: u64,
        confirmed: Option<u64>,
        failed_or_stale: bool,
        now: Instant,
    ) {
        if !self.active {
            return;
        }
        let Some(entry) = Entry::capture(
            queue,
            audio,
            generation,
            confirmed,
            failed_or_stale,
            now_ms(),
        ) else {
            return;
        };
        if self.last.as_ref().is_some_and(|(visit, render, at)| {
            *visit == entry.visit
                && *render == generation
                && now.saturating_duration_since(*at) < INTERVAL
        }) {
            return;
        }
        let visit = entry.visit;
        if self
            .tx
            .try_send(Request {
                operation: Operation::Observe(entry),
                done: None,
            })
            .is_ok()
        {
            self.last = Some((visit, generation, now));
        } else {
            self.dropped = true;
            self.view.lock().unwrap().message =
                "Listening archive is busy; some observations have not been saved.".into();
        }
    }
}
pub fn path() -> Result<PathBuf> {
    crate::storage::path()
        .map(|p| p.parent().unwrap().join("archive"))
        .map_err(|_| "Listening archive path unavailable.")
}
