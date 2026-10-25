// THROWAWAY instrumentation. Forward real ALSA calls; copy only accepted frames.
// No simulated hw endpoint, volume manipulation, or global audio configuration.
#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    collections::HashMap,
    ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void},
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};
#[link(name = "dl")]
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}
unsafe fn symbol(name: &[u8]) -> *mut c_void {
    let p = dlsym((-1isize) as *mut c_void, name.as_ptr().cast());
    assert!(!p.is_null());
    p
}
macro_rules! real {
    ($name:literal,$ty:ty) => {
        std::mem::transmute::<*mut c_void, $ty>(symbol(concat!($name, "\0").as_bytes()))
    };
}
struct Stream {
    id: usize,
    name: String,
    channels: usize,
    width: usize,
    format: String,
    rate: u32,
    access: i32,
    raw: File,
    events: File,
    accepted: u64,
    last_state: i32,
}
static STREAMS: OnceLock<Mutex<HashMap<usize, Stream>>> = OnceLock::new();
static START: OnceLock<std::time::Instant> = OnceLock::new();
static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn streams() -> &'static Mutex<HashMap<usize, Stream>> {
    STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn log(s: &mut Stream, line: String) {
    let now = START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64();
    let line = format!(
        "{},\"t\":{:.6}}}",
        line.strip_suffix('}').unwrap_or(&line),
        now
    );
    let _ = writeln!(s.events, "{line}");
    let _ = s.events.flush();
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_open(
    out: *mut *mut c_void,
    name: *const c_char,
    kind: c_int,
    mode: c_int,
) -> c_int {
    let f = real!(
        "snd_pcm_open",
        unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int
    );
    let rc = f(out, name, kind, mode);
    if let Ok(dir) = std::env::var("QODA_TAP_DIR") {
        let actual = CStr::from_ptr(name).to_string_lossy();
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(PathBuf::from(dir).join("routes.jsonl"))
        {
            let _ = writeln!(f, "{{\"route\":\"{}\",\"result\":{}}}", actual, rc);
        }
    }
    let wanted = std::env::var("QODA_TAP_DEVICE").unwrap_or_default();
    let actual = CStr::from_ptr(name).to_string_lossy().into_owned();
    if rc >= 0 && kind == 0 && actual == wanted {
        if let Ok(dir) = std::env::var("QODA_TAP_DIR") {
            let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let path = PathBuf::from(dir);
            let raw = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(path.join(format!("stream-{id}.pcm")))
                .unwrap();
            let events = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(path.join(format!("stream-{id}.jsonl")))
                .unwrap();
            let mut s = Stream {
                id,
                name: actual,
                channels: 0,
                width: 0,
                format: String::new(),
                rate: 0,
                access: -1,
                raw,
                events,
                accepted: 0,
                last_state: -1,
            };
            log(
                &mut s,
                format!(
                    "{{\"event\":\"open\",\"stream\":{id},\"route\":\"{}\"}}",
                    wanted
                ),
            );
            streams().lock().unwrap().insert(*out as usize, s);
        }
    }
    rc
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_hw_params(pcm: *mut c_void, params: *mut c_void) -> c_int {
    let f = real!(
        "snd_pcm_hw_params",
        unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int
    );
    let rc = f(pcm, params);
    if rc >= 0 {
        let mut map = streams().lock().unwrap();
        if let Some(s) = map.get_mut(&(pcm as usize)) {
            let getf = real!(
                "snd_pcm_hw_params_get_format",
                unsafe extern "C" fn(*const c_void, *mut c_int) -> c_int
            );
            let getr = real!(
                "snd_pcm_hw_params_get_rate",
                unsafe extern "C" fn(*const c_void, *mut c_uint, *mut c_int) -> c_int
            );
            let getc = real!(
                "snd_pcm_hw_params_get_channels",
                unsafe extern "C" fn(*const c_void, *mut c_uint) -> c_int
            );
            let geta = real!(
                "snd_pcm_hw_params_get_access",
                unsafe extern "C" fn(*const c_void, *mut c_int) -> c_int
            );
            let physical = real!(
                "snd_pcm_format_physical_width",
                unsafe extern "C" fn(c_int) -> c_int
            );
            let valid = real!("snd_pcm_format_width", unsafe extern "C" fn(c_int) -> c_int);
            let fname = real!(
                "snd_pcm_format_name",
                unsafe extern "C" fn(c_int) -> *const c_char
            );
            let (mut fmt, mut rate, mut ch, mut dir, mut access) = (0, 0, 0, 0, 0);
            getf(params, &mut fmt);
            getr(params, &mut rate, &mut dir);
            getc(params, &mut ch);
            geta(params, &mut access);
            s.format = CStr::from_ptr(fname(fmt)).to_string_lossy().into_owned();
            s.channels = ch as usize;
            s.width = physical(fmt) as usize / 8;
            s.rate = rate;
            s.access = access;
            let proc = std::fs::read_to_string("/proc/asound/card1/pcm1p/sub0/hw_params")
                .unwrap_or_default()
                .replace('\n', "\\n");
            log(
                s,
                format!("{{\"event\":\"proc-hw-params\",\"value\":\"{}\"}}", proc),
            );
            let pcm_type = real!("snd_pcm_type", unsafe extern "C" fn(*mut c_void) -> c_int)(pcm);
            let line = format!(
                "{{\"pcm_type\":{},\"event\":\"negotiated\",\"format\":\"{}\",\"rate\":{},\"channels\":{},\"container_bits\":{},\"valid_bits\":{},\"access\":{}}}",
                pcm_type,
                s.format,
                s.rate,
                s.channels,
                s.width * 8,
                valid(fmt),
                s.access
            );
            log(s, line);
        }
    }
    rc
}
fn accepted(pcm: *mut c_void, data: &[u8], frames: usize, method: &str) {
    let mut map = streams().lock().unwrap();
    if let Some(s) = map.get_mut(&(pcm as usize)) {
        let bytes = frames * s.channels * s.width;
        if bytes <= data.len() {
            let _ = s.raw.write_all(&data[..bytes]);
            s.accepted += frames as u64;
            let line = format!(
                "{{\"event\":\"submit\",\"method\":\"{method}\",\"frames\":{frames},\"accepted_total\":{}}}",
                s.accepted
            );
            log(s, line);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_writei(
    pcm: *mut c_void,
    buffer: *const c_void,
    frames: c_ulong,
) -> c_long {
    let f = real!(
        "snd_pcm_writei",
        unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long
    );
    if let Ok(limit) = std::env::var("QODA_TAP_LOSS_AFTER") {
        let n = limit.parse::<u64>().unwrap();
        if let Some(s) = streams().lock().unwrap().get_mut(&(pcm as usize)) {
            if s.accepted >= n {
                let total = s.accepted;
                log(
                    s,
                    format!("{{\"event\":\"injected-device-loss\",\"accepted_total\":{total}}}"),
                );
                return -19;
            }
        }
    }
    let size = streams()
        .lock()
        .unwrap()
        .get(&(pcm as usize))
        .map(|s| frames as usize * s.channels * s.width)
        .unwrap_or(0);
    let before = if size > 0 {
        std::slice::from_raw_parts(buffer.cast::<u8>(), size).to_vec()
    } else {
        vec![]
    };
    let rc = f(pcm, buffer, frames);
    if rc < 0 {
        if let Some(s) = streams().lock().unwrap().get_mut(&(pcm as usize)) {
            log(s, format!("{{\"event\":\"write-error\",\"error\":{rc}}}"));
        }
    }
    if rc > 0 && !before.is_empty() {
        accepted(pcm, &before, rc as usize, "writei");
    }
    rc
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_writen(
    pcm: *mut c_void,
    buffers: *const *const c_void,
    frames: c_ulong,
) -> c_long {
    let f = real!(
        "snd_pcm_writen",
        unsafe extern "C" fn(*mut c_void, *const *const c_void, c_ulong) -> c_long
    );
    let shape = streams()
        .lock()
        .unwrap()
        .get(&(pcm as usize))
        .map(|s| (s.channels, s.width));
    let mut before = Vec::new();
    if let Some((ch, w)) = shape {
        for i in 0..frames as usize {
            for c in 0..ch {
                before.extend_from_slice(std::slice::from_raw_parts(
                    (*buffers.add(c)).cast::<u8>().add(i * w),
                    w,
                ));
            }
        }
    }
    let rc = f(pcm, buffers, frames);
    if rc > 0 && !before.is_empty() {
        accepted(pcm, &before, rc as usize, "writen");
    }
    rc
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_drop(pcm: *mut c_void) -> c_int {
    let f = real!("snd_pcm_drop", unsafe extern "C" fn(*mut c_void) -> c_int);
    let rc = f(pcm);
    if let Some(s) = streams().lock().unwrap().get_mut(&(pcm as usize)) {
        let total = s.accepted;
        log(
            s,
            format!("{{\"event\":\"drop\",\"result\":{rc},\"accepted_total\":{total}}}"),
        );
    }
    rc
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_close(pcm: *mut c_void) -> c_int {
    let f = real!("snd_pcm_close", unsafe extern "C" fn(*mut c_void) -> c_int);
    let rc = f(pcm);
    if let Some(mut s) = streams().lock().unwrap().remove(&(pcm as usize)) {
        let line = format!(
            "{{\"event\":\"close\",\"stream\":{},\"route\":\"{}\",\"accepted_frames\":{}}}",
            s.id, s.name, s.accepted
        );
        log(&mut s, line);
        let _ = s.raw.flush();
    }
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_status(pcm: *mut c_void, status: *mut c_void) -> c_int {
    let f = real!(
        "snd_pcm_status",
        unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int
    );
    let rc = f(pcm, status);
    if rc >= 0 {
        let get = real!(
            "snd_pcm_status_get_state",
            unsafe extern "C" fn(*const c_void) -> c_int
        );
        let state = get(status);
        if let Some(s) = streams().lock().unwrap().get_mut(&(pcm as usize)) {
            if s.last_state != state {
                s.last_state = state;
                let total = s.accepted;
                log(
                    s,
                    format!(
                        "{{\"event\":\"pcm-state\",\"state\":{state},\"accepted_total\":{total}}}"
                    ),
                );
            }
        }
    }
    rc
}
