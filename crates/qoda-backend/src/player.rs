//! Native ownership and playlist IDs stay on one thread. Decoder selection is
//! distinct from a positive nominal clock; buffered predecessors can be unknown.
use crate::continuous::{self, Description};
use crate::gain::Applied;
use crate::{alsa_guard, output, source_format::Format};
use serde::Serialize;
use std::{
    collections::HashMap,
    ffi::{CString, c_char, c_int, c_void},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
struct LogMessage {
    prefix: *const c_char,
    level: *const c_char,
    text: *const c_char,
    log_level: c_int,
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
    fn mpv_request_log_messages(h: *mut c_void, level: *const c_char) -> c_int;
    fn mpv_free(data: *mut c_void);
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
    Independent(&'static str),
}
pub struct Load {
    pub timeline_offset: f64,
    pub transcoded: bool,
    pub generation: u64,
    pub url: String,
    pub position: f64,
    pub gain: Applied,
    pub source: Option<Format>,
    pub description: Option<crate::continuous::Description>,
    pub chain: bool,
    pub output: Option<output::Selection>,
    pub playing: bool,
}
impl Load {
    fn local_position(&self) -> Result<f64, &'static str> {
        if !self.timeline_offset.is_finite()
            || !self.position.is_finite()
            || self.timeline_offset < 0.0
            || self.position < self.timeline_offset
            || (!self.transcoded && self.timeline_offset != 0.0)
        {
            return Err("Invalid source timeline offset; playback stays paused.");
        }
        Ok(self.position - self.timeline_offset)
    }
}
pub enum Command {
    #[cfg(test)]
    Load(u64, String, f64),
    #[cfg(test)]
    LoadGain(u64, String, f64, Applied),
    #[cfg(test)]
    LoadProcessed(u64, String, Applied, output::Plan),
    #[cfg(test)]
    StageProcessed(
        u64,
        String,
        Applied,
        output::Plan,
        mpsc::Sender<Result<StageResult, &'static str>>,
    ),
    #[cfg(test)]
    LoadContinuous(u64, String, Applied, Description, output::Plan),
    #[cfg(test)]
    StageContinuous(
        u64,
        u64,
        String,
        Applied,
        Description,
        mpsc::Sender<Result<StageResult, &'static str>>,
    ),
    LoadPrepared(Load),
    Configure(
        Option<output::Selection>,
        mpsc::Sender<Result<(), &'static str>>,
    ),
    FinishChain(u64),
    Pause(bool),
    Stop,
    Quit,
    Stage {
        owner: u64,
        next: u64,
        url: String,
        gain: Option<Applied>,
        source: Option<Format>,
        description: Option<crate::continuous::Description>,
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
    Reset(Box<Status>),
}
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Status {
    pub transcoded: bool,
    pub decoder_codec: Option<String>,
    pub decoder_layout: Option<String>,
    pub decoder_format: Option<String>,
    pub unsupported_format: bool,
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
    pub source: Option<Format>,
    pub output: Option<alsa_guard::Observed>,
    pub output_selection: Option<output::Selection>,
    pub processing: Option<output::Plan>,
    pub continuous: bool,
    pub chain_fallback: Option<&'static str>,
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
    latest_load: Arc<AtomicU64>,
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
fn text_property(h: *mut c_void, name: &str) -> Option<String> {
    let ptr = unsafe { property::<*mut c_char>(h, name, 1) }?;
    if ptr.is_null() {
        return None;
    }
    let value = unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe {
        mpv_free(ptr.cast());
    }
    Some(value)
}
fn decoded_format(h: *mut c_void, hint: Option<Format>) -> Option<Format> {
    use crate::source_format::SampleType;
    let rate = u32::try_from(unsafe { property::<i64>(h, "audio-params/samplerate", 4) }?).ok()?;
    let channels =
        u16::try_from(unsafe { property::<i64>(h, "audio-params/channel-count", 4) }?).ok()?;
    let layout = text_property(h, "audio-params/channels")?;
    let format = text_property(h, "audio-params/format")?;
    let (sample_type, mut bits) = match format.trim_end_matches('p') {
        "u8" => (SampleType::Integer, 8),
        "s16" => (SampleType::Integer, 16),
        "s32" => (SampleType::Integer, 32),
        "s64" => (SampleType::Integer, 64),
        "float" => (SampleType::Float, 32),
        "double" => (SampleType::Float, 64),
        _ => return None,
    };
    let stereo = channels == 2 && layout == "stereo";
    if let Some(hint) = hint.filter(|h| {
        h.rate == rate
            && h.channels == channels
            && h.stereo == stereo
            && h.sample_type == sample_type
            && h.bits <= bits
    }) {
        bits = hint.bits;
    }
    (rate > 0 && channels > 0).then_some(Format {
        rate,
        bits,
        sample_type,
        channels,
        stereo,
    })
}
struct PendingProbe {
    load: Load,
    deadline: std::time::Instant,
}
#[derive(Clone)]
struct Entry {
    generation: u64,
    gain: Option<Applied>,
    source: Option<Format>,
}
#[derive(Clone, Copy)]
struct PendingStart {
    target: f64,
    loaded: bool,
    deadline: std::time::Instant,
}
struct Active {
    plan: output::Plan,
}
fn create_handle(output: &str, extra: &[(String, String)]) -> Result<*mut c_void, &'static str> {
    let h = unsafe { mpv_create() };
    if h.is_null() {
        return Err("Audio engine could not start");
    }
    let configured = (|| {
        let mut options = vec![
            ("config".to_owned(), "no".to_owned()),
            ("load-scripts".into(), "no".into()),
            ("ytdl".into(), "no".into()),
            ("access-references".into(), "no".into()),
            ("network-timeout".into(), "10".into()),
            ("vo".into(), "null".into()),
            ("vid".into(), "no".into()),
            ("audio-display".into(), "no".into()),
            ("audio-fallback-to-null".into(), "no".into()),
            ("idle".into(), "yes".into()),
            ("pause".into(), "yes".into()),
            ("ao".into(), output.to_owned()),
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

        options.extend(extra.iter().cloned());
        for (key, value) in options {
            let key = CString::new(key).map_err(|_| "Invalid audio configuration")?;
            let value = CString::new(value).map_err(|_| "Invalid audio configuration")?;
            if unsafe { mpv_set_option_string(h, key.as_ptr(), value.as_ptr()) } < 0 {
                return Err("Audio engine rejected output configuration");
            }
        }
        if unsafe { mpv_initialize(h) } < 0 {
            return Err("Audio engine could not initialize output");
        }
        let level = CString::new("warn").unwrap();
        let _ = unsafe { mpv_request_log_messages(h, level.as_ptr()) };
        Ok(())
    })();
    if let Err(error) = configured {
        unsafe { mpv_terminate_destroy(h) };
        return Err(error);
    }
    Ok(h)
}
struct PcmActive {
    session: continuous::Session,
    pending: Option<Entry>,
}
struct Native {
    h: *mut c_void,
    current: Status,
    entry: Option<i64>,
    entries: HashMap<i64, Entry>,
    desktop_output: String,
    extra: Vec<(String, String)>,
    route: Option<output::Selection>,
    active: Option<Active>,
    pending_start: Option<PendingStart>,
    probe: Option<PendingProbe>,
    unsupported_decoder: bool,
    transcoded: bool,
    timeline_offset: f64,
    pcm: Option<PcmActive>,
    registration: Option<continuous::input::Registration>,
    future: Option<(i64, u64)>,
    ended: Option<(i64, u64)>,
    intent: Arc<AtomicBool>,
    latest_load: Arc<AtomicU64>,
}
fn qualified(d: Description, source: Option<Format>) -> bool {
    source
        == Some(Format {
            rate: d.rate,
            bits: d.bits,
            ..Default::default()
        })
        && d.frames >= u64::from(d.rate) * 2
}
fn file_options(position: f64, gain: Option<&Applied>, plan: Option<output::Plan>) -> String {
    let filter = match plan {
        Some(plan) => Some(plan.filter(gain)),
        None => gain.map(Applied::filter),
    };
    // Do not request even a zero seek on the nonseekable continuous input.
    // Ordinary fresh files also start at zero without an explicit start option.
    let mut options = if position > 0.0 {
        vec![format!("start={position}")]
    } else {
        Vec::new()
    };
    if let Some(filter) = filter {
        let value = filter.strip_prefix("af=").unwrap_or(&filter);
        options.push(format!("af=%{}%{value}", value.len()));
    }
    options.join(",")
}

impl Native {
    fn replace_handle(&mut self, source: Option<Format>) -> Result<(), &'static str> {
        self.cancel_pcm();
        let old = std::mem::replace(&mut self.h, std::ptr::null_mut());
        if !old.is_null() {
            unsafe { mpv_terminate_destroy(old) };
        }
        self.registration = None;
        alsa_guard::disable();
        self.active = None;
        self.pending_start = None;
        self.entry = None;
        self.entries.clear();
        self.future = None;
        self.ended = None;
        let mut extra = self.extra.clone();
        let driver = match &self.route {
            Some(output::Selection::Desktop) => self.desktop_output.as_str(),
            Some(selection) if selection.fidelity() => {
                if let Some(source) = source {
                    let device = selection.resolve()?.ok_or("No fidelity output selected")?;
                    let plan = output::preflight(
                        &device,
                        source,
                        selection.conversion_allowed(),
                        selection.assumes_stereo(),
                    )?;
                    if selection.assumes_stereo() {
                        alsa_guard::arm_with_stereo_policy(
                            device.clone(),
                            plan.target,
                            plan.sample,
                            true,
                        )?;
                    } else {
                        alsa_guard::arm(device.clone(), plan.target, plan.sample)?;
                    }
                    extra.extend([
                        ("audio-device".into(), format!("alsa/{}", device.route())),
                        ("alsa-resample".into(), "no".into()),
                        ("alsa-non-interleaved".into(), "no".into()),
                        (
                            "alsa-ignore-chmap".into(),
                            if selection.assumes_stereo() {
                                "yes"
                            } else {
                                "no"
                            }
                            .into(),
                        ),
                        ("audio-format".into(), plan.sample.mpv().into()),
                        ("audio-samplerate".into(), plan.target.rate.to_string()),
                        ("audio-channels".into(), "stereo".into()),
                    ]);
                    self.active = Some(Active { plan });
                    "alsa"
                } else {
                    "null"
                }
            }
            _ => {
                if source.is_some() {
                    return Err("Output settings unavailable; playback stays paused.");
                }
                "null"
            }
        };
        self.h = create_handle(driver, &extra)?;
        Ok(())
    }
    fn configure(&mut self, route: Option<output::Selection>) -> Result<(), &'static str> {
        let reset = self.reset();
        // Retain the requested route even after a reset failure. A later load
        // must never silently run through the previous Desktop output.
        self.route = route;
        let configured = self.replace_handle(None);
        self.current = Status {
            idle: true,
            paused: true,
            output_selection: if self.h.is_null() {
                None
            } else {
                self.route.clone()
            },
            ..Default::default()
        };
        reset.and(configured)
    }
    fn command(&self, args: &[String]) -> bool {
        if self.h.is_null() {
            return false;
        }
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
        if self.h.is_null() {
            return None;
        }
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
        if self.h.is_null() {
            self.current.idle = true;
            self.current.paused = true;
            self.current.entry_ready = false;
            self.current.position_seconds = None;
            self.current.output = None;
            self.current.output_selection = None;
            return;
        }
        if self.active.is_some()
            && let Some(error) = alsa_guard::error()
            && self.current.error != Some(error)
        {
            self.current.error = Some(error);
            self.intent.store(false, Ordering::Release);
            self.call(&["set", "pause", "yes"]);
            self.stop();
        }
        self.current.output_selection = self.route.clone();
        self.current.transcoded = self.transcoded;
        if !self.intent.load(Ordering::Acquire)
            && unsafe { property::<c_int>(self.h, "pause", 3) } == Some(0)
        {
            self.call(&["set", "pause", "yes"]);
        }
        if let Some(start) = self.pending_start {
            self.call(&["set", "pause", "yes"]);
            let position = if start.loaded && self.playing() == self.entry {
                unsafe { property::<f64>(self.h, "time-pos", 5) }
            } else {
                None
            };
            if position.is_some_and(|position| {
                position.is_finite()
                    && position >= start.target - 0.05
                    && position <= start.target + 0.25
            }) {
                self.pending_start = None;
                if self.intent.load(Ordering::Acquire) && self.current.error.is_none() {
                    self.call(&["set", "pause", "no"]);
                }
            } else if std::time::Instant::now() >= start.deadline {
                self.current.error = Some(
                    "Resume position could not be verified; source may not support seeking. Playback stays paused.",
                );
                self.intent.store(false, Ordering::Release);
                self.stop();
            }
        }
        let before = self.playing();
        let pts = unsafe { property::<f64>(self.h, "audio-pts", 5) }
            .filter(|p| p.is_finite() && *p >= 0.0);
        self.current.idle =
            unsafe { property::<c_int>(self.h, "idle-active", 3) }.unwrap_or(1) != 0;
        self.current.paused = unsafe { property::<c_int>(self.h, "pause", 3) }.unwrap_or(1) != 0;
        let after = self.playing();
        self.current.entry_ready =
            !self.current.idle && before.is_some() && before == self.entry && after == self.entry;
        self.current.output =
            if self.active.is_some() && self.current.entry_ready && self.current.error.is_none() {
                alsa_guard::observed()
            } else {
                None
            };
        if self.current.entry_ready {
            self.current.decoder_codec = text_property(self.h, "audio-codec-name");
            self.current.decoder_layout = text_property(self.h, "audio-params/channels");
            self.current.decoder_format = text_property(self.h, "audio-params/format");
            if self.current.source.is_none() {
                self.current.source = decoded_format(self.h, None);
            }
        }
        if self.probe.is_some() {
            self.call(&["set", "pause", "yes"]);
            let stale = self
                .probe
                .as_ref()
                .is_some_and(|p| self.latest_load.load(Ordering::Acquire) != p.load.generation);
            if stale {
                self.stop();
                return;
            }
            let facts = decoded_format(self.h, self.probe.as_ref().and_then(|p| p.load.source));
            if let Some(facts) = facts {
                let mut probe = self.probe.take().unwrap();
                probe.load.source = Some(facts);
                // A verified header span remains eligible only if decoder facts agree.
                if probe
                    .load
                    .description
                    .is_some_and(|d| !qualified(d, Some(facts)))
                {
                    probe.load.description = None;
                }
                let gain = Some(probe.load.gain.clone());
                self.load_with_gain(probe.load, gain);
                if self.intent.load(Ordering::Acquire)
                    && self.current.error.is_none()
                    && self.pending_start.is_none()
                {
                    self.call(&["set", "pause", "no"]);
                }
                return;
            }
            if self.current.error.is_some()
                || self
                    .probe
                    .as_ref()
                    .is_some_and(|p| std::time::Instant::now() >= p.deadline)
            {
                self.probe = None;
                if self.current.error.is_none() {
                    self.current.error = Some(
                        "The decoder did not provide a verified audio format; playback stays paused.",
                    );
                }
                self.intent.store(false, Ordering::Release);
                self.stop();
            }
            self.current.entry_ready = false;
            self.current.position_seconds = None;
            return;
        }
        self.current.processing = if self.current.entry_ready && self.current.error.is_none() {
            self.active.as_ref().and_then(|active| {
                self.current.source.map(|source| output::Plan {
                    source,
                    ..active.plan
                })
            })
        } else {
            None
        };
        self.current.position_seconds = if self.current.entry_ready && self.pending_start.is_none()
        {
            pts.filter(|p| self.timeline_offset == 0.0 || *p > 0.0)
                .map(|p| p + self.timeline_offset)
        } else {
            None
        };
        if let Some(pcm) = &mut self.pcm {
            pcm.session.poll();
            self.current.continuous = true;
            self.current.chain_fallback = pcm.session.stream.fallback();
            let result = if let Some(error) = pcm.session.stream.error() {
                Err(error)
            } else if self
                .registration
                .as_ref()
                .is_some_and(continuous::input::Registration::cancelled)
            {
                Err("Continuous input was interrupted; playback stays paused")
            } else if self.current.entry_ready && self.pending_start.is_none() {
                pts.filter(|p| *p > 0.0)
                    .map(|p| pcm.session.observe(p))
                    .transpose()
            } else {
                Ok(None)
            };
            match result {
                Ok(Some(observation)) => {
                    if observation.generation != self.current.generation {
                        if let Some(entry) = pcm
                            .pending
                            .take()
                            .filter(|e| e.generation == observation.generation)
                        {
                            self.current.generation = entry.generation;
                            self.current.gain = entry.gain.clone();
                            self.current.source = entry.source;
                            if let Some(id) = self.entry {
                                self.entries.insert(id, entry);
                            }
                        } else {
                            self.current.error =
                                Some("Continuous queue binding changed; playback paused");
                        }
                    }
                    self.current.position_seconds = observation.position;
                    self.current.handoff_from = observation.handoff_from;
                    self.current.staged_generation = observation.staged_generation;
                }
                Ok(None) => self.current.position_seconds = None,
                Err(error) => self.current.error = Some(error),
            }
            if self.current.error.is_some() {
                self.intent.store(false, Ordering::Release);
                self.call(&["set", "pause", "yes"]);
                self.stop();
            }
        }
    }
    fn cancel_pcm(&mut self) {
        if let Some(pcm) = self.pcm.take() {
            pcm.session.cancel();
        }
        if let Some(registration) = &self.registration {
            registration.cancel();
        }
    }
    fn finish_chain(&self, owner: u64) {
        if let Some(pcm) = &self.pcm
            && pcm.session.stream.owner() == Ok(owner)
        {
            let _ = pcm.session.seal(owner);
        }
    }
    fn stop(&mut self) -> bool {
        self.probe = None;
        self.cancel_pcm();
        self.current.continuous = false;
        self.pending_start = None;
        let stopped = self.h.is_null() || self.call(&["stop"]);
        self.entry = None;
        self.entries.clear();
        self.future = None;
        self.ended = None;
        self.current.natural_eof = false;
        self.current.position_seconds = None;
        self.current.staged_generation = None;
        self.current.handoff_from = None;
        self.current.gain = None;
        self.current.output = None;
        self.current.processing = None;
        stopped
    }
    fn pump(&mut self) {
        if self.h.is_null() {
            return;
        }
        loop {
            let event = unsafe { &*mpv_wait_event(self.h, 0.0) };
            if event.id == 0 {
                break;
            }
            if event.id == 2 && !event.data.is_null() {
                let message = unsafe { &*event.data.cast::<LogMessage>() };
                if !message.text.is_null() {
                    let text = unsafe { std::ffi::CStr::from_ptr(message.text) }.to_string_lossy();
                    if text.contains("Cannot find codec") || text.contains("No audio decoder") {
                        self.unsupported_decoder = true;
                    }
                }
            }
            if event.id == 6 && !event.data.is_null() {
                let id = unsafe { (*event.data.cast::<StartFile>()).entry };
                if let Some(Entry {
                    generation,
                    gain,
                    source,
                }) = self.entries.get(&id).cloned()
                {
                    let previous = self.current.generation;
                    if generation != previous {
                        self.transcoded = false;
                        self.timeline_offset = 0.0;
                    }
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
                        source,
                        handoff_from: handoff,
                        paused: self.current.paused,
                        chain_fallback: (generation == previous)
                            .then_some(self.current.chain_fallback)
                            .flatten(),
                        ..Default::default()
                    };
                    self.entry = Some(id);
                    if self.future.is_some_and(|(entry, _)| entry == id) {
                        self.future = None;
                    }
                }
            }
            if event.id == 8
                && self.entry.is_some()
                && let Some(start) = &mut self.pending_start
            {
                start.loaded = true;
            }
            if event.id == 7 && !event.data.is_null() {
                let end = unsafe { &*event.data.cast::<EndFile>() };
                if Some(end.entry) == self.entry {
                    if end.reason == 0 && end.error >= 0 {
                        self.current.natural_eof = true;
                        self.ended = Some((end.entry, self.current.generation));
                    }
                    if end.error < 0 {
                        self.current.unsupported_format = matches!(end.error, -17 | -18)
                            || self.unsupported_decoder && end.error == -16;
                        self.current.error =
                            Some("Playback failed. Check server access and audio output.");
                        self.intent.store(false, Ordering::Release);
                        self.call(&["set", "pause", "yes"]);
                        let handoff = self.current.handoff_from;
                        let gain = self.current.gain.clone();
                        self.stop();
                        self.current.gain = gain;
                        self.current.handoff_from = handoff;
                    }
                }
            }
        }
        if self.pcm.is_none() {
            self.current.staged_generation = self.future.map(|(_, generation)| generation);
        }
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
    fn load(&mut self, load: Load) {
        self.transcoded = load.transcoded;
        if self.route.as_ref().is_some_and(output::Selection::fidelity) {
            if let Err(error) = self.replace_handle(None) {
                self.current.error = Some(error);
                self.intent.store(false, Ordering::Release);
                return;
            }
            self.load_file(
                load.generation,
                load.url.clone(),
                0.0,
                Some(load.gain.clone()),
                None,
                None,
            );
            self.probe = Some(PendingProbe {
                load,
                deadline: std::time::Instant::now() + Duration::from_secs(10),
            });
        } else {
            let gain = Some(load.gain.clone());
            self.load_with_gain(load, gain);
        }
    }
    fn load_with_gain(&mut self, load: Load, gain: Option<Applied>) {
        let position = match load.local_position() {
            Ok(position) => position,
            Err(error) => {
                self.stop();
                self.current = Status {
                    generation: load.generation,
                    error: Some(error),
                    paused: true,
                    idle: true,
                    ..Default::default()
                };
                self.intent.store(false, Ordering::Release);
                return;
            }
        };
        let Load {
            timeline_offset,
            transcoded: _,
            generation,
            url,
            position: _,
            gain: _,
            source,
            description,
            chain,
            output: _,
            playing: _,
        } = load;
        self.timeline_offset = timeline_offset;
        if self.route.is_none()
            || self.route.as_ref().is_some_and(output::Selection::fidelity) && source.is_none()
        {
            self.current = Status {
                generation,
                error: Some("Fidelity source/output could not be verified; playback stays paused."),
                idle: true,
                paused: true,
                ..Default::default()
            };
            self.intent.store(false, Ordering::Release);
            return;
        }
        if (!matches!(self.route, Some(output::Selection::Desktop)) || self.registration.is_some())
            && let Err(error) = self.replace_handle(source)
        {
            self.current = Status {
                generation,
                source,
                error: Some(error),
                idle: true,
                paused: true,
                ..Default::default()
            };
            self.intent.store(false, Ordering::Release);
            return;
        }
        let processing = self.active.as_ref().map(|active| active.plan);
        if chain
            && position == 0.0
            && processing.is_some_and(output::Plan::converted)
            && let Some(description) = description.filter(|d| qualified(*d, source))
        {
            if let Err(error) =
                self.load_pcm(generation, url, gain, description, processing.unwrap())
            {
                self.stop();
                self.current = Status {
                    generation,
                    source,
                    error: Some(error),
                    idle: true,
                    paused: true,
                    ..Default::default()
                };
                self.intent.store(false, Ordering::Release);
            }
            return;
        }
        self.load_file(generation, url, position, gain, source, processing);
        if processing.is_some_and(output::Plan::converted) {
            self.current.chain_fallback =
                Some("Independent conversion: short, unverified or resumed source");
        }
    }
    fn load_pcm(
        &mut self,
        generation: u64,
        url: String,
        gain: Option<Applied>,
        description: Description,
        plan: output::Plan,
    ) -> Result<(), &'static str> {
        let session = continuous::Session::new(
            continuous::stream::Input {
                generation,
                uri: url,
                description,
                gain: gain.as_ref().map_or(1.0, |g| 10_f64.powf(g.db / 20.0)),
            },
            u64::from(description.rate) * 2,
        )?;
        for (key, value) in continuous::input::options(description.rate) {
            let key = CString::new(key).map_err(|_| "Invalid continuous input option")?;
            let value = CString::new(value).map_err(|_| "Invalid continuous input option")?;
            if unsafe { mpv_set_option_string(self.h, key.as_ptr(), value.as_ptr()) } < 0 {
                return Err("Continuous input options rejected");
            }
        }
        let registration =
            unsafe { continuous::input::Registration::attach(self.h, session.stream.clone()) }?;
        self.registration = Some(registration);
        self.pcm = Some(PcmActive {
            session,
            pending: None,
        });
        self.load_file(
            generation,
            continuous::input::URI.into(),
            0.0,
            None,
            Some(plan.source),
            Some(plan),
        );
        for entry in self.entries.values_mut() {
            entry.gain = gain.clone();
        }
        self.current.continuous = true;
        Ok(())
    }
    // The post-negotiation file pipeline is shared by production loads and
    // synthetic PCM-file qualification, without opening a physical device.
    fn load_file(
        &mut self,
        generation: u64,
        url: String,
        position: f64,
        gain: Option<Applied>,
        source: Option<Format>,
        processing: Option<output::Plan>,
    ) {
        self.call(&["set", "pause", "yes"]);
        if let Some(plan) = processing {
            let filter = plan.filter(gain.as_ref());
            let value = filter.strip_prefix("af=").unwrap_or(&filter);
            // Validate and install the mandatory graph before the file can
            // unpause. mpv can otherwise ignore malformed per-file options.
            if !self.call(&["set", "af", value]) {
                self.stop();
                self.current = Status {
                    generation,
                    source,
                    error: Some(
                        "The planned audio processing filter was rejected; playback stays paused.",
                    ),
                    idle: true,
                    paused: true,
                    ..Default::default()
                };
                self.intent.store(false, Ordering::Release);
                return;
            }
        }
        self.pending_start = (position > 0.0).then(|| PendingStart {
            target: position,
            loaded: false,
            deadline: std::time::Instant::now() + Duration::from_secs(5),
        });
        let options = file_options(position, gain.as_ref(), processing);
        self.unsupported_decoder = false;
        self.entries.clear();
        self.entry = None;
        self.future = None;
        self.ended = None;
        self.current = Status {
            generation,
            source,
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
            self.entries.insert(
                id,
                Entry {
                    generation,
                    gain,
                    source,
                },
            );
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
        source: Option<Format>,
        description: Option<crate::continuous::Description>,
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
        if let Some(pcm) = &self.pcm {
            if pcm.pending.is_some() {
                return Ok(StageResult::MissedWindow);
            }
            let compatible = description.filter(|d| qualified(*d, source)).filter(|d| {
                self.current.source == source && d.rate == self.current.source.map_or(0, |s| s.rate)
            });
            let Some(description) = compatible else {
                self.finish_chain(owner);
                return Ok(StageResult::Independent(
                    "Independent conversion: successor has a different format, short or unverified span",
                ));
            };
            let Some(active) = &self.active else {
                return Err("Continuous conversion plan disappeared");
            };
            let Some(observed) = alsa_guard::observed() else {
                return Err("Continuous output could not be verified");
            };
            if observed.rate != active.plan.target.rate
                || observed.valid_bits < active.plan.target.bits
                || observed.format != active.plan.sample
            {
                return Err("Continuous output changed; reapply output before resuming");
            }
            return self.stage_pcm(owner, next, url, gain, description);
        }
        if self.active.as_ref().is_some_and(|a| a.plan.converted()) {
            return Ok(StageResult::Independent(
                "Independent conversion: start a new qualified group at the next source",
            ));
        }
        let processing = if let Some(active) = &self.active {
            let Some(source) = source else {
                return Ok(StageResult::MissedWindow);
            };
            let Some(observed) = alsa_guard::observed() else {
                return Ok(StageResult::MissedWindow);
            };
            let Some(plan) = active.plan.successor(
                source,
                self.route
                    .as_ref()
                    .is_some_and(output::Selection::conversion_allowed),
            ) else {
                return Ok(StageResult::MissedWindow);
            };
            if observed.rate != plan.target.rate
                || observed.valid_bits < plan.target.bits
                || observed.format != plan.sample
            {
                return Ok(StageResult::MissedWindow);
            }
            if plan.converted()
                && plan.filter(gain.as_ref()) != active.plan.filter(self.current.gain.as_ref())
            {
                // A changed conversion graph/gain is installed under pause
                // on the ordinary EOF load rather than risking an unverified
                // future filter transition at the submission boundary.
                return Ok(StageResult::MissedWindow);
            }
            alsa_guard::raise_precision(plan.target.bits)?;
            Some(plan)
        } else {
            None
        };
        self.append_file(next, url, gain, source, processing)
    }
    fn stage_pcm(
        &mut self,
        owner: u64,
        next: u64,
        url: String,
        gain: Option<Applied>,
        description: Description,
    ) -> Result<StageResult, &'static str> {
        let pcm = self.pcm.as_mut().ok_or("Continuous session disappeared")?;
        if let Err(error) = pcm.session.stage(
            owner,
            continuous::stream::Input {
                generation: next,
                uri: url,
                description,
                gain: gain.as_ref().map_or(1.0, |g| 10_f64.powf(g.db / 20.0)),
            },
        ) {
            if pcm.session.stream.error().is_some() {
                return Err(error);
            }
            let _ = pcm.session.seal(owner);
            return Ok(StageResult::Independent(
                "Independent conversion: continuous preload window closed",
            ));
        }
        pcm.pending = Some(Entry {
            generation: next,
            gain,
            source: Some(Format {
                rate: description.rate,
                bits: description.bits,
                ..Default::default()
            }),
        });
        self.current.staged_generation = Some(next);
        Ok(StageResult::Ready)
    }
    fn append_file(
        &mut self,
        next: u64,
        url: String,
        gain: Option<Applied>,
        source: Option<Format>,
        processing: Option<output::Plan>,
    ) -> Result<StageResult, &'static str> {
        let entry = self.entry.ok_or("The current native entry disappeared")?;
        if !self.call(&["playlist-clear"]) {
            return Err("The native playlist could not be bounded");
        }
        self.entries.retain(|id, _| *id == entry);
        if !self.command(&[
            "loadfile".into(),
            url,
            "append".into(),
            "-1".into(),
            file_options(0.0, gain.as_ref(), processing),
        ]) {
            return Err("The audio engine rejected next-file preparation");
        }
        let id = unsafe { property::<i64>(self.h, "playlist/1/id", 4) }
            .ok_or("The next native entry was not identified")?;
        self.entries.insert(
            id,
            Entry {
                generation: next,
                gain,
                source,
            },
        );
        self.future = Some((id, next));
        Ok(StageResult::Ready)
    }
    fn unstage(&mut self, owner: u64, next: u64) -> Result<Removal, &'static str> {
        self.pump();
        self.sample();
        if let Some(pcm) = &mut self.pcm {
            match pcm.session.remove(owner, next)? {
                continuous::stream::Removal::Untouched => {
                    pcm.pending = None;
                    self.current.staged_generation = None;
                    return Ok(Removal::Untouched);
                }
                continuous::stream::Removal::Committed | continuous::stream::Removal::Stale => {
                    return self.reset().map(|s| Removal::Reset(Box::new(s)));
                }
            }
        }
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
        self.reset().map(|status| Removal::Reset(Box::new(status)))
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.cancel_pcm();
        unsafe {
            if !self.h.is_null() {
                mpv_terminate_destroy(self.h);
            }
            if self.route.as_ref().is_some_and(output::Selection::fidelity) {
                alsa_guard::disable();
            }
        }
    }
}
#[cfg(test)]
fn test_load(generation: u64, url: String, position: f64, gain: Applied) -> Load {
    Load {
        timeline_offset: 0.0,
        transcoded: false,
        generation,
        url,
        position,
        gain,
        source: None,
        description: None,
        chain: false,
        output: Some(output::Selection::Desktop),
        playing: false,
    }
}
impl Player {
    #[cfg(test)]
    pub(crate) fn test_output(output: &str) -> Result<Self, &'static str> {
        Self::with_output(output)
    }
    #[cfg(test)]
    pub(crate) fn test_options(
        output: &str,
        extra: &[(&str, String)],
    ) -> Result<Self, &'static str> {
        Self::with_options(output, extra)
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
        let latest_load = Arc::new(AtomicU64::new(0));
        let worker_latest = latest_load.clone();
        let (tx, rx) = mpsc::sync_channel(64);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let status = Arc::new(Mutex::new(Status {
            idle: true,
            paused: true,
            ..Default::default()
        }));
        let shared = status.clone();
        let join = std::thread::spawn(move || {
            let h = match create_handle(&output, &extra) {
                Ok(h) => h,
                Err(_) => {
                    let _ = ready_tx.send(false);
                    return;
                }
            };
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
                latest_load: worker_latest,
                desktop_output: output,
                extra,
                route: Some(output::Selection::Desktop),
                active: None,
                pending_start: None,
                probe: None,
                unsupported_decoder: false,
                transcoded: false,
                timeline_offset: 0.0,
                pcm: None,
                registration: None,
            };
            let _ = ready_tx.send(true);
            loop {
                native.pump();
                native.sample();
                *shared.lock().unwrap() = native.current.clone();
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Ok(Command::FinishChain(owner)) => native.finish_chain(owner),
                    Ok(Command::Pause(p)) => {
                        let pause = p
                            || !native.intent.load(Ordering::Acquire)
                            || native.pending_start.is_some()
                            || native.probe.is_some();
                        if !native.h.is_null()
                            && !native.call(&["set", "pause", if pause { "yes" } else { "no" }])
                        {
                            native.current.error = Some("The audio engine rejected pause");
                        }
                    }
                    Ok(Command::Stop) => {
                        if !native.stop() {
                            native.current.error = Some("The audio engine rejected stop");
                        }
                    }
                    #[cfg(test)]
                    Ok(Command::Load(generation, url, position)) => native
                        .load_with_gain(test_load(generation, url, position, Applied::off()), None),
                    #[cfg(test)]
                    Ok(Command::LoadGain(generation, url, position, gain)) => {
                        native.load(test_load(generation, url, position, gain))
                    }
                    #[cfg(test)]
                    Ok(Command::LoadProcessed(generation, url, gain, plan)) => {
                        assert_eq!(native.desktop_output, "pcm");
                        assert_eq!(native.route, Some(output::Selection::Desktop));
                        native.load_file(
                            generation,
                            url,
                            0.0,
                            Some(gain),
                            Some(plan.source),
                            Some(plan),
                        );
                    }
                    #[cfg(test)]
                    Ok(Command::StageProcessed(next, url, gain, plan, reply)) => {
                        assert_eq!(native.desktop_output, "pcm");
                        assert!(native.current.entry_ready && native.current.paused);
                        let result = native.append_file(
                            next,
                            url,
                            Some(gain),
                            Some(plan.source),
                            Some(plan),
                        );
                        let _ = reply.send(result);
                    }
                    #[cfg(test)]
                    Ok(Command::LoadContinuous(generation, url, gain, description, plan)) => {
                        assert!(matches!(native.desktop_output.as_str(), "pcm" | "null"));
                        let result = native.replace_handle(None).and_then(|()| {
                            native.load_pcm(generation, url, Some(gain), description, plan)
                        });
                        if let Err(error) = result {
                            native.current.error = Some(error);
                        }
                    }
                    #[cfg(test)]
                    Ok(Command::StageContinuous(owner, next, url, gain, description, reply)) => {
                        let result = native.stage_pcm(owner, next, url, Some(gain), description);
                        let _ = reply.send(result);
                    }
                    Ok(Command::LoadPrepared(load)) => {
                        if native.latest_load.load(Ordering::Acquire) != load.generation {
                            continue;
                        }
                        if native.route != load.output {
                            native.intent.store(false, Ordering::Release);
                            native.call(&["set", "pause", "yes"]);
                            native.stop();
                            native.current = Status {
                                generation: load.generation,
                                source: load.source,
                                error: Some(
                                    "Output change was not acknowledged; reapply output before resuming.",
                                ),
                                idle: true,
                                paused: true,
                                ..Default::default()
                            };
                        } else {
                            let generation = load.generation;
                            native.load(load);
                            let stale = native.latest_load.load(Ordering::Acquire) != generation;
                            if stale {
                                native.stop();
                            }
                            let playing = !stale
                                && native.pending_start.is_none()
                                && native.probe.is_none()
                                && native.current.error.is_none()
                                && native.intent.load(Ordering::Acquire);
                            if !native.h.is_null() {
                                native.call(&["set", "pause", if playing { "no" } else { "yes" }]);
                            }
                        }
                    }
                    Ok(Command::Configure(route, reply)) => {
                        let result = native.configure(route);
                        *shared.lock().unwrap() = native.current.clone();
                        let _ = reply.send(result);
                    }
                    Ok(Command::Stage {
                        owner,
                        next,
                        url,
                        gain,
                        source,
                        description,
                        reply,
                    }) => {
                        let result = native.stage(owner, next, url, gain, source, description);
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
            latest_load,
            join: Some(join),
        })
    }
    pub fn invalidate_load(&self, generation: u64) {
        self.latest_load.fetch_max(generation, Ordering::AcqRel);
        self.desired_playing.store(false, Ordering::Release);
    }
    pub fn load_prepared(&self, load: Load) -> Result<(), &'static str> {
        load.local_position()?;
        if self
            .latest_load
            .fetch_max(load.generation, Ordering::AcqRel)
            > load.generation
        {
            return Err("Stale playback preparation rejected");
        }
        self.desired_playing.store(load.playing, Ordering::Release);
        self.tx.try_send(Command::LoadPrepared(load)).map_err(|_| {
            self.desired_playing.store(false, Ordering::Release);
            "Playback command queue is busy; selection stays paused. Retry explicitly."
        })
    }
    pub fn finish_chain(&self, owner: u64) {
        if self.status().continuous {
            self.send(Command::FinishChain(owner));
        }
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
            source: None,
            description: None,
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
        source: Option<Format>,
        description: Option<crate::continuous::Description>,
    ) -> Result<StageResult, &'static str> {
        self.reply(|reply| Command::Stage {
            owner,
            next,
            url,
            gain: Some(gain),
            source,
            description,
            reply,
        })
        .await
    }
    pub async fn configure(
        &self,
        selection: Option<output::Selection>,
    ) -> Result<(), &'static str> {
        self.desired_playing.store(false, Ordering::Release);
        self.reply(|reply| Command::Configure(selection, reply))
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
    fn alsa_guard_catches_real_mpv_calls_before_pcm_open() {
        let _serial = crate::alsa_guard::TEST_SERIAL.lock().unwrap();
        crate::alsa_guard::disable();
        let device = crate::output::Device {
            id: "0".repeat(64),
            name: "Fictional".into(),
            card: 9999,
            device: 0,
            card_id: "qoda_no_such_card".into(),
        };
        crate::alsa_guard::arm(
            device,
            Format {
                rate: 48000,
                bits: 16,
                ..Default::default()
            },
            crate::output::Sample::S16,
        )
        .unwrap();
        let path = silent_file(1);
        let player = Player::with_options("alsa", &[("audio-device", "alsa/null".into())]).unwrap();
        player.send(Command::Load(1, path.to_str().unwrap().into(), 0.0));
        wait_for(&player, |s| s.error.is_some());
        assert!(
            crate::alsa_guard::error()
                .is_some_and(|e| e.contains("non-selected") || e.contains("indirect"))
        );
        drop(player);
        std::fs::remove_file(path).unwrap();
        crate::alsa_guard::disable();
    }
    fn prepared(generation: u64, output: Option<output::Selection>, url: &str) -> Load {
        Load {
            timeline_offset: 0.0,
            transcoded: false,
            generation,
            output,
            url: url.into(),
            position: 0.0,
            gain: Applied::off(),
            source: None,
            description: None,
            chain: false,
            playing: true,
        }
    }
    #[test]
    fn prepared_aac_uses_native_decoded_facts_instead_of_a_container_whitelist() {
        let path = std::env::temp_dir().join(format!("qoda-format-{}.m4a", uuid::Uuid::new_v4()));
        assert!(
            std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "anullsrc=r=44100:cl=stereo",
                    "-t",
                    "0.5",
                    "-c:a",
                    "aac",
                    "-y"
                ])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let player = Player::with_output("null").unwrap();
        player
            .load_prepared(prepared(
                1,
                Some(output::Selection::Desktop),
                path.to_str().unwrap(),
            ))
            .unwrap();
        let status = wait_for(&player, |s| {
            (s.entry_ready && s.source.is_some()) || s.error.is_some()
        });
        assert_eq!(status.error, None);
        let source = status
            .source
            .expect("production playback must expose decoded audio facts");
        assert_eq!(source.rate, 44100);
        drop(player);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn native_decoder_accepts_compressed_float_and_unusual_pcm_inputs() {
        for (extension, codec, rate, channels) in [
            ("mp3", "libmp3lame", "32000", "1"),
            ("ogg", "libvorbis", "44100", "2"),
            ("opus", "libopus", "48000", "2"),
            ("m4a", "alac", "88200", "2"),
            ("flac", "flac", "192000", "2"),
            ("wav", "pcm_f32le", "88200", "6"),
            ("wav", "pcm_f64le", "192000", "1"),
        ] {
            let path = std::env::temp_dir().join(format!(
                "qoda-format-{}.{}",
                uuid::Uuid::new_v4(),
                extension
            ));
            assert!(
                std::process::Command::new("ffmpeg")
                    .args([
                        "-v", "error", "-f", "lavfi", "-i", "anullsrc", "-t", "0.5", "-ar", rate,
                        "-ac", channels, "-c:a", codec, "-y"
                    ])
                    .arg(&path)
                    .status()
                    .unwrap()
                    .success()
            );
            let player = Player::with_output("null").unwrap();
            let mut load = prepared(1, Some(output::Selection::Desktop), path.to_str().unwrap());
            load.playing = false;
            player.load_prepared(load).unwrap();
            let status = wait_for(&player, |s| {
                (s.entry_ready && s.source.is_some()) || s.error.is_some()
            });
            assert_eq!(status.error, None, "{codec}: {status:?}");
            let source = status.source.expect(codec);
            assert_eq!(source.rate, rate.parse::<u32>().unwrap(), "{codec}");
            assert_eq!(source.channels, channels.parse::<u16>().unwrap(), "{codec}");
            assert_eq!(
                source.sample_type == crate::source_format::SampleType::Float,
                codec.contains("f32")
                    || codec.contains("f64")
                    || matches!(codec, "libmp3lame" | "libvorbis" | "libopus"),
                "{codec}: {source:?}"
            );
            assert!(status.paused);
            drop(player);
            std::fs::remove_file(path).unwrap();
        }
    }
    #[test]
    fn fidelity_inspects_unknown_input_silently_before_resolving_hardware() {
        let _serial = crate::alsa_guard::TEST_SERIAL.lock().unwrap();
        let path = silent_file(1);
        let player = Player::with_output("null").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let route = Some(output::Selection::FidelityResampled {
            device: "0".repeat(64),
            assume_stereo: true,
        });
        runtime.block_on(player.configure(route.clone())).unwrap();
        player
            .load_prepared(prepared(1, route, path.to_str().unwrap()))
            .unwrap();
        let status = wait_for(&player, |s| s.generation == 1 && s.error.is_some());
        assert_eq!(status.source.unwrap().rate, 48000);
        assert!(status.paused && status.idle);
        assert!(status.error.unwrap().contains("device"), "{status:?}");
        assert_eq!(status.position_seconds, None);
        assert!(status.output.is_none());
        drop(player);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn invalid_source_timeline_is_rejected_before_native_load() {
        let player = Player::with_output("null").unwrap();
        let mut load = prepared(1, Some(output::Selection::Desktop), "/must-not-be-opened");
        load.timeline_offset = 1.0;
        load.position = 2.0;
        assert!(
            player
                .load_prepared(load)
                .unwrap_err()
                .contains("source timeline")
        );
        assert_eq!(player.status().generation, 0);
        let mut load = prepared(2, Some(output::Selection::Desktop), "/must-not-be-opened");
        load.timeline_offset = f64::NAN;
        assert!(player.load_prepared(load).is_err());
    }
    #[test]
    fn fidelity_placeholder_never_loads_without_verified_source_or_falls_back() {
        let _serial = crate::alsa_guard::TEST_SERIAL.lock().unwrap();
        let player = Player::with_output("null").unwrap();
        let route = Some(output::Selection::Fidelity {
            device: "0".repeat(64),
        });
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(player.configure(route.clone())).unwrap();
        assert!(player.status().idle && player.status().paused);
        assert_eq!(player.status().output_selection, route);
        // Unknown input is inspected only on null output; failed inspection must not open hardware.
        player
            .load_prepared(prepared(1, route, "/must-not-be-opened.wav"))
            .unwrap();
        let status = wait_for(&player, |s| s.generation == 1 && s.error.is_some());
        assert!(status.idle && status.paused);
        assert!(status.error.unwrap().contains("Playback failed"));
        runtime
            .block_on(player.configure(Some(output::Selection::Desktop)))
            .unwrap();
        let status = player.status();
        assert!(status.idle && status.paused);
        assert_eq!(status.output_selection, Some(output::Selection::Desktop));
    }
    #[test]
    fn unacknowledged_route_and_stale_load_cannot_start_playback() {
        let player = Player::with_output("null").unwrap();
        let route = Some(output::Selection::Fidelity {
            device: "0".repeat(64),
        });
        player
            .load_prepared(prepared(1, route, "/must-not-be-opened.wav"))
            .unwrap();
        let status = wait_for(&player, |s| s.generation == 1 && s.error.is_some());
        assert!(status.idle && status.paused);
        assert!(status.error.unwrap().contains("acknowledged"));
        player.invalidate_load(3);
        assert!(
            player
                .load_prepared(prepared(
                    2,
                    Some(output::Selection::Desktop),
                    "/must-not-be-opened.wav"
                ))
                .unwrap_err()
                .contains("Stale")
        );
        assert!(!player.desired_playing.load(Ordering::Acquire));
        assert_eq!(player.status().generation, 1);
    }
    #[test]
    fn pause_after_prepared_load_stays_paused_on_null_output() {
        let player = Player::with_output("null").unwrap();
        let file = silent_file(4);
        player
            .load_prepared(prepared(
                1,
                Some(output::Selection::Desktop),
                file.to_str().unwrap(),
            ))
            .unwrap();
        player.send(Command::Pause(true));
        let status = wait_for(&player, |s| s.generation == 1 && s.entry_ready && s.paused);
        assert!(!status.idle);
        std::thread::sleep(Duration::from_millis(100));
        assert!(player.status().paused);
        drop(player);
        std::fs::remove_file(file).unwrap();
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
                            metadata: None,
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
                                .stage_gain(
                                    1,
                                    2,
                                    files[1].to_str().unwrap().into(),
                                    gain(next_db),
                                    None,
                                    None
                                )
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

#[cfg(test)]
mod hq_conversion_tests {
    use super::*;
    #[test]
    fn unavailable_conversion_filter_cannot_silently_use_default_resampling() {
        let root = std::env::temp_dir().join(format!("qoda-hq-failure-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let capture = root.join("capture.pcm");
        let input = super::tests::silent_file(1);
        if let Ok(player) = Player::with_options(
            "pcm",
            &[
                ("ao-pcm-file", capture.to_str().unwrap().into()),
                ("ao-pcm-waveheader", "no".into()),
                ("audio-format", "s16".into()),
                ("audio-samplerate", "48000".into()),
                (
                    "af",
                    "lavfi=[aresample=48000:resampler=qoda_unavailable_engine]".into(),
                ),
            ],
        ) {
            player.send(Command::Load(1, input.to_str().unwrap().into(), 0.0));
            super::tests::wait_for(&player, |state| state.error.is_some() || state.entry_ready);
            player.send(Command::Pause(false));
            let state = super::tests::wait_for(&player, |state| {
                state.error.is_some() || state.idle && state.natural_eof
            });
            assert!(
                state.error.is_some(),
                "Rejected filter fell back to unprocessed playback: {state:?}"
            );
            drop(player);
        }
        assert!(std::fs::read(&capture).unwrap_or_default().is_empty());
        std::fs::remove_file(input).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn planned_soxr_pipeline_preserves_duration_gain_and_rejects_aliasing_at_pcm_boundary() {
        // Actual per-file mpv options, synthetic input and PCM-file output only.
        // This does not open a physical PCM or qualify downstream hardware.
        for (rate, bits, frequency, db) in [
            (44100_u32, 16_u16, 1000.0, 0.0),
            (96000, 24, 1000.0, -6.0),
            (96000, 24, 30000.0, 0.0),
            (48000, 24, 0.0, 0.0),
        ] {
            let root = std::env::temp_dir().join(format!("qoda-hq-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let input = root.join("source.wav");
            let capture = root.join("capture.pcm");
            let diagnostic = root.join("mpv.log");
            let frames = rate * 2;
            let mut pcm = Vec::new();
            for frame in 0..frames {
                let sample = (0.25
                    * (2.0 * std::f64::consts::PI * frequency * f64::from(frame) / f64::from(rate))
                        .sin()
                    * (2_f64.powi(i32::from(bits) - 1) - 1.0))
                    .round() as i32;
                for _ in 0..2 {
                    pcm.extend_from_slice(&sample.to_le_bytes()[..usize::from(bits / 8)]);
                }
            }
            let mut wav = b"RIFF".to_vec();
            wav.extend((36 + pcm.len() as u32).to_le_bytes());
            wav.extend(b"WAVEfmt ");
            wav.extend(16_u32.to_le_bytes());
            wav.extend(1_u16.to_le_bytes());
            wav.extend(2_u16.to_le_bytes());
            wav.extend(rate.to_le_bytes());
            wav.extend((rate * u32::from(bits) / 4).to_le_bytes());
            wav.extend((bits / 4).to_le_bytes());
            wav.extend(bits.to_le_bytes());
            wav.extend(b"data");
            wav.extend((pcm.len() as u32).to_le_bytes());
            wav.extend(pcm);
            std::fs::write(&input, wav).unwrap();
            let plan = output::Plan {
                source: Format {
                    rate,
                    bits,
                    ..Default::default()
                },
                target: Format {
                    rate: 48000,
                    bits: 16,
                    ..Default::default()
                },
                sample: output::Sample::S16,
            };
            let gain = Applied {
                db,
                ..Applied::off()
            };
            let player = Player::with_options(
                "pcm",
                &[
                    ("log-file", diagnostic.to_str().unwrap().into()),
                    ("ao-pcm-file", capture.to_str().unwrap().into()),
                    ("ao-pcm-waveheader", "no".into()),
                    ("audio-format", "s16".into()),
                    ("audio-samplerate", "48000".into()),
                    ("audio-channels", "stereo".into()),
                ],
            )
            .unwrap();
            player.send(Command::LoadProcessed(
                1,
                input.to_str().unwrap().into(),
                gain.clone(),
                plan,
            ));
            super::tests::wait_for(&player, |s| s.error.is_some() || s.entry_ready && s.paused);
            let status = player.status();
            assert!(
                status.error.is_none(),
                "{status:?}: {}",
                std::fs::read_to_string(&diagnostic).unwrap_or_default()
            );
            let streams = if rate == 44100 {
                let (reply, result) = mpsc::channel();
                player.send(Command::StageProcessed(
                    2,
                    input.to_str().unwrap().into(),
                    gain,
                    plan,
                    reply,
                ));
                assert_eq!(
                    result
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap()
                        .unwrap(),
                    StageResult::Ready
                );
                2
            } else {
                1
            };
            player.send(Command::Pause(false));
            super::tests::wait_for(&player, |s| s.error.is_some() || s.idle && s.natural_eof);
            assert!(player.status().error.is_none());
            drop(player);
            let bytes = std::fs::read(&capture).unwrap();
            assert!(
                bytes.len() as i64 == 96000 * 4 * streams,
                "Resampling changed duration: {} bytes; {}",
                bytes.len(),
                std::fs::read_to_string(&diagnostic).unwrap_or_default()
            );
            let samples = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .skip(1024)
                .take(90000)
                .map(|frame| f64::from(i16::from_le_bytes([frame[0], frame[1]])))
                .collect::<Vec<_>>();
            let rms = (samples.iter().map(|sample| sample * sample).sum::<f64>()
                / samples.len() as f64)
                .sqrt();
            if frequency == 1000.0 {
                let expected = 32768.0 * 0.25 * 10_f64.powf(db / 20.0) / 2_f64.sqrt();
                assert!(
                    (rms / expected - 1.0).abs() < 0.002,
                    "Gain changed: {rms} vs {expected}"
                );
            } else if frequency == 30000.0 {
                assert!(
                    rms < 2.0,
                    "Out-of-band signal aliased into output: {rms} LSB rms"
                );
            } else {
                assert!(
                    (0.3..0.8).contains(&rms),
                    "Expected TPDF quantization dither, got {rms} LSB rms"
                );
                let mean = samples.iter().sum::<f64>() / samples.len() as f64;
                assert!(mean.abs() < 0.03, "Dither has a DC bias: {mean}");
            }
            if streams == 2 {
                // mpv drains/resets its resampler per file. Compare the join
                // with two independently converted files, not a continuous
                // album resampler (tracked separately in the audio scope).
                let reference_path = root.join("reference.pcm");
                let filter = plan.filter(Some(&Applied::off()));
                let graph = filter
                    .strip_prefix("af=lavfi=[")
                    .unwrap()
                    .strip_suffix(']')
                    .unwrap();
                let result = std::process::Command::new("/usr/bin/ffmpeg")
                    .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                    .arg(&input)
                    .args(["-af", graph, "-f", "s16le", "-c:a", "pcm_s16le"])
                    .arg(&reference_path)
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                let reference = std::fs::read(reference_path).unwrap();
                assert_eq!(reference.len(), 96000 * 4);
                let maximum_error = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .enumerate()
                    .skip(96000 - 256)
                    .take(512)
                    .map(|(index, frame)| {
                        let actual = i32::from(i16::from_le_bytes([frame[0], frame[1]]));
                        let offset = (index % 96000) * 4;
                        let expected = i32::from(i16::from_le_bytes([
                            reference[offset],
                            reference[offset + 1],
                        ]));
                        (actual - expected).abs()
                    })
                    .max()
                    .unwrap();
                assert!(
                    maximum_error <= 2,
                    "Per-file conversion join differs from reference: {maximum_error} LSB"
                );
                println!(
                    "44100→48000 chain: {} frames, per-file reference join error {maximum_error} LSB",
                    bytes.len() / 4
                );
            }
            let log = std::fs::read_to_string(&diagnostic).unwrap();
            assert!(
                !log.contains("Error parsing option"),
                "Per-file filter was ignored: {log}"
            );
            println!(
                "HQ {rate} Hz/{bits} bit → 48000 Hz/16 bit, {db} dB, {frequency} Hz tone: {rms:.6} LSB rms"
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(test)]
mod continuous_tests {
    use super::*;
    struct Fixture {
        root: std::path::PathBuf,
        files: Vec<std::path::PathBuf>,
        description: Description,
    }
    impl Fixture {
        fn new(rate: u32, bits: u16) -> Self {
            Self::with_frames(rate, bits, rate * 2 + 17)
        }
        fn with_frames(rate: u32, bits: u16, frames: u32) -> Self {
            let root = std::env::temp_dir()
                .join(format!("qoda-continuous-native-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let mut files = Vec::new();
            for seed in 0..3 {
                let path = root.join(format!("{seed}.wav"));
                let mut pcm = Vec::<u8>::new();
                for frame in 0..frames {
                    let value = ((f64::from(frame + seed * 131) * 0.11).sin()
                        * 0.2
                        * 2_f64.powi(i32::from(bits) - 1))
                    .round() as i32;
                    for _ in 0..2 {
                        pcm.extend(&value.to_le_bytes()[..usize::from(bits / 8)]);
                    }
                }
                let mut bytes = b"RIFF".to_vec();
                bytes.extend((36 + pcm.len() as u32).to_le_bytes());
                bytes.extend(b"WAVEfmt ");
                bytes.extend(16_u32.to_le_bytes());
                bytes.extend(1_u16.to_le_bytes());
                bytes.extend(2_u16.to_le_bytes());
                bytes.extend(rate.to_le_bytes());
                bytes.extend((rate * u32::from(bits) / 4).to_le_bytes());
                bytes.extend((bits / 4).to_le_bytes());
                bytes.extend(bits.to_le_bytes());
                bytes.extend(b"data");
                bytes.extend((pcm.len() as u32).to_le_bytes());
                bytes.extend(pcm);
                std::fs::write(&path, bytes).unwrap();
                files.push(path);
            }
            Self {
                root,
                files,
                description: Description {
                    rate,
                    bits,
                    frames: u64::from(frames),
                },
            }
        }
        fn plan(&self, rate: u32) -> output::Plan {
            output::Plan {
                source: Format {
                    rate: self.description.rate,
                    bits: self.description.bits,
                    ..Default::default()
                },
                target: Format {
                    rate,
                    bits: 16,
                    ..Default::default()
                },
                sample: output::Sample::S16,
            }
        }
        fn load(&self, player: &Player, generation: u64, plan: output::Plan) {
            player.send(Command::LoadContinuous(
                generation,
                self.files[0].to_str().unwrap().into(),
                Applied::off(),
                self.description,
                plan,
            ));
            let state = wait(player, |s| s.entry_ready && s.paused);
            assert!(state.continuous);
        }
        fn stage(&self, player: &Player, owner: u64, next: u64, index: usize, db: f64) {
            let (tx, rx) = mpsc::channel();
            player.send(Command::StageContinuous(
                owner,
                next,
                self.files[index].to_str().unwrap().into(),
                Applied {
                    db,
                    ..Applied::off()
                },
                self.description,
                tx,
            ));
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap(),
                StageResult::Ready
            );
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if std::thread::panicking() {
                eprintln!(
                    "{}",
                    std::fs::read_to_string(self.root.join("mpv.log")).unwrap_or_default()
                );
            }
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }
    fn wait(player: &Player, predicate: impl Fn(&Status) -> bool) -> Status {
        let until = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            let s = player.status();
            assert!(s.error.is_none(), "{s:?}");
            if predicate(&s) {
                return s;
            }
            assert!(std::time::Instant::now() < until, "{s:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn rolling_three_track_conversion_uses_nominal_clock_and_preserves_gain_per_occurrence() {
        // Actual paced libmpv null AO, no device or account. Exactly one staged
        // source, minimum two seconds, all qualified source/target rate families.
        for (source, bits, target) in [
            (44100, 24, 48000),
            (48000, 24, 44100),
            (96000, 24, 48000),
            (48000, 24, 48000),
            (44100, 16, 96000),
            (96000, 24, 44100),
            (48000, 16, 96000),
        ] {
            let fixture = Fixture::with_frames(source, bits, source * 2);
            let player = Player::with_options(
                "null",
                &[
                    ("audio-format", "s16".into()),
                    ("audio-samplerate", target.to_string()),
                    ("audio-channels", "stereo".into()),
                    (
                        "log-file",
                        fixture.root.join("mpv.log").to_str().unwrap().into(),
                    ),
                ],
            )
            .unwrap();
            fixture.load(&player, 10, fixture.plan(target));
            fixture.stage(&player, 10, 11, 1, -6.0);
            assert_eq!(player.status().generation, 10);
            player.send(Command::Pause(false));
            let next = wait(&player, |s| {
                s.generation == 11 && s.position_seconds.is_some()
            });
            assert_eq!(next.handoff_from, Some(10));
            assert_eq!(next.gain.unwrap().db, -6.0);
            assert!(!next.natural_eof);
            player.send(Command::Pause(true));
            let paused = wait(&player, |s| s.paused);
            assert_eq!(paused.generation, 11);
            if source == 44100 && target == 48000 {
                // Exceed the decoder's network inactivity budget while output
                // intentionally applies backpressure. This must not be a stall.
                std::thread::sleep(Duration::from_secs(6));
                let held = player.status();
                assert!(held.paused && held.error.is_none());
                assert_eq!(held.generation, 11);
                assert!(held.position_seconds.unwrap() <= paused.position_seconds.unwrap() + 0.05);
            }
            fixture.stage(&player, 11, 12, 2, -3.0);
            player.send(Command::Pause(false));
            let last = wait(&player, |s| {
                s.generation == 12 && s.position_seconds.is_some()
            });
            assert_eq!(last.handoff_from, Some(11));
            assert_eq!(last.gain.unwrap().db, -3.0);
            assert!(!last.natural_eof);
            player.finish_chain(12);
            let end = wait(&player, |s| s.idle && s.natural_eof);
            assert_eq!(end.generation, 12);
            drop(player);
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn committed_future_cancellation_returns_reset_and_raw_options_do_not_leak_to_next_load()
    {
        let fixture = Fixture::new(44100, 16);
        let player = Player::with_output("null").unwrap();
        fixture.load(&player, 1, fixture.plan(48000));
        fixture.stage(&player, 1, 2, 1, 0.0);
        player.send(Command::Pause(false));
        wait(&player, |s| {
            s.generation == 1 && s.position_seconds.is_some_and(|p| p > 1.9)
        });
        let Removal::Reset(before) = player.unstage(1, 2).await.unwrap() else {
            panic!("admitted future was acknowledged untouched");
        };
        assert!(matches!(before.generation, 1 | 2));
        let state = player.status();
        assert!(state.idle && state.paused);
        assert!(!state.natural_eof);
        assert!(!state.continuous);
        player.send(Command::Load(
            3,
            fixture.files[0].to_str().unwrap().into(),
            0.0,
        ));
        wait(&player, |s| s.generation == 3 && s.entry_ready && s.paused);
        player.send(Command::Pause(false));
        wait(&player, |s| s.generation == 3 && s.idle && s.natural_eof);
        drop(player);
    }
    #[test]
    fn native_continuous_graph_matches_uninterrupted_conversion_with_per_source_gain() {
        let fixture = Fixture::new(44100, 24);
        let capture = fixture.root.join("capture.pcm");
        let reference = fixture.root.join("reference.pcm");
        let raw = fixture.root.join("reference.f64");
        let plan = fixture.plan(48000);
        let mut joined = Vec::new();
        for (index, db) in [0.0, -6.0, -3.0].into_iter().enumerate() {
            let wav = std::fs::read(&fixture.files[index]).unwrap();
            for bytes in wav[44..].as_chunks::<3>().0 {
                let value = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8;
                let sample = f64::from(value) / 8388608.0 * 10_f64.powf(db / 20.0);
                joined.extend(sample.to_le_bytes());
            }
        }
        std::fs::write(&raw, joined).unwrap();
        let graph = plan.filter(None);
        let graph = graph
            .strip_prefix("af=lavfi=[")
            .unwrap()
            .strip_suffix(']')
            .unwrap();
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin", "-v", "error", "-f", "f64le", "-ar", "44100", "-ac", "2", "-i",
            ])
            .arg(&raw)
            .args(["-af", graph, "-f", "s16le"])
            .arg(&reference)
            .status()
            .unwrap();
        assert!(status.success());
        let player = Player::with_options(
            "pcm",
            &[
                ("ao-pcm-file", capture.to_str().unwrap().into()),
                ("ao-pcm-waveheader", "no".into()),
                ("audio-format", "s16".into()),
                ("audio-samplerate", "48000".into()),
                ("audio-channels", "stereo".into()),
            ],
        )
        .unwrap();
        fixture.load(&player, 20, plan);
        fixture.stage(&player, 20, 21, 2, -30.0);
        let (reply, result) = mpsc::channel();
        player.send(Command::Unstage {
            owner: 20,
            next: 21,
            reply,
        });
        assert!(matches!(
            result
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap(),
            Removal::Untouched
        ));
        fixture.stage(&player, 20, 22, 1, -6.0);
        player.send(Command::Pause(false));
        wait(&player, |s| s.generation == 22);
        fixture.stage(&player, 22, 23, 2, -3.0);
        wait(&player, |s| s.generation == 23);
        player.finish_chain(23);
        wait(&player, |s| s.idle && s.natural_eof);
        drop(player);
        let actual = std::fs::read(capture).unwrap();
        let expected = std::fs::read(reference).unwrap();
        assert_eq!(actual.len(), expected.len());
        let error = actual
            .as_chunks::<2>()
            .0
            .iter()
            .zip(expected.as_chunks::<2>().0.iter())
            .map(|(a, b)| {
                (i32::from(i16::from_le_bytes(*a)) - i32::from(i16::from_le_bytes(*b))).abs()
            })
            .max()
            .unwrap();
        assert!(
            error <= 2,
            "Continuous graph differs from uninterrupted reference by {error} output LSB"
        );
    }
}
