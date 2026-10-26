use super::{Entry, LIMIT, PAGE, Result, View};
use crate::cache_files::{Directory, private_file};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    time::Duration,
};
const APP: i32 = 0x514f4841;
const DB: &str = "visits.sqlite3";
const PRESERVE: &str = "Listening archive is unsafe, damaged or unsupported; existing files were preserved. Repair storage and restart Qoda.";
pub(super) struct Store {
    root: Directory,
    lock: File,
    conn: Option<Connection>,
    identity: (u64, u64),
    location: std::path::PathBuf,
}
impl Drop for Store {
    fn drop(&mut self) {
        drop(self.conn.take());
        let _ = self.lock.unlock();
    }
}
impl Store {
    pub fn open(path: &Path, now: u64) -> Result<Self> {
        let root = Directory::open(path)?;
        let location = root.resolved_path()?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(root.path().join("archive.lock"))
            .map_err(|_| PRESERVE)?;
        if !private_file(&lock.metadata().map_err(|_| PRESERVE)?) {
            return Err(PRESERVE);
        }
        lock.try_lock()
            .map_err(|_| "Listening archive is in use by another backend.")?;
        let mut orphan = false;
        for suffix in ["-journal", "-wal", "-shm"] {
            orphan |= root.file(&format!("{DB}{suffix}"))?.is_some();
        }
        let existed = root.file(DB)?.is_some();
        if !existed && orphan {
            return Err(PRESERVE);
        }
        if !existed {
            if std::fs::read_dir(root.path())
                .map_err(|_| PRESERVE)?
                .any(|e| {
                    e.is_ok_and(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with("archive-preserved-")
                    })
                })
            {
                return Err(PRESERVE);
            }
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(root.path().join(DB))
                .map_err(|_| PRESERVE)?;
        }
        let file = root.file(DB)?.ok_or(PRESERVE)?;
        let meta = file.metadata().map_err(|_| PRESERVE)?;
        if meta.len() > 128 * 1024 * 1024 {
            return Err(PRESERVE);
        }
        let identity = (meta.dev(), meta.ino());
        if existed && meta.len() != 0 {
            let check = Connection::open_with_flags(
                location.join(DB),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .map_err(|_| PRESERVE)?;
            check
                .pragma_update(None, "trusted_schema", false)
                .map_err(|_| PRESERVE)?;
            let version: i64 = check
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(|_| PRESERVE)?;
            let app: i32 = check
                .pragma_query_value(None, "application_id", |r| r.get(0))
                .map_err(|_| PRESERVE)?;
            if version != 1 || app != APP {
                return Err(PRESERVE);
            }
            let health: String = check
                .query_row("PRAGMA quick_check(1)", [], |r| r.get(0))
                .map_err(|_| PRESERVE)?;
            if health != "ok" {
                return Err(PRESERVE);
            }
        } else if existed {
            return Err(PRESERVE);
        }
        let mut conn =
            Connection::open_with_flags(root.path().join(DB), OpenFlags::SQLITE_OPEN_READ_WRITE)
                .map_err(|_| PRESERVE)?;
        conn.pragma_update(None, "trusted_schema", false)
            .map_err(|_| PRESERVE)?;
        conn.busy_timeout(Duration::from_millis(250))
            .map_err(|_| PRESERVE)?;
        if !existed {
            let tx = conn.transaction().map_err(|_| PRESERVE)?;
            tx.execute_batch("CREATE TABLE config(slot INTEGER PRIMARY KEY CHECK(slot=1),account TEXT,days INTEGER NOT NULL,clock INTEGER NOT NULL);INSERT INTO config VALUES(1,NULL,90,0);CREATE TABLE visits(id TEXT PRIMARY KEY,first INTEGER NOT NULL,last INTEGER NOT NULL,payload TEXT NOT NULL);CREATE INDEX visits_last ON visits(last DESC,id);").map_err(|_|PRESERVE)?;
            tx.pragma_update(None, "application_id", APP)
                .map_err(|_| PRESERVE)?;
            tx.pragma_update(None, "user_version", 1)
                .map_err(|_| PRESERVE)?;
            tx.commit().map_err(|_| PRESERVE)?;
        }
        let mut store = Self {
            root,
            lock,
            conn: Some(conn),
            identity,
            location,
        };
        store.check()?;
        store.policy()?;
        store.entries(0, LIMIT + 1)?;
        store
            .db()
            .pragma_update(None, "journal_mode", "DELETE")
            .map_err(|_| PRESERVE)?;
        store
            .db()
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|_| PRESERVE)?;
        store
            .db()
            .pragma_update(None, "secure_delete", "ON")
            .map_err(|_| PRESERVE)?;
        let page_size: i64 = store
            .db()
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(|_| PRESERVE)?;
        if page_size <= 0 {
            return Err(PRESERVE);
        }
        store
            .db()
            .pragma_update(None, "max_page_count", 128 * 1024 * 1024 / page_size)
            .map_err(|_| PRESERVE)?;
        store.retention(store.policy()?.0, now)?;
        store.root.0.sync_all().map_err(|_| PRESERVE)?;
        Ok(store)
    }
    fn db(&self) -> &Connection {
        self.conn.as_ref().unwrap()
    }
    fn check(&self) -> Result<()> {
        let file = self.root.file(DB)?.ok_or(PRESERVE)?;
        let m = file.metadata().map_err(|_| PRESERVE)?;
        if (m.dev(), m.ino()) != self.identity || self.root.resolved_path()? != self.location {
            return Err(PRESERVE);
        }
        Ok(())
    }
    fn policy(&self) -> Result<(u16, Option<String>, u64)> {
        let (days, account, clock): (u16, Option<String>, i64) = self
            .db()
            .query_row(
                "SELECT days,account,clock FROM config WHERE slot=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|_| PRESERVE)?;
        if ![0, 7, 30, 90, 365].contains(&days)
            || account
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 256)
        {
            return Err(PRESERVE);
        }
        Ok((days, account, u64::try_from(clock).map_err(|_| PRESERVE)?))
    }
    fn entries(&self, offset: usize, limit: usize) -> Result<Vec<Entry>> {
        let count: i64 = self
            .db()
            .query_row("SELECT count(*) FROM visits", [], |r| r.get(0))
            .map_err(|_| PRESERVE)?;
        let count = usize::try_from(count).map_err(|_| PRESERVE)?;
        if count > LIMIT {
            return Err(PRESERVE);
        }
        let mut query=self.db().prepare("SELECT id,first,last,payload FROM visits WHERE length(CAST(payload AS BLOB))<=32768 ORDER BY last DESC,id LIMIT ?1 OFFSET ?2").map_err(|_|PRESERVE)?;
        let mut rows = query
            .query((limit as i64, offset as i64))
            .map_err(|_| PRESERVE)?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next().map_err(|_| PRESERVE)? {
            let id: String = row.get(0).map_err(|_| PRESERVE)?;
            let first =
                u64::try_from(row.get::<_, i64>(1).map_err(|_| PRESERVE)?).map_err(|_| PRESERVE)?;
            let last =
                u64::try_from(row.get::<_, i64>(2).map_err(|_| PRESERVE)?).map_err(|_| PRESERVE)?;
            let payload: String = row.get(3).map_err(|_| PRESERVE)?;
            let entry: Entry = serde_json::from_str(&payload).map_err(|_| PRESERVE)?;
            if !entry.valid()
                || entry.visit.to_string() != id
                || entry.first_observed_ms != first
                || entry.last_observed_ms != last
            {
                return Err(PRESERVE);
            }
            entries.push(entry);
        }
        if entries.len() != count.saturating_sub(offset).min(limit) {
            return Err(PRESERVE);
        }
        Ok(entries)
    }
    pub fn view(&self, offset: usize) -> Result<View> {
        self.check()?;
        let (days, _, _) = self.policy()?;
        let total: i64 = self
            .db()
            .query_row("SELECT count(*) FROM visits", [], |r| r.get(0))
            .map_err(|_| PRESERVE)?;
        let total = usize::try_from(total).map_err(|_| PRESERVE)?;
        let offset = offset.min(total.saturating_sub(1) / PAGE * PAGE);
        let entries = self.entries(offset, PAGE)?;
        Ok(View {
            available: true,
            cleanup_pending: false,
            incomplete: false,
            retention_days: days,
            total,
            offset,
            has_next: offset + entries.len() < total,
            entries,
            message: if days == 0 {
                "Archive recording is off; retained visits cleared.".into()
            } else {
                format!(
                    "Nominal observations · {days} days · at most {LIMIT} visits. Positions are not listening duration."
                )
            },
            export_path: None,
        })
    }
    pub fn bind(&mut self, account: &str) -> Result<bool> {
        self.check()?;
        if account.is_empty() || account.len() > 256 {
            return Err(PRESERVE);
        }
        let changed = self.policy()?.1.as_deref() != Some(account);
        if changed {
            let tx = self
                .conn
                .as_mut()
                .unwrap()
                .transaction()
                .map_err(|_| PRESERVE)?;
            tx.execute("DELETE FROM visits", []).map_err(|_| PRESERVE)?;
            tx.execute("UPDATE config SET account=?1 WHERE slot=1", [account])
                .map_err(|_| PRESERVE)?;
            tx.commit().map_err(|_| PRESERVE)?;
        }
        Ok(changed)
    }
    pub fn clear(&mut self, unbind: bool) -> Result<()> {
        self.check()?;
        let tx = self
            .conn
            .as_mut()
            .unwrap()
            .transaction()
            .map_err(|_| PRESERVE)?;
        tx.execute("DELETE FROM visits", []).map_err(|_| PRESERVE)?;
        if unbind {
            tx.execute("UPDATE config SET account=NULL WHERE slot=1", [])
                .map_err(|_| PRESERVE)?;
        }
        tx.commit().map_err(|_| PRESERVE)
    }
    fn trim(tx: &rusqlite::Transaction<'_>, days: u16, now: u64) -> Result<()> {
        let cutoff = now.saturating_sub(u64::from(days) * 86_400_000);
        if days == 0 {
            tx.execute("DELETE FROM visits", []).map_err(|_| PRESERVE)?;
        } else {
            tx.execute("DELETE FROM visits WHERE last<?1", [cutoff as i64])
                .map_err(|_| PRESERVE)?;
            tx.execute("DELETE FROM visits WHERE id IN(SELECT id FROM visits ORDER BY last DESC,id LIMIT -1 OFFSET ?1)",[LIMIT as i64]).map_err(|_|PRESERVE)?;
        }
        tx.execute("UPDATE config SET clock=?1 WHERE slot=1", [now as i64])
            .map_err(|_| PRESERVE)?;
        Ok(())
    }
    pub fn retention(&mut self, days: u16, now: u64) -> Result<()> {
        self.check()?;
        if ![0, 7, 30, 90, 365].contains(&days) {
            return Err("Choose Off, 7, 30, 90 or 365 days.");
        }
        let clock = self.policy()?.2.max(now).min(i64::MAX as u64);
        let tx = self
            .conn
            .as_mut()
            .unwrap()
            .transaction()
            .map_err(|_| PRESERVE)?;
        tx.execute("UPDATE config SET days=?1 WHERE slot=1", [days])
            .map_err(|_| PRESERVE)?;
        Self::trim(&tx, days, clock)?;
        tx.commit().map_err(|_| PRESERVE)
    }
    pub fn observe(&mut self, mut entry: Entry, now: u64) -> Result<()> {
        self.check()?;
        if !entry.valid() {
            return Err(
                "Nominal archive observation exceeded supported metadata bounds; observation was not saved.",
            );
        }
        let (days, account, clock) = self.policy()?;
        if days == 0 || account.is_none() {
            return Ok(());
        }
        let clock = clock.max(now).min(i64::MAX as u64);
        entry.first_observed_ms = entry.first_observed_ms.max(clock).min(i64::MAX as u64);
        entry.last_observed_ms = entry.first_observed_ms;
        let length: Option<i64> = self
            .db()
            .query_row(
                "SELECT length(CAST(payload AS BLOB)) FROM visits WHERE id=?1",
                [entry.visit.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| PRESERVE)?;
        if length.is_some_and(|length| !(0..=32768).contains(&length)) {
            return Err(PRESERVE);
        }
        let old: Option<String> = self
            .db()
            .query_row(
                "SELECT payload FROM visits WHERE id=?1 AND length(CAST(payload AS BLOB))<=32768",
                [entry.visit.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| PRESERVE)?;
        if let Some(old) = old {
            let old: Entry = serde_json::from_str(&old).map_err(|_| PRESERVE)?;
            if !old.valid() || !old.same_visit(&entry) {
                return Err(PRESERVE);
            }
            entry.first_observed_ms = old.first_observed_ms;
            entry.last_observed_ms = old.last_observed_ms.max(entry.last_observed_ms);
            entry.max_position_ms = old.max_position_ms.max(entry.last_position_ms);
            entry.title = old.title;
            entry.artist = old.artist;
            entry.album = old.album;
        }
        let payload = serde_json::to_string(&entry).map_err(|_| PRESERVE)?;
        if payload.len() > 32768 {
            return Err(PRESERVE);
        }
        let tx = self
            .conn
            .as_mut()
            .unwrap()
            .transaction()
            .map_err(|_| PRESERVE)?;
        tx.execute("INSERT INTO visits(id,first,last,payload) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET last=excluded.last,payload=excluded.payload",(entry.visit.to_string(),entry.first_observed_ms as i64,entry.last_observed_ms as i64,payload)).map_err(|_|PRESERVE)?;
        Self::trim(&tx, days, clock)?;
        tx.commit()
            .map_err(|_| "Listening archive observation was not saved; existing visits retained.")
    }
    pub fn export(&self, now: u64, incomplete: bool) -> Result<String> {
        self.check()?;
        let entries = self.entries(0, LIMIT)?;
        let (days, _, _) = self.policy()?;
        let bytes=serde_json::to_vec_pretty(&serde_json::json!({"format":1,"exported_at_ms":now,"retention_days":days,"known_incomplete_this_run":incomplete,"meaning":"Nominal observation positions, not listening duration, completion or physical audibility.","entries":entries})).map_err(|_|PRESERVE)?;
        let exports = Directory::open(&self.root.path().join("exports"))?;
        let name = format!("qoda-listening-{}.json", uuid::Uuid::new_v4());
        let temporary = format!(".tmp-{}", uuid::Uuid::new_v4());
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(exports.path().join(&temporary))
                .map_err(|_| "Archive export could not be created.")?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| "Archive export could not be saved.")?;
            std::fs::hard_link(exports.path().join(&temporary), exports.path().join(&name))
                .map_err(|_| "Archive export could not be published.")?;
            std::fs::remove_file(exports.path().join(&temporary)).map_err(|_| PRESERVE)?;
            exports.0.sync_all().map_err(|_| PRESERVE)?;
            // Resolve through the stable user-facing root, not /proc/self/fd.
            Ok(self
                .root
                .resolved_path()?
                .join("exports")
                .join(&name)
                .to_string_lossy()
                .into_owned())
        })();
        let _ = std::fs::remove_file(exports.path().join(temporary));
        result
    }
}
