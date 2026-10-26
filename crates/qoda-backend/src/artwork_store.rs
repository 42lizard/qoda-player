//! Bounded disposable JPEG files. One worker owns disk I/O; epoch fences block late writes.
use crate::cache_files::{Directory, private_file};
use bytes::Bytes;
use qoda_provider::AlbumRef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::{
    fs::{File, FileTimes, OpenOptions},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot};
const MAX_FILES: usize = 8192;
const IMAGE_BYTES: u64 = 256 * 1024;
const FRESH: Duration = Duration::from_secs(7 * 24 * 60 * 60);
type Result<T> = std::result::Result<T, &'static str>;
#[derive(Clone)]
pub struct Paths {
    pub cache: PathBuf,
    pub settings: PathBuf,
}
impl Paths {
    pub fn discover() -> Result<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".cache")))
            .ok_or("No cache directory is available.")?;
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".config")))
            .ok_or("No settings directory is available.")?;
        if !cache.is_absolute() || !config.is_absolute() {
            return Err("Cache and settings directories must use absolute paths.");
        }
        Ok(Self {
            cache: cache.join("qoda/artwork-v1"),
            settings: config.join("qoda/cache-settings"),
        })
    }
}
#[derive(Clone, Serialize)]
pub struct View {
    pub state: &'static str,
    pub message: String,
    pub usage_known: bool,
    pub bytes: u64,
    pub entries: usize,
    pub budget_mib: u16,
    pub cleanup_pending: bool,
}
impl Default for View {
    fn default() -> Self {
        Self {
            state: "memory_only",
            message: "Artwork uses a bounded memory cache.".into(),
            usage_known: false,
            bytes: 0,
            entries: 0,
            budget_mib: 1024,
            cleanup_pending: false,
        }
    }
}
#[derive(Clone)]
pub struct Loaded {
    pub image: Bytes,
    pub fresh_for: Duration,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u8,
    budget_mib: u16,
    purge_pending: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            budget_mib: 1024,
            purge_pending: false,
        }
    }
}
fn managed(name: &str) -> bool {
    name.strip_suffix(".jpg").is_some_and(|id| {
        id.len() == 64
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) || name
        .strip_prefix(".tmp-")
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}
fn key(reference: &AlbumRef) -> String {
    format!(
        "{:x}.jpg",
        Sha256::digest(serde_json::to_vec(reference).expect("serializable reference"))
    )
}
struct Entry {
    name: String,
    bytes: u64,
    accessed: SystemTime,
}
struct Store {
    directory: Directory,
    settings: Directory,
    prefs: Preferences,
    _lock: File,
    bytes: u64,
    entries: usize,
}
impl Drop for Store {
    fn drop(&mut self) {
        // fork() can temporarily retain the same open file description in a
        // child before exec closes it. Closing our descriptor alone would
        // leave its flock held, rejecting an immediate store reopen.
        let _ = File::unlock(&self._lock);
    }
}
impl Store {
    fn open(paths: &Paths, clear: bool) -> Result<Self> {
        let settings = Directory::open(&paths.settings)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(settings.path().join("artwork.lock"))
            .map_err(|_| "Artwork cache lock is unavailable.")?;
        if !private_file(
            &lock
                .metadata()
                .map_err(|_| "Artwork cache lock could not be inspected.")?,
        ) {
            return Err("Artwork cache lock is unsafe.");
        }
        lock.try_lock()
            .map_err(|_| "Artwork cache is already in use by another backend.")?;
        let mut prefs = if let Some(file) = settings.file("artwork.json")? {
            if file
                .metadata()
                .map_err(|_| "Artwork settings could not be inspected.")?
                .len()
                > 4096
            {
                return Err(
                    "Artwork cache settings are invalid; existing settings were preserved.",
                );
            }
            serde_json::from_reader(file.take(4097)).map_err(
                |_| "Artwork cache settings are invalid; existing settings were preserved.",
            )?
        } else {
            Preferences::default()
        };
        if prefs.version != 1 || prefs.budget_mib > 4096 {
            return Err("Unsupported artwork cache settings were preserved.");
        }
        if clear {
            prefs.purge_pending = true;
            settings.atomic("artwork.json", &serde_json::to_vec(&prefs).unwrap())?;
        }
        let directory = Directory::open(&paths.cache)?;
        let mut store = Self {
            directory,
            settings,
            prefs,
            _lock: lock,
            bytes: 0,
            entries: 0,
        };
        if store.prefs.purge_pending {
            store.clear()?;
        } else {
            store.trim(0, None)?;
        }
        Ok(store)
    }
    fn save_preferences(&self) -> Result<()> {
        self.settings
            .atomic("artwork.json", &serde_json::to_vec(&self.prefs).unwrap())
    }
    fn scan(&self) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        let files = std::fs::read_dir(self.directory.path())
            .map_err(|_| "Artwork cache could not be inspected.")?;
        for (count, entry) in files.enumerate() {
            if count >= MAX_FILES + 32 {
                return Err("Artwork cache has too many files; cleanup is required.");
            }
            let entry = entry.map_err(|_| "Artwork cache could not be inspected.")?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !managed(&name) {
                continue;
            }
            let m = std::fs::symlink_metadata(entry.path())
                .map_err(|_| "Artwork cache could not be inspected.")?;
            if !private_file(&m) {
                return Err("Unsafe artwork cache entry preserved; cleanup is incomplete.");
            }
            if name.starts_with(".tmp-") || m.len() > IMAGE_BYTES {
                std::fs::remove_file(entry.path())
                    .map_err(|_| "Incomplete artwork files could not be removed.")?;
                continue;
            }
            entries.push(Entry {
                name,
                bytes: m.len(),
                accessed: m.accessed().unwrap_or(UNIX_EPOCH),
            });
        }
        Ok(entries)
    }
    fn trim(&mut self, incoming: u64, replacing: Option<&str>) -> Result<()> {
        let mut entries = self.scan()?;
        entries.sort_by_key(|e| e.accessed);
        let mut bytes = entries.iter().map(|e| e.bytes).sum::<u64>();
        let mut count = entries.len();
        for entry in entries {
            if Some(entry.name.as_str()) == replacing
                || bytes.saturating_add(incoming) > u64::from(self.prefs.budget_mib) * 1024 * 1024
                || count + usize::from(incoming > 0) > MAX_FILES
            {
                std::fs::remove_file(self.directory.path().join(entry.name))
                    .map_err(|_| "Artwork eviction failed; the cache target could not be met.")?;
                bytes -= entry.bytes;
                count -= 1;
            }
        }
        self.bytes = bytes;
        self.entries = count;
        Ok(())
    }
    fn get(&mut self, name: &str) -> Result<Option<Loaded>> {
        if self.prefs.purge_pending || self.prefs.budget_mib == 0 {
            return Ok(None);
        }
        let Some(mut file) = self.directory.file(name)? else {
            return Ok(None);
        };
        let m = file
            .metadata()
            .map_err(|_| "Artwork cache data could not be inspected.")?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Artwork cache data could not be read.")?;
        if !qoda_provider::artwork::bounded_jpeg(&bytes) {
            self.remove(name)?;
            return Ok(None);
        }
        let now = SystemTime::now();
        file.set_times(FileTimes::new().set_accessed(now))
            .map_err(|_| "Artwork cache access could not be recorded.")?;
        let fresh_for = m
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .map(|age| FRESH.saturating_sub(age))
            .unwrap_or_default();
        Ok(Some(Loaded {
            image: bytes.into(),
            fresh_for,
        }))
    }
    fn put(&mut self, name: &str, image: &Bytes) -> Result<()> {
        if self.prefs.purge_pending || self.prefs.budget_mib == 0 {
            return Ok(());
        }
        if !qoda_provider::artwork::bounded_jpeg(image) {
            return Err("Invalid artwork was not cached.");
        }
        self.trim(image.len() as u64, Some(name))?;
        self.directory.atomic(name, image)?;
        self.bytes += image.len() as u64;
        self.entries += 1;
        Ok(())
    }
    fn remove(&mut self, name: &str) -> Result<()> {
        match std::fs::remove_file(self.directory.path().join(name)) {
            Ok(()) => self.trim(0, None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err("Artwork cache entry could not be removed."),
        }
    }
    fn clear(&mut self) -> Result<()> {
        self.prefs.purge_pending = true;
        self.save_preferences()?;
        // Unlike reads, unlinking a symlink removes only the cache node, never its target.
        let files = std::fs::read_dir(self.directory.path())
            .map_err(|_| "Artwork cleanup could not inspect the cache.")?;
        for (count, entry) in files.enumerate() {
            if count >= MAX_FILES + 32 {
                return Err("Artwork cleanup is incomplete; retry clearing the cache.");
            }
            let entry = entry.map_err(|_| "Artwork cleanup is incomplete.")?;
            if entry.file_name().to_str().is_some_and(managed) {
                std::fs::remove_file(entry.path()).map_err(
                    |_| "Artwork cleanup is incomplete; fix cache permissions and retry.",
                )?;
            }
        }
        self.directory
            .0
            .sync_all()
            .map_err(|_| "Artwork cleanup could not be synced.")?;
        self.bytes = 0;
        self.entries = 0;
        self.prefs.purge_pending = false;
        self.save_preferences()
    }
    fn budget(&mut self, mib: u16) -> Result<()> {
        if mib > 4096 {
            return Err("Choose an artwork cache target between 0 and 4096 MiB.");
        }
        let old = self.prefs.clone();
        self.prefs.budget_mib = mib;
        if let Err(error) = self.save_preferences() {
            self.prefs = old;
            return Err(error);
        }
        self.trim(0, None)
    }
}
enum Op {
    Get(String),
    Put(String, Bytes),
    Remove(String),
    Clear,
    Budget(u16),
    Stop,
}
enum Reply {
    Image(Option<Loaded>),
    Done,
}
struct Job {
    epoch: u64,
    op: Op,
    reply: oneshot::Sender<Result<Reply>>,
}
#[derive(Clone)]
pub struct Disk {
    tx: Option<mpsc::Sender<Job>>,
    epoch: Arc<AtomicU64>,
    blocked: Arc<AtomicBool>,
    view: Arc<Mutex<View>>,
}
impl Disk {
    pub fn disabled() -> Self {
        Self {
            tx: None,
            epoch: Default::default(),
            blocked: Default::default(),
            view: Default::default(),
        }
    }
    pub async fn start(paths: Result<Paths>) -> Self {
        let (tx, mut rx) = mpsc::channel::<Job>(32);
        let (ready, wait) = oneshot::channel();
        let this = Self {
            tx: Some(tx),
            ..Self::disabled()
        };
        let view = this.view.clone();
        let epoch = this.epoch.clone();
        let blocked = this.blocked.clone();
        std::thread::spawn(move || {
            let opened = paths
                .as_ref()
                .map_err(|e| *e)
                .and_then(|p| Store::open(p, false));
            let (mut store, mut last_error) = match opened {
                Ok(store) => {
                    update_view(&view, Some(&store), false, None);
                    (Some(store), None)
                }
                Err(error) => {
                    blocked.store(true, Ordering::Release);
                    update_view(&view, None, true, Some(error));
                    (None, Some(error))
                }
            };
            let _ = ready.send(());
            while let Some(job) = rx.blocking_recv() {
                if matches!(&job.op, Op::Stop) {
                    drop(store);
                    let _ = job.reply.send(Ok(Reply::Done));
                    break;
                }
                let clear = matches!(&job.op, Op::Clear);
                let mutating = !matches!(&job.op, Op::Get(_));
                let suppressed = !clear
                    && (job.epoch != epoch.load(Ordering::Acquire)
                        || blocked.load(Ordering::Acquire));
                let result = if suppressed {
                    Err("Artwork cache work was cancelled.")
                } else {
                    if clear && store.is_none() {
                        match paths
                            .as_ref()
                            .map_err(|error| *error)
                            .and_then(|paths| Store::open(paths, true))
                        {
                            Ok(opened) => store = Some(opened),
                            Err(error) => last_error = Some(error),
                        }
                    }
                    match store.as_mut() {
                        None => Err(last_error
                            .unwrap_or("Artwork cache unavailable; cleanup may be incomplete.")),
                        Some(store) => match job.op {
                            Op::Get(name) => store.get(&name).map(Reply::Image),
                            Op::Put(name, image) => store.put(&name, &image).map(|_| Reply::Done),
                            Op::Remove(name) => store.remove(&name).map(|_| Reply::Done),
                            Op::Clear => store.clear().map(|_| Reply::Done),
                            Op::Budget(mib) => store.budget(mib).map(|_| Reply::Done),
                            Op::Stop => unreachable!("handled before cache operations"),
                        },
                    }
                };
                if clear && result.is_ok() && job.epoch == epoch.load(Ordering::Acquire) {
                    blocked.store(false, Ordering::Release);
                }
                let cancelled = job.epoch != epoch.load(Ordering::Acquire);
                if !cancelled && !suppressed {
                    if let Err(error) = &result {
                        last_error = Some(*error);
                    } else if mutating {
                        last_error = None;
                    }
                    update_view(
                        &view,
                        store.as_ref(),
                        blocked.load(Ordering::Acquire),
                        last_error,
                    );
                }
                let _ = job.reply.send(result);
            }
        });
        let _ = wait.await;
        this
    }
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    pub fn view(&self) -> View {
        self.view.lock().unwrap().clone()
    }
    async fn request(&self, epoch: u64, op: Op) -> Result<Reply> {
        let Some(tx) = &self.tx else {
            return Ok(Reply::Done);
        };
        let (reply, rx) = oneshot::channel();
        let clear = matches!(&op, Op::Clear);
        let job = Job { epoch, op, reply };
        if clear {
            tokio::time::timeout(Duration::from_secs(2), tx.send(job))
                .await
                .map_err(|_| "Artwork cleanup is still pending; retry.")?
                .map_err(|_| "Artwork cache worker stopped.")?;
        } else {
            tx.try_send(job)
                .map_err(|_| "Artwork cache is busy; using provider access.")?;
        }
        tokio::time::timeout(Duration::from_secs(3), rx)
            .await
            .map_err(|_| "Artwork cache operation is still pending.")?
            .map_err(|_| "Artwork cache worker stopped.")?
    }
    pub async fn get(&self, epoch: u64, reference: &AlbumRef) -> Option<Loaded> {
        match self.request(epoch, Op::Get(key(reference))).await {
            Ok(Reply::Image(value)) => value,
            _ => None,
        }
    }
    pub async fn put(&self, epoch: u64, reference: &AlbumRef, image: Bytes) {
        let _ = self.request(epoch, Op::Put(key(reference), image)).await;
    }
    pub async fn remove(&self, epoch: u64, reference: &AlbumRef) {
        let _ = self.request(epoch, Op::Remove(key(reference))).await;
    }
    pub fn begin_clear(&self) -> Option<u64> {
        self.tx.as_ref()?;
        self.blocked.store(true, Ordering::Release);
        let next = self
            .epoch
            .try_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                old.checked_add(1)
            });
        let mut view = self.view.lock().unwrap();
        view.cleanup_pending = true;
        view.state = "cleanup_pending";
        view.message = "Artwork cleanup pending; disk reads and writes are disabled.".into();
        next.ok().map(|old| old + 1)
    }
    pub async fn finish_clear(&self, epoch: u64) -> Result<()> {
        let result = self.request(epoch, Op::Clear).await.map(|_| ());
        if let Err(error) = result {
            self.view.lock().unwrap().message = error.into();
        }
        result
    }
    pub async fn clear(&self) {
        if let Some(epoch) = self.begin_clear() {
            let _ = self.finish_clear(epoch).await;
        }
    }
    pub async fn shutdown(&self) {
        let _ = self.request(self.epoch(), Op::Stop).await;
    }
    pub async fn budget(&self, mib: u16) -> Result<()> {
        self.request(self.epoch(), Op::Budget(mib))
            .await
            .map(|_| ())
    }
}
fn update_view(
    view: &Mutex<View>,
    store: Option<&Store>,
    blocked: bool,
    error: Option<&'static str>,
) {
    let mut view = view.lock().unwrap();
    if let Some(store) = store {
        view.bytes = store.bytes;
        view.entries = store.entries;
        view.budget_mib = store.prefs.budget_mib;
    }
    view.usage_known = store.is_some() && error.is_none();
    view.cleanup_pending = blocked || store.is_some_and(|s| s.prefs.purge_pending);
    view.state = if view.cleanup_pending {
        "cleanup_pending"
    } else if error.is_some() || store.is_none() {
        "unavailable"
    } else if view.budget_mib == 0 {
        "disabled"
    } else {
        "ready"
    };
    view.message = error
        .unwrap_or(if view.cleanup_pending {
            "Artwork cleanup is incomplete. Fix storage access and Retry cleanup."
        } else if view.budget_mib == 0 {
            "Disk artwork cache disabled. Memory caching remains available."
        } else {
            "Artwork cache ready. Visible covers are cached on demand."
        })
        .into();
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use qoda_core::SourceId;
    use std::os::unix::fs::symlink;
    const JPEG: &[u8] = include_bytes!("../../qoda-provider/tests/fixtures/cover.jpg");
    pub(crate) struct Fixture {
        root: PathBuf,
        pub(crate) paths: Paths,
    }
    impl Fixture {
        pub fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("qoda-art-cache-{}", uuid::Uuid::new_v4()));
            Self {
                paths: Paths {
                    cache: root.join("cache"),
                    settings: root.join("settings"),
                },
                root,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn reference(source: SourceId, key: &str) -> AlbumRef {
        AlbumRef {
            source,
            key: key.into(),
        }
    }
    #[test]
    fn persisted_artwork_is_source_scoped_and_reading_does_not_extend_freshness() {
        let fixture = Fixture::new();
        let source = SourceId::new();
        let first = reference(source, "same");
        let other = reference(SourceId::new(), "same");
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.put(&key(&first), &Bytes::from_static(JPEG)).unwrap();
        assert!(store.get(&key(&other)).unwrap().is_none());
        assert!(
            store.get(&key(&first)).unwrap().unwrap().fresh_for > Duration::from_secs(6 * 86400)
        );
        let file = store.directory.file(&key(&first)).unwrap().unwrap();
        let old = SystemTime::now() - Duration::from_secs(8 * 86400);
        file.set_times(FileTimes::new().set_modified(old)).unwrap();
        let loaded = store.get(&key(&first)).unwrap().unwrap();
        assert_eq!(loaded.image.as_ref(), JPEG);
        assert!(loaded.fresh_for.is_zero());
        assert_eq!(file.metadata().unwrap().modified().unwrap(), old);
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        drop(store);
        let mut reopened = Store::open(&fixture.paths, false).unwrap();
        assert!(
            reopened
                .get(&key(&first))
                .unwrap()
                .unwrap()
                .fresh_for
                .is_zero()
        );
    }
    #[test]
    fn closing_store_releases_lock_even_while_an_inherited_descriptor_survives() {
        let fixture = Fixture::new();
        let store = Store::open(&fixture.paths, false).unwrap();
        let inherited = store._lock.try_clone().unwrap();
        assert!(Store::open(&fixture.paths, false).is_err());
        drop(store);
        let reopened = Store::open(&fixture.paths, false).unwrap();
        drop(inherited);
        assert!(Store::open(&fixture.paths, false).is_err());
        drop(reopened);
        assert!(Store::open(&fixture.paths, false).is_ok());
    }
    #[test]
    fn eviction_respects_access_order_budget_and_preferences_survive_clear() {
        let fixture = Fixture::new();
        let source = SourceId::new();
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.budget(1).unwrap();
        let mut large = JPEG[..JPEG.len() - 2].to_vec();
        large.resize(220 * 1024 - 2, 0);
        large.extend([0xff, 0xd9]);
        let image = Bytes::from(large);
        let refs = (0..5)
            .map(|i| reference(source, &i.to_string()))
            .collect::<Vec<_>>();
        for (i, reference) in refs.iter().take(4).enumerate() {
            store.put(&key(reference), &image).unwrap();
            store
                .directory
                .file(&key(reference))
                .unwrap()
                .unwrap()
                .set_times(
                    FileTimes::new().set_accessed(UNIX_EPOCH + Duration::from_secs(i as u64 + 1)),
                )
                .unwrap();
        }
        store.get(&key(&refs[0])).unwrap();
        store.put(&key(&refs[4]), &image).unwrap();
        assert!(store.get(&key(&refs[1])).unwrap().is_none());
        assert!(store.get(&key(&refs[0])).unwrap().is_some());
        assert!(store.bytes <= 1024 * 1024);
        assert_eq!(store.entries, 4);
        store.clear().unwrap();
        assert_eq!(store.entries, 0);
        drop(store);
        let store = Store::open(&fixture.paths, false).unwrap();
        assert_eq!(store.prefs.budget_mib, 1);
        assert!(!store.prefs.purge_pending);
    }
    #[test]
    fn invalid_images_and_newer_preferences_are_not_trusted_or_silently_overwritten() {
        let fixture = Fixture::new();
        let mut store = Store::open(&fixture.paths, false).unwrap();
        let name = key(&reference(SourceId::new(), "1"));
        assert!(store.put(&name, &Bytes::from_static(b"not jpeg")).is_err());
        store.directory.atomic(&name, b"broken image").unwrap();
        assert!(store.get(&name).unwrap().is_none());
        assert!(!store.directory.path().join(name).exists());
        store
            .settings
            .atomic(
                "artwork.json",
                br#"{"version":2,"budget_mib":12,"purge_pending":false}"#,
            )
            .unwrap();
        drop(store);
        let before = std::fs::read(fixture.paths.settings.join("artwork.json")).unwrap();
        assert!(Store::open(&fixture.paths, false).is_err());
        assert_eq!(
            std::fs::read(fixture.paths.settings.join("artwork.json")).unwrap(),
            before
        );
    }
    #[test]
    fn file_links_are_refused_and_open_directory_identity_survives_root_path_replacement() {
        let fixture = Fixture::new();
        let mut store = Store::open(&fixture.paths, false).unwrap();
        let name = key(&reference(SourceId::new(), "1"));
        store.put(&name, &Bytes::from_static(JPEG)).unwrap();
        let outside = fixture.root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("sentinel"), b"preserved").unwrap();
        let renamed = fixture.root.join("original-cache");
        std::fs::rename(&fixture.paths.cache, &renamed).unwrap();
        symlink(&outside, &fixture.paths.cache).unwrap();
        assert_eq!(store.get(&name).unwrap().unwrap().image.as_ref(), JPEG);
        let evil = key(&reference(SourceId::new(), "2"));
        symlink(outside.join("sentinel"), renamed.join(&evil)).unwrap();
        assert!(store.get(&evil).is_err());
        std::fs::set_permissions(
            outside.join("sentinel"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let hard = key(&reference(SourceId::new(), "hardlink"));
        std::fs::hard_link(outside.join("sentinel"), renamed.join(&hard)).unwrap();
        assert!(store.get(&hard).is_err());
        store.clear().unwrap();
        assert_eq!(
            std::fs::read(outside.join("sentinel")).unwrap(),
            b"preserved"
        );
        assert!(std::fs::read_dir(&outside).unwrap().count() == 1);
    }
    #[test]
    fn incomplete_purge_is_durable_and_restart_finishes_it_before_cache_reuse() {
        let fixture = Fixture::new();
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.budget(2).unwrap();
        let name = key(&reference(SourceId::new(), "blocking"));
        let obstacle = store.directory.path().join(&name);
        std::fs::create_dir(&obstacle).unwrap();
        assert!(store.clear().is_err());
        assert!(store.prefs.purge_pending);
        drop(store);
        let profile: Preferences = serde_json::from_slice(
            &std::fs::read(fixture.paths.settings.join("artwork.json")).unwrap(),
        )
        .unwrap();
        assert!(profile.purge_pending);
        assert!(Store::open(&fixture.paths, false).is_err());
        std::fs::remove_dir(fixture.paths.cache.join(name)).unwrap();
        let store = Store::open(&fixture.paths, false).unwrap();
        assert!(!store.prefs.purge_pending);
        assert_eq!(store.bytes, 0);
        assert_eq!(store.prefs.budget_mib, 2);
    }
    #[tokio::test]
    async fn clear_fences_queued_and_late_writes_and_releases_profile_lock_on_shutdown() {
        let fixture = Fixture::new();
        let disk = Disk::start(Ok(fixture.paths.clone())).await;
        let source = SourceId::new();
        let old = disk.epoch();
        disk.put(old, &reference(source, "first"), Bytes::from_static(JPEG))
            .await;
        assert_eq!(disk.view().entries, 1);
        let mut replies = Vec::new();
        for i in 0..20 {
            let (reply, rx) = oneshot::channel();
            disk.tx
                .as_ref()
                .unwrap()
                .try_send(Job {
                    epoch: old,
                    op: Op::Put(
                        key(&reference(source, &i.to_string())),
                        Bytes::from_static(JPEG),
                    ),
                    reply,
                })
                .unwrap();
            replies.push(rx);
        }
        disk.clear().await;
        for reply in replies {
            let _ = reply.await;
        }
        disk.put(old, &reference(source, "late"), Bytes::from_static(JPEG))
            .await;
        assert_eq!(disk.view().entries, 0);
        assert!(!disk.view().cleanup_pending);
        disk.shutdown().await;
        let store = Store::open(&fixture.paths, false).unwrap();
        assert_eq!(store.entries, 0);
    }
    #[tokio::test]
    async fn write_failures_remain_visible_after_reads_and_clear_after_successful_recovery() {
        let fixture = Fixture::new();
        let disk = Disk::start(Ok(fixture.paths.clone())).await;
        let good = reference(SourceId::new(), "good");
        disk.put(disk.epoch(), &good, Bytes::from_static(JPEG))
            .await;
        let bad = reference(SourceId::new(), "blocked");
        let obstacle = fixture.paths.cache.join(key(&bad));
        std::fs::create_dir(&obstacle).unwrap();
        disk.put(disk.epoch(), &bad, Bytes::from_static(JPEG)).await;
        assert_eq!(disk.view().state, "unavailable");
        assert!(disk.get(disk.epoch(), &good).await.is_some());
        assert_eq!(disk.view().state, "unavailable");
        std::fs::remove_dir(obstacle).unwrap();
        disk.budget(1).await.unwrap();
        assert_eq!(disk.view().state, "ready");
        disk.shutdown().await;
    }
    #[test]
    fn one_profile_has_one_disk_owner_and_shared_directory_permissions_are_refused() {
        let fixture = Fixture::new();
        let store = Store::open(&fixture.paths, false).unwrap();
        assert!(Store::open(&fixture.paths, false).is_err());
        drop(store);
        std::fs::set_permissions(&fixture.paths.cache, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(Store::open(&fixture.paths, false).is_err());
    }
}
