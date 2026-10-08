//! `leapfrog-panel`: the driver's settings, written live into SteamVR.
//!
//! `leapfrog-panel overlay <pid>` runs the dashboard tab until SteamVR quits or the given process
//! (the vrserver that spawned it) ends; the driver starts one per SteamVR run. Its log is
//! `leapfrog-panel.log` next to the exe.
//!
//! `leapfrog-panel set <key> <value>` writes one setting from a terminal (`get <key>` reads it
//! back). The value's shape picks the type: `true`/`false`, an integer, a float, else a string.
//! The driver reads most of its settings once a second, so a write shows up in its log within
//! that.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};

mod ffi {
    use super::*;
    extern "C" {
        pub fn lfp_connect(dll: *const u16, overlay: c_int, err: *mut c_char, err_len: usize) -> c_int;
        pub fn lfp_disconnect();
        pub fn lfp_set_float(key: *const c_char, value: f32) -> c_int;
        pub fn lfp_set_bool(key: *const c_char, value: c_int) -> c_int;
        pub fn lfp_set_int(key: *const c_char, value: c_int) -> c_int;
        pub fn lfp_set_string(key: *const c_char, value: *const c_char) -> c_int;
        pub fn lfp_get_float(key: *const c_char, value: *mut f32) -> c_int;
        pub fn lfp_get_bool(key: *const c_char, value: *mut c_int) -> c_int;
        pub fn lfp_get_int(key: *const c_char, value: *mut c_int) -> c_int;
        pub fn lfp_get_string(key: *const c_char, value: *mut c_char, len: usize) -> c_int;
        pub fn lfp_error_name(code: c_int) -> *const c_char;
        pub fn lfp_overlay_open(thumbnail: *const c_char, err: *mut c_char, err_len: usize) -> c_int;
        pub fn lfp_overlay_frame() -> c_int;
        pub fn lfp_overlay_visible() -> c_int;
        pub fn lfp_overlay_close();
        pub fn lfp_process_gone(pid: u32) -> c_int;
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["set", key, value] => run(|| set(key, value)),
        ["get", key] => run(|| get(key)),
        ["overlay", pid] => overlay(pid.parse().unwrap_or(0)),
        _ => {
            eprintln!("usage: leapfrog-panel set <key> <value> | get <key> | overlay <vrserver pid>");
            2
        }
    };
    std::process::exit(code);
}

