//! Process-local ALSA submission gate. Symbols are exported by this executable,
//! not injected into other processes. No capture, mixer writes or loader asserts.
use crate::{
    output::{Device, Sample},
    source_format::Format,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Mutex, OnceLock},
};
#[link(name = "asound")]
unsafe extern "C" {
    fn snd_pcm_type(pcm: *mut c_void) -> c_int;
    fn snd_pcm_info_malloc(info: *mut *mut c_void) -> c_int;
    fn snd_pcm_info_free(info: *mut c_void);
    fn snd_pcm_info(pcm: *mut c_void, info: *mut c_void) -> c_int;
    fn snd_pcm_info_get_card(info: *const c_void) -> c_int;
    fn snd_pcm_info_get_device(info: *const c_void) -> c_uint;
    fn snd_pcm_info_get_subdevice(info: *const c_void) -> c_uint;
    fn snd_pcm_info_get_id(info: *const c_void) -> *const c_char;
    fn snd_pcm_hw_params_get_access(params: *const c_void, access: *mut c_int) -> c_int;
    fn snd_pcm_hw_params_get_format(params: *const c_void, format: *mut c_int) -> c_int;
    fn snd_pcm_hw_params_get_rate(
        params: *const c_void,
        rate: *mut c_uint,
        dir: *mut c_int,
    ) -> c_int;
    fn snd_pcm_hw_params_get_channels(params: *const c_void, channels: *mut c_uint) -> c_int;
    fn snd_pcm_hw_params_get_sbits(params: *const c_void) -> c_int;
    fn snd_pcm_get_chmap(pcm: *mut c_void) -> *mut c_uint;
}
#[derive(Clone)]
struct Expected {
    device: Device,
    source: Format,
    sample: Sample,
    epoch: u64,
    assume_stereo: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Observed {
    pub rate: u32,
    pub channels: u32,
    pub container_bits: u16,
    pub valid_bits: u16,
    pub format: Sample,
    pub stereo_layout: bool,
    pub stereo_assumed: bool,
}
#[derive(Default)]
struct Gate {
    expected: Option<Expected>,
    epoch: u64,
    handles: HashMap<usize, Option<Observed>>,
    observed: Option<Observed>,
    error: Option<&'static str>,
}
static GATE: OnceLock<Mutex<Gate>> = OnceLock::new();
fn gate() -> std::sync::MutexGuard<'static, Gate> {
    GATE.get_or_init(|| Mutex::new(Gate::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
pub fn disable() {
    let mut g = gate();
    g.expected = None;
    g.handles.clear();
    g.observed = None;
    g.error = None;
}
pub fn arm(device: Device, source: Format, sample: Sample) -> Result<(), &'static str> {
    arm_with_stereo_policy(device, source, sample, false)
}
pub fn arm_with_stereo_policy(
    device: Device,
    source: Format,
    sample: Sample,
    assume_stereo: bool,
) -> Result<(), &'static str> {
    let mut g = gate();
    let epoch = g
        .epoch
        .checked_add(1)
        .ok_or("Fidelity guard epochs exhausted")?;
    g.epoch = epoch;
    g.expected = Some(Expected {
        device,
        source,
        sample,
        epoch,
        assume_stereo,
    });
    g.handles.clear();
    g.observed = None;
    g.error = None;
    Ok(())
}
pub fn observed() -> Option<Observed> {
    gate().observed.clone()
}
pub fn error() -> Option<&'static str> {
    gate().error
}
fn blocked(reason: &'static str) -> c_int {
    gate().error = Some(reason);
    -libc::ENODEV
}
fn resolve(name: &CStr) -> Option<*mut c_void> {
    let symbol = unsafe { libc::dlsym(libc::RTLD_NEXT, name.as_ptr()) };
    (!symbol.is_null()).then_some(symbol)
}
macro_rules! real {
    ($name:literal,$ty:ty) => {{
        static SYMBOL: OnceLock<Option<$ty>> = OnceLock::new();
        *SYMBOL.get_or_init(|| {
            resolve($name).map(|p| unsafe { std::mem::transmute::<*mut c_void, $ty>(p) })
        })
    }};
}
fn boundary<T: Copy>(failure: T, call: impl FnOnce() -> T) -> T {
    // All Rust work completes inside this boundary; a panic never crosses ALSA's C ABI.
    catch_unwind(AssertUnwindSafe(call)).unwrap_or_else(|_| {
        gate().error = Some("Fidelity guard failed; playback stays paused");
        failure
    })
}
fn failure(code: c_int) -> c_int {
    if gate().expected.is_some()
        && matches!(-code, libc::ENODEV | libc::ENXIO | libc::EBADFD | libc::EIO)
    {
        blocked("Fidelity device was lost; reconnect it and explicitly Resume.");
    }
    code
}
fn actual_device(pcm: *mut c_void, expected: &Expected) -> bool {
    // ALSA owns these opaque pointers and guarantees the info allocation layout.
    unsafe {
        if snd_pcm_type(pcm) != 0 {
            return false;
        }
        let mut info = std::ptr::null_mut();
        if snd_pcm_info_malloc(&mut info) < 0 || info.is_null() {
            return false;
        }
        let result = if snd_pcm_info(pcm, info) >= 0 {
            let id = snd_pcm_info_get_id(info);
            snd_pcm_info_get_card(info) == expected.device.card
                && snd_pcm_info_get_device(info) == expected.device.device
                && snd_pcm_info_get_subdevice(info) == 0
                && !id.is_null()
                && crate::output::identity(
                    expected.device.card,
                    expected.device.device,
                    CStr::from_ptr(id).to_bytes(),
                )
                .as_deref()
                    == Some(expected.device.id.as_str())
        } else {
            false
        };
        snd_pcm_info_free(info);
        result
    }
}
#[derive(Clone, Copy)]
struct Parameters {
    access: i32,
    format: i32,
    rate: u32,
    dir: i32,
    channels: u32,
    bits: i32,
}
fn checked_parameters(source: Format, sample: Sample, p: Parameters) -> Option<Observed> {
    if p.access != 3
        || p.format != sample.raw()
        || p.rate != source.rate
        || p.dir != 0
        || p.channels != 2
        || p.bits < i32::from(source.bits)
        || p.bits > i32::from(sample.bits())
    {
        return None;
    }
    Some(Observed {
        rate: p.rate,
        channels: p.channels,
        container_bits: sample.bits(),
        valid_bits: p.bits as u16,
        format: sample,
        stereo_layout: false,
        stereo_assumed: false,
    })
}
fn params(pcm: *mut c_void, raw: *mut c_void, expected: &Expected) -> Option<Observed> {
    unsafe {
        let mut p = Parameters {
            access: 0,
            format: 0,
            rate: 0,
            dir: 0,
            channels: 0,
            bits: 0,
        };
        if !actual_device(pcm, expected)
            || snd_pcm_hw_params_get_access(raw, &mut p.access) < 0
            || snd_pcm_hw_params_get_format(raw, &mut p.format) < 0
            || snd_pcm_hw_params_get_rate(raw, &mut p.rate, &mut p.dir) < 0
            || snd_pcm_hw_params_get_channels(raw, &mut p.channels) < 0
        {
            return None;
        }
        p.bits = snd_pcm_hw_params_get_sbits(raw);
        checked_parameters(expected.source, expected.sample, p)
    }
}
fn stereo_policy(mapping: Option<[u32; 2]>, assume: bool) -> Option<bool> {
    match mapping {
        Some([3, 4]) => Some(true),
        None | Some([0, 0]) if assume => Some(false),
        _ => None,
    }
}
fn permit(pcm: *mut c_void) -> bool {
    let mut g = gate();
    if g.expected.is_none() {
        return true;
    }
    if g.error.is_some() {
        return false;
    }
    let Some(Some(observed)) = g.handles.get(&(pcm as usize)).cloned() else {
        g.error = Some("Fidelity negotiation was not verified before sample submission.");
        return false;
    };
    // Only the separate explicit compatibility policy may accept wholly
    // unreported stereo. A reported wrong/reversed/partial map still fails.
    let assume = g
        .expected
        .as_ref()
        .is_some_and(|expected| expected.assume_stereo);
    let map = unsafe { snd_pcm_get_chmap(pcm) };
    let mapping = if map.is_null() {
        None
    } else {
        unsafe {
            let mapping = if *map == 2 {
                [*map.add(1), *map.add(2)]
            } else {
                [u32::MAX; 2]
            };
            libc::free(map.cast());
            Some(mapping)
        }
    };
    let Some(reported) = stereo_policy(mapping, assume) else {
        g.error = Some(if mapping.is_none() || mapping == Some([0, 0]) {
            "Fidelity driver does not report stereo mapping. Enable the separate stereo compatibility option or choose Desktop."
        } else {
            "Fidelity stereo channel mapping is incompatible; no samples submitted."
        });
        return false;
    };
    let mut observed = observed;
    observed.stereo_layout = reported;
    observed.stereo_assumed = !reported;
    g.observed = Some(observed);
    true
}
fn transfer_result(result: c_long, frames: c_ulong) -> c_long {
    if gate().expected.is_none() {
        return result;
    }
    if result < 0 {
        blocked(if -result == c_long::from(libc::EPIPE) {
            "Fidelity underrun; buffered position is uncertain. Explicit Resume is required."
        } else {
            "Fidelity transfer failed or device was lost; playback stays paused until explicit Resume."
        });
    } else if result as c_ulong != frames {
        blocked(
            "Fidelity accepted a partial transfer; buffered position is uncertain. Explicit Resume is required.",
        );
        return -c_long::from(libc::EIO);
    }
    result
}
// Safety contract for every exported function: callers provide the valid ALSA
// handles/buffers and lengths required by the corresponding ALSA C API. We never
// retain or inspect sample buffer memory; only the real ALSA function sees it.
#[unsafe(no_mangle)]
unsafe extern "C" fn snd_pcm_open(
    out: *mut *mut c_void,
    name: *const c_char,
    stream: c_int,
    mode: c_int,
) -> c_int {
    boundary(-libc::EIO, || {
        if out.is_null() || name.is_null() {
            return -libc::EINVAL;
        }
        let Some(real) = real!(
            c"snd_pcm_open",
            unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int
        ) else {
            return blocked("ALSA loader unavailable; fidelity stays paused.");
        };
        let (expected, latched) = {
            let g = gate();
            (g.expected.clone(), g.error.is_some())
        };
        if expected.is_some() && latched {
            unsafe { *out = std::ptr::null_mut() };
            return -libc::ENODEV;
        }
        if let Some(expected) = &expected
            && (stream != 0
                || unsafe { CStr::from_ptr(name) }.to_bytes() != expected.device.route().as_bytes())
        {
            unsafe { *out = std::ptr::null_mut() };
            return blocked("Fidelity refused a non-selected or indirect ALSA route.");
        }
        let result = unsafe { real(out, name, stream, mode) };
        if result < 0 {
            if expected.is_some() {
                blocked(if -result == libc::EBUSY {
                    "Fidelity device is busy; release it elsewhere and explicitly Resume."
                } else {
                    "Fidelity device is unavailable; playback stays paused."
                });
            }
            return result;
        }
        if let Some(expected) = expected {
            let handle = unsafe { *out };
            if handle.is_null() || !actual_device(handle, &expected) {
                if !handle.is_null()
                    && let Some(close) =
                        real!(c"snd_pcm_close", unsafe extern "C" fn(*mut c_void) -> c_int)
                {
                    unsafe { close(handle) };
                }
                unsafe { *out = std::ptr::null_mut() };
                return blocked("Fidelity device identity/type changed; no samples submitted.");
            }
            let mut g = gate();
            if g.expected.as_ref().map(|e| e.epoch) != Some(expected.epoch) {
                drop(g);
                if let Some(close) =
                    real!(c"snd_pcm_close", unsafe extern "C" fn(*mut c_void) -> c_int)
                {
                    unsafe { close(handle) };
                }
                unsafe { *out = std::ptr::null_mut() };
                return -libc::ENODEV;
            }
            g.handles.insert(handle as usize, None);
        }
        result
    })
}
#[unsafe(no_mangle)]
unsafe extern "C" fn snd_pcm_hw_params(pcm: *mut c_void, raw: *mut c_void) -> c_int {
    boundary(-libc::EIO, || {
        let Some(real) = real!(
            c"snd_pcm_hw_params",
            unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int
        ) else {
            return blocked("ALSA loader unavailable; fidelity stays paused.");
        };
        let expected = gate().expected.clone();
        let allowed = {
            let g = gate();
            g.handles.contains_key(&(pcm as usize)) && g.error.is_none()
        };
        if expected.is_some() && !allowed {
            return -libc::ENODEV;
        }
        let result = unsafe { real(pcm, raw) };
        if result < 0 {
            return failure(result);
        }
        if let Some(expected) = expected {
            let Some(observed) = params(pcm, raw, &expected) else {
                return blocked(
                    "Fidelity rejected changed rate, layout, access or precision before sample submission.",
                );
            };
            let mut g = gate();
            if g.expected.as_ref().map(|e| e.epoch) != Some(expected.epoch) {
                return -libc::ENODEV;
            }
            g.observed = Some(observed.clone());
            g.handles.insert(pcm as usize, Some(observed));
        }
        result
    })
}
#[unsafe(no_mangle)]
unsafe extern "C" fn snd_pcm_writei(
    pcm: *mut c_void,
    buffer: *const c_void,
    frames: c_ulong,
) -> c_long {
    boundary(-c_long::from(libc::EIO), || {
        let Some(real) = real!(
            c"snd_pcm_writei",
            unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long
        ) else {
            return c_long::from(blocked("ALSA transfer loader unavailable."));
        };
        if !permit(pcm) {
            return -c_long::from(libc::ENODEV);
        }
        let result = unsafe { real(pcm, buffer, frames) };
        transfer_result(result, frames)
    })
}
#[unsafe(no_mangle)]
unsafe extern "C" fn snd_pcm_writen(
    pcm: *mut c_void,
    buffers: *mut *mut c_void,
    frames: c_ulong,
) -> c_long {
    boundary(-c_long::from(libc::EIO), || {
        if gate().expected.is_some() {
            return c_long::from(blocked(
                "Fidelity refuses unqualified non-interleaved transfers.",
            ));
        }
        let Some(real) = real!(
            c"snd_pcm_writen",
            unsafe extern "C" fn(*mut c_void, *mut *mut c_void, c_ulong) -> c_long
        ) else {
            return -c_long::from(libc::ENOSYS);
        };
        unsafe { real(pcm, buffers, frames) }
    })
}
#[unsafe(no_mangle)]
unsafe extern "C" fn snd_pcm_mmap_begin(
    pcm: *mut c_void,
    areas: *mut *const c_void,
    offset: *mut c_ulong,
    frames: *mut c_ulong,
) -> c_int {
    boundary(-libc::EIO, || {
        if gate().expected.is_some() {
            return blocked("Fidelity refuses unqualified mmap transfers.");
        }
        let Some(real) = real!(
            c"snd_pcm_mmap_begin",
            unsafe extern "C" fn(
                *mut c_void,
                *mut *const c_void,
                *mut c_ulong,
                *mut c_ulong,
            ) -> c_int
        ) else {
            return -libc::ENOSYS;
        };
        unsafe { real(pcm, areas, offset, frames) }
    })
}
#[unsafe(no_mangle)]
unsafe extern "C" fn snd_pcm_close(pcm: *mut c_void) -> c_int {
    boundary(-libc::EIO, || {
        let Some(real) = real!(c"snd_pcm_close", unsafe extern "C" fn(*mut c_void) -> c_int) else {
            return -libc::ENOSYS;
        };
        gate().handles.remove(&(pcm as usize));
        unsafe { real(pcm) }
    })
}

