//! The UltraleapTracking Windows service, started with the driver and stopped with it: paused,
//! the service still runs the camera and its infrared LEDs, only stopped turns them off. The
//! user's account was granted start/stop on the service (`sc sdset`, once; a Gemini reinstall
//! resets it), so no elevation is involved. From leaphint (E:\Games\VR\leaphint), folded in.

use crate::log::log;

#[cfg(all(windows, not(feature = "host-stubs")))]
mod win {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ};

    use windows_sys::Win32::Foundation::{ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_NOT_ACTIVE, GetLastError};
    use windows_sys::Win32::System::Services::*;

    const NAME: &[u16] = &[
        b'U' as u16, b'l' as u16, b't' as u16, b'r' as u16, b'a' as u16, b'l' as u16, b'e' as u16, b'a' as u16, b'p' as u16,
        b'T' as u16, b'r' as u16, b'a' as u16, b'c' as u16, b'k' as u16, b'i' as u16, b'n' as u16, b'g' as u16, 0,
    ];

    /// Starts or stops the service and waits up to 10 s for it to settle. Ok(true) when it is in
    /// the wanted state, Ok(false) when it did not get there in time, Err(code) without access.
    pub fn set(run: bool) -> Result<bool, u32> {
        // SAFETY: Win32 service control API, handles closed on every path below.
        unsafe {
            let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
            if manager.is_null() {
                return Err(GetLastError());
            }
            let service = OpenServiceW(manager, NAME.as_ptr(), SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS);
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

    /// A watchdog for the case Cleanup never runs (vrserver crashed or killed): a hidden
    /// PowerShell that waits on this very process and then stops the service. Detached, so it
    /// outlives vrserver; on a clean exit the service is already stopped and it does nothing.
    pub fn spawn_watchdog() -> Result<u32, String> {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        let pid = std::process::id();
        let script = format!(
            "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Stop-Service -Name UltraleapTracking -ErrorAction SilentlyContinue"
        );
        let child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start powershell: {e}"))?;
        Ok(child.id())
    }

    /// `leapctl config tracking resume|pause`: the saved pause state of the service. LeapSetPause
    /// from the SDK reports success and changes nothing on Gemini 5.20.
    pub fn tracking(state: &str) -> Result<i32, String> {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
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
}

#[cfg_attr(feature = "host-stubs", allow(dead_code))]
pub fn start() {
    report("start", win::set(true));
    // A pause left behind (the old helper paused at every SteamVR exit) survives restarts and
    // means a running service with no hands, so every start resumes.
    match win::tracking("resume") {
        Ok(0) => log!("Leap tracking resumed"),
        Ok(code) => log!("Leap tracking resume: leapctl exit {code}"),
        Err(why) => log!("Leap tracking resume skipped: {why}"),
    }
    match win::spawn_watchdog() {
        Ok(pid) => log!("service watchdog started (pid {pid}): stops the service if vrserver dies without cleanup"),
        Err(why) => log!("service watchdog not started: {why}"),
    }
}

pub fn stop() {
    report("stop", win::set(false));
}

fn report(what: &str, result: Result<bool, u32>) {
    match result {
        Ok(true) => log!("UltraleapTracking service: {what} done"),
        Ok(false) => log!("UltraleapTracking service: {what} requested, not settled within 10 s"),
        Err(code) => log!("UltraleapTracking service: {what} failed, Win32 error {code} (no access? see doc/Design.md)"),
    }
}
