//! Durable listening-session checkpoint lifecycle, above the serialized store.
use super::{Controller, Error, Reply};
use crate::{Event, queue::Queue};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Clone, Copy)]
struct Pending {
    serial: u64,
    purge: u64,
}
#[derive(Clone, Copy)]
enum State {
    Saved,
    Saving,
    Unsaved,
    Blocked,
}
/// Owns the caller-visible checkpoint protocol. The bounded worker below this
/// interface retains serialization, coalescing, security and backup policy.
pub(crate) struct Session {
    worker: Controller,
    last_requested: (qoda_core::SessionId, qoda_core::Counter, u64),
    last_purge: u64,
    purge: u64,
    pending: Option<Pending>,
    state: State,
    message: String,
    serial: u64,
    cleanup_pending: bool,
}
impl Session {
    pub async fn start(
        path: Result<PathBuf, Error>,
        events: tokio::sync::mpsc::Sender<Event>,
    ) -> (Self, Queue) {
        let (worker, initial) = Controller::start(path, events).await;
        Self::opened(worker, initial)
    }
    fn opened(worker: Controller, initial: Result<Queue, Error>) -> (Self, Queue) {
        let (queue, error) = match initial {
            Ok(queue) => (queue, None),
            Err(error) => (Queue::default(), Some(error)),
        };
        let mut session = Self {
            worker,
            last_requested: queue.durable_stamp(),
            last_purge: 0,
            purge: 0,
            pending: None,
            state: State::Saved,
            message: "Session storage ready.".into(),
            serial: 0,
            cleanup_pending: false,
        };
        if let Some(error) = error {
            session.result(Err(error));
        }
        (session, queue)
    }
    pub fn view(&self) -> Value {
        json!({"state":match self.state {
            State::Saved=>"saved", State::Saving=>"saving", State::Unsaved=>"unsaved", State::Blocked=>"blocked",
        },"message":self.message,"serial":self.serial.to_string(),"cleanup_pending":self.cleanup_pending})
    }
    /// Account ownership chooses Sign out; durable ownership retains its cleanup
    /// obligation until a matching save completes successfully.
    pub fn purge(&mut self) -> Result<(), &'static str> {
        self.purge = self
            .purge
            .checked_add(1)
            .ok_or("Cleanup generation exhausted")?;
        self.cleanup_pending = true;
        self.pending = None;
        Ok(())
    }
    pub fn checkpoint(&mut self, queue: &Queue, force: bool) {
        let stamp = queue.durable_stamp();
        if !force && stamp == self.last_requested && self.purge == self.last_purge {
            return;
        }
        // Failed submission still counts as attempted, matching manual Retry save.
        self.last_requested = stamp;
        self.last_purge = self.purge;
        self.pending = None;
        match self.worker.submit(queue.clone(), self.purge) {
            Ok(()) => {
                self.pending = Some(Pending {
                    serial: self.worker.serial(),
                    purge: self.purge,
                });
                self.state = State::Saving;
                self.message = "Saving session…".into();
            }
            Err(error) => self.result(Err(error)),
        }
    }
    pub fn accept(&mut self, reply: Reply) -> bool {
        if !self
            .pending
            .is_some_and(|pending| pending.serial == reply.serial && pending.purge == self.purge)
        {
            return false;
        }
        self.pending = None;
        self.serial = reply.serial;
        self.result(reply.result);
        true
    }
    fn result(&mut self, result: Result<Option<Error>, Error>) {
        match result {
            Ok(warning) => {
                self.state = State::Saved;
                self.cleanup_pending = false;
                self.message = warning
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "Session saved locally.".into());
            }
            Err(error) => {
                self.state = if matches!(
                    error,
                    Error::Corrupt | Error::Newer | Error::Unsafe | Error::ReopenRequired
                ) {
                    State::Blocked
                } else {
                    State::Unsaved
                };
                self.message = if self.cleanup_pending {
                    format!("Saved session cleanup is incomplete. {error}")
                } else {
                    error.to_string()
                };
            }
        }
    }
    pub async fn flush(&mut self, queue: &Queue) -> Result<Option<Error>, Error> {
        self.pending = None;
        let result = self.worker.flush(queue.clone(), self.purge).await;
        self.result(result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Event;
    use std::time::Duration;

    #[tokio::test]
    async fn unchanged_session_is_not_saved_until_an_explicit_retry_and_can_reopen() {
        let directory =
            std::env::temp_dir().join(format!("qoda-checkpoint-{}", uuid::Uuid::new_v4()));
        let path = directory.join("session.sqlite3");
        let (events, mut replies) = tokio::sync::mpsc::channel(8);
        let (mut session, queue) = Session::start(Ok(path.clone()), events.clone()).await;
        session.checkpoint(&queue, false);
        assert_eq!(session.view()["state"], "saved");
        assert!(replies.try_recv().is_err());
        session.checkpoint(&queue, true);
        assert_eq!(session.view()["state"], "saving");
        let Event::Stored(reply) = tokio::time::timeout(Duration::from_secs(3), replies.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("No completion")
        };
        assert!(session.accept(reply));
        assert_eq!(session.view()["state"], "saved");
        assert_eq!(session.view()["serial"], "1");
        session.flush(&queue).await.unwrap();
        drop(session);
        let (session, restored) = Session::start(Ok(path), events).await;
        assert_eq!(restored.snapshot().membership_count, 0);
        assert_eq!(restored.position(), 0.0);
        drop(session);
        let _ = std::fs::remove_dir_all(directory);
    }
    /// Controllable storage transport: real bounded worker input, with external
    /// completion timing chosen by the test. No alternative checkpoint policy.
    struct Writes {
        requests: std::sync::mpsc::Receiver<super::super::Request>,
    }
    impl Writes {
        fn open() -> (Session, Queue, Self) {
            let (tx, requests) = std::sync::mpsc::sync_channel(1);
            let worker = Controller { tx, serial: 0 };
            let (session, queue) = Session::opened(worker, Ok(Queue::default()));
            (session, queue, Self { requests })
        }
        fn complete(&self, result: Result<Option<Error>, Error>) -> Reply {
            let request = self.requests.try_recv().expect("No save requested");
            Reply {
                serial: request.serial,
                result,
            }
        }
    }
    #[test]
    fn newer_request_rejects_old_completion_and_accepts_latest_once() {
        let (mut session, queue, writes) = Writes::open();
        session.checkpoint(&queue, true);
        let old = writes.complete(Ok(None));
        session.checkpoint(&queue, true);
        let current = writes.complete(Ok(None));
        assert!(!session.accept(old));
        assert_eq!(session.view()["state"], "saving");
        assert_eq!(session.view()["serial"], "0");
        assert!(session.accept(current.clone()));
        assert_eq!(session.view()["serial"], "2");
        assert_eq!(session.view()["state"], "saved");
        assert!(!session.accept(current));
    }
    #[test]
    fn busy_submission_requires_explicit_retry_and_does_not_accept_older_success() {
        let (mut session, queue, writes) = Writes::open();
        session.checkpoint(&queue, true); // keep the bounded input occupied
        session.checkpoint(&queue, true);
        assert_eq!(session.view()["state"], "unsaved");
        assert!(session.view()["message"].as_str().unwrap().contains("busy"));
        let old = writes.complete(Ok(None));
        session.checkpoint(&queue, false);
        assert!(writes.requests.try_recv().is_err());
        assert!(!session.accept(old));
        session.checkpoint(&queue, true);
        assert!(session.accept(writes.complete(Ok(None))));
        assert_eq!(session.view()["state"], "saved");
        assert_eq!(session.view()["serial"], "3");
    }
    #[test]
    fn signout_cleanup_rejects_old_success_and_stays_pending_until_retried_save_commits() {
        let (mut session, queue, writes) = Writes::open();
        session.checkpoint(&queue, true);
        let before_signout = writes.complete(Ok(None));
        session.purge().unwrap();
        assert!(!session.accept(before_signout));
        assert_eq!(session.view()["cleanup_pending"], true);
        session.checkpoint(&queue, false); // purge alone forces a changed checkpoint
        assert!(session.accept(writes.complete(Err(Error::Unavailable))));
        assert_eq!(session.view()["cleanup_pending"], true);
        assert_eq!(session.view()["state"], "unsaved");
        assert!(
            session.view()["message"]
                .as_str()
                .unwrap()
                .contains("cleanup is incomplete")
        );
        session.checkpoint(&queue, false);
        assert!(writes.requests.try_recv().is_err());
        session.checkpoint(&queue, true);
        assert!(session.accept(writes.complete(Ok(Some(Error::Backup)))));
        assert_eq!(session.view()["cleanup_pending"], false);
        assert_eq!(session.view()["state"], "saved");
        assert!(
            session.view()["message"]
                .as_str()
                .unwrap()
                .contains("backup")
        );
    }
    #[tokio::test]
    async fn blocked_storage_is_preserved_and_retry_reports_its_actual_failure() {
        let (tx, mut replies) = tokio::sync::mpsc::channel(8);
        let (mut session, queue) = Session::start(Err(Error::Unsafe), tx).await;
        assert_eq!(session.view()["state"], "blocked");
        session.checkpoint(&queue, false);
        assert!(replies.try_recv().is_err());
        session.checkpoint(&queue, true);
        let Event::Stored(reply) = tokio::time::timeout(Duration::from_secs(3), replies.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("No result")
        };
        assert!(session.accept(reply));
        assert_eq!(session.view()["state"], "blocked");
        assert!(
            session.view()["message"]
                .as_str()
                .unwrap()
                .contains("preserved")
        );
    }
    #[tokio::test]
    async fn signout_flush_removes_the_saved_session_and_backups_before_reopen() {
        let directory =
            std::env::temp_dir().join(format!("qoda-checkpoint-purge-{}", uuid::Uuid::new_v4()));
        let path = directory.join("session.sqlite3");
        let (events, _) = tokio::sync::mpsc::channel(8);
        let (mut session, _) = Session::start(Ok(path.clone()), events.clone()).await;
        let queue = crate::gapless::tests::Provider::new().queue();
        session.flush(&queue).await.unwrap();
        drop(session);
        let (mut session, restored) = Session::start(Ok(path.clone()), events.clone()).await;
        assert_eq!(restored.snapshot().membership_count, 3);
        session.purge().unwrap();
        session.flush(&restored.empty_session()).await.unwrap();
        assert_eq!(session.view()["cleanup_pending"], false);
        drop(session);
        let (session, restored) = Session::start(Ok(path), events).await;
        assert_eq!(restored.snapshot().membership_count, 0);
        assert_eq!(
            std::fs::read_dir(directory.join("backups"))
                .unwrap()
                .count(),
            0
        );
        drop(session);
        let _ = std::fs::remove_dir_all(directory);
    }
}
