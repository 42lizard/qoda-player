mod account;
mod activation;
mod albums;
mod alsa_guard;
mod archive;
mod artwork;
mod artwork_store;
mod buffered_reset;
mod cache_files;
mod catalog;
mod continuous;
mod gain;
mod gapless;
mod maintenance;
mod metadata_cache;
mod mpris;
mod observation;
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
use qoda_provider::{BrowserPin, Credential, MusicProvider};
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
    Unsupported(u64, bool),
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
    Maintenance(maintenance::Reply),
    Connected(u64, Result<account::Connected, ConnectError>, Credential),
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
fn identities(
    path: Option<&std::path::Path>,
) -> Result<HashMap<String, SourceId>, qoda_provider::ProviderError> {
    use qoda_provider::ProviderError::InvalidReply;
    let path = path.ok_or(InvalidReply)?;
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
fn save_identities(
    path: Option<&std::path::Path>,
    ids: &HashMap<String, SourceId>,
) -> std::io::Result<()> {
    let path = path.ok_or_else(|| std::io::Error::other("No config directory"))?;
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
        view["transcoded"] = json!(audio.transcoded);
        view["decoder_codec"] = json!(audio.decoder_codec);
        view["source_layout"] = json!(audio.decoder_layout);
        view["decoder_format"] = json!(audio.decoder_format);
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

    let mut playlists = playlists::Controller::default();
    let metadata = metadata_cache::Cache::start(metadata_cache::Paths::discover()).await;
    let cached_discovery = metadata
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
    let (mut durable, mut queue) = storage::Session::start(storage::path(), tx.clone()).await;
    state["storage"] = durable.view();
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
    let mut access = account::Controller::new(
        cached_discovery,
        vault::start(tx.clone()),
        identities_path(),
        Arc::new(account::Plex),
    );
    let mut maintenance = maintenance::Controller::default();
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
    let mut preparation = prepared_playback::Controller::default();
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
    let mut observations = observation::Controller::default();
    let mut reporting = reporting::Controller::new();
    state["reporting"] = reporting.view();
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
        access.start(c, tx.clone())?;
        access.publish(&mut state, &mut reporting);
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
                let tick = observations.tick(&mut observation::Runtime {
                    queue: &mut queue, player: &player, epochs: &mut epochs,
                    gapless: &mut gapless, proxy: &proxy, confirmed: &mut presented_generation,
                    playing: &mut desired_playing, reporting: &mut reporting, archive: &mut archive,
                    scope: observation::Scope {
                        account: access.generation(), provider: access.provider(),
                        can_raise: !ui_peers.is_empty() || ui_launcher.available(),
                    },
                }).await?;
                if tick.handoff {state["selection"]=queue.selected().and_then(|id|queue.visible_track(id).ok()).map(|track|json!(track)).unwrap_or(Value::Null);}
                if let Some(problem)=tick.problem {
                    state["error"]=json!(match problem {
                        observation::Problem::Handoff(error)=>error,
                        observation::Problem::Audio(error)=>output::explain_preflight(error,tick.audio.source,output_settings.selection.as_ref().is_some_and(output::Selection::conversion_allowed)),
                    });
                    state["intent"]=json!("paused");
                }
                state["reporting"]=tick.reporting;
                state["archive"]=tick.archive;
                if tick.changed {
                    state["audio"]=json!(tick.audio);
                    state["presentation"]=tick.presentation.presentation();
                    state["gapless"]=json!(gapless.view);
                    state["artwork_cache"]=artwork.view().await;
                    state["metadata_cache"]=json!(metadata.view());
                    revision=revision.checked_add(1).ok_or("Revision exhausted")?;
                    state["revision"]=json!(revision.to_string());updates.send_replace(state.clone());
                    if let Some(desktop)=&mut desktop {desktop.publish(tick.presentation);}
                }
                if let Some(event)=tick.event {event} else if tick.checkpoint {Event::Checkpoint} else {continue}
            },
            _ = checkpoint_tick.tick()=>Event::Checkpoint,
            _ = interrupts.recv() => break,
        };
        maintenance.observe_activity(&event, &mut state);
        let command_projection = if matches!(&event, Event::Request(_) | Event::Desktop(_)) {
            let before = observations
                .before_command(&mut observation::Runtime {
                    queue: &mut queue,
                    player: &player,
                    epochs: &mut epochs,
                    gapless: &mut gapless,
                    proxy: &proxy,
                    confirmed: &mut presented_generation,
                    playing: &mut desired_playing,
                    reporting: &mut reporting,
                    archive: &mut archive,
                    scope: observation::Scope {
                        account: access.generation(),
                        provider: access.provider(),
                        can_raise: !ui_peers.is_empty() || ui_launcher.available(),
                    },
                })
                .await;
            if before.handoff {
                state["selection"] = queue
                    .selected()
                    .and_then(|id| queue.visible_track(id).ok())
                    .map(|track| json!(track))
                    .unwrap_or(Value::Null);
            }
            if let Some(error) = before.error {
                state["error"] = json!(error);
            }
            Some(before.view)
        } else {
            None
        };
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
            let view = command_projection.as_ref().unwrap().clone();
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
                account: access.generation(),
                connected: access.provider().is_some(),
                credential: access.has_credential(),
                observations: &mut observations,
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
        if command_effects.stop_reporting {
            reporting.request_stop();
        }
        let mut to_prepare = command_effects.prepare;
        let mut automatic_next = command_effects.automatic;
        let mut force_save = command_effects.force_save;
        let seeked = command_effects.seeked;
        match event {
            Event::Search(reply) => {
                search.accept(reply, access.generation())?;
            }
            Event::Unsupported(generation, playing) => {
                preparation
                    .unsupported(
                        generation,
                        playing,
                        access.provider().map(|p| p as Arc<dyn MusicProvider>),
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
                            account: access.generation(),
                        },
                    )
                    .await?;
            }
            Event::StreamFailed(error, playing) => {
                preparation
                    .interrupted(
                        error,
                        playing,
                        access.provider().map(|p| p as Arc<dyn MusicProvider>),
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
                            account: access.generation(),
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
                            account: access.generation(),
                        },
                    )
                    .await;
                to_prepare = next.track;
                automatic_next = next.automatic;
            }
            Event::Gapless(reply) => {
                let completed = buffered_reset::Runtime {
                    queue: &mut queue,
                    player: &player,
                    gapless: &mut gapless,
                    epochs: &mut epochs,
                    proxy: &proxy,
                    confirmed: &mut presented_generation,
                    playing: &mut desired_playing,
                }
                .prepared(reply)
                .await?;
                if completed.reset {
                    state["intent"] = json!("paused");
                    state["audio"] = Value::Null;
                }
                if let Some(error) = completed.problem {
                    state["error"] = json!(error);
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
                if durable.accept(reply) {
                    state["storage"] = durable.view();
                    revision = revision.checked_add(1).ok_or("Revision exhausted")?;
                    state["revision"] = json!(revision.to_string());
                    updates.send_replace(state.clone());
                }
                continue;
            }
            Event::Checkpoint | Event::Artwork => {}
            Event::Maintenance(reply) => {
                maintenance
                    .complete(
                        reply,
                        &mut maintenance::Runtime {
                            state: &mut state,
                            account: &mut access,
                            catalog: &mut catalog,
                            albums: &mut albums,
                            search: &mut search,
                            artwork: &artwork,
                            metadata: &metadata,
                            events: &tx,
                        },
                    )
                    .await;
            }
            Event::Albums(reply) => {
                if albums.apply(reply, access.generation())
                    && let Some(provider) = access.provider()
                {
                    albums.view.covers =
                        artwork.publish(provider, albums.artwork_references()).await;
                }
            }
            Event::Playlists(reply) => {
                playlists.apply(reply, access.generation());
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
                let catalog_access = access.catalog_access();
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
                    "clear_artwork_cache"
                    | "set_artwork_budget"
                    | "clear_metadata_cache"
                    | "set_metadata_budget" => {
                        maintenance
                            .command(
                                &v,
                                &mut maintenance::Runtime {
                                    state: &mut state,
                                    account: &mut access,
                                    catalog: &mut catalog,
                                    albums: &mut albums,
                                    search: &mut search,
                                    artwork: &artwork,
                                    metadata: &metadata,
                                    events: &tx,
                                },
                            )
                            .await?;
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
                    "reconnect" | "restore_signin" | "save_signin" | "auth_start"
                    | "disconnect" | "forget_signin" => {
                        access
                            .command(
                                &v,
                                &mut account::Runtime {
                                    state: &mut state,
                                    tx: &tx,
                                    queue: &mut queue,
                                    player: &player,
                                    preparation: &mut preparation,
                                    reporting: &mut reporting,
                                    catalog: &mut catalog,
                                    epochs: &mut epochs,
                                    gapless: &mut gapless,
                                    proxy: &proxy,
                                    artwork: &artwork,
                                    search: &mut search,
                                    albums: &mut albums,
                                    playlists: &mut playlists,
                                    metadata: &metadata,
                                    archive: &mut archive,
                                    durable: &mut durable,
                                    desired_playing: &mut desired_playing,
                                },
                            )
                            .await?;
                    }
                    "playlist_list" | "playlist_open" | "playlist_refresh" | "playlist_create"
                    | "playlist_edit" => {
                        if matches!(v["op"].as_str(), Some("playlist_create" | "playlist_edit"))
                            && v["account_epoch"].as_str()
                                != Some(access.generation().to_string().as_str())
                        {
                            state["error"] = json!(
                                "The account changed. Refresh the app before editing a playlist."
                            );
                        } else if let Some(provider) = access.provider() {
                            if let Err(error) =
                                playlists.command(&v, provider, access.generation(), tx.clone())
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
                                albums.command(&v, provider, access.generation(), tx.clone())
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
                                access.generation(),
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
                    "archive_page" | "archive_export" | "archive_clear" | "archive_retention" => {
                        let result = if v["account_epoch"].as_str()
                            != Some(access.generation().to_string().as_str())
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
                    _ => {}
                }
            }
            event @ (Event::Pin(..)
            | Event::Credential(..)
            | Event::Connected(..)
            | Event::Vault(..)) => {
                access
                    .accept(
                        event,
                        &mut account::Runtime {
                            state: &mut state,
                            tx: &tx,
                            queue: &mut queue,
                            player: &player,
                            preparation: &mut preparation,
                            reporting: &mut reporting,
                            catalog: &mut catalog,
                            epochs: &mut epochs,
                            gapless: &mut gapless,
                            proxy: &proxy,
                            artwork: &artwork,
                            search: &mut search,
                            albums: &mut albums,
                            playlists: &mut playlists,
                            metadata: &metadata,
                            archive: &mut archive,
                            durable: &mut durable,
                            desired_playing: &mut desired_playing,
                        },
                    )
                    .await?;
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
                            account: access.generation(),
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
                    access.provider().map(|p| p as Arc<dyn MusicProvider>),
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
                        account: access.generation(),
                    },
                )
                .await?;
        }
        let view = observations
            .project(observation::Presentation {
                queue: &queue,
                player: &player,
                proxy: &proxy,
                account: access.generation(),
                generation: epochs.current,
                confirmed: &mut presented_generation,
                connected: access.provider().is_some(),
                playing: desired_playing,
                can_raise: !ui_peers.is_empty() || ui_launcher.available(),
            })
            .await;
        let audio = player.status();
        if desired_playing
            && presented_generation == Some(epochs.current)
            && audio.generation == epochs.current
            && !audio.idle
            && !audio.paused
            && audio.error.is_none()
            && let Some(provider) = access.provider()
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
        access.publish(&mut state, &mut reporting);
        reporting.fence_generation(epochs.current);
        state["reporting"] = reporting.view();
        durable.checkpoint(&queue, force_save);
        state["storage"] = durable.view();
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
    maintenance.shutdown(&artwork, &metadata).await;
    server.abort();
    let archive_checkpoint = archive.flush().await;
    let checkpoint = shutdown::checkpoint(durable.flush(&queue).await);
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
                ..Default::default()
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
            gain::PrepareError::Provider(qoda_provider::ProviderError::Unsupported)
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
