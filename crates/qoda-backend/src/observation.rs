//! Sampling and qualification of nominal output evidence. Sink-specific policy
//! stays with reporting, archive and desktop presentation.
use crate::{
    Event, alsa_guard, archive, buffered_reset, gapless, mpris, player, proxy, queue, reporting,
};
use qoda_provider::MusicProvider;
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub struct Scope {
    pub account: u64,
    pub provider: Option<Arc<dyn MusicProvider>>,
    pub can_raise: bool,
}
pub struct Runtime<'a> {
    pub queue: &'a mut queue::Queue,
    pub player: &'a player::Player,
    pub epochs: &'a mut gapless::Epochs,
    pub gapless: &'a mut gapless::Controller,
    pub proxy: &'a proxy::Proxy,
    pub confirmed: &'a mut Option<u64>,
    pub playing: &'a mut bool,
    pub reporting: &'a mut reporting::Controller,
    pub archive: &'a mut archive::Controller,
    pub scope: Scope,
}
/// Status and stream failure sampled together; freshness is assessed later with
/// this capture time, rather than attaching an old flag to a new native status.
pub struct Sample {
    pub audio: player::Status,
    pub failed: bool,
    pub at: Instant,
}
impl Sample {
    pub async fn barrier(
        player: &player::Player,
        proxy: &proxy::Proxy,
    ) -> Result<Self, &'static str> {
        let audio = player.barrier().await?;
        let at = Instant::now();
        let failed = proxy.failed().await;
        Ok(Self { audio, at, failed })
    }
}
pub struct Confirmation<'a> {
    pub queue: &'a mut queue::Queue,
    pub account: u64,
    pub generation: u64,
    pub confirmed: &'a mut Option<u64>,
}
/// Borrowed output facts for a fresh desktop/UI projection. Callers supply
/// identity and intent; only this owner supplies clock/failure qualification.
pub struct Presentation<'a> {
    pub queue: &'a queue::Queue,
    pub player: &'a player::Player,
    pub proxy: &'a proxy::Proxy,
    pub account: u64,
    pub generation: u64,
    pub confirmed: &'a mut Option<u64>,
    pub connected: bool,
    pub playing: bool,
    pub can_raise: bool,
}
pub struct BeforeCommand {
    pub view: mpris::View,
    pub handoff: bool,
    pub error: Option<String>,
}
pub enum Problem {
    Handoff(String),
    Audio(&'static str),
}
#[derive(Default)]
pub struct Tick {
    pub audio: player::Status,
    pub presentation: mpris::View,
    pub reporting: Value,
    pub archive: Value,
    pub changed: bool,
    pub checkpoint: bool,
    pub handoff: bool,
    pub event: Option<Event>,
    pub problem: Option<Problem>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Owner {
    account: u64,
    generation: u64,
}
struct Previous {
    audio: player::Status,
    stream_failed: bool,
    unqualified: bool,
}
#[derive(Default)]
pub struct Controller {
    clock: Clock,
    owner: Option<Owner>,
    blocked_account_epoch: Option<u64>,
    last: Option<Previous>,
    last_reporting: Value,
    last_archive: Value,
    handled_end: Option<u64>,
}
impl Controller {
    fn qualify(
        &mut self,
        sample: &Sample,
        now: Instant,
        owner: Owner,
        confirmed: &mut Option<u64>,
    ) -> bool {
        if self.owner != Some(owner) {
            if self.owner.is_some_and(|old| {
                old.account != owner.account && old.generation == owner.generation
            }) {
                self.blocked_account_epoch = Some(owner.generation);
                *confirmed = None;
            }
            if self.blocked_account_epoch != Some(owner.generation) {
                self.blocked_account_epoch = None;
            }
            if self.owner.is_some_and(|old| old.account != owner.account) {
                self.clock = Clock::default();
            }
            self.last = None;
            self.owner = Some(owner);
        }
        self.clock.observe(&sample.audio, sample.at);
        sample.failed
            || self.clock.stale(&sample.audio, now)
            || self.blocked_account_epoch == Some(owner.generation)
    }
    /// Output-reset barriers require current qualified evidence. Position can be
    /// retained without allowing a stalled clock to establish presentation.
    pub fn record_barrier(&mut self, sample: Sample, now: Instant, cx: Confirmation<'_>) -> bool {
        let failed = self.qualify(
            &sample,
            now,
            Owner {
                account: cx.account,
                generation: cx.generation,
            },
            cx.confirmed,
        );
        record_clock(cx.queue, &sample.audio, cx.generation, cx.confirmed, failed)
    }
    fn projection(sample: &Sample, failed: bool, cx: &Runtime<'_>) -> mpris::View {
        mpris::View::capture(
            cx.queue,
            &sample.audio,
            cx.epochs.current,
            *cx.confirmed,
            cx.scope.provider.is_some(),
            *cx.playing,
            failed,
        )
        .with_ui(cx.scope.can_raise)
    }
    async fn sample(cx: &Runtime<'_>) -> Sample {
        let audio = cx.player.status();
        let at = Instant::now();
        let failed = cx.proxy.failed().await;
        Sample { audio, at, failed }
    }
    pub async fn project(&mut self, cx: Presentation<'_>) -> mpris::View {
        let audio = cx.player.status();
        let at = Instant::now();
        let failed = cx.proxy.failed().await;
        let sample = Sample { audio, at, failed };
        let failed = self.qualify(
            &sample,
            Instant::now(),
            Owner {
                account: cx.account,
                generation: cx.generation,
            },
            cx.confirmed,
        );
        mpris::View::capture(
            cx.queue,
            &sample.audio,
            cx.generation,
            *cx.confirmed,
            cx.connected,
            cx.playing,
            failed,
        )
        .with_ui(cx.can_raise)
    }
    pub async fn before_command(&mut self, cx: &mut Runtime<'_>) -> BeforeCommand {
        let sample = Self::sample(cx).await;
        let (handoff, error) = match cx
            .gapless
            .handoff(cx.queue, cx.epochs, &sample.audio, cx.proxy)
            .await
        {
            Ok(handoff) => (handoff, None),
            Err(error) => {
                *cx.playing = false;
                cx.player.send(player::Command::Pause(true));
                cx.gapless.review();
                (false, Some(error))
            }
        };
        let failed = self.qualify(
            &sample,
            Instant::now(),
            Owner {
                account: cx.scope.account,
                generation: cx.epochs.current,
            },
            cx.confirmed,
        );
        if handoff {
            record_clock(
                cx.queue,
                &sample.audio,
                cx.epochs.current,
                cx.confirmed,
                sample.failed || self.blocked_account_epoch == Some(cx.epochs.current),
            );
            self.handled_end = None;
        }
        BeforeCommand {
            view: Self::projection(&sample, failed, cx),
            handoff,
            error,
        }
    }
    pub async fn tick(&mut self, cx: &mut Runtime<'_>) -> Result<Tick, &'static str> {
        let mut sample = Self::sample(cx).await;
        let mut handoff_changed = false;
        let mut handoff = false;
        let mut problem = None;
        match cx
            .gapless
            .handoff(cx.queue, cx.epochs, &sample.audio, cx.proxy)
            .await
        {
            Ok(true) => {
                handoff_changed = true;
                handoff = true;
                self.handled_end = None;
            }
            Ok(false) if sample.audio.generation <= cx.epochs.current => {}
            result => {
                if !cx.proxy.failed().await {
                    buffered_reset::Runtime {
                        queue: cx.queue,
                        player: cx.player,
                        gapless: cx.gapless,
                        epochs: cx.epochs,
                        proxy: cx.proxy,
                        confirmed: cx.confirmed,
                        playing: cx.playing,
                    }
                    .after_failure()
                    .await?;
                    problem=Some(Problem::Handoff(result.err().unwrap_or_else(||"An unbound native transition was reset; review the selected occurrence.".into())));
                    handoff_changed = true;
                }
            }
        }
        // Handoff/reset may revoke grants. Keep the failure paired with the
        // resulting output scope before any observer consumes it.
        sample.failed = cx.proxy.failed().await;
        let mut tick = self.observe(sample, Instant::now(), cx);
        tick.checkpoint |= handoff_changed;
        tick.handoff = handoff;
        if tick.problem.is_none() {
            tick.problem = problem;
        }
        if let Some(Event::StreamFailed(error, _)) = &mut tick.event {
            *error = cx
                .proxy
                .failure()
                .await
                .unwrap_or(qoda_provider::ProviderError::Network);
        }
        Ok(tick)
    }
    /// Shared production fan-out for native samples and synthetic status adapters.
    /// Reporting deliberately sees prior confirmation; archive and display see
    /// confirmation established by this observation, matching the prior ordering.
    pub fn observe(&mut self, sample: Sample, now: Instant, cx: &mut Runtime<'_>) -> Tick {
        let failed = self.qualify(
            &sample,
            now,
            Owner {
                account: cx.scope.account,
                generation: cx.epochs.current,
            },
            cx.confirmed,
        );
        let previous_failed = self
            .last
            .as_ref()
            .is_some_and(|previous| previous.stream_failed);
        let interrupted = sample.failed
            && !previous_failed
            && alsa_guard::error().is_none()
            && self.blocked_account_epoch != Some(cx.epochs.current);
        let was_playing = *cx.playing;
        cx.reporting
            .bind(cx.scope.account, cx.scope.provider.clone());
        cx.reporting.fence_generation(cx.epochs.current);
        cx.reporting.observe_stop(&sample.audio);
        cx.reporting.observe(
            reporting::Observation::capture(
                cx.queue,
                &sample.audio,
                reporting::Context {
                    account: cx.scope.account,
                    generation: cx.epochs.current,
                    confirmed: *cx.confirmed,
                    failed_or_stale: failed,
                },
            ),
            now,
        );
        let reporting = cx.reporting.view();
        let archive = serde_json::json!(cx.archive.view());
        let changed = self.last.as_ref().is_none_or(|previous| {
            previous.audio != sample.audio
                || previous.stream_failed != sample.failed
                || previous.unqualified != failed
        }) || reporting != self.last_reporting
            || archive != self.last_archive;
        let mut checkpoint = false;
        let mut problem = None;
        if changed {
            checkpoint = record_clock(
                cx.queue,
                &sample.audio,
                cx.epochs.current,
                cx.confirmed,
                sample.failed || self.blocked_account_epoch == Some(cx.epochs.current),
            );
            cx.gapless.observed(&sample.audio, *cx.confirmed);
            cx.archive.observe(
                cx.queue,
                &sample.audio,
                cx.epochs.current,
                *cx.confirmed,
                failed,
                now,
            );
            if let Some(error) = sample
                .audio
                .error
                .filter(|_| sample.audio.generation == cx.epochs.current)
                .or(sample
                    .failed
                    .then_some("The media stream was interrupted. Retry the selected track."))
            {
                *cx.playing = false;
                cx.player.send(player::Command::Pause(true));
                problem = Some(Problem::Audio(error));
            }
        }
        let presentation = Self::projection(&sample, failed, cx);
        let event = if interrupted {
            // The asynchronous tick resolves the provider-specific failure below.
            Some(Event::StreamFailed(
                qoda_provider::ProviderError::Network,
                was_playing,
            ))
        } else if sample.audio.generation == cx.epochs.current
            && sample.audio.unsupported_format
            && !sample.audio.transcoded
            && !sample.failed
            && self.handled_end != Some(cx.epochs.current)
        {
            self.handled_end = Some(cx.epochs.current);
            Some(Event::Unsupported(cx.epochs.current, was_playing))
        } else if sample.audio.generation == cx.epochs.current
            && sample.audio.idle
            && sample.audio.natural_eof
            && self.handled_end != Some(cx.epochs.current)
            && sample.audio.error.is_none()
            && !sample.failed
            && self.blocked_account_epoch != Some(cx.epochs.current)
        {
            self.handled_end = Some(cx.epochs.current);
            Some(Event::Advance(cx.epochs.current))
        } else {
            None
        };
        self.last = Some(Previous {
            audio: sample.audio.clone(),
            stream_failed: sample.failed,
            unqualified: failed,
        });
        self.last_reporting = reporting.clone();
        self.last_archive = archive.clone();
        Tick {
            audio: sample.audio,
            presentation,
            reporting,
            archive,
            changed,
            checkpoint,
            handoff: false,
            event,
            problem,
        }
    }
}
pub(crate) fn record_clock(
    queue: &mut queue::Queue,
    audio: &player::Status,
    generation: u64,
    presented: &mut Option<u64>,
    failed: bool,
) -> bool {
    if audio.generation != generation {
        return false;
    }
    if let Some(position) = audio.position_seconds {
        let _ = queue.set_position(position);
    }
    if audio
        .position_seconds
        .is_some_and(|position| position > 0.0)
        && !audio.paused
        && !audio.idle
        && audio.error.is_none()
        && !failed
    {
        *presented = Some(generation);
        return queue.confirm_presented().unwrap_or(false);
    }
    false
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

#[cfg(test)]
mod tests;
