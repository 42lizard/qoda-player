//! All native calls and pointers stay on one thread. Positions are nominal audio clocks,
//! not a physical output or listening-history guarantee.
use serde::Serialize;
use std::{
    ffi::{CString, c_char, c_int, c_void},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
#[repr(C)]
struct NativeEvent {
    id: c_int,
    error: c_int,
    userdata: u64,
    data: *mut c_void,
}
#[repr(C)]
struct EndFile {
    reason: c_int,
    error: c_int,
    entry: i64,
    insert: i64,
    count: c_int,
}
#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_create() -> *mut c_void;
    fn mpv_set_option_string(h: *mut c_void, k: *const c_char, v: *const c_char) -> c_int;
    fn mpv_initialize(h: *mut c_void) -> c_int;
    fn mpv_command(h: *mut c_void, args: *const *const c_char) -> c_int;
    fn mpv_get_property(
        h: *mut c_void,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    fn mpv_wait_event(h: *mut c_void, timeout: f64) -> *const NativeEvent;
    fn mpv_terminate_destroy(h: *mut c_void);
}
pub enum Command {
    Load(u64, String),
    Pause(bool),
    Seek(f64),
    Stop,
    Quit,
}
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Status {
    #[serde(serialize_with = "decimal")]
    pub generation: u64,
    pub position_seconds: Option<f64>,
    pub idle: bool,
    pub paused: bool,
    pub error: Option<&'static str>,
}
fn decimal<S: serde::Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&value.to_string())
}
pub struct Player {
    tx: mpsc::SyncSender<Command>,
    status: Arc<Mutex<Status>>,
    desired_playing: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}
