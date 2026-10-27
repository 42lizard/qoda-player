//! Shared buffered-output reset protocol; trigger policy remains explicit.
use crate::{gapless, observation, player, proxy, queue};

pub(crate) struct Runtime<'a> {
    pub queue: &'a mut queue::Queue,
    pub player: &'a player::Player,
    pub gapless: &'a mut gapless::Controller,
    pub epochs: &'a mut gapless::Epochs,
    pub proxy: &'a proxy::Proxy,
    pub confirmed: &'a mut Option<u64>,
    pub playing: &'a mut bool,
}
pub(crate) enum Cancellation {
    Untouched,
    Reset { problem: Option<String> },
}
#[derive(Default)]
pub(crate) struct Preparation {
    pub reset: bool,
    pub problem: Option<String>,
}
#[derive(Clone, Copy)]
enum Trigger {
    Command,
    Automatic,
}
impl Runtime<'_> {
    /// Untouched speculative input does not require output invalidation. A
    /// committed successor (or unconfirmed current change) requires a barrier.
    pub async fn cancel_for_command(
        &mut self,
        changes_current: bool,
    ) -> Result<Cancellation, &'static str> {
        let snapshot = match self.gapless.cancel(self.player, self.proxy).await {
            Ok(Some(before)) => Ok(before),
            Ok(None)
                if self.gapless.chained
                    && changes_current
                    && *self.confirmed != Some(self.epochs.current) =>
            {
                self.player.barrier().await
            }
            Ok(None) => return Ok(Cancellation::Untouched),
            Err(_) => self.player.barrier().await,
        };
        let problem = self.finish(snapshot, Trigger::Command).await?;
        Ok(Cancellation::Reset { problem })
    }
    /// The observation caller decides whether a failed current grant should
    /// instead follow stream recovery. Automatic cleanup retains its own error.
    pub async fn after_failure(&mut self) -> Result<(), &'static str> {
        *self.playing = false;
        let snapshot = self.player.barrier().await;
        let _ = self.finish(snapshot, Trigger::Automatic).await?;
        Ok(())
    }
    /// Same completion entry for the coordinator and tests: classify unavailable
    /// media without resetting current output, and reset only staging failures.
    pub async fn prepared(&mut self, reply: gapless::Reply) -> Result<Preparation, &'static str> {
        let unavailable = self
            .gapless
            .unavailable_reply(&reply, self.queue, self.epochs);
        if let Err(error) = self
            .gapless
            .ready(reply, self.queue, self.epochs, self.player, self.proxy)
            .await
        {
            self.after_failure().await?;
            return Ok(Preparation {
                reset: true,
                problem: Some(error),
            });
        }
        let problem = unavailable
            .and_then(|(id, reason)| self.queue.mark_unavailable(id, reason).err())
            .map(|e| e.to_string());
        Ok(Preparation {
            problem,
            ..Default::default()
        })
    }
    async fn finish(
        &mut self,
        snapshot: Result<player::Status, &'static str>,
        trigger: Trigger,
    ) -> Result<Option<String>, &'static str> {
        *self.playing = false;
        let problem = match snapshot {
            Ok(before) => {
                let failed = self.proxy.failed().await;
                let handoff = self
                    .gapless
                    .handoff(self.queue, self.epochs, &before, self.proxy)
                    .await;
                // This consumes the barrier's historical snapshot, not a new
                // polling sample or a report/archive observation. Keep the
                // existing clock rules with the nominal observation owner.
                observation::record_clock(
                    self.queue,
                    &before,
                    self.epochs.current,
                    self.confirmed,
                    failed,
                );
                self.gapless.invalidate();
                self.proxy.revoke().await;
                // Successful-snapshot epoch exhaustion historically rejects the
                // command; automatic callers preserve their original error.
                if let Err(error) = self.epochs.reset() {
                    return Ok(Some(error.into()));
                }
                handoff.err()
            }
            Err(error) => {
                self.gapless.invalidate();
                self.proxy.revoke().await;
                // Failed-barrier epoch exhaustion remains a fatal runtime error.
                self.epochs.reset()?;
                if matches!(trigger, Trigger::Command) {
                    self.player.invalidate_load(self.epochs.current);
                    self.player.send(player::Command::Pause(true));
                }
                Some(error.into())
            }
        };
        self.gapless.review();
        Ok(problem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gapless::tests::{Provider, wait};
    use crate::{Event, gain};
    use qoda_provider::MusicProvider;
    use std::sync::Arc;

    struct Environment {
        queue: queue::Queue,
        player: player::Player,
        proxy: proxy::Proxy,
        epochs: gapless::Epochs,
        gapless: gapless::Controller,
        confirmed: Option<u64>,
        playing: bool,
        url: String,
        provider: Arc<Provider>,
    }
    impl Environment {
        async fn new(position: f64) -> Self {
            let provider = Provider::new();
            let mut queue = provider.queue();
            queue.confirm_presented().unwrap();
            let player = player::Player::test_output("null").unwrap();
            let proxy = proxy::Proxy::start().await.unwrap();
            let media = provider
                .prepare(provider.source, "a".to_owned().try_into().unwrap())
                .await
                .unwrap();
            let url = proxy.grant(media.stream).await;
            player.send(player::Command::Load(1, url.clone(), position));
            wait(&player, |s| s.entry_ready).await;
            let mut epochs = gapless::Epochs::default();
            epochs.reset().unwrap();
            Self {
                queue,
                player,
                proxy,
                epochs,
                gapless: Default::default(),
                confirmed: Some(1),
                playing: true,
                url,
                provider,
            }
        }
        async fn prepare(&mut self, provider: Arc<dyn MusicProvider>) -> gapless::Reply {
            let (tx, mut rx) = tokio::sync::mpsc::channel(8);
            self.gapless
                .schedule(
                    &self.queue,
                    &mut self.epochs,
                    provider,
                    tx,
                    gain::Mode::Off,
                    false,
                )
                .unwrap();
            let Some(Event::Gapless(reply)) = rx.recv().await else {
                panic!("No preparation")
            };
            reply
        }
        fn runtime(&mut self) -> Runtime<'_> {
            Runtime {
                queue: &mut self.queue,
                player: &self.player,
                gapless: &mut self.gapless,
                epochs: &mut self.epochs,
                proxy: &self.proxy,
                confirmed: &mut self.confirmed,
                playing: &mut self.playing,
            }
        }
    }
    #[tokio::test]
    async fn preparation_staging_failure_resets_output_and_keeps_the_selected_visit() {
        let mut env = Environment::new(0.0).await;
        let selected = env.queue.selected();
        let visit = env.queue.visit();
        let reply = env.prepare(env.provider.clone()).await;
        env.player.send(player::Command::Quit);
        let result = env.runtime().prepared(reply).await.unwrap();
        assert!(result.reset && result.problem.is_some());
        assert!(!env.playing);
        assert!(env.epochs.current > 1);
        assert_eq!(env.gapless.view.phase, "review");
        assert_eq!(env.queue.selected(), selected);
        assert_eq!(env.queue.visit(), visit);
        assert_eq!(env.confirmed, Some(1));
        assert_eq!(reqwest::get(env.url).await.unwrap().status(), 404);
    }
    #[tokio::test]
    async fn stale_preparation_completion_leaves_output_and_its_grant_untouched() {
        let mut env = Environment::new(0.0).await;
        let reply = env.prepare(env.provider.clone()).await;
        env.gapless.invalidate();
        let before = serde_json::to_value(&env.queue).unwrap();
        let result = env.runtime().prepared(reply).await.unwrap();
        assert!(!result.reset && result.problem.is_none());
        assert!(env.playing);
        assert_eq!(env.epochs.current, 1);
        assert_eq!(env.gapless.view.phase, "idle");
        assert_eq!(serde_json::to_value(&env.queue).unwrap(), before);
        assert_eq!(reqwest::get(&env.url).await.unwrap().status(), 200);
        env.proxy.revoke().await;
    }
    #[tokio::test]
    async fn unavailable_preparation_marks_only_the_successor_without_resetting_current_output() {
        use crate::recovery::tests::{Answer, fixture};
        let mut env = Environment::new(0.0).await;
        let (_, missing) = fixture(vec![Answer::Error(
            qoda_provider::ProviderError::MissingTrack,
        )]);
        let successor = env.queue.pending()[0];
        let selected = env.queue.selected();
        let visit = env.queue.visit();
        let reply = env.prepare(missing).await;
        let result = env.runtime().prepared(reply).await.unwrap();
        assert!(!result.reset && result.problem.is_none());
        assert!(env.playing);
        assert_eq!(env.epochs.current, 1);
        assert_eq!(env.queue.selected(), selected);
        assert_eq!(env.queue.visit(), visit);
        assert!(env.queue.is_unavailable(successor));
        assert!(!env.queue.selected_unavailable());
        assert_eq!(env.gapless.view.phase, "fallback");
        assert_eq!(reqwest::get(&env.url).await.unwrap().status(), 200);
        env.proxy.revoke().await;
    }
    #[tokio::test]
    async fn paused_barrier_snapshot_retains_position_without_new_confirmation() {
        let mut env = Environment::new(0.25).await;
        env.confirmed = None;
        let visit = env.queue.visit();
        assert_eq!(env.queue.position(), 0.0);
        env.runtime().after_failure().await.unwrap();
        assert!(env.queue.position() >= 0.2 && env.queue.position() <= 0.3);
        assert!(env.confirmed.is_none());
        assert_eq!(env.queue.visit(), visit);
        assert!(!env.playing);
        assert_eq!(env.gapless.view.phase, "review");
        assert_eq!(reqwest::get(env.url).await.unwrap().status(), 404);
    }
}
