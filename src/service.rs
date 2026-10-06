//! The UltraleapTracking Windows service, started with the driver and stopped with it: paused,
//! the service still runs the camera and its infrared LEDs, only stopped turns them off. The
//! user's account was granted start/stop on the service (`sc sdset`, once; a Gemini reinstall
//! resets it), so no elevation is involved. From leaphint (E:\Games\VR\leaphint), folded in.

use crate::log::log;

#[cfg(all(windows, not(feature = "host-stubs")))]
mod win {
    use std::time::{Duration, Instant};

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
}

#[cfg(not(all(windows, not(feature = "host-stubs"))))]
mod win {
    pub fn set(_run: bool) -> Result<bool, u32> {
        Ok(true)
    }
}

#[cfg_attr(feature = "host-stubs", allow(dead_code))]
pub fn start() {
    report("start", win::set(true));
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
