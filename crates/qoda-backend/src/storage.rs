//! Durable local sessions, independent of provider credentials and temporary access.
use crate::{Event, queue::Queue};
use rusqlite::{Connection, MAIN_DB, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::oneshot;
const APP_ID: i32 = 0x514f4441;
const MAX_PAYLOAD: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Unsafe,
    Corrupt,
    Newer,
    Busy,
    Backup,
    ReopenRequired,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self{
  Self::Unavailable=>"The session could not be saved. Check storage access and free space, then Retry save.",
  Self::Unsafe=>"Session storage permissions or file ownership are unsafe. Existing data was preserved.",
  Self::Corrupt=>"The saved session is damaged. Existing data was preserved. Quit Qoda and use --restore-backup to recover a backup.",
  Self::Newer=>"The session uses a newer unsupported schema. Existing data will not be modified; use a compatible Qoda version.",
  Self::Busy=>"Session storage is busy. This change is not yet saved; choose Retry save.",
  Self::Backup=>"The session was saved, but its backup could not be updated. Choose Retry save.",Self::ReopenRequired=>"Session storage could not be opened. Existing data was preserved. Fix the problem and restart Qoda; new in-memory changes are not saved.",
 })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: u8,
    queue: Queue,
}
fn decode(value: &str) -> Result<Queue, Error> {
    #[derive(Deserialize)]
    struct Version {
        format: u8,
    }
    match serde_json::from_str::<Version>(value)
        .map_err(|_| Error::Corrupt)?
        .format
    {
        1 => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Legacy {
                format: u8,
                queue: serde_json::Value,
            }
            let legacy = serde_json::from_str::<Legacy>(value).map_err(|_| Error::Corrupt)?;
            if legacy.format != 1 {
                return Err(Error::Corrupt);
            }
            Queue::from_legacy(legacy.queue).map_err(|_| Error::Corrupt)
        }
        2 | 3 => serde_json::from_str::<Document>(value)
            .map(|d| d.queue)
            .map_err(|_| Error::Corrupt),
        _ => Err(Error::Newer),
    }
}

