//! User-requested cache maintenance and its application operation lifetime.
use crate::{Event, account, albums, artwork, catalog, metadata_cache, offline, search};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{sync::mpsc, task::JoinHandle};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Artwork,
    Metadata,
}
impl Target {
    fn index(self) -> usize {
        match self {
            Self::Artwork => 0,
            Self::Metadata => 1,
        }
    }
    fn field(self) -> &'static str {
        match self {
            Self::Artwork => "cache_action",
            Self::Metadata => "metadata_cache_action",
        }
    }
}
#[derive(Clone, Copy)]
enum Action {
    Clear,
    Budget(u16),
}
impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Budget(_) => "budget",
        }
    }
    fn removes_discovery(self) -> bool {
        matches!(self, Self::Clear | Self::Budget(0))
    }
}
pub struct Reply {
    id: u64,
    target: Target,
    account: u64,
    result: Result<(), String>,
}
struct Pending {
    id: u64,
    account: u64,
    action: Action,
    task: JoinHandle<()>,
}
struct Presentation {
    id: u64,
    previous: Value,
}
#[derive(Default)]
pub struct Controller {
    serial: u64,
    pending: [Option<Pending>; 2],
    presentation: Option<Presentation>,
}
pub struct Runtime<'a> {
    pub state: &'a mut Value,
    pub account: &'a mut account::Controller,
    pub catalog: &'a mut catalog::Controller,
    pub albums: &'a mut albums::Controller,
    pub search: &'a mut search::Controller,
    pub artwork: &'a artwork::Artwork,
    pub metadata: &'a metadata_cache::Cache,
    pub events: &'a mpsc::Sender<Event>,
}
fn parse(request: &Value) -> Result<(Target, Action), &'static str> {
    let (target, clear, maximum) = match request["op"].as_str() {
        Some("clear_artwork_cache") => (Target::Artwork, true, 4096),
        Some("set_artwork_budget") => (Target::Artwork, false, 4096),
        Some("clear_metadata_cache") => (Target::Metadata, true, 1024),
        Some("set_metadata_budget") => (Target::Metadata, false, 1024),
        _ => return Err("Unsupported cache operation."),
    };
    if clear {
        return Ok((target, Action::Clear));
    }
    let mib = request["mib"]
        .as_u64()
        .filter(|n| *n <= maximum)
        .ok_or(match target {
            Target::Artwork => "Choose an artwork cache target between 0 and 4096 MiB.",
            Target::Metadata => "Choose a metadata target from 0 to 1024 MiB.",
        })?;
    Ok((target, Action::Budget(mib as u16)))
}
impl Controller {
    /// A temporary maintenance status belongs to one operation. Foreground work
    /// relinquishes it before producing its own status; no text is interpreted.
    pub fn observe_activity(&mut self, event: &Event, state: &mut Value) {
        let intervenes = match event {
            Event::Request(request) => !matches!(
                request["op"].as_str(),
                Some(
                    "clear_artwork_cache"
                        | "set_artwork_budget"
                        | "clear_metadata_cache"
                        | "set_metadata_budget"
                )
            ),
            Event::Maintenance(_)
            | Event::Checkpoint
            | Event::Artwork
            | Event::OutputDevices(_)
            | Event::Stored(_)
            | Event::UiPeer(..)
            | Event::UiLaunchDone(..)
            | Event::DesktopLost(_)
            | Event::DesktopReady(_) => false,
            _ => true,
        };
        if intervenes {
            self.release_presentation(state);
        }
    }
    fn release_presentation(&mut self, state: &mut Value) {
        if let Some(presentation) = self.presentation.take() {
            state["status"] = presentation.previous;
        }
    }
    pub fn publish(&self, state: &mut Value) {
        for target in [Target::Artwork, Target::Metadata] {
            state[target.field()] = self.pending[target.index()]
                .as_ref()
                .map(|p| json!(p.action.name()))
                .unwrap_or(Value::Null);
        }
    }
    pub async fn command(
        &mut self,
        request: &Value,
        cx: &mut Runtime<'_>,
    ) -> Result<(), &'static str> {
        let (target, action) = match parse(request) {
            Ok(parsed) => parsed,
            Err(error) => {
                cx.state["error"] = json!(error);
                return Ok(());
            }
        };
        if self.pending[target.index()].is_some() {
            cx.state["error"] = json!(match target {
                Target::Artwork => "Wait for the current artwork cache operation.",
                Target::Metadata => "Wait for the current metadata cache operation.",
            });
            return Ok(());
        }
        let id = self
            .serial
            .checked_add(1)
            .ok_or("Cache operation generation exhausted")?;
        self.serial = id;
        let account = cx.account.generation();
        let epoch = match target {
            Target::Artwork if matches!(action, Action::Clear) => cx.artwork.begin_clear().await,
            Target::Metadata if action.removes_discovery() => {
                cx.account.forget_cached_discovery();
                let live = cx
                    .account
                    .provider()
                    .map(|p| p.libraries())
                    .unwrap_or_default();
                cx.state["libraries"] = offline::library_views(&live, &live);
                cx.state["discovery"] = json!({"origin":if live.is_empty() {"none"} else {"live"},"stale":live.is_empty(),"incomplete":true,"message":"Offline library cache cleared or disabled. Reconnect Plex to save library discovery again."});
                cx.catalog.invalidate()?;
                cx.albums.invalidate();
                cx.search.invalidate()?;
                cx.artwork.revoke(false).await;
                self.release_presentation(cx.state);
                self.presentation = Some(Presentation {
                    id,
                    previous: cx.state["status"].take(),
                });
                cx.state["status"] =
                    json!("Metadata cache clearing. Browse again to load catalog pages.");
                if matches!(action, Action::Clear) {
                    cx.metadata.begin_clear()
                } else {
                    None
                }
            }
            _ => None,
        };
        let disk = cx.artwork.disk.clone();
        let metadata = cx.metadata.clone();
        let events = cx.events.clone();
        let task = tokio::spawn(async move {
            let result = match (target, action) {
                (Target::Artwork, Action::Clear) => match epoch {
                    Some(epoch) => disk.finish_clear(epoch).await,
                    None => Err("Artwork cleanup could not start."),
                },
                (Target::Metadata, Action::Clear) => match epoch {
                    Some(epoch) => metadata.finish_clear(epoch).await,
                    None => Err("Metadata cleanup could not start."),
                },
                (Target::Artwork, Action::Budget(mib)) => disk.budget(mib).await,
                (Target::Metadata, Action::Budget(mib)) => metadata.budget(mib).await,
            }
            .map_err(str::to_owned);
            let _ = events
                .send(Event::Maintenance(Reply {
                    id,
                    target,
                    account,
                    result,
                }))
                .await;
        });
        self.pending[target.index()] = Some(Pending {
            id,
            account,
            action,
            task,
        });
        self.publish(cx.state);
        Ok(())
    }
    pub async fn complete(&mut self, reply: Reply, cx: &mut Runtime<'_>) {
        let slot = &mut self.pending[reply.target.index()];
        if !slot
            .as_ref()
            .is_some_and(|p| p.id == reply.id && p.account == reply.account)
        {
            return;
        }
        let pending = slot.take().unwrap();
        // Completed work no longer needs its event-producing task. A storage job
        // already admitted to a store remains owned by that store.
        pending.task.abort();
        let current_account = reply.account == cx.account.generation();
        if self.presentation.as_ref().is_some_and(|p| p.id == reply.id) {
            if current_account {
                self.presentation = None;
                cx.state["status"] = json!(if reply.result.is_ok() {
                    "Metadata cache cleared. Browse again to load catalog pages."
                } else {
                    "Metadata cache cleanup is incomplete."
                });
            } else {
                self.release_presentation(cx.state);
            }
        }
        // These stores are application-wide. Storage failures remain diagnostic
        // even after account replacement; account-bound publication is fenced.
        if let Err(error) = reply.result {
            cx.state["error"] = json!(error);
        }
        if reply.target == Target::Artwork
            && matches!(pending.action, Action::Clear)
            && current_account
            && let Some(provider) = cx.account.provider()
        {
            cx.artwork
                .publish(provider, cx.albums.artwork_references())
                .await;
        }
        self.publish(cx.state);
    }
    pub async fn shutdown(&mut self, artwork: &artwork::Artwork, metadata: &metadata_cache::Cache) {
        for target in [Target::Artwork, Target::Metadata] {
            if let Some(mut pending) = self.pending[target.index()].take()
                && tokio::time::timeout(Duration::from_secs(3), &mut pending.task)
                    .await
                    .is_err()
            {
                pending.task.abort();
                let _ = pending.task.await;
            }
            match target {
                Target::Artwork => artwork.disk.shutdown().await,
                Target::Metadata => metadata.shutdown().await,
            }
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        for pending in self.pending.iter_mut().filter_map(Option::take) {
            pending.task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{artwork_store, offline};
    use qoda_core::SourceId;
    use serde_json::json;
    use std::{path::PathBuf, sync::Arc};
    use tokio::sync::mpsc;

    struct Environment {
        root: PathBuf,
        state: Value,
        account: account::Controller,
        catalog: catalog::Controller,
        albums: albums::Controller,
        search: search::Controller,
        artwork: artwork::Artwork,
        metadata: metadata_cache::Cache,
        tx: mpsc::Sender<crate::Event>,
        events: mpsc::Receiver<crate::Event>,
    }
    impl Environment {
        async fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("qoda-maintenance-{}", uuid::Uuid::new_v4()));
            let metadata = metadata_cache::Cache::start(Ok(metadata_cache::Paths {
                cache: root.join("metadata"),
                settings: root.join("metadata-settings"),
            }))
            .await;
            let discovery = offline::Discovery {
                version: 1,
                account: "a".repeat(64),
                fetched: 1,
                libraries: vec![qoda_provider::Library {
                    id: SourceId::new(),
                    title: "Cached".into(),
                    server: "Fixture".into(),
                }],
            };
            metadata.install_discovery(discovery.clone()).await.unwrap();
            let (tx, events) = mpsc::channel(16);
            let (vault, _requests) = mpsc::channel(8);
            let disk = artwork_store::Disk::start(Ok(artwork_store::Paths {
                cache: root.join("artwork"),
                settings: root.join("artwork-settings"),
            }))
            .await;
            Self {
                account: account::Controller::new(
                    Some(discovery),
                    vault,
                    None,
                    Arc::new(account::Plex),
                ),
                catalog: catalog::Controller::new(metadata.clone()),
                albums: albums::Controller::with_cache(metadata.clone()),
                search: Default::default(),
                artwork: artwork::Artwork::with_disk(disk, None).await.unwrap(),
                root,
                state: json!({"status":"Browsing","libraries":[{"title":"Cached"}]}),
                metadata,
                tx,
                events,
            }
        }
        async fn reply(&mut self) -> Reply {
            loop {
                let event = tokio::time::timeout(Duration::from_secs(5), self.events.recv())
                    .await
                    .unwrap()
                    .unwrap();
                if let Event::Maintenance(reply) = event {
                    return reply;
                }
            }
        }
        async fn command(&mut self, owner: &mut Controller, request: Value) {
            owner.command(&request, &mut self.runtime()).await.unwrap();
        }
        async fn complete(&mut self, owner: &mut Controller, reply: Reply) {
            owner.complete(reply, &mut self.runtime()).await;
        }
        fn runtime(&mut self) -> Runtime<'_> {
            Runtime {
                state: &mut self.state,
                account: &mut self.account,
                catalog: &mut self.catalog,
                albums: &mut self.albums,
                search: &mut self.search,
                artwork: &self.artwork,
                metadata: &self.metadata,
                events: &self.tx,
            }
        }
    }
    impl Drop for Environment {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[tokio::test]
    async fn invalid_budget_preserves_discovery_and_does_not_start_work() {
        let mut env = Environment::new().await;
        let mut maintenance = Controller::default();
        maintenance
            .command(
                &json!({"op":"set_metadata_budget","mib":1025}),
                &mut env.runtime(),
            )
            .await
            .unwrap();
        assert!(env.state["error"].is_string());
        assert!(env.state["metadata_cache_action"].is_null());
        assert!(env.account.catalog_access().is_some());
        assert!(env.metadata.discovery().await.is_some());
    }
    #[tokio::test]
    async fn artwork_clear_preserves_discovery_and_refuses_overlapping_target_work() {
        let mut env = Environment::new().await;
        let mut owner = Controller::default();
        env.command(&mut owner, json!({"op":"clear_artwork_cache"}))
            .await;
        let reply = env.reply().await; // completed store work, application reply deliberately held
        env.command(&mut owner, json!({"op":"set_artwork_budget","mib":2}))
            .await;
        assert!(env.state["error"].as_str().unwrap().contains("Wait"));
        assert_eq!(env.state["cache_action"], "clear");
        assert!(env.metadata.discovery().await.is_some());
        env.complete(&mut owner, reply).await;
        assert!(env.state["cache_action"].is_null());
        assert!(env.account.catalog_access().is_some());
        env.command(&mut owner, json!({"op":"set_artwork_budget","mib":2}))
            .await;
        let reply = env.reply().await;
        env.complete(&mut owner, reply).await;
        assert_eq!(env.artwork.disk.view().budget_mib, 2);
        owner.shutdown(&env.artwork, &env.metadata).await;
    }
    #[tokio::test]
    async fn metadata_clear_invalidates_authority_before_cleanup_and_cannot_overwrite_new_browsing()
    {
        let mut env = Environment::new().await;
        let mut owner = Controller::default();
        let (queue, provider) = crate::recovery::tests::fixture(vec![]);
        let source = queue
            .visible_track(queue.selected().unwrap())
            .unwrap()
            .reference
            .source;
        let urls = env
            .artwork
            .publish(
                provider,
                [qoda_provider::AlbumRef {
                    source,
                    key: "cover".into(),
                }],
            )
            .await;
        assert!(!urls.is_empty());
        env.catalog.publish(&mut env.state);
        let revision = env.state["catalog"]["revision"].clone();
        env.command(&mut owner, json!({"op":"clear_metadata_cache"}))
            .await;
        env.catalog.publish(&mut env.state);
        assert_ne!(env.state["catalog"]["revision"], revision);
        assert!(env.account.catalog_access().is_none());
        assert!(env.artwork.urls().await.is_empty());
        assert_eq!(env.state["libraries"], json!([]));
        let reply = env.reply().await;
        owner.observe_activity(&Event::Request(json!({"op":"browse"})), &mut env.state);
        env.state["status"] = json!("Loading new tracks");
        env.complete(&mut owner, reply).await;
        assert_eq!(env.state["status"], "Loading new tracks");
        assert!(env.state["metadata_cache_action"].is_null());
        assert!(env.metadata.discovery().await.is_none());
        owner.shutdown(&env.artwork, &env.metadata).await;
    }
    #[tokio::test]
    async fn zero_budget_removes_discovery_and_nonzero_budget_does_not_restore_it() {
        let mut env = Environment::new().await;
        let mut owner = Controller::default();
        env.command(&mut owner, json!({"op":"set_metadata_budget","mib":0}))
            .await;
        assert!(env.account.catalog_access().is_none());
        let reply = env.reply().await;
        env.complete(&mut owner, reply).await;
        assert_eq!(env.metadata.view().budget_mib, 0);
        assert!(env.metadata.discovery().await.is_none());
        env.command(&mut owner, json!({"op":"set_metadata_budget","mib":1}))
            .await;
        let reply = env.reply().await;
        env.complete(&mut owner, reply).await;
        assert_eq!(env.metadata.view().budget_mib, 1);
        assert!(env.metadata.discovery().await.is_none());
        owner.shutdown(&env.artwork, &env.metadata).await;
    }
    #[tokio::test]
    async fn account_replacement_cannot_gain_old_completion_authority_but_store_failures_remain_diagnostic()
     {
        let mut env = Environment::new().await;
        let mut owner = Controller::default();
        env.metadata.shutdown().await;
        env.command(&mut owner, json!({"op":"clear_metadata_cache"}))
            .await;
        let reply = env.reply().await;
        owner.observe_activity(&Event::Request(json!({"op":"disconnect"})), &mut env.state);
        // Advance the real account owner's generation through its public start;
        // no identity path means it cannot contact Plex or read user identities.
        env.account
            .start(
                qoda_provider::Credential::new("fixture".into(), "test-secret".into()).unwrap(),
                env.tx.clone(),
            )
            .unwrap();
        env.state["status"] = json!("Disconnected");
        env.complete(&mut owner, reply).await;
        assert_eq!(env.state["status"], "Disconnected");
        assert!(env.state["error"].as_str().unwrap().contains("stopped"));
        assert!(env.state["metadata_cache_action"].is_null());
        env.command(&mut owner, json!({"op":"clear_metadata_cache"}))
            .await;
        let reply = env.reply().await;
        env.complete(&mut owner, reply).await;
        assert!(env.state["metadata_cache_action"].is_null());
        owner.shutdown(&env.artwork, &env.metadata).await;
    }
    #[tokio::test]
    async fn shutdown_drains_both_targets_before_closing_stores() {
        let mut env = Environment::new().await;
        let mut owner = Controller::default();
        env.command(&mut owner, json!({"op":"clear_artwork_cache"}))
            .await;
        env.command(&mut owner, json!({"op":"clear_metadata_cache"}))
            .await;
        owner.shutdown(&env.artwork, &env.metadata).await;
        owner.publish(&mut env.state);
        assert!(env.state["cache_action"].is_null());
        assert!(env.state["metadata_cache_action"].is_null());
        assert!(env.artwork.disk.budget(1).await.is_err());
        assert!(env.metadata.budget(1).await.is_err());
    }
}
