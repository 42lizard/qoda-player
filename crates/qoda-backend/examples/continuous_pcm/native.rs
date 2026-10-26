//! Experiment-only final libmpv: PCM-file capture or paced null, never a device AO.
use super::{
    input::{self, Reader, Registration},
    ring::Ring,
};
use std::{
    ffi::{CString, c_char, c_int, c_void},
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
#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_create() -> *mut c_void;
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

}
pub struct Native {
    handle: *mut c_void,
    registration: Option<Registration>,
}
impl Native {
    pub fn new(
        ring: Arc<Ring>,
        capture: &str,
        rate: u32,
        graph: &str,
        bits: u16,
        target_rate: u32,
    ) -> Result<Self, &'static str> {
        Self::configured(ring, capture, rate, graph, bits, false, target_rate)
    }
    pub fn for_session(
        session: &super::session::Session,
        capture: &str,
        rate: u32,
        graph: &str,
        bits: u16,
        target_rate: u32,
    ) -> Result<Self, &'static str> {
        Self::configured(
            session.stream.clone(),
            capture,
            rate,
            graph,
            bits,
            false,
            target_rate,
        )
    }
    fn clock(&self) -> Option<f64> {
        let mut pts = 0_f64;
        (unsafe {
            mpv_get_property(
                self.handle,
                c"audio-pts".as_ptr(),
                5,
                (&mut pts as *mut f64).cast(),
            )
        } >= 0
            && pts.is_finite()
            && pts >= 0.0)
            .then_some(pts)
    }
    pub fn run_session(
        &self,
        session: &super::session::Session,
        upcoming: &[super::stream::Input],
    ) -> Result<Vec<u64>, &'static str> {
        let mut upcoming = upcoming.iter();
        let mut observations = Vec::new();
        let mut staged_for = 0_u64;
        let initial = session.stream.owner()?;
        if let Some(next) = upcoming.next() {
            session.stage(initial, next.clone())?;
        } else {
            session.seal(initial)?;
        }
        self.start()?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            session.poll();
            if let Some(error) = session.stream.error() {
                return Err(error);
            }
            if let Some(pts) = self.clock() {
                let observation = session.observe(pts)?;
                if observations.last() != Some(&observation.generation) {
                    observations.push(observation.generation);
                }
                if observation.handoff_from.is_some() && staged_for != observation.generation {
                    if let Some(next) = upcoming.next() {
                        session.stage(observation.generation, next.clone())?;
                    } else {
                        session.seal(observation.generation)?;
                    }
                    staged_for = observation.generation;
                }
            }
            let event = unsafe { &*mpv_wait_event(self.handle, 0.002) };
            if event.id == 7 && !event.data.is_null() {
                let end = unsafe { &*event.data.cast::<End>() };
                return if end.reason == 0 && end.error >= 0 {
                    Ok(observations)
                } else {
                    Err("Continuous native playback failed")
                };
            }
            if Instant::now() >= deadline {
                return Err("Continuous native playback deadline exceeded");
            }
        }
    }
    fn configured(
        reader: Arc<dyn Reader>,
        capture: &str,
        rate: u32,
        graph: &str,
        bits: u16,
        clocked: bool,
        target_rate: u32,
    ) -> Result<Self, &'static str> {
        let mut native = Self {
            handle: unsafe { mpv_create() },
            registration: None,
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
            ("audio-samplerate", target_rate.to_string()),
            ("audio-channels", "stereo".into()),
            ("af", format!("lavfi=[{graph}]")),
            ("audio-swresample-o", "dither_method=none".into()),
        ] {
            native.option(key, &value)?;
        }
        if unsafe { mpv_initialize(native.handle) } < 0 {
            return Err("mpv initialize failed");
        }
        // This is also the production integration seam: output is negotiated
        // first, then trusted input options are installed on the initialized core.
        for (key, value) in input::options(rate) {
            native.option(key, &value)?;
        }
        // SAFETY: the registration field is dropped only after the engine in Drop.
        native.registration = Some(unsafe { Registration::attach(native.handle, reader) }?);
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
            CString::new(input::URI).unwrap(),
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
            if self
                .registration
                .as_ref()
                .is_some_and(Registration::cancelled)
            {
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
        if let Some(registration) = &self.registration {
            registration.cancel();
        }
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
        let native = Native::new(
            ring.clone(),
            capture.to_str().unwrap(),
            48000,
            "anull",
            16,
            48000,
        )
        .unwrap();
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
    let native = Native::configured(ring.clone(), "", 44100, graph, 16, true, 48000)?;
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
