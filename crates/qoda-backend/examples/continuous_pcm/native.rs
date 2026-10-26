//! Experiment-only final libmpv: PCM-file capture or paced null, never a device AO.
use super::ring::Ring;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    time::{Duration, Instant},
};
#[repr(C)]
struct Event {
    id: c_int,
    error: c_int,
    userdata: u64,
    data: *mut c_void,
}
#[repr(C)]
struct End {
    reason: c_int,
    error: c_int,
    entry: i64,
    insert: i64,
    count: c_int,
}
#[repr(C)]
struct Info {
    cookie: *mut c_void,
    read: Option<unsafe extern "C" fn(*mut c_void, *mut c_char, u64) -> i64>,
    seek: Option<unsafe extern "C" fn(*mut c_void, i64) -> i64>,
    size: Option<unsafe extern "C" fn(*mut c_void) -> i64>,
    close: Option<unsafe extern "C" fn(*mut c_void)>,
    cancel: Option<unsafe extern "C" fn(*mut c_void)>,
}
#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_create() -> *mut c_void;
    fn mpv_client_api_version() -> std::ffi::c_ulong;
    fn mpv_set_option_string(h: *mut c_void, k: *const c_char, v: *const c_char) -> c_int;
    fn mpv_initialize(h: *mut c_void) -> c_int;
    fn mpv_command(h: *mut c_void, args: *const *const c_char) -> c_int;
    fn mpv_get_property(
        h: *mut c_void,
        key: *const c_char,
        format: c_int,
        value: *mut c_void,
    ) -> c_int;
    fn mpv_wait_event(h: *mut c_void, timeout: f64) -> *const Event;
    fn mpv_terminate_destroy(h: *mut c_void);
    fn mpv_stream_cb_add_ro(
        h: *mut c_void,
        protocol: *const c_char,
        data: *mut c_void,
        open: unsafe extern "C" fn(*mut c_void, *mut c_char, *mut Info) -> c_int,
    ) -> c_int;
}
unsafe extern "C" fn read(cookie: *mut c_void, output: *mut c_char, size: u64) -> i64 {
    catch_unwind(AssertUnwindSafe(|| {
        let size = usize::try_from(size)
            .ok()
            .filter(|size| *size <= isize::MAX as usize)
            .ok_or(())?;
        if output.is_null() || cookie.is_null() {
            return Err(());
        }
        // mpv owns the buffer for this call and keeps cookie alive until close.
        let bytes = unsafe { std::slice::from_raw_parts_mut(output.cast::<u8>(), size) };
        let ring = unsafe { &*cookie.cast::<Ring>() };
        ring.read(bytes).map(|count| count as i64).map_err(|_| ())
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(-1)
}
unsafe extern "C" fn cancel(cookie: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !cookie.is_null() {
            unsafe { &*cookie.cast::<Ring>() }.cancel();
        }
    }));
}
unsafe extern "C" fn close(cookie: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !cookie.is_null() {
            drop(unsafe { Arc::from_raw(cookie.cast::<Ring>()) });
        }
    }));
}
unsafe extern "C" fn open(data: *mut c_void, uri: *mut c_char, info: *mut Info) -> c_int {
    catch_unwind(AssertUnwindSafe(|| {
        if data.is_null() || uri.is_null() || info.is_null() {
            return -13;
        }
        if unsafe { CStr::from_ptr(uri) }.to_bytes() != b"qodapcm://session" {
            return -13;
        }
        unsafe {
            Arc::increment_strong_count(data.cast::<Ring>());
        }
        unsafe {
            *info = Info {
                cookie: data,
                read: Some(read),
                seek: None,
                size: None,
                close: Some(close),
                cancel: Some(cancel),
            };
        }
        0
    }))
    .unwrap_or(-13)
}
pub struct Native {
    handle: *mut c_void,
    registration: Arc<Ring>,
}
impl Native {
    pub fn new(
        ring: Arc<Ring>,
        capture: &str,
        rate: u32,
        graph: &str,
        bits: u16,
    ) -> Result<Self, &'static str> {
        Self::configured(ring, capture, rate, graph, bits, false)
    }
    fn configured(
        ring: Arc<Ring>,
        capture: &str,
        rate: u32,
        graph: &str,
        bits: u16,
        clocked: bool,
    ) -> Result<Self, &'static str> {
        // cancel_fn was added with 1.106. Do not write a newer struct to older APIs.
        if unsafe { mpv_client_api_version() } < ((1 << 16) | 106) {
            return Err("mpv stream cancellation API unavailable");
        }
        let native = Self {
            handle: unsafe { mpv_create() },
            registration: ring,
        };
        if native.handle.is_null() {
            return Err("mpv create failed");
        }
        for (key, value) in [
            ("config", "no".into()),
            ("clipboard-backends", "".into()),
            ("load-auto-profiles", "no".into()),
            ("osc", "no".into()),
            ("load-scripts", "no".into()),
            ("ytdl", "no".into()),
            ("access-references", "no".into()),
            ("terminal", "no".into()),
            ("idle", "yes".into()),
            ("vo", "null".into()),
            ("vid", "no".into()),
            ("ao", if clocked { "null" } else { "pcm" }.into()),
            ("ao-null-untimed", "no".into()),
            ("ao-pcm-file", capture.to_owned()),
            ("ao-pcm-waveheader", "no".into()),
            ("gapless-audio", "no".into()),
            ("volume", "100".into()),
            ("volume-gain", "0".into()),
            ("replaygain", "no".into()),
            ("speed", "1".into()),
            (
                "audio-format",
                if bits == 16 { "s16" } else { "s32" }.into(),
            ),
            ("audio-samplerate", "48000".into()),
            ("audio-channels", "stereo".into()),
            ("demuxer", "lavf".into()),
            (
                "demuxer-lavf-format",
                if cfg!(target_endian = "little") {
                    "f64le"
                } else {
                    "f64be"
                }
                .into(),
            ),
            (
                "demuxer-lavf-o",
                format!("sample_rate={rate},ch_layout=stereo"),
            ),
            ("cache", "no".into()),
            ("demuxer-readahead-secs", "0".into()),
            ("demuxer-max-bytes", "256KiB".into()),
            ("demuxer-max-back-bytes", "0".into()),
            ("stream-buffer-size", "64KiB".into()),
            ("af", format!("lavfi=[{graph}]")),
            ("audio-swresample-o", "dither_method=none".into()),
        ] {
            native.option(key, &value)?;
        }
        if unsafe { mpv_initialize(native.handle) } < 0 {
            return Err("mpv initialize failed");
        }
        let protocol = CString::new("qodapcm").map_err(|_| "protocol invalid")?;
        if unsafe {
            mpv_stream_cb_add_ro(
                native.handle,
                protocol.as_ptr(),
                Arc::as_ptr(&native.registration).cast_mut().cast(),
                open,
            )
        } < 0
        {
            return Err("stream registration failed");
        }
        Ok(native)
    }
    fn option(&self, key: &str, value: &str) -> Result<(), &'static str> {
        let key = CString::new(key).map_err(|_| "option invalid")?;
        let value = CString::new(value).map_err(|_| "value invalid")?;
        if unsafe { mpv_set_option_string(self.handle, key.as_ptr(), value.as_ptr()) } < 0 {
            return Err("mpv option rejected");
        }
        Ok(())
    }
    fn start(&self) -> Result<(), &'static str> {
        let args = [
            CString::new("loadfile").unwrap(),
            CString::new("qodapcm://session").unwrap(),
        ];
        let pointers = [args[0].as_ptr(), args[1].as_ptr(), std::ptr::null()];
        if unsafe { mpv_command(self.handle, pointers.as_ptr()) } < 0 {
            return Err("raw load rejected");
        }
        Ok(())
    }
    pub fn run(&self) -> Result<(), &'static str> {
        self.start()?;
        self.wait()
    }
    fn wait(&self) -> Result<(), &'static str> {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            let event = unsafe { &*mpv_wait_event(self.handle, 0.02) };
            if self.registration.cancelled() {
                return Err("continuous input cancelled");
            }
            if event.id == 7 && !event.data.is_null() {
                let end = unsafe { &*event.data.cast::<End>() };
                return if end.reason == 0 && end.error >= 0 {
                    Ok(())
                } else {
                    Err("raw playback failed")
                };
            }
        }
        Err("raw playback deadline exceeded")
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.registration.cancel();
        if !self.handle.is_null() {
            unsafe {
                mpv_terminate_destroy(self.handle);
            }
        }
        // registration remains alive through core destruction and all close calls.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelling_empty_native_input_closes_callbacks_before_context_release() {
        let directory =
            std::env::temp_dir().join(format!("qoda-pcm-native-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let capture = directory.join("capture.pcm");
        let ring = Ring::new();
        let native =
            Native::new(ring.clone(), capture.to_str().unwrap(), 48000, "anull", 16).unwrap();
        let copy = ring.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            copy.cancel();
        });
        assert!(native.run().is_err());
        let start = Instant::now();
        drop(native);
        assert!(start.elapsed() < Duration::from_millis(500));
        worker.join().unwrap();
        assert_eq!(Arc::strong_count(&ring), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

/// Bounded paced-null experiment: only one one-frame successor is available.
/// The third input is withheld until the measurement window ends. No sound AO.
pub fn horizon() -> Result<serde_json::Value, &'static str> {
    let ring = Ring::new();
    let graph = "aresample=48000:resampler=soxr:precision=28:cheby=1:osf=s16:output_sample_bits=16:dither_method=triangular:async=0";
    let native = Native::configured(ring.clone(), "", 44100, graph, 16, true)?;
    let copy = ring.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let producer = std::thread::spawn(move || {
        let first = [0.25_f64.to_ne_bytes(), 0.125_f64.to_ne_bytes()]
            .concat()
            .repeat(44101);
        copy.write(&first)?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| "horizon release timed out")?;
        copy.write(&first[..44100 * 16])?;
        copy.finish()
    });
    let measured = (|| {
        native.start()?;
        let key = CString::new("audio-pts").map_err(|_| "clock key invalid")?;
        let started = Instant::now();
        let mut maximum = None::<f64>;
        while started.elapsed() < Duration::from_millis(2300) {
            let _event = unsafe { mpv_wait_event(native.handle, 0.0) };
            let mut pts = 0_f64;
            if unsafe {
                mpv_get_property(
                    native.handle,
                    key.as_ptr(),
                    5,
                    (&mut pts as *mut f64).cast(),
                )
            } >= 0
                && pts.is_finite()
                && pts > 0.0
            {
                maximum = Some(maximum.unwrap_or(0.0).max(pts));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        tx.send(()).map_err(|_| "horizon producer ended")?;
        native.wait()?;
        Ok(
            serde_json::json!({"sink":"paced null", "source_rate":44100,"first_frames":44100,
            "only_successor_frames":1,"nominal_boundary_seconds":1.0,
            "maximum_observed_before_third_input":maximum.ok_or("No positive nominal clock")?,
            "completed_after_third_input":true}),
        )
    })();
    drop(native);
    let produced = producer.join().map_err(|_| "horizon producer panicked")?;
    produced?;
    measured
}