pub fn available() -> bool {
    let expected = [
        (c"snd_pcm_open", snd_pcm_open as *const () as *mut c_void),
        (
            c"snd_pcm_hw_params",
            snd_pcm_hw_params as *const () as *mut c_void,
        ),
        (
            c"snd_pcm_writei",
            snd_pcm_writei as *const () as *mut c_void,
        ),
        (
            c"snd_pcm_writen",
            snd_pcm_writen as *const () as *mut c_void,
        ),
        (
            c"snd_pcm_mmap_begin",
            snd_pcm_mmap_begin as *const () as *mut c_void,
        ),
        (c"snd_pcm_close", snd_pcm_close as *const () as *mut c_void),
        (
            c"snd_pcm_mmap_commit",
            snd_pcm_mmap_commit as *const () as *mut c_void,
        ),
        (
            c"snd_mixer_selem_set_playback_volume",
            mixer_volume as *const () as *mut c_void,
        ),
        (
            c"snd_mixer_selem_set_playback_volume_all",
            mixer_volume_all as *const () as *mut c_void,
        ),
        (
            c"snd_mixer_selem_set_playback_dB",
            mixer_db as *const () as *mut c_void,
        ),
        (
            c"snd_mixer_selem_set_playback_dB_all",
            mixer_db_all as *const () as *mut c_void,
        ),
        (
            c"snd_mixer_selem_set_playback_switch",
            mixer_switch as *const () as *mut c_void,
        ),
        (
            c"snd_mixer_selem_set_playback_switch_all",
            mixer_switch_all as *const () as *mut c_void,
        ),
    ];
    expected.iter().all(|(name,own)|unsafe{libc::dlsym(libc::RTLD_DEFAULT,name.as_ptr())==*own} && resolve(name).is_some_and(|real|real!=*own))
}

