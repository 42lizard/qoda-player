//! Disposable, source-scoped catalog pages. Credentials, media grants and edits are excluded.
use crate::cache_files::{Directory, private_file};
use qoda_core::{Counter, SourceId};
use qoda_provider::{
    AlbumPage, AlbumRef, AlbumSnapshot, ArtistPage, ArtistRef, CatalogPage, MusicProvider,
    ProviderError, Track,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    future::Future,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot};
const APP_ID: i32 = 0x514f4443;
const MAX_PAYLOAD: usize = 3 * 1024 * 1024;
const MAX_PAGES: i64 = 4096;
const MAX_TRACKS: i64 = 50_000;
const DB_FILES: [&str; 4] = [
    "catalog.sqlite3",
    "catalog.sqlite3-journal",
    "catalog.sqlite3-wal",
    "catalog.sqlite3-shm",
];
type Result<T> = std::result::Result<T, &'static str>;
fn source_key(source: SourceId) -> String {
    uuid::Uuid::from(source).to_string()
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| d.as_secs().try_into().ok())
        .unwrap_or(0)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Query {
    AlbumDetail {
        reference: AlbumRef,
    },
    Tracks {
        source: SourceId,
        offset: u64,
        query: String,
    },
    Albums {
        source: SourceId,
        offset: u64,
        query: String,
        artist: Option<ArtistRef>,
    },
    Artists {
        source: SourceId,
        offset: u64,
        query: String,
    },
}
impl Query {
    pub fn source(&self) -> SourceId {
        match self {
            Self::AlbumDetail { reference } => reference.source,
            Self::Tracks { source, .. }
            | Self::Albums { source, .. }
            | Self::Artists { source, .. } => *source,
        }
    }
    fn offset(&self) -> u64 {
        match self {
            Self::AlbumDetail { .. } => 0,
            Self::Tracks { offset, .. }
            | Self::Albums { offset, .. }
            | Self::Artists { offset, .. } => *offset,
        }
    }
    fn text(&self) -> &str {
        match self {
            Self::AlbumDetail { .. } => "",
            Self::Tracks { query, .. }
            | Self::Albums { query, .. }
            | Self::Artists { query, .. } => query,
        }
    }
    fn key(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).expect("query"))
        )
    }
    async fn fetch(
        &self,
        provider: &dyn MusicProvider,
    ) -> std::result::Result<Page, ProviderError> {
        match self {
            Self::AlbumDetail { reference } => provider
                .album(reference.clone())
                .await
                .map(Page::AlbumDetail),
            Self::Tracks {
                source,
                offset,
                query,
            } => provider
                .tracks(*source, *offset, query.clone())
                .await
                .map(Page::Tracks),
            Self::Albums {
                source,
                offset,
                query,
                artist,
            } => match artist {
                Some(artist) => provider
                    .artist_albums(artist.clone(), *offset, query.clone())
                    .await
                    .map(Page::Albums),
                None => provider
                    .albums(*source, *offset, query.clone())
                    .await
                    .map(Page::Albums),
            },
            Self::Artists {
                source,
                offset,
                query,
            } => provider
                .artists(*source, *offset, query.clone())
                .await
                .map(Page::Artists),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "page", rename_all = "snake_case")]
