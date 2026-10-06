//! Lines into vrserver's log (`vrserver.txt`), prefixed so they're easy to grep.

use std::ffi::CString;

pub fn write(msg: &str) {
    let line = format!("[LeapFrog] {msg}");
    if let Ok(c) = CString::new(line) {
        // SAFETY: the shim null-checks the host's log interface; the string outlives the call.
        unsafe { crate::host::ffi::lfh_log(c.as_ptr()) };
    }
}

macro_rules! log {
    ($($arg:tt)*) => { $crate::log::write(&format!($($arg)*)) };
}
pub(crate) use log;
