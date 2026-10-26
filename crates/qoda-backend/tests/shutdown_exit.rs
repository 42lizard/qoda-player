//! Real process shutdown at private SQLite/runtime boundaries; no media or bus.
use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    process: Child,
    stop_mode: &'static str,
}
impl Fixture {
    fn start() -> Self {
        Self::prepared(|_| {})
    }
    fn prepared(before: impl FnOnce(&std::path::Path)) -> Self {
        let root = std::env::temp_dir().join(format!("qoda-shutdown-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for name in ["runtime", "config", "state", "cache", "data"] {
            fs::create_dir(root.join(name)).unwrap();
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        before(&root);
        let process = Self::spawn(&root);
        Self {
            root,
            process,
            stop_mode: "startup",
        }
    }
    fn spawn(root: &std::path::Path) -> Child {
        Command::new(env!("CARGO_BIN_EXE_qoda-backend"))
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}/absent-bus", root.display()),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(root.join("backend.log")).unwrap())
            .spawn()
            .unwrap()
    }
    fn connect(&mut self) -> BufReader<UnixStream> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(
                self.process.try_wait().unwrap().is_none(),
                "Backend exited during readiness"
            );
            if let Ok(mut socket) = UnixStream::connect(self.root.join("runtime/qoda/backend.sock"))
            {
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                socket
                    .write_all(b"{\"op\":\"hello\",\"protocol\":1}\n")
                    .unwrap();
                return BufReader::new(socket);
            }
            assert!(Instant::now() < deadline, "Backend readiness timed out");
            thread::sleep(Duration::from_millis(25));
        }
    }
    fn ready(reader: &mut BufReader<UnixStream>) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut line = String::new();
        let mut save_requested = false;
        loop {
            match reader.read_line(&mut line) {
                Ok(0) => panic!("Backend closed before initial save"),
                Ok(_) => {
                    let state: Value = serde_json::from_str(&line).unwrap();
                    line.clear();
                    if !save_requested && state["storage"]["serial"] == "0" {
                        writeln!(
                            reader.get_mut(),
                            "{}",
                            serde_json::json!({"op":"retry_save","instance":state["instance"]})
                        )
                        .unwrap();
                        save_requested = true;
                    }
                    if state["storage"]["serial"] != "0" && state["storage"]["state"] == "saved" {
                        assert_eq!(state["intent"], "paused");
                        assert!(state["queue"]["entries"].as_array().unwrap().is_empty());
                        return state;
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => panic!("Readiness failed: {error}"),
            }
            assert!(Instant::now() < deadline, "Initial save timed out");
        }
    }
    fn wait(&mut self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.process.try_wait().unwrap() {
                let code = status.code().expect("Backend died from a signal");
                if matches!(code, 0 | 74 | 75) {
                    assert!(
                        !self.root.join("runtime/qoda/backend.sock").exists(),
                        "Shutdown left its socket"
                    );
                }
                return code;
            }
            assert!(
                Instant::now() < deadline,
                "Shutdown timed out via {}. Signal state: {}. Backend log: {}",
                self.stop_mode,
                fs::read_to_string(format!("/proc/{}/status", self.process.id()))
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| line.starts_with("Sig"))
                    .collect::<Vec<_>>()
                    .join("; "),
                fs::read_to_string(self.root.join("backend.log")).unwrap_or_default()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
    fn signal(&mut self) {
        self.stop_mode = "SIGINT";
        // SAFETY: this child is owned by the fixture and has not been reaped.
        assert_eq!(
            unsafe { libc::kill(self.process.id().try_into().unwrap(), libc::SIGINT) },
            0
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.process.try_wait().ok().flatten().is_none() {
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn ipc_quit_and_service_signal_exit_cleanly_after_successful_checkpoint() {
    for signal in std::iter::once(false).chain(std::iter::repeat_n(true, 12)) {
        let mut fixture = Fixture::start();
        let mut reader = fixture.connect();
        let state = Fixture::ready(&mut reader);
        if signal {
            fixture.signal();
        } else {
            fixture.stop_mode = "IPC Quit";
            writeln!(
                reader.get_mut(),
                "{}",
                serde_json::json!({"op":"quit","instance":state["instance"]})
            )
            .unwrap();
        }
        assert_eq!(fixture.wait(), 0);
        let database = Connection::open(fixture.root.join("state/qoda/session.sqlite3")).unwrap();
        let payload: String = database
            .query_row("SELECT payload FROM session WHERE slot=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&payload).unwrap()["format"],
            4
        );
    }
}

#[test]
fn failed_final_checkpoint_exits_74_and_preserves_last_committed_database() {
    for signal in [false, true] {
        let mut fixture = Fixture::start();
        let mut reader = fixture.connect();
        let state = Fixture::ready(&mut reader);
        let path = fixture.root.join("state/qoda/session.sqlite3");
        let committed = fs::read(&path).unwrap();
        let database = Connection::open(&path).unwrap();
        // Keep every other descriptor for this database closed while holding
        // the SQLite lock: POSIX close() can release the process's file locks.
        // https://sqlite.org/howtocorrupt.html section 2.2.
        database.execute_batch("BEGIN IMMEDIATE").unwrap();
        writeln!(reader.get_mut(), "{}", serde_json::json!({"op":"set_repeat","instance":state["instance"],"mode":"all","account_epoch":state["account_epoch"],"queue_session":state["queue"]["session"],"queue_revision":state["queue"]["revision"]})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0);
            let pending: Value = serde_json::from_str(&line).unwrap();
            if pending["queue"]["repeat"] == "all" {
                assert_eq!(pending["intent"], "paused");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Pending repeat change was not accepted"
            );
        }
        if signal {
            fixture.signal();
        } else {
            fixture.stop_mode = "IPC Quit";
            writeln!(
                reader.get_mut(),
                "{}",
                serde_json::json!({"op":"quit","instance":state["instance"]})
            )
            .unwrap();
        }
        assert_eq!(fixture.wait(), 74);
        assert!(
            fs::read_to_string(fixture.root.join("backend.log"))
                .unwrap()
                .contains("final checkpoint failed")
        );
        database.execute_batch("ROLLBACK").unwrap();
        assert_eq!(fs::read(&path).unwrap(), committed);
        // The old checkpoint remains readable and no recovery data was deleted.
        let saved: i64 = database
            .query_row("SELECT count(*) FROM session", [], |row| row.get(0))
            .unwrap();
        assert_eq!(saved, 1);
    }
}

#[test]
fn cleanup_failure_exits_76_and_preserves_replaced_path() {
    for conflict in ["file", "directory", "symlink"] {
        let mut fixture = Fixture::start();
        let mut reader = fixture.connect();
        let state = Fixture::ready(&mut reader);
        let socket = fixture.root.join("runtime/qoda/backend.sock");
        let target = fixture.root.join("sentinel");
        fs::remove_file(&socket).unwrap();
        match conflict {
            "directory" => fs::create_dir(&socket).unwrap(),
            "symlink" => {
                fs::write(&target, "preserved conflict").unwrap();
                std::os::unix::fs::symlink(&target, &socket).unwrap();
            }
            _ => fs::write(&socket, "preserved conflict").unwrap(),
        }
        let preserved = || {
            if conflict == "directory" {
                assert!(socket.is_dir());
            } else {
                assert_eq!(fs::read_to_string(&socket).unwrap(), "preserved conflict");
            }
            if conflict == "symlink" {
                assert!(socket.is_symlink());
            }
        };
        writeln!(
            reader.get_mut(),
            "{}",
            serde_json::json!({"op":"quit","instance":state["instance"]})
        )
        .unwrap();
        assert_eq!(fixture.wait(), 76);
        preserved();
        assert!(
            fs::read_to_string(fixture.root.join("backend.log"))
                .unwrap()
                .contains("runtime socket cleanup failed")
        );
        fixture.process = Fixture::spawn(&fixture.root);
        assert_eq!(fixture.wait(), 78);
        preserved();
    }
}

#[test]
fn startup_preserves_a_live_unmanaged_socket() {
    let mut listener = None;
    let mut fixture = Fixture::prepared(|root| {
        let directory = root.join("runtime/qoda");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        listener =
            Some(std::os::unix::net::UnixListener::bind(directory.join("backend.sock")).unwrap());
    });
    assert_eq!(fixture.wait(), 78);
    assert!(fixture.root.join("runtime/qoda/backend.sock").exists());
    drop(listener);
}