// Format values and layout are from the installed libmpv client.h (API >= 1.108).
unsafe fn property<T: Default>(h: *mut c_void, name: &str, format: c_int) -> Option<T> {
    let mut v = T::default();
    let name = CString::new(name).ok()?;
    if unsafe { mpv_get_property(h, name.as_ptr(), format, (&mut v as *mut T).cast()) } < 0 {
        None
    } else {
        Some(v)
    }
}
impl Player {
    pub fn start() -> Result<Self, &'static str> {
        Self::with_output("pipewire")
    }
    fn with_output(output: &str) -> Result<Self, &'static str> {
        let output = output.to_owned();
        let desired_playing = Arc::new(AtomicBool::new(false));
        let worker_intent = desired_playing.clone();
        let (tx, rx) = mpsc::sync_channel(64);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let status = Arc::new(Mutex::new(Status {
            idle: true,
            paused: true,
            ..Default::default()
        }));
        let shared = status.clone();
        let join = std::thread::spawn(move || unsafe {
            let h = mpv_create();
            if h.is_null() {
                let _ = ready_tx.send(false);
                return;
            }
            for (k, v) in [
                ("config", "no"),
                ("load-scripts", "no"),
                ("ytdl", "no"),
                ("access-references", "no"),
                ("vo", "null"),
                ("vid", "no"),
                ("audio-display", "no"),
                ("idle", "yes"),
                ("pause", "yes"),
                ("ao", output.as_str()),
                ("volume", "100"),
                ("replaygain", "no"),
                ("terminal", "no"),
            ] {
                if mpv_set_option_string(
                    h,
                    CString::new(k).unwrap().as_ptr(),
                    CString::new(v).unwrap().as_ptr(),
                ) < 0
                {
                    mpv_terminate_destroy(h);
                    let _ = ready_tx.send(false);
                    return;
                }
            }
            if mpv_initialize(h) < 0 {
                mpv_terminate_destroy(h);
                let _ = ready_tx.send(false);
                return;
            }
            let _ = ready_tx.send(true);
            let mut current = Status {
                idle: true,
                paused: true,
                ..Default::default()
            };
            let mut entry: Option<i64> = None;
            loop {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Ok(command) => {
                        let loading = matches!(command, Command::Load(..));
                        let args: Vec<String> = match command {
                            Command::Load(g, url) => {
                                current.generation = g;
                                current.error = None;
                                current.position_seconds = None;
                                entry = None;
                                vec!["loadfile".into(), url, "replace".into()]
                            }
                            Command::Pause(p) => vec![
                                "set".into(),
                                "pause".into(),
                                if p || !worker_intent.load(Ordering::Acquire) {
                                    "yes"
                                } else {
                                    "no"
                                }
                                .into(),
                            ],
                            Command::Seek(pos) => {
                                vec!["seek".into(), pos.to_string(), "absolute+exact".into()]
                            }
                            Command::Stop => {
                                entry = None;
                                current.position_seconds = None;
                                vec!["stop".into()]
                            }
                            Command::Quit => unreachable!(),
                        };
                        let strings: Vec<_> =
                            args.into_iter().map(|s| CString::new(s).unwrap()).collect();
                        let mut ptrs: Vec<_> = strings.iter().map(|s| s.as_ptr()).collect();
                        ptrs.push(std::ptr::null());
                        if mpv_command(h, ptrs.as_ptr()) < 0 {
                            current.error = Some("The audio engine rejected a playback command");
                        }
                        if loading {
                            entry = property::<i64>(h, "playlist/0/id", 4);
                        }
                    }
                }
                loop {
                    let event = &*mpv_wait_event(h, 0.0);
                    if event.id == 0 {
                        break;
                    }
                    if event.id == 7 && !event.data.is_null() {
                        let end = &*event.data.cast::<EndFile>();
                        if Some(end.entry) == entry && end.error < 0 {
                            current.error =
                                Some("Playback failed. Check server access and audio output.");
                        }
                    }
                }
                current.idle = property::<c_int>(h, "idle-active", 3).unwrap_or(1) != 0;
                current.paused = property::<c_int>(h, "pause", 3).unwrap_or(1) != 0;
                let before = property::<i64>(h, "playlist/0/id", 4);
                let position =
                    property::<f64>(h, "audio-pts", 5).filter(|p| p.is_finite() && *p >= 0.0);
                let after = property::<i64>(h, "playlist/0/id", 4);
                current.position_seconds = if !current.idle && before == entry && after == entry {
                    position
                } else {
                    None
                };
                *shared.lock().unwrap() = current.clone();
            }
            mpv_terminate_destroy(h);
        });
        if ready_rx.recv_timeout(Duration::from_secs(10)) != Ok(true) {
            return Err("Audio engine could not start");
        }
        Ok(Self {
            tx,
            status,
            desired_playing,
            join: Some(join),
        })
    }
    pub fn send(&self, c: Command) {
        if let Command::Pause(p) = &c {
            self.desired_playing.store(!p, Ordering::Release);
        }
        if self.tx.try_send(c).is_err() {
            self.status.lock().unwrap().error = Some("Playback command queue is busy; retry");
        }
    }
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Quit);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_null_output_load_pause_and_error() {
        let player = Player::with_output("null").unwrap();
        assert!(player.status().idle);
        // A generated silent file can never reach a real audio output in this test.
        let path = std::env::temp_dir().join(format!("qoda-test-{}.wav", uuid::Uuid::new_v4()));
        let samples = 48000_u32 * 2;
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + samples * 2).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16_u32.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(48000_u32.to_le_bytes());
        wav.extend(96000_u32.to_le_bytes());
        wav.extend(2_u16.to_le_bytes());
        wav.extend(16_u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((samples * 2).to_le_bytes());
        wav.resize(44 + samples as usize * 2, 0);
        std::fs::write(&path, wav).unwrap();
        player.send(Command::Load(7, path.to_str().unwrap().into()));
        player.send(Command::Pause(false));
        let until = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let s = player.status();
            if s.generation == 7 && s.position_seconds.is_some() {
                break;
            }
            assert!(std::time::Instant::now() < until, "No nominal clock: {s:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
        player.send(Command::Pause(true));
        std::thread::sleep(Duration::from_millis(150));
        assert!(player.status().paused);
        player.send(Command::Load(8, "/qoda-does-not-exist.wav".into()));
        let until = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let s = player.status();
            if s.generation == 8 && s.error.is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < until,
                "Missing load error: {s:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(player);
        std::fs::remove_file(path).unwrap();
    }
}
