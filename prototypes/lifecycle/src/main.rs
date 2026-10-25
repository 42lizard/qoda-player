// THROWAWAY transport/ownership experiment. Fictional catalog, null audio only.
mod mpv;
mod storage;
use serde_json::{Value, json};
use std::os::fd::AsRawFd;
use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpStream, UnixListener, UnixStream},
    sync::{mpsc, oneshot},
};
#[derive(Clone)]
struct Out {
    sender: mpsc::Sender<Value>,
    close: Arc<tokio::sync::Notify>,
}
impl Out {
    async fn send(&self, v: Value) -> Result<(), mpsc::error::SendError<Value>> {
        self.sender.send(v).await
    }
    fn try_send(&self, v: Value) -> Result<(), mpsc::error::TrySendError<Value>> {
        let r = self.sender.try_send(v);
        if r.is_err() {
            self.close.notify_one();
        }
        r
    }
}
enum Msg {
    Subscribe(Out, Value),
    Command(Value, oneshot::Sender<Value>),
    Mpv(Value),
}
#[derive(Clone)]
struct Mirror {
    playing: bool,
    position: f64,
    loaded: bool,
    volume: f64,
    fidelity_guard: bool,
    generation: u64,
    persistence_error: Option<String>,
}
struct Player {
    tx: mpsc::UnboundedSender<Msg>,
    mirror: Arc<RwLock<Mirror>>,
}
impl Player {
    async fn command(&self, op: &str) {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(Msg::Command(json!({"op":op}), tx));
        let _ = rx.await;
    }
}
#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    async fn play(&self) {
        self.command("play").await;
    }
    async fn pause(&self) {
        self.command("pause").await;
    }
    async fn play_pause(&self) {
        self.command("toggle").await;
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.mirror.read().unwrap().volume
    }
    #[zbus(property)]
    async fn set_volume(&self, value: f64) -> zbus::fdo::Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Msg::Command(json!({"op":"volume","value":value}), tx))
            .map_err(|_| zbus::fdo::Error::Failed("backend unavailable".into()))?;
        let value = rx
            .await
            .map_err(|_| zbus::fdo::Error::Failed("backend unavailable".into()))?;
        if let Some(error) = value["result"]["error"].as_str() {
            Err(zbus::fdo::Error::NotSupported(error.into()))
        } else {
            Ok(())
        }
    }
    #[zbus(property)]
    fn playback_status(&self) -> String {
        if self.mirror.read().unwrap().playing {
            "Playing"
        } else {
            "Paused"
        }
        .into()
    }
    #[zbus(property)]
    fn position(&self) -> i64 {
        (self.mirror.read().unwrap().position * 1e6) as i64
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, zbus::zvariant::OwnedValue> {
        let mut m = HashMap::new();
        m.insert(
            "xesam:title".into(),
            zbus::zvariant::Str::from("Synthetic null-output fixture").into(),
        );
        m.insert(
            "mpris:trackid".into(),
            zbus::zvariant::ObjectPath::try_from("/org/qoda/Prototype/track/1")
                .unwrap()
                .into(),
        );
        m
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
}
struct Root;
fn lifecycle(action: &str) {
    let root = std::env::var("QODA_STUDY_HOME").unwrap();
    let launcher = std::env::var("QODA_STUDY_LAUNCHER").unwrap();
    let unit = format!(
        "qoda-study-control-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let _ = std::process::Command::new("systemd-run")
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            std::env::var("QODA_MANAGER_BUS").unwrap(),
        )
        .args([
            "--user",
            "--collect",
            "--quiet",
            "--unit",
            &unit,
            "/usr/bin/python3",
            &launcher,
            "--root",
            &root,
            action,
        ])
        .status();
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    async fn raise(&self) {
        lifecycle("show");
    }
    async fn quit(&self) {
        lifecycle("quit");
    }
    #[zbus(property)]
    fn identity(&self) -> &str {
        "Qoda Player"
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "qoda-player"
    }
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec![]
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        vec![]
    }
}
fn state(instance: &str, rev: u64, queue: &[Value], m: &Arc<RwLock<Mirror>>) -> Value {
    let m = m.read().unwrap();
    json!({"instance":instance,"revision":rev,"queue":queue,"playing":m.playing,"loaded":m.loaded,"position":m.position,"volume":m.volume,"fidelityGuard":m.fidelity_guard,"generation":m.generation,"persistenceError":m.persistence_error,"release":std::env::var("QODA_STUDY_RELEASE").unwrap_or_default()})
}
async fn client(
    socket: UnixStream,
    tx: mpsc::UnboundedSender<Msg>,
    catalog: Arc<Vec<Value>>,
    cache: PathBuf,
    port: u16,
) {
    let (read, mut write) = socket.into_split();
    let (sender, mut receive) = mpsc::channel::<Value>(128);
    let close = Arc::new(tokio::sync::Notify::new());
    let out = Out {
        sender,
        close: close.clone(),
    };
    let writer = tokio::spawn(async move {
        loop {
            let v = tokio::select! { _=close.notified()=>break,v=receive.recv()=>match v{Some(v)=>v,None=>break} };
            let mut s = v.to_string();
            s.push('\n');
            if write.write_all(s.as_bytes()).await.is_err() {
                break;
            }
            if v["error"] == "protocol-mismatch" {
                break;
            }
        }
    });
    let mut read = BufReader::new(read);
    let mut line = Vec::new();
    let generation = Arc::new(AtomicU64::new(0));
    let mut handshaken = false;
    loop {
        line.clear();
        let mut limited = (&mut read).take(1_048_577);
        let count = match limited.read_until(b'\n', &mut line).await {
            Ok(n) => n,
            Err(_) => break,
        };
        if count == 0 {
            break;
        }
        if count > 1_048_576 {
            break;
        }
        let req: Value = match serde_json::from_slice(&line) {
            Ok(v) => v,
            Err(_) => {
                let _ = out
                    .send(json!({"type":"error","error":"invalid-json"}))
                    .await;
                continue;
            }
        };
        let op = req["op"].as_str().unwrap_or("");
        if op == "hello" {
            if req["protocol"] != 1 {
                let _ = out
                    .send(json!({"type":"error","error":"protocol-mismatch"}))
                    .await;
                let _ = writer.await;
                return;
            }
            handshaken = true;
            let _ = tx.send(Msg::Subscribe(out.clone(), req));
            continue;
        }
        if !handshaken {
            let _ = out
                .send(json!({"type":"error","error":"hello-required"}))
                .await;
            continue;
        }
        match op {
            "search" => {
                let g = generation.fetch_add(1, Ordering::SeqCst) + 1;
                let gen_ref = generation.clone();
                let out = out.clone();
                let cat = catalog.clone();
                tokio::spawn(async move {
                    let query = req["query"].as_str().unwrap_or("").to_lowercase();
                    let limit = req["limit"].as_u64().unwrap_or(100).min(200) as usize;
                    let offset = req["offset"].as_u64().unwrap_or(0) as usize;
                    tokio::time::sleep(Duration::from_millis(
                        req["delay_ms"].as_u64().unwrap_or(0).min(1000),
                    ))
                    .await;
                    let (rows, total, scanned, cancelled) =
                        tokio::task::spawn_blocking(move || {
                            let mut rows = Vec::new();
                            let mut total = 0;
                            let mut scanned = 0;
                            for (i, r) in cat.iter().enumerate() {
                                if i % 256 == 0 && gen_ref.load(Ordering::SeqCst) != g {
                                    return (rows, total, scanned, true);
                                }
                                scanned += 1;
                                if r["title"].as_str().unwrap().to_lowercase().contains(&query) {
                                    if total >= offset && rows.len() < limit {
                                        rows.push(r.clone());
                                    }
                                    total += 1;
                                }
                            }
                            (rows, total, scanned, false)
                        })
                        .await
                        .unwrap();
                    let _=out.send(json!({"type":if cancelled{"search-cancelled"}else{"page"},"id":req["id"],"rows":rows,"total":total,"scanned":scanned})).await;
                });
            }
            "artwork" => {
                let out = out.clone();
                let cache = cache.clone();
                tokio::spawn(async move {
                    let version = req["version"].as_u64().unwrap_or(1).clamp(1, 2);
                    let mut http = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                    // Fixed loopback provider, fictional token. Neither token nor provider URL is returned.
                    http.write_all(format!("GET /art/{version} HTTP/1.0\r\nHost: localhost\r\nAuthorization: Bearer fictional-prototype-only\r\n\r\n").as_bytes()).await.unwrap();
                    let mut data = Vec::new();
                    http.read_to_end(&mut data).await.unwrap();
                    let split = data.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                    assert!(data.starts_with(b"HTTP/1.0 200"));
                    let path = cache.join(format!("art-{version}.png"));
                    std::fs::write(&path, &data[split..]).unwrap();
                    let _=out.send(json!({"type":"artwork","id":req["id"],"url":format!("file://{}",path.display()),"version":version})).await;
                });
            }
            "disconnect" => break,
            _ => {
                let (reply, rx) = oneshot::channel();
                let _ = tx.send(Msg::Command(req.clone(), reply));
                if let Ok(v) = rx.await {
                    if req["drop_reply"] != true {
                        let _ = out.send(v).await;
                    }
                }
            }
        }
    }
    generation.fetch_add(1, Ordering::SeqCst);
    drop(out);
    writer.abort();
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(std::env::var("QODA_PROTO_RUNTIME")?);
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("backend.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        std::process::exit(73);
    }
    let dbpath = PathBuf::from(std::env::var("QODA_STUDY_DB")?);
    let store = match storage::Store::open(&dbpath) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fatal:{e}");
            std::process::exit(78)
        }
    };
    let restored = match store.restore() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("fatal:{e}");
            std::process::exit(78)
        }
    };
    if let Ok(ms) = std::env::var("QODA_STUDY_DELAY") {
        tokio::time::sleep(Duration::from_millis(ms.parse().unwrap_or(0))).await;
    }
    let socket = dir.join("backend.sock");
    if socket.exists() {
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    let count = std::env::var("QODA_PROTO_ROWS")
        .unwrap_or("100000".into())
        .parse::<usize>()?;
    let catalog=Arc::new((0..count).map(|i|json!({"trackId":format!("fixture:track:{i}"),"title":format!("Track {i:06}"),"artist":format!("Artist {:04}",i%2000),"album":format!("Album {:05}",i/10)})).collect::<Vec<_>>());
    let instance = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let mirror = Arc::new(RwLock::new(Mirror {
        playing: false,
        position: 0.0,
        loaded: false,
        volume: 1.0,
        fidelity_guard: false,
        generation: 0,
        persistence_error: None,
    }));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (audio, thread) = mpv::start(
        dir.join("fixture.wav").to_string_lossy().into_owned(),
        tx.clone(),
    );
    let bus = zbus::connection::Builder::session()?
        .name("org.mpris.MediaPlayer2.qoda")?
        .serve_at("/org/mpris/MediaPlayer2", Root)?
        .serve_at(
            "/org/mpris/MediaPlayer2",
            Player {
                tx: tx.clone(),
                mirror: mirror.clone(),
            },
        )?
        .build()
        .await?;
    let accept_tx = tx.clone();
    let cache = dir.clone();
    let port = 1;
    tokio::spawn(async move {
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                if stream.peer_cred().map(|c| c.uid()).ok() != Some(unsafe { libc::geteuid() }) {
                    continue;
                }
                tokio::spawn(client(
                    stream,
                    accept_tx.clone(),
                    catalog.clone(),
                    cache.clone(),
                    port,
                ));
            }
        }
    });
    println!("{}", json!({"ready":true,"instance":instance,"rows":count}));
    let mut subscribers: Vec<Out> = Vec::new();
    let mut queue = restored["queue"].as_array().cloned().unwrap_or_default();
    let restored_position = restored["position"].as_f64().unwrap_or(0.0);
    let mut restore_seek = Some(restored_position);
    let mut checkpoint_tick = tokio::time::interval(Duration::from_secs(5));
    let mut rev = 0u64;
    let mut seen = HashMap::<String, Value>::new();
    loop {
        let msg = tokio::select! {
            m=rx.recv()=>match m{Some(m)=>m,None=>break},
            _=checkpoint_tick.tick()=>{
                let pos=mirror.read().unwrap().position;
                let error=store.checkpoint(&queue,pos).err();
                mirror.write().unwrap().persistence_error=error;
                continue;
            }
        };
        let mut changed = false;
        match msg {
            Msg::Subscribe(out, req) => {
                // Snapshot enqueue and subscription registration are a single actor turn.
                let _=out.try_send(json!({"type":"snapshot","id":req["id"],"protocol":1,"state":state(&instance,rev,&queue,&mirror)}));
                subscribers.push(out);
            }
            Msg::Command(req, reply) => {
                let op = req["op"].as_str().unwrap_or("");
                let key = req["operationId"].as_str().map(str::to_owned);
                let result = if req.get("instance").is_some() && req["instance"] != instance {
                    json!({"error":"stale-instance"})
                } else if let Some(old) = key.as_ref().and_then(|k| seen.get(k)) {
                    old.clone()
                } else if req.get("expectedRevision").is_some() && req["expectedRevision"] != rev {
                    json!({"error":"revision-conflict"})
                } else {
                    let result = match op {
                        "append" => {
                            queue.push(json!({"entryId":format!("entry-{}",rev+1),"trackId":req["trackId"]}));
                            changed = true;
                            json!({"accepted":true})
                        }
                        "checkpoint" => {
                            let result = store.checkpoint(&queue, mirror.read().unwrap().position);
                            mirror.write().unwrap().persistence_error = result.clone().err();
                            match result {
                                Ok(()) => json!({"accepted":true}),
                                Err(e) => json!({"error":e}),
                            }
                        }
                        "stop-clear" => {
                            let _ = audio.send(vec!["set".into(), "pause".into(), "yes".into()]);
                            queue.clear();
                            mirror.write().unwrap().generation += 1;
                            changed = true;
                            json!({"accepted":true})
                        }
                        "mode-guard" => {
                            let _ = audio.send(vec!["set".into(), "pause".into(), "yes".into()]);
                            let _ = audio.send(vec!["set".into(), "volume".into(), "100".into()]);
                            let mut m = mirror.write().unwrap();
                            m.fidelity_guard = req["enabled"] == true;
                            m.generation += 1;
                            changed = true;
                            json!({"accepted":true})
                        }
                        "completion" => {
                            if req["generation"].as_u64() != Some(mirror.read().unwrap().generation)
                            {
                                json!({"error":"stale-completion"})
                            } else {
                                json!({"accepted":true})
                            }
                        }
                        "volume" => {
                            let value = req["value"].as_f64().unwrap_or(f64::NAN);
                            if !value.is_finite()
                                || value < 0.0
                                || (mirror.read().unwrap().fidelity_guard && value != 1.0)
                            {
                                json!({"error":"volume-rejected"})
                            } else {
                                let _ = audio.send(vec![
                                    "set".into(),
                                    "volume".into(),
                                    (value * 100.0).to_string(),
                                ]);
                                json!({"accepted":true})
                            }
                        }
                        "play" | "pause" | "toggle" => {
                            let paused = match op {
                                "play" => false,
                                "pause" => true,
                                _ => mirror.read().unwrap().playing,
                            };
                            if paused {
                                mirror.write().unwrap().generation += 1;
                            }
                            let _ = audio.send(vec![
                                "set".into(),
                                "pause".into(),
                                if paused { "yes" } else { "no" }.into(),
                            ]);
                            json!({"accepted":true})
                        }
                        "fault-gap" => {
                            rev += 1;
                            changed = true;
                            json!({"accepted":true})
                        }
                        "snapshot" => json!({"state":state(&instance,rev,&queue,&mirror)}),
                        "shutdown" => {
                            let _ = audio.send(vec!["set".into(), "pause".into(), "yes".into()]);
                            let result = store.checkpoint(&queue, mirror.read().unwrap().position);
                            if let Err(e) = result {
                                mirror.write().unwrap().persistence_error = Some(e.clone());
                                let _ = reply.send(
                                    json!({"type":"ack","id":req["id"],"result":{"error":e}}),
                                );
                                continue;
                            }
                            let _ =
                                reply.send(json!({"type":"ack","id":req["id"],"accepted":true}));
                            break;
                        }
                        _ => json!({"error":"unsupported-command"}),
                    };
                    if let Some(k) = key {
                        if seen.len() >= 256 {
                            seen.clear();
                        }
                        seen.insert(k, result.clone());
                    }
                    result
                };
                if changed {
                    let error = store
                        .checkpoint(&queue, mirror.read().unwrap().position)
                        .err();
                    mirror.write().unwrap().persistence_error = error;
                    rev += 1;
                }
                let _=reply.send(json!({"type":"ack","id":req["id"],"result":result,"instance":instance,"revision":rev}));
            }
            Msg::Mpv(v) => {
                let mut m = mirror.write().unwrap();
                if let Some(p) = v["paused"].as_bool() {
                    m.playing = !p;
                }
                if let Some(p) = v["position"].as_f64() {
                    m.position = p;
                }
                if let Some(volume) = v["volume"].as_f64() {
                    m.volume = volume / 100.0;
                }
                if v["loaded"] == true {
                    m.loaded = true;
                    if let Some(position) = restore_seek.take() {
                        if position > 0.0 {
                            let _ = audio.send(vec![
                                "seek".into(),
                                position.to_string(),
                                "absolute+exact".into(),
                            ]);
                        }
                    }
                }
                drop(m);
                rev += 1;
                changed = true;
                let mut props = HashMap::<&str, zbus::zvariant::Value<'_>>::new();
                props.insert(
                    "PlaybackStatus",
                    zbus::zvariant::Value::from(if mirror.read().unwrap().playing {
                        "Playing"
                    } else {
                        "Paused"
                    }),
                );
                bus.emit_signal(
                    None::<&str>,
                    "/org/mpris/MediaPlayer2",
                    "org.freedesktop.DBus.Properties",
                    "PropertiesChanged",
                    &("org.mpris.MediaPlayer2.Player", props, Vec::<&str>::new()),
                )
                .await?;
            }
        }
        if changed {
            let update = json!({"type":"update","state":state(&instance,rev,&queue,&mirror)});
            subscribers.retain(|s| s.try_send(update.clone()).is_ok());
        }
    }
    let _ = audio.send(vec!["destroy".into()]);
    thread.join().unwrap();
    std::fs::remove_file(socket)?;
    Ok(())
}
