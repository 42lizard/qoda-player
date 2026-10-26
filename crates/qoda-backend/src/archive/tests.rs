use super::*;
use qoda_core::{Counter, SourceId};
use qoda_provider::Track;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("qoda-archive-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn queue() -> queue::Queue {
    let mut q = queue::Queue::default();
    let track = Track {
        reference: TrackRef {
            source: SourceId::new(),
            key: "synthetic".to_owned().try_into().unwrap(),
        },
        title: "Synthetic".into(),
        artist: "Fixture".into(),
        album: "Original tone".into(),
        duration_ms: Some(Counter::new(10000)),
    };
    let id = q.enqueue(track, false).unwrap();
    q.select(id).unwrap();
    q
}
fn audio() -> player::Status {
    player::Status {
        generation: 7,
        position_seconds: Some(1.25),
        entry_ready: true,
        idle: false,
        paused: false,
        ..Default::default()
    }
}
fn entry(q: &queue::Queue, position: f64, wall: u64) -> Entry {
    let mut a = audio();
    a.position_seconds = Some(position);
    Entry::capture(q, &a, 7, Some(7), false, wall).unwrap()
}
fn store(f: &Fixture) -> store::Store {
    let mut s = store::Store::open(&f.0, 10000).unwrap();
    s.bind("synthetic-account").unwrap();
    s
}
#[test]
fn preparation_pause_stale_error_and_other_render_never_archive() {
    let q = queue();
    let a = audio();
    assert!(Entry::capture(&q, &a, 7, Some(7), false, 1).is_some());
    for (generation, confirmed, failed) in
        [(8, Some(8), false), (7, None, false), (7, Some(7), true)]
    {
        assert!(Entry::capture(&q, &a, generation, confirmed, failed, 1).is_none());
    }
    for position in [
        None,
        Some(0.0),
        Some(-1.0),
        Some(f64::NAN),
        Some(f64::INFINITY),
    ] {
        let mut a = audio();
        a.position_seconds = position;
        assert!(Entry::capture(&q, &a, 7, Some(7), false, 1).is_none());
    }
    for condition in 0..4 {
        let mut a = audio();
        match condition {
            0 => a.paused = true,
            1 => a.idle = true,
            2 => a.entry_ready = false,
            _ => a.error = Some("synthetic"),
        };
        assert!(Entry::capture(&q, &a, 7, Some(7), false, 1).is_none());
    }
}
#[test]
fn seeks_recovery_and_restore_keep_visit_repeat_and_duplicates_do_not() {
    let f = Fixture::new();
    let mut s = store(&f);
    let mut q = queue();
    let visit = q.visit();
    s.observe(entry(&q, 5.0, 10000), 10000).unwrap();
    q.seek_to(1.0).unwrap();
    s.observe(entry(&q, 1.0, 9000), 9000).unwrap();
    let mut a = audio();
    a.generation = 8;
    assert_eq!(
        Entry::capture(&q, &a, 8, Some(8), false, 11000)
            .unwrap()
            .visit,
        visit
    );
    let restored: queue::Queue = serde_json::from_value(serde_json::to_value(&q).unwrap()).unwrap();
    assert_eq!(restored.visit(), visit);
    let v = s.view(0).unwrap();
    assert_eq!(v.total, 1);
    assert_eq!(v.entries[0].first_observed_ms, 10000);
    assert_eq!(v.entries[0].last_observed_ms, 10000);
    assert_eq!(v.entries[0].last_position_ms, 1000);
    assert_eq!(v.entries[0].max_position_ms, 5000);
    q.set_repeat(queue::Repeat::One).unwrap();
    q.complete(true).unwrap();
    assert_ne!(q.visit(), visit);
    s.observe(entry(&q, 0.2, 11000), 11000).unwrap();
    let track = q.visible_track(q.selected().unwrap()).unwrap();
    let duplicate = q.enqueue(track, false).unwrap();
    q.select(duplicate).unwrap();
    s.observe(entry(&q, 0.3, 12000), 12000).unwrap();
    let v = s.view(0).unwrap();
    assert_eq!(v.total, 3);
    assert_eq!(v.entries[0].track, v.entries[1].track);
    assert_ne!(v.entries[0].occurrence, v.entries[1].occurrence);
    q.remove(duplicate).unwrap();
    assert_eq!(s.view(0).unwrap().total, 3);
}
#[test]
fn private_exports_survive_clear_but_accounts_and_off_clear_internal_archive() {
    let f = Fixture::new();
    let mut s = store(&f);
    let q = queue();
    s.observe(entry(&q, 1.0, 10000), 10000).unwrap();
    let exported = s.export(10000, false).unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&exported).unwrap()).unwrap();
    assert_eq!(value["format"], 1);
    assert_eq!(value["entries"].as_array().unwrap().len(), 1);
    for path in [f.0.join("visits.sqlite3"), PathBuf::from(&exported)] {
        let m = std::fs::metadata(path).unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o600);
        assert_eq!(m.nlink(), 1);
    }
    s.clear(false).unwrap();
    assert_eq!(s.view(0).unwrap().total, 0);
    assert!(PathBuf::from(&exported).exists());
    s.observe(entry(&q, 2.0, 11000), 11000).unwrap();
    s.bind("other-account").unwrap();
    assert_eq!(s.view(0).unwrap().total, 0);
    s.observe(entry(&q, 2.0, 12000), 12000).unwrap();
    s.clear(true).unwrap();
    s.observe(entry(&q, 3.0, 13000), 13000).unwrap();
    assert_eq!(s.view(0).unwrap().total, 0);
    s.bind("other-account").unwrap();
    s.retention(0, 14000).unwrap();
    s.observe(entry(&q, 3.0, 15000), 15000).unwrap();
    assert_eq!(s.view(0).unwrap().total, 0);
    drop(s);
    let s = store::Store::open(&f.0, 16000).unwrap();
    assert_eq!(s.view(0).unwrap().retention_days, 0);
}
#[test]
fn retention_is_transactional_and_oldest_last_observations_are_trimmed() {
    let f = Fixture::new();
    let mut s = store(&f);
    let q = queue();
    s.observe(entry(&q, 1.0, 10000), 10000).unwrap();
    s.retention(7, 10000 + 8 * 86_400_000).unwrap();
    assert_eq!(s.view(0).unwrap().total, 0);
    assert!(s.retention(8, 10000).is_err());
    // Populate the cap in one fixture transaction; public writes must evict the oldest.
    let db = rusqlite::Connection::open(f.0.join("visits.sqlite3")).unwrap();
    let tx = db.unchecked_transaction().unwrap();
    for n in 0..LIMIT {
        let mut e = entry(&q, 1.0, 1_000_000_000 + n as u64);
        e.visit = uuid::Uuid::from_u128(n as u128 + 1);
        let payload = serde_json::to_string(&e).unwrap();
        tx.execute(
            "INSERT INTO visits VALUES(?1,?2,?2,?3)",
            (e.visit.to_string(), e.last_observed_ms as i64, payload),
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let mut newest = entry(&q, 2.0, 1_000_020_000);
    newest.visit = uuid::Uuid::new_v4();
    s.observe(newest, 1_000_020_000).unwrap();
    assert_eq!(s.view(0).unwrap().total, LIMIT);
    let removed: i64 = db
        .query_row(
            "SELECT count(*) FROM visits WHERE id=?1",
            [uuid::Uuid::from_u128(1).to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(removed, 0);
}
#[test]
fn failed_writes_and_foreign_visit_identity_leave_committed_observation() {
    let f = Fixture::new();
    let mut s = store(&f);
    let q = queue();
    s.observe(entry(&q, 1.0, 10000), 10000).unwrap();
    let mut foreign = entry(&q, 2.0, 11000);
    foreign.track.source = SourceId::new();
    assert!(s.observe(foreign, 11000).is_err());
    let db = rusqlite::Connection::open(f.0.join("visits.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_update BEFORE UPDATE ON visits BEGIN SELECT RAISE(ABORT,'synthetic interruption');END;").unwrap();
    assert!(s.observe(entry(&q, 5.0, 12000), 12000).is_err());
    assert_eq!(s.view(0).unwrap().entries[0].last_position_ms, 1000);
}
#[test]
fn newer_corrupt_unknown_and_unsafe_archives_are_preserved() {
    for variant in 0..4 {
        let f = Fixture::new();
        {
            let mut s = store(&f);
            s.observe(entry(&queue(), 1.0, 10000), 10000).unwrap();
        }
        let path = f.0.join("visits.sqlite3");
        match variant {
            0 => {
                let db = rusqlite::Connection::open(&path).unwrap();
                db.pragma_update(None, "user_version", 2).unwrap();
            }
            1 => std::fs::write(&path, b"damaged synthetic archive").unwrap(),
            2 => {
                let db = rusqlite::Connection::open(&path).unwrap();
                db.pragma_update(None, "application_id", 123).unwrap();
            }
            _ => std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap(),
        };
        let before = std::fs::read(&path).unwrap();
        assert!(store::Store::open(&f.0, 11000).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
#[tokio::test]
async fn serialized_clear_and_account_fences_cannot_be_overtaken_by_observations() {
    let f = Fixture::new();
    let mut c = Controller::start(Ok(f.0.clone()));
    c.bind("account-a".into()).await.unwrap();
    let q = queue();
    let now = Instant::now();
    c.observe(&q, &audio(), 7, Some(7), false, now);
    c.clear(true).await.unwrap();
    c.observe(&q, &audio(), 7, Some(7), false, now + INTERVAL);
    c.flush().await.unwrap();
    assert_eq!(c.view().total, 0);
    c.bind("account-b".into()).await.unwrap();
    c.observe(&q, &audio(), 7, Some(7), false, now + INTERVAL);
    c.flush().await.unwrap();
    assert_eq!(c.view().total, 1);
    c.clear(false).await.unwrap();
    assert_eq!(c.view().total, 0);
}

#[test]
fn moved_directory_or_database_and_orphan_journal_are_not_reinitialized() {
    let f = Fixture::new();
    let mut s = store(&f);
    let q = queue();
    s.observe(entry(&q, 1.0, 10000), 10000).unwrap();
    let old = f.0.with_extension("moved");
    std::fs::rename(&f.0, &old).unwrap();
    assert!(s.observe(entry(&q, 2.0, 11000), 11000).is_err());
    std::fs::rename(&old, &f.0).unwrap();
    assert_eq!(s.view(0).unwrap().entries[0].last_position_ms, 1000);
    drop(s);
    std::fs::rename(
        f.0.join("visits.sqlite3"),
        f.0.join("archive-preserved-synthetic.sqlite3"),
    )
    .unwrap();
    assert!(store::Store::open(&f.0, 12000).is_err());
    assert!(!f.0.join("visits.sqlite3").exists());
    let orphan = Fixture::new();
    std::fs::write(
        orphan.0.join("visits.sqlite3-journal"),
        b"private orphan fixture",
    )
    .unwrap();
    std::fs::set_permissions(
        orphan.0.join("visits.sqlite3-journal"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(store::Store::open(&orphan.0, 0).is_err());
    assert!(!orphan.0.join("visits.sqlite3").exists());
}
#[test]
fn failed_export_preserves_archive_and_existing_copies() {
    let f = Fixture::new();
    let mut s = store(&f);
    s.observe(entry(&queue(), 1.0, 10000), 10000).unwrap();
    let path = s.export(10000, false).unwrap();
    let before = std::fs::read(&path).unwrap();
    std::fs::set_permissions(f.0.join("exports"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(s.export(11000, false).is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(s.view(0).unwrap().total, 1);
}
#[tokio::test]
async fn a_failed_cleanup_hides_old_account_rows_and_can_be_retried() {
    let f = Fixture::new();
    let mut c = Controller::start(Ok(f.0.clone()));
    c.bind("old-account".into()).await.unwrap();
    let q = queue();
    c.observe(&q, &audio(), 7, Some(7), false, Instant::now());
    c.flush().await.unwrap();
    assert_eq!(c.view().total, 1);
    let db = rusqlite::Connection::open(f.0.join("visits.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_clear BEFORE DELETE ON visits BEGIN SELECT RAISE(ABORT,'fixture cleanup failure');END;").unwrap();
    assert!(c.clear(true).await.is_err());
    let hidden = c.view();
    assert!(hidden.cleanup_pending);
    assert!(hidden.entries.is_empty());
    assert!(hidden.export_path.is_none());
    assert!(!hidden.available);
    db.execute_batch("DROP TRIGGER reject_clear").unwrap();
    c.clear(true).await.unwrap();
    assert!(!c.view().cleanup_pending);
    assert_eq!(c.view().total, 0);
}

#[tokio::test]
async fn failed_observation_stays_visible_and_final_flush_reports_incompleteness() {
    let f = Fixture::new();
    let mut c = Controller::start(Ok(f.0.clone()));
    c.bind("account".into()).await.unwrap();
    let db = rusqlite::Connection::open(f.0.join("visits.sqlite3")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_archive BEFORE INSERT ON visits BEGIN SELECT RAISE(ABORT,'synthetic write failure');END;").unwrap();
    c.observe(&queue(), &audio(), 7, Some(7), false, Instant::now());
    assert!(c.flush().await.is_err());
    assert!(c.view().incomplete);
    assert!(c.view().entries.is_empty());
    db.execute_batch("DROP TRIGGER reject_archive").unwrap();
    c.page(0).await.unwrap();
    assert!(c.view().incomplete);
    assert!(c.flush().await.is_err());
    c.clear(false).await.unwrap();
    c.flush().await.unwrap();
    assert!(!c.view().incomplete);
}