pub fn path() -> Result<PathBuf, Error> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .map(|p| p.join("qoda/session.sqlite3"))
        .ok_or(Error::Unavailable)
}
fn private_dir(path: &Path) -> Result<(), Error> {
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(|_| Error::Unavailable)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Unavailable)?;
    }
    let m = std::fs::symlink_metadata(path).map_err(|_| Error::Unavailable)?;
    if !m.is_dir() || m.uid() != unsafe { libc::getuid() } || m.permissions().mode() & 0o077 != 0 {
        return Err(Error::Unsafe);
    }
    Ok(())
}
fn private_file(path: &Path, create: bool) -> Result<(), Error> {
    if create {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(Error::Unavailable),
        }
    }
    let m = std::fs::symlink_metadata(path).map_err(|_| Error::Unavailable)?;
    if !m.is_file() || m.uid() != unsafe { libc::getuid() } || m.permissions().mode() & 0o077 != 0 {
        return Err(Error::Unsafe);
    }
    Ok(())
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}
fn sync_dir(path: &Path) -> Result<(), Error> {
    std::fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Unavailable)
}
fn connection(path: &Path, read_only: bool) -> Result<Connection, Error> {
    private_file(path, !read_only)?;
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_NOFOLLOW
            | if read_only {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            } else {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            },
    )
    .map_err(|_| Error::Corrupt)
}
fn schema(conn: &Connection) -> Result<i64, Error> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|_| Error::Corrupt)?;
    if version > 3 {
        return Err(Error::Newer);
    }
    let id: i32 = conn
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(|_| Error::Corrupt)?;
    if version >= 1 && id != APP_ID || version == 0 && id != 0 {
        return Err(Error::Corrupt);
    }
    let check: String = conn
        .query_row("PRAGMA quick_check(1)", [], |r| r.get(0))
        .map_err(|_| Error::Corrupt)?;
    if check != "ok" {
        return Err(Error::Corrupt);
    }
    Ok(version)
}
fn load(conn: &Connection) -> Result<Queue, Error> {
    let size: Option<i64> = conn
        .query_row(
            "SELECT length(payload) FROM session WHERE slot=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| Error::Corrupt)?;
    match size {
        None => Ok(Queue::default()),
        Some(size) if size < 0 || size as usize > MAX_PAYLOAD => Err(Error::Corrupt),
        Some(_) => {
            let value: String = conn
                .query_row("SELECT payload FROM session WHERE slot=1", [], |r| r.get(0))
                .map_err(|_| Error::Corrupt)?;
            decode(&value)
        }
    }
}
struct Store {
    conn: Connection,
    path: PathBuf,
}
impl Store {
    fn open(path: &Path) -> Result<(Self, Queue), Error> {
        private_dir(path.parent().ok_or(Error::Unsafe)?)?;
        if path.parent().unwrap().join(".restore-in-progress").exists() {
            return Err(Error::Corrupt);
        }
        if !path.exists()
            && std::fs::read_dir(path.parent().unwrap())
                .map_err(|_| Error::Unavailable)?
                .any(|entry| {
                    entry.is_ok_and(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with("session-preserved-")
                    })
                })
        {
            return Err(Error::Corrupt);
        }
        let mut conn = connection(path, false)?;
        conn.busy_timeout(Duration::from_millis(250))
            .map_err(|_| Error::Unavailable)?;
        let version = schema(&conn)?;
        if version == 0 {
            let tables: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|_| Error::Corrupt)?;
            if tables != 0 {
                return Err(Error::Corrupt);
            }
            let tx = conn.transaction().map_err(|_| Error::Unavailable)?;
            tx.execute_batch("CREATE TABLE session(slot INTEGER PRIMARY KEY CHECK(slot=1),payload TEXT NOT NULL);").map_err(|_|Error::Unavailable)?;
            tx.pragma_update(None, "application_id", APP_ID)
                .map_err(|_| Error::Unavailable)?;
            tx.pragma_update(None, "user_version", 3)
                .map_err(|_| Error::Unavailable)?;
            tx.commit().map_err(|_| Error::Unavailable)?;
        }
        let queue = load(&conn)?;
        if (1..3).contains(&version) {
            // Copy a consistent pre-migration database before the transaction.
            let legacy_store = Self {
                conn,
                path: path.to_owned(),
            };
            legacy_store.backup(true)?;
            conn = legacy_store.conn;
            let payload = serde_json::to_string(&Document {
                format: 3,
                queue: queue.clone(),
            })
            .map_err(|_| Error::Corrupt)?;
            let tx = conn.transaction().map_err(|_| Error::Unavailable)?;
            tx.execute("INSERT INTO session(slot,payload) VALUES(1,?1) ON CONFLICT(slot) DO UPDATE SET payload=excluded.payload",[payload]).map_err(|_|Error::Unavailable)?;
            tx.pragma_update(None, "user_version", 3)
                .map_err(|_| Error::Unavailable)?;
            tx.commit().map_err(|_| Error::Unavailable)?;
        }
        conn.pragma_update(None, "journal_mode", "DELETE")
            .map_err(|_| Error::Unavailable)?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|_| Error::Unavailable)?;
        conn.pragma_update(None, "secure_delete", "ON")
            .map_err(|_| Error::Unavailable)?;
        Ok((
            Self {
                conn,
                path: path.to_owned(),
            },
            queue,
        ))
    }
    fn save(&mut self, queue: &Queue) -> Result<Option<Error>, Error> {
        let payload = serde_json::to_string(&Document {
            format: 3,
            queue: queue.clone(),
        })
        .map_err(|_| Error::Corrupt)?;
        if payload.len() > MAX_PAYLOAD {
            return Err(Error::Corrupt);
        }
        let tx = self.conn.transaction().map_err(|_| Error::Unavailable)?;
        tx.execute("INSERT INTO session(slot,payload) VALUES(1,?1) ON CONFLICT(slot) DO UPDATE SET payload=excluded.payload",[payload]).map_err(|_|Error::Unavailable)?;
        tx.commit().map_err(|_| Error::Unavailable)?;
        if queue.snapshot().membership_count > 0 {
            Ok(self.backup_daily().err().map(|_| Error::Backup))
        } else {
            Ok(None)
        }
    }
    fn backups(&self) -> PathBuf {
        self.path.parent().unwrap().join("backups")
    }
    fn backup_daily(&self) -> Result<(), Error> {
        self.backup(false)
    }
    fn backup(&self, migration: bool) -> Result<(), Error> {
        let dir = self.backups();
        private_dir(&dir)?;
        let day = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Backup)?
            .as_secs()
            / 86400;
        let prefix = format!("session-{day:010}-");
        let mut backups = backup_files(&dir)?;
        if migration
            || !backups.iter().any(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&prefix)
            })
        {
            let target = dir.join(format!("{prefix}{}.sqlite3", uuid::Uuid::new_v4()));
            let temp = dir.join(format!(".backup-{}.sqlite3", uuid::Uuid::new_v4()));
            private_file(&temp, true)?;
            let _temporary = Temporary(temp.clone());
            let result = self
                .conn
                .backup(MAIN_DB, &temp, rusqlite::backup::NO_PROGRESS)
                .map_err(|_| Error::Backup)
                .and_then(|_| {
                    std::fs::File::open(&temp)
                        .and_then(|f| f.sync_all())
                        .map_err(|_| Error::Backup)?;
                    std::fs::rename(&temp, &target).map_err(|_| Error::Backup)
                });
            if result.is_err() {
                let _ = std::fs::remove_file(&temp);
            }
            result?;
            backups = backup_files(&dir)?;
        }
        while backups.len() > 3 {
            std::fs::remove_file(backups.remove(0)).map_err(|_| Error::Backup)?;
        }
        sync_dir(&dir)?;
        Ok(())
    }
    fn purge(&mut self) -> Result<(), Error> {
        self.conn
            .execute("DELETE FROM session", [])
            .map_err(|_| Error::Unavailable)?;
        self.conn
            .execute_batch("VACUUM")
            .map_err(|_| Error::Unavailable)?;
        let dir = self.backups();
        if dir.exists() {
            private_dir(&dir)?;
            for entry in std::fs::read_dir(&dir).map_err(|_| Error::Unavailable)? {
                let entry = entry.map_err(|_| Error::Unavailable)?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("session-") || name.starts_with(".backup-") {
                    private_file(&entry.path(), false)?;
                    std::fs::remove_file(entry.path()).map_err(|_| Error::Unavailable)?;
                }
            }
            sync_dir(&dir)?;
        }
        for item in
            std::fs::read_dir(self.path.parent().unwrap()).map_err(|_| Error::Unavailable)?
        {
            let item = item.map_err(|_| Error::Unavailable)?;
            if item
                .file_name()
                .to_string_lossy()
                .starts_with("session-preserved-")
                || item.file_name().to_string_lossy().starts_with(".restore-")
            {
                private_file(&item.path(), false)?;
                std::fs::remove_file(item.path()).map_err(|_| Error::Unavailable)?;
            }
        }
        sync_dir(self.path.parent().unwrap())
    }
}
fn backup_files(dir: &Path) -> Result<Vec<PathBuf>, Error> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    private_dir(dir)?;
    let mut files = Vec::new();
    for item in std::fs::read_dir(dir).map_err(|_| Error::Unavailable)? {
        let item = item.map_err(|_| Error::Unavailable)?;
        let name = item.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("session-") && name.ends_with(".sqlite3") {
            private_file(&item.path(), false)?;
            files.push(item.path());
        }
    }
    files.sort_by_key(|path| {
        let day = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .chars()
            .skip(8)
            .take(10)
            .collect::<String>();
        let modified = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        (day, modified, path.clone())
    });
    Ok(files)
}
pub fn restore_latest(path: &Path) -> Result<(), Error> {
    let parent = path.parent().ok_or(Error::Unsafe)?;
    private_dir(parent)?;
    if path.exists()
        && let Ok(conn) = connection(path, true)
        && (schema(&conn) == Err(Error::Newer) || matches!(load(&conn), Err(Error::Newer)))
    {
        return Err(Error::Newer);
    }
    let backup = backup_files(&parent.join("backups"))?
        .pop()
        .ok_or(Error::Backup)?;
    let source = connection(&backup, true)?;
    if !matches!(schema(&source)?, 1..=3) {
        return Err(Error::Corrupt);
    }
    load(&source)?;
    let temp = parent.join(format!(".restore-{}.sqlite3", uuid::Uuid::new_v4()));
    private_file(&temp, true)?;
    let _temporary = Temporary(temp.clone());
    source
        .backup(MAIN_DB, &temp, rusqlite::backup::NO_PROGRESS)
        .map_err(|_| Error::Backup)?;
    let restored = connection(&temp, true)?;
    load(&restored)?;
    drop(restored);
    std::fs::File::open(&temp)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Unavailable)?;
    let marker = parent.join(".restore-in-progress");
    private_file(&marker, true)?;
    std::fs::File::open(&marker)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Unavailable)?;
    sync_dir(parent)?;
    let preserved = parent.join(format!(
        "session-preserved-{}.sqlite3",
        uuid::Uuid::new_v4()
    ));
    let mut moved = Vec::new();
    let result = (|| {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let old = PathBuf::from(format!("{}{suffix}", path.display()));
            if old.exists() {
                private_file(&old, false)?;
                let saved = PathBuf::from(format!("{}{suffix}", preserved.display()));
                std::fs::rename(&old, &saved).map_err(|_| Error::Unavailable)?;
                moved.push((old, saved));
            }
        }
        sync_dir(parent)?;
        std::fs::rename(&temp, path).map_err(|_| Error::Unavailable)?;
        sync_dir(parent)?;
        Ok(())
    })();
    if result.is_err() {
        // Only roll back before replacement; otherwise retain the recovery marker.
        if !path.exists() {
            let mut ok = true;
            for (old, saved) in moved.iter().rev() {
                ok &= std::fs::rename(saved, old).is_ok();
            }
            if ok {
                let _ = std::fs::remove_file(&marker);
            }
        }
        return result;
    }
    std::fs::remove_file(marker).map_err(|_| Error::Unavailable)?;
    sync_dir(parent)
}

