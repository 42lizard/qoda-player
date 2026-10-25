mod albums;
mod artwork;
mod gapless;
mod mpris;
mod player;
mod playlists;
mod proxy;
mod queue;
mod storage;
mod vault;
use player::{Command, Player};
use qoda_core::{OccurrenceId, SourceId, TrackRef};
use qoda_provider::{BrowserPin, Credential, MusicProvider, PlexProvider, PreparedMedia};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
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
    DesktopLost,
    Advance(u64),
    Stored(storage::Reply),
    Checkpoint,
    Playlists(playlists::Reply),
    Albums(albums::Reply),
    Gapless(gapless::Reply),
    Connected(
        u64,
        Result<(PlexProvider, HashMap<String, SourceId>), qoda_provider::ProviderError>,
        Credential,
    ),
    Vault(vault::Reply),
    Page(
        u64,
        Result<qoda_provider::CatalogPage, qoda_provider::ProviderError>,
    ),
    Media(u64, Result<PreparedMedia, qoda_provider::ProviderError>),
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
    mut updates: watch::Receiver<Value>,
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
    reference: TrackRef,
    tx: mpsc::Sender<Event>,
) -> Result<(), &'static str> {
    let next = generation.allocate()?;
    if let Some(task) = pending.take() {
        task.abort();
    }
    generation.current = next;
    player.send(Command::Pause(true));
    player.send(Command::Stop);
    proxy.revoke().await;
    *pending = Some(tokio::spawn(async move {
        let result = provider.prepare(reference.source, reference.key).await;
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
async fn finish_gapless_reset(
    gapless: &mut gapless::Controller,
    queue: &mut queue::Queue,
    epochs: &mut gapless::Epochs,
    before: &player::Status,
    presented: &mut Option<u64>,
    proxy: &proxy::Proxy,
) -> Result<(), String> {
    let promoted = gapless.handoff(queue, epochs, before, proxy).await;
    record_clock(queue, before, epochs.current, presented, false);
    gapless.invalidate();
    proxy.revoke().await;
    epochs.reset().map_err(str::to_owned)?;
    gapless.review();
    promoted.map(|_| ())
}
fn guarded_queue_command(op: &str) -> bool {
    matches!(
        op,
        "play"
            | "album_play"
            | "playlist_play"
            | "queue_play"
            | "replay"
            | "previous"
            | "next"
            | "seek"
            | "stop"
            | "stop_clear"
            | "queue_remove"
            | "clear_upcoming"
            | "queue_move"
            | "enqueue"
            | "play_next"
            | "set_shuffle"
            | "set_repeat"
            | "set_gapless"
            | "disconnect"
            | "forget_signin"
            | "quit"
    )
}
fn changes_current(op: &str, request: &Value, queue: &queue::Queue) -> bool {
    matches!(
        op,
        "play"
            | "album_play"
            | "playlist_play"
            | "queue_play"
            | "replay"
            | "previous"
            | "next"
            | "seek"
            | "stop"
            | "stop_clear"
            | "disconnect"
            | "forget_signin"
            | "quit"
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
    let track = if queue.ended() || queue.selected().is_none() {
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
    let proxy = proxy::Proxy::start().await?;
    let artwork = artwork::Artwork::start().await?;
    let instance = uuid::Uuid::new_v4().to_string();
    let mut revision = 0_u64;

    let mut handled_end = None;
    let mut playlists = playlists::Controller::default();
    let mut albums = albums::Controller::default();
    let mut presented_generation = None;
    let mut state = json!({"protocol":1,"instance":instance,"revision":"0","status":"Connect Plex to browse music","libraries":[],"page":null,"selection":null,"intent":"paused","audio_profile":"Desktop / PipeWire","error":null,"signin_url":null,"sign_in":{"connected":false,"can_save":false,"keyring_action":null,"saved":"unknown","message":"Choose Restore to check for a saved sign-in."}});
    let (tx, mut events) = mpsc::channel(64);
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
    state["queue"] = json!(queue.snapshot());
    state["playlists"] = json!(playlists.view);
    state["albums"] = json!(albums.view);
    state["account_epoch"] = json!("0");
    if queue.snapshot().membership_count > 0 {
        state["status"] = json!("Session restored paused. Restore Plex sign-in to resume.");
    }
    state["selection"] = queue
        .selected()
        .and_then(|id| queue.visible_track(id).ok())
        .map(|track| json!(track))
        .unwrap_or(Value::Null);
    let (updates, rx) = watch::channel(state.clone());
    let vault = vault::start(tx.clone());
    let mut vault_generation = 0_u64;
    let mut current_credential: Option<Credential> = None;
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
    state["gapless"] = json!(gapless.view);
    let mut desired_playing = false;
    let mut audio_tick = tokio::time::interval(std::time::Duration::from_millis(250));
    let mut last_audio = player.status();
    let mut last_stream_failed = false;
    let mut desktop_clock = mpris::Clock::default();
    let mut last_clock_stale = false;
    let mut checkpoint_tick = tokio::time::interval(std::time::Duration::from_secs(5));
    checkpoint_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut desktop = match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        mpris::Service::start(tx.clone()),
    )
    .await
    {
        Ok(Ok(service)) => {
            state["mpris"] =
                json!({"state":"available","message":"Desktop media controls available."});
            Some(service)
        }
        _ => {
            state["mpris"] = json!({"state":"unavailable","message":"Desktop media controls are unavailable. Restart Qoda after the session bus is available or another Qoda instance closes."});
            None
        }
    };
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
            _ = audio_tick.tick() => {
                let audio = player.status();
                let mut gapless_changed=false;
                match gapless.handoff(&mut queue,&mut epochs,&audio,&proxy).await {
                    Ok(true) => { gapless_changed=true; state["selection"]=queue.selected().and_then(|id|queue.visible_track(id).ok()).map(|track|json!(track)).unwrap_or(Value::Null); handled_end=None; },
                    Ok(false) if audio.generation<=epochs.current => {},
                    result => {
                        desired_playing=false;
                        if let Ok(before)=player.barrier().await {let _=finish_gapless_reset(&mut gapless,&mut queue,&mut epochs,&before,&mut presented_generation,&proxy).await;}
                        else {gapless.invalidate();proxy.revoke().await;epochs.reset()?;gapless.review();}
                        state["error"]=json!(result.err().unwrap_or_else(||"An unbound native transition was reset; review the selected occurrence.".into()));
                        state["intent"]=json!("paused");gapless_changed=true;
                    },
                }
                let stream_failed=proxy.failed().await;
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
                        state["error"] = json!(error);
                        desired_playing = false;
                        state["intent"] = json!("paused");
                        player.send(Command::Pause(true));
                    }
                    revision = revision.checked_add(1).ok_or("Revision exhausted")?;
                    state["revision"] = json!(revision.to_string());
                    updates.send_replace(state.clone());
                    if let Some(desktop) = &mut desktop {
                        desktop.publish(mpris::View::capture(&queue,&audio,epochs.current,presented_generation,provider.is_some(),desired_playing,stream_failed || clock_stale));
                    }
                }
                if audio.generation==epochs.current && audio.idle && audio.natural_eof && handled_end!=Some(epochs.current) && audio.error.is_none() && !stream_failed {
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
        let mut event = if let Event::Desktop(request) = event {
            if !request.is_live() {
                let _ = request.reply.send(Err(
                    "The desktop command expired before it was applied.".into(),
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
            );
            if let Some(command) = mpris::translate(&request.action, &view, &queue, &instance) {
                desktop_deadline = Some(request.deadline);
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
                || desktop_reply
                    .as_ref()
                    .is_some_and(|reply| reply.is_closed())
            {
                event = Event::Checkpoint;
                state["error"] = json!("The desktop command expired before queue mutation.");
            } else if matches!(event, Event::Request(_)) {
                let view = mpris::View::capture(
                    &queue,
                    &player.status(),
                    epochs.current,
                    presented_generation,
                    provider.is_some(),
                    desired_playing,
                    last_stream_failed || last_clock_stale,
                );
                event = mpris::translate(action, &view, &queue, &instance)
                    .map(Event::Request)
                    .unwrap_or(Event::Checkpoint);
            }
        }
        let mut to_prepare = None;
        let mut force_save = false;
        let mut seeked = false;
        match event {
            Event::Gapless(reply) => {
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
                }
            }
            Event::DesktopLost => {
                if let Some(desktop) = desktop.take() {
                    desktop.shutdown().await;
                }
                state["mpris"] = json!({"state":"unavailable","message":"Desktop media controls disconnected. Playback controls in Qoda remain available; restart Qoda to reconnect the session bus."});
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
            Event::Checkpoint => {}
            Event::Albums(reply) => {
                if albums.apply(reply, auth_generation)
                    && let (Some(page), Some(provider)) = (&albums.view.page, provider.clone())
                {
                    albums.view.covers = artwork
                        .publish(provider, page.albums.iter().map(|a| a.reference.clone()))
                        .await;
                }
            }
            Event::Playlists(reply) => {
                playlists.apply(reply, auth_generation);
            }
            Event::Advance(g) if g == epochs.current => {
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
                    "retry_save" => force_save = true,
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
                    "browse" => {
                        if let Some(p) = provider.clone()
                            && let Ok(source) =
                                serde_json::from_value::<SourceId>(v["source"].clone())
                        {
                            if albums.source_changed(source) {
                                artwork.revoke(true).await;
                            }
                            let Ok(offset) =
                                serde_json::from_value::<qoda_core::Counter>(v["offset"].clone())
                            else {
                                continue;
                            };
                            let offset = offset.get();
                            let query = v["query"]
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
                            browse_task = Some(tokio::spawn(async move {
                                let r = p.tracks(source, offset, query).await;
                                let _ = tx.send(Event::Page(g, r)).await;
                            }));
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
                    "enqueue" | "play_next" | "queue_play" | "queue_move" | "queue_remove"
                    | "clear_upcoming" | "stop_clear" | "next" | "replay" | "previous"
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
                                || (v["op"] == "next" && to_prepare.is_none())
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
                        playlists.invalidate();
                        albums.invalidate();
                        artwork.revoke(true).await;
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
            Event::Page(g, r) if g == browse_generation => match r {
                Ok(page) => {
                    state["page"] = json!(page);
                    state["status"] = json!("Ready");
                }
                Err(e) => {
                    state["status"] = json!("Browse failed");
                    state["error"] = json!(e.to_string());
                }
            },
            Event::Media(g, r) if g == epochs.current => match r {
                Ok(media) => {
                    if let Err(error) = queue.update_selected(media.track.clone(), &media.version) {
                        state["error"] = json!(error.to_string());
                        desired_playing = false;
                        state["intent"] = json!("paused");
                    } else {
                        state["selection"] = json!(media.track);
                        let url = proxy.grant(media.stream).await;
                        player.send(Command::Load(g, url, queue.position()));
                        player.send(Command::Pause(!desired_playing));
                        state["status"] = json!("Track selected; playback requested");
                    }
                }
                Err(e) => {
                    state["status"] = json!("Playback preparation failed");
                    state["error"] = json!(e.to_string());
                    desired_playing = false;
                    state["intent"] = json!("paused");
                }
            },
            _ => continue,
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
                track.reference,
                tx.clone(),
            )
            .await?;
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
            && let Err(error) = gapless.schedule(&queue, &mut epochs, provider, tx.clone())
        {
            state["error"] = json!(error);
        }
        state["gapless"] = json!(gapless.view);
        if let Some(desktop) = &mut desktop {
            if seeked {
                desktop.mark_seek(&queue, epochs.current);
            }
            desktop.publish(mpris::View::capture(
                &queue,
                &player.status(),
                epochs.current,
                presented_generation,
                provider.is_some(),
                desired_playing,
                last_stream_failed || last_clock_stale,
            ));
        }
        state["queue"] = json!(queue.snapshot());
        state["playlists"] = json!(playlists.view);
        state["albums"] = json!(albums.view);
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
    proxy.revoke().await;
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
