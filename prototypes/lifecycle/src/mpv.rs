// THROWAWAY: the handle and every borrowed event pointer stay on one owner thread.
use serde_json::json;
use std::{
    ffi::{CString, c_char, c_int, c_void},
    sync::mpsc,
    thread,
    time::Duration,
};
use tokio::sync::mpsc::UnboundedSender;
#[repr(C)]
struct Event {
    id: c_int,
    error: c_int,
    userdata: u64,
    data: *mut c_void,
}
#[repr(C)]
struct Property {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}
#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_create() -> *mut c_void;
    fn mpv_set_option_string(h: *mut c_void, n: *const c_char, v: *const c_char) -> c_int;
    fn mpv_initialize(h: *mut c_void) -> c_int;
    fn mpv_observe_property(h: *mut c_void, u: u64, n: *const c_char, f: c_int) -> c_int;
    fn mpv_command(h: *mut c_void, a: *const *const c_char) -> c_int;
    fn mpv_wait_event(h: *mut c_void, t: f64) -> *const Event;
    fn mpv_terminate_destroy(h: *mut c_void);
}
pub fn start(
    path: String,
    tx: UnboundedSender<super::Msg>,
) -> (mpsc::Sender<Vec<String>>, thread::JoinHandle<()>) {
    let (commands, rx) = mpsc::channel::<Vec<String>>();
    let join = thread::spawn(move || unsafe {
        let h = mpv_create();
        assert!(!h.is_null());
        for (k, v) in [
            ("config", "no"),
            ("load-scripts", "no"),
            ("vo", "null"),
            ("ao", "null"),
            ("pause", "yes"),
            ("idle", "yes"),
            ("terminal", "no"),
            ("volume", "100"),
            ("volume-gain", "0"),
            ("keep-open", "yes"),
        ] {
            assert!(
                mpv_set_option_string(
                    h,
                    CString::new(k).unwrap().as_ptr(),
                    CString::new(v).unwrap().as_ptr()
                ) >= 0
            );
        }
        assert!(mpv_initialize(h) >= 0);
        for (n, f) in [("pause", 3), ("time-pos", 5), ("volume", 5)] {
            assert!(mpv_observe_property(h, 0, CString::new(n).unwrap().as_ptr(), f) >= 0);
        }
        let command = |args: Vec<String>| {
            let strings: Vec<_> = args
                .iter()
                .map(|a| CString::new(a.as_str()).unwrap())
                .collect();
            let mut ptrs: Vec<_> = strings.iter().map(|a| a.as_ptr()).collect();
            ptrs.push(std::ptr::null());
            mpv_command(h, ptrs.as_ptr())
        };
        assert!(command(vec!["loadfile".into(), path]) >= 0);
        loop {
            match rx.try_recv() {
                Ok(a) if a[0] == "destroy" => break,
                Ok(a) => {
                    let code = command(a);
                    if code < 0 {
                        let _ = tx.send(super::Msg::Mpv(json!({"error":code})));
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => break,
                _ => {}
            }
            let e = &*mpv_wait_event(h, 0.01);
            if e.id == 8 {
                let _ = tx.send(super::Msg::Mpv(json!({"loaded":true})));
            }
            if e.id == 22 && !e.data.is_null() {
                let p = &*(e.data as *const Property);
                if !p.data.is_null() {
                    let value = match p.format {
                        3 => json!({"paused":*(p.data as *const c_int)!=0}),
                        5 => {
                            if std::ffi::CStr::from_ptr(p.name).to_bytes() == b"volume" {
                                json!({"volume":*(p.data as *const f64)})
                            } else {
                                json!({"position":*(p.data as *const f64)})
                            }
                        }
                        _ => continue,
                    };
                    let _ = tx.send(super::Msg::Mpv(value));
                }
            }
        }
        mpv_terminate_destroy(h); // No callbacks registered, no pointer leaves this thread.
        thread::sleep(Duration::from_millis(1));
    });
    (commands, join)
}