pub enum Page {
    AlbumDetail(AlbumSnapshot),
    Tracks(CatalogPage),
    Albums(AlbumPage),
    Artists(ArtistPage),
}
impl Page {
    /// Covers have their own cache and budget; snapshots persist only catalog metadata.
    fn metadata_only(&self) -> Self {
        let mut page = self.clone();
        if let Self::AlbumDetail(snapshot) = &mut page {
            snapshot.artwork = None;
            snapshot.artwork_unavailable = true;
        }
        page
    }
    fn valid(&self, request: &Query) -> bool {
        let source = request.source();
        let offset = request.offset();
        let query = request.text();
        let bounded = |s: &str| s.len() <= 1024;
        let reference = |s: &str| !s.trim().is_empty() && s.len() <= 512;
        let track = |t: &Track| {
            t.reference.source == source
                && reference(&String::from(t.reference.key.clone()))
                && bounded(&t.title)
                && bounded(&t.artist)
                && bounded(&t.album)
        };
        let shape = match (self, request) {
            (
                Self::AlbumDetail(snapshot),
                Query::AlbumDetail {
                    reference: requested,
                },
            ) => {
                let album = &snapshot.album;
                album.reference == *requested
                    && reference(&album.reference.key)
                    && bounded(&album.title)
                    && bounded(&album.artist)
                    && album
                        .artist_ref
                        .as_ref()
                        .is_none_or(|a| a.source == source && reference(&a.key))
                    && album
                        .track_count
                        .is_some_and(|count| count.get() == snapshot.tracks.len() as u64)
                    && reference(&snapshot.revision)
                    && snapshot.artwork.is_none()
                    && snapshot.artwork_unavailable
                    && snapshot.tracks.len() <= 500
                    && snapshot.tracks.iter().all(|t| track(&t.track))
                    && snapshot.tracks.windows(2).all(|pair| {
                        (pair[0].disc, pair[0].number) <= (pair[1].disc, pair[1].number)
                    })
                    && snapshot
                        .tracks
                        .iter()
                        .map(|t| String::from(t.track.reference.key.clone()))
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        == snapshot.tracks.len()
            }
            (Self::Tracks(page), Query::Tracks { .. }) => {
                page.source == source
                    && page.offset.get() == offset
                    && page.query == query
                    && page.tracks.len() <= 100
                    && page.tracks.iter().all(track)
                    && page
                        .tracks
                        .iter()
                        .map(|t| String::from(t.reference.key.clone()))
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        == page.tracks.len()
            }
            (Self::Albums(page), Query::Albums { artist, .. }) => {
                page.source == source
                    && page.offset.get() == offset
                    && page.query == query
                    && page.albums.len() <= 100
                    && page.artist.as_ref().map(|a| &a.reference) == artist.as_ref()
                    && page.artist.as_ref().is_none_or(|a| {
                        a.reference.source == source
                            && reference(&a.reference.key)
                            && bounded(&a.title)
                    })
                    && page.albums.iter().all(|a| {
                        a.reference.source == source
                            && reference(&a.reference.key)
                            && bounded(&a.title)
                            && bounded(&a.artist)
                            && a.artist_ref
                                .as_ref()
                                .is_none_or(|a| a.source == source && reference(&a.key))
                    })
                    && page
                        .albums
                        .iter()
                        .map(|a| &a.reference.key)
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        == page.albums.len()
            }
            (Self::Artists(page), Query::Artists { .. }) => {
                page.source == source
                    && page.offset.get() == offset
                    && page.query == query
                    && page.artists.len() <= 100
                    && page.artists.iter().all(|a| {
                        a.reference.source == source
                            && reference(&a.reference.key)
                            && bounded(&a.title)
                    })
                    && page
                        .artists
                        .iter()
                        .map(|a| &a.reference.key)
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        == page.artists.len()
            }
            _ => false,
        };
        shape
            && query.chars().count() <= 512
            && serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= MAX_PAYLOAD)
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Info {
    pub origin: &'static str,
    pub stale: bool,
    pub incomplete: bool,
    pub refreshing: bool,
    pub message: &'static str,
}
impl Default for Info {
    fn default() -> Self {
        Self {
            origin: "live",
            stale: false,
            incomplete: false,
            refreshing: false,
            message: "Live catalog page.",
        }
    }
}
impl Info {
    fn album_detail(stale: bool, refreshing: bool) -> Self {
        Self {
            origin: "cached",
            stale,
            incomplete: false,
            refreshing,
            message: if refreshing {
                "Cached album details · checking current library · queue actions wait for refresh."
            } else {
                "Cached album details · refresh unavailable · reopen or refresh before queueing."
            },
        }
    }
    fn cached(stale: bool, refreshing: bool) -> Self {
        Self {
            origin: "cached",
            stale,
            incomplete: true,
            refreshing,
            message: if refreshing {
                "Cached page · may be stale/incomplete · refreshing…"
            } else if stale {
                "Cached page · refresh unavailable · may be stale/incomplete."
            } else {
                "Cached page · catalog coverage may be incomplete."
            },
        }
    }
    fn search(refreshing: bool) -> Self {
        Self {
            origin: "cached_search",
            stale: true,
            incomplete: true,
            refreshing,
            message: if refreshing {
                "Cached tracks only · incomplete search · refreshing…"
            } else {
                "Cached tracks only · incomplete search · up to 100 matches."
            },
        }
    }
}
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
            .ok_or("No cache directory.")?;
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".config")))
            .ok_or("No cache settings directory.")?;
        if !cache.is_absolute() || !config.is_absolute() {
            return Err("Cache directories must be absolute.");
        }
        Ok(Self {
            cache: cache.join("qoda/catalog-v1"),
            settings: config.join("qoda/cache-settings"),
        })
    }
}
#[derive(Clone, Serialize)]
pub struct View {
    pub state: &'static str,
    pub message: String,
    pub bytes: u64,
    pub pages: u64,
    pub tracks: u64,
    pub budget_mib: u16,
    pub cleanup_pending: bool,
    pub usage_known: bool,
    pub rebuilt: bool,
}
impl Default for View {
    fn default() -> Self {
        Self {
            state: "memory_only",
            message: "Persistent metadata cache disabled.".into(),
            bytes: 0,
            pages: 0,
            tracks: 0,
            budget_mib: 256,
            cleanup_pending: false,
            usage_known: false,
            rebuilt: false,
        }
    }
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
            budget_mib: 256,
            purge_pending: false,
        }
    }
}
#[derive(Default)]
struct Stats {
    bytes: i64,
    pages: i64,
    tracks: i64,
}
enum DatabaseError {
    Corrupt,
    Unsupported,
    Unavailable,
}
fn database_error(error: rusqlite::Error) -> DatabaseError {
    if matches!(error,rusqlite::Error::SqliteFailure(error,_) if matches!(error.code,rusqlite::ErrorCode::DatabaseCorrupt|rusqlite::ErrorCode::NotADatabase))
    {
        DatabaseError::Corrupt
    } else {
        DatabaseError::Unavailable
    }
}
struct Store {
    root: Directory,
    settings: Directory,
    _lock: File,
    prefs: Preferences,
    conn: Option<Connection>,
    stats: Stats,
    rebuilt: bool,
    clock: i64,
    db_path: PathBuf,
    db_identity: (u64, u64),
}
#[derive(Clone)]
struct Hit {
    page: Page,
    fetched: i64,
}
impl Drop for Store {
    fn drop(&mut self) {
        // Close SQLite before releasing ownership. A parallel fork can briefly retain
        // this open file description despite CLOEXEC; close alone then keeps its lock.
        drop(self.conn.take());
        let _ = self._lock.unlock();
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
            .open(settings.path().join("catalog.lock"))
            .map_err(|_| "Metadata cache lock unavailable.")?;
        if !private_file(
            &lock
                .metadata()
                .map_err(|_| "Metadata cache lock unavailable.")?,
        ) {
            return Err("Metadata cache lock is unsafe.");
        }
        lock.try_lock()
            .map_err(|_| "Metadata cache is in use by another backend.")?;
        let mut prefs: Preferences = if let Some(file) = settings.file("catalog.json")? {
            if file
                .metadata()
                .map_err(|_| "Metadata settings unavailable.")?
                .len()
                > 4096
            {
                return Err("Metadata settings are oversized; preserved existing settings.");
            }
            serde_json::from_reader(file.take(4097))
                .map_err(|_| "Metadata cache settings are damaged; preserved existing settings.")?
        } else {
            Preferences::default()
        };
        if prefs.version != 1 || prefs.budget_mib > 1024 {
            return Err("Unsupported metadata cache settings were preserved.");
        }
        if clear {
            prefs.purge_pending = true;
            settings.atomic("catalog.json", &serde_json::to_vec(&prefs).unwrap())?;
        }
        let root = Directory::open(&paths.cache)?;
        let mut store = Self {
            root,
            settings,
            _lock: lock,
            prefs,
            conn: None,
            stats: Stats::default(),
            rebuilt: false,
            clock: 0,
            db_path: PathBuf::new(),
            db_identity: (0, 0),
        };
        if store.prefs.purge_pending {
            store.clear()?;
        } else {
            store.connect()?;
            store.trim()?;
        }
        Ok(store)
    }
    fn save_preferences(&self) -> Result<()> {
        self.settings
            .atomic("catalog.json", &serde_json::to_vec(&self.prefs).unwrap())
    }
    fn remove_files(&self) -> Result<()> {
        for name in DB_FILES {
            match std::fs::remove_file(self.root.path().join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err("Metadata cleanup is incomplete. Fix cache storage and retry.");
                }
            }
        }
        self.root
            .0
            .sync_all()
            .map_err(|_| "Metadata cleanup could not be synced.")
    }
    fn connect(&mut self) -> Result<()> {
        for name in DB_FILES {
            let _ = self.root.file(name)?;
        }
        let path = self.root.resolved_path()?.join(DB_FILES[0]);
        if self.root.file(DB_FILES[0])?.is_none() {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
                .map_err(|_| "Metadata cache cannot create its database.")?;
        }
        let opened = Self::database(&path);
        match opened {
            Ok(conn) => self.conn = Some(conn),
            Err(DatabaseError::Unsupported) => {
                return Err(
                    "Unsupported or foreign metadata database preserved. Clear only if it is disposable cache data.",
                );
            }
            Err(DatabaseError::Unavailable) => {
                return Err("Metadata cache database is unavailable; existing data was preserved.");
            }
            Err(DatabaseError::Corrupt) => {
                self.remove_files()?;
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&path)
                    .map_err(|_| "Metadata cache recovery failed.")?;
                self.conn =
                    Some(Self::database(&path).map_err(|_| "Metadata cache recovery failed.")?);
                self.rebuilt = true;
            }
        };
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| "Metadata database identity unavailable.")?;
        self.db_identity = (metadata.dev(), metadata.ino());
        self.db_path = path;
        self.clock=self.connection()?.query_row("SELECT max(coalesce((SELECT max(accessed) FROM pages),0),coalesce((SELECT max(accessed) FROM tracks),0))",[],|row|row.get(0)).map_err(|_|"Metadata access clock unavailable.")?;
        Ok(())
    }
    fn tick(&mut self) -> Result<i64> {
        self.clock = self
            .clock
            .checked_add(1)
            .ok_or("Metadata access clock exhausted.")?;
        Ok(self.clock)
    }
    fn database(path: &std::path::Path) -> std::result::Result<Connection, DatabaseError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_NOFOLLOW | OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .map_err(database_error)?;
        conn.busy_timeout(Duration::from_millis(200))
            .map_err(database_error)?;
        conn.pragma_update(None, "trusted_schema", false)
            .map_err(database_error)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(database_error)?;
        let id: i32 = conn
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .map_err(database_error)?;
        if version > 1 || (id != 0 && id != APP_ID) {
            return Err(DatabaseError::Unsupported);
        }
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )
            .map_err(database_error)?;
        if version == 0 {
            if id != 0 || tables != 0 {
                return Err(DatabaseError::Unsupported);
            }
            conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")
                .map_err(database_error)?;
            conn.execute_batch(&format!("BEGIN;CREATE TABLE pages(key TEXT PRIMARY KEY,source TEXT NOT NULL,payload BLOB NOT NULL,fetched INTEGER NOT NULL,accessed INTEGER NOT NULL,bytes INTEGER NOT NULL CHECK(bytes>0));CREATE INDEX pages_source ON pages(source);CREATE INDEX pages_lru ON pages(accessed,key);CREATE TABLE tracks(source TEXT NOT NULL,key TEXT NOT NULL,payload BLOB NOT NULL,title TEXT NOT NULL,artist TEXT NOT NULL,album TEXT NOT NULL,fetched INTEGER NOT NULL,accessed INTEGER NOT NULL,bytes INTEGER NOT NULL CHECK(bytes>0),PRIMARY KEY(source,key));CREATE INDEX tracks_lru ON tracks(accessed,source,key);PRAGMA application_id={APP_ID};PRAGMA user_version=1;COMMIT;")).map_err(database_error)?;
        } else if id != APP_ID {
            return Err(DatabaseError::Unsupported);
        }
        conn.pragma_update(None, "journal_mode", "DELETE")
            .map_err(database_error)?;
        conn.pragma_update(None, "secure_delete", true)
            .map_err(database_error)?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(database_error)?;
        let check: String = conn
            .query_row("PRAGMA quick_check(1)", [], |r| r.get(0))
            .map_err(database_error)?;
        if check != "ok" {
            return Err(DatabaseError::Corrupt);
        }
        conn.prepare("SELECT key,source,payload,fetched,accessed,bytes FROM pages LIMIT 0")
            .map_err(database_error)?;
        conn.prepare("SELECT source,key,payload,title,artist,album,fetched,accessed,bytes FROM tracks LIMIT 0").map_err(database_error)?;
        Ok(conn)
    }
    fn connection(&self) -> Result<&Connection> {
        if self.root.resolved_path()?.join(DB_FILES[0]) != self.db_path {
            return Err("Metadata directory moved; restart the cache before reuse.");
        }
        let file = self
            .root
            .file(DB_FILES[0])?
            .ok_or("Metadata database disappeared.")?;
        let metadata = file
            .metadata()
            .map_err(|_| "Metadata database identity unavailable.")?;
        if (metadata.dev(), metadata.ino()) != self.db_identity {
            return Err("Metadata database identity changed; restart before reuse.");
        }
        for name in &DB_FILES[1..] {
            let _ = self.root.file(name)?;
        }
        self.conn
            .as_ref()
            .ok_or("Metadata database is unavailable.")
    }
    fn refresh_stats(&mut self) -> Result<()> {
        self.stats=self.connection()?.query_row("SELECT (SELECT coalesce(sum(bytes),0) FROM pages)+(SELECT coalesce(sum(bytes),0) FROM tracks),(SELECT count(*) FROM pages),(SELECT count(*) FROM tracks)",[],|r|Ok(Stats{bytes:r.get(0)?,pages:r.get(1)?,tracks:r.get(2)?})).map_err(|_|"Metadata cache accounting failed.")?;
        Ok(())
    }
    fn trim(&mut self) -> Result<()> {
        self.refresh_stats()?;
        let target = i64::from(self.prefs.budget_mib) * 1024 * 1024;
        let conn = self.conn.as_ref().ok_or("Metadata cache unavailable.")?;
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| "Metadata eviction transaction failed.")?;
        let result = (|| {
            if target == 0 {
                conn.execute_batch("DELETE FROM pages;DELETE FROM tracks;")
                    .map_err(|_| "Metadata eviction failed.")?;
                return Ok(());
            }
            for _ in 0..MAX_PAGES + MAX_TRACKS + 102 {
                if self.stats.bytes <= target
                    && self.stats.pages <= MAX_PAGES
                    && self.stats.tracks <= MAX_TRACKS
                {
                    break;
                }
                let victim:Option<(String,i64,i64)>=conn.prepare_cached("SELECT kind,rowid,bytes FROM (SELECT * FROM (SELECT 'pages' kind,rowid,bytes,accessed FROM pages ORDER BY accessed,key LIMIT 1) UNION ALL SELECT * FROM (SELECT 'tracks' kind,rowid,bytes,accessed FROM tracks ORDER BY accessed,source,key LIMIT 1)) ORDER BY accessed,kind LIMIT 1").map_err(|_|"Metadata eviction failed.")?.query_row([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|_|"Metadata eviction failed.")?;
                let Some((kind, row, bytes)) = victim else {
                    break;
                };
                conn.execute(
                    if kind == "pages" {
                        "DELETE FROM pages WHERE rowid=?1"
                    } else {
                        "DELETE FROM tracks WHERE rowid=?1"
                    },
                    [row],
                )
                .map_err(|_| "Metadata eviction failed.")?;
                self.stats.bytes -= bytes;
                if kind == "pages" {
                    self.stats.pages -= 1;
                } else {
                    self.stats.tracks -= 1;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(error);
        }
        conn.execute_batch("COMMIT;PRAGMA incremental_vacuum;")
            .map_err(|_| "Metadata cache space could not be reclaimed.")?;
        self.refresh_stats()
    }
    fn get(&mut self, query: &Query) -> Result<Option<Hit>> {
        if self.prefs.purge_pending || self.prefs.budget_mib == 0 {
            return Ok(None);
        }
        let row: Option<(Vec<u8>, i64)> = self
            .connection()?
            .query_row(
                "SELECT substr(payload,1,?2),fetched FROM pages WHERE key=?1",
                params![query.key(), (MAX_PAYLOAD + 1) as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| "Cached page could not be read.")?;
        let Some((payload, fetched)) = row else {
            return Ok(None);
        };
        let page = serde_json::from_slice::<Page>(&payload)
            .ok()
            .filter(|page| page.valid(query));
        if let Some(page) = page {
            let stamp = self.tick()?;
            self.connection()?
                .execute(
                    "UPDATE pages SET accessed=?2 WHERE key=?1",
                    params![query.key(), stamp],
                )
                .map_err(|_| "Cache access could not be recorded.")?;
            Ok(Some(Hit { page, fetched }))
        } else {
            self.connection()?
                .execute("DELETE FROM pages WHERE key=?1", [query.key()])
                .map_err(|_| "Damaged cache row could not be discarded.")?;
            self.refresh_stats()?;
            Ok(None)
        }
    }
    fn put(&mut self, query: &Query, page: &Page) -> Result<()> {
        if self.prefs.budget_mib == 0 || self.prefs.purge_pending {
            return Ok(());
        }
        let page = page.metadata_only();
        if !page.valid(query) {
            return Err("Invalid catalog page was not cached.");
        }
        let _ = self.connection()?;
        let payload = serde_json::to_vec(&page).map_err(|_| "Metadata encoding failed.")?;
        let source = source_key(query.source());
        let time = now();
        let stamp = self.tick()?;
        let tx = self
            .conn
            .as_mut()
            .ok_or("Metadata cache unavailable.")?
            .transaction()
            .map_err(|_| "Metadata cache transaction failed.")?;
        tx.execute("INSERT INTO pages VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(key) DO UPDATE SET payload=excluded.payload,fetched=excluded.fetched,accessed=excluded.accessed,bytes=excluded.bytes",params![query.key(),source,payload,time,stamp,(payload.len()+128) as i64]).map_err(|_|"Metadata page could not be saved.")?;
        if let Page::Tracks(page) = &page {
            for track in &page.tracks {
                let payload = serde_json::to_vec(track).map_err(|_| "Track encoding failed.")?;
                let key = String::from(track.reference.key.clone());
                let title = track.title.to_lowercase();
                let artist = track.artist.to_lowercase();
                let album = track.album.to_lowercase();
                let bytes = payload.len()
                    + key.len()
                    + source.len()
                    + title.len()
                    + artist.len()
                    + album.len()
                    + 64;
                tx.execute("INSERT INTO tracks VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(source,key) DO UPDATE SET payload=excluded.payload,title=excluded.title,artist=excluded.artist,album=excluded.album,fetched=excluded.fetched,accessed=excluded.accessed,bytes=excluded.bytes",params![source,key,payload,title,artist,album,time,stamp,bytes as i64]).map_err(|_|"Track index could not be saved.")?;
            }
        }
        tx.commit()
            .map_err(|_| "Metadata cache transaction failed.")?;
        self.trim()
    }
    fn search(&mut self, query: &Query) -> Result<Option<Page>> {
        let Query::Tracks {
            source,
            offset,
            query: text,
        } = query
        else {
            return Ok(None);
        };
        if text.is_empty() || self.prefs.budget_mib == 0 || self.prefs.purge_pending {
            return Ok(None);
        }
        let Ok(offset) = i64::try_from(*offset) else {
            return Ok(None);
        };
        let mut statement=self.connection()?.prepare("SELECT substr(key,1,513),substr(payload,1,32769) FROM tracks WHERE source=?1 AND (instr(title,?2)>0 OR instr(artist,?2)>0 OR instr(album,?2)>0) ORDER BY title,key LIMIT 100 OFFSET ?3").map_err(|_|"Cached search unavailable.")?;
        let rows = statement
            .query_map(
                params![source_key(*source), text.to_lowercase(), offset],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)),
            )
            .map_err(|_| "Cached search unavailable.")?;
        let mut tracks = Vec::new();
        for row in rows {
            let (key, payload) = row.map_err(|_| "Cached search failed.")?;
            if let Ok(track) = serde_json::from_slice::<Track>(&payload) {
                let needle = text.to_lowercase();
                if String::from(track.reference.key.clone()) != key
                    || ![&track.title, &track.artist, &track.album]
                        .iter()
                        .any(|value| value.to_lowercase().contains(&needle))
                {
                    continue;
                }
                let probe = Page::Tracks(CatalogPage {
                    source: *source,
                    offset: Counter::new(query.offset()),
                    query: text.clone(),
                    total: None,
                    tracks: vec![track.clone()],
                    has_more: false,
                });
                if probe.valid(query) {
                    tracks.push(track);
                }
            }
        }
        drop(statement);
        if tracks.is_empty() {
            return Ok(None);
        }
        let stamp = self.tick()?;
        for track in &tracks {
            self.connection()?
                .execute(
                    "UPDATE tracks SET accessed=?3 WHERE source=?1 AND key=?2",
                    params![
                        source_key(*source),
                        String::from(track.reference.key.clone()),
                        stamp
                    ],
                )
                .map_err(|_| "Cached search access failed.")?;
        }
        Ok(Some(Page::Tracks(CatalogPage {
            source: *source,
            offset: Counter::new(query.offset()),
            query: text.clone(),
            tracks,
            total: None,
            has_more: false,
        })))
    }
    fn purge_source(&mut self, source: SourceId) -> Result<()> {
        let _ = self.connection()?;
        let tx = self
            .conn
            .as_mut()
            .ok_or("Metadata unavailable.")?
            .transaction()
            .map_err(|_| "Metadata invalidation failed.")?;
        tx.execute("DELETE FROM pages WHERE source=?1", [source_key(source)])
            .map_err(|_| "Metadata invalidation failed.")?;
        tx.execute("DELETE FROM tracks WHERE source=?1", [source_key(source)])
            .map_err(|_| "Metadata invalidation failed.")?;
        tx.commit().map_err(|_| "Metadata invalidation failed.")?;
        self.trim()
    }
    fn clear(&mut self) -> Result<()> {
        self.prefs.purge_pending = true;
        self.save_preferences()?;
        self.conn.take();
        self.remove_files()?;
        self.connect()?;
        self.stats = Stats::default();
        self.prefs.purge_pending = false;
        self.rebuilt = false;
        self.save_preferences()
    }
    fn budget(&mut self, mib: u16) -> Result<()> {
        if mib > 1024 {
            return Err("Choose a metadata target from 0 to 1024 MiB.");
        }
        let old = self.prefs.clone();
        self.prefs.budget_mib = mib;
        if let Err(error) = self.save_preferences() {
            self.prefs = old;
            return Err(error);
        }
        self.trim()
    }
}

enum Op {
    Get(Query),
    Put(Query, Box<Page>),
    Search(Query),
    Purge(SourceId),
    Clear,
    Budget(u16),
    Stop,
}
enum Reply {
    Hit(Option<Hit>),
    Search(Option<Page>),
    Done,
}
struct Job {
    epoch: u64,
    op: Op,
    reply: oneshot::Sender<Result<Reply>>,
}
#[derive(Clone, Default)]
pub struct Cache {
    tx: Option<mpsc::Sender<Job>>,
    epoch: Arc<AtomicU64>,
    blocked: Arc<AtomicBool>,
    view: Arc<Mutex<View>>,
}
impl Cache {
    pub async fn start(paths: Result<Paths>) -> Self {
        let (tx, mut rx) = mpsc::channel::<Job>(16);
        let (ready, wait) = oneshot::channel();
        let this = Self {
            tx: Some(tx),
            ..Self::default()
        };
        {
            let mut view = this.view.lock().unwrap();
            view.state = "opening";
            view.message = "Opening disposable metadata cache…".into();
        }
        let view = this.view.clone();
        let epoch = this.epoch.clone();
        let blocked = this.blocked.clone();
        std::thread::spawn(move || {
            let opened = paths
                .as_ref()
                .map_err(|error| *error)
                .and_then(|paths| Store::open(paths, false));
            let (mut store, mut warning) = match opened {
                Ok(store) => (Some(store), None),
                Err(error) => {
                    blocked.store(true, Ordering::Release);
                    (None, Some(error))
                }
            };
            update_view(
                &view,
                store.as_ref(),
                blocked.load(Ordering::Acquire),
                warning,
            );
            let _ = ready.send(());
            while let Some(job) = rx.blocking_recv() {
                if matches!(&job.op, Op::Stop) {
                    drop(store);
                    let _ = job.reply.send(Ok(Reply::Done));
                    break;
                }
                let clear = matches!(&job.op, Op::Clear);
                let mutating = !matches!(&job.op, Op::Get(_) | Op::Search(_));
                let suppressed = !clear
                    && (job.epoch != epoch.load(Ordering::Acquire)
                        || blocked.load(Ordering::Acquire));
                let result = if suppressed {
                    Err("Metadata cache work was cancelled.")
                } else {
                    if clear && store.is_none() {
                        match paths
                            .as_ref()
                            .map_err(|error| *error)
                            .and_then(|paths| Store::open(paths, true))
                        {
                            Ok(opened) => store = Some(opened),
                            Err(error) => warning = Some(error),
                        }
                    }
                    match store.as_mut() {
                        None => Err(warning.unwrap_or("Metadata cache unavailable.")),
                        Some(store) => match job.op {
                            Op::Get(query) => store.get(&query).map(Reply::Hit),
                            Op::Put(query, page) => store.put(&query, &page).map(|_| Reply::Done),
                            Op::Search(query) => store.search(&query).map(Reply::Search),
                            Op::Purge(source) => store.purge_source(source).map(|_| Reply::Done),
                            Op::Clear => store.clear().map(|_| Reply::Done),
                            Op::Budget(mib) => store.budget(mib).map(|_| Reply::Done),
                            Op::Stop => unreachable!("handled above"),
                        },
                    }
                };
                if clear && result.is_ok() && job.epoch == epoch.load(Ordering::Acquire) {
                    blocked.store(false, Ordering::Release);
                }
                if !suppressed && job.epoch == epoch.load(Ordering::Acquire) {
                    if let Err(error) = &result {
                        warning = Some(*error);
                    } else if mutating {
                        warning = None;
                    }
                    update_view(
                        &view,
                        store.as_ref(),
                        blocked.load(Ordering::Acquire),
                        warning,
                    );
                }
                let _ = job.reply.send(result);
            }
        });
        let _ = tokio::time::timeout(Duration::from_secs(3), wait).await;
        this
    }
    pub fn view(&self) -> View {
        self.view.lock().unwrap().clone()
    }
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    async fn request(&self, epoch: u64, op: Op) -> Result<Reply> {
        let Some(tx) = &self.tx else {
            return Ok(Reply::Done);
        };
        let (reply, rx) = oneshot::channel();
        let wait = matches!(&op, Op::Clear | Op::Purge(_) | Op::Stop);
        let job = Job { epoch, op, reply };
        if wait {
            tokio::time::timeout(Duration::from_secs(2), tx.send(job))
                .await
                .map_err(|_| "Metadata cache operation is pending; retry.")?
                .map_err(|_| "Metadata cache worker stopped.")?;
        } else {
            tx.try_send(job)
                .map_err(|_| "Metadata cache is busy; using provider data.")?;
        }
        tokio::time::timeout(Duration::from_secs(3), rx)
            .await
            .map_err(|_| "Metadata cache operation is still pending.")?
            .map_err(|_| "Metadata cache worker stopped.")?
    }
    fn advance(&self) -> Option<u64> {
        self.epoch
            .try_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                old.checked_add(1)
            })
            .ok()
            .map(|old| old + 1)
    }
    pub fn begin_clear(&self) -> Option<u64> {
        self.tx.as_ref()?;
        self.blocked.store(true, Ordering::Release);
        let next = self.advance();
        let mut view = self.view.lock().unwrap();
        view.cleanup_pending = true;
        view.state = "cleanup_pending";
        view.message = "Metadata cleanup pending; cache reads and writes are disabled.".into();
        next
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
    pub async fn budget(&self, mib: u16) -> Result<()> {
        self.request(self.epoch(), Op::Budget(mib))
            .await
            .map(|_| ())
    }
    pub async fn shutdown(&self) {
        let _ = self.request(self.epoch(), Op::Stop).await;
    }
    async fn get(&self, epoch: u64, query: &Query) -> Option<Hit> {
        match self.request(epoch, Op::Get(query.clone())).await {
            Ok(Reply::Hit(hit)) => hit,
            _ => None,
        }
    }
    async fn put(&self, epoch: u64, query: &Query, page: &Page) {
        let _ = self
            .request(epoch, Op::Put(query.clone(), Box::new(page.clone())))
            .await;
    }
    async fn search(&self, epoch: u64, query: &Query) -> Option<Page> {
        match self.request(epoch, Op::Search(query.clone())).await {
            Ok(Reply::Search(page)) => page,
            _ => None,
        }
    }
    async fn purge_source(&self, source: SourceId) {
        if let Some(epoch) = self.advance() {
            let _ = self.request(epoch, Op::Purge(source)).await;
        }
    }
    pub async fn read<F, Fut>(&self, provider: Arc<dyn MusicProvider>, query: Query, mut emit: F)
    where
        F: FnMut(std::result::Result<Page, ProviderError>, Info) -> Fut,
        Fut: Future<Output = ()>,
    {
        if !provider
            .libraries()
            .iter()
            .any(|library| library.id == query.source())
        {
            emit(Err(ProviderError::UnknownSource), Info::default()).await;
            return;
        }
        let detail = matches!(&query, Query::AlbumDetail { .. });
        let provenance = |stale, refreshing| {
            if detail {
                Info::album_detail(stale, refreshing)
            } else {
                Info::cached(stale, refreshing)
            }
        };
        let epoch = self.epoch();
        let cached = self.get(epoch, &query).await;
        let cached_search = if cached.is_none() {
            self.search(epoch, &query).await
        } else {
            None
        };
        if let Some(hit) = &cached {
            let age = now().checked_sub(hit.fetched);
            let fresh = hit.fetched > 0 && age.is_some_and(|age| (0..900).contains(&age));
            emit(Ok(hit.page.clone()), provenance(!fresh, !fresh || detail)).await;
            if fresh && !detail {
                return;
            }
        } else if let Some(page) = &cached_search {
            emit(Ok(page.clone()), Info::search(true)).await;
        }
        let timeout = Duration::from_secs(if detail { 45 } else { 30 });
        let result = tokio::time::timeout(timeout, query.fetch(provider.as_ref()))
            .await
            .unwrap_or(Err(ProviderError::Network));
        match result {
            Ok(page) if page.metadata_only().valid(&query) => {
                emit(Ok(page.clone()), Info::default()).await;
                self.put(epoch, &query, &page).await;
            }
            Err(error @ (ProviderError::AccessDenied | ProviderError::UnknownSource)) => {
                self.purge_source(query.source()).await;
                emit(Err(error), Info::default()).await;
            }
            error => {
                if let Some(hit) = cached {
                    emit(Ok(hit.page), provenance(true, false)).await;
                } else if let Some(page) = cached_search {
                    emit(Ok(page), Info::search(false)).await;
                } else if matches!(
                    error,
                    Err(ProviderError::Network | ProviderError::SignInRequired)
                ) {
                    if let Some(page) = self.search(epoch, &query).await {
                        emit(Ok(page), Info::search(false)).await;
                    } else {
                        emit(
                            Err(error.err().unwrap_or(ProviderError::InvalidReply)),
                            Info::default(),
                        )
                        .await;
                    }
                } else {
                    emit(
                        Err(error.err().unwrap_or(ProviderError::InvalidReply)),
                        Info::default(),
                    )
                    .await;
                }
            }
        }
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
        view.bytes = store.stats.bytes.max(0) as u64;
        view.pages = store.stats.pages.max(0) as u64;
        view.tracks = store.stats.tracks.max(0) as u64;
        view.budget_mib = store.prefs.budget_mib;
        view.rebuilt = store.rebuilt;
    }
    view.usage_known = store.is_some() && error.is_none();
    view.cleanup_pending = blocked || store.is_some_and(|s| s.prefs.purge_pending);
    view.state = if view.cleanup_pending {
        "cleanup_pending"
    } else if store.is_none() || error.is_some() {
        "unavailable"
    } else if view.budget_mib == 0 {
        "disabled"
    } else {
        "ready"
    };
    view.message = error
        .unwrap_or(if view.cleanup_pending {
            "Metadata cleanup incomplete. Fix storage access and retry."
        } else if view.rebuilt {
            "Metadata cache rebuilt; coverage is limited to newly accessed pages."
        } else if view.budget_mib == 0 {
            "Disk metadata cache disabled."
        } else {
            "Metadata cache ready; cached catalog coverage is incomplete."
        })
        .into();
}

