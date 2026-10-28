//! Durable explicit original-file download jobs. No playback ownership.
use crate::{Event, cache_files};
use futures_util::StreamExt;
use qoda_core::TrackRef;
use qoda_provider::{MusicProvider, Track, local::Root};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
mod import;
use import::Destination;
const MAX_JOBS: usize = 200;
const MAX_IMPORTS: usize = 2000;
const MAX_FILE: u64 = 8 * 1024 * 1024 * 1024;
const BUDGET: u64 = 100 * 1024 * 1024 * 1024;
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn import_key(reference: &TrackRef, version: &str) -> String {
    hash(&serde_json::to_vec(&(reference, version)).expect("serializing track identity"))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportedCopy {
    track: Track,
    origin: TrackRef,
    version: String,
    key: String,
    root: Root,
    filename: String,
    checksum: String,
    bytes: u64,
    device: u64,
    inode: u64,
    fingerprint: String,
    reference: TrackRef,
}
impl ImportedCopy {
    fn valid(&self) -> bool {
        let digest =
            |value: &str| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
        import::valid_name(&self.filename)
            && self.bytes > 0
            && self.bytes <= MAX_FILE
            && digest(&self.checksum)
            && !self.version.is_empty()
            && self.version.len() <= 256
            && self.key == import_key(&self.origin, &self.version)
            && (self.fingerprint.is_empty() || digest(&self.fingerprint))
            && self.track.reference == self.reference
            && self.reference.source == self.root.id
            && String::from(self.reference.key.clone())
                == hash(
                    Path::new(import::MANAGED)
                        .join(&self.filename)
                        .as_os_str()
                        .as_encoded_bytes(),
                )
            && self.track.title.len() <= 4096
            && self.track.artist.len() <= 4096
            && self.track.album.len() <= 4096
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    id: String,
    track: Track,
    root: Root,
    phase: Phase,
    bytes: u64,
    total: Option<u64>,
    version: Option<String>,
    copy: Option<ImportedCopy>,
    message: String,
}
impl Job {
    fn new(track: Track, root: Root) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            track,
            root,
            phase: Phase::Queued,
            bytes: 0,
            total: None,
            version: None,
            copy: None,
            message: String::new(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    version: u8,
    destination: Option<Root>,
    destination_revision: u64,
    jobs: Vec<Job>,
    imports: Vec<ImportedCopy>,
}
impl Default for Store {
    fn default() -> Self {
        Self {
            version: 1,
            destination: None,
            destination_revision: 0,
            jobs: vec![],
            imports: vec![],
        }
    }
}
impl Store {
    fn import_records(&self) -> Vec<ImportedCopy> {
        let mut copies = self.imports.clone();
        for copy in self.jobs.iter().filter_map(|job| job.copy.as_ref()) {
            if !copies.iter().any(|c| {
                c.root.id == copy.root.id && c.filename == copy.filename && c.inode == copy.inode
            }) {
                copies.push(copy.clone());
            }
        }
        copies
    }
    fn load(directory: &cache_files::Directory) -> Result<Self, String> {
        let Some(file) = directory.file("state.json").map_err(str::to_owned)? else {
            return Ok(Self::default());
        };
        if file
            .metadata()
            .map_err(|_| "Download history could not be read.")?
            .len()
            > 8 * 1024 * 1024
        {
            return Err("Download history is oversized; existing data was preserved.".into());
        }
        let store: Self = serde_json::from_reader(file.take(8 * 1024 * 1024 + 1))
            .map_err(|_| "Download history is invalid; existing data was preserved.")?;
        let valid_root =
            |root: &Root| root.path.as_os_str().len() <= 4096 && root.has_valid_identity();
        if store.version != 1
            || store.jobs.len() > MAX_JOBS
            || store
                .jobs
                .iter()
                .map(|j| &j.id)
                .collect::<std::collections::HashSet<_>>()
                .len()
                != store.jobs.len()
            || store.imports.len() > MAX_IMPORTS
            || store.destination.as_ref().is_some_and(|r| !valid_root(r))
            || store.jobs.iter().any(|j| {
                uuid::Uuid::parse_str(&j.id).is_err()
                    || !valid_root(&j.root)
                    || j.track.title.len() > 4096
                    || j.track.artist.len() > 4096
                    || j.track.album.len() > 4096
                    || j.message.len() > 4096
                    || j.bytes > MAX_FILE
                    || j.version.as_ref().is_some_and(|v| v.len() > 256)
            })
            || store
                .imports
                .iter()
                .chain(store.jobs.iter().filter_map(|j| j.copy.as_ref()))
                .any(|c| !valid_root(&c.root) || !c.valid())
        {
            return Err(
                "Unsupported download history was preserved; use a compatible Qoda version.".into(),
            );
        }
        Ok(store)
    }
    fn save(&self, directory: &cache_files::Directory) -> Result<(), String> {
        directory.resolved_path().map_err(str::to_owned)?;
        let _preserved = Self::load(directory)?;
        let bytes = serde_json::to_vec(self).map_err(|_| "Download history could not be saved.")?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Download history limit reached; clear finished jobs.".into());
        }
        directory
            .atomic("state.json", &bytes)
            .map_err(str::to_owned)
    }
}
fn reconcile(job: &mut Job, alive: &AtomicBool) -> Option<ImportedCopy> {
    if job.phase == Phase::Completed {
        return None;
    }
    let copy = job.copy.as_ref()?;
    let copy = Destination::existing(copy.root.clone())
        .and_then(|d| d.verify_while(copy, true, || alive.load(Ordering::Acquire)))
        .ok()?;
    job.phase = Phase::Completed;
    job.bytes = copy.bytes;
    job.total = Some(copy.bytes);
    job.copy = Some(copy.clone());
    job.message = "Downloaded original; the local copy is available without Plex.".into();
    Some(copy)
}
struct Context {
    generation: u64,
    account: u64,
    provider: Option<Arc<dyn MusicProvider>>,
    roots: Vec<Root>,
}
enum Command {
    Shutdown(oneshot::Sender<()>),
    Context(Context),
    Request { request: Value, tracks: Vec<Track> },
}
enum Work {
    Pin {
        id: String,
        version: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Progress {
        id: String,
        bytes: u64,
        total: Option<u64>,
    },
    Publish {
        id: String,
        copy: ImportedCopy,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Done {
        id: String,
        result: Result<ImportedCopy, String>,
    },
}
struct Owner(std::fs::File);
impl Owner {
    fn open(directory: &cache_files::Directory) -> Result<Self, String> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(directory.path().join("owner.lock"))
            .map_err(|_| "Download ownership lock is unavailable.")?;
        if !cache_files::private_file(
            &file
                .metadata()
                .map_err(|_| "Download ownership lock is unavailable.")?,
        ) {
            return Err("Download ownership lock is unsafe.".into());
        }
        file.try_lock()
            .map_err(|_| "Downloads are in use by another backend.")?;
        Ok(Self(file))
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
/// Bounded commands cross the coordinator boundary. The gate serializes an
/// account/destination change with the single atomic publication syscall.
pub struct Controller {
    alive: Arc<AtomicBool>,
    commands: mpsc::Sender<Command>,
    gate: Arc<Mutex<u64>>,
    key: Option<(u64, usize, String)>,
    task: JoinHandle<()>,
}
impl Controller {
    pub fn start(path: Result<PathBuf, &'static str>, events: mpsc::Sender<Event>) -> Self {
        let (commands, rx) = mpsc::channel(32);
        let gate = Arc::new(Mutex::new(0));
        let worker_gate = gate.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        let task =
            tokio::spawn(
                async move { Actor::run(path, rx, worker_gate, worker_alive, events).await },
            );
        Self {
            alive,
            commands,
            gate,
            key: None,
            task,
        }
    }
    pub fn sync(
        &mut self,
        account: u64,
        provider: Option<Arc<dyn MusicProvider>>,
        roots: Vec<Root>,
    ) {
        let identity = provider
            .as_ref()
            .map_or(0, |p| Arc::as_ptr(p) as *const () as usize);
        let key = (
            account,
            identity,
            serde_json::to_string(&roots).unwrap_or_default(),
        );
        if self.key.as_ref() == Some(&key) {
            return;
        }
        let generation = {
            let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            *gate = gate.saturating_add(1);
            *gate
        };
        // Retry a full mailbox on the next coordinator sample; the incremented
        // gate already prevents publication using the obsolete context.
        if self
            .commands
            .try_send(Command::Context(Context {
                generation,
                account,
                provider,
                roots,
            }))
            .is_ok()
        {
            self.key = Some(key)
        }
    }
    pub fn invalidate(&mut self) {
        let generation = {
            let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            *gate = gate.saturating_add(1);
            *gate
        };
        self.key = None;
        let _ = self.commands.try_send(Command::Context(Context {
            generation,
            account: 0,
            provider: None,
            roots: vec![],
        }));
    }
    pub async fn shutdown(&mut self) {
        if self.task.is_finished() {
            return;
        }
        self.invalidate();
        self.alive.store(false, Ordering::Release);
        let (reply, ack) = oneshot::channel();
        if self.commands.try_send(Command::Shutdown(reply)).is_ok() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), ack).await;
        }
        self.task.abort();
        let _ = (&mut self.task).await;
    }
    pub fn request(&self, request: Value, tracks: Vec<Track>) -> Result<(), &'static str> {
        self.commands
            .try_send(Command::Request { request, tracks })
            .map_err(|_| "Downloads are busy; retry shortly.")
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        *gate = gate.saturating_add(1);
        self.task.abort();
    }
}
struct Active {
    id: String,
    generation: u64,
    task: JoinHandle<()>,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct Actor {
    _owner: Owner,
    directory: cache_files::Directory,
    store: Store,
    context: Context,
    gate: Arc<Mutex<u64>>,
    active: Option<Active>,
    events: mpsc::Sender<Event>,
    work: mpsc::Sender<(u64, Work)>,
    problem: String,
    writable: bool,
    dirty: bool,
    checkpoint: std::time::Instant,
}
impl Actor {
    async fn run(
        path: Result<PathBuf, &'static str>,
        mut commands: mpsc::Receiver<Command>,
        gate: Arc<Mutex<u64>>,
        alive: Arc<AtomicBool>,
        events: mpsc::Sender<Event>,
    ) {
        let opened = path
            .map_err(str::to_owned)
            .and_then(|p| cache_files::Directory::open(&p).map_err(str::to_owned))
            .and_then(|d| {
                Owner::open(&d).and_then(|owner| Store::load(&d).map(|store| (d, owner, store)))
            });
        let (directory, owner, mut store) = match opened {
            Ok(value) => value,
            Err(error) => {
                let _ = events
                    .send(Event::Downloads(
                        json!({"available":false,"jobs":[],"message":error}),
                        false,
                    ))
                    .await;
                return;
            }
        };
        // Recover only a journaled publication whose inode and checksum prove it
        // is ours. Never adopt an arbitrary pre-existing destination file.
        for job in &mut store.jobs {
            if matches!(job.phase, Phase::Running | Phase::Queued) {
                job.phase = Phase::Paused;
                job.message = "Continue restarts the original download from zero.".into();
            }
            if let Some(copy) = reconcile(job, &alive)
                && !store.imports.iter().any(|c| {
                    c.root.id == copy.root.id
                        && c.filename == copy.filename
                        && c.inode == copy.inode
                })
            {
                store.imports.push(copy);
            }
            if let Ok(destination) = Destination::existing(job.root.clone()) {
                let _ = destination.cleanup(&job.id);
            }
        }

        let (work, mut replies) = mpsc::channel(32);
        let mut this = Self {
            _owner: owner,
            directory,
            store,
            context: Context {
                generation: 0,
                account: 0,
                provider: None,
                roots: vec![],
            },
            gate,
            active: None,
            events,
            work,
            problem: String::new(),
            writable: true,
            dirty: false,
            checkpoint: std::time::Instant::now(),
        };
        this.save();
        this.publish(false).await;
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tokio::select! {
                command=commands.recv()=>{let Some(command)=command else{break};match command{
                    Command::Shutdown(reply)=>{
                        this.cancel_active("Download paused because Qoda stopped.");
                        for job in &mut this.store.jobs{if job.phase==Phase::Queued{job.phase=Phase::Paused;}}
                        this.save();let _=reply.send(());break;
                    }
                    Command::Context(context)=>{
                        this.cancel_active("Download paused because its account or destination changed.");
                        for job in &mut this.store.jobs {if job.phase==Phase::Queued{job.phase=Phase::Paused;}}
                        this.context=context;
                        this.save();
                    }
                    Command::Request{request,tracks}=>{if this.writable {
                        match this.command(&request,tracks){Ok(())=>{this.save();},Err(error)=>this.problem=error}
                    }}
                }this.schedule();this.publish(false).await;}
                reply=replies.recv()=>{if let Some(reply)=reply{let published=this.apply(reply);this.schedule();this.publish(published).await;}}
                _=tick.tick()=>{if this.dirty && this.checkpoint.elapsed()>=std::time::Duration::from_secs(5){this.save();}this.publish(false).await;}
            }
        }
    }
    fn save(&mut self) -> bool {
        match self.store.save(&self.directory) {
            Ok(()) => {
                if !self.writable {
                    self.problem.clear();
                }
                self.writable = true;
                self.dirty = false;
                self.checkpoint = std::time::Instant::now();
                true
            }
            Err(error) => {
                self.writable = false;
                self.dirty = true;
                self.problem = error;
                false
            }
        }
    }
    fn advance_gate(&mut self) {
        let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        *gate = gate.saturating_add(1);
        self.context.generation = *gate;
    }
    fn cancel_active(&mut self, message: &str) {
        if let Some(active) = self.active.take()
            && let Some(job) = self.store.jobs.iter_mut().find(|j| j.id == active.id)
        {
            job.phase = Phase::Paused;
            job.message = message.into();
            if let Ok(destination) = Destination::existing(job.root.clone()) {
                let _ = destination.cleanup(&job.id);
            }
        }
    }
    fn command(&mut self, v: &Value, tracks: Vec<Track>) -> Result<(), String> {
        if self.context.generation == u64::MAX {
            return Err("Download generation exhausted; restart Qoda.".into());
        }
        if self.context.generation != *self.gate.lock().unwrap_or_else(|e| e.into_inner())
            || v["account_epoch"].as_str() != Some(self.context.account.to_string().as_str())
            || v["downloads_revision"].as_str()
                != Some(self.store.destination_revision.to_string().as_str())
        {
            return Err(
                "The account or download destination changed. Review Downloads and retry.".into(),
            );
        }
        self.problem.clear();
        match v["op"].as_str().unwrap_or_default() {
            "download_destination" => {
                let revision = self
                    .store
                    .destination_revision
                    .checked_add(1)
                    .ok_or("Download destination revision exhausted.")?;
                let source: qoda_core::SourceId = serde_json::from_value(v["source"].clone())
                    .map_err(|_| "Choose a watched local folder first.")?;
                let root = self
                    .context
                    .roots
                    .iter()
                    .find(|r| r.id == source)
                    .cloned()
                    .ok_or("Choose a watched local folder first.")?;
                Destination::open(root.clone())?;
                self.advance_gate();
                self.cancel_active("Download paused because its destination changed.");
                for job in &mut self.store.jobs {
                    if job.phase == Phase::Queued {
                        job.phase = Phase::Paused;
                    }
                }
                self.store.destination = Some(root);
                self.store.destination_revision = revision;
            }
            "download_start" => {
                let root = self
                    .store
                    .destination
                    .clone()
                    .filter(|r| self.context.roots.iter().any(|p| p.id == r.id))
                    .ok_or("Choose a watched download destination first.")?;
                if tracks.is_empty()
                    || tracks.len() > 100
                    || self.store.jobs.len() + tracks.len() > MAX_JOBS
                {
                    return Err("Choose 1 to 100 tracks and keep at most 200 download jobs.".into());
                }
                let provider = self
                    .context
                    .provider
                    .as_ref()
                    .ok_or("Connect Plex before downloading.")?;
                if tracks
                    .iter()
                    .any(|t| !provider.downloads(t.reference.source))
                {
                    return Err("Original downloads are unavailable for this source.".into());
                }
                for track in tracks {
                    if !self.store.jobs.iter().any(|j| {
                        j.track.reference == track.reference
                            && matches!(j.phase, Phase::Queued | Phase::Running)
                    }) {
                        self.store.jobs.push(Job::new(track, root.clone()));
                    }
                }
            }
            "download_retry" | "download_cancel" => {
                let id = v["job_id"].as_str().ok_or("Choose a download job.")?;
                let active = self.active.as_ref().is_some_and(|a| a.id == id);
                if active {
                    self.advance_gate();
                    self.cancel_active("Download cancelled.");
                }
                let job = self
                    .store
                    .jobs
                    .iter_mut()
                    .find(|j| j.id == id)
                    .ok_or("This download job is no longer available.")?;
                if job.phase == Phase::Completed {
                    return Err("Completed downloads are kept as local music.".into());
                }
                if v["op"] == "download_cancel" {
                    job.phase = Phase::Cancelled;
                    job.message = "Download cancelled; completed music was not deleted.".into();
                } else {
                    if self
                        .store
                        .destination
                        .as_ref()
                        .is_none_or(|r| r.id != job.root.id)
                        || !self.context.roots.iter().any(|r| r.id == job.root.id)
                    {
                        return Err(
                            "Restore this job's watched destination before continuing.".into()
                        );
                    }
                    if job.copy.is_none()
                        && self
                            .context
                            .provider
                            .as_ref()
                            .is_none_or(|p| !p.downloads(job.track.reference.source))
                    {
                        return Err("Reconnect the original Plex source before continuing.".into());
                    }
                    job.phase = Phase::Queued;
                    job.bytes = 0;
                    job.message = "Continue restarts the original download from zero.".into();
                }
            }
            "download_clear" => {
                self.store.jobs.retain(|j| {
                    matches!(j.phase, Phase::Running | Phase::Queued | Phase::Paused)
                        || j.copy.is_some() && j.phase != Phase::Completed
                });
                if self.store.jobs.iter().any(|j| {
                    j.copy.is_some() && matches!(j.phase, Phase::Failed | Phase::Cancelled)
                }) {
                    self.problem = "Unresolved publications were kept. Retry them to verify the local copy before clearing history.".into();
                }
            }
            _ => return Err("Unknown download action.".into()),
        }
        Ok(())
    }
    fn schedule(&mut self) {
        if !self.writable
            || self.context.generation == u64::MAX
            || self.context.generation != *self.gate.lock().unwrap_or_else(|e| e.into_inner())
            || self.active.is_some()
        {
            return;
        }
        let Some(job) = self
            .store
            .jobs
            .iter_mut()
            .find(|j| j.phase == Phase::Queued)
        else {
            return;
        };
        let provider = self.context.provider.clone();
        if provider.is_none() && job.copy.is_none() {
            return;
        }
        job.phase = Phase::Running;
        let job = job.clone();
        if !self.save() {
            if let Some(job) = self.store.jobs.iter_mut().find(|j| j.id == job.id) {
                job.phase = Phase::Paused;
                job.message = self.problem.clone();
            }
            return;
        }
        let gate = self.gate.clone();
        let generation = self.context.generation;
        let work = self.work.clone();
        let imports = self.store.import_records();
        let remaining = BUDGET
            .saturating_sub(imports.iter().map(|c| c.bytes).sum::<u64>())
            .min(MAX_FILE);
        let id = job.id.clone();
        let task = tokio::spawn(async move {
            let result = transfer(
                job.clone(),
                provider,
                gate,
                generation,
                work.clone(),
                imports,
                remaining,
            )
            .await;
            let _ = work
                .send((generation, Work::Done { id: job.id, result }))
                .await;
        });
        self.active = Some(Active {
            id,
            generation,
            task,
        });
    }
    fn apply(&mut self, (generation, work): (u64, Work)) -> bool {
        let id = match &work {
            Work::Pin { id, .. }
            | Work::Progress { id, .. }
            | Work::Publish { id, .. }
            | Work::Done { id, .. } => id,
        };
        if self
            .active
            .as_ref()
            .is_none_or(|a| a.id != *id || a.generation != generation)
        {
            return false;
        }
        let Some(job) = self.store.jobs.iter_mut().find(|j| j.id == *id) else {
            return false;
        };
        let mut published = false;
        match work {
            Work::Pin { version, reply, .. } => {
                job.version = Some(version);
                let result = if self.save() {
                    Ok(())
                } else {
                    Err(self.problem.clone())
                };
                let _ = reply.send(result);
                return false;
            }
            Work::Progress { bytes, total, .. } => {
                job.bytes = bytes;
                job.total = total;
                self.dirty = true;
                if self.checkpoint.elapsed() < std::time::Duration::from_secs(5) {
                    return false;
                }
            }
            Work::Publish { copy, reply, .. } => {
                job.copy = Some(copy);
                let result = if self.save() {
                    Ok(())
                } else {
                    Err(self.problem.clone())
                };
                let _ = reply.send(result);
                return false;
            }
            Work::Done { result, .. } => {
                match result {
                    Ok(copy) => {
                        job.phase = Phase::Completed;
                        job.bytes = copy.bytes;
                        job.total = Some(copy.bytes);
                        job.copy = Some(copy.clone());
                        job.message =
                            "Downloaded original; the local copy is available without Plex.".into();
                        if !self.store.imports.iter().any(|c| {
                            c.root.id == copy.root.id
                                && c.filename == copy.filename
                                && c.inode == copy.inode
                        }) {
                            self.store.imports.push(copy);
                        }
                        published = true;
                    }
                    Err(error) => {
                        job.phase = Phase::Failed;
                        job.message = error;
                    }
                }
                self.active = None;
            }
        }
        self.save();
        published
    }
    async fn publish(&self, published: bool) {
        let watched = |copy: &ImportedCopy| {
            self.context
                .roots
                .iter()
                .any(|root| root.id == copy.root.id)
        };
        let jobs:Vec<_>=self.store.jobs.iter().rev().map(|job| {
            let copy=job.copy.as_ref().filter(|_|job.phase==Phase::Completed);
            let checked=copy.and_then(|copy|Destination::existing(copy.root.clone()).and_then(|destination|destination.verify(copy,false)).ok());
            let local=checked.as_ref().filter(|copy|watched(copy));
            json!({
                "id":job.id,"track":job.track,"phase":job.phase,
                "bytes":job.bytes.to_string(),"total":job.total.map(|n|n.to_string()),
                "message":job.message,"destination":job.root.path.join(import::MANAGED),
                "local":local.map(|copy|&copy.reference),"local_version":local.map(|copy|&copy.fingerprint),
                "missing":copy.is_some()&&checked.is_none(),"unwatched":checked.as_ref().is_some_and(|copy|!watched(copy))
            })
        }).collect();
        let copies:Vec<_>=self.store.imports.iter().rev().filter(|copy|watched(copy))
            .filter_map(|copy|Destination::existing(copy.root.clone()).and_then(|destination|destination.verify(copy,false)).ok())
            .map(|copy|json!({"key":copy.key,"origin":copy.origin,"track":copy.track,"local":copy.reference,"local_version":copy.fingerprint})).collect();
        let view = json!({
            "available":self.writable,
            "destination":self.store.destination.as_ref().map(|root|json!({"source":root.id,"path":root.path.join(import::MANAGED)})),
            "destination_revision":self.store.destination_revision.to_string(),"jobs":jobs,"copies":copies,
            "message":self.problem,"max_jobs":MAX_JOBS,"max_file_bytes":MAX_FILE.to_string(),"budget_bytes":BUDGET.to_string()
        });
        let _ = self.events.send(Event::Downloads(view, published)).await;
    }
}
async fn transfer(
    job: Job,
    provider: Option<Arc<dyn MusicProvider>>,
    gate: Arc<Mutex<u64>>,
    generation: u64,
    work: mpsc::Sender<(u64, Work)>,
    imports: Vec<ImportedCopy>,
    limit: u64,
) -> Result<ImportedCopy, String> {
    let current = || *gate.lock().unwrap_or_else(|e| e.into_inner()) == generation;
    if !current() {
        return Err("Download cancelled because its account or destination changed.".into());
    }
    if let Some(copy) = &job.copy
        && let Ok(destination) = Destination::existing(copy.root.clone())
        && let Ok(copy) = destination.verify_while(copy, true, current)
    {
        return Ok(copy);
    }
    if !current() {
        return Err("Download cancelled because its account or destination changed.".into());
    }
    let provider = provider.ok_or("Reconnect the original Plex source before continuing.")?;
    let download = provider
        .download(job.track.reference.source, job.track.reference.key.clone())
        .await
        .map_err(|e| e.to_string())?;
    if download.media.track.reference != job.track.reference
        || download.media.version.is_empty()
        || download.media.version.len() > 256
        || download.media.stream.transcoded()
    {
        return Err("The source did not provide a pinned original file.".into());
    }
    let version = download.media.version;
    if job.version.as_ref().is_some_and(|v| v != &version) {
        return Err(
            "The original Plex file changed. Start a new download to accept its new version."
                .into(),
        );
    }
    let destination = Destination::existing(job.root.clone())?;
    let key = import_key(&job.track.reference, &version);
    if let Some(copy) = imports
        .iter()
        .rev()
        .find(|c| c.key == key && c.root.id == job.root.id)
        && let Ok(copy) = destination.verify_while(copy, true, current)
    {
        return Ok(copy);
    }
    if imports.len() >= MAX_IMPORTS {
        return Err("The managed download limit of 2000 imports was reached.".into());
    }
    if !current() {
        return Err("Download cancelled because its account or destination changed.".into());
    }
    let (reply, ack) = oneshot::channel();
    work.send((
        generation,
        Work::Pin {
            id: job.id.clone(),
            version: version.clone(),
            reply,
        },
    ))
    .await
    .map_err(|_| "Downloads stopped.")?;
    ack.await.map_err(|_| "Downloads stopped.")??;
    let mut response = download
        .media
        .stream
        .open(None, false)
        .await
        .map_err(|e| e.to_string())?;
    if response
        .content_length
        .zip(download.expected_length)
        .is_some_and(|(a, b)| a != b)
    {
        return Err("The original file length disagrees with Plex metadata.".into());
    }
    response.content_length = response.content_length.or(download.expected_length);
    if response.content_type.as_ref().is_some_and(|kind| {
        let kind = kind.to_ascii_lowercase();
        kind.starts_with("text/")
            || kind.starts_with("application/json")
            || kind.starts_with("application/xml")
    }) {
        return Err("The server did not return an original audio file.".into());
    }
    if response.status != 200 || response.content_range.is_some() {
        return Err("The server did not return a complete original file.".into());
    }
    if response.content_length.is_some_and(|n| n == 0 || n > limit) || limit == 0 {
        return Err(
            "This download exceeds the 8 GiB file or 100 GiB managed storage limit.".into(),
        );
    }
    destination.space(response.content_length.unwrap_or(16 * 1024 * 1024))?;
    let mut part = destination.partial(&job.id)?;
    let mut bytes = 0u64;
    let mut digest = Sha256::new();
    let mut progress: Option<std::time::Instant> = None;
    while let Some(chunk) =
        tokio::time::timeout(std::time::Duration::from_secs(30), response.body.next())
            .await
            .map_err(|_| "Download timed out; retry explicitly.")?
    {
        if *gate.lock().unwrap_or_else(|e| e.into_inner()) != generation {
            return Err("Download cancelled because its account or destination changed.".into());
        }
        let chunk = chunk.map_err(|e| e.to_string())?;
        bytes = bytes
            .checked_add(chunk.len() as u64)
            .ok_or("Download byte count overflow.")?;
        if bytes > limit || response.content_length.is_some_and(|n| bytes > n) {
            return Err(
                "The original response exceeded its declared length or storage limit.".into(),
            );
        }
        Destination::write(&mut part, &chunk)?;
        digest.update(&chunk);
        if progress.is_none_or(|time| time.elapsed() >= std::time::Duration::from_millis(250)) {
            let _ = work.try_send((
                generation,
                Work::Progress {
                    id: job.id.clone(),
                    bytes,
                    total: response.content_length,
                },
            ));
            progress = Some(std::time::Instant::now());
        }
    }
    if bytes == 0 || response.content_length.is_some_and(|n| bytes != n) {
        return Err("The original download was truncated; no local file was published.".into());
    }
    let mut copy = destination.planned(
        &part,
        &job,
        &version,
        &download.extension,
        format!("{:x}", digest.finalize()),
        bytes,
    )?;
    let (reply, ack) = oneshot::channel();
    work.send((
        generation,
        Work::Publish {
            id: job.id.clone(),
            copy: copy.clone(),
            reply,
        },
    ))
    .await
    .map_err(|_| "Downloads stopped.")?;
    ack.await.map_err(|_| "Downloads stopped.")??;
    destination.publish(&part, &mut copy, &gate, generation)?;
    let _ = download.media.stream.close().await;
    Ok(copy)
}
#[derive(Clone)]
pub struct LocalCopy {
    pub track: Track,
    pub version: String,
}
/// A verified actor snapshot is enough to admit an occurrence. Pin its file
/// version now; ordinary background preparation rechecks bytes before playback.
/// No filesystem work runs in the serialized command loop.
pub fn local_copy(view: &Value, request: &Value) -> Result<LocalCopy, String> {
    let row = if request["job_id"].is_string() {
        view["jobs"]
            .as_array()
            .and_then(|rows| rows.iter().find(|j| j["id"] == request["job_id"]))
    } else {
        view["copies"].as_array().and_then(|rows| {
            rows.iter().find(|j| {
                j["key"] == request["copy_key"] && j["local"]["source"] == request["copy_source"]
            })
        })
    }
    .ok_or("This download job is no longer available.")?;
    let reference: TrackRef = serde_json::from_value(row["local"].clone())
        .map_err(|_| "The local copy is missing or changed. Rescan its folder.")?;
    let mut track: Track = serde_json::from_value(row["track"].clone())
        .map_err(|_| "The local copy is missing or changed. Rescan its folder.")?;
    track.reference = reference;
    let version = row["local_version"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("The local copy is missing or changed. Rescan its folder.")?
        .to_owned();
    Ok(LocalCopy { track, version })
}

pub fn path() -> Result<PathBuf, &'static str> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|p| p.join(".local/state"))
        })
        .map(|p| p.join("qoda/downloads-v1"))
        .ok_or("Download state directory is unavailable.")
}

#[cfg(test)]
mod tests;