/// The dashboard tab. Connecting is retried for two minutes: the driver spawns the panel while
/// SteamVR is still coming up, and an overlay app can only init once the compositor is there.
fn overlay(parent: u32) -> i32 {
    let exe = std::env::current_exe().ok();
    let dir = exe.as_deref().and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or_default();
    let mut log = std::fs::File::create(dir.join("leapfrog-panel.log")).ok();
    let mut note = |line: String| {
        use std::io::Write;
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    };
    let started = std::time::Instant::now();
    loop {
        match connect(true) {
            Ok(()) => break,
            Err(e) if started.elapsed().as_secs() < 120 => {
                // SAFETY: plain Win32 process query.
                if parent != 0 && unsafe { ffi::lfp_process_gone(parent) } != 0 {
                    note(format!("vrserver {parent} gone before SteamVR was ready: {e}"));
                    return 0;
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            Err(e) => {
                note(format!("gave up connecting after two minutes: {e}"));
                return 1;
            }
        }
    }
    note(format!("connected as an overlay app after {:.1} s", started.elapsed().as_secs_f32()));
    let thumb = dir.join("..").join("..").join("resources").join("icons").join("right_hand_status_ready.png");
    let thumb_c = CString::new(thumb.to_string_lossy().as_bytes()).unwrap_or_default();
    let mut err = vec![0 as c_char; 256];
    // SAFETY: NUL-terminated path, error buffer of the given length.
    if unsafe { ffi::lfp_overlay_open(thumb_c.as_ptr(), err.as_mut_ptr(), err.len()) } != 0 {
        // SAFETY: the C side wrote a NUL-terminated string.
        note(format!("overlay not opened: {}", unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()));
        unsafe { ffi::lfp_disconnect() };
        return 1;
    }
    note("dashboard tab created".to_string());
    loop {
        // SAFETY: the overlay is open.
        let quit = unsafe { ffi::lfp_overlay_frame() } != 0;
        if quit {
            note("SteamVR quit".to_string());
            break;
        }
        // SAFETY: plain Win32 process query.
        if parent != 0 && unsafe { ffi::lfp_process_gone(parent) } != 0 {
            note(format!("vrserver {parent} gone"));
            break;
        }
        // 60 Hz while the tab is up; a slow poll while it is not.
        let visible = unsafe { ffi::lfp_overlay_visible() } != 0;
        std::thread::sleep(std::time::Duration::from_millis(if visible { 16 } else { 100 }));
    }
    // SAFETY: open above.
    unsafe {
        ffi::lfp_overlay_close();
        ffi::lfp_disconnect();
    }
    0
}

fn run(f: impl FnOnce() -> Result<(), String>) -> i32 {
    match connect(false).and_then(|()| {
        let r = f();
        // SAFETY: connected above.
        unsafe { ffi::lfp_disconnect() };
        r
    }) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// VR_Init as a background app (or an overlay app) through the runtime SteamVR registered in
/// openvrpaths.vrpath.
fn connect(overlay: bool) -> Result<(), String> {
    let dll = runtime_dir()?.join("bin").join("win64").join("openvr_api.dll");
    let wide: Vec<u16> = dll.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    let mut err = vec![0 as c_char; 256];
    // SAFETY: NUL-terminated path, error buffer of the given length.
    if unsafe { ffi::lfp_connect(wide.as_ptr(), overlay as c_int, err.as_mut_ptr(), err.len()) } != 0 {
        // SAFETY: the shim wrote a NUL-terminated string.
        let text = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy();
        return Err(format!("SteamVR: {text}"));
    }
    Ok(())
}

/// The first `runtime` entry of %LOCALAPPDATA%\openvr\openvrpaths.vrpath.
fn runtime_dir() -> Result<std::path::PathBuf, String> {
    let local = std::env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set")?;
    let path = std::path::Path::new(&local).join("openvr").join("openvrpaths.vrpath");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let after = text.split("\"runtime\"").nth(1).ok_or("openvrpaths.vrpath: no runtime entry")?;
    let start = after.find('[').and_then(|i| after[i..].find('"').map(|j| i + j + 1)).ok_or("openvrpaths.vrpath: runtime list unreadable")?;
    let end = after[start..].find('"').ok_or("openvrpaths.vrpath: runtime path unterminated")?;
    Ok(std::path::PathBuf::from(after[start..start + end].replace("\\\\", "\\").replace("\\/", "/")))
}

fn check(code: c_int) -> Result<(), String> {
    if code == 0 {
        return Ok(());
    }
    // SAFETY: the shim returns a static string.
    let name = unsafe { CStr::from_ptr(ffi::lfp_error_name(code)) }.to_string_lossy();
    Err(format!("settings error {code} ({name})"))
}

fn set(key: &str, value: &str) -> Result<(), String> {
    let k = CString::new(key).map_err(|_| "bad key")?;
    // SAFETY: NUL-terminated key and value.
    let code = unsafe {
        if value == "true" || value == "false" {
            ffi::lfp_set_bool(k.as_ptr(), (value == "true") as c_int)
        } else if let Ok(i) = value.parse::<i32>() {
            ffi::lfp_set_int(k.as_ptr(), i)
        } else if let Ok(f) = value.parse::<f32>() {
            ffi::lfp_set_float(k.as_ptr(), f)
        } else {
            let v = CString::new(value).map_err(|_| "bad value")?;
            ffi::lfp_set_string(k.as_ptr(), v.as_ptr())
        }
    };
    check(code)?;
    println!("{key} = {value}");
    Ok(())
}

/// Tries each type in turn; the store knows which one the key holds.
fn get(key: &str) -> Result<(), String> {
    let k = CString::new(key).map_err(|_| "bad key")?;
    let mut f = 0f32;
    let mut i = 0 as c_int;
    let mut s = vec![0 as c_char; 256];
    // SAFETY: NUL-terminated key, out-parameters of the right size.
    unsafe {
        if ffi::lfp_get_float(k.as_ptr(), &mut f) == 0 {
            println!("{key} = {f}");
        } else if ffi::lfp_get_int(k.as_ptr(), &mut i) == 0 {
            println!("{key} = {i}");
        } else if ffi::lfp_get_bool(k.as_ptr(), &mut i) == 0 {
            println!("{key} = {}", i != 0);
        } else {
            check(ffi::lfp_get_string(k.as_ptr(), s.as_mut_ptr(), s.len()))?;
            println!("{key} = {}", CStr::from_ptr(s.as_ptr()).to_string_lossy());
        }
    }
    Ok(())
}