#[cfg(test)]
mod tests {
    use super::*;
    use qoda_core::{ProviderTrackKey, TrackRef};
    use qoda_provider::{Library, PreparedMedia, ProviderFuture};
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        sync::atomic::AtomicUsize,
    };
    struct Fixture {
        root: PathBuf,
        paths: Paths,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("qoda-metadata-{}", uuid::Uuid::new_v4()));
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
    fn query(source: SourceId, offset: u64, text: &str) -> Query {
        Query::Tracks {
            source,
            offset,
            query: text.into(),
        }
    }
    fn page(request: &Query, keys: &[&str]) -> Page {
        Page::Tracks(CatalogPage {
            source: request.source(),
            offset: Counter::new(request.offset()),
            query: request.text().into(),
            total: None,
            has_more: false,
            tracks: keys
                .iter()
                .map(|key| Track {
                    reference: TrackRef {
                        source: request.source(),
                        key: (*key).to_owned().try_into().unwrap(),
                    },
                    title: format!("Track {key}"),
                    artist: "Ärtist".into(),
                    album: "Fixture album".into(),
                    duration_ms: Some(Counter::new(1000)),
                })
                .collect(),
        })
    }
    #[test]
    fn source_request_and_large_offset_identity_survive_restart_without_extending_freshness() {
        let fixture = Fixture::new();
        let source = SourceId::new();
        let request = query(source, 9_007_199_254_740_993, "%_雪");
        let value = page(&request, &["A", "B"]);
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.put(&request, &value).unwrap();
        store
            .connection()
            .unwrap()
            .execute("UPDATE pages SET fetched=?1", [now() - 901])
            .unwrap();
        let hit = store.get(&request).unwrap().unwrap();
        assert!(hit.page.valid(&request));
        assert!(now() - hit.fetched >= 901);
        assert!(
            store
                .get(&query(source, 9_007_199_254_740_994, "%_雪"))
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get(&query(SourceId::new(), request.offset(), request.text()))
                .unwrap()
                .is_none()
        );
        drop(store);
        let mut reopened = Store::open(&fixture.paths, false).unwrap();
        assert!(now() - reopened.get(&request).unwrap().unwrap().fetched >= 901);
    }
    #[test]
    fn cached_search_is_literal_unicode_lowercase_source_scoped_and_incomplete() {
        let fixture = Fixture::new();
        let source = SourceId::new();
        let request = query(source, 0, "");
        let mut value = page(&request, &["A", "B"]);
        if let Page::Tracks(page) = &mut value {
            page.tracks[0].title = "100%_ literal".into();
        }
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.put(&request, &value).unwrap();
        let found = store.search(&query(source, 0, "%_")).unwrap().unwrap();
        let Page::Tracks(found) = found else {
            panic!("wrong page")
        };
        assert_eq!(found.tracks.len(), 1);
        assert!(found.total.is_none());
        assert!(!found.has_more);
        let found = store.search(&query(source, 0, "ÄRTIST")).unwrap().unwrap();
        let Page::Tracks(found) = found else {
            panic!("wrong page")
        };
        assert_eq!(found.tracks.len(), 2);
        assert!(
            store
                .search(&query(SourceId::new(), 0, "literal"))
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .search(&query(source, 0, "' OR 1=1 --"))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn lru_eviction_reclaims_payloads_and_preserves_target_across_clearing() {
        let fixture = Fixture::new();
        let source = SourceId::new();
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.budget(2).unwrap();
        let mut requests = Vec::new();
        for offset in [0, 100] {
            let request = query(source, offset, "");
            let keys = (offset..offset + 100)
                .map(|i| i.to_string())
                .collect::<Vec<_>>();
            let refs = keys.iter().map(String::as_str).collect::<Vec<_>>();
            let mut value = page(&request, &refs);
            if let Page::Tracks(page) = &mut value {
                for track in &mut page.tracks {
                    track.title = "t".repeat(1024);
                    track.artist = "a".repeat(1024);
                    track.album = "b".repeat(1024);
                }
            }
            store.put(&request, &value).unwrap();
            requests.push(request);
        }
        store.get(&requests[0]).unwrap().unwrap();
        let request = query(source, 200, "");
        let keys = (200..300).map(|i| i.to_string()).collect::<Vec<_>>();
        let refs = keys.iter().map(String::as_str).collect::<Vec<_>>();
        let mut value = page(&request, &refs);
        if let Page::Tracks(page) = &mut value {
            for track in &mut page.tracks {
                track.title = "t".repeat(1024);
                track.artist = "a".repeat(1024);
                track.album = "b".repeat(1024);
            }
        }
        store.put(&request, &value).unwrap();
        assert!(store.stats.bytes <= 2 * 1024 * 1024);
        assert!(store.get(&requests[0]).unwrap().is_some());
        assert!(store.get(&requests[1]).unwrap().is_none());
        assert!(store.get(&request).unwrap().is_some());
        store.clear().unwrap();
        assert_eq!(store.stats.pages, 0);
        assert_eq!(store.stats.tracks, 0);
        drop(store);
        let mut reopened = Store::open(&fixture.paths, false).unwrap();
        assert_eq!(reopened.prefs.budget_mib, 2);
        reopened.budget(0).unwrap();
        reopened.put(&request, &value).unwrap();
        assert!(reopened.get(&request).unwrap().is_none());
    }
    #[test]
    fn album_and_artist_pages_keep_type_and_artist_scope_identity() {
        let fixture = Fixture::new();
        let source = SourceId::new();
        let mut store = Store::open(&fixture.paths, false).unwrap();
        let artist = qoda_provider::Artist {
            reference: ArtistRef {
                source,
                key: "artist".into(),
            },
            title: "Fixture artist".into(),
        };
        let albums = Query::Albums {
            source,
            offset: 0,
            query: "".into(),
            artist: Some(artist.reference.clone()),
        };
        let value = Page::Albums(AlbumPage {
            source,
            offset: Counter::ZERO,
            query: "".into(),
            has_more: false,
            artist: Some(artist.clone()),
            albums: vec![qoda_provider::Album {
                reference: qoda_provider::AlbumRef {
                    source,
                    key: "album".into(),
                },
                artist_ref: Some(artist.reference.clone()),
                title: "Album".into(),
                artist: "Fixture artist".into(),
                year: None,
                track_count: Some(Counter::new(2)),
            }],
        });
        store.put(&albums, &value).unwrap();
        assert!(store.get(&albums).unwrap().is_some());
        assert!(
            store
                .get(&Query::Albums {
                    source,
                    offset: 0,
                    query: "".into(),
                    artist: None
                })
                .unwrap()
                .is_none()
        );
        let other = Query::Albums {
            source,
            offset: 0,
            query: "".into(),
            artist: Some(ArtistRef {
                source,
                key: "other".into(),
            }),
        };
        assert!(store.put(&other, &value).is_err());
        let artists = Query::Artists {
            source,
            offset: 0,
            query: "".into(),
        };
        store
            .put(
                &artists,
                &Page::Artists(ArtistPage {
                    source,
                    offset: Counter::ZERO,
                    query: "".into(),
                    artists: vec![artist],
                    has_more: false,
                }),
            )
            .unwrap();
        assert!(matches!(
            store.get(&artists).unwrap().unwrap().page,
            Page::Artists(_)
        ));
    }
    #[test]
    fn sqlite_operations_refuse_a_moved_database_directory_and_clear_uses_its_opened_identity() {
        let fixture = Fixture::new();
        let mut store = Store::open(&fixture.paths, false).unwrap();
        let request = query(SourceId::new(), 0, "");
        store.put(&request, &page(&request, &["A"])).unwrap();
        let moved = fixture.root.join("original-cache");
        let outside = fixture.root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("sentinel"), b"preserved").unwrap();
        std::fs::rename(&fixture.paths.cache, &moved).unwrap();
        symlink(&outside, &fixture.paths.cache).unwrap();
        assert!(store.get(&request).is_err());
        assert!(store.put(&request, &page(&request, &["B"])).is_err());
        store.clear().unwrap();
        assert_eq!(
            std::fs::read(outside.join("sentinel")).unwrap(),
            b"preserved"
        );
        assert_eq!(std::fs::read_dir(outside).unwrap().count(), 1);
    }
    #[test]
    fn damaged_disposable_database_rebuilds_but_foreign_and_newer_databases_are_preserved() {
        let fixture = Fixture::new();
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.budget(3).unwrap();
        drop(store);
        let path = fixture.paths.cache.join(DB_FILES[0]);
        std::fs::write(&path, b"damaged disposable cache").unwrap();
        let store = Store::open(&fixture.paths, false).unwrap();
        assert!(store.rebuilt);
        assert_eq!(store.prefs.budget_mib, 3);
        store
            .connection()
            .unwrap()
            .pragma_update(None, "user_version", 2)
            .unwrap();
        drop(store);
        let before = std::fs::read(&path).unwrap();
        assert!(Store::open(&fixture.paths, false).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let store = Store::open(&fixture.paths, true).unwrap();
        store
            .connection()
            .unwrap()
            .pragma_update(None, "application_id", 7)
            .unwrap();
        drop(store);
        let before = std::fs::read(&path).unwrap();
        assert!(Store::open(&fixture.paths, false).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    #[test]
    fn cache_owner_drop_releases_lock_even_when_a_child_retains_the_file_description() {
        let fixture = Fixture::new();
        let store = Store::open(&fixture.paths, false).unwrap();
        // dup shares the open file description, as a child does between fork and exec.
        // Pin that otherwise tiny subprocess-creation window without forking a test process.
        let inherited = store._lock.try_clone().unwrap();
        assert!(Store::open(&fixture.paths, false).is_err());
        drop(store);
        let reopened = Store::open(&fixture.paths, false);
        assert!(
            reopened.is_ok(),
            "A departed owner must explicitly unlock: {:?}",
            reopened.err()
        );
        drop(inherited);
    }
    #[test]
    fn unsafe_database_links_and_interrupted_purge_cannot_touch_the_saved_session() {
        let fixture = Fixture::new();
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.budget(4).unwrap();
        drop(store);
        let saved = fixture.root.join("session.sqlite3");
        std::fs::write(&saved, b"saved listening session").unwrap();
        std::fs::set_permissions(&saved, std::fs::Permissions::from_mode(0o600)).unwrap();
        let path = fixture.paths.cache.join(DB_FILES[0]);
        std::fs::remove_file(&path).unwrap();
        symlink(&saved, &path).unwrap();
        assert!(Store::open(&fixture.paths, false).is_err());
        let mut store = Store::open(&fixture.paths, true).unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), b"saved listening session");
        store.conn.take();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(store.clear().is_err());
        assert!(store.prefs.purge_pending);
        drop(store);
        assert!(Store::open(&fixture.paths, false).is_err());
        std::fs::remove_dir(&path).unwrap();
        let store = Store::open(&fixture.paths, false).unwrap();
        assert!(!store.prefs.purge_pending);
        assert_eq!(store.prefs.budget_mib, 4);
        assert_eq!(std::fs::read(&saved).unwrap(), b"saved listening session");
    }
    #[test]
    fn invalid_cached_payload_and_source_mismatch_are_rejected() {
        let fixture = Fixture::new();
        let source = SourceId::new();
        let request = query(source, 0, "");
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.put(&request, &page(&request, &["A"])).unwrap();
        let other = query(SourceId::new(), 0, "");
        let payload = serde_json::to_vec(&page(&other, &["A"])).unwrap();
        store
            .connection()
            .unwrap()
            .execute("UPDATE pages SET payload=?1", [payload])
            .unwrap();
        assert!(store.get(&request).unwrap().is_none());
        assert!(store.put(&request, &page(&other, &["A"])).is_err());
        assert_eq!(store.stats.pages, 0);
    }
    fn detail(reference: AlbumRef, title: &str) -> AlbumSnapshot {
        use qoda_provider::{Album, AlbumTrack};
        let source = reference.source;
        AlbumSnapshot {
            album: Album {
                reference,
                artist_ref: None,
                title: title.into(),
                artist: "Fixture".into(),
                year: None,
                track_count: Some(Counter::new(2)),
            },
            revision: title.into(),
            tracks: ["1", "2"]
                .into_iter()
                .map(|key| AlbumTrack {
                    track: Track {
                        reference: TrackRef {
                            source,
                            key: key.to_owned().try_into().unwrap(),
                        },
                        title: key.into(),
                        artist: "Fixture".into(),
                        album: title.into(),
                        duration_ms: None,
                    },
                    disc: 1,
                    number: key.parse().unwrap(),
                })
                .collect(),
            artwork: Some("data:image/jpeg;base64,not-persisted".into()),
            artwork_unavailable: false,
        }
    }
    #[test]
    fn album_details_persist_without_cover_bytes_and_share_budget_clear_and_source_isolation() {
        let fixture = Fixture::new();
        let reference = AlbumRef {
            source: SourceId::new(),
            key: "album".into(),
        };
        let request = Query::AlbumDetail {
            reference: reference.clone(),
        };
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store
            .put(
                &request,
                &Page::AlbumDetail(detail(reference, "Cached album")),
            )
            .unwrap();
        assert_eq!(store.stats.pages, 1);
        assert_eq!(store.stats.tracks, 0); // Detail snapshots do not expand cached-search coverage.
        drop(store);
        let mut store = Store::open(&fixture.paths, false).unwrap();
        let Page::AlbumDetail(snapshot) = store.get(&request).unwrap().unwrap().page else {
            panic!("Expected album details")
        };
        assert_eq!(snapshot.tracks.len(), 2);
        assert_eq!(snapshot.revision, "Cached album");
        assert!(snapshot.artwork.is_none() && snapshot.artwork_unavailable);
        assert!(
            !std::fs::read(&store.db_path)
                .unwrap()
                .windows(b"not-persisted".len())
                .any(|window| window == b"not-persisted")
        );
        let other = Query::AlbumDetail {
            reference: AlbumRef {
                source: SourceId::new(),
                key: "album".into(),
            },
        };
        assert!(store.get(&other).unwrap().is_none());
        let other_key = Query::AlbumDetail {
            reference: AlbumRef {
                source: request.source(),
                key: "other".into(),
            },
        };
        assert!(store.get(&other_key).unwrap().is_none());
        store.budget(0).unwrap();
        assert_eq!(store.stats.pages, 0);
        store.budget(1).unwrap();
        store.put(&request, &Page::AlbumDetail(snapshot)).unwrap();
        store.clear().unwrap();
        assert_eq!(store.stats.pages, 0);
        assert_eq!(store.prefs.budget_mib, 1);
    }
    #[test]
    fn cached_album_details_reject_partial_duplicate_wrong_source_unordered_and_cover_payloads() {
        let fixture = Fixture::new();
        let reference = AlbumRef {
            source: SourceId::new(),
            key: "album".into(),
        };
        let request = Query::AlbumDetail {
            reference: reference.clone(),
        };
        let value = Page::AlbumDetail(detail(reference, "Cached")).metadata_only();
        let Page::AlbumDetail(snapshot) = value.clone() else {
            unreachable!()
        };
        assert!(value.valid(&request));
        let mut variants = Vec::new();
        let mut changed = snapshot.clone();
        changed.tracks.pop();
        variants.push(changed);
        let mut changed = snapshot.clone();
        changed.tracks[1].track.reference = changed.tracks[0].track.reference.clone();
        variants.push(changed);
        let mut changed = snapshot.clone();
        changed.tracks[0].track.reference.source = SourceId::new();
        variants.push(changed);
        let mut changed = snapshot.clone();
        changed.tracks.reverse();
        variants.push(changed);
        let mut changed = snapshot.clone();
        changed.album.artist_ref = Some(ArtistRef {
            source: SourceId::new(),
            key: "1".into(),
        });
        variants.push(changed);
        let mut changed = snapshot.clone();
        changed.revision.clear();
        variants.push(changed);
        let mut changed = snapshot.clone();
        changed.album.reference.key = "other".into();
        variants.push(changed);
        let mut changed = snapshot.clone();
        changed.tracks[0].track.title = "x".repeat(1025);
        variants.push(changed);
        let mut changed = snapshot.clone();
        changed.artwork = Some("https://not-a-cache-image".into());
        variants.push(changed);
        let mut store = Store::open(&fixture.paths, false).unwrap();
        for changed in variants {
            let invalid = Page::AlbumDetail(changed);
            assert!(!invalid.valid(&request));
            store.put(&request, &value).unwrap();
            store
                .connection()
                .unwrap()
                .execute(
                    "UPDATE pages SET payload=?1",
                    [serde_json::to_vec(&invalid).unwrap()],
                )
                .unwrap();
            assert!(store.get(&request).unwrap().is_none());
        }
        assert!(!value.valid(&query(request.source(), 0, "")));
    }
    #[tokio::test]
    async fn cached_album_details_always_refresh_and_denial_never_retains_a_cached_snapshot() {
        let fixture = Fixture::new();
        let reference = AlbumRef {
            source: SourceId::new(),
            key: "album".into(),
        };
        let source = reference.source;
        let request = Query::AlbumDetail {
            reference: reference.clone(),
        };
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store
            .put(&request, &Page::AlbumDetail(detail(reference, "Cached")))
            .unwrap();
        drop(store);
        let cache = Cache::start(Ok(fixture.paths.clone())).await;
        for mode in [1, 0, 2] {
            let provider = Arc::new(Provider {
                source,
                calls: AtomicUsize::new(0),
                mode,
            });
            let values = observe(&cache, provider.clone(), request.clone()).await;
            assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
            assert_eq!(values.len(), 2);
            assert_eq!(values[0].1.origin, "cached");
            assert!(values[0].1.refreshing && !values[0].1.incomplete);
            let Page::AlbumDetail(first) = values[0].0.as_ref().unwrap() else {
                panic!("Expected cached album")
            };
            assert!(first.artwork.is_none());
            if mode == 0 {
                assert_eq!(values[1].1.origin, "live");
                let Page::AlbumDetail(live) = values[1].0.as_ref().unwrap() else {
                    panic!("Expected live album")
                };
                assert_eq!(live.album.title, "Live");
                assert!(live.artwork.is_some());
            } else if mode == 1 {
                assert_eq!(values[1].1.origin, "cached");
                assert!(values[1].1.stale && !values[1].1.refreshing);
            } else {
                assert_eq!(
                    values[1].0.as_ref().unwrap_err(),
                    &ProviderError::AccessDenied
                );
                assert_eq!(cache.view().pages, 0);
            }
        }
        cache.shutdown().await;
    }
    struct Provider {
        source: SourceId,
        calls: AtomicUsize,
        mode: u8,
    }
    impl MusicProvider for Provider {
        fn libraries(&self) -> Vec<Library> {
            vec![Library {
                id: self.source,
                title: "Synthetic library".into(),
                server: "Fixture".into(),
            }]
        }
        fn prepare(&self, _: SourceId, _: ProviderTrackKey) -> ProviderFuture<'_, PreparedMedia> {
            Box::pin(async { Err(ProviderError::Unsupported) })
        }
        fn album(&self, reference: AlbumRef) -> ProviderFuture<'_, AlbumSnapshot> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                match self.mode {
                    1 => Err(ProviderError::Network),
                    2 => Err(ProviderError::AccessDenied),
                    3 => std::future::pending().await,
                    _ => Ok(detail(reference, "Live")),
                }
            })
        }
        fn tracks(
            &self,
            source: SourceId,
            offset: u64,
            text: String,
        ) -> ProviderFuture<'_, CatalogPage> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                match self.mode {
                    1 => Err(ProviderError::Network),
                    2 => Err(ProviderError::AccessDenied),
                    3 => std::future::pending().await,
                    _ => {
                        let Page::Tracks(page) = page(&query(source, offset, &text), &["live"])
                        else {
                            unreachable!()
                        };
                        Ok(page)
                    }
                }
            })
        }
    }
    async fn observe(
        cache: &Cache,
        provider: Arc<Provider>,
        request: Query,
    ) -> Vec<(std::result::Result<Page, ProviderError>, Info)> {
        let output = Arc::new(Mutex::new(Vec::new()));
        let result = output.clone();
        cache
            .read(provider, request, move |page, info| {
                let output = output.clone();
                async move {
                    output.lock().unwrap().push((page, info));
                }
            })
            .await;
        result.lock().unwrap().clone()
    }
    #[tokio::test]
    async fn fresh_cached_page_skips_provider_and_stale_page_emits_then_refreshes() {
        let fixture = Fixture::new();
        let source = SourceId::new();
        let request = query(source, 0, "");
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.put(&request, &page(&request, &["cached"])).unwrap();
        drop(store);
        let cache = Cache::start(Ok(fixture.paths.clone())).await;
        let provider = Arc::new(Provider {
            source,
            calls: AtomicUsize::new(0),
            mode: 0,
        });
        let values = observe(&cache, provider.clone(), request.clone()).await;
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].1.origin, "cached");
        assert!(!values[0].1.stale);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        cache.shutdown().await;
        let store = Store::open(&fixture.paths, false).unwrap();
        store
            .connection()
            .unwrap()
            .execute("UPDATE pages SET fetched=?1", [now() - 901])
            .unwrap();
        drop(store);
        let cache = Cache::start(Ok(fixture.paths.clone())).await;
        let values = observe(&cache, provider.clone(), request).await;
        assert_eq!(values.len(), 2);
        assert!(values[0].1.stale && values[0].1.refreshing);
        assert_eq!(values[1].1.origin, "live");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        cache.shutdown().await;
    }
    #[tokio::test]
    async fn source_allowlist_precedes_cache_reads_and_network_failure_can_search_only_cached_tracks()
     {
        let fixture = Fixture::new();
        let source = SourceId::new();
        let request = query(source, 0, "");
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.put(&request, &page(&request, &["needle"])).unwrap();
        drop(store);
        let cache = Cache::start(Ok(fixture.paths.clone())).await;
        let provider = Arc::new(Provider {
            source,
            calls: AtomicUsize::new(0),
            mode: 1,
        });
        let values = observe(&cache, provider.clone(), query(SourceId::new(), 0, "")).await;
        assert_eq!(
            values[0].0.as_ref().unwrap_err(),
            &ProviderError::UnknownSource
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        let values = observe(&cache, provider.clone(), query(source, 0, "needle")).await;
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].1.origin, "cached_search");
        assert!(values[0].1.refreshing);
        assert!(!values[1].1.refreshing);
        assert!(values[1].1.stale && values[1].1.incomplete);
        assert_eq!(cache.view().pages, 1);
        cache.shutdown().await;
    }
    #[tokio::test]
    async fn denied_refresh_removes_source_cache_and_clear_fences_late_writes() {
        let fixture = Fixture::new();
        let source = SourceId::new();
        let request = query(source, 0, "");
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.put(&request, &page(&request, &["old"])).unwrap();
        store
            .connection()
            .unwrap()
            .execute("UPDATE pages SET fetched=?1", [now() - 901])
            .unwrap();
        drop(store);
        let cache = Cache::start(Ok(fixture.paths.clone())).await;
        let provider = Arc::new(Provider {
            source,
            calls: AtomicUsize::new(0),
            mode: 2,
        });
        let values = observe(&cache, provider, request.clone()).await;
        assert!(values[0].0.is_ok());
        assert_eq!(
            values[1].0.as_ref().unwrap_err(),
            &ProviderError::AccessDenied
        );
        assert_eq!(cache.view().pages, 0);
        assert_eq!(cache.view().tracks, 0);
        let old = cache.epoch();
        cache.clear().await;
        cache.put(old, &request, &page(&request, &["late"])).await;
        assert_eq!(cache.view().pages, 0);
        cache.shutdown().await;
    }
    #[tokio::test]
    async fn stale_cached_page_is_delivered_while_provider_is_hung_and_cancelled_work_cannot_repopulate_after_clear()
     {
        let fixture = Fixture::new();
        let source = SourceId::new();
        let request = query(source, 0, "");
        let mut store = Store::open(&fixture.paths, false).unwrap();
        store.put(&request, &page(&request, &["old"])).unwrap();
        store
            .connection()
            .unwrap()
            .execute("UPDATE pages SET fetched=?1", [now() - 901])
            .unwrap();
        drop(store);
        let cache = Cache::start(Ok(fixture.paths.clone())).await;
        let provider = Arc::new(Provider {
            source,
            calls: AtomicUsize::new(0),
            mode: 3,
        });
        let (tx, mut rx) = mpsc::channel(4);
        let reader = cache.clone();
        let task = tokio::spawn(async move {
            reader
                .read(provider, request, move |page, info| {
                    let tx = tx.clone();
                    async move {
                        let _ = tx.send((page, info)).await;
                    }
                })
                .await;
        });
        let value = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(value.0.is_ok() && value.1.refreshing);
        task.abort();
        cache.clear().await;
        assert_eq!(cache.view().pages, 0);
        cache.shutdown().await;
    }
}