#[derive(Clone)]
pub struct Reply {
    pub serial: u64,
    pub result: Result<Option<Error>, Error>,
}
struct Request {
    serial: u64,
    queue: Queue,
    purge_epoch: u64,
    reply: Option<oneshot::Sender<Result<Option<Error>, Error>>>,
}
pub struct Controller {
    tx: mpsc::SyncSender<Request>,
    serial: u64,
}
impl Controller {
    pub async fn start(
        path: Result<PathBuf, Error>,
        events: tokio::sync::mpsc::Sender<Event>,
    ) -> (Self, Result<Queue, Error>) {
        let (tx, rx) = mpsc::sync_channel::<Request>(8);
        let (ready, initial) = oneshot::channel();
        std::thread::spawn(move || {
            let opened = path
                .as_ref()
                .map_err(|e| *e)
                .and_then(|path| Store::open(path));
            let opening_error = opened.as_ref().err().copied();
            let (mut store, queue) = match opened {
                Ok((s, q)) => (Some(s), Ok(q)),
                Err(e) => (None, Err(e)),
            };
            let queue = queue.map_err(|error| match error {
                Error::Corrupt | Error::Newer | Error::Unsafe => error,
                _ => Error::ReopenRequired,
            });
            let _ = ready.send(queue);
            let mut purged = 0;
            while let Ok(first) = rx.recv() {
                let mut batch = vec![first];
                for _ in 1..8 {
                    match rx.try_recv() {
                        Ok(next) => batch.push(next),
                        Err(_) => break,
                    }
                }
                let latest = batch.last().unwrap();
                let serial = latest.serial;
                let result = (|| {
                    if store.is_none() {
                        return Err(match opening_error {
                            Some(Error::Corrupt) => Error::Corrupt,
                            Some(Error::Newer) => Error::Newer,
                            Some(Error::Unsafe) => Error::Unsafe,
                            _ => Error::ReopenRequired,
                        });
                    }
                    let store = store.as_mut().unwrap();
                    if latest.purge_epoch > purged {
                        store.purge()?;
                        purged = latest.purge_epoch;
                    }
                    store.save(&latest.queue)
                })();
                for request in batch {
                    if let Some(reply) = request.reply {
                        let _ = reply.send(result);
                    }
                }
                let _ = events.try_send(Event::Stored(Reply { serial, result }));
            }
        });
        (
            Self { tx, serial: 0 },
            initial.await.unwrap_or(Err(Error::Unavailable)),
        )
    }
    pub fn serial(&self) -> u64 {
        self.serial
    }
    pub fn submit(&mut self, queue: Queue, purge_epoch: u64) -> Result<(), Error> {
        self.serial = self.serial.checked_add(1).ok_or(Error::Busy)?;
        self.tx
            .try_send(Request {
                serial: self.serial,
                queue,
                purge_epoch,
                reply: None,
            })
            .map_err(|_| Error::Busy)
    }
    pub async fn flush(&mut self, queue: Queue, purge_epoch: u64) -> Result<Option<Error>, Error> {
        self.serial = self.serial.checked_add(1).ok_or(Error::Busy)?;
        let (reply, done) = oneshot::channel();
        let tx = self.tx.clone();
        let serial = self.serial;
        tokio::task::spawn_blocking(move || {
            tx.send(Request {
                serial,
                queue,
                purge_epoch,
                reply: Some(reply),
            })
            .map_err(|_| Error::Unavailable)
        })
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_| Error::Unavailable)?;
        tokio::time::timeout(Duration::from_secs(5), done)
            .await
            .map_err(|_| Error::Busy)?
            .map_err(|_| Error::Unavailable)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qoda_core::{SourceId, TrackRef};
    fn queued(name: &str) -> Queue {
        let mut queue = Queue::default();
        queue
            .enqueue(
                qoda_provider::Track {
                    reference: TrackRef {
                        source: SourceId::new(),
                        key: name.to_owned().try_into().unwrap(),
                    },
                    title: name.into(),
                    artist: "Fixture".into(),
                    album: "Fixture".into(),
                    duration_ms: None,
                },
                false,
            )
            .unwrap();
        queue
    }
    struct Fixture {
        dir: PathBuf,
        path: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("qoda-storage-test-{}", uuid::Uuid::new_v4()));
            private_dir(&dir).unwrap();
            let path = dir.join("session.sqlite3");
            Self { dir, path }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    fn legacy_payload(queue: &Queue) -> String {
        let mut value = serde_json::to_value(queue).unwrap();
        let fields = value.as_object_mut().unwrap();
        for key in [
            "order",
            "shuffled",
            "repeat",
            "route",
            "cursor",
            "navigating",
            "recorded",
            "navigation_dropped",
            "unavailable",
        ] {
            fields.remove(key);
        }
        serde_json::json!({"format":1,"queue":value}).to_string()
    }
    #[tokio::test]
    async fn a_failed_initial_load_cannot_later_overwrite_recovered_data_with_empty_state() {
        let f = Fixture::new();
        private_file(&f.path, true).unwrap();
        std::fs::write(&f.path, "corrupt fixture").unwrap();
        let (events, _replies) = tokio::sync::mpsc::channel(8);
        let (mut controller, ready) = Controller::start(Ok(f.path.clone()), events).await;
        assert!(ready.is_err());
        std::fs::remove_file(&f.path).unwrap();
        let (mut repaired, _) = Store::open(&f.path).unwrap();
        repaired.save(&queued("recovered")).unwrap();
        drop(repaired);
        assert!(controller.flush(Queue::default(), 0).await.is_err());
        let (_, saved) = Store::open(&f.path).unwrap();
        assert_eq!(saved.snapshot().entries[0].track.title, "recovered");
    }
    #[test]
    fn legacy_migration_backs_up_then_preserves_queue_and_position() {
        let f = Fixture::new();
        let (store, _) = Store::open(&f.path).unwrap();
        let mut queue = queued("legacy");
        queue.advance().unwrap();
        queue.set_position(7.5).unwrap();
        let session = queue.snapshot().session;
        let payload = legacy_payload(&queue);
        store
            .conn
            .execute("INSERT INTO session VALUES(1,?1)", [payload.clone()])
            .unwrap();
        store.conn.pragma_update(None, "user_version", 1).unwrap();
        drop(store);
        let (store, restored) = Store::open(&f.path).unwrap();
        assert_eq!(restored.snapshot().session, session);
        assert_eq!(restored.position(), 7.5);
        assert!(!restored.snapshot().shuffled);
        assert_eq!(restored.snapshot().repeat, crate::queue::Repeat::Off);
        assert_eq!(
            store
                .conn
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        let backup = backup_files(&store.backups()).unwrap().pop().unwrap();
        let old = connection(&backup, true).unwrap();
        assert_eq!(schema(&old).unwrap(), 1);
        assert_eq!(
            old.query_row("SELECT payload FROM session", [], |r| r.get::<_, String>(0))
                .unwrap(),
            payload
        );
    }
    #[test]
    fn version_two_migrates_with_backup_and_failure_information_roundtrips() {
        let f = Fixture::new();
        let (store, _) = Store::open(&f.path).unwrap();
        let mut q = queued("migration");
        let id = q.snapshot().entries[0].id;
        q.select(id).unwrap();
        let mut old = serde_json::to_value(&q).unwrap();
        old.as_object_mut().unwrap().remove("unavailable");
        let payload = serde_json::json!({"format":2,"queue":old}).to_string();
        store
            .conn
            .execute("INSERT INTO session VALUES(1,?1)", [payload.clone()])
            .unwrap();
        store.conn.pragma_update(None, "user_version", 2).unwrap();
        drop(store);
        let (mut store, restored) = Store::open(&f.path).unwrap();
        assert_eq!(schema(&store.conn).unwrap(), 3);
        assert!(restored.snapshot().unavailable_entries.is_empty());
        let backup = backup_files(&store.backups()).unwrap().pop().unwrap();
        let old = connection(&backup, true).unwrap();
        assert_eq!(schema(&old).unwrap(), 2);
        assert_eq!(
            old.query_row("SELECT payload FROM session", [], |r| r.get::<_, String>(0))
                .unwrap(),
            payload
        );
        q.mark_unavailable(id, crate::queue::Unavailable::Missing)
            .unwrap();
        store.save(&q).unwrap();
        drop(store);
        let (_, restored) = Store::open(&f.path).unwrap();
        assert_eq!(restored.snapshot().unavailable_entries[0].id, id);
    }
    #[test]
    fn failed_migration_rolls_back_without_erasing_legacy_data() {
        let f = Fixture::new();
        let (store, _) = Store::open(&f.path).unwrap();
        let payload = legacy_payload(&queued("legacy"));
        store
            .conn
            .execute("INSERT INTO session VALUES(1,?1)", [payload.clone()])
            .unwrap();
        store.conn.pragma_update(None, "user_version", 1).unwrap();
        store.conn.execute_batch("CREATE TRIGGER reject_migration BEFORE UPDATE ON session BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        drop(store);
        assert!(Store::open(&f.path).is_err());
        let conn = connection(&f.path, true).unwrap();
        assert_eq!(schema(&conn).unwrap(), 1);
        assert_eq!(
            conn.query_row("SELECT payload FROM session", [], |r| r.get::<_, String>(0))
                .unwrap(),
            payload
        );
        assert!(!backup_files(&f.dir.join("backups")).unwrap().is_empty());
    }
    #[test]
    fn daily_backup_retention_is_bounded_and_purge_covers_partial_artifacts() {
        let f = Fixture::new();
        let (mut store, _) = Store::open(&f.path).unwrap();
        store.save(&queued("fixture")).unwrap();
        for day in 1..=4 {
            let path = store
                .backups()
                .join(format!("session-{day:010}-fixture.sqlite3"));
            private_file(&path, true).unwrap();
            store
                .conn
                .backup(MAIN_DB, &path, rusqlite::backup::NO_PROGRESS)
                .unwrap();
        }
        store.backup_daily().unwrap();
        assert_eq!(backup_files(&store.backups()).unwrap().len(), 3);
        for path in [
            store.backups().join(".backup-partial.sqlite3"),
            f.dir.join(".restore-partial.sqlite3"),
        ] {
            private_file(&path, true).unwrap();
            std::fs::write(path, "old account fixture").unwrap();
        }
        store.purge().unwrap();
        assert!(backup_files(&store.backups()).unwrap().is_empty());
        assert!(!f.dir.join(".restore-partial.sqlite3").exists());
        assert!(!store.backups().join(".backup-partial.sqlite3").exists());
    }
    #[test]
    fn interrupted_recovery_is_not_silently_initialized_as_an_empty_session() {
        let f = Fixture::new();
        let (mut store, _) = Store::open(&f.path).unwrap();
        store.save(&queued("fixture")).unwrap();
        drop(store);
        private_file(&f.dir.join(".restore-in-progress"), true).unwrap();
        std::fs::rename(&f.path, f.dir.join("session-preserved-fixture.sqlite3")).unwrap();
        assert!(matches!(Store::open(&f.path), Err(Error::Corrupt)));
        assert!(!f.path.exists());
        restore_latest(&f.path).unwrap();
        assert_eq!(
            Store::open(&f.path).unwrap().1.snapshot().membership_count,
            1
        );
        assert!(!f.dir.join(".restore-in-progress").exists());
    }
    #[test]
    fn typed_session_roundtrip_backups_and_private_permissions() {
        let f = Fixture::new();
        let (mut store, _) = Store::open(&f.path).unwrap();
        let mut q = queued("same");
        let track = q.snapshot().entries[0].track.clone();
        q.enqueue(track.clone(), true).unwrap();
        q.advance().unwrap();
        q.update_selected(track, "media:part").unwrap();
        q.set_position(12.25).unwrap();
        let saved = serde_json::to_value(&q).unwrap();
        assert!(store.save(&q).unwrap().is_none());
        assert_eq!(
            std::fs::metadata(&f.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(store);
        let (store, restored) = Store::open(&f.path).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), saved);
        assert_eq!(backup_files(&store.backups()).unwrap().len(), 1);
    }
    #[test]
    fn corrupt_and_newer_data_are_not_replaced() {
        let f = Fixture::new();
        private_file(&f.path, true).unwrap();
        std::fs::write(&f.path, b"broken database fixture").unwrap();
        let before = std::fs::read(&f.path).unwrap();
        assert!(matches!(Store::open(&f.path), Err(Error::Corrupt)));
        assert_eq!(std::fs::read(&f.path).unwrap(), before);
        std::fs::remove_file(&f.path).unwrap();
        let (store, _) = Store::open(&f.path).unwrap();
        store.conn.pragma_update(None, "user_version", 4).unwrap();
        drop(store);
        let before = std::fs::read(&f.path).unwrap();
        assert!(matches!(Store::open(&f.path), Err(Error::Newer)));
        assert!(matches!(restore_latest(&f.path), Err(Error::Newer)));
        assert_eq!(std::fs::read(&f.path).unwrap(), before);
        let future = Fixture::new();
        let (mut store, _) = Store::open(&future.path).unwrap();
        store.save(&queued("future")).unwrap();
        store
            .conn
            .execute("UPDATE session SET payload='{\"format\":4}'", [])
            .unwrap();
        drop(store);
        let before = std::fs::read(&future.path).unwrap();
        assert!(matches!(Store::open(&future.path), Err(Error::Newer)));
        assert!(matches!(restore_latest(&future.path), Err(Error::Newer)));
        assert_eq!(std::fs::read(&future.path).unwrap(), before);
    }
    #[test]
    fn failed_transaction_preserves_previous_checkpoint() {
        let f = Fixture::new();
        let (mut store, _) = Store::open(&f.path).unwrap();
        let original = queued("original");
        store.save(&original).unwrap();
        store.conn.execute_batch("CREATE TRIGGER reject_write BEFORE UPDATE ON session BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(store.save(&queued("replacement")).is_err());
        assert_eq!(
            serde_json::to_value(load(&store.conn).unwrap()).unwrap(),
            serde_json::to_value(original).unwrap()
        );
    }
    #[test]
    fn explicit_backup_recovery_preserves_input_and_purge_removes_recovery_copies() {
        let f = Fixture::new();
        let (mut store, _) = Store::open(&f.path).unwrap();
        let original = queued("original");
        let wanted = serde_json::to_value(&original).unwrap();
        store.save(&original).unwrap();
        drop(store);
        std::fs::write(&f.path, b"damaged durable session").unwrap();
        restore_latest(&f.path).unwrap();
        let (mut store, restored) = Store::open(&f.path).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), wanted);
        assert!(std::fs::read_dir(&f.dir).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("session-preserved-")
        }));
        store.purge().unwrap();
        store.save(&Queue::default()).unwrap();
        assert!(backup_files(&store.backups()).unwrap().is_empty());
        assert!(!std::fs::read_dir(&f.dir).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("session-preserved-")
        }));
        assert_eq!(load(&store.conn).unwrap().snapshot().membership_count, 0);
    }
    #[test]
    fn malformed_session_payload_is_preserved_and_does_not_become_empty_state() {
        let f = Fixture::new();
        let (store, _) = Store::open(&f.path).unwrap();
        store
            .conn
            .execute(
                "INSERT INTO session VALUES(1,'{\"format\":1,\"queue\":{}}')",
                [],
            )
            .unwrap();
        drop(store);
        let before = std::fs::read(&f.path).unwrap();
        assert!(matches!(Store::open(&f.path), Err(Error::Corrupt)));
        assert_eq!(std::fs::read(&f.path).unwrap(), before);
    }
    #[tokio::test]
    async fn serialized_flush_is_durable_and_cleanup_cannot_be_overtaken() {
        let f = Fixture::new();
        let (events, mut replies) = tokio::sync::mpsc::channel(32);
        let (mut controller, initial) = Controller::start(Ok(f.path.clone()), events).await;
        assert!(initial.is_ok());
        controller.submit(queued("old-account"), 0).unwrap();
        controller.submit(Queue::default(), 1).unwrap();
        controller.flush(Queue::default(), 1).await.unwrap();
        drop(controller);
        while let Ok(event) = replies.try_recv() {
            assert!(matches!(event, Event::Stored(_)));
        }
        let (store, queue) = Store::open(&f.path).unwrap();
        assert_eq!(queue.snapshot().membership_count, 0);
        assert!(backup_files(&store.backups()).unwrap().is_empty());
    }
}
