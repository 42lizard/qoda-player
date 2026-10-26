//! Reporting is a bounded observer, never playback or queue authority.
use crate::{player, queue};
use qoda_core::{OccurrenceId, SessionId, TrackRef};
use qoda_provider::{
    MusicProvider,
    playback::{Outcome, Report, State},
};
use std::{
    collections::VecDeque,
    num::NonZeroU64,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Notify, watch};

const INTERVAL: Duration = Duration::from_secs(10);
const CAPACITY: usize = 8;
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    account: u64,
    session: SessionId,
    occurrence: OccurrenceId,
    generation: u64,
    track: TrackRef,
}
pub struct Context {
    pub account: u64,
    pub generation: u64,
    pub confirmed: Option<u64>,
    pub failed_or_stale: bool,
}
pub struct Observation(Option<Sample>);
struct Sample {
    key: Key,
    duration: NonZeroU64,
    position: Option<u64>,
    paused: bool,
}
impl Observation {
    pub fn capture(queue: &queue::Queue, audio: &player::Status, context: Context) -> Self {
        let sample = (|| {
            if context.failed_or_stale
                || context.confirmed != Some(context.generation)
                || audio.generation != context.generation
                || audio.idle
                || !audio.entry_ready
                || audio.error.is_some()
                || queue.ended()
            {
                return None;
            }
            let occurrence = queue.selected()?;
            let track = queue.visible_track(occurrence).ok()?;
            let duration = NonZeroU64::new(track.duration_ms?.get())?;
            let position = audio
                .position_seconds
                .filter(|p| p.is_finite() && *p > 0.0)
                .map(|p| ((p * 1000.0).round() as u64).min(duration.get()));
            if !audio.paused && position.is_none() {
                return None;
            }
            Some(Sample {
                key: Key {
                    account: context.account,
                    session: queue.durable_stamp().0,
                    occurrence,
                    generation: context.generation,
                    track: track.reference,
                },
                duration,
                position,
                paused: audio.paused,
            })
        })();
        Self(sample)
    }
}
struct Owner {
    key: Key,
    report: Report,
    last_update: Instant,
}
#[derive(Default)]
struct Tracker {
    owner: Option<Owner>,
}
impl Tracker {
    fn stop(&mut self, continuing: bool) -> Option<Report> {
        self.owner.take().map(|mut owner| {
            owner.report.state = State::Stopped;
            owner.report.continuing = continuing;
            owner.report
        })
    }
    fn observe(&mut self, sample: Sample, now: Instant) -> Vec<Report> {
        let mut reports = Vec::new();
        if self
            .owner
            .as_ref()
            .is_none_or(|owner| owner.key != sample.key)
        {
            // A restored/prepared/paused successor cannot establish a new owner.
            if sample.paused || sample.position.is_none() {
                return reports;
            }
            if let Some(stop) = self.stop(true) {
                reports.push(stop);
            }
            self.owner = Some(Owner {
                report: Report {
                    track: sample.key.track.clone(),
                    session: uuid::Uuid::new_v4(),
                    state: State::Playing,
                    position_ms: sample.position.unwrap(),
                    duration_ms: sample.duration,
                    continuing: false,
                },
                key: sample.key,
                last_update: now,
            });
            reports.push(self.owner.as_ref().unwrap().report.clone());
            return reports;
        }
        let owner = self.owner.as_mut().unwrap();
        // Native pause acknowledgement keeps the last qualified playing position;
        // a stale decoder/preparation timestamp never advances a paused owner.
        let state = if sample.paused {
            State::Paused
        } else {
            State::Playing
        };
        if !sample.paused {
            owner.report.position_ms = sample.position.unwrap();
        }
        if state != owner.report.state
            || now.saturating_duration_since(owner.last_update) >= INTERVAL
        {
            owner.report.state = state;
            owner.last_update = now;
            reports.push(owner.report.clone());
        }
        reports
    }
}
#[derive(Default)]
struct Mailbox {
    reports: VecDeque<Report>,
    suspended: bool,
    busy: bool,
}
struct Dispatch {
    mailbox: Arc<Mutex<Mailbox>>,
    notify: Arc<Notify>,
    status: watch::Receiver<&'static str>,
    task: tokio::task::JoinHandle<()>,
}
impl Dispatch {
    fn new(provider: Arc<dyn MusicProvider>, serial: Arc<tokio::sync::Mutex<()>>) -> Self {
        let mailbox = Arc::new(Mutex::new(Mailbox::default()));
        let notify = Arc::new(Notify::new());
        let (status_tx, status) = watch::channel("ready");
        let (pending, wake) = (mailbox.clone(), notify.clone());
        let task = tokio::spawn(async move {
            loop {
                let notified = wake.notified();
                let report = {
                    let mut mailbox = pending.lock().unwrap();
                    let report = mailbox.reports.pop_front();
                    mailbox.busy = report.is_some();
                    report
                };
                let Some(report) = report else {
                    notified.await;
                    continue;
                };
                let result = tokio::time::timeout(Duration::from_secs(3), async {
                    // Shared across canceled/replacement workers. A new account
                    // or render cannot dispatch before the old future releases.
                    let _serial = serial.lock().await;
                    provider.report_playback(report).await
                })
                .await;
                let status = match result {
                    Ok(Ok(Outcome::Accepted)) => "accepted",
                    Ok(Ok(Outcome::ServerTermination)) => {
                        let mut mailbox = pending.lock().unwrap();
                        mailbox.suspended = true;
                        mailbox.reports.clear();
                        "server_termination"
                    }
                    _ => "unavailable",
                };
                status_tx.send_replace(status);
                pending.lock().unwrap().busy = false;
                // No automatic retries or durable replay of failed reports.
            }
        });
        Self {
            mailbox,
            notify,
            status,
            task,
        }
    }
    fn submit(&self, report: Report) -> bool {
        let mut mailbox = self.mailbox.lock().unwrap();
        if mailbox.suspended {
            return false;
        }
        if let Some(last) = mailbox.reports.back_mut()
            && last.session == report.session
            && last.state == report.state
        {
            *last = report;
        } else if mailbox.reports.len() < CAPACITY {
            mailbox.reports.push_back(report);
        } else {
            mailbox.suspended = true;
            mailbox.reports.clear();
            return false;
        }
        self.notify.notify_one();
        true
    }
}
impl Drop for Dispatch {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub struct Controller {
    enabled: bool,
    account: u64,
    provider: Option<Arc<dyn MusicProvider>>,
    tracker: Tracker,
    dispatch: Option<Dispatch>,
    congested: bool,
    stopping: bool,
    generation: Option<u64>,
    serial: Arc<tokio::sync::Mutex<()>>,
}
impl Controller {
    pub fn new() -> Self {
        // Deliberate build-time gate. #16 must qualify real history effects.
        Self::with_policy(true) // VALIDATION BRANCH ONLY: owned fixture, no speaker output
    }
    fn with_policy(enabled: bool) -> Self {
        Self {
            enabled,
            account: 0,
            provider: None,
            tracker: Tracker::default(),
            dispatch: None,
            congested: false,
            stopping: false,
            generation: None,
            serial: Arc::default(),
        }
    }
    pub fn bind(&mut self, account: u64, provider: Option<Arc<dyn MusicProvider>>) {
        let same = match (&self.provider, &provider) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if self.account == account && same {
            return;
        }
        // Abort the old owned worker before binding another account/provider.
        // No sign-out terminal report is sent using revoked credentials.
        self.dispatch = None;
        self.tracker = Tracker::default();
        self.generation = None;
        self.stopping = false;
        self.congested = false;
        self.account = account;
        self.provider = provider;
        if self.enabled
            && let Some(provider) = &self.provider
        {
            self.dispatch = Some(Dispatch::new(provider.clone(), self.serial.clone()));
        }
    }
    pub fn fence_generation(&mut self, generation: u64) {
        if self.generation == Some(generation) {
            return;
        }
        let replacing = self.generation.replace(generation).is_some();
        if replacing {
            // Cancel stale positions before the successor has a nominal clock.
            // A later terminal report still belongs to the predecessor.
            if self
                .dispatch
                .as_ref()
                .is_some_and(|dispatch| dispatch.mailbox.lock().unwrap().suspended)
            {
                self.congested = true;
            }
            self.dispatch = None;
            if self.enabled
                && !self.congested
                && let Some(provider) = &self.provider
            {
                self.dispatch = Some(Dispatch::new(provider.clone(), self.serial.clone()));
            }
        }
    }
    fn submit(&mut self, reports: impl IntoIterator<Item = Report>) {
        for report in reports {
            if self
                .dispatch
                .as_ref()
                .is_some_and(|dispatch| !dispatch.submit(report))
            {
                self.congested = true;
                self.dispatch = None;
                self.tracker = Tracker::default();
                break;
            }
        }
    }
    pub fn observe(&mut self, observation: Observation, now: Instant) {
        if !self.enabled || self.congested || self.provider.is_none() {
            return;
        }
        if let Some(sample) = observation.0
            && sample.key.account == self.account
            && self
                .generation
                .is_none_or(|generation| sample.key.generation == generation)
        {
            if !sample.paused
                && self
                    .tracker
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.key != sample.key)
            {
                self.stopping = false;
            }
            let reports = self.tracker.observe(sample, now);
            self.submit(reports);
        }
    }
    pub fn request_stop(&mut self) {
        self.stopping = self.tracker.owner.is_some();
    }
    pub fn observe_stop(&mut self, audio: &player::Status) {
        if self.stopping
            && audio.idle
            && !audio.natural_eof
            && audio.error.is_none()
            && self
                .tracker
                .owner
                .as_ref()
                .is_some_and(|owner| owner.key.generation == audio.generation)
        {
            self.stopping = false;
            let stop = self.tracker.stop(false);
            self.submit(stop);
        }
    }
    pub async fn finish(&mut self, player: &player::Player) {
        if self.dispatch.is_none() {
            return;
        }
        // A successful native reset acknowledges stop; native EOF alone does not.
        if player.barrier().await.is_ok() {
            let stop = self.tracker.stop(false);
            self.submit(stop);
            if let Some(dispatch) = &self.dispatch {
                let _ = tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        let drained = {
                            let mailbox = dispatch.mailbox.lock().unwrap();
                            mailbox.reports.is_empty() && !mailbox.busy
                        };
                        if drained {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await;
            }
        }
        self.dispatch = None;
    }
    pub fn view(&self) -> serde_json::Value {
        let state = if !self.enabled {
            "disabled_pending_live_qualification"
        } else if self.congested {
            "suspended"
        } else if let Some(dispatch) = &self.dispatch {
            *dispatch.status.borrow()
        } else {
            "disconnected"
        };
        serde_json::json!({"state":state})
    }
}

#[cfg(test)]
mod tests;
