mod albums;
mod alsa_guard;
mod artwork;
mod artwork_store;
mod cache_files;
mod catalog;
mod gain;
mod gapless;
mod metadata_cache;
mod mpris;
mod output;
mod player;
mod playlists;
mod proxy;
mod queue;
mod recovery;
mod search;
mod source_format;
mod storage;
mod vault;
use player::{Command, Player};
use qoda_core::{OccurrenceId, SourceId, TrackRef};
use qoda_provider::{BrowserPin, Credential, MusicProvider, PlexProvider};
use serde_json::{Value, json};
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

enum Event {
    Request(Value),
    Desktop(mpris::Request),
    DesktopLost(uuid::Uuid),
    DesktopReady(Result<mpris::Service, ()>),
    UiPeer(uuid::Uuid, bool),
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
        Result<(PlexProvider, HashMap<String, SourceId>), qoda_provider::ProviderError>,
        Credential,
    ),
    Vault(vault::Reply),
    Page(
        u64,
        Result<qoda_provider::CatalogPage, qoda_provider::ProviderError>,
        metadata_cache::Info,
    ),
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
fn identities() -> HashMap<String, SourceId> {
    identities_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
fn save_identities(ids: &HashMap<String, SourceId>) -> std::io::Result<()> {
    let p = identities_path().ok_or_else(|| std::io::Error::other("No config directory"))?;
    let parent = p.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".sources-{}", uuid::Uuid::new_v4()));
    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)?;
    use std::io::Write;
    f.write_all(&serde_json::to_vec(ids)?)?;
    f.sync_all()?;
    std::fs::rename(tmp, p)
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
        let r = PlexProvider::connect(credential.clone(), identities()).await;
        let _ = tx.send(Event::Connected(generation, r, credential)).await;
    })
}
async fn prepare_track(
    player: &Player,
    proxy: &proxy::Proxy,
    pending: &mut Option<tokio::task::JoinHandle<()>>,
    generation: &mut gapless::Epochs,
    provider: Arc<dyn MusicProvider>,
    request: gain::Request,
    tx: mpsc::Sender<Event>,
) -> Result<(), &'static str> {
    let next = generation.allocate()?;
    if let Some(task) = pending.take() {
        task.abort();
    }
    generation.current = next;
    player.invalidate_load(next);
    player.send(Command::Pause(true));
    player.send(Command::Stop);
    proxy.revoke().await;
    *pending = Some(tokio::spawn(async move {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            gain::prepare(provider.as_ref(), request),
        )
        .await
        .unwrap_or(Err(gain::PrepareError::Provider(
            qoda_provider::ProviderError::Network,
        )));
        let _ = tx.send(Event::Media(next, result)).await;
    }));
    Ok(())
}
fn catalog_track(
    state: &Value,
    albums: &albums::Controller,
    request: &Value,
) -> Result<qoda_provider::Track, &'static str> {
    let reference = serde_json::from_value::<TrackRef>(
        json!({"source":request["source"],"key":request["key"]}),
    )
    .map_err(|_| "Choose a track from the current catalog page.")?;
    state["page"]["tracks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| serde_json::from_value::<qoda_provider::Track>(v.clone()).ok())
        .find(|t| t.reference == reference)
        .or_else(|| albums.track(&reference))
        .ok_or("That track is no longer on the displayed page. Browse again.")
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
        }
        if audio.entry_ready && !audio.idle && audio.error.is_none() {
            view["observed"] = json!(audio.output);
        }
    }
    view
}
async fn finish_gapless_reset(
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
fn navigation_request(
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
fn version_failure(error: &queue::Error, version: &str) -> Option<queue::Unavailable> {
    (*error == queue::Error::Version && !version.is_empty() && version.len() <= 512)
        .then_some(queue::Unavailable::VersionChanged)
}
fn preparation_failure(
    queue: &mut queue::Queue,
    reason: Option<queue::Unavailable>,
    automatic: bool,
    playing: bool,
) -> Result<Option<qoda_provider::Track>, queue::Error> {
    let Some(id) = queue.selected() else {
        return Ok(None);
    };
    if let Some(reason) = reason {
        queue.mark_unavailable(id, reason)?;
        if automatic && playing {
            return queue.advance();
        }
    }
    Ok(None)
}
fn unavailable_request(
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
fn guarded_queue_command(op: &str) -> bool {
    matches!(
        op,
        "play"
            | "play_selected"
            | "album_play"
            | "playlist_play"
            | "queue_play"
            | "replay"
            | "previous"
            | "navigation_select"
            | "retry_unavailable"
            | "skip_unavailable"
            | "next"
            | "seek"
            | "stop"
            | "stop_clear"
            | "queue_remove"
            | "clear_upcoming"
            | "queue_move"
            | "enqueue"
            | "play_next"
            | "enqueue_selected"
            | "play_next_selected"
            | "set_shuffle"
            | "set_repeat"
            | "set_gapless"
            | "set_replaygain"
            | "set_output"
            | "album_enqueue"
            | "playlist_enqueue"
            | "disconnect"
            | "forget_signin"
            | "search_play"
            | "search_next"
            | "search_enqueue"
            | "quit"
    )
}
fn changes_current(op: &str, request: &Value, queue: &queue::Queue) -> bool {
    matches!(
        op,
        "play"
            | "play_selected"
            | "album_play"
            | "playlist_play"
            | "queue_play"
            | "replay"
            | "previous"
            | "navigation_select"
            | "retry_unavailable"
            | "skip_unavailable"
            | "next"
            | "seek"
            | "stop"
            | "stop_clear"
            | "disconnect"
            | "forget_signin"
            | "quit"
            | "set_output"
            | "search_play"
    ) || op == "queue_remove"
        && serde_json::from_value::<OccurrenceId>(request["occurrence"].clone()).ok()
            == queue.selected()
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
fn check_queue(queue: &queue::Queue, request: &Value) -> Result<(), queue::Error> {
    let session = serde_json::from_value(request["queue_session"].clone())
        .map_err(|_| queue::Error::Conflict)?;
    let revision = serde_json::from_value(request["queue_revision"].clone())
        .map_err(|_| queue::Error::Conflict)?;
    queue.check(session, revision)
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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let credential = if std::env::args().any(|s| s == "--credential-stdin") {
        let mut input = Zeroizing::new(String::new());
        std::io::stdin().take(65537).read_to_string(&mut input)?;
        if input.len() > 65536 {
            return Err("Credential input too large".into());
        }
        Some(serde_json::from_str::<Credential>(&input).map_err(|_| "Invalid credential input")?)
    } else {
        None
    };
    let dir = runtime()?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir.join("backend.lock"))?;
    use std::os::fd::AsRawFd;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Qoda backend is already running".into());
    }
    if std::env::args().any(|s| s == "--restore-backup") {
        storage::restore_latest(&storage::path().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        println!(
            "Session backup restored. The original database was preserved. Next launch stays paused."
        );
        return Ok(());
    }
    let path = dir.join("backend.sock");
    if path.exists() {
        std::fs::remove_file(&path)?
    }
    let listener = tokio::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let player = Player::start()?;
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
    let mut albums = albums::Controller::with_cache(metadata.clone());
    let mut presented_generation = None;
    let mut catalog_revision = 0_u64;
    let mut state = json!({"protocol":1,"instance":instance,"revision":"0","status":"Connect Plex to browse music","libraries":[],"page":null,"selection":null,"intent":"paused","audio_profile":"Desktop / PipeWire","error":null,"signin_url":null,"sign_in":{"connected":false,"can_save":false,"keyring_action":null,"saved":"unknown","message":"Choose Restore to check for a saved sign-in."}});
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
    let mut navigation_offset = None;
    state["navigation"] = json!(queue.navigation_page(navigation_offset));
    state["queue"] = json!(queue.snapshot());
    state["playlists"] = json!(playlists.view);
    state["albums"] = json!(albums.view);
    state["account_epoch"] = json!("0");
    state["catalog"] = json!({"revision":"0","busy":false});
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
    let mut browse_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut prepare_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut auth_task: Option<tokio::task::JoinHandle<()>> = None;
    let (mut auth_generation, mut browse_generation) = (0_u64, 0_u64);
    let mut epochs = gapless::Epochs::default();
    let mut gapless = gapless::Controller::default();
    let mut recovery = recovery::Controller::default();
    state["recovery"] = json!(recovery.view);
    state["gapless"] = json!(gapless.view);
    let mut desired_playing = false;
    let mut automatic_preparation = None;
    let mut ui_peers = HashSet::new();
    state["ui"] = json!({"can_raise":false,"raise_token":null});
    let mut output_refresh = tokio::time::interval(std::time::Duration::from_secs(5));
    let mut output_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut audio_tick = tokio::time::interval(std::time::Duration::from_millis(250));
    let mut last_audio = player.status();
    let mut last_stream_failed = false;
    let mut desktop_clock = mpris::Clock::default();
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
                        if let Ok(before)=player.barrier().await {let _=finish_gapless_reset(&mut gapless,&mut queue,&mut epochs,&before,&mut presented_generation,&proxy).await;}
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
                let mut presentation_changed=false;
                if audio != last_audio || stream_failed!=last_stream_failed || clock_stale!=last_clock_stale {
                    last_stream_failed=stream_failed;
                    last_clock_stale=clock_stale;
                    presentation_changed=record_clock(&mut queue,&audio,epochs.current,&mut presented_generation,stream_failed);
                    gapless.observed(&audio,presented_generation);
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
                    let view=mpris::View::capture(&queue,&audio,epochs.current,presented_generation,provider.is_some(),desired_playing,stream_failed || clock_stale).with_ui(!ui_peers.is_empty());
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
            _ = tokio::signal::ctrl_c() => break,
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
        let mut desktop_reply = None;
        let mut desktop_action = None;
        let mut desktop_deadline = None;
        let mut desktop_owner = None;
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
            let view = mpris::View::capture(
                &queue,
                &player.status(),
                epochs.current,
                presented_generation,
                provider.is_some(),
                desired_playing,
                last_stream_failed || last_clock_stale,
            )
            .with_ui(!ui_peers.is_empty());
            if let Some(command) = mpris::translate(&request.action, &view, &queue, &instance) {
                desktop_deadline = Some(request.deadline);
                desktop_owner = Some(request.owner);
                desktop_action = Some(request.action);
                desktop_reply = Some(request.reply);
                Event::Request(command)
            } else {
                let _ = request.reply.send(Ok(()));
                Event::Checkpoint
            }
        } else {
            event
        };
        if let Event::Request(request) = &event
            && request["instance"] == instance
            && matches!(
                request["op"].as_str(),
                Some("search_play" | "search_next" | "search_enqueue")
            )
            && let Err(error) = check_queue(&queue, request)
                .map_err(|error| error.to_string())
                .and_then(|()| {
                    if provider.is_some() {
                        search
                            .resolve(request, auth_generation)
                            .map(|_| ())
                            .map_err(str::to_owned)
                    } else {
                        Err("Connect Plex before using search results.".into())
                    }
                })
        {
            state["error"] = json!(error);
            event = Event::Checkpoint;
        }
        if let Event::Request(request) = &event
            && request["instance"] == instance
            && catalog::is_batch(request["op"].as_str().unwrap_or(""))
            && let Err(error) = catalog::selected_tracks(&state, request, auth_generation)
        {
            state["error"] = json!(error);
            event = Event::Checkpoint;
        }
        if let Event::Request(request) = &mut event
            && request["instance"] == instance
            && matches!(request["op"].as_str(), Some("retry_stream" | "skip_stream"))
        {
            if recovery.view.phase != "exhausted"
                || check_queue(&queue, request).is_err()
                || serde_json::from_value::<OccurrenceId>(request["occurrence"].clone()).ok()
                    != queue.selected()
            {
                state["error"] = json!("Recovery selection changed; review the current queue.");
                event = Event::Checkpoint;
            } else {
                request["op"] = json!(if request["op"] == "retry_stream" {
                    "play"
                } else {
                    "next"
                });
            }
        }
        // Reject stale/invalid path rows before cancelling any speculative output.
        if let Event::Request(request) = &event
            && request["instance"] == instance
            && request["op"] == "navigation_select"
            && let Err(error) = navigation_request(&queue, request)
        {
            state["error"] = json!(error);
            event = Event::Checkpoint;
        }
        if let Event::Request(request) = &event
            && request["instance"] == instance
            && matches!(
                request["op"].as_str(),
                Some("retry_unavailable" | "skip_unavailable")
            )
            && let Err(error) = unavailable_request(&queue, request)
        {
            state["error"] = json!(error);
            event = Event::Checkpoint;
        }
        if let Event::Request(request) = &event
            && request["instance"] == instance
            && request["op"] == "set_output"
        {
            match output_settings.request(request, &output_devices) {
                Ok(selection)
                    if output_settings.selection.as_ref() == Some(&selection)
                        && player.status().output_selection.as_ref() == Some(&selection) =>
                {
                    state["error"] = Value::Null;
                    event = Event::Checkpoint;
                }
                Ok(selection) => {
                    if let Err(error) = output_settings.set(selection) {
                        state["error"] = json!(error);
                        event = Event::Checkpoint;
                    }
                }
                Err(error) => {
                    state["error"] = json!(error);
                    event = Event::Checkpoint;
                }
            }
        }
        // Validate and persist before touching speculative output. A failed
        // save or an unchanged mode must not cancel/rebuild a staged successor.
        if let Event::Request(request) = &event
            && request["instance"] == instance
            && request["op"] == "set_replaygain"
        {
            match serde_json::from_value::<gain::Mode>(request["mode"].clone()) {
                Ok(mode) if mode == gain_settings.mode => {
                    state["error"] = Value::Null;
                    event = Event::Checkpoint;
                }
                Ok(mode) => {
                    if let Err(error) = gain_settings.set(mode) {
                        state["error"] = json!(error);
                        event = Event::Checkpoint;
                    }
                }
                Err(_) => {
                    state["error"] = json!("Choose Off, Track, Album or Auto ReplayGain.");
                    event = Event::Checkpoint;
                }
            }
        }
        if let Event::Request(request) = &event
            && request["instance"] == instance
            && (guarded_queue_command(request["op"].as_str().unwrap_or(""))
                || request["op"] == "pause")
            && (request.get("queue_session").is_none() || check_queue(&queue, request).is_ok())
            && recovery.cancel()
        {
            desired_playing = false;
            player.send(Command::Pause(true));
            player.send(Command::Stop);
            epochs.reset()?;
            player.invalidate_load(epochs.current);
            gapless.invalidate();
            proxy.revoke().await;
            presented_generation = None;
            state["audio"] = Value::Null;
            state["intent"] = json!("paused");
            state["status"] = json!("Recovery cancelled; resume explicitly.");
        }
        if let Event::Request(request) = &event
            && request["instance"] == instance
            && guarded_queue_command(request["op"].as_str().unwrap_or(""))
            && (request.get("queue_session").is_none() || check_queue(&queue, request).is_ok())
        {
            let op = request["op"].as_str().unwrap_or("");
            let cancelled = gapless.cancel(&player, &proxy).await;
            let reset = match cancelled {
                Ok(Some(before)) => Some(Ok(before)),
                Ok(None)
                    if gapless.chained
                        && changes_current(op, request, &queue)
                        && presented_generation != Some(epochs.current) =>
                {
                    Some(player.barrier().await)
                }
                Ok(None) => None,
                Err(_) => Some(player.barrier().await),
            };
            if let Some(reset) = reset {
                desired_playing = false;
                match reset {
                    Ok(before) => {
                        if let Err(error) = finish_gapless_reset(
                            &mut gapless,
                            &mut queue,
                            &mut epochs,
                            &before,
                            &mut presented_generation,
                            &proxy,
                        )
                        .await
                        {
                            state["error"] = json!(error);
                            event = Event::Checkpoint;
                        }
                    }
                    Err(error) => {
                        gapless.invalidate();
                        proxy.revoke().await;
                        epochs.reset()?;
                        player.send(Command::Pause(true));
                        gapless.review();
                        state["error"] = json!(error);
                        event = Event::Checkpoint;
                    }
                }
                state["audio"] = Value::Null;
                state["intent"] = json!("paused");
                state["status"] = json!("Buffered transition reset; playback paused for review.");
            }
        }
        if let Some(action) = &desktop_action {
            if desktop_deadline.is_some_and(|deadline| deadline <= tokio::time::Instant::now())
                || !desktop_owner.is_some_and(|owner| {
                    desktop
                        .as_ref()
                        .is_some_and(|service| service.id() == owner && service.is_live())
                })
                || desktop_reply
                    .as_ref()
                    .is_some_and(|reply| reply.is_closed())
            {
                event = Event::Checkpoint;
                state["error"] = json!(
                    "The desktop command expired or its connection closed before queue mutation."
                );
            } else if matches!(event, Event::Request(_)) {
                let view = mpris::View::capture(
                    &queue,
                    &player.status(),
                    epochs.current,
                    presented_generation,
                    provider.is_some(),
                    desired_playing,
                    last_stream_failed || last_clock_stale,
                )
                .with_ui(!ui_peers.is_empty());
                event = mpris::translate(action, &view, &queue, &instance)
                    .map(Event::Request)
                    .unwrap_or(Event::Checkpoint);
            }
        }
        if let Event::Recovery(reply) = event {
            let generation = reply.key.generation;
            if !recovery.owns(reply.key) {
                event = Event::Checkpoint;
            } else if !reply.key.matches(&queue, auth_generation, epochs.current) {
                recovery.cancel();
                event = Event::Checkpoint;
            } else {
                event = match recovery.accept(reply) {
                    Some((playing, Ok(prepared))) => {
                        desired_playing = playing;
                        state["error"] = Value::Null;
                        Event::Media(generation, Ok(prepared))
                    }
                    Some((_, Err(error))) => {
                        desired_playing = false;
                        state["intent"] = json!("paused");
                        state["error"] = json!(error.message());
                        state["status"] = json!("Recovery exhausted; Retry or Skip explicitly.");
                        if let Some(reason) = error.reason()
                            && let Some(id) = queue.selected()
                        {
                            let _ = queue.mark_unavailable(id, reason);
                        }
                        Event::Checkpoint
                    }
                    None => Event::Checkpoint,
                };
            }
        }
        let mut to_prepare = None;
        let mut automatic_next = false;
        let mut force_save = false;
        let mut seeked = false;
        match event {
            Event::Search(reply) => {
                search.accept(reply, auth_generation)?;
            }
            Event::StreamFailed(error, playing) => {
                desired_playing = false;
                if let Some(task) = prepare_task.take() {
                    task.abort();
                }
                automatic_preparation = None;
                let _ = player.barrier().await;
                gapless.invalidate();
                proxy.revoke().await;
                epochs.reset()?;
                player.invalidate_load(epochs.current);
                presented_generation = None;
                state["audio"] = Value::Null;
                state["intent"] = json!("paused");
                if let (Some(provider), Some(key), Some(version), Some(track)) = (
                    provider.clone(),
                    recovery::Key::capture(&queue, auth_generation, epochs.current),
                    queue.selected_version().map(str::to_owned),
                    queue.selected().and_then(|id| queue.visible_track(id).ok()),
                ) {
                    if let Some(reason) = queue::Unavailable::from_provider(error) {
                        let _ = queue.mark_unavailable(queue.selected().unwrap(), reason);
                    }
                    recovery.begin(
                        recovery::Work {
                            key,
                            version,
                            playing,
                            failure: error,
                            request: gain::Request {
                                reference: track.reference,
                                mode: gain_settings.mode,
                                album_session: queue.album_gain_eligible(),
                                fidelity: output_settings
                                    .selection
                                    .as_ref()
                                    .is_some_and(output::Selection::fidelity),
                            },
                        },
                        provider,
                        tx.clone(),
                    );
                    state["error"] = if recovery.view.phase == "recovering" {
                        Value::Null
                    } else {
                        json!(error.to_string())
                    };
                    state["status"] = json!(recovery.view.message);
                } else {
                    state["error"] = json!(
                        "Stream interrupted without a pinned recovery target; retry explicitly."
                    );
                }
            }
            Event::Gapless(reply) => {
                let unavailable = gapless.unavailable_reply(&reply, &queue, &epochs);
                if let Err(error) = gapless.ready(reply, &queue, &epochs, &player, &proxy).await {
                    desired_playing = false;
                    match player.barrier().await {
                        Ok(before) => {
                            let _ = finish_gapless_reset(
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
            Event::UiPeer(peer, attached) => {
                let changed = if attached {
                    ui_peers.insert(peer)
                } else {
                    ui_peers.remove(&peer)
                };
                if !changed {
                    continue;
                }
                state["ui"]["can_raise"] = json!(!ui_peers.is_empty());
            }
            Event::DesktopLost(id)
                if desktop.as_ref().is_some_and(|service| service.id() == id) =>
            {
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
                        if let Some(reply) = desktop_reply.take() {
                            let _ = reply.send(Ok(()));
                        }
                        break;
                    }
                    "show_ui" => {
                        if !ui_peers.is_empty() {
                            state["ui"]["raise_token"] = json!(uuid::Uuid::new_v4().to_string());
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
                                let epoch = if clear {
                                    if let Some(task) = browse_task.take() {
                                        task.abort();
                                    }
                                    browse_generation = browse_generation
                                        .checked_add(1)
                                        .ok_or("Browse generation exhausted")?;
                                    catalog_revision = catalog_revision
                                        .checked_add(1)
                                        .ok_or("Catalog revision exhausted")?;
                                    state["page"] = Value::Null;
                                    state["catalog"] = json!({"revision":catalog_revision.to_string(),"busy":false});
                                    albums.invalidate();
                                    search.invalidate()?;
                                    artwork.revoke(false).await;
                                    state["status"] = json!(
                                        "Metadata cache clearing. Browse again to load catalog pages."
                                    );
                                    metadata.begin_clear()
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
                    "set_output" => {
                        desired_playing = false;
                        automatic_preparation = None;
                        if let Some(task) = prepare_task.take() {
                            task.abort();
                        }
                        match player.barrier().await {
                            Ok(before) => {
                                record_clock(
                                    &mut queue,
                                    &before,
                                    epochs.current,
                                    &mut presented_generation,
                                    last_stream_failed || last_clock_stale,
                                );
                            }
                            Err(error) => state["error"] = json!(error),
                        }
                        epochs.reset()?;
                        player.invalidate_load(epochs.current);
                        gapless.invalidate();
                        proxy.revoke().await;
                        presented_generation = None;
                        if let Err(error) =
                            player.configure(output_settings.selection.clone()).await
                        {
                            state["error"] = json!(error);
                        }
                        state["audio"] = Value::Null;
                        state["intent"] = json!("paused");
                        state["status"] =
                            json!("Output changed; position retained. Choose Resume explicitly.");
                    }
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
                    "playlist_play" | "playlist_enqueue" => {
                        let result: Result<(), String> = (|| {
                            check_queue(&queue, &v).map_err(|e| e.to_string())?;
                            let reference = serde_json::from_value::<qoda_provider::PlaylistRef>(
                                v["reference"].clone(),
                            )
                            .map_err(|_| "Choose a loaded playlist.")?;
                            let snapshot = playlists.checked_snapshot(
                                &reference,
                                v["expected_revision"].as_str().unwrap_or_default(),
                            )?;
                            let start = if let Some(id) = v["item_id"].as_str() {
                                snapshot
                                    .entries
                                    .iter()
                                    .position(|e| e.id == id)
                                    .ok_or("This playlist entry changed. Refresh first.")?
                            } else {
                                0
                            };
                            let tracks=snapshot.entries[start..].iter().map(|entry|entry.track.clone().ok_or_else(||"Some playlist source references could not be resolved; the snapshot was not partially queued.".to_owned())).collect::<Result<Vec<_>,_>>()?;
                            if v["op"] == "playlist_play" {
                                if provider.is_none() {
                                    return Err("Restore Plex sign-in before playback.".into());
                                }
                                to_prepare =
                                    queue.replace_many(tracks).map_err(|e| e.to_string())?;
                                desired_playing = true;
                            } else {
                                queue.enqueue_many(tracks).map_err(|e| e.to_string())?;
                            }
                            Ok(())
                        })();
                        if let Err(error) = result {
                            state["error"] = json!(error);
                        }
                    }
                    "album_list" | "album_open" | "artist_list" | "artist_open" => {
                        if let Some(provider) = provider.clone() {
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
                    "album_play" | "album_enqueue" => {
                        let result: Result<(), String> = (|| {
                            check_queue(&queue, &v).map_err(|e| e.to_string())?;
                            let tracks = albums.tracks(&v)?;
                            if provider.is_none() {
                                return Err("Connect Plex before queueing an album.".into());
                            }
                            if v["op"] == "album_play" {
                                // tracks() already validated this complete live snapshot and revision.
                                let album = albums
                                    .view
                                    .selected
                                    .as_ref()
                                    .ok_or("Missing album snapshot")?;
                                to_prepare = queue
                                    .replace_album(
                                        tracks,
                                        album.album.reference.clone(),
                                        album
                                            .tracks
                                            .iter()
                                            .map(|entry| entry.track.reference.clone())
                                            .collect(),
                                    )
                                    .map_err(|e| e.to_string())?;
                                desired_playing = true;
                            } else {
                                queue.enqueue_many(tracks).map_err(|e| e.to_string())?;
                            }
                            Ok(())
                        })();
                        if let Err(error) = result {
                            state["error"] = json!(error);
                        }
                    }
                    "search_all" => {
                        if let Some(provider) = provider.clone() {
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
                    "search_play" | "search_next" | "search_enqueue" => {
                        let result = check_queue(&queue, &v)
                            .map_err(|error| error.to_string())
                            .and_then(|()| {
                                search.resolve(&v, auth_generation).map_err(str::to_owned)
                            });
                        match result {
                            Ok(track) => {
                                let result = match v["op"].as_str() {
                                    Some("search_play") => queue.replace(track.clone()).map(|_| {
                                        desired_playing = true;
                                        to_prepare = Some(track);
                                    }),
                                    Some("search_next") => queue.enqueue(track, true).map(|_| ()),
                                    _ => queue.enqueue(track, false).map(|_| ()),
                                };
                                if let Err(error) = result {
                                    state["error"] = json!(error.to_string());
                                }
                            }
                            Err(error) => state["error"] = json!(error),
                        }
                    }
                    "browse" => {
                        if let Some(p) = provider.clone()
                            && let Ok(source) =
                                serde_json::from_value::<SourceId>(v["source"].clone())
                        {
                            if albums.source_changed(source) {
                                artwork.revoke(false).await;
                            }
                            let Ok(offset) =
                                serde_json::from_value::<qoda_core::Counter>(v["offset"].clone())
                            else {
                                continue;
                            };
                            let offset = offset.get();
                            let query: String = v["query"]
                                .as_str()
                                .unwrap_or("")
                                .chars()
                                .take(512)
                                .collect();
                            if let Some(task) = browse_task.take() {
                                task.abort();
                            }
                            browse_generation = browse_generation
                                .checked_add(1)
                                .ok_or("Generation exhausted")?;
                            let g = browse_generation;
                            let tx = tx.clone();
                            state["status"] = json!("Loading tracks");
                            state["page"] = Value::Null;
                            catalog_revision = catalog_revision
                                .checked_add(1)
                                .ok_or("Catalog revision exhausted")?;
                            state["catalog"] = json!({"revision":catalog_revision.to_string(),"busy":true,"refreshing":false});
                            let cache = metadata.clone();
                            browse_task = Some(tokio::spawn(async move {
                                cache
                                    .read(
                                        p,
                                        metadata_cache::Query::Tracks {
                                            source,
                                            offset,
                                            query,
                                        },
                                        move |result, info| {
                                            let tx = tx.clone();
                                            async move {
                                                let result = result.and_then(|page| match page {
                                                    metadata_cache::Page::Tracks(page) => Ok(page),
                                                    _ => Err(
                                                        qoda_provider::ProviderError::InvalidReply,
                                                    ),
                                                });
                                                let _ = tx.send(Event::Page(g, result, info)).await;
                                            }
                                        },
                                    )
                                    .await;
                            }));
                        }
                    }
                    "play_selected" | "enqueue_selected" | "play_next_selected" => {
                        let result: Result<(), String> = (|| {
                            check_queue(&queue, &v).map_err(|error| error.to_string())?;
                            let tracks = catalog::selected_tracks(&state, &v, auth_generation)
                                .map_err(str::to_owned)?;
                            let count = tracks.len();
                            match v["op"].as_str() {
                                Some("play_selected") => {
                                    to_prepare = queue
                                        .replace_many(tracks)
                                        .map_err(|error| error.to_string())?;
                                    desired_playing = true;
                                }
                                Some("play_next_selected") => queue
                                    .enqueue_next_many(tracks)
                                    .map_err(|error| error.to_string())?,
                                _ => queue
                                    .enqueue_many(tracks)
                                    .map_err(|error| error.to_string())?,
                            }
                            state["status"] =
                                json!(format!("Applied selection of {count} tracks."));
                            Ok(())
                        })();
                        if let Err(error) = result {
                            state["error"] = json!(error);
                        }
                    }
                    "play" => {
                        if provider.is_some() {
                            match catalog_track(&state, &albums, &v) {
                                Ok(track) => match queue.replace(track.clone()) {
                                    Ok(_) => {
                                        desired_playing = true;
                                        to_prepare = Some(track)
                                    }
                                    Err(e) => state["error"] = json!(e.to_string()),
                                },
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
                    "enqueue" | "play_next" | "queue_play" | "queue_move" | "queue_remove"
                    | "clear_upcoming" | "stop_clear" | "next" | "replay" | "previous"
                    | "navigation_select" | "retry_unavailable" | "skip_unavailable"
                    | "set_shuffle" | "set_repeat" => {
                        if let Err(error) = check_queue(&queue, &v) {
                            state["error"] = json!(error.to_string());
                        } else {
                            let result: Result<(), String> = (|| {
                                match v["op"].as_str().unwrap_or("") {
                                    "enqueue" | "play_next" => {
                                        if provider.is_none() {
                                            return Err("Connect Plex before adding tracks.".into());
                                        }
                                        let track = catalog_track(&state, &albums, &v)
                                            .map_err(str::to_owned)?;
                                        queue
                                            .enqueue(track, v["op"] == "play_next")
                                            .map_err(|e| e.to_string())?;
                                    }
                                    "queue_play" => {
                                        if provider.is_none() {
                                            return Err(
                                                "Restore Plex sign-in before playback.".into()
                                            );
                                        }
                                        let id = serde_json::from_value::<OccurrenceId>(
                                            v["occurrence"].clone(),
                                        )
                                        .map_err(|_| queue::Error::Missing.to_string())?;
                                        to_prepare =
                                            Some(queue.select(id).map_err(|e| e.to_string())?);
                                        desired_playing = true;
                                    }
                                    "retry_unavailable" | "skip_unavailable" => {
                                        let id = unavailable_request(&queue, &v)?;
                                        if provider.is_none() {
                                            return Err("Restore Plex sign-in before retrying or continuing playback.".into());
                                        }
                                        to_prepare = if let Some(id) = id {
                                            Some(
                                                queue
                                                    .retry_unavailable(id)
                                                    .map_err(|error| error.to_string())?,
                                            )
                                        } else {
                                            automatic_next = true;
                                            queue.advance().map_err(|error| error.to_string())?
                                        };
                                        desired_playing = to_prepare.is_some();
                                        if !desired_playing {
                                            state["status"] = json!(
                                                "No playable entries remain. Open Unavailable to retry or remove an entry."
                                            );
                                        }
                                    }
                                    "queue_move" => {
                                        let id = serde_json::from_value(v["occurrence"].clone())
                                            .map_err(|_| queue::Error::Missing.to_string())?;
                                        let before = serde_json::from_value(v["before"].clone())
                                            .map_err(|_| queue::Error::Missing.to_string())?;
                                        queue.move_before(id, before).map_err(|e| e.to_string())?;
                                    }
                                    "queue_remove" => {
                                        let id = serde_json::from_value(v["occurrence"].clone())
                                            .map_err(|_| queue::Error::Missing.to_string())?;
                                        if queue.remove(id).map_err(|e| e.to_string())? {
                                            to_prepare = queue
                                                .selected()
                                                .map(|id| queue.visible_track(id))
                                                .transpose()
                                                .map_err(|e| e.to_string())?;
                                            if to_prepare.is_none() {
                                                desired_playing = false;
                                            }
                                        }
                                    }
                                    "clear_upcoming" => {
                                        queue.clear_upcoming().map_err(|e| e.to_string())?
                                    }
                                    "stop_clear" => {
                                        queue = queue.empty_session();
                                        desired_playing = false;
                                    }
                                    "previous" => {
                                        let audio = player.status();
                                        if audio.generation == epochs.current
                                            && let Some(position) = audio.position_seconds
                                        {
                                            let _ = queue.set_position(position);
                                        }
                                        to_prepare = queue.previous().map_err(|e| e.to_string())?;
                                    }
                                    "navigation_select" => {
                                        let (index, occurrence) = navigation_request(&queue, &v)?;
                                        to_prepare = Some(
                                            queue
                                                .revisit(index, occurrence)
                                                .map_err(|error| error.to_string())?,
                                        );
                                    }
                                    "set_shuffle" => {
                                        let enabled = v["enabled"]
                                            .as_bool()
                                            .ok_or("Choose shuffle on or off.")?;
                                        queue.set_shuffle(enabled).map_err(|e| e.to_string())?;
                                    }
                                    "set_repeat" => {
                                        let mode = serde_json::from_value::<queue::Repeat>(
                                            v["mode"].clone(),
                                        )
                                        .map_err(|_| "Choose Repeat Off, One or All.")?;
                                        queue.set_repeat(mode).map_err(|e| e.to_string())?;
                                    }
                                    "next" | "replay" => {
                                        if v["op"] == "replay" && provider.is_none() {
                                            return Err(
                                                "Restore Plex sign-in before playback.".into()
                                            );
                                        }
                                        to_prepare = if v["op"] == "replay" {
                                            queue.replay()
                                        } else {
                                            queue.advance()
                                        }
                                        .map_err(|e| e.to_string())?;
                                        if v["op"] == "replay" && to_prepare.is_some() {
                                            desired_playing = true;
                                        }
                                        if to_prepare.is_none() {
                                            desired_playing = false;
                                            state["status"] = json!("End of queue");
                                        }
                                    }
                                    _ => {}
                                }
                                Ok(())
                            })();
                            if let Err(error) = result {
                                state["error"] = json!(error);
                            }
                            if v["op"] == "stop_clear"
                                || (v["op"] == "queue_remove" && queue.selected().is_none())
                                || (matches!(v["op"].as_str(), Some("next" | "skip_unavailable"))
                                    && to_prepare.is_none())
                            {
                                if let Some(task) = prepare_task.take() {
                                    task.abort();
                                }
                                epochs.reset()?;
                                gapless.invalidate();
                                player.send(Command::Pause(true));
                                player.send(Command::Stop);
                                proxy.revoke().await;
                                state["selection"] = Value::Null;
                                state["audio"] = Value::Null;
                            }
                            state["intent"] =
                                json!(if desired_playing { "playing" } else { "paused" });
                        }
                    }
                    "stop" => {
                        if let Some(task) = prepare_task.take() {
                            task.abort();
                        }
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
                        let audio = player.status();
                        if audio.generation == epochs.current
                            && let Some(position) = audio.position_seconds
                        {
                            let _ = queue.set_position(position);
                        }
                        force_save = true;
                        desired_playing = false;
                        player.send(Command::Pause(true));
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
                            let preparing = prepare_task
                                .as_ref()
                                .is_some_and(|task| !task.is_finished());
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
                    "seek" => {
                        if let Some(position) = v["seconds"].as_f64() {
                            match queue.seek_to(position) {
                                Ok(()) => {
                                    force_save = true;
                                    seeked = true;
                                    if provider.is_some() {
                                        to_prepare = queue
                                            .selected()
                                            .and_then(|id| queue.visible_track(id).ok());
                                    }
                                }
                                Err(error) => state["error"] = json!(error.to_string()),
                            }
                        }
                    }
                    "disconnect" | "forget_signin" => {
                        let forget = v["op"] == "forget_signin";
                        for task in [browse_task.take(), prepare_task.take(), auth_task.take()]
                            .into_iter()
                            .flatten()
                        {
                            task.abort();
                        }
                        auth_generation = auth_generation
                            .checked_add(1)
                            .ok_or("Generation exhausted")?;
                        browse_generation = browse_generation
                            .checked_add(1)
                            .ok_or("Generation exhausted")?;
                        epochs.reset()?;
                        gapless.invalidate();
                        provider = None;
                        search.invalidate()?;
                        playlists.invalidate();
                        albums.invalidate();
                        player.send(Command::Pause(true));
                        player.send(Command::Stop);
                        tokio::join!(artwork.revoke(true), metadata.clear());
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
                        state["page"] = Value::Null;
                        state["catalog"] =
                            json!({"revision":browse_generation.to_string(),"busy":false});
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
                    if save_identities(&ids).is_err() {
                        state["error"] = json!("Library identities could not be saved");
                    }
                    search.invalidate()?;
                    state["libraries"] = json!(p.libraries());
                    state["status"] = json!("Choose a music library");
                    provider = Some(Arc::new(p));
                    current_credential = Some(credential);
                    state["sign_in"]["connected"] = json!(true);
                    state["sign_in"]["can_save"] = json!(true);
                    if state["sign_in"]["saved"] != "present" {
                        state["sign_in"]["message"] =
                            json!("Connected for this session. Save sign-in to reconnect later.");
                    }
                }
                Err(e) => {
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
            Event::Page(g, r, info) if g == browse_generation => {
                state["catalog"]["busy"] = json!(false);
                state["catalog"]["refreshing"] = json!(info.refreshing);
                state["catalog"]["cache"] = json!(info);
                match r {
                    Ok(page) => {
                        catalog::publish_page(&mut state, &mut catalog_revision, Some(page))?;
                        state["status"] = json!(if info.origin == "live" {
                            "Ready"
                        } else {
                            info.message
                        });
                    }
                    Err(error) => {
                        state["status"] = json!("Browse failed");
                        state["error"] = json!(error.to_string());
                        if matches!(
                            error,
                            qoda_provider::ProviderError::AccessDenied
                                | qoda_provider::ProviderError::UnknownSource
                        ) {
                            catalog::publish_page(&mut state, &mut catalog_revision, None)?;
                        }
                    }
                }
            }
            Event::OutputDevices(devices) => {
                output_devices = devices;
            }
            Event::Media(g, r) if g == epochs.current => match r {
                Ok(prepared) => {
                    let gain::Prepared {
                        media,
                        gain,
                        source,
                    } = prepared;
                    if let Err(error) = queue.update_selected(media.track.clone(), &media.version) {
                        let reason = version_failure(&error, &media.version);
                        match preparation_failure(
                            &mut queue,
                            reason,
                            automatic_preparation == Some(g),
                            desired_playing,
                        ) {
                            Ok(next) => {
                                state["error"] = if next.is_some() {
                                    Value::Null
                                } else {
                                    json!(error.to_string())
                                };
                                to_prepare = next;
                            }
                            Err(failure) => state["error"] = json!(failure.to_string()),
                        }
                        automatic_next = to_prepare.is_some();
                        desired_playing = automatic_next;
                        state["intent"] = json!(if desired_playing { "playing" } else { "paused" });
                    } else {
                        state["selection"] = json!(media.track);
                        let url = proxy.grant(media.stream).await;
                        if let Err(error) = player.load_prepared(player::Load {
                            generation: g,
                            url,
                            position: queue.position(),
                            gain,
                            source,
                            output: output_settings.selection.clone(),
                            playing: desired_playing,
                        }) {
                            desired_playing = false;
                            state["intent"] = json!("paused");
                            state["error"] = json!(error);
                        }
                        state["status"] = json!("Track selected; playback requested");
                    }
                }
                Err(e) => {
                    match preparation_failure(
                        &mut queue,
                        e.provider().and_then(queue::Unavailable::from_provider),
                        automatic_preparation == Some(g),
                        desired_playing,
                    ) {
                        Ok(next) => {
                            state["error"] = if next.is_some() {
                                Value::Null
                            } else {
                                json!(e.to_string())
                            };
                            to_prepare = next;
                        }
                        Err(error) => state["error"] = json!(error.to_string()),
                    }
                    automatic_next = to_prepare.is_some();
                    desired_playing = automatic_next;
                    state["status"] = json!(if automatic_next {
                        "Skipping unavailable entry"
                    } else if e.provider().is_none() {
                        "Fidelity verification failed; review audio settings and explicitly retry"
                    } else {
                        "Playback preparation failed; review unavailable entries or retry"
                    });
                    state["intent"] = json!(if desired_playing { "playing" } else { "paused" });
                }
            },
            _ => continue,
        }
        // A normal queue visit gets a fresh recovery budget; recovery reloads
        // enter Event::Media directly and retain the existing budget.
        if to_prepare.is_some() {
            recovery.cancel();
        }
        if let Some(track) = to_prepare.as_ref()
            && provider.is_none()
        {
            state["selection"] = json!(track);
            state["audio"] = Value::Null;
            state["status"] = json!("Selection updated locally. Restore Plex sign-in to play.");
        }
        if let Some(track) = to_prepare
            && let Some(provider) = provider.clone()
        {
            gapless.invalidate();
            prepare_track(
                &player,
                &proxy,
                &mut prepare_task,
                &mut epochs,
                provider,
                gain::Request {
                    reference: track.reference,
                    mode: gain_settings.mode,
                    album_session: queue.album_gain_eligible(),
                    fidelity: output_settings
                        .selection
                        .as_ref()
                        .is_some_and(output::Selection::fidelity),
                },
                tx.clone(),
            )
            .await?;
            automatic_preparation = (automatic_next && desired_playing).then_some(epochs.current);
            state["selection"] = Value::Null;
            state["audio"] = Value::Null;
            state["intent"] = json!(if desired_playing { "playing" } else { "paused" });
            state["status"] = json!("Preparing selected queue entry");
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
        .with_ui(!ui_peers.is_empty());
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
        state["navigation"] = json!(queue.navigation_page(navigation_offset));
        state["queue"] = json!(queue.snapshot());
        state["recovery"] = json!(recovery.view);
        state["search"] = search.view();
        state["playlists"] = json!(playlists.view);
        albums.view.covers = artwork.urls().await;
        state["albums"] = json!(albums.view);
        state["artwork_cache"] = artwork.view().await;
        state["metadata_cache"] = json!(metadata.view());
        state["account_epoch"] = json!(auth_generation.to_string());
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
        if let Some(reply) = desktop_reply.take() {
            let result = state["error"]
                .as_str()
                .map(|error| Err(error.to_owned()))
                .unwrap_or(Ok(()));
            let _ = reply.send(result);
        }
    }
    let audio = player.status();
    if audio.generation == epochs.current
        && let Some(position) = audio.position_seconds
    {
        let _ = queue.set_position(position);
    }
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
    match durable.flush(queue.clone(), purge_epoch).await {
        Err(error) => eprintln!("Qoda: final checkpoint failed: {error}"),
        Ok(Some(warning)) => eprintln!("Qoda: {warning}"),
        Ok(None) => {}
    }
    if let Some(desktop) = desktop {
        desktop.shutdown().await;
    }
    drop(player);
    std::fs::remove_file(path)?;
    drop(lock);
    Ok(())
}

#[cfg(test)]
mod tests {
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
                preparation_failure(&mut q, queue::Unavailable::from_provider(error), true, true)
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
                preparation_failure(
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
        assert!(version_failure(&queue::Error::Version, "").is_none());
        assert!(version_failure(&queue::Error::Version, &"x".repeat(513)).is_none());
        assert_eq!(
            version_failure(&queue::Error::Version, "valid-other-version"),
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
                let next =
                    preparation_failure(&mut q, Some(queue::Unavailable::Missing), true, true)
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
        assert!(guarded_queue_command("retry_unavailable"));
        assert!(changes_current("skip_unavailable", &request, &q));
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
        assert!(guarded_queue_command("navigation_select"));
        assert!(changes_current("navigation_select", &request, &q));
        assert!(!guarded_queue_command("navigation_page"));
        assert!(!changes_current("navigation_page", &request, &q));
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
