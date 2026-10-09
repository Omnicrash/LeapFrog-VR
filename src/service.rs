//! The UltraleapTracking Windows service, started with the driver and stopped with it: paused,
//! the service still runs the camera and its infrared LEDs, only stopped turns them off. The
//! user's account was granted start/stop on the service (by the installer; a Gemini reinstall
//! resets it), so no elevation is involved. Also the helper processes started next to the DLL
//! and the process checks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::log::log;

const SERVICE: &str = "UltraleapTracking";

#[cfg(all(windows, not(feature = "host-stubs")))]
mod win {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_NOT_ACTIVE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ};
    use windows_sys::Win32::System::Services::*;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    /// Starts or stops the service and waits up to 10 s for it to settle. Ok(true) when it is in
    /// the wanted state, Ok(false) when it did not get there in time, Err(code) without access.
    pub fn set(run: bool) -> Result<bool, u32> {
        let name = wide(super::SERVICE);
        // SAFETY: Win32 service control API, handles closed on every path below.
        unsafe {
            let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
            if manager.is_null() {
                return Err(GetLastError());
            }
            let service = OpenServiceW(manager, name.as_ptr(), SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS);
            if service.is_null() {
                let error = GetLastError();
                CloseServiceHandle(manager);
                return Err(error);
            }
            let mut status: SERVICE_STATUS = std::mem::zeroed();
            let ok = if run {
                StartServiceW(service, 0, std::ptr::null()) != 0
            } else {
                ControlService(service, SERVICE_CONTROL_STOP, &mut status) != 0
            };
            let result = if ok {
                Ok(())
            } else {
                match GetLastError() {
                    ERROR_SERVICE_ALREADY_RUNNING | ERROR_SERVICE_NOT_ACTIVE => Ok(()),
                    error => Err(error),
                }
            };
            let settled = match result {
                Ok(()) => {
                    let wanted = if run { SERVICE_RUNNING } else { SERVICE_STOPPED };
                    let deadline = Instant::now() + Duration::from_secs(10);
                    loop {
                        if QueryServiceStatus(service, &mut status) != 0 && status.dwCurrentState == wanted {
                            break Ok(true);
                        }
                        if Instant::now() > deadline {
                            break Ok(false);
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                Err(error) => Err(error),
            };
            CloseServiceHandle(service);
            CloseServiceHandle(manager);
            settled
        }
    }

    /// Whether a process with this image name runs (case-insensitive), via a process snapshot;
    /// no allocation per entry, so it is cheap enough for a background poll.
    pub fn process_running(name: &str) -> bool {
        let wanted: Vec<u16> = name.encode_utf16().map(|c| ascii_lower(c)).collect();
        // SAFETY: Win32 ToolHelp API, handle closed below; the entry struct is sized before use.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return false;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut found = false;
            let mut ok = Process32FirstW(snapshot, &mut entry) != 0;
            while ok && !found {
                let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                found = end == wanted.len() && entry.szExeFile[..end].iter().zip(&wanted).all(|(&a, &b)| ascii_lower(a) == b);
                ok = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
            found
        }
    }

    fn ascii_lower(c: u16) -> u16 {
        if (b'A' as u16..=b'Z' as u16).contains(&c) { c + 32 } else { c }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The service's own control tool, next to LeapSvc.exe (from the service's ImagePath).
    fn leapctl() -> Result<String, String> {
        let key = wide(r"SYSTEM\CurrentControlSet\Services\UltraleapTracking");
        let value = wide("ImagePath");
        let mut buf = [0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: buffer and size passed together; RegGetValueW null-terminates REG_SZ data.
        let status = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut size)
        };
        if status != 0 {
            return Err(format!("service not installed (registry error {status})"));
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let image = String::from_utf16_lossy(&buf[..end]);
        let image = image.trim_matches('"');
        match image.find("LeapSvc.exe") {
            Some(i) => Ok(format!("{}leapctl.exe", &image[..i])),
            None => Err(format!("unexpected service path {image}")),
        }
    }

    /// The folder this DLL was loaded from (bin/win64 of the driver).
    fn own_directory() -> Option<std::path::PathBuf> {
        use windows_sys::Win32::System::LibraryLoader::{
            GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        };
        let mut module = std::ptr::null_mut();
        let mut path = [0u16; 1024];
        // SAFETY: the address of this function lies inside the DLL; buffer and size go together.
        let n = unsafe {
            if GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                own_directory as *const () as *const u16,
                &mut module,
            ) == 0
            {
                return None;
            }
            GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32)
        };
        if n == 0 {
            return None;
        }
        let file = std::path::PathBuf::from(String::from_utf16_lossy(&path[..n as usize]));
        file.parent().map(|p| p.to_path_buf())
    }

    /// Starts a helper exe from this DLL's folder with vrserver's pid as the last argument, no
    /// window, no inherited stdio. Detached helpers outlive vrserver.
    fn spawn(exe: &str, args: &[&str], detached: bool) -> Result<u32, String> {
        let path = own_directory().ok_or("own path unknown")?.join(exe);
        let child = Command::new(&path)
            .args(args)
            .arg(std::process::id().to_string())
            .creation_flags(if detached { DETACHED_PROCESS } else { CREATE_NO_WINDOW })
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", path.display()))?;
        Ok(child.id())
    }

    /// A watchdog for the case Cleanup never runs (vrserver crashed or killed): it waits on this
    /// very process and then stops the service; after a clean exit it finds it stopped already.
    pub fn spawn_watchdog() -> Result<u32, String> {
        spawn("leapfrog-watchdog.exe", &[], true)
    }

    /// The settings panel's `ensure`: registers the panel as a startup overlay app the first
    /// time; afterwards SteamVR launches it itself, per its "Choose Startup Overlay Apps" list.
    pub fn spawn_panel() -> Result<u32, String> {
        spawn("leapfrog-panel.exe", &["ensure"], false)
    }

    /// `leapctl config tracking resume|pause`: the saved pause state of the service. LeapSetPause
    /// from the SDK reports success and changes nothing on Gemini 5.20.
    pub fn tracking(state: &str) -> Result<i32, String> {
        let tool = leapctl()?;
        let status = Command::new(&tool)
            .args(["config", "tracking", state])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| format!("could not run {tool}: {e}"))?;
        Ok(status.code().unwrap_or(-1))
    }
}

