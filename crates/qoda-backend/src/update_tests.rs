//! Synthetic native decoding -> installed updater RPC -> private checkpoint.
use super::*;
use std::{process::Stdio, time::Duration};
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[tokio::test]
async fn installed_pause_handshake_waits_for_native_pause_and_a_new_durable_serial() {
    let path = std::env::temp_dir().join(format!("qoda-update-native-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    let directory = Directory(path);
    std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = directory.0.join("runtime");
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::create_dir(runtime.join("qoda")).unwrap();
    std::fs::set_permissions(runtime.join("qoda"), std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = tokio::net::UnixListener::bind(runtime.join("qoda/backend.sock")).unwrap();
    let player = Player::test_output("null").unwrap();
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/replaygain/gain.wav"
    );
    let extended = directory.0.join("synthetic-ten-seconds.wav");
    let generated = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-stream_loop",
            "39",
            "-i",
            file,
            "-t",
            "10",
            "-c:a",
            "pcm_s16le",
            "-map_metadata",
            "-1",
        ])
        .arg(&extended)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    player.send(Command::Load(7, extended.to_str().unwrap().into(), 0.0));
    let ready_by = std::time::Instant::now() + Duration::from_secs(3);
    while !player.status().entry_ready {
        assert!(
            std::time::Instant::now() < ready_by,
            "Synthetic decoder did not load: {:?}",
            player.status()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    player.send(Command::Pause(false));
    let mut queue = queue::Queue::default();
    let id = queue
        .enqueue(
            qoda_provider::Track {
                reference: TrackRef {
                    source: SourceId::new(),
                    key: "synthetic-updater-tone".to_owned().try_into().unwrap(),
                },
                title: "Original mathematical tone".into(),
                artist: "Fixture".into(),
                album: "Fixture".into(),
                duration_ms: Some(qoda_core::Counter::new(10000)),
            },
            false,
        )
        .unwrap();
    queue.select(id).unwrap();
    let visit = queue.visit();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut confirmed = None;
    loop {
        record_clock(&mut queue, &player.status(), 7, &mut confirmed, false);
        if confirmed == Some(7) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Synthetic decoder did not provide a nominal clock: {:?}",
            player.status()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let database = directory.0.join("state/session.sqlite3");
    let (events, _) = mpsc::channel(8);
    let (mut durable, opened) = storage::Controller::start(Ok(database.clone()), events).await;
    opened.unwrap();
    durable.flush(queue.clone(), 0).await.unwrap();
    let initial_serial = durable.serial();
    let mut child=tokio::process::Command::new("python3").args(["-c","import importlib.util,sys; s=importlib.util.spec_from_file_location('qoda',sys.argv[1]); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); m.Launch().pause_checkpoint()",concat!(env!("CARGO_MANIFEST_DIR"),"/../../tools/launch.py")]).env("XDG_RUNTIME_DIR",&runtime).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true).spawn().unwrap();
    let (socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let (reader, mut writer) = socket.into_split();
    let mut lines = BufReader::new(reader).lines();
    let hello: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(hello["op"], "hello");
    let frame = |intent: &str, serial: u64, audio: player::Status| {
        json!({"protocol":1,"instance":"native-update-fixture","intent":intent,"audio":audio,"storage":{"state":"saved","serial":serial.to_string(),"cleanup_pending":false}}).to_string()+"\n"
    };
    writer
        .write_all(frame("playing", initial_serial, player.status()).as_bytes())
        .await
        .unwrap();
    let pause: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(pause["op"], "pause");
    assert_eq!(pause["instance"], "native-update-fixture");
    session_commands::pause_for_checkpoint(&mut queue, &player, 7);
    let audio = player.barrier().await.unwrap();
    assert!(audio.paused);
    assert_eq!(queue.visit(), visit);
    // Intent/native pause alone cannot acknowledge the old saved serial.
    writer
        .write_all(frame("paused", initial_serial, audio.clone()).as_bytes())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(child.try_wait().unwrap().is_none());
    durable.flush(queue.clone(), 0).await.unwrap();
    writer
        .write_all(frame("paused", durable.serial(), audio).as_bytes())
        .await
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let conn = rusqlite::Connection::open(database).unwrap();
    let payload: String = conn
        .query_row("SELECT payload FROM session WHERE slot=1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let value: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(value["queue"]["visit"], visit.to_string());
    assert!(value["queue"]["position_seconds"].as_f64().unwrap() > 0.0);
    player.send(Command::Stop);
}
