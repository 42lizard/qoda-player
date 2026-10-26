//! Narrow libmpv input callback boundary. Guard outlives engine destruction.
use std::{
    ffi::{CStr, c_char, c_int, c_ulong, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
pub const URI: &str = "qodapcm://session";
pub trait Reader: Send + Sync {
    fn read(&self, bytes: &mut [u8]) -> Result<usize, &'static str>;
    fn cancel(&self);
    fn cancelled(&self) -> bool;
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
    fn mpv_client_api_version() -> c_ulong;
    fn mpv_stream_cb_add_ro(
        handle: *mut c_void,
        protocol: *const c_char,
        data: *mut c_void,
        open: unsafe extern "C" fn(*mut c_void, *mut c_char, *mut Info) -> c_int,
    ) -> c_int;
}
struct Context {
    reader: Arc<dyn Reader>,
    opened: AtomicBool,
}
unsafe extern "C" fn read(cookie: *mut c_void, output: *mut c_char, size: u64) -> i64 {
    catch_unwind(AssertUnwindSafe(|| {
        let size = usize::try_from(size)
            .ok()
            .filter(|size| *size <= isize::MAX as usize)
            .ok_or(())?;
        if cookie.is_null() || output.is_null() {
            return Err(());
        }
        // mpv owns this writable buffer and keeps the cookie alive until close.
        let bytes = unsafe { std::slice::from_raw_parts_mut(output.cast::<u8>(), size) };
        let context = unsafe { &*cookie.cast::<Context>() };
        context
            .reader
            .read(bytes)
            .map(|count| count as i64)
            .map_err(|_| ())
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(-1)
}
unsafe extern "C" fn cancel(cookie: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !cookie.is_null() {
            unsafe { &*cookie.cast::<Context>() }.reader.cancel();
        }
    }));
}
unsafe extern "C" fn close(cookie: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if !cookie.is_null() {
            drop(unsafe { Arc::from_raw(cookie.cast::<Context>()) });
        }
    }));
}
unsafe extern "C" fn open(data: *mut c_void, uri: *mut c_char, info: *mut Info) -> c_int {
    catch_unwind(AssertUnwindSafe(|| {
        if data.is_null() || uri.is_null() || info.is_null() {
            return -13;
        }
        if unsafe { CStr::from_ptr(uri) }.to_bytes() != URI.as_bytes() {
            return -13;
        }
        let context = unsafe { &*data.cast::<Context>() };
        // One nonseekable input per engine. Never silently reopen partway into
        // the same producer or allow a provider URI to select another source.
        if context
            .opened
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return -13;
        }
        unsafe {
            Arc::increment_strong_count(data.cast::<Context>());
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
pub struct Registration {
    context: Arc<Context>,
}
impl Registration {
    /// # Safety
    /// `handle` must be a live initialized engine. Keep this guard until after
    /// mpv_terminate_destroy(handle) returns; registration cannot be detached.
    pub unsafe fn attach(
        handle: *mut c_void,
        reader: Arc<dyn Reader>,
    ) -> Result<Self, &'static str> {
        let version = unsafe { mpv_client_api_version() };
        if handle.is_null() || version < ((1 << 16) | 106) || version >> 16 > 2 {
            return Err("mpv cancellable input API unavailable");
        }
        let registration = Self {
            context: Arc::new(Context {
                reader,
                opened: AtomicBool::new(false),
            }),
        };
        if unsafe {
            mpv_stream_cb_add_ro(
                handle,
                c"qodapcm".as_ptr(),
                Arc::as_ptr(&registration.context).cast_mut().cast(),
                open,
            )
        } < 0
        {
            return Err("Continuous input registration failed");
        }
        Ok(registration)
    }
    pub fn cancel(&self) {
        self.context.reader.cancel();
    }
    pub fn cancelled(&self) -> bool {
        self.context.reader.cancelled()
    }
}

/// Installed before loading our internal URI, never applied to provider files.
pub fn options(rate: u32) -> Vec<(&'static str, String)> {
    vec![
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
    ]
}