pub fn raise_precision(bits: u16) -> Result<(), &'static str> {
    let mut g = gate();
    if g.error.is_some() || !g.observed.as_ref().is_some_and(|o| o.valid_bits >= bits) {
        return Err("Fidelity successor precision is unavailable");
    }
    if let Some(expected) = &mut g.expected {
        expected.source.bits = expected.source.bits.max(bits);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) static TEST_SERIAL: Mutex<()> = Mutex::new(());
#[cfg(test)]
mod tests {
    use super::*;
    fn device() -> Device {
        Device {
            id: "0".repeat(64),
            name: "Fictional device".into(),
            card: 9999,
            device: 0,
            card_id: "qoda_no_such_card".into(),
        }
    }
    #[test]
    fn unreported_stereo_requires_explicit_policy_and_wrong_maps_never_pass() {
        assert_eq!(stereo_policy(None, false), None);
        assert_eq!(stereo_policy(Some([0, 0]), false), None);
        assert_eq!(stereo_policy(None, true), Some(false));
        assert_eq!(stereo_policy(Some([0, 0]), true), Some(false));
        for assume in [false, true] {
            assert_eq!(stereo_policy(Some([3, 4]), assume), Some(true));
            assert_eq!(stereo_policy(Some([4, 3]), assume), None);
            assert_eq!(stereo_policy(Some([3, 0]), assume), None);
        }
    }
    #[test]
    fn partial_or_failed_submissions_latch_failure_instead_of_silently_dropping_audio() {
        let _serial = TEST_SERIAL.lock().unwrap();
        for result in [
            0,
            5,
            -c_long::from(libc::ENODEV),
            -c_long::from(libc::EPIPE),
        ] {
            disable();
            arm(
                device(),
                Format {
                    rate: 48000,
                    bits: 16,
                },
                Sample::S16,
            )
            .unwrap();
            assert!(transfer_result(result, 10) < 0);
            assert!(error().is_some());
        }
        disable();
        arm(
            device(),
            Format {
                rate: 48000,
                bits: 16,
            },
            Sample::S16,
        )
        .unwrap();
        assert_eq!(transfer_result(10, 10), 10);
        assert!(error().is_none());
        disable();
    }
    #[test]
    fn negotiation_requires_exact_rate_layout_access_and_adequate_valid_precision() {
        let source = Format {
            rate: 48000,
            bits: 24,
        };
        let good = Parameters {
            access: 3,
            format: 10,
            rate: 48000,
            dir: 0,
            channels: 2,
            bits: 32,
        };
        assert!(checked_parameters(source, Sample::S32, good).is_some());
        for bad in [
            Parameters {
                rate: 44100,
                ..good
            },
            Parameters {
                channels: 1,
                ..good
            },
            Parameters { bits: 16, ..good },
            Parameters { format: 2, ..good },
            Parameters { access: 0, ..good },
            Parameters { access: 4, ..good },
            Parameters { dir: 1, ..good },
            Parameters { bits: 64, ..good },
        ] {
            assert!(checked_parameters(source, Sample::S32, bad).is_none());
        }
    }
    #[test]
    fn hardware_mixer_and_mmap_commit_are_refused_without_touching_the_handles() {
        let _serial = TEST_SERIAL.lock().unwrap();
        disable();
        arm(
            device(),
            Format {
                rate: 48000,
                bits: 16,
            },
            Sample::S16,
        )
        .unwrap();
        let null = std::ptr::null_mut();
        assert!(unsafe { mixer_volume(null, 0, 100) } < 0);
        assert!(unsafe { mixer_volume_all(null, 100) } < 0);
        assert!(unsafe { mixer_db(null, 0, 0, 0) } < 0);
        assert!(unsafe { mixer_db_all(null, 0, 0) } < 0);
        assert!(unsafe { mixer_switch(null, 0, 1) } < 0);
        assert!(unsafe { mixer_switch_all(null, 1) } < 0);
        assert!(unsafe { snd_pcm_mmap_commit(null, 0, 1) } < 0);
        disable();
    }
    #[test]
    fn exported_guard_and_real_loader_are_distinct_and_null_delegation_does_not_abort() {
        let _serial = TEST_SERIAL.lock().unwrap();
        disable();
        assert!(
            available(),
            "ALSA exports must resolve to this executable and RTLD_NEXT to real libasound"
        );
        assert!(resolve(c"qoda_intentionally_missing_alsa_symbol").is_none());
        let mut handle = std::ptr::null_mut();
        assert_eq!(
            unsafe { snd_pcm_open(&mut handle, c"null".as_ptr(), 0, 1) },
            0
        );
        assert!(!handle.is_null());
        assert_eq!(unsafe { snd_pcm_close(handle) }, 0);
        disable();
    }
    #[test]
    fn indirect_route_and_unnegotiated_writes_are_denied_before_real_transfer() {
        let _serial = TEST_SERIAL.lock().unwrap();
        disable();
        arm(
            device(),
            Format {
                rate: 48000,
                bits: 24,
            },
            Sample::S32,
        )
        .unwrap();
        let mut handle = std::ptr::null_mut();
        assert!(unsafe { snd_pcm_open(&mut handle, c"null".as_ptr(), 0, 1) } < 0);
        assert!(handle.is_null());
        assert!(error().is_some());
        // No actual ALSA call can be made for this null/unregistered handle.
        assert!(unsafe { snd_pcm_writei(std::ptr::null_mut(), std::ptr::null(), 1) } < 0);
        assert!(unsafe { snd_pcm_writen(std::ptr::null_mut(), std::ptr::null_mut(), 1) } < 0);
        assert!(
            unsafe {
                snd_pcm_mmap_begin(
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            } < 0
        );
        disable();
    }
    #[test]
    fn c_boundary_contains_rust_unwinding_and_latches_a_failure() {
        let _serial = TEST_SERIAL.lock().unwrap();
        disable();
        assert_eq!(
            boundary(-libc::EIO, || panic!("synthetic guarded panic")),
            -libc::EIO
        );
        assert!(error().is_some());
        disable();
    }
}

pub fn verify_preflight() -> Result<(), &'static str> {
    let handle = {
        let g = gate();
        if g.handles.len() != 1 {
            return Err("Actual fidelity handle could not be identified");
        }
        g.handles
            .keys()
            .next()
            .copied()
            .ok_or("Actual fidelity handle unavailable")?
    };
    if permit(handle as *mut c_void) {
        Ok(())
    } else {
        Err(error().unwrap_or("Fidelity layout is unverifiable; no samples submitted."))
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn snd_pcm_mmap_commit(
    pcm: *mut c_void,
    offset: c_ulong,
    frames: c_ulong,
) -> c_long {
    boundary(-c_long::from(libc::EIO), || {
        if gate().expected.is_some() {
            return c_long::from(blocked("Fidelity refuses unqualified mmap transfers."));
        }
        let Some(real) = real!(
            c"snd_pcm_mmap_commit",
            unsafe extern "C" fn(*mut c_void, c_ulong, c_ulong) -> c_long
        ) else {
            return -c_long::from(libc::ENOSYS);
        };
        unsafe { real(pcm, offset, frames) }
    })
}
macro_rules! mixer_write {
    ($function:ident,$export:literal,$symbol:literal,($($arg:ident:$ty:ty),*))=>{
        #[unsafe(export_name=$export)]
        unsafe extern "C" fn $function($($arg:$ty),*)->c_int{
            boundary(-libc::EIO,||{
                if gate().expected.is_some(){return blocked("Fidelity refused a hardware mixer write; use Pause or the system mixer explicitly.");}
                let Some(real)=real!($symbol,unsafe extern "C" fn($($ty),*)->c_int)else{return -libc::ENOSYS;};
                unsafe{real($($arg),*)}
            })
        }
    }
}
mixer_write!(mixer_volume,"snd_mixer_selem_set_playback_volume",c"snd_mixer_selem_set_playback_volume",(elem:*mut c_void,channel:c_int,value:c_long));
mixer_write!(mixer_volume_all,"snd_mixer_selem_set_playback_volume_all",c"snd_mixer_selem_set_playback_volume_all",(elem:*mut c_void,value:c_long));
mixer_write!(mixer_db,"snd_mixer_selem_set_playback_dB",c"snd_mixer_selem_set_playback_dB",(elem:*mut c_void,channel:c_int,value:c_long,dir:c_int));
mixer_write!(mixer_db_all,"snd_mixer_selem_set_playback_dB_all",c"snd_mixer_selem_set_playback_dB_all",(elem:*mut c_void,value:c_long,dir:c_int));
mixer_write!(mixer_switch,"snd_mixer_selem_set_playback_switch",c"snd_mixer_selem_set_playback_switch",(elem:*mut c_void,channel:c_int,value:c_int));
mixer_write!(mixer_switch_all,"snd_mixer_selem_set_playback_switch_all",c"snd_mixer_selem_set_playback_switch_all",(elem:*mut c_void,value:c_int));