#[cfg(not(all(windows, not(feature = "host-stubs"))))]
mod win {
    pub fn set(_run: bool) -> Result<bool, u32> {
        Ok(true)
    }

    pub fn tracking(_state: &str) -> Result<i32, String> {
        Ok(0)
    }

    pub fn spawn_watchdog() -> Result<u32, String> {
        Ok(0)
    }

    pub fn spawn_panel() -> Result<u32, String> {
        Ok(0)
    }

    pub fn process_running(_name: &str) -> bool {
        false
    }
}

/// Starts the service, optionally resumes tracking, and spawns the watchdog.
#[cfg_attr(feature = "host-stubs", allow(dead_code))]
pub fn start(resume_tracking: bool) {
    report("start", win::set(true));
    // A saved pause (the Ultraleap tray app, leapctl, an old helper) survives restarts and means
    // a running service with no hands, and nothing in SteamVR says why.
    if resume_tracking {
        match win::tracking("resume") {
            Ok(0) => log!("Leap tracking resumed"),
            Ok(code) => log!("Leap tracking resume: leapctl exit {code}"),
            Err(why) => log!("Leap tracking resume skipped: {why}"),
        }
    }
    match win::spawn_watchdog() {
        Ok(pid) => log!("service watchdog started (pid {pid}): stops the service if vrserver dies without cleanup"),
        Err(why) => log!("service watchdog not started: {why}"),
    }
}

pub fn stop() {
    report("stop", win::set(false));
}

/// The settings panel: a dashboard tab in SteamVR's menu, registered as a startup overlay app.
pub fn start_panel() {
    match win::spawn_panel() {
        Ok(pid) => log!("settings panel ensure started (pid {pid})"),
        Err(why) => log!("settings panel not started: {why}"),
    }
}

fn report(what: &str, result: Result<bool, u32>) {
    match result {
        Ok(true) => log!("{SERVICE} service: {what} done"),
        Ok(false) => log!("{SERVICE} service: {what} requested, not settled within 10 s"),
        Err(code) => log!("{SERVICE} service: {what} failed, Win32 error {code} (no access? run leapfrog-register.exe register)"),
    }
}

/// Whether a process by image name is running, polled once a second on its own thread, so the
/// frame loop reads a flag instead of taking a process snapshot.
pub struct ProcessWatch {
    running: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ProcessWatch {
    pub fn new(name: &'static str) -> ProcessWatch {
        let running = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (running, stop) = (running.clone(), stop.clone());
            std::thread::Builder::new()
                .name("leapfrog-procwatch".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        running.store(win::process_running(name), Ordering::Relaxed);
                        for _ in 0..10 {
                            if stop.load(Ordering::Relaxed) {
                                return;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                })
                .ok()
        };
        ProcessWatch { running, stop, thread }
    }

    pub fn running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

impl Drop for ProcessWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
