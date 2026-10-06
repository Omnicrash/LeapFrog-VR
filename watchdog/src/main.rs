//! `leapfrog-watchdog <pid>`: waits for that process to end, however it ends, then stops the
//! UltraleapTracking service. The driver starts one per SteamVR run so a vrserver crash still
//! turns the Leap's camera and LEDs off; after a clean exit the service is already stopped and
//! this does nothing. No window, no console, no interpreter.
#![windows_subsystem = "windows"]

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Services::*;
use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, INFINITE};

const SYNCHRONIZE: u32 = 0x0010_0000;

fn main() {
    let Some(pid) = std::env::args().nth(1).and_then(|a| a.parse::<u32>().ok()) else {
        std::process::exit(2);
    };
    // SAFETY: Win32 API; the process handle is closed after the wait.
    unsafe {
        let process = OpenProcess(SYNCHRONIZE, 0, pid);
        if !process.is_null() {
            WaitForSingleObject(process, INFINITE);
            CloseHandle(process);
        }
        // An already-gone pid falls through to the stop as well: better off than on.
        let name: Vec<u16> = "UltraleapTracking".encode_utf16().chain(std::iter::once(0)).collect();
        let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            std::process::exit(1);
        }
        let service = OpenServiceW(manager, name.as_ptr(), SERVICE_STOP);
        if !service.is_null() {
            let mut status: SERVICE_STATUS = std::mem::zeroed();
            ControlService(service, SERVICE_CONTROL_STOP, &mut status);
            CloseServiceHandle(service);
        }
        CloseServiceHandle(manager);
    }
}
