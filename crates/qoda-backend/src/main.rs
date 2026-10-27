mod activation;
mod albums;
mod alsa_guard;
mod archive;
mod artwork;
mod artwork_store;
mod cache_files;
mod catalog;
mod continuous;
mod gain;
mod gapless;
mod metadata_cache;
mod mpris;
mod offline;
mod output;
mod player;
mod playlists;
mod prepared_playback;
mod proxy;
mod queue;
mod recovery;
mod reporting;
mod search;
mod session_commands;
mod shutdown;
mod source_format;
mod storage;
#[cfg(test)]
mod update_tests;
mod vault;
use player::{Command, Player};
use qoda_core::SourceId;
#[cfg(test)]
use qoda_core::TrackRef;
use qoda_provider::{BrowserPin, Credential, MusicProvider, PlexProvider};
use serde_json::{Value, json};
use session_commands::check_queue;
#[cfg(test)]
use session_commands::{navigation_request, unavailable_request};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, watch},
};
use zeroize::Zeroizing;

enum ConnectError {
    Identity,
    Provider(qoda_provider::ProviderError),
}
impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity => f.write_str("Saved library identities are unsafe, damaged or unsupported; existing data was preserved. Repair or remove source-identities.json before reconnecting."),
            Self::Provider(error) => error.fmt(f),
        }
    }
}
enum Event {
    Request(Value),
    Desktop(mpris::Request),
    DesktopLost(uuid::Uuid),
    DesktopReady(Result<mpris::Service, ()>),
    UiPeer(uuid::Uuid, bool),
    UiLaunchDone(uuid::Uuid, Result<(), &'static str>),
    Advance(u64),
    StreamFailed(qoda_provider::ProviderError, bool),
    Recovery(recovery::Reply),
    Search(search::Reply),
    Stored(storage::Reply),
    Checkpoint,
    Playlists(playlists::Reply),
    Albums(albums::Reply),
    Gapless(gapless::Reply),
    Artwork,
    OutputDevices(Vec<output::Device>),
    CacheDone(u64, bool, Result<(), String>),
    MetadataCacheDone(u64, Result<(), String>),
    Connected(
        u64,
        Result<(PlexProvider, HashMap<String, SourceId>), ConnectError>,
        Credential,
    ),
    Vault(vault::Reply),
    Page(catalog::Reply),
    Media(u64, Result<gain::Prepared, gain::PrepareError>),
    Pin(u64, Result<BrowserPin, qoda_provider::ProviderError>),
    Credential(u64, Result<Credential, qoda_provider::ProviderError>),
}
fn runtime() -> std::io::Result<PathBuf> {
    let base = PathBuf::from(
        std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| std::io::Error::other("XDG_RUNTIME_DIR is required"))?,
    );
    let uid = unsafe { libc::getuid() };
    let meta = std::fs::symlink_metadata(&base)?;
    if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 {
        return Err(std::io::Error::other("Unsafe runtime directory"));
    }
    let dir = base.join("qoda");
    match std::fs::create_dir(&dir) {
        Ok(()) => std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let m = std::fs::symlink_metadata(&dir)?;
    if !m.is_dir() || m.uid() != uid || m.permissions().mode() & 0o077 != 0 {
        return Err(std::io::Error::other("Unsafe Qoda runtime directory"));
    }
    Ok(dir)
}
fn identities_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .map(|p| p.join("qoda/source-identities.json"))
}
fn parse_identities(
    bytes: &[u8],
) -> Result<HashMap<String, SourceId>, qoda_provider::ProviderError> {
    use qoda_provider::ProviderError::InvalidReply;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(InvalidReply);
    }
    let ids: HashMap<String, SourceId> = serde_json::from_slice(bytes).map_err(|_| InvalidReply)?;
    let mut seen = HashSet::new();
    if ids.len() > 16384
        || !ids.iter().all(|(key, id)| {
            let scope = serde_json::from_str::<[String; 3]>(key).ok();
            seen.insert(*id)
                && scope.is_some_and(|scope| {
                    scope.iter().all(|s| !s.is_empty() && s.len() <= 1024)
                        && serde_json::to_string(&scope).ok().as_deref() == Some(key)
                })
        })
    {
        return Err(InvalidReply);
    }
    Ok(ids)
}
fn identities() -> Result<HashMap<String, SourceId>, qoda_provider::ProviderError> {
    use qoda_provider::ProviderError::InvalidReply;
    let path = identities_path().ok_or(InvalidReply)?;
    let directory = cache_files::Directory::open(path.parent().ok_or(InvalidReply)?)
        .map_err(|_| InvalidReply)?;
    let Some(file) = directory
        .file("source-identities.json")
        .map_err(|_| InvalidReply)?
    else {
        return Ok(HashMap::new());
    };
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| InvalidReply)?;
    parse_identities(&bytes)
}
fn save_identities(ids: &HashMap<String, SourceId>) -> std::io::Result<()> {
    let path = identities_path().ok_or_else(|| std::io::Error::other("No config directory"))?;
    let bytes = serde_json::to_vec(ids)?;
    parse_identities(&bytes).map_err(std::io::Error::other)?;
    cache_files::Directory::open(path.parent().unwrap())
        .and_then(|directory| directory.atomic("source-identities.json", &bytes))
        .map_err(std::io::Error::other)
}
async fn connection(
    stream: tokio::net::UnixStream,
    tx: mpsc::Sender<Event>,
    updates: watch::Receiver<Value>,
) -> std::io::Result<()> {
    let peer = uuid::Uuid::new_v4();
    let result = connection_io(stream, tx.clone(), updates, peer).await;
    let _ = tx.send(Event::UiPeer(peer, false)).await;
    result
}
async fn connection_io(
    stream: tokio::net::UnixStream,
    tx: mpsc::Sender<Event>,
    mut updates: watch::Receiver<Value>,
    peer: uuid::Uuid,
) -> std::io::Result<()> {
    if stream.peer_cred()?.uid() != unsafe { libc::getuid() } {
        return Ok(());
    }
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let mut hello = false;
    let mut frame = Vec::new();
    loop {
        tokio::select! {
            result = async {
                use tokio::io::AsyncReadExt;
                (&mut read).take((65537 - frame.len()) as u64)
                    .read_until(b'\n', &mut frame).await
            } => {
                if result? == 0 { return Ok(()); }
                if frame.len() > 65536 || frame.last() != Some(&b'\n') { return Ok(()); }
                let Ok(value) = serde_json::from_slice::<Value>(&frame) else { return Ok(()); };
                frame.clear();
                if !hello {
                    if value["op"] != "hello" || value["protocol"] != 1 { return Ok(()); }
                    hello = true;
                    if value["role"] == "shell" { tx.send(Event::UiPeer(peer, true)).await.map_err(|_| std::io::Error::other("Backend stopped"))?; }
                } else {
                    tx.send(Event::Request(value)).await.map_err(|_| std::io::Error::other("Backend stopped"))?;
                }
                let mut line = serde_json::to_vec(&*updates.borrow_and_update())?;
                line.push(b'\n');
                tokio::time::timeout(std::time::Duration::from_secs(3), write.write_all(&line)).await??;
            },
            changed = updates.changed(), if hello => {
                if changed.is_err() { return Ok(()); }
                let mut line = serde_json::to_vec(&*updates.borrow_and_update())?;
                line.push(b'\n');
                tokio::time::timeout(std::time::Duration::from_secs(3), write.write_all(&line)).await??;
            }
        }
    }
}
fn connect(
    tx: mpsc::Sender<Event>,
    generation: u64,
    credential: Credential,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let r = match identities() {
            Ok(ids) => PlexProvider::connect(credential.clone(), ids)
                .await
                .map_err(ConnectError::Provider),
            Err(_) => Err(ConnectError::Identity),
        };
        let _ = tx.send(Event::Connected(generation, r, credential)).await;
    })
}
fn record_clock(
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
fn pause_for_checkpoint(queue: &mut queue::Queue, player: &Player, generation: u64) {
    let audio = player.status();
    if audio.generation == generation
        && let Some(position) = audio.position_seconds
    {
        let _ = queue.set_position(position);
    }
    player.send(Command::Pause(true));
}
fn gain_view(settings: &gain::Settings, audio: &player::Status, generation: u64) -> Value {
    let mut view = settings.view();
    if audio.generation == generation && audio.entry_ready && !audio.idle && audio.error.is_none() {
        view["applied"] = json!(audio.gain);
    }
    view
}
fn output_view(
    settings: &output::Settings,
    devices: &[output::Device],
    audio: &player::Status,
    generation: u64,
) -> Value {
    let mut view = settings.view(devices);
    view["ready"] =
        json!(settings.selection.is_some() && settings.selection == audio.output_selection);
    if view["ready"] == true && audio.generation == generation {
        view["source"] = json!(audio.source);
        if audio.entry_ready && audio.error.is_none() {
            view["processing"] = json!(audio.processing);
            view["continuous"] = json!(audio.continuous);
            view["chain_fallback"] = json!(audio.chain_fallback);
        }
        if audio.entry_ready && !audio.idle && audio.error.is_none() {
            view["observed"] = json!(audio.output);
        }
    }
    view
}
enum ResumePlan {
    Waiting,
    Unpause,
    Prepare(qoda_provider::Track),
    Exhausted,
}
// Native Stop is asynchronous. An old engine generation must never be unpaused
// in place of loading the retained selection after Stop or another reset.
fn resume_selection(
    queue: &mut queue::Queue,
    audio: &player::Status,
    generation: u64,
    preparing: bool,
) -> Result<ResumePlan, queue::Error> {
    if preparing {
        return Ok(ResumePlan::Waiting);
    }
    if audio.generation == generation && !audio.idle && audio.error.is_none() {
        return Ok(ResumePlan::Unpause);
    }
    let track = if queue.ended() && queue.selected_unavailable() {
        queue
            .selected()
            .map(|id| queue.retry_unavailable(id))
            .transpose()?
    } else if queue.ended() || queue.selected().is_none() {
        queue.advance()?
    } else {
        queue
            .selected()
            .map(|id| queue.visible_track(id))
            .transpose()?
    };
    Ok(match track {
        Some(track) => ResumePlan::Prepare(track),
        None => ResumePlan::Exhausted,
    })
}
fn storage_result(state: &mut Value, result: Result<Option<storage::Error>, storage::Error>) {
    match result {
        Ok(warning) => {
            state["storage"]["state"] = json!("saved");
            state["storage"]["cleanup_pending"] = json!(false);
            state["storage"]["message"] = json!(
                warning
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "Session saved locally.".into())
            );
        }
        Err(error) => {
            state["storage"]["state"] = json!(if matches!(
                error,
                storage::Error::Corrupt
                    | storage::Error::Newer
                    | storage::Error::Unsafe
                    | storage::Error::ReopenRequired
            ) {
                "blocked"
            } else {
                "unsaved"
            });
            state["storage"]["message"] = json!(if state["storage"]["cleanup_pending"] == true {
                format!("Saved session cleanup is incomplete. {error}")
            } else {
                error.to_string()
            });
        }
    }
}
fn keyring_request(
    vault: &mpsc::Sender<vault::Request>,
    state: &mut Value,
    generation: &mut u64,
    auth_generation: u64,
    operation: vault::Operation,
    action: &str,
) -> Result<(), &'static str> {
    let next = generation
        .checked_add(1)
        .ok_or("Keyring generation exhausted")?;
    vault
        .try_send(vault::Request {
            generation: next,
            auth_generation,
            operation,
        })
        .map_err(|_| "Keyring worker is busy or unavailable; retry.")?;
    *generation = next;
    state["sign_in"]["keyring_action"] = json!(action);
    state["sign_in"]["message"] = json!("Waiting for desktop keyring. Unlock it if prompted.");
    Ok(())
}
#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Qoda: {error}");
            std::process::ExitCode::from(error.downcast_ref::<shutdown::Error>().map_or_else(
                || {
                    if error.is::<activation::ConfigError>() {
                        78
                    } else {
                        1
                    }
                },
                shutdown::Error::exit_status,
            ))
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut ui_launcher = activation::configured(std::env::args_os().skip(1))?;
    let credential = if std::env::args().any(|s| s == "--credential-stdin") {
        let mut input = Zeroizing::new(String::new());
        std::io::stdin().take(65537).read_to_string(&mut input)?;
        if input.len() > 65536 {
            return Err(activation::ConfigError("Credential input too large").into());
        }
        Some(
            serde_json::from_str::<Credential>(&input)
                .map_err(|_| activation::ConfigError("Invalid credential input"))?,
        )
    } else {
        None
    };
    let dir = runtime()
        .map_err(|_| activation::ConfigError("Runtime directory is unavailable or unsafe"))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir.join("backend.lock"))?;
    use std::os::fd::AsRawFd;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(activation::ConfigError("Qoda backend is already running").into());
    }
    if std::env::args().any(|s| s == "--restore-backup") {
        storage::restore_latest(&storage::path().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        println!(
            "Session backup restored. The original database was preserved. Next launch stays paused."
        );
        return Ok(());
    }
    // Keep one cancellation-safe receiver across coordinator iterations.
    let mut interrupts = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let path = dir.join("backend.sock");
    if path.symlink_metadata().is_ok() {
        shutdown::SocketIdentity::clear_stale(&path)
            .await
            .map_err(|_| {
                activation::ConfigError(
                    "Runtime socket conflict preserved; repair the runtime path before retrying",
                )
            })?;
    }
    let listener = tokio::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let socket_identity = shutdown::SocketIdentity::capture(&path)?;
    let player = Player::start().map_err(|_| {
        activation::ConfigError(
            "The playback engine could not initialize; check native dependencies",
        )
    })?;
    let mut gain_settings = gain::Settings::load(
        identities_path().and_then(|p| p.parent().map(std::path::Path::to_owned)),
    );
    let mut output_settings = output::Settings::load(
        identities_path().and_then(|p| p.parent().map(std::path::Path::to_owned)),
    );
    let mut output_devices = tokio::task::spawn_blocking(output::discover)
        .await
        .unwrap_or_default();
    let output_init_error =
        if !matches!(output_settings.selection, Some(output::Selection::Desktop)) {
            player
                .configure(output_settings.selection.clone())
                .await
                .err()
        } else {
            None
        };
    let proxy = proxy::Proxy::start().await?;
    let instance = uuid::Uuid::new_v4().to_string();
    let mut revision = 0_u64;

    let mut handled_end = None;
    let mut playlists = playlists::Controller::default();
    let metadata = metadata_cache::Cache::start(metadata_cache::Paths::discover()).await;
    let mut cached_discovery = metadata
        .discovery()
        .await
        .filter(|discovery| !discovery.libraries.is_empty());
    let mut albums = albums::Controller::with_cache(metadata.clone());
    let mut presented_generation = None;
    let mut catalog = catalog::Controller::new(metadata.clone());
    let mut state = json!({"protocol":1,"instance":instance,"revision":"0","status":"Connect Plex to browse music","libraries":[],"page":null,"selection":null,"intent":"paused","audio_profile":"Desktop / PipeWire","error":null,"signin_url":null,"sign_in":{"connected":false,"can_save":false,"keyring_action":null,"saved":"unknown","message":"Choose Restore to check for a saved sign-in."}});
    if let Some(discovery) = &cached_discovery {
        state["libraries"] = offline::library_views(&discovery.libraries, &[]);
        state["status"] = json!(
            "Offline library cache available. Choose a library; restore sign-in for playback."
        );
    }
    state["discovery"] = json!({"origin":if cached_discovery.is_some() {"offline"} else {"none"},"stale":true,"incomplete":true,"fetched":cached_discovery.as_ref().map(|d|d.fetched.to_string()),"message":if cached_discovery.is_some() {"Offline cached libraries · unverified and incomplete. Connect Plex for playback, playlists and edits."} else {"No cached library discovery available. Connect Plex to browse."}});
    state["replaygain"] = gain_settings.view();
    state["output"] = output_view(&output_settings, &output_devices, &player.status(), 0);
    if let Some(error) = output_init_error {
        state["error"] = json!(error);
    }
    let (tx, mut events) = mpsc::channel(64);
    let artwork = artwork::Artwork::persistent(tx.clone()).await?;
    state["artwork_cache"] = artwork.view().await;
    state["metadata_cache"] = json!(metadata.view());
    let (mut durable, restored) = storage::Controller::start(storage::path(), tx.clone()).await;
    let (mut queue, start_error) = match restored {
        Ok(queue) => (queue, None),
        Err(error) => (queue::Queue::default(), Some(error)),
    };
    let mut purge_epoch = 0_u64;
    let mut last_requested = (queue.durable_stamp(), purge_epoch);
    state["storage"] = json!({"state":"saved","message":"Session storage ready.","serial":"0","cleanup_pending":false});
    if let Some(error) = start_error {
        storage_result(&mut state, Err(error));
    }
    let mut archive = archive::Controller::start(archive::path());
    state["archive"] = json!(archive.view());
    let mut navigation_offset = None;
    state["navigation"] = json!(queue.navigation_page(navigation_offset));
    state["queue"] = json!(queue.snapshot());
    state["playlists"] = json!(playlists.view);
    state["albums"] = json!(albums.view);
    state["account_epoch"] = json!("0");
    catalog.publish(&mut state);
    if queue.snapshot().membership_count > 0 {
        state["status"] = json!("Session restored paused. Restore Plex sign-in to resume.");
    }
    state["selection"] = queue
        .selected()
        .and_then(|id| queue.visible_track(id).ok())
        .map(|track| json!(track))
        .unwrap_or(Value::Null);
    state["presentation"] = mpris::View::default().presentation();
    let mut search = search::Controller::default();
    state["search"] = search.view();
    let (updates, rx) = watch::channel(state.clone());
    let vault = vault::start(tx.clone());
    let mut vault_generation = 0_u64;
    let mut current_credential: Option<Credential> = None;
    let mut cache_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut metadata_task: Option<tokio::task::JoinHandle<()>> = None;
    let permits = Arc::new(tokio::sync::Semaphore::new(16));
    let server_tx = tx.clone();
    let server = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let Ok(permit) = permits.clone().try_acquire_owned() else {
                continue;
            };
            let tx = server_tx.clone();
            let rx = rx.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let _ = connection(stream, tx, rx).await;
            });
        }
    });
    let mut provider: Option<Arc<dyn MusicProvider>> = None;
    let mut preparation = prepared_playback::Controller::default();
    let mut auth_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut auth_generation = 0_u64;
    let mut epochs = gapless::Epochs::default();
    let mut gapless = gapless::Controller::default();
    state["recovery"] = json!(preparation.recovering());
    state["gapless"] = json!(gapless.view);
    let mut desired_playing = false;
    let mut ui_peers = HashSet::new();
    state["ui"] =
        json!({"can_raise":ui_launcher.available(),"raise_token":null,"launch_error":null});
    let mut output_refresh = tokio::time::interval(std::time::Duration::from_secs(5));
    let mut output_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut audio_tick = tokio::time::interval(std::time::Duration::from_millis(250));
    let mut last_audio = player.status();
    let mut last_stream_failed = false;
    let mut desktop_clock = mpris::Clock::default();
    let mut reporting = reporting::Controller::new();
    state["reporting"] = reporting.view();
    let mut last_clock_stale = false;
    let mut checkpoint_tick = tokio::time::interval(std::time::Duration::from_secs(5));
    checkpoint_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut desktop = match mpris::Service::start(tx.clone()).await {
        Ok(service) if service.is_live() => {
            state["mpris"] =
                json!({"state":"available","message":"Desktop media controls available."});
            Some(service)
        }
        _ => {
            state["mpris"] = json!({"state":"unavailable","message":"Desktop media controls unavailable. Retry after the session bus is available or another Qoda instance closes."});
            None
        }
    };
    let mut desktop_task: Option<tokio::task::JoinHandle<()>> = None;
    updates.send_replace(state.clone());
    if let Some(c) = credential {
        auth_generation = auth_generation
            .checked_add(1)
            .ok_or("Generation exhausted")?;
        reporting.bind(auth_generation, None);
        auth_task = Some(connect(tx.clone(), auth_generation, c));
        state["status"] = json!("Connecting Plex");
        updates.send_replace(state.clone());
    }
    loop {
        let event = tokio::select! {
            event = events.recv() => match event { Some(event) => event, None => break },
            _ = output_refresh.tick(), if output_task.as_ref().is_none_or(|task|task.is_finished()) => {
                let tx=tx.clone();output_task=Some(tokio::spawn(async move{let devices=tokio::task::spawn_blocking(output::discover).await.unwrap_or_default();let _=tx.send(Event::OutputDevices(devices)).await;}));
                continue;
            },
            _ = audio_tick.tick() => {
                let audio = player.status();
                let mut gapless_changed=false;
                match gapless.handoff(&mut queue,&mut epochs,&audio,&proxy).await {
                    Ok(true) => { gapless_changed=true; state["selection"]=queue.selected().and_then(|id|queue.visible_track(id).ok()).map(|track|json!(track)).unwrap_or(Value::Null); handled_end=None; },
                    Ok(false) if audio.generation<=epochs.current => {},
                    result => {
                        if !proxy.failed().await {
                        desired_playing=false;
                        if let Ok(before)=player.barrier().await {let _=session_commands::finish_gapless_reset(&mut gapless,&mut queue,&mut epochs,&before,&mut presented_generation,&proxy).await;}
                        else {gapless.invalidate();proxy.revoke().await;epochs.reset()?;gapless.review();}
                        state["error"]=json!(result.err().unwrap_or_else(||"An unbound native transition was reset; review the selected occurrence.".into()));
                        state["intent"]=json!("paused");gapless_changed=true;
                        }
                    },
                }
                let stream_failed=proxy.failed().await;
                let interrupted = stream_failed && !last_stream_failed && alsa_guard::error().is_none();
                let interrupted_playing = desired_playing;
                let observed_at=std::time::Instant::now();
                desktop_clock.observe(&audio,observed_at);
                let clock_stale=desktop_clock.stale(&audio,observed_at);
                reporting.bind(auth_generation,provider.clone().map(|p|p as Arc<dyn MusicProvider>));
                reporting.fence_generation(epochs.current);
                reporting.observe_stop(&audio);
                reporting.observe(reporting::Observation::capture(&queue,&audio,reporting::Context {
                    account:auth_generation,generation:epochs.current,confirmed:presented_generation,
                    failed_or_stale:stream_failed || clock_stale,
                }),observed_at);
                let reporting_view=reporting.view();
                let reporting_changed=state["reporting"]!=reporting_view;
                state["reporting"]=reporting_view;
                let archive_view=json!(archive.view());
                let archive_changed=state["archive"]!=archive_view;
                state["archive"]=archive_view;
                let mut presentation_changed=false;
                if archive_changed || audio != last_audio || stream_failed!=last_stream_failed || clock_stale!=last_clock_stale || reporting_changed {
                    last_stream_failed=stream_failed;
                    last_clock_stale=clock_stale;
                    presentation_changed=record_clock(&mut queue,&audio,epochs.current,&mut presented_generation,stream_failed);
                    gapless.observed(&audio,presented_generation);
                    archive.observe(&queue,&audio,epochs.current,presented_generation,stream_failed || clock_stale,observed_at);
                    last_audio = audio.clone();
                    state["audio"] = json!(audio);
                    if let Some(error) = audio.error.filter(|_|audio.generation==epochs.current).or(stream_failed.then_some("The media stream was interrupted. Retry the selected track.")) {
                        state["error"] = json!(output::explain_preflight(error, audio.source, output_settings.selection.as_ref().is_some_and(output::Selection::conversion_allowed)));
                        desired_playing = false;
                        state["intent"] = json!("paused");
                        player.send(Command::Pause(true));
                    }
                    revision = revision.checked_add(1).ok_or("Revision exhausted")?;
                    state["revision"] = json!(revision.to_string());
                    let view=mpris::View::capture(&queue,&audio,epochs.current,presented_generation,provider.is_some(),desired_playing,stream_failed || clock_stale).with_ui(!ui_peers.is_empty() || ui_launcher.available());
                    state["presentation"]=view.presentation();
                    state["gapless"]=json!(gapless.view);
                    state["artwork_cache"]=artwork.view().await;
                    state["metadata_cache"]=json!(metadata.view());
                    updates.send_replace(state.clone());
                    if let Some(desktop) = &mut desktop { desktop.publish(view); }
                }
                if interrupted {
                    Event::StreamFailed(proxy.failure().await.unwrap_or(qoda_provider::ProviderError::Network), interrupted_playing)
                } else if audio.generation==epochs.current && audio.idle && audio.natural_eof && handled_end!=Some(epochs.current) && audio.error.is_none() && !stream_failed {
                    handled_end=Some(epochs.current);
                    Event::Advance(epochs.current)
                } else if presentation_changed || gapless_changed || state["queue"]["selected"]!=json!(queue.selected()) {Event::Checkpoint} else {continue}
            },
            _ = checkpoint_tick.tick()=>Event::Checkpoint,
            _ = interrupts.recv() => break,
        };
        if matches!(&event, Event::Request(_) | Event::Desktop(_)) {
            let audio = player.status();
            match gapless
                .handoff(&mut queue, &mut epochs, &audio, &proxy)
                .await
            {
                Ok(true) => {
                    record_clock(
                        &mut queue,
                        &audio,
                        epochs.current,
                        &mut presented_generation,
                        false,
                    );
                    state["selection"] = queue
                        .selected()
                        .and_then(|id| queue.visible_track(id).ok())
                        .map(|track| json!(track))
                        .unwrap_or(Value::Null);
                    handled_end = None;
                }
                Ok(false) => {}
                Err(error) => {
                    desired_playing = false;
                    player.send(Command::Pause(true));
                    state["error"] = json!(error);
                    gapless.review();
                }
            }
        }
        let mut desktop_command = None;
        let mut event = if let Event::Desktop(request) = event {
            if !desktop
                .as_ref()
                .is_some_and(|service| service.accepts(&request))
            {
                let _ = request.reply.send(Err(
                    "The desktop command expired or its connection was replaced.".into(),
                ));
                continue;
            }
            if request.action == mpris::Action::Raise
                && ui_peers.is_empty()
                && ui_launcher.available()
            {
                ui_launcher.start(&instance, Some(request), tx.clone());
                continue;
            }
            let view = mpris::View::capture(
                &queue,
                &player.status(),
                epochs.current,
                presented_generation,
                provider.is_some(),
                desired_playing,
                last_stream_failed || last_clock_stale,
            )
            .with_ui(!ui_peers.is_empty() || ui_launcher.available());
            if let Some(command) = mpris::translate(&request.action, &view, &queue, &instance) {
                desktop_command = Some(session_commands::Desktop::new(request));
                Event::Request(command)
            } else {
                let _ = request.reply.send(Ok(()));
                Event::Checkpoint
            }
        } else {
            event
        };
        let mut command_effects = session_commands::Effects::default();
        if let Event::Request(request) = &mut event {
            let mut commands = session_commands::Playback {
                queue: &mut queue,
                player: &player,
                proxy: &proxy,
                epochs: &mut epochs,
                gapless: &mut gapless,
                preparation: &mut preparation,
                desired_playing: &mut desired_playing,
                presented: &mut presented_generation,
            };
            let mut context = session_commands::Context {
                instance: &instance,
                account: auth_generation,
                connected: provider.is_some(),
                credential: current_credential.is_some(),
                failed_or_stale: last_stream_failed || last_clock_stale,
                can_raise: !ui_peers.is_empty() || ui_launcher.available(),
                state: &mut state,
                catalog: &catalog,
                albums: &albums,
                playlists: &playlists,
                search: &search,
                output: &mut output_settings,
                devices: &output_devices,
                gain: &mut gain_settings,
            };
            match commands
                .admit(
                    request,
                    &mut context,
                    desktop_command.as_ref(),
                    desktop.as_ref(),
                )
                .await?
            {
                session_commands::Admission::Forward => {}
                session_commands::Admission::Applied(effects) => {
                    command_effects = effects;
                    event = Event::Checkpoint;
                }
                session_commands::Admission::Rejected => event = Event::Checkpoint,
            }
        }
        let mut to_prepare = command_effects.prepare;
        let mut automatic_next = command_effects.automatic;
        let mut force_save = command_effects.force_save;
        let seeked = command_effects.seeked;
        match event {
            Event::Search(reply) => {
                search.accept(reply, auth_generation)?;
            }
            Event::StreamFailed(error, playing) => {
                preparation
                    .interrupted(
                        error,
                        playing,
                        provider.clone().map(|p| p as Arc<dyn MusicProvider>),
                        tx.clone(),
                        &mut prepared_playback::Runtime {
                            queue: &mut queue,
                            player: &player,
                            proxy: &proxy,
                            epochs: &mut epochs,
                            gapless: &mut gapless,
                            playing: &mut desired_playing,
                            presented: &mut presented_generation,
                            state: &mut state,
                            output: &output_settings,
                            gain: &gain_settings,
                            account: auth_generation,
                        },
                    )
                    .await?;
            }
            Event::Recovery(reply) => {
                let next = preparation
                    .recovered(
                        reply,
                        &mut prepared_playback::Runtime {
                            queue: &mut queue,
                            player: &player,
                            proxy: &proxy,
                            epochs: &mut epochs,
                            gapless: &mut gapless,
                            playing: &mut desired_playing,
                            presented: &mut presented_generation,
                            state: &mut state,
                            output: &output_settings,
                            gain: &gain_settings,
                            account: auth_generation,
                        },
                    )
                    .await;
                to_prepare = next.track;
                automatic_next = next.automatic;
            }
            Event::Gapless(reply) => {
                let unavailable = gapless.unavailable_reply(&reply, &queue, &epochs);
                if let Err(error) = gapless.ready(reply, &queue, &epochs, &player, &proxy).await {
                    desired_playing = false;
                    match player.barrier().await {
                        Ok(before) => {
                            let _ = session_commands::finish_gapless_reset(
                                &mut gapless,
                                &mut queue,
                                &mut epochs,
                                &before,
                                &mut presented_generation,
                                &proxy,
                            )
                            .await;
                        }
                        Err(_) => {
                            gapless.invalidate();
                            proxy.revoke().await;
                            epochs.reset()?;
                            gapless.review();
                        }
                    }
                    state["error"] = json!(error);
                    state["intent"] = json!("paused");
                    state["audio"] = Value::Null;
                } else if let Some((id, reason)) = unavailable
                    && let Err(error) = queue.mark_unavailable(id, reason)
                {
                    state["error"] = json!(error.to_string());
                }
            }
            Event::UiLaunchDone(id, result) => {
                let Some(requests) = ui_launcher.complete(id) else {
                    continue;
                };
                state["ui"]["launch_error"] = result.err().map(Value::from).unwrap_or(Value::Null);
                for request in requests {
                    let accepted = desktop
                        .as_ref()
                        .is_some_and(|service| service.accepts(&request));
                    let response = if accepted {
                        result.map_err(str::to_owned)
                    } else {
                        Err("UI activation expired or its desktop connection was replaced".into())
                    };
                    let _ = request.reply.send(response);
                }
            }
            Event::UiPeer(peer, attached) => {
                let changed = if attached {
                    ui_peers.insert(peer)
                } else {
                    ui_peers.remove(&peer)
                };
                if !changed {
                    continue;
                }
                state["ui"]["can_raise"] = json!(!ui_peers.is_empty() || ui_launcher.available());
            }
            Event::DesktopLost(id)
                if desktop.as_ref().is_some_and(|service| service.id() == id) =>
            {
                ui_launcher.cancel();
                if let Some(service) = desktop.take() {
                    service.shutdown().await;
                }
                state["mpris"] = json!({"state":"unavailable","message":"Desktop media controls disconnected. Retry to reconnect; playback controls in Qoda remain available."});
            }
            Event::DesktopReady(result) => {
                desktop_task = None;
                match result {
                    Ok(service) if service.is_live() => {
                        desktop = Some(service);
                        state["mpris"] = json!({"state":"available","message":"Desktop media controls available."});
                    }
                    _ => {
                        state["mpris"] = json!({"state":"unavailable","message":"Desktop media controls unavailable. Retry after the session bus is available or another Qoda instance closes."})
                    }
                }
            }
            Event::Stored(reply) => {
                if reply.serial == durable.serial() {
                    storage_result(&mut state, reply.result);
                    state["storage"]["serial"] = json!(reply.serial.to_string());
                    revision = revision.checked_add(1).ok_or("Revision exhausted")?;
                    state["revision"] = json!(revision.to_string());
                    updates.send_replace(state.clone());
                }
                continue;
            }
            Event::Checkpoint | Event::Artwork => {}
            Event::MetadataCacheDone(g, result) => {
                metadata_task = None;
                state["metadata_cache_action"] = Value::Null;
                if g == auth_generation {
                    let clearing = state["status"]
                        == "Metadata cache clearing. Browse again to load catalog pages.";
                    match result {
                        Ok(()) if clearing => {
                            state["status"] =
                                json!("Metadata cache cleared. Browse again to load catalog pages.")
                        }
                        Err(error) => {
                            state["error"] = json!(error);
                            if clearing {
                                state["status"] = json!("Metadata cache cleanup is incomplete.");
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::CacheDone(g, clear, result) => {
                cache_task = None;
                state["cache_action"] = Value::Null;
                if let Err(error) = result {
                    state["error"] = json!(error);
                }
                if clear
                    && g == auth_generation
                    && let Some(provider) = provider.clone()
                {
                    let refs = albums.artwork_references();
                    artwork.publish(provider, refs).await;
                }
            }
            Event::Albums(reply) => {
                if albums.apply(reply, auth_generation)
                    && let Some(provider) = provider.clone()
                {
                    albums.view.covers =
                        artwork.publish(provider, albums.artwork_references()).await;
                }
            }
            Event::Playlists(reply) => {
                playlists.apply(reply, auth_generation);
            }
            Event::Advance(g) if g == epochs.current => {
                automatic_next = true;
                match queue.complete(presented_generation == Some(g)) {
                    Ok(Some(track)) => to_prepare = Some(track),
                    Ok(None) => {
                        desired_playing = false;
                        state["intent"] = json!("paused");
                        state["status"] = json!("End of queue");
                    }
                    Err(error) => {
                        state["error"] = json!(error.to_string());
                        desired_playing = false;
                        state["intent"] = json!("paused");
                    }
                }
            }
            Event::Request(v) => {
                let catalog_access = offline::access(provider.clone(), cached_discovery.clone());
                if v["instance"] != instance {
                    continue;
                }
                state["error"] = Value::Null;
                match v["op"].as_str().unwrap_or("") {
                    "quit" => {
                        state["shutdown"] = json!(true);
                        revision = revision.checked_add(1).ok_or("Revision exhausted")?;
                        state["revision"] = json!(revision.to_string());
                        updates.send_replace(state.clone());
                        if let Some(command) = desktop_command.take() {
                            command.complete(&state);
                        }
                        break;
                    }
                    "show_ui" => {
                        if !ui_peers.is_empty() {
                            state["ui"]["raise_token"] = json!(uuid::Uuid::new_v4().to_string());
                        } else if ui_launcher.available() {
                            ui_launcher.start(&instance, None, tx.clone());
                        }
                    }
                    "retry_save" => force_save = true,
                    "clear_artwork_cache" | "set_artwork_budget" => {
                        if cache_task.is_some() {
                            state["error"] = json!("Wait for the current artwork cache operation.");
                        } else {
                            let clear = v["op"] == "clear_artwork_cache";
                            let mib = v["mib"].as_u64().filter(|mib| *mib <= 4096);
                            if !clear && mib.is_none() {
                                state["error"] =
                                    json!("Choose an artwork cache target between 0 and 4096 MiB.");
                            } else {
                                let epoch = if clear {
                                    artwork.begin_clear().await
                                } else {
                                    None
                                };
                                let disk = artwork.disk.clone();
                                let tx = tx.clone();
                                let g = auth_generation;
                                state["cache_action"] =
                                    json!(if clear { "clear" } else { "budget" });
                                cache_task = Some(tokio::spawn(async move {
                                    let result = if clear {
                                        match epoch {
                                            Some(epoch) => disk.finish_clear(epoch).await,
                                            None => Err("Artwork cleanup could not start."),
                                        }
                                    } else {
                                        disk.budget(mib.unwrap() as u16).await
                                    };
                                    let _ = tx
                                        .send(Event::CacheDone(
                                            g,
                                            clear,
                                            result.map_err(str::to_owned),
                                        ))
                                        .await;
                                }));
                            }
                        }
                    }
                    "clear_metadata_cache" | "set_metadata_budget" => {
                        if metadata_task.is_some() {
                            state["error"] =
                                json!("Wait for the current metadata cache operation.");
                        } else {
                            let clear = v["op"] == "clear_metadata_cache";
                            let mib = v["mib"].as_u64().filter(|mib| *mib <= 1024);
                            if !clear && mib.is_none() {
                                state["error"] =
                                    json!("Choose a metadata target from 0 to 1024 MiB.");
                            } else {
                                if clear || mib == Some(0) {
                                    cached_discovery = None;
                                    let live = provider
                                        .as_ref()
                                        .map(|p| p.libraries())
                                        .unwrap_or_default();
                                    state["libraries"] = offline::library_views(&live, &live);
                                    state["discovery"] = json!({"origin":if provider.is_some() {"live"} else {"none"},"stale":provider.is_none(),"incomplete":true,"message":"Offline library cache cleared or disabled. Reconnect Plex to save library discovery again."});
                                }
                                let epoch = if clear || mib == Some(0) {
                                    catalog.invalidate()?;
                                    albums.invalidate();
                                    search.invalidate()?;
                                    artwork.revoke(false).await;
                                    state["status"] = json!(
                                        "Metadata cache clearing. Browse again to load catalog pages."
                                    );
                                    if clear { metadata.begin_clear() } else { None }
                                } else {
                                    None
                                };
                                let cache = metadata.clone();
                                let tx = tx.clone();
                                let g = auth_generation;
                                state["metadata_cache_action"] =
                                    json!(if clear { "clear" } else { "budget" });
                                metadata_task = Some(tokio::spawn(async move {
                                    let result = if clear {
                                        match epoch {
                                            Some(epoch) => cache.finish_clear(epoch).await,
                                            None => Err("Metadata cleanup could not start."),
                                        }
                                    } else {
                                        cache.budget(mib.unwrap() as u16).await
                                    };
                                    let _ = tx
                                        .send(Event::MetadataCacheDone(
                                            g,
                                            result.map_err(str::to_owned),
                                        ))
                                        .await;
                                }));
                            }
                        }
                    }
                    "set_output" => {} // Applied atomically by command admission.
                    "refresh_outputs" => {
                        if output_task.as_ref().is_none_or(|task| task.is_finished()) {
                            let tx = tx.clone();
                            output_task = Some(tokio::spawn(async move {
                                let devices = tokio::task::spawn_blocking(output::discover)
                                    .await
                                    .unwrap_or_default();
                                let _ = tx.send(Event::OutputDevices(devices)).await;
                            }));
                        }
                    }
                    "set_replaygain" => {} // Persisted before the speculative-output guard.
                    "set_gapless" => {
                        if let Some(enabled) = v["enabled"].as_bool() {
                            gapless.view.enabled = enabled;
                            gapless.retry();
                            gapless.view.phase = if enabled { "idle" } else { "off" };
                            gapless.view.message = if enabled {
                                "Gapless chaining enabled for compatible audio."
                            } else {
                                "Gapless chaining disabled for this backend session."
                            }
                            .into();
                        }
                    }
                    "reconnect" => {
                        let result = check_queue(&queue, &v).map_err(|error| error.to_string());
                        if let Err(error) = result {
                            state["error"] = json!(error);
                        } else if v["account_epoch"].as_str()
                            != Some(auth_generation.to_string().as_str())
                        {
                            state["error"] = json!(
                                "The account changed. Review the connection before retrying."
                            );
                        } else if let Some(credential) = current_credential.clone() {
                            if let Some(task) = auth_task.take() {
                                task.abort();
                            }
                            auth_generation = auth_generation
                                .checked_add(1)
                                .ok_or("Account generation exhausted")?;
                            preparation.cancel();
                            preparation.cancel_recovery();
                            reporting.bind(auth_generation, None);
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
                            provider = None;
                            cached_discovery = metadata.discovery().await;
                            state["libraries"] = offline::library_views(
                                &cached_discovery
                                    .as_ref()
                                    .map(|d| d.libraries.clone())
                                    .unwrap_or_default(),
                                &[],
                            );
                            state["discovery"] = json!({"origin":"offline","stale":true,"incomplete":true,"message":"Reconnecting Plex · cached browsing remains read-only; playback stays paused."});
                            state["sign_in"]["connected"] = json!(false);
                            desired_playing = false;
                            state["intent"] = json!("paused");
                            state["audio"] = Value::Null;
                            state["status"] = json!(
                                "Reconnecting Plex; choose Resume explicitly after connection."
                            );
                            auth_task = Some(connect(tx.clone(), auth_generation, credential));
                        } else {
                            state["error"] =
                                json!("Restore sign-in or connect Plex before reconnecting.");
                        }
                    }
                    "restore_signin" => {
                        if provider.is_some() || !state["sign_in"]["keyring_action"].is_null() {
                            state["error"] = json!(
                                "Disconnect and wait for the current keyring action before restoring sign-in."
                            );
                        } else {
                            if let Some(task) = auth_task.take() {
                                task.abort();
                            }
                            auth_generation = auth_generation
                                .checked_add(1)
                                .ok_or("Generation exhausted")?;
                            preparation.cancel();
                            preparation.cancel_recovery();
                            reporting.bind(auth_generation, None);
                            state["signin_url"] = Value::Null;
                            state["status"] = json!("Restoring saved Plex sign-in");
                            if let Err(error) = keyring_request(
                                &vault,
                                &mut state,
                                &mut vault_generation,
                                auth_generation,
                                vault::Operation::Restore,
                                "restore",
                            ) {
                                state["error"] = json!(error);
                            }
                        }
                    }
                    "save_signin" => {
                        if !state["sign_in"]["keyring_action"].is_null() {
                            state["error"] =
                                json!("Wait for the current keyring action to finish.");
                        } else if let Some(credential) = current_credential.clone() {
                            if let Err(error) = keyring_request(
                                &vault,
                                &mut state,
                                &mut vault_generation,
                                auth_generation,
                                vault::Operation::Save(credential),
                                "save",
                            ) {
                                state["error"] = json!(error);
                            }
                        } else {
                            state["error"] = json!("Connect Plex before saving sign-in.");
                        }
                    }
                    "auth_start" => {
                        if provider.is_some() {
                            state["error"] = json!("Disconnect before changing Plex accounts");
                            revision = revision.checked_add(1).ok_or("Revision exhausted")?;
                            state["revision"] = json!(revision.to_string());
                            updates.send_replace(state.clone());
                            continue;
                        }
                        search.invalidate()?;
                        if let Some(task) = auth_task.take() {
                            task.abort();
                        }
                        state["signin_url"] = Value::Null;
                        auth_generation = auth_generation
                            .checked_add(1)
                            .ok_or("Generation exhausted")?;
                        preparation.cancel();
                        preparation.cancel_recovery();
                        reporting.bind(auth_generation, None);
                        let g = auth_generation;
                        let tx = tx.clone();
                        state["status"] = json!("Starting Plex sign-in");
                        auth_task = Some(tokio::spawn(async move {
                            let r =
                                BrowserPin::begin(format!("qoda-player-{}", uuid::Uuid::new_v4()))
                                    .await;
                            let _ = tx.send(Event::Pin(g, r)).await;
                        }));
                    }
                    "playlist_list" | "playlist_open" | "playlist_refresh" | "playlist_create"
                    | "playlist_edit" => {
                        if matches!(v["op"].as_str(), Some("playlist_create" | "playlist_edit"))
                            && v["account_epoch"].as_str()
                                != Some(auth_generation.to_string().as_str())
                        {
                            state["error"] = json!(
                                "The account changed. Refresh the app before editing a playlist."
                            );
                        } else if let Some(provider) = provider.clone() {
                            if let Err(error) =
                                playlists.command(&v, provider, auth_generation, tx.clone())
                            {
                                state["error"] = json!(error);
                            }
                        } else {
                            state["error"] =
                                json!("Connect Plex before browsing or editing playlists.");
                        }
                    }
                    "album_list" | "album_open" | "artist_list" | "artist_open" => {
                        if let Some(provider) = catalog_access.clone() {
                            if let Err(error) =
                                albums.command(&v, provider, auth_generation, tx.clone())
                            {
                                state["error"] = json!(error);
                            } else if v["op"] != "album_open" {
                                artwork.revoke(false).await;
                            }
                        } else {
                            state["error"] = json!("Connect Plex before browsing albums.");
                        }
                    }
                    "search_all" => {
                        if let Some(provider) = catalog_access.clone() {
                            if let Err(error) = search.begin(
                                v["query"].as_str().unwrap_or(""),
                                auth_generation,
                                provider,
                                metadata.clone(),
                                tx.clone(),
                            ) {
                                state["error"] = json!(error);
                            }
                        } else {
                            state["error"] = json!("Connect Plex to search music libraries.");
                        }
                    }
                    "browse" => {
                        if let Some(access) = catalog_access.clone() {
                            if let Ok(source) =
                                serde_json::from_value::<SourceId>(v["source"].clone())
                                && albums.source_changed(source)
                            {
                                artwork.revoke(false).await;
                            }
                            match catalog.begin(&v, access, tx.clone()) {
                                Ok(()) => state["status"] = json!("Loading tracks"),
                                Err(error) => state["error"] = json!(error),
                            }
                        }
                    }
                    "retry_desktop" => {
                        if desktop_task.is_none()
                            && desktop.as_ref().is_some_and(|service| !service.is_live())
                            && let Some(service) = desktop.take()
                        {
                            service.shutdown().await;
                        }
                        if desktop.is_none() && desktop_task.is_none() {
                            state["mpris"] = json!({"state":"connecting","message":"Connecting desktop media controls…"});
                            let tx = tx.clone();
                            desktop_task = Some(tokio::spawn(async move {
                                let result =
                                    mpris::Service::start(tx.clone()).await.map_err(|_| ());
                                let _ = tx.send(Event::DesktopReady(result)).await;
                            }));
                        }
                    }
                    "navigation_page" => {
                        let result: Result<Option<u64>, String> = (|| {
                            check_queue(&queue, &v).map_err(|error| error.to_string())?;
                            if v["offset"].is_null() {
                                return Ok(None);
                            }
                            serde_json::from_value::<qoda_core::Counter>(v["offset"].clone())
                                .map(|offset| Some(offset.get()))
                                .map_err(|_| "Choose a valid Previous path page.".into())
                        })();
                        match result {
                            Ok(offset) => navigation_offset = offset,
                            Err(error) => state["error"] = json!(error),
                        }
                    }
                    "stop" => {
                        reporting.request_stop();
                        preparation.cancel();
                        if queue.selected().is_some() {
                            let _ = queue.seek_to(0.0);
                        }
                        desired_playing = false;
                        epochs.reset()?;
                        gapless.invalidate();
                        player.send(Command::Pause(true));
                        player.send(Command::Stop);
                        proxy.revoke().await;
                        state["selection"] = Value::Null;
                        state["audio"] = Value::Null;
                        state["intent"] = json!("paused");
                        state["status"] = json!(
                            "Stopped. Resume starts the retained selection from the beginning."
                        );
                        force_save = true;
                    }
                    "pause" => {
                        pause_for_checkpoint(&mut queue, &player, epochs.current);
                        force_save = true;
                        desired_playing = false;
                        state["intent"] = json!("paused");
                    }
                    "resume" => {
                        if provider.is_some() {
                            desired_playing = true;
                            if gapless.view.phase == "review" {
                                gapless.view.phase = "idle";
                                gapless.view.message =
                                    "Resuming the selected occurrence after review.".into();
                            }
                            let preparing = preparation.preparing();
                            match resume_selection(
                                &mut queue,
                                &player.status(),
                                epochs.current,
                                preparing,
                            ) {
                                Ok(ResumePlan::Waiting) => {}
                                Ok(ResumePlan::Unpause) => player.send(Command::Pause(false)),
                                Ok(ResumePlan::Prepare(track)) => to_prepare = Some(track),
                                Ok(ResumePlan::Exhausted) => {
                                    desired_playing = false;
                                    state["status"] = json!("End of queue — choose Replay");
                                }
                                Err(error) => {
                                    desired_playing = false;
                                    state["error"] = json!(error.to_string());
                                }
                            }
                            state["intent"] =
                                json!(if desired_playing { "playing" } else { "paused" });
                        }
                    }
                    "archive_page" | "archive_export" | "archive_clear" | "archive_retention" => {
                        let result = if v["account_epoch"].as_str()
                            != Some(auth_generation.to_string().as_str())
                        {
                            Err("The account changed; review the archive before retrying.")
                        } else {
                            match v["op"].as_str().unwrap() {
                                "archive_page" => match v["offset"]
                                    .as_u64()
                                    .and_then(|n| usize::try_from(n).ok())
                                    .filter(|n| *n <= 10000)
                                {
                                    Some(offset) => archive.page(offset).await,
                                    None => Err("Invalid archive page."),
                                },
                                "archive_export" => archive.export(),
                                "archive_clear" => archive.clear(archive.cleanup_pending()).await,
                                _ => match v["days"].as_u64().and_then(|n| u16::try_from(n).ok()) {
                                    Some(days) => archive.retention(days).await,
                                    None => Err("Invalid archive retention."),
                                },
                            }
                        };
                        if let Err(error) = result {
                            state["error"] = json!(error);
                        }
                    }
                    "disconnect" | "forget_signin" => {
                        let forget = v["op"] == "forget_signin";
                        if let Some(task) = auth_task.take() {
                            task.abort();
                        }
                        auth_generation = auth_generation
                            .checked_add(1)
                            .ok_or("Generation exhausted")?;
                        preparation.cancel();
                        preparation.cancel_recovery();
                        reporting.bind(auth_generation, None);
                        catalog.invalidate()?;
                        epochs.reset()?;
                        gapless.invalidate();
                        provider = None;
                        cached_discovery = None;
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
                        queue = queue.empty_session();
                        purge_epoch = purge_epoch
                            .checked_add(1)
                            .ok_or("Cleanup generation exhausted")?;
                        state["storage"]["cleanup_pending"] = json!(true);
                        current_credential = None;
                        state["sign_in"]["connected"] = json!(false);
                        state["sign_in"]["can_save"] = json!(false);
                        desired_playing = false;
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
                            && state["sign_in"]["keyring_action"] != "forget"
                            && let Err(error) = keyring_request(
                                &vault,
                                &mut state,
                                &mut vault_generation,
                                auth_generation,
                                vault::Operation::Forget,
                                "forget",
                            )
                        {
                            state["error"] = json!(error);
                        }
                    }
                    _ => {}
                }
            }
            Event::Pin(g, r) if g == auth_generation => match r {
                Ok(pin) => {
                    state["signin_url"] = json!(pin.url);
                    state["status"] = json!("Finish sign-in in your browser");
                    let tx = tx.clone();
                    auth_task = Some(tokio::spawn(async move {
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
            Event::Credential(g, r) if g == auth_generation => match r {
                Ok(c) => {
                    state["signin_url"] = Value::Null;
                    state["status"] = json!("Connecting Plex");
                    auth_task = Some(connect(tx.clone(), g, c));
                }
                Err(e) => state["error"] = json!(e.to_string()),
            },
            Event::Connected(g, r, credential) if g == auth_generation => match r {
                Ok((p, ids)) => {
                    // A verified account replaces all cached browse authority. Fence
                    // in-flight cached results before publishing its source allowlist.
                    catalog.invalidate()?;
                    albums.invalidate();
                    playlists.invalidate();
                    artwork.revoke(true).await;
                    let live = p.libraries();
                    let account = p.account_scope().to_owned();
                    if let Err(error) = archive.bind(account.clone()).await {
                        state["error"] = json!(error);
                    }
                    let snapshot = offline::reconcile(
                        cached_discovery.as_ref(),
                        account,
                        live.clone(),
                        p.unavailable_sources(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_secs() as i64),
                    );
                    let mut libraries = snapshot.libraries.clone();
                    cached_discovery = None;
                    match metadata.install_discovery(snapshot.clone()).await {
                        Ok(()) => cached_discovery = Some(snapshot),
                        Err(error) => {
                            state["error"] = json!(error);
                            libraries = live.clone();
                        }
                    }
                    let missing = libraries.len().saturating_sub(live.len());
                    state["discovery"] = json!({"origin":if libraries.is_empty() && p.unavailable_server_count()==0 {"none"} else if live.is_empty() {"offline"} else if p.unavailable_server_count()>0 {"partial"} else {"live"},"stale":missing>0,"incomplete":true,"message":format!("{} live libraries · {} cached libraries · {} servers unavailable. Cached sources are read-only; catalog coverage may be incomplete.",live.len(),missing,p.unavailable_server_count())});
                    state["libraries"] = offline::library_views(&libraries, &live);
                    if save_identities(&ids).is_err() {
                        state["error"] = json!("Library identities could not be saved");
                    }
                    search.invalidate()?;
                    state["status"] = json!(if libraries.is_empty() {
                        "No available music libraries. Reconnect after server access changes."
                    } else {
                        "Choose a library; cached sources require a successful reconnect for playback."
                    });
                    provider = if live.is_empty() {
                        None
                    } else {
                        Some(Arc::new(p))
                    };
                    current_credential = Some(credential);
                    state["sign_in"]["connected"] = json!(provider.is_some());
                    state["sign_in"]["can_save"] = json!(true);
                    if state["sign_in"]["saved"] != "present" {
                        state["sign_in"]["message"] =
                            json!("Connected for this session. Save sign-in to reconnect later.");
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
                        preparation.cancel_recovery();
                        reporting.bind(auth_generation, None);
                        catalog.invalidate()?;
                        albums.invalidate();
                        search.invalidate()?;
                        metadata.clear().await;
                        let _ = archive.clear(true).await;
                        provider = None;
                        current_credential = None;
                        state["sign_in"]["connected"] = json!(false);
                        state["sign_in"]["can_save"] = json!(false);
                        cached_discovery = None;
                        state["libraries"] = json!([]);
                        state["discovery"] = json!({"origin":"none","stale":true,"incomplete":true,"message":"Plex denied account access; cached library discovery was removed."});
                    }
                    state["status"] = json!("Plex connection failed");
                    state["error"] = json!(e.to_string());
                }
            },
            Event::Vault(reply) if reply.generation == vault_generation => {
                state["sign_in"]["keyring_action"] = Value::Null;
                match reply.result {
                    Ok(vault::Outcome::Saved) => {
                        state["sign_in"]["saved"] = json!("present");
                        state["sign_in"]["message"] =
                            json!("Plex sign-in saved in the desktop keyring.");
                    }
                    Ok(vault::Outcome::Forgotten) => {
                        state["sign_in"]["saved"] = json!("missing");
                        state["sign_in"]["message"] =
                            json!("Qoda's saved Plex sign-in was removed.");
                    }
                    Ok(vault::Outcome::Restored(credential)) => {
                        state["sign_in"]["saved"] = json!(if credential.is_some() {
                            "present"
                        } else {
                            "missing"
                        });
                        if reply.auth_generation == auth_generation {
                            if let Some(credential) = credential {
                                state["status"] = json!("Connecting saved Plex account");
                                state["sign_in"]["message"] =
                                    json!("Saved sign-in restored. Playback stays paused.");
                                auth_task = Some(connect(tx.clone(), auth_generation, credential));
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
                        state["sign_in"]["saved"] = json!("unknown");
                        state["error"] = json!(error.to_string());
                        state["sign_in"]["message"] =
                            json!("Keyring action did not complete; retry when it is available.");
                    }
                }
            }
            Event::Page(reply) => {
                catalog.accept(reply, &mut state)?;
            }
            Event::OutputDevices(devices) => {
                output_devices = devices;
            }
            Event::Media(g, r) => {
                let next = preparation
                    .complete(
                        g,
                        r,
                        &mut prepared_playback::Runtime {
                            queue: &mut queue,
                            player: &player,
                            proxy: &proxy,
                            epochs: &mut epochs,
                            gapless: &mut gapless,
                            playing: &mut desired_playing,
                            presented: &mut presented_generation,
                            state: &mut state,
                            output: &output_settings,
                            gain: &gain_settings,
                            account: auth_generation,
                        },
                    )
                    .await;
                to_prepare = next.track;
                automatic_next = next.automatic;
            }
            _ => continue,
        }
        if let Some(track) = to_prepare {
            preparation
                .start(
                    track,
                    automatic_next,
                    provider.clone().map(|p| p as Arc<dyn MusicProvider>),
                    tx.clone(),
                    &mut prepared_playback::Runtime {
                        queue: &mut queue,
                        player: &player,
                        proxy: &proxy,
                        epochs: &mut epochs,
                        gapless: &mut gapless,
                        playing: &mut desired_playing,
                        presented: &mut presented_generation,
                        state: &mut state,
                        output: &output_settings,
                        gain: &gain_settings,
                        account: auth_generation,
                    },
                )
                .await?;
        }
        let audio = player.status();
        if desired_playing
            && presented_generation == Some(epochs.current)
            && audio.generation == epochs.current
            && !audio.idle
            && !audio.paused
            && audio.error.is_none()
            && let Some(provider) = provider.clone()
            && let Err(error) = gapless.schedule(
                &queue,
                &mut epochs,
                provider,
                tx.clone(),
                gain_settings.mode,
                output_settings
                    .selection
                    .as_ref()
                    .is_some_and(output::Selection::fidelity),
            )
        {
            state["error"] = json!(error);
        }
        gapless.finish_chain_if_needed(&queue, &epochs, &player);
        catalog.publish(&mut state);
        state["gapless"] = json!(gapless.view);
        state["replaygain"] = gain_view(&gain_settings, &audio, epochs.current);
        state["output"] = output_view(&output_settings, &output_devices, &audio, epochs.current);
        let view = mpris::View::capture(
            &queue,
            &player.status(),
            epochs.current,
            presented_generation,
            provider.is_some(),
            desired_playing,
            last_stream_failed || last_clock_stale,
        )
        .with_ui(!ui_peers.is_empty() || ui_launcher.available());
        state["presentation"] = view.presentation();
        if let Some(desktop) = &mut desktop {
            if seeked {
                desktop.mark_seek(&queue, epochs.current);
            }
            desktop.publish(view);
        }
        if state["navigation"]["session"] != json!(queue.durable_stamp().0) {
            navigation_offset = None;
        }
        state["archive"] = json!(archive.view());
        state["navigation"] = json!(queue.navigation_page(navigation_offset));
        state["queue"] = json!(queue.snapshot());
        state["recovery"] = json!(preparation.recovering());
        state["search"] = search.view();
        state["playlists"] = json!(playlists.view);
        albums.view.covers = artwork.urls().await;
        state["albums"] = json!(albums.view);
        state["artwork_cache"] = artwork.view().await;
        state["metadata_cache"] = json!(metadata.view());
        state["sign_in"]["can_reconnect"] = json!(current_credential.is_some());
        state["sign_in"]["connecting"] =
            json!(auth_task.as_ref().is_some_and(|task| !task.is_finished()));
        state["account_epoch"] = json!(auth_generation.to_string());
        reporting.bind(
            auth_generation,
            provider.clone().map(|p| p as Arc<dyn MusicProvider>),
        );
        reporting.fence_generation(epochs.current);
        state["reporting"] = reporting.view();
        let stamp = (queue.durable_stamp(), purge_epoch);
        if force_save || stamp != last_requested {
            last_requested = stamp;
            match durable.submit(queue.clone(), purge_epoch) {
                Ok(()) => {
                    state["storage"]["state"] = json!("saving");
                    state["storage"]["message"] = json!("Saving session…");
                }
                Err(error) => storage_result(&mut state, Err(error)),
            }
        }
        revision = revision.checked_add(1).ok_or("Revision exhausted")?;
        state["revision"] = json!(revision.to_string());
        updates.send_replace(state.clone());
        if let Some(command) = desktop_command.take() {
            command.complete(&state);
        }
    }
    let audio = player.status();
    if audio.generation == epochs.current
        && let Some(position) = audio.position_seconds
    {
        let _ = queue.set_position(position);
    }
    ui_launcher.cancel();
    reporting.finish(&player).await;
    gapless.invalidate();
    player.send(Command::Pause(true));
    player.send(Command::Stop);
    if let Some(task) = desktop_task {
        task.abort();
        let _ = task.await;
    }
    events.close();
    while let Ok(event) = events.try_recv() {
        if let Event::DesktopReady(Ok(service)) = event {
            service.shutdown().await;
        }
    }
    proxy.revoke().await;
    artwork.revoke(false).await;
    if let Some(task) = cache_task {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), task).await;
    }
    artwork.disk.shutdown().await;
    if let Some(task) = metadata_task {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), task).await;
    }
    metadata.shutdown().await;
    server.abort();
    let archive_checkpoint = archive.flush().await;
    let checkpoint = shutdown::checkpoint(durable.flush(queue.clone(), purge_epoch).await);
    if let Some(desktop) = desktop {
        desktop.shutdown().await;
    }
    drop(player);
    let socket_cleanup = socket_identity.remove(&path);
    drop(lock);
    // Persistence failure must remain visible even if socket cleanup also fails.
    if checkpoint.is_err()
        && let Err(error) = &socket_cleanup
    {
        eprintln!("Qoda: runtime socket cleanup failed: {error}");
    }
    checkpoint?;
    archive_checkpoint.map_err(shutdown::Error::Archive)?;
    socket_cleanup.map_err(shutdown::Error::Cleanup)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn persisted_source_identity_rejects_newer_malformed_and_cross_account_aliases() {
        let id = SourceId::new();
        let key = serde_json::to_string(&["account", "server", "section"]).unwrap();
        let good = json!({key: id});
        assert!(parse_identities(&serde_json::to_vec(&good).unwrap()).is_ok());
        for invalid in [
            json!({"version":2,"sources":{}}),
            json!({"not a scope":id}),
            json!({serde_json::to_string(&["a","s","1"]).unwrap():id,serde_json::to_string(&["b","s","1"]).unwrap():id}),
        ] {
            assert!(parse_identities(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
    }
    #[test]
    fn reconnect_requires_current_queue_account_and_explicit_session_credential() {
        let queue = queue::Queue::default();
        let snapshot = queue.snapshot();
        let mut request = json!({"queue_session":snapshot.session,"queue_revision":snapshot.revision,"account_epoch":"3"});
        assert!(session_commands::reconnect_request(&queue, &request, 3, true).is_ok());
        assert!(session_commands::reconnect_request(&queue, &request, 3, false).is_err());
        request["account_epoch"] = json!("2");
        assert!(session_commands::reconnect_request(&queue, &request, 3, true).is_err());
        request["account_epoch"] = json!("3");
        request["queue_revision"] = json!("18446744073709551615");
        assert!(session_commands::reconnect_request(&queue, &request, 3, true).is_err());
    }
    use super::*;
    fn queued_selection() -> queue::Queue {
        let track = qoda_provider::Track {
            reference: TrackRef {
                source: SourceId::new(),
                key: "1".to_owned().try_into().unwrap(),
            },
            title: "Synthetic fixture".into(),
            artist: "Fixture".into(),
            album: "Fixture".into(),
            duration_ms: None,
        };
        let mut q = queue::Queue::default();
        q.replace(track.clone()).unwrap();
        q.enqueue(track, false).unwrap();
        q
    }
    #[test]
    fn gain_projection_rejects_stale_unready_stopped_and_failed_render_owners() {
        let settings = gain::Settings::load(None);
        let mut audio = player::Status {
            generation: 7,
            entry_ready: true,
            idle: false,
            gain: Some(gain::Applied::off()),
            ..Default::default()
        };
        assert!(!gain_view(&settings, &audio, 7)["applied"].is_null());
        assert!(gain_view(&settings, &audio, 8)["applied"].is_null());
        audio.entry_ready = false;
        assert!(gain_view(&settings, &audio, 7)["applied"].is_null());
        audio.entry_ready = true;
        audio.idle = true;
        assert!(gain_view(&settings, &audio, 7)["applied"].is_null());
        audio.idle = false;
        audio.error = Some("Synthetic failure");
        assert!(gain_view(&settings, &audio, 7)["applied"].is_null());
    }
    #[test]
    fn output_projection_does_not_reuse_stale_or_failed_negotiation() {
        let path = std::env::temp_dir().join(format!("qoda-output-view-{}", uuid::Uuid::new_v4()));
        let settings = output::Settings::load(Some(path.clone()));
        let mut audio = player::Status {
            generation: 7,
            entry_ready: true,
            output_selection: Some(output::Selection::Desktop),
            source: Some(source_format::Format {
                rate: 48000,
                bits: 24,
            }),
            output: Some(alsa_guard::Observed {
                rate: 48000,
                channels: 2,
                container_bits: 32,
                valid_bits: 24,
                format: output::Sample::S32,
                stereo_layout: true,
                stereo_assumed: false,
            }),
            ..Default::default()
        };
        assert!(!output_view(&settings, &[], &audio, 7)["observed"].is_null());
        assert!(output_view(&settings, &[], &audio, 8)["observed"].is_null());
        assert!(output_view(&settings, &[], &audio, 8)["source"].is_null());
        audio.entry_ready = false;
        assert!(output_view(&settings, &[], &audio, 7)["observed"].is_null());
        audio.entry_ready = true;
        audio.error = Some("Synthetic device loss");
        assert!(output_view(&settings, &[], &audio, 7)["observed"].is_null());
        audio.output_selection = None;
        assert_eq!(output_view(&settings, &[], &audio, 7)["ready"], false);
        assert!(
            gain::PrepareError::Fidelity("Unsupported source header")
                .provider()
                .is_none()
        );
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn only_individual_failures_advance_automatic_playing_preparations() {
        use qoda_provider::ProviderError;
        for error in [
            ProviderError::Network,
            ProviderError::SignInRequired,
            ProviderError::AccessDenied,
            ProviderError::UnknownSource,
            ProviderError::InvalidReply,
        ] {
            let mut q = queued_selection();
            q.set_position(7.0).unwrap();
            let before = serde_json::to_value(&q).unwrap();
            assert!(
                prepared_playback::preparation_failure(
                    &mut q,
                    queue::Unavailable::from_provider(error),
                    true,
                    true
                )
                .unwrap()
                .is_none()
            );
            assert_eq!(serde_json::to_value(q).unwrap(), before);
        }
        for (automatic, playing) in [(false, true), (true, false)] {
            let mut q = queued_selection();
            let selected = q.selected();
            q.set_position(7.0).unwrap();
            assert!(
                prepared_playback::preparation_failure(
                    &mut q,
                    Some(queue::Unavailable::Missing),
                    automatic,
                    playing
                )
                .unwrap()
                .is_none()
            );
            assert_eq!(q.selected(), selected);
            assert_eq!(q.position(), 7.0);
            assert!(q.selected_unavailable());
        }
        assert!(prepared_playback::version_failure(&queue::Error::Version, "").is_none());
        assert!(
            prepared_playback::version_failure(&queue::Error::Version, &"x".repeat(513)).is_none()
        );
        assert_eq!(
            prepared_playback::version_failure(&queue::Error::Version, "valid-other-version"),
            Some(queue::Unavailable::VersionChanged)
        );
        assert_eq!(
            queue::Unavailable::from_provider(ProviderError::UnsupportedMedia),
            Some(queue::Unavailable::Unsupported)
        );
    }
    #[test]
    fn all_unavailable_automatic_scans_terminate_even_in_repeat_modes() {
        for repeat in [queue::Repeat::One, queue::Repeat::All] {
            let track = queued_selection().snapshot().entries[0].track.clone();
            let mut q = queue::Queue::default();
            q.replace_many(vec![track; 500]).unwrap();
            q.set_repeat(repeat).unwrap();
            for index in 0..500 {
                let next = prepared_playback::preparation_failure(
                    &mut q,
                    Some(queue::Unavailable::Missing),
                    true,
                    true,
                )
                .unwrap();
                assert_eq!(next.is_some(), index < 499);
            }
            assert!(q.ended());
            assert_eq!(q.snapshot().unavailable_entries.len(), 500);
            assert!(q.navigation_page(None).entries.is_empty());
            assert!(q.advance().unwrap().is_none());
            let failed = q.selected();
            assert!(matches!(
                resume_selection(
                    &mut q,
                    &player::Status {
                        idle: true,
                        ..Default::default()
                    },
                    0,
                    false
                )
                .unwrap(),
                ResumePlan::Prepare(_)
            ));
            assert_eq!(q.selected(), failed);
            assert!(!q.ended());
            assert_eq!(q.snapshot().unavailable_entries.len(), 500);
        }
    }
    #[test]
    fn unavailable_actions_reject_stale_or_unmarked_rows_before_output_changes() {
        let mut q = queued_selection();
        let id = q.selected().unwrap();
        let mut request = json!({"op":"retry_unavailable","occurrence":id,"queue_session":q.snapshot().session,"queue_revision":q.snapshot().revision});
        assert!(unavailable_request(&q, &request).is_err());
        q.mark_unavailable(id, queue::Unavailable::Missing).unwrap();
        assert!(unavailable_request(&q, &request).is_err());
        request["queue_revision"] = json!(q.snapshot().revision);
        assert_eq!(unavailable_request(&q, &request).unwrap(), Some(id));
        request["op"] = json!("skip_unavailable");
        assert_eq!(unavailable_request(&q, &request).unwrap(), None);
    }
    #[test]
    fn navigation_commands_require_current_session_revision_and_exact_route_identity() {
        let mut q = queued_selection();
        q.confirm_presented().unwrap();
        let page = q.navigation_page(None);
        let request = json!({"queue_session":page.session,"queue_revision":page.revision,"index":page.entries[0].index,"occurrence":page.entries[0].occurrence});
        assert!(navigation_request(&q, &request).is_ok());
        let mut invalid = request.clone();
        invalid["index"] = json!(0);
        assert!(navigation_request(&q, &invalid).is_err());
        invalid["index"] = json!("18446744073709551615");
        assert!(navigation_request(&q, &invalid).is_err());
        invalid = request.clone();
        invalid["occurrence"] = json!(q.pending()[0]);
        assert!(navigation_request(&q, &invalid).is_err());
        let other = queued_selection();
        assert!(navigation_request(&other, &request).is_err());
        q.set_shuffle(true).unwrap();
        assert!(navigation_request(&q, &request).is_err());
    }
    #[test]
    fn stop_then_immediate_play_reloads_the_same_occurrence_instead_of_unpausing_stale_native_state()
     {
        let mut q = queued_selection();
        let selected = q.selected();
        let before = q.snapshot();
        q.seek_to(0.0).unwrap();
        let audio = player::Status {
            generation: 7,
            idle: false,
            paused: false,
            ..Default::default()
        };
        let plan = resume_selection(&mut q, &audio, 8, false).unwrap();
        assert!(matches!(plan, ResumePlan::Prepare(_)));
        assert_eq!(q.selected(), selected);
        assert_eq!(q.snapshot().membership_count, before.membership_count);
        assert_eq!(q.snapshot().session, before.session);
        assert_eq!(q.position(), 0.0);
        assert!(matches!(
            resume_selection(&mut q, &audio, 8, true).unwrap(),
            ResumePlan::Waiting
        ));
        assert_eq!(q.selected(), selected);
    }
    #[test]
    fn resume_unpauses_only_the_matching_live_engine_and_explicitly_retries_errors() {
        let mut q = queued_selection();
        let audio = player::Status {
            generation: 7,
            idle: false,
            paused: true,
            ..Default::default()
        };
        assert!(matches!(
            resume_selection(&mut q, &audio, 7, false).unwrap(),
            ResumePlan::Unpause
        ));
        let failed = player::Status {
            error: Some("Synthetic engine failure"),
            ..audio
        };
        assert!(matches!(
            resume_selection(&mut q, &failed, 7, false).unwrap(),
            ResumePlan::Prepare(_)
        ));
        let mut empty = queue::Queue::default();
        let idle = player::Status {
            idle: true,
            ..Default::default()
        };
        assert!(matches!(
            resume_selection(&mut empty, &idle, 0, false).unwrap(),
            ResumePlan::Exhausted
        ));
        assert_eq!(empty.snapshot().membership_count, 0);
    }
    #[tokio::test]
    async fn socket_requires_hello_and_preserves_partial_frames_across_updates() {
        let (server, client) = tokio::net::UnixStream::pair().unwrap();
        let (tx, mut events) = mpsc::channel(4);
        let (updates, rx) = watch::channel(json!({"instance":"fixture","revision":"0"}));
        let task = tokio::spawn(connection(server, tx, rx));
        let (read, mut write) = client.into_split();
        let mut read = BufReader::new(read);
        let mut line = String::new();
        write
            .write_all(b"{\"op\":\"hello\",\"protocol\":1}\n")
            .await
            .unwrap();
        read.read_line(&mut line).await.unwrap();
        assert!(line.contains("fixture"));
        write.write_all(b"{\"op\":\"pa").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        updates.send_replace(json!({"instance":"fixture","revision":"1"}));
        line.clear();
        read.read_line(&mut line).await.unwrap();
        assert!(line.contains("\"1\""));
        write
            .write_all(b"use\",\"instance\":\"fixture\"}\n")
            .await
            .unwrap();
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event,Event::Request(v)if v["op"]=="pause"));
        line.clear();
        read.read_line(&mut line).await.unwrap();
        drop(write);
        drop(read);
        task.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn only_explicit_ui_handshakes_attach_and_disconnect_their_own_peer() {
        let (tx, mut events) = mpsc::channel(8);
        let (updates, rx) = watch::channel(json!({"instance":"fixture","revision":"0"}));
        let (server, mut client) = tokio::net::UnixStream::pair().unwrap();
        let task = tokio::spawn(connection(server, tx.clone(), rx));
        client
            .write_all(b"{\"op\":\"hello\",\"protocol\":1,\"role\":\"shell\"}\n")
            .await
            .unwrap();
        let Some(Event::UiPeer(id, true)) = events.recv().await else {
            panic!("UI was not attached")
        };
        drop(client);
        let _ = task.await.unwrap();
        let Some(Event::UiPeer(closed, false)) = events.recv().await else {
            panic!("UI was not detached")
        };
        assert_eq!(id, closed);
        let (server, mut client) = tokio::net::UnixStream::pair().unwrap();
        let task = tokio::spawn(connection(server, tx, updates.subscribe()));
        client
            .write_all(b"{\"op\":\"hello\",\"protocol\":1}\n")
            .await
            .unwrap();
        drop(client);
        let _ = task.await.unwrap();
        assert!(matches!(events.recv().await, Some(Event::UiPeer(_, false))));
    }
    #[tokio::test]
    async fn socket_rejects_wrong_version_and_oversized_frames() {
        for data in [
            b"{\"op\":\"hello\",\"protocol\":2}\n".to_vec(),
            vec![b'a'; 65537],
        ] {
            let (server, mut client) = tokio::net::UnixStream::pair().unwrap();
            let (tx, _events) = mpsc::channel(4);
            let (_updates, rx) = watch::channel(json!({}));
            let task = tokio::spawn(connection(server, tx, rx));
            client.write_all(&data).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }
}
