//! Account sign-in, connection authority and asynchronous result lifetime.
//! Reconnect preserves read-only cached discovery; explicit disconnect purges account data.
use super::{
    ConnectError, Event, albums, archive, artwork, catalog, check_queue, gapless, identities,
    metadata_cache, offline,
    player::{Command, Player},
    playlists, prepared_playback, proxy, queue, reporting, save_identities, search, vault,
};
use qoda_core::SourceId;
use qoda_provider::{BrowserPin, Credential, MusicProvider, PlexProvider};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Restore,
    Save,
    Forget,
}
impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Restore => "restore",
            Self::Save => "save",
            Self::Forget => "forget",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Saved {
    Unknown,
    Present,
    Missing,
}
impl Saved {
    fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Present => "present",
            Self::Missing => "missing",
        }
    }
}
pub struct Connected {
    pub provider: Arc<dyn MusicProvider>,
    pub account: String,
    pub unavailable: HashSet<SourceId>,
    pub unavailable_servers: usize,
    pub identities: HashMap<String, SourceId>,
}
// External account access seam: Plex in production, a pending adapter in tests.
pub trait SignIn: Send + Sync {
    fn begin(&self) -> qoda_provider::ProviderFuture<'_, BrowserPin>;
    fn connect(
        &self,
        credential: Credential,
        identities: HashMap<String, SourceId>,
    ) -> qoda_provider::ProviderFuture<'_, Connected>;
}
pub struct Plex;
impl SignIn for Plex {
    fn begin(&self) -> qoda_provider::ProviderFuture<'_, BrowserPin> {
        Box::pin(BrowserPin::begin(format!(
            "qoda-player-{}",
            uuid::Uuid::new_v4()
        )))
    }
    fn connect(
        &self,
        credential: Credential,
        identities: HashMap<String, SourceId>,
    ) -> qoda_provider::ProviderFuture<'_, Connected> {
        Box::pin(async move {
            let (provider, identities) = PlexProvider::connect(credential, identities).await?;
            let account = provider.account_scope().to_owned();
            let unavailable = provider.unavailable_sources().clone();
            let unavailable_servers = provider.unavailable_server_count();
            Ok(Connected {
                provider: Arc::new(provider),
                identities,
                account,
                unavailable,
                unavailable_servers,
            })
        })
    }
}
struct Routing {
    local: Arc<dyn MusicProvider>,
    remote: Option<Arc<dyn MusicProvider>>,
    provider: Arc<dyn MusicProvider>,
}
pub struct Controller {
    local: Option<Arc<dyn MusicProvider>>,
    routing: Mutex<Option<Routing>>,
    signin: Arc<dyn SignIn>,
    identities_path: Option<PathBuf>,
    generation: u64,
    vault_generation: u64,
    credential: Option<Credential>,
    task: Option<tokio::task::JoinHandle<()>>,
    provider: Option<Arc<dyn MusicProvider>>,
    discovery: Option<offline::Discovery>,
    vault: mpsc::Sender<vault::Request>,
    action: Option<Action>,
    saved: Saved,
}
pub struct Runtime<'a> {
    pub state: &'a mut Value,
    pub tx: &'a mpsc::Sender<Event>,
    pub queue: &'a mut queue::Queue,
    pub player: &'a Player,
    pub preparation: &'a mut prepared_playback::Controller,
    pub reporting: &'a mut reporting::Controller,
    pub catalog: &'a mut catalog::Controller,
    pub epochs: &'a mut gapless::Epochs,
    pub gapless: &'a mut gapless::Controller,
    pub proxy: &'a proxy::Proxy,
    pub artwork: &'a artwork::Artwork,
    pub search: &'a mut search::Controller,
    pub albums: &'a mut albums::Controller,
    pub playlists: &'a mut playlists::Controller,
    pub metadata: &'a metadata_cache::Cache,
    pub archive: &'a mut archive::Controller,
    pub durable: &'a mut crate::storage::Session,
    pub desired_playing: &'a mut bool,
}
impl Controller {
    pub fn new(
        discovery: Option<offline::Discovery>,
        vault: mpsc::Sender<vault::Request>,
        identities_path: Option<PathBuf>,
        signin: Arc<dyn SignIn>,
    ) -> Self {
        Self {
            local: None,
            routing: Mutex::new(None),
            identities_path,
            signin,
            generation: 0,
            vault_generation: 0,
            credential: None,
            task: None,
            provider: None,
            discovery,
            vault,
            action: None,
            saved: Saved::Unknown,
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn provider(&self) -> Option<Arc<dyn MusicProvider>> {
        let Some(local) = self.local.as_ref().filter(|p| !p.libraries().is_empty()) else {
            *self.routing.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return self.provider.clone();
        };
        let mut routing = self.routing.lock().unwrap_or_else(|e| e.into_inner());
        let same = routing.as_ref().is_some_and(|r| {
            Arc::ptr_eq(&r.local, local)
                && match (&r.remote, &self.provider) {
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    (None, None) => true,
                    _ => false,
                }
        });
        if !same {
            *routing = Some(Routing {
                local: local.clone(),
                remote: self.provider.clone(),
                provider: Arc::new(qoda_provider::router::Router::new(
                    local.clone(),
                    self.provider.clone(),
                )),
            });
        }
        routing.as_ref().map(|r| r.provider.clone())
    }
    pub fn set_local(&mut self, provider: Arc<dyn MusicProvider>) {
        self.local = Some(provider);
        *self.routing.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    pub fn has_credential(&self) -> bool {
        self.credential.is_some()
    }
    pub fn catalog_access(&self) -> Option<offline::Access> {
        offline::access(self.provider(), self.discovery.clone())
    }
    pub fn forget_cached_discovery(&mut self) {
        self.discovery = None;
    }
    pub fn start(
        &mut self,
        credential: Credential,
        tx: mpsc::Sender<Event>,
    ) -> Result<(), &'static str> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("Generation exhausted")?;
        self.task = Some(connect(
            tx,
            self.generation,
            credential,
            self.identities_path.clone(),
            self.signin.clone(),
        ));
        Ok(())
    }
    pub fn publish(&self, state: &mut Value, reporting: &mut reporting::Controller) {
        state["sign_in"]["keyring_action"] = self
            .action
            .map(|action| json!(action.name()))
            .unwrap_or(Value::Null);
        state["sign_in"]["saved"] = json!(self.saved.name());
        state["sign_in"]["connected"] = json!(self.provider.is_some());
        state["sign_in"]["can_save"] = json!(self.credential.is_some());
        state["sign_in"]["can_reconnect"] = json!(self.credential.is_some());
        state["sign_in"]["connecting"] =
            json!(self.task.as_ref().is_some_and(|task| !task.is_finished()));
        state["account_epoch"] = json!(self.generation.to_string());
        state["playback_available"] = json!(self.provider().is_some());
        let mut libraries = state["libraries"].as_array().cloned().unwrap_or_default();
        libraries.retain(|l| l["kind"] != "local");
        if let Some(local) = &self.local {
            libraries.extend(local.libraries().into_iter().map(|l|json!({"id":l.id,"title":l.title,"server":l.server,"offline":false,"kind":"local","playlists":false})));
        }
        state["libraries"] = json!(libraries);
        reporting.bind(self.generation, self.provider());
    }
    fn keyring_request(
        &mut self,
        state: &mut Value,
        operation: vault::Operation,
    ) -> Result<(), &'static str> {
        let action = match &operation {
            vault::Operation::Restore => Action::Restore,
            vault::Operation::Save(_) => Action::Save,
            vault::Operation::Forget => Action::Forget,
        };
        let next = self
            .vault_generation
            .checked_add(1)
            .ok_or("Keyring generation exhausted")?;
        self.vault
            .try_send(vault::Request {
                generation: next,
                auth_generation: self.generation,
                operation,
            })
            .map_err(|_| "Keyring worker is busy or unavailable; retry.")?;
        self.vault_generation = next;
        self.action = Some(action);
        state["sign_in"]["message"] = json!("Waiting for desktop keyring. Unlock it if prompted.");
        Ok(())
    }
    pub async fn command(
        &mut self,
        v: &Value,
        runtime: &mut Runtime<'_>,
    ) -> Result<(), &'static str> {
        let Runtime {
            state,
            tx,
            queue,
            player,
            preparation,
            reporting,
            catalog,
            epochs,
            gapless,
            proxy,
            artwork,
            search,
            albums,
            playlists,
            metadata,
            archive,
            durable,
            desired_playing,
        } = runtime;
        match v["op"].as_str().unwrap_or("") {
            "reconnect" => {
                let result = check_queue(queue, v).map_err(|error| error.to_string());
                if let Err(error) = result {
                    state["error"] = json!(error);
                } else if v["account_epoch"].as_str() != Some(self.generation.to_string().as_str())
                {
                    state["error"] =
                        json!("The account changed. Review the connection before retrying.");
                } else if let Some(credential) = self.credential.clone() {
                    if let Some(task) = self.task.take() {
                        task.abort();
                    }
                    self.generation = self
                        .generation
                        .checked_add(1)
                        .ok_or("Account generation exhausted")?;
                    preparation.cancel();
                    reporting.bind(self.generation, None);
                    catalog.invalidate()?;
                    epochs.reset()?;
                    gapless.invalidate();
                    player.invalidate_load(epochs.current);
                    player.send(Command::Pause(true));
                    player.send(Command::Stop);
                    proxy.revoke().await;
                    artwork.revoke(false).await;
                    search.invalidate()?;
                    albums.invalidate();
                    playlists.invalidate();
                    self.provider = None;
                    self.discovery = metadata.discovery().await;
                    state["libraries"] = offline::library_views(
                        &self
                            .discovery
                            .as_ref()
                            .map(|d| d.libraries.clone())
                            .unwrap_or_default(),
                        &[],
                    );
                    state["discovery"] = json!({"origin":"offline","stale":true,"incomplete":true,"message":"Reconnecting Plex · cached browsing remains read-only; playback stays paused."});
                    state["sign_in"]["connected"] = json!(false);
                    **desired_playing = false;
                    state["intent"] = json!("paused");
                    state["audio"] = Value::Null;
                    state["status"] =
                        json!("Reconnecting Plex; choose Resume explicitly after connection.");
                    self.task = Some(connect(
                        tx.clone(),
                        self.generation,
                        credential,
                        self.identities_path.clone(),
                        self.signin.clone(),
                    ));
                } else {
                    state["error"] = json!("Restore sign-in or connect Plex before reconnecting.");
                }
            }
            "restore_signin" => {
                if self.provider.is_some() || self.action.is_some() {
                    state["error"] = json!(
                        "Disconnect and wait for the current keyring action before restoring sign-in."
                    );
                } else {
                    if let Some(task) = self.task.take() {
                        task.abort();
                    }
                    self.generation = self
                        .generation
                        .checked_add(1)
                        .ok_or("Generation exhausted")?;
                    preparation.cancel();
                    reporting.bind(self.generation, None);
                    state["signin_url"] = Value::Null;
                    state["status"] = json!("Restoring saved Plex sign-in");
                    if let Err(error) = self.keyring_request(state, vault::Operation::Restore) {
                        state["error"] = json!(error);
                    }
                }
            }
            "save_signin" => {
                if self.action.is_some() {
                    state["error"] = json!("Wait for the current keyring action to finish.");
                } else if let Some(credential) = self.credential.clone() {
                    if let Err(error) =
                        self.keyring_request(state, vault::Operation::Save(credential))
                    {
                        state["error"] = json!(error);
                    }
                } else {
                    state["error"] = json!("Connect Plex before saving sign-in.");
                }
            }
            "auth_start" => {
                if self.provider.is_some() {
                    state["error"] = json!("Disconnect before changing Plex accounts");
                    return Ok(());
                }
                search.invalidate()?;
                if let Some(task) = self.task.take() {
                    task.abort();
                }
                state["signin_url"] = Value::Null;
                self.generation = self
                    .generation
                    .checked_add(1)
                    .ok_or("Generation exhausted")?;
                preparation.cancel();
                reporting.bind(self.generation, None);
                let g = self.generation;
                let tx = tx.clone();
                state["status"] = json!("Starting Plex sign-in");
                let signin = self.signin.clone();
                self.task = Some(tokio::spawn(async move {
                    let r = signin.begin().await;
                    let _ = tx.send(Event::Pin(g, r)).await;
                }));
            }
            "disconnect" | "forget_signin" => {
                let forget = v["op"] == "forget_signin";
                if let Some(task) = self.task.take() {
                    task.abort();
                }
                self.generation = self
                    .generation
                    .checked_add(1)
                    .ok_or("Generation exhausted")?;
                preparation.cancel();
                reporting.bind(self.generation, None);
                catalog.invalidate()?;
                epochs.reset()?;
                gapless.invalidate();
                self.provider = None;
                self.discovery = None;
                state["discovery"] = json!({"origin":"none","stale":true,"incomplete":true,"message":"Disconnected; cached library discovery removed."});
                search.invalidate()?;
                playlists.invalidate();
                albums.invalidate();
                player.send(Command::Pause(true));
                player.send(Command::Stop);
                tokio::join!(artwork.revoke(true), metadata.clear());
                if let Err(error) = archive.clear(true).await {
                    state["error"] = json!(error);
                }
                **queue = queue.empty_session();
                durable.purge()?;
                state["storage"] = durable.view();
                self.credential = None;
                state["sign_in"]["connected"] = json!(false);
                state["sign_in"]["can_save"] = json!(false);
                **desired_playing = false;
                player.send(Command::Pause(true));
                player.send(Command::Stop);
                proxy.revoke().await;
                state["libraries"] = json!([]);
                state["selection"] = Value::Null;
                state["audio"] = Value::Null;
                state["signin_url"] = Value::Null;
                state["intent"] = json!("paused");
                state["status"] = json!("Disconnected");
                if forget
                    && self.action != Some(Action::Forget)
                    && let Err(error) = self.keyring_request(state, vault::Operation::Forget)
                {
                    state["error"] = json!(error);
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub async fn accept(
        &mut self,
        event: Event,
        runtime: &mut Runtime<'_>,
    ) -> Result<(), &'static str> {
        let Runtime {
            state,
            tx,
            preparation,
            reporting,
            catalog,
            artwork,
            search,
            albums,
            playlists,
            metadata,
            archive,
            ..
        } = runtime;
        match event {
            Event::Pin(g, r) if g == self.generation => match r {
                Ok(pin) => {
                    state["signin_url"] = json!(pin.url);
                    state["status"] = json!("Finish sign-in in your browser");
                    let tx = tx.clone();
                    self.task = Some(tokio::spawn(async move {
                        let until = tokio::time::Instant::now()
                            + std::time::Duration::from_secs(pin.expires_in);
                        while tokio::time::Instant::now() < until {
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            match pin.poll().await {
                                Ok(Some(c)) => {
                                    let _ = tx.send(Event::Credential(g, Ok(c))).await;
                                    return;
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    let _ = tx.send(Event::Credential(g, Err(e))).await;
                                    return;
                                }
                            }
                        }
                        let _ = tx
                            .send(Event::Credential(
                                g,
                                Err(qoda_provider::ProviderError::SignInRequired),
                            ))
                            .await;
                    }));
                }
                Err(e) => state["error"] = json!(e.to_string()),
            },
            Event::Credential(g, r) if g == self.generation => match r {
                Ok(c) => {
                    state["signin_url"] = Value::Null;
                    state["status"] = json!("Connecting Plex");
                    self.task = Some(connect(
                        tx.clone(),
                        g,
                        c,
                        self.identities_path.clone(),
                        self.signin.clone(),
                    ));
                }
                Err(e) => state["error"] = json!(e.to_string()),
            },
            Event::Connected(g, r, credential) if g == self.generation => {
                if let Some(task) = self.task.take() {
                    task.abort();
                }
                match r {
                    Ok(connected) => {
                        let p = connected.provider;
                        let ids = connected.identities;
                        // A verified account replaces all cached browse authority. Fence
                        // in-flight cached results before publishing its source allowlist.
                        catalog.invalidate()?;
                        albums.invalidate();
                        playlists.invalidate();
                        artwork.revoke(true).await;
                        let live = p.libraries();
                        let account = connected.account;
                        if let Err(error) = archive.bind(account.clone()).await {
                            state["error"] = json!(error);
                        }
                        let snapshot = offline::reconcile(
                            self.discovery.as_ref(),
                            account,
                            live.clone(),
                            &connected.unavailable,
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_secs() as i64),
                        );
                        let mut libraries = snapshot.libraries.clone();
                        self.discovery = None;
                        match metadata.install_discovery(snapshot.clone()).await {
                            Ok(()) => self.discovery = Some(snapshot),
                            Err(error) => {
                                state["error"] = json!(error);
                                libraries = live.clone();
                            }
                        }
                        let missing = libraries.len().saturating_sub(live.len());
                        state["discovery"] = json!({"origin":if libraries.is_empty() && connected.unavailable_servers==0 {"none"} else if live.is_empty() {"offline"} else if connected.unavailable_servers>0 {"partial"} else {"live"},"stale":missing>0,"incomplete":true,"message":format!("{} live libraries · {} cached libraries · {} servers unavailable. Cached sources are read-only; catalog coverage may be incomplete.",live.len(),missing,connected.unavailable_servers)});
                        state["libraries"] = offline::library_views(&libraries, &live);
                        if save_identities(self.identities_path.as_deref(), &ids).is_err() {
                            state["error"] = json!("Library identities could not be saved");
                        }
                        search.invalidate()?;
                        state["status"] = json!(if libraries.is_empty() {
                            "No available music libraries. Reconnect after server access changes."
                        } else {
                            "Choose a library; cached sources require a successful reconnect for playback."
                        });
                        self.provider = if live.is_empty() { None } else { Some(p) };
                        self.credential = Some(credential);
                        state["sign_in"]["connected"] = json!(self.provider.is_some());
                        state["sign_in"]["can_save"] = json!(true);
                        if self.saved != Saved::Present {
                            state["sign_in"]["message"] = json!(
                                "Connected for this session. Save sign-in to reconnect later."
                            );
                        }
                    }
                    Err(e) => {
                        if matches!(
                            e,
                            ConnectError::Provider(
                                qoda_provider::ProviderError::AccessDenied
                                    | qoda_provider::ProviderError::SignInRequired
                            )
                        ) {
                            preparation.cancel();
                            reporting.bind(self.generation, None);
                            catalog.invalidate()?;
                            albums.invalidate();
                            search.invalidate()?;
                            metadata.clear().await;
                            let _ = archive.clear(true).await;
                            self.provider = None;
                            self.credential = None;
                            state["sign_in"]["connected"] = json!(false);
                            state["sign_in"]["can_save"] = json!(false);
                            self.discovery = None;
                            state["libraries"] = json!([]);
                            state["discovery"] = json!({"origin":"none","stale":true,"incomplete":true,"message":"Plex denied account access; cached library discovery was removed."});
                        }
                        state["status"] = json!("Plex connection failed");
                        state["error"] = json!(e.to_string());
                    }
                }
            }
            Event::Vault(reply) if reply.generation == self.vault_generation => {
                self.action = None;
                match reply.result {
                    Ok(vault::Outcome::Saved) => {
                        self.saved = Saved::Present;
                        state["sign_in"]["message"] =
                            json!("Plex sign-in saved in the desktop keyring.");
                    }
                    Ok(vault::Outcome::Forgotten) => {
                        self.saved = Saved::Missing;
                        state["sign_in"]["message"] =
                            json!("Qoda's saved Plex sign-in was removed.");
                    }
                    Ok(vault::Outcome::Restored(credential)) => {
                        self.saved = if credential.is_some() {
                            Saved::Present
                        } else {
                            Saved::Missing
                        };
                        if reply.auth_generation == self.generation {
                            if let Some(credential) = credential {
                                state["status"] = json!("Connecting saved Plex account");
                                state["sign_in"]["message"] =
                                    json!("Saved sign-in restored. Playback stays paused.");
                                self.task = Some(connect(
                                    tx.clone(),
                                    self.generation,
                                    credential,
                                    self.identities_path.clone(),
                                    self.signin.clone(),
                                ));
                            } else {
                                state["status"] = json!("No saved Plex sign-in");
                                state["sign_in"]["message"] =
                                    json!("Connect Plex, then choose Save sign-in.");
                            }
                        } else {
                            state["sign_in"]["message"] =
                                json!("Saved sign-in checked; the restore request was cancelled.");
                        }
                    }
                    Err(error) => {
                        self.saved = Saved::Unknown;
                        state["error"] = json!(error.to_string());
                        state["sign_in"]["message"] =
                            json!("Keyring action did not complete; retry when it is available.");
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
fn connect(
    tx: mpsc::Sender<Event>,
    generation: u64,
    credential: Credential,
    path: Option<PathBuf>,
    signin: Arc<dyn SignIn>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let r = match identities(path.as_deref()) {
            Ok(ids) => signin
                .connect(credential.clone(), ids)
                .await
                .map_err(ConnectError::Provider),
            Err(_) => Err(ConnectError::Identity),
        };
        let _ = tx.send(Event::Connected(generation, r, credential)).await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::tests::fixture;
    use std::time::Duration;

    struct Environment {
        root: PathBuf,
        state: Value,
        tx: mpsc::Sender<Event>,
        events: mpsc::Receiver<Event>,
        queue: queue::Queue,
        player: Player,
        preparation: prepared_playback::Controller,
        reporting: reporting::Controller,
        catalog: catalog::Controller,
        epochs: gapless::Epochs,
        gapless: gapless::Controller,
        proxy: proxy::Proxy,
        artwork: artwork::Artwork,
        search: search::Controller,
        albums: albums::Controller,
        playlists: playlists::Controller,
        metadata: metadata_cache::Cache,
        archive: archive::Controller,
        durable: crate::storage::Session,
        playing: bool,
    }
    impl Environment {
        async fn new() -> Self {
            let root = std::env::temp_dir().join(format!("qoda-account-{}", uuid::Uuid::new_v4()));
            let (tx, events) = mpsc::channel(32);
            let metadata = metadata_cache::Cache::start(Ok(metadata_cache::Paths {
                cache: root.join("cache"),
                settings: root.join("settings"),
            }))
            .await;
            let (queue, _) = fixture(vec![]);
            Self {
                archive: archive::Controller::start(Ok(root.join("archive.sqlite3"))),
                root: root.clone(),
                state: json!({"sign_in":{},"storage":{}}),
                tx: tx.clone(),
                events,
                queue,
                player: Player::test_output("null").unwrap(),
                preparation: Default::default(),
                reporting: reporting::Controller::new(),
                catalog: catalog::Controller::new(metadata.clone()),
                epochs: Default::default(),
                gapless: Default::default(),
                proxy: proxy::Proxy::start().await.unwrap(),
                artwork: artwork::Artwork::start().await.unwrap(),
                search: Default::default(),
                albums: albums::Controller::with_cache(metadata.clone()),
                playlists: Default::default(),
                metadata,
                durable: crate::storage::Session::start(
                    Ok(root.join("session.sqlite3")),
                    tx.clone(),
                )
                .await
                .0,
                playing: false,
            }
        }
        fn runtime(&mut self) -> Runtime<'_> {
            Runtime {
                state: &mut self.state,
                tx: &self.tx,
                queue: &mut self.queue,
                player: &self.player,
                preparation: &mut self.preparation,
                reporting: &mut self.reporting,
                catalog: &mut self.catalog,
                epochs: &mut self.epochs,
                gapless: &mut self.gapless,
                proxy: &self.proxy,
                artwork: &self.artwork,
                search: &mut self.search,
                albums: &mut self.albums,
                playlists: &mut self.playlists,
                metadata: &self.metadata,
                archive: &mut self.archive,
                durable: &mut self.durable,
                desired_playing: &mut self.playing,
            }
        }
        fn owner(&self, vault: mpsc::Sender<vault::Request>) -> Controller {
            Controller::new(
                None,
                vault,
                Some(self.root.join("config/source-identities.json")),
                Arc::new(PendingSignIn),
            )
        }
        async fn command(&mut self, owner: &mut Controller, op: &str) {
            let snapshot = self.queue.snapshot();
            let request = json!({"op":op,"account_epoch":owner.generation().to_string(),"queue_session":snapshot.session,"queue_revision":snapshot.revision});
            owner.command(&request, &mut self.runtime()).await.unwrap();
            owner.publish(&mut self.state, &mut self.reporting);
        }
        async fn accept(&mut self, owner: &mut Controller, event: Event) {
            owner.accept(event, &mut self.runtime()).await.unwrap();
            owner.publish(&mut self.state, &mut self.reporting);
        }
        async fn vault_reply(&mut self, owner: &mut Controller) {
            let event = tokio::time::timeout(Duration::from_secs(5), self.events.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(event, Event::Vault(_)));
            self.accept(owner, event).await;
        }
        async fn connected(
            &mut self,
            owner: &mut Controller,
            libraries: Vec<qoda_provider::Library>,
            unavailable: HashSet<SourceId>,
        ) {
            let generation = owner.generation();
            self.accept(
                owner,
                Event::Connected(
                    generation,
                    Ok(Connected {
                        provider: Arc::new(TestDiscovery { libraries }),
                        account: "a".repeat(64),
                        unavailable_servers: usize::from(!unavailable.is_empty()),
                        unavailable,
                        identities: HashMap::new(),
                    }),
                    Credential::new("qoda-test".into(), "fixture-secret".into()).unwrap(),
                ),
            )
            .await;
        }
    }
    impl Drop for Environment {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    // A discovery-only provider uses the existing recovery fixture for unrelated
    // playback operations; all assertions cross the production account interface.
    struct TestDiscovery {
        libraries: Vec<qoda_provider::Library>,
    }
    impl MusicProvider for TestDiscovery {
        fn libraries(&self) -> Vec<qoda_provider::Library> {
            self.libraries.clone()
        }
        fn tracks(
            &self,
            _: SourceId,
            _: u64,
            _: String,
        ) -> qoda_provider::ProviderFuture<'_, qoda_provider::CatalogPage> {
            Box::pin(async { Err(qoda_provider::ProviderError::Unsupported) })
        }
        fn prepare(
            &self,
            _: SourceId,
            _: qoda_core::ProviderTrackKey,
        ) -> qoda_provider::ProviderFuture<'_, qoda_provider::PreparedMedia> {
            Box::pin(async { Err(qoda_provider::ProviderError::Unsupported) })
        }
    }
    struct PendingSignIn;
    impl SignIn for PendingSignIn {
        fn begin(&self) -> qoda_provider::ProviderFuture<'_, BrowserPin> {
            Box::pin(std::future::pending())
        }
        fn connect(
            &self,
            _: Credential,
            _: HashMap<String, SourceId>,
        ) -> qoda_provider::ProviderFuture<'_, Connected> {
            Box::pin(std::future::pending())
        }
    }
    fn credential() -> Credential {
        Credential::new("qoda-test".into(), "fixture-secret".into()).unwrap()
    }
    fn library(title: &str) -> qoda_provider::Library {
        qoda_provider::Library {
            id: SourceId::new(),
            title: title.into(),
            server: "Fixture".into(),
        }
    }

    #[tokio::test]
    async fn late_restore_and_connection_cannot_reinstall_a_disconnected_account() {
        let mut env = Environment::new().await;
        let (vault, mut requests) = mpsc::channel(8);
        let mut owner = env.owner(vault);
        env.command(&mut owner, "restore_signin").await;
        let request = requests.recv().await.unwrap();
        assert_eq!(env.state["sign_in"]["keyring_action"], "restore");
        env.command(&mut owner, "disconnect").await;
        env.accept(
            &mut owner,
            Event::Vault(vault::Reply {
                generation: request.generation,
                auth_generation: request.auth_generation,
                result: Ok(vault::Outcome::Restored(Some(credential()))),
            }),
        )
        .await;
        env.accept(
            &mut owner,
            Event::Credential(request.auth_generation, Ok(credential())),
        )
        .await;
        env.accept(
            &mut owner,
            Event::Connected(
                request.auth_generation,
                Ok(Connected {
                    provider: Arc::new(TestDiscovery {
                        libraries: vec![library("Late")],
                    }),
                    account: "a".repeat(64),
                    unavailable: HashSet::new(),
                    unavailable_servers: 0,
                    identities: HashMap::new(),
                }),
                credential(),
            ),
        )
        .await;
        assert_eq!(env.state["sign_in"]["saved"], "present");
        assert_eq!(env.state["sign_in"]["connecting"], false);
        assert_eq!(env.state["sign_in"]["can_reconnect"], false);
        assert_eq!(env.state["account_epoch"], "2");
        assert_eq!(env.state["libraries"], json!([]));
        assert!(owner.provider().is_none());
        assert_eq!(env.queue.snapshot().membership_count, 0);
        assert_eq!(env.durable.view()["cleanup_pending"], true);
        assert!(env.metadata.discovery().await.is_none());
    }
    #[tokio::test]
    async fn new_browser_signin_keeps_its_authority_when_an_old_restore_finishes() {
        let mut env = Environment::new().await;
        let (vault, mut requests) = mpsc::channel(8);
        let mut owner = env.owner(vault);
        env.command(&mut owner, "restore_signin").await;
        let request = requests.recv().await.unwrap();
        env.command(&mut owner, "auth_start").await;
        env.accept(
            &mut owner,
            Event::Vault(vault::Reply {
                generation: request.generation,
                auth_generation: request.auth_generation,
                result: Ok(vault::Outcome::Restored(Some(credential()))),
            }),
        )
        .await;
        assert_eq!(env.state["status"], "Starting Plex sign-in");
        assert_eq!(env.state["sign_in"]["saved"], "present");
        assert_eq!(env.state["sign_in"]["connecting"], true);
        assert_eq!(env.state["sign_in"]["can_reconnect"], false);
        assert_eq!(owner.generation(), 2);
    }
    #[tokio::test]
    async fn save_then_forget_uses_serialized_helper_and_ignores_obsolete_save_reply() {
        let mut env = Environment::new().await;
        let fixture = vault::tests::Fixture::new();
        let mut owner = env.owner(fixture.worker(env.tx.clone()));
        env.connected(&mut owner, vec![library("Live")], HashSet::new())
            .await;
        env.command(&mut owner, "save_signin").await;
        env.command(&mut owner, "forget_signin").await;
        env.vault_reply(&mut owner).await;
        assert_eq!(env.state["sign_in"]["keyring_action"], "forget");
        env.vault_reply(&mut owner).await;
        assert_eq!(env.state["sign_in"]["saved"], "missing");
        assert!(env.state["sign_in"]["keyring_action"].is_null());
        assert!(!owner.has_credential());
        assert_eq!(env.state["sign_in"]["can_save"], false);
        assert!(!env.state.to_string().contains("fixture-secret"));
    }
    #[tokio::test]
    async fn reconnect_retains_same_account_discovery_and_stays_paused_after_partial_results() {
        let mut env = Environment::new().await;
        let (vault, _requests) = mpsc::channel(8);
        let mut owner = env.owner(vault);
        let retained = library("Unreachable");
        env.connected(&mut owner, vec![retained.clone()], HashSet::new())
            .await;
        let stamp = env.queue.durable_stamp();
        env.playing = true;
        env.command(&mut owner, "reconnect").await;
        assert!(!env.playing);
        assert_eq!(env.state["intent"], "paused");
        assert!(owner.provider().is_none());
        assert!(owner.catalog_access().is_some());
        assert_eq!(env.queue.durable_stamp(), stamp);
        assert_eq!(env.durable.view()["cleanup_pending"], false);
        env.connected(
            &mut owner,
            vec![library("Live")],
            HashSet::from([retained.id]),
        )
        .await;
        assert_eq!(env.state["discovery"]["origin"], "partial");
        assert_eq!(env.state["libraries"].as_array().unwrap().len(), 2);
        assert_eq!(env.state["sign_in"]["can_save"], true);
        assert!(!env.playing);
        env.command(&mut owner, "reconnect").await;
        let generation = owner.generation();
        env.accept(
            &mut owner,
            Event::Connected(
                generation,
                Err(ConnectError::Provider(
                    qoda_provider::ProviderError::Network,
                )),
                credential(),
            ),
        )
        .await;
        assert!(owner.has_credential());
        assert!(owner.catalog_access().is_some());
        assert_eq!(env.state["status"], "Plex connection failed");
        assert!(env.metadata.discovery().await.is_some());
    }
    #[tokio::test]
    async fn failed_discovery_persistence_publishes_only_live_authority() {
        let mut env = Environment::new().await;
        let (vault, _requests) = mpsc::channel(8);
        let mut owner = env.owner(vault);
        let retained = library("Previously cached");
        env.connected(&mut owner, vec![retained.clone()], HashSet::new())
            .await;
        env.metadata.shutdown().await;
        let live = library("Live despite failed cache");
        env.connected(&mut owner, vec![live.clone()], HashSet::from([retained.id]))
            .await;
        assert_eq!(env.state["libraries"].as_array().unwrap().len(), 1);
        assert_eq!(env.state["libraries"][0]["title"], live.title);
        assert!(env.state["error"].is_string());
        assert!(matches!(
            owner.catalog_access(),
            Some(offline::Access::Live(_))
        ));
        assert!(owner.has_credential());
        assert_eq!(env.state["sign_in"]["connecting"], false);
    }
    #[tokio::test]
    async fn denied_and_expired_signin_currently_share_discovery_cleanup_policy() {
        // Characterizes current policy; an expired sign-in is not proof of access loss.
        for error in [
            qoda_provider::ProviderError::AccessDenied,
            qoda_provider::ProviderError::SignInRequired,
        ] {
            let mut env = Environment::new().await;
            let (vault, _requests) = mpsc::channel(8);
            let mut owner = env.owner(vault);
            env.connected(&mut owner, vec![library("Live")], HashSet::new())
                .await;
            let generation = owner.generation();
            env.accept(
                &mut owner,
                Event::Connected(generation, Err(ConnectError::Provider(error)), credential()),
            )
            .await;
            assert!(!owner.has_credential());
            assert!(owner.catalog_access().is_none());
            assert_eq!(env.state["libraries"], json!([]));
            assert!(env.metadata.discovery().await.is_none());
        }
    }
    #[tokio::test]
    async fn local_libraries_survive_plex_disconnect_with_stable_routing_identity() {
        let mut env = Environment::new().await;
        let (vault, _requests) = mpsc::channel(8);
        let mut owner = env.owner(vault);
        let root = qoda_provider::local::Root::new(env.root.join("music")).unwrap();
        owner.set_local(Arc::new(qoda_provider::local::Local::new(vec![
            root.clone(),
        ])));
        owner.publish(&mut env.state, &mut env.reporting);
        assert_eq!(env.state["sign_in"]["connected"], false);
        assert_eq!(env.state["playback_available"], true);
        let offline = owner.provider().unwrap();
        assert!(Arc::ptr_eq(&offline, &owner.provider().unwrap()));
        env.connected(&mut owner, vec![library("Remote")], HashSet::new())
            .await;
        let connected = owner.provider().unwrap();
        assert!(!Arc::ptr_eq(&offline, &connected));
        assert!(Arc::ptr_eq(&connected, &owner.provider().unwrap()));
        assert_eq!(env.state["libraries"].as_array().unwrap().len(), 2);
        env.command(&mut owner, "disconnect").await;
        assert_eq!(env.state["sign_in"]["connected"], false);
        assert_eq!(env.state["playback_available"], true);
        assert_eq!(env.state["libraries"].as_array().unwrap().len(), 1);
        assert_eq!(env.state["libraries"][0]["id"], json!(root.id));
        assert_eq!(env.state["libraries"][0]["playlists"], false);
        assert!(Arc::ptr_eq(
            &owner.provider().unwrap(),
            &owner.provider().unwrap()
        ));
        assert!(!env.playing);
    }
}
