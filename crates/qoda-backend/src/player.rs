//! Native ownership and playlist IDs stay on one thread. Decoder selection is
//! distinct from a positive nominal clock; buffered predecessors can be unknown.
use crate::gain::Applied;
use serde::Serialize;
use std::{
    collections::HashMap,
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
struct StartFile {
    entry: i64,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageResult {
    Ready,
    MissedWindow,
}
pub enum Command {
    #[cfg(test)]
    Load(u64, String, f64),
    LoadGain(u64, String, f64, Applied),
    Pause(bool),
    Stop,
    Quit,
    Stage {
        owner: u64,
        next: u64,
        url: String,
        gain: Option<Applied>,
        reply: mpsc::Sender<Result<StageResult, &'static str>>,
    },
    Unstage {
        owner: u64,
        next: u64,
        reply: mpsc::Sender<Result<Removal, &'static str>>,
    },
    Barrier(mpsc::Sender<Result<Status, &'static str>>),
}
#[derive(Clone, Debug)]
pub enum Removal {
    Untouched,
    Reset(Status),
}
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Status {
    #[serde(serialize_with = "decimal")]
    pub generation: u64,
    pub position_seconds: Option<f64>,
    pub entry_ready: bool,
    pub idle: bool,
    pub natural_eof: bool,
    pub paused: bool,
    pub error: Option<&'static str>,
    #[serde(serialize_with = "optional_decimal")]
    pub staged_generation: Option<u64>,
    #[serde(serialize_with = "optional_decimal")]
    pub handoff_from: Option<u64>,
    pub gain: Option<Applied>,
}
fn decimal<S: serde::Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&value.to_string())
}
fn optional_decimal<S: serde::Serializer>(value: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
    value.map(|v| v.to_string()).serialize(s)
}
pub struct Player {
    tx: mpsc::SyncSender<Command>,
    status: Arc<Mutex<Status>>,
    desired_playing: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}
unsafe fn property<T: Default>(h: *mut c_void, name: &str, format: c_int) -> Option<T> {
    let mut value = T::default();
    let name = CString::new(name).ok()?;
    if unsafe { mpv_get_property(h, name.as_ptr(), format, (&mut value as *mut T).cast()) } < 0 {
        None
    } else {
        Some(value)
    }
}
struct Native {
    h: *mut c_void,
    current: Status,
    entry: Option<i64>,
    entries: HashMap<i64, (u64, Option<Applied>)>,
    future: Option<(i64, u64)>,
    ended: Option<(i64, u64)>,
    intent: Arc<AtomicBool>,
}
impl Native {
    fn command(&self, args: &[String]) -> bool {
        let Ok(strings) = args
            .iter()
            .map(|s| CString::new(s.as_str()))
            .collect::<Result<Vec<_>, _>>()
        else {
            return false;
        };
        let mut ptrs = strings.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();
        ptrs.push(std::ptr::null());
        unsafe { mpv_command(self.h, ptrs.as_ptr()) >= 0 }
    }
    fn call(&self, args: &[&str]) -> bool {
        self.command(&args.iter().map(|s| (*s).into()).collect::<Vec<_>>())
    }
    fn playing(&self) -> Option<i64> {
        let pos = unsafe { property::<i64>(self.h, "playlist-playing-pos", 4) }?;
        if pos < 0 {
            None
        } else {
            unsafe { property(self.h, &format!("playlist/{pos}/id"), 4) }
        }
    }
    fn index(&self, id: i64) -> Option<i64> {
        let count = unsafe { property::<i64>(self.h, "playlist-count", 4) }?;
        (0..count.min(3)).find(
            |pos| unsafe { property::<i64>(self.h, &format!("playlist/{pos}/id"), 4) } == Some(id),
        )
    }
    fn sample(&mut self) {
        let before = self.playing();
        let pts = unsafe { property::<f64>(self.h, "audio-pts", 5) }
            .filter(|p| p.is_finite() && *p >= 0.0);
        self.current.idle =
            unsafe { property::<c_int>(self.h, "idle-active", 3) }.unwrap_or(1) != 0;
        self.current.paused = unsafe { property::<c_int>(self.h, "pause", 3) }.unwrap_or(1) != 0;
        let after = self.playing();
        self.current.entry_ready =
            !self.current.idle && before.is_some() && before == self.entry && after == self.entry;
        self.current.position_seconds = if self.current.entry_ready { pts } else { None };
    }
    fn stop(&mut self) -> bool {
        let stopped = self.call(&["stop"]);
        self.entry = None;
        self.entries.clear();
        self.future = None;
        self.ended = None;
        self.current.natural_eof = false;
        self.current.position_seconds = None;
        self.current.staged_generation = None;
        self.current.handoff_from = None;
        self.current.gain = None;
        stopped
    }
    fn pump(&mut self) {
        loop {
            let event = unsafe { &*mpv_wait_event(self.h, 0.0) };
            if event.id == 0 {
                break;
            }
            if event.id == 6 && !event.data.is_null() {
                let id = unsafe { (*event.data.cast::<StartFile>()).entry };
                if let Some((generation, gain)) = self.entries.get(&id).cloned() {
                    let previous = self.current.generation;
                    let handoff = if generation != previous {
                        if self.ended != self.entry.map(|entry| (entry, previous)) {
                            self.current.error =
                                Some("Unexpected audio transition; playback paused");
                            self.intent.store(false, Ordering::Release);
                            self.call(&["set", "pause", "yes"]);
                            self.stop();
                            continue;
                        }
                        Some(previous)
                    } else {
                        None
                    };
                    self.current = Status {
                        generation,
                        gain,
                        handoff_from: handoff,
                        paused: self.current.paused,
                        ..Default::default()
                    };
                    self.entry = Some(id);
                    if self.future.is_some_and(|(entry, _)| entry == id) {
                        self.future = None;
                    }
                }
            }
            if event.id == 7 && !event.data.is_null() {
                let end = unsafe { &*event.data.cast::<EndFile>() };
                if Some(end.entry) == self.entry {
                    if end.reason == 0 && end.error >= 0 {
                        self.current.natural_eof = true;
                        self.ended = Some((end.entry, self.current.generation));
                    }
                    if end.error < 0 {
                        self.current.error =
                            Some("Playback failed. Check server access and audio output.");
                        self.intent.store(false, Ordering::Release);
                        self.call(&["set", "pause", "yes"]);
                        let handoff = self.current.handoff_from;
                        self.stop();
                        self.current.handoff_from = handoff;
                    }
                }
            }
        }
        self.current.staged_generation = self.future.map(|(_, generation)| generation);
    }
    fn reset(&mut self) -> Result<Status, &'static str> {
        self.pump();
        self.sample();
        let before = self.current.clone();
        self.intent.store(false, Ordering::Release);
        self.call(&["set", "pause", "yes"]);
        if !self.stop() {
            return Err("The audio engine rejected its reset; playback stays paused");
        }
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        loop {
            self.pump();
            self.sample();
            if self.current.idle && self.playing().is_none() {
                return Ok(before);
            }
            if std::time::Instant::now() >= deadline {
                return Err("The audio reset timed out; review playback before retrying");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn load(&mut self, generation: u64, url: String, position: f64, gain: Option<Applied>) {
        let options = match &gain {
            Some(gain) => format!("start={position},{}", gain.filter()),
            None => format!("start={position}"),
        };
        self.entries.clear();
        self.entry = None;
        self.future = None;
        self.ended = None;
        self.current = Status {
            generation,
            paused: true,
            ..Default::default()
        };
        if !self.command(&[
            "loadfile".into(),
            url,
            "replace".into(),
            "-1".into(),
            options,
        ]) {
            self.current.error = Some("The audio engine rejected a playback command");
        } else if let Some(id) = unsafe { property::<i64>(self.h, "playlist/0/id", 4) } {
            self.entries.insert(id, (generation, gain));
        } else {
            self.current.error = Some("The audio engine did not identify the loaded entry");
        }
    }
    fn stage(
        &mut self,
        owner: u64,
        next: u64,
        url: String,
        gain: Option<Applied>,
    ) -> Result<StageResult, &'static str> {
        self.pump();
        self.sample();
        if self.current.generation != owner
            || self.current.idle
            || self.current.error.is_some()
            || self.future.is_some()
            || next <= owner
            || self.playing() != self.entry
            || self.entry.is_none()
        {
            return Ok(StageResult::MissedWindow);
        }
        let entry = self.entry.unwrap();
        if !self.call(&["playlist-clear"]) {
            return Err("The native playlist could not be bounded");
        }
        self.entries.retain(|id, _| *id == entry);
        if !self.command(&[
            "loadfile".into(),
            url,
            "append".into(),
            "-1".into(),
            gain.as_ref()
                .map(|gain| format!("start=0,{}", gain.filter()))
                .unwrap_or_else(|| "start=0".into()),
        ]) {
            return Err("The audio engine rejected next-file preparation");
        }
        let id = unsafe { property::<i64>(self.h, "playlist/1/id", 4) }
            .ok_or("The next native entry was not identified")?;
        self.entries.insert(id, (next, gain));
        self.future = Some((id, next));
        Ok(StageResult::Ready)
    }
    fn unstage(&mut self, owner: u64, next: u64) -> Result<Removal, &'static str> {
        self.pump();
        self.sample();
        if self.current.generation == owner && self.entry.is_some() && self.playing() == self.entry
        {
            if let Some((id, generation)) = self.future {
                if generation != next {
                    return Err("The native future binding changed");
                }
                let index = self
                    .index(id)
                    .ok_or("The native future entry disappeared")?;
                if !self.command(&["playlist-remove".into(), index.to_string()]) {
                    return Err("The next native entry could not be cancelled");
                }
                self.future = None;
                self.pump();
                self.sample();
                if self.current.generation == owner {
                    self.entries.remove(&id);
                }
            }
            if self.current.generation == owner && self.playing() == self.entry {
                return Ok(Removal::Untouched);
            }
        }
        self.reset().map(Removal::Reset)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            mpv_terminate_destroy(self.h);
        }
    }
}
impl Player {
    #[cfg(test)]
    pub(crate) fn test_output(output: &str) -> Result<Self, &'static str> {
        Self::with_output(output)
    }
    pub fn start() -> Result<Self, &'static str> {
        Self::with_output("pipewire")
    }
    fn with_output(output: &str) -> Result<Self, &'static str> {
        Self::with_options(output, &[])
    }
    fn with_options(output: &str, extra: &[(&str, String)]) -> Result<Self, &'static str> {
        let output = output.to_owned();
        let extra = extra
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<Vec<_>>();
        let intent = Arc::new(AtomicBool::new(false));
        let worker_intent = intent.clone();
        let (tx, rx) = mpsc::sync_channel(64);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let status = Arc::new(Mutex::new(Status {
            idle: true,
            paused: true,
            ..Default::default()
        }));
        let shared = status.clone();
        let join = std::thread::spawn(move || {
            let h = unsafe { mpv_create() };
            if h.is_null() {
                let _ = ready_tx.send(false);
                return;
            }
            let mut native = Native {
                h,
                current: Status {
                    idle: true,
                    paused: true,
                    ..Default::default()
                },
                entry: None,
                entries: HashMap::new(),
                future: None,
                ended: None,
                intent: worker_intent,
            };
            let mut options = vec![
                ("config".to_owned(), "no".to_owned()),
                ("load-scripts".into(), "no".into()),
                ("ytdl".into(), "no".into()),
                ("access-references".into(), "no".into()),
                ("vo".into(), "null".into()),
                ("vid".into(), "no".into()),
                ("audio-display".into(), "no".into()),
                ("idle".into(), "yes".into()),
                ("pause".into(), "yes".into()),
                ("ao".into(), output),
                ("volume".into(), "100".into()),
                ("replaygain".into(), "no".into()),
                ("volume-gain".into(), "0".into()),
                ("speed".into(), "1".into()),
                ("audio-swresample-o".into(), "dither_method=none".into()),
                ("terminal".into(), "no".into()),
                ("gapless-audio".into(), "weak".into()),
                ("prefetch-playlist".into(), "yes".into()),
                ("demuxer-max-bytes".into(), "32MiB".into()),
                ("demuxer-max-back-bytes".into(), "8MiB".into()),
            ];
            options.extend(extra);
            for (k, v) in options {
                if unsafe {
                    mpv_set_option_string(
                        h,
                        CString::new(k).unwrap().as_ptr(),
                        CString::new(v).unwrap().as_ptr(),
                    )
                } < 0
                {
                    let _ = ready_tx.send(false);
                    return;
                }
            }
            if unsafe { mpv_initialize(h) } < 0 {
                let _ = ready_tx.send(false);
                return;
            }
            let _ = ready_tx.send(true);
            loop {
                native.pump();
                native.sample();
                *shared.lock().unwrap() = native.current.clone();
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Ok(Command::Pause(p)) => {
                        let pause = p || !native.intent.load(Ordering::Acquire);
                        if !native.call(&["set", "pause", if pause { "yes" } else { "no" }]) {
                            native.current.error = Some("The audio engine rejected pause");
                        }
                    }
                    Ok(Command::Stop) => {
                        if !native.stop() {
                            native.current.error = Some("The audio engine rejected stop");
                        }
                    }
                    #[cfg(test)]
                    Ok(Command::Load(generation, url, position)) => {
                        native.load(generation, url, position, None)
                    }
                    Ok(Command::LoadGain(generation, url, position, gain)) => {
                        native.load(generation, url, position, Some(gain))
                    }
                    Ok(Command::Stage {
                        owner,
                        next,
                        url,
                        gain,
                        reply,
                    }) => {
                        let result = native.stage(owner, next, url, gain);
                        native.pump();
                        native.sample();
                        *shared.lock().unwrap() = native.current.clone();
                        let _ = reply.send(result);
                    }
                    Ok(Command::Unstage { owner, next, reply }) => {
                        let result = native.unstage(owner, next);
                        native.pump();
                        native.sample();
                        *shared.lock().unwrap() = native.current.clone();
                        let _ = reply.send(result);
                    }
                    Ok(Command::Barrier(reply)) => {
                        let result = native.reset();
                        *shared.lock().unwrap() = native.current.clone();
                        let _ = reply.send(result);
                    }
                }
            }
        });
        if ready_rx.recv_timeout(Duration::from_secs(10)) != Ok(true) {
            return Err("Audio engine could not start");
        }
        Ok(Self {
            tx,
            status,
            desired_playing: intent,
            join: Some(join),
        })
    }
    pub fn send(&self, command: Command) {
        if let Command::Pause(p) = &command {
            self.desired_playing.store(!p, Ordering::Release);
        }
        if self.tx.try_send(command).is_err() {
            self.status.lock().unwrap().error = Some("Playback command queue is busy; retry");
        }
    }
    async fn reply<T: Send + 'static>(
        &self,
        make: impl FnOnce(mpsc::Sender<Result<T, &'static str>>) -> Command,
    ) -> Result<T, &'static str> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .try_send(make(tx))
            .map_err(|_| "The native command queue is busy")?;
        tokio::task::spawn_blocking(move || {
            rx.recv_timeout(Duration::from_secs(2))
                .map_err(|_| "The native acknowledgement timed out")?
        })
        .await
        .map_err(|_| "The native acknowledgement worker failed")?
    }
    #[cfg(test)]
    pub async fn stage(
        &self,
        owner: u64,
        next: u64,
        url: String,
    ) -> Result<StageResult, &'static str> {
        self.reply(|reply| Command::Stage {
            owner,
            next,
            url,
            gain: None,
            reply,
        })
        .await
    }
    pub async fn stage_gain(
        &self,
        owner: u64,
        next: u64,
        url: String,
        gain: Applied,
    ) -> Result<StageResult, &'static str> {
        self.reply(|reply| Command::Stage {
            owner,
            next,
            url,
            gain: Some(gain),
            reply,
        })
        .await
    }
    pub async fn unstage(&self, owner: u64, next: u64) -> Result<Removal, &'static str> {
        self.reply(|reply| Command::Unstage { owner, next, reply })
            .await
    }
    pub async fn barrier(&self) -> Result<Status, &'static str> {
        self.desired_playing.store(false, Ordering::Release);
        self.reply(Command::Barrier).await
    }
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Quit);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn silent_file(seconds: u32) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("qoda-native-{}.wav", uuid::Uuid::new_v4()));
        let samples = 48000_u32 * seconds;
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
        path
    }
    pub(super) fn wait_for(player: &Player, predicate: impl Fn(&Status) -> bool) -> Status {
        let until = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let status = player.status();
            if predicate(&status) {
                return status;
            }
            assert!(
                std::time::Instant::now() < until,
                "Native state timed out: {status:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[test]
    fn restored_position_is_applied_before_unpause() {
        let player = Player::with_output("null").unwrap();
        let file = silent_file(4);
        player.send(Command::Load(20, file.to_str().unwrap().into(), 2.0));
        wait_for(&player, |s| s.generation == 20 && !s.idle && s.paused);
        assert!(player.status().paused);
        player.send(Command::Pause(false));
        let status = wait_for(&player, |s| {
            s.generation == 20 && s.position_seconds.is_some()
        });
        assert!(
            status.position_seconds.unwrap() >= 1.8,
            "Resume loaded from zero: {status:?}"
        );
        drop(player);
        std::fs::remove_file(file).unwrap();
    }
    #[test]
    fn natural_end_is_generation_scoped_and_stop_is_not_completion() {
        let player = Player::with_output("null").unwrap();
        let file = silent_file(1);
        player.send(Command::Load(10, file.to_str().unwrap().into(), 0.0));
        player.send(Command::Pause(false));
        wait_for(&player, |s| s.generation == 10 && s.idle && s.natural_eof);
        assert!(player.status().error.is_none());
        player.send(Command::Pause(true));
        player.send(Command::Load(11, file.to_str().unwrap().into(), 0.0));
        wait_for(&player, |s| s.generation == 11 && !s.idle && s.paused);
        assert!(!player.status().natural_eof);
        player.send(Command::Stop);
        wait_for(&player, |s| s.idle);
        assert!(!player.status().natural_eof);
        // A second native visit can complete; a stale previous EOF is not reused.
        player.send(Command::Load(12, file.to_str().unwrap().into(), 0.0));
        player.send(Command::Pause(false));
        wait_for(&player, |s| s.generation == 12 && s.idle && s.natural_eof);
        drop(player);
        std::fs::remove_file(file).unwrap();
    }
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
        player.send(Command::Load(7, path.to_str().unwrap().into(), 0.0));
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
        player.send(Command::Load(8, "/qoda-does-not-exist.wav".into(), 0.0));
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

#[cfg(test)]
mod gapless_tests {
    use super::*;
    fn pattern_file(seed: i16, seconds: usize) -> (std::path::PathBuf, Vec<u8>) {
        let path = std::env::temp_dir().join(format!("qoda-gapless-{}.wav", uuid::Uuid::new_v4()));
        let pcm = (0..48_000 * seconds)
            .flat_map(|i| (((i % 997) as i16 - 498) * 20 + seed).to_le_bytes())
            .collect::<Vec<_>>();
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + pcm.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16_u32.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(1_u16.to_le_bytes());
        wav.extend(48000_u32.to_le_bytes());
        wav.extend(96000_u32.to_le_bytes());
        wav.extend(2_u16.to_le_bytes());
        wav.extend(16_u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((pcm.len() as u32).to_le_bytes());
        wav.extend(&pcm);
        std::fs::write(&path, wav).unwrap();
        (path, pcm)
    }
    fn wait(player: &Player, predicate: impl Fn(&Status) -> bool) -> Status {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let status = player.status();
            if predicate(&status) {
                return status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Gapless native timeout: {status:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    // Named boundary: ao=pcm, 44.1/48/96 kHz, stereo signed 32-bit, no dither.
    // This is player-side digital capture, not ALSA/DAC qualification.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn per_file_double_gain_and_unity_transitions_preserve_sample_count_within_one_output_lsb()
     {
        for rate in [44100_u32, 48000, 96000] {
            for flac in [false, true] {
                for bits in [16u16, 24] {
                    for next_db in [0.0, -12.0, 20.0 * 2_f64.log10()] {
                        let mut files = Vec::new();
                        let mut expected = Vec::new();
                        for (index, db) in [-6.0, next_db].into_iter().enumerate() {
                            let path = std::env::temp_dir()
                                .join(format!("qoda-gain-{}.wav", uuid::Uuid::new_v4()));
                            let mut pcm = Vec::new();
                            for sample in 0..rate * 4 * 2 {
                                let value = ((sample % 997) as i32 - 498)
                                    * if bits == 16 { 20 } else { 4000 }
                                    + index as i32 * 123;
                                if bits == 16 {
                                    pcm.extend((value as i16).to_le_bytes());
                                } else {
                                    pcm.extend(&value.to_le_bytes()[..3]);
                                }
                                let decoded = value * if bits == 16 { 65536 } else { 256 };
                                expected.push(
                                    (f64::from(decoded) * 10_f64.powf(db / 20.0)).round_ties_even()
                                        as i32,
                                );
                            }
                            let mut wav = b"RIFF".to_vec();
                            wav.extend((36 + pcm.len() as u32).to_le_bytes());
                            wav.extend(b"WAVEfmt ");
                            wav.extend(16_u32.to_le_bytes());
                            wav.extend(1_u16.to_le_bytes());
                            wav.extend(2_u16.to_le_bytes());
                            wav.extend(rate.to_le_bytes());
                            wav.extend((rate * 2 * u32::from(bits) / 8).to_le_bytes());
                            wav.extend((2 * bits / 8).to_le_bytes());
                            wav.extend(bits.to_le_bytes());
                            wav.extend(b"data");
                            wav.extend((pcm.len() as u32).to_le_bytes());
                            wav.extend(pcm);
                            std::fs::write(&path, wav).unwrap();
                            let path = if flac {
                                let encoded = path.with_extension("flac");
                                let result = std::process::Command::new("/usr/bin/ffmpeg")
                                    .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                                    .arg(&path)
                                    .args([
                                        "-c:a",
                                        "flac",
                                        "-metadata",
                                        "REPLAYGAIN_TRACK_GAIN=12 dB",
                                        "-metadata",
                                        "REPLAYGAIN_TRACK_PEAK=0.8",
                                    ])
                                    .arg(&encoded)
                                    .output()
                                    .unwrap();
                                assert!(result.status.success());
                                std::fs::remove_file(path).unwrap();
                                encoded
                            } else {
                                path
                            };
                            files.push(path);
                        }
                        let capture = std::env::temp_dir()
                            .join(format!("qoda-gain-capture-{}.pcm", uuid::Uuid::new_v4()));
                        let diagnostic = capture.with_extension("log");
                        let player = Player::with_options(
                            "pcm",
                            &[
                                ("log-file", diagnostic.to_str().unwrap().into()),
                                ("ao-pcm-file", capture.to_str().unwrap().into()),
                                ("ao-pcm-waveheader", "no".into()),
                                ("ao-pcm-append", "yes".into()),
                                ("audio-format", "s32".into()),
                                ("audio-samplerate", rate.to_string()),
                                ("audio-channels", "stereo".into()),
                            ],
                        )
                        .unwrap();
                        let gain = |db| Applied {
                            mode: crate::gain::Mode::Track,
                            basis: "track",
                            db,
                            limited: false,
                            reason: "Synthetic capture",
                        };
                        player.send(Command::LoadGain(
                            1,
                            files[0].to_str().unwrap().into(),
                            0.0,
                            gain(-6.0),
                        ));
                        wait(&player, |s| {
                            s.generation == 1 && s.entry_ready && !s.idle && s.paused
                        });
                        assert_eq!(
                            player
                                .stage_gain(1, 2, files[1].to_str().unwrap().into(), gain(next_db))
                                .await
                                .unwrap(),
                            StageResult::Ready
                        );
                        player.send(Command::Pause(false));
                        let status =
                            wait(&player, |s| s.generation == 2 && s.idle && s.natural_eof);
                        assert_eq!(status.gain.as_ref().unwrap().db, next_db);
                        assert_eq!(status.handoff_from, Some(1));
                        assert!(status.error.is_none());
                        drop(player);
                        let bytes = std::fs::read(&capture).unwrap();
                        assert_eq!(
                            bytes.len(),
                            expected.len() * 4,
                            "sample count changed: {}",
                            std::fs::read_to_string(&diagnostic).unwrap_or_default()
                        );
                        for (index, (raw, reference)) in
                            bytes.as_chunks::<4>().0.iter().zip(expected).enumerate()
                        {
                            let actual = i32::from_le_bytes(*raw);
                            let tolerance = if next_db == 0.0 && index >= rate as usize * 4 * 2 {
                                0
                            } else {
                                1
                            };
                            assert!(
                                (i64::from(actual) - i64::from(reference)).abs() <= tolerance,
                                "{bits}-bit, next={next_db}, sample={index}: actual {actual}, reference {reference}; {}",
                                std::fs::read_to_string(&diagnostic).unwrap_or_default()
                            );
                        }
                        for path in files.into_iter().chain([capture, diagnostic]) {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                }
            }
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn compatible_native_chain_emits_exact_concatenated_pcm_without_inserted_or_lost_samples()
    {
        for flac in [false, true] {
            // ao=pcm advertises a 65536-sample buffer. A one-second file can
            // reach EOF while still paused, before the test resumes it. Keep
            // each fixture longer than that buffer so staging cannot consume A.
            let (a, mut expected) = pattern_file(123, 4);
            let (b, second) = pattern_file(567, 4);
            let join_offset = expected.len();
            let originals = [a.clone(), b.clone()];
            let (a, b) = if flac {
                let first = a.with_extension("flac");
                let second = b.with_extension("flac");
                for (input, output) in [(&a, &first), (&b, &second)] {
                    let result = std::process::Command::new("/usr/bin/ffmpeg")
                        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                        .arg(input)
                        .args(["-c:a", "flac"])
                        .arg(output)
                        .output()
                        .unwrap();
                    assert!(result.status.success(), "Synthetic FLAC encoding failed");
                }
                (first, second)
            } else {
                (a, b)
            };
            expected.extend(second);
            let capture = std::env::temp_dir()
                .join(format!("qoda-gapless-capture-{}.pcm", uuid::Uuid::new_v4()));
            let diagnostic = capture.with_extension("log");
            let player = Player::with_options(
                "pcm",
                &[
                    ("log-file", diagnostic.to_str().unwrap().into()),
                    ("ao-pcm-file", capture.to_str().unwrap().into()),
                    ("ao-pcm-waveheader", "no".into()),
                    ("ao-pcm-append", "yes".into()),
                    ("audio-format", "s16".into()),
                    ("audio-samplerate", "48000".into()),
                    ("audio-channels", "mono".into()),
                    ("af", "lavfi=[arealtime]".into()),
                ],
            )
            .unwrap();
            player.send(Command::Load(1, a.to_str().unwrap().into(), 0.0));
            wait(&player, |s| {
                s.generation == 1 && !s.idle && s.paused && s.entry_ready
            });
            // Give the untimed sink time to fill while paused, as on a busy CI
            // runner. It must still own A when staging and resuming begin.
            std::thread::sleep(Duration::from_secs(2));
            let paused = player.status();
            assert_eq!(paused.generation, 1);
            assert!(paused.paused && !paused.idle && !paused.natural_eof);
            assert_eq!(
                player
                    .stage(1, 2, b.to_str().unwrap().into())
                    .await
                    .unwrap(),
                StageResult::Ready
            );
            player.send(Command::Pause(false));
            let final_status = wait(&player, |s| s.generation == 2 && s.idle && s.natural_eof);
            assert_eq!(final_status.handoff_from, Some(1));
            assert!(final_status.error.is_none());
            drop(player);
            let actual = std::fs::read(&capture).unwrap();
            if actual != expected {
                eprintln!(
                    "Capture mismatch: flac={flac}, bytes={}, prefix={:?}; expected prefix={:?}, second prefix={:?}\n{}",
                    actual.len(),
                    actual.get(..16),
                    expected.get(..16),
                    expected.get(join_offset..join_offset + 16),
                    std::fs::read_to_string(&diagnostic).unwrap_or_default()
                );
            }
            std::fs::remove_file(diagnostic).unwrap();
            assert_eq!(
                actual.len(),
                expected.len(),
                "Gapless capture length changed"
            );
            assert_eq!(
                actual, expected,
                "Gapless decoded output differed at or around the join"
            );
            for file in [a, b, capture] {
                std::fs::remove_file(file).unwrap();
            }
            if flac {
                for file in originals {
                    std::fs::remove_file(file).unwrap();
                }
            }
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn removing_an_untouched_native_future_keeps_the_current_file_and_pause_intent() {
        let (a, _) = pattern_file(123, 1);
        let (b, _) = pattern_file(567, 1);
        let player = Player::with_output("null").unwrap();
        player.send(Command::Load(10, a.to_str().unwrap().into(), 0.0));
        wait(&player, |s| {
            s.generation == 10 && !s.idle && s.paused && s.entry_ready
        });
        assert_eq!(
            player
                .stage(10, 11, b.to_str().unwrap().into())
                .await
                .unwrap(),
            StageResult::Ready
        );
        assert!(matches!(
            player.unstage(10, 11).await.unwrap(),
            Removal::Untouched
        ));
        let status = player.status();
        assert_eq!(status.generation, 10);
        assert!(status.paused);
        assert_eq!(status.staged_generation, None);
        player.send(Command::Pause(false));
        let status = wait(&player, |s| s.idle && s.natural_eof);
        assert_eq!(status.generation, 10);
        drop(player);
        for file in [a, b] {
            std::fs::remove_file(file).unwrap();
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_after_decoder_handoff_returns_a_reset_barrier_instead_of_false_future_ack()
     {
        let (a, _) = pattern_file(123, 1);
        let (b, _) = pattern_file(567, 1);
        let player = Player::with_output("null").unwrap();
        player.send(Command::Load(20, a.to_str().unwrap().into(), 0.0));
        wait(&player, |s| {
            s.generation == 20 && !s.idle && s.paused && s.entry_ready
        });
        player
            .stage(20, 21, b.to_str().unwrap().into())
            .await
            .unwrap();
        player.send(Command::Pause(false));
        wait(&player, |s| s.generation == 21 && !s.idle);
        let Removal::Reset(before) = player.unstage(20, 21).await.unwrap() else {
            panic!("A decoded future was incorrectly acknowledged untouched");
        };
        assert_eq!(before.generation, 21);
        let stopped = player.status();
        assert!(stopped.paused && stopped.idle);
        assert!(!stopped.natural_eof);
        assert_eq!(stopped.staged_generation, None);
        drop(player);
        for file in [a, b] {
            std::fs::remove_file(file).unwrap();
        }
    }
}

#[cfg(test)]
mod successor_failure_tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_successor_keeps_its_handoff_binding_and_stops_without_skipping_to_another_track()
     {
        let file = super::tests::silent_file(1);
        let player = Player::with_output("null").unwrap();
        player.send(Command::Load(40, file.to_str().unwrap().into(), 0.0));
        super::tests::wait_for(&player, |s| s.generation == 40 && s.entry_ready && s.paused);
        assert_eq!(
            player
                .stage(40, 41, "/qoda-missing-gapless-successor.wav".into())
                .await
                .unwrap(),
            StageResult::Ready
        );
        player.send(Command::Pause(false));
        let status = super::tests::wait_for(&player, |s| s.generation == 41 && s.error.is_some());
        assert_eq!(status.handoff_from, Some(40));
        assert!(!status.natural_eof);
        drop(player);
        std::fs::remove_file(file).unwrap();
    }
}
