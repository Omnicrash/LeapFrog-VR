//! What needs Win32 directly: elevation (is this process elevated, relaunch it through UAC), the
//! system folder, the date, the user's SID, whether a process runs, and the Apps entry's key.

use std::path::PathBuf;
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, INVALID_HANDLE_VALUE, SYSTEMTIME};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_ANY,
};
use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemDirectoryW};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, INFINITE};
use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub fn is_elevated() -> bool {
    // SAFETY: plain call.
    unsafe { IsUserAnAdmin() != 0 }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(s: &[u16]) -> String {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

/// One argument quoted the way CommandLineToArgvW reads it back: backslashes are literal except
/// before a quote, where they double, and an embedded quote is escaped.
fn quote_arg(arg: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat('\\').take(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            c => {
                out.extend(std::iter::repeat('\\').take(slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    // The closing quote follows, so trailing backslashes double too.
    out.extend(std::iter::repeat('\\').take(slashes * 2));
    out.push('"');
    out
}

/// Runs this exe again with `args` through the "runas" verb and returns its exit code.
pub fn relaunch_elevated(args: &[&str]) -> Result<u32, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let params: Vec<String> = args.iter().map(|a| quote_arg(a)).collect();
    let (file, params, verb) = (wide(&exe.to_string_lossy()), wide(&params.join(" ")), wide("runas"));
    // SAFETY: the struct is sized and zeroed; the strings outlive the call; the handle is closed.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_SHOWNORMAL as i32;
        if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() {
            return Err("elevation was refused".into());
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        Ok(code)
    }
}

/// A tool of Windows' own, from the system folder: a bare name would search the current
/// directory and PATH first.
pub fn system_tool(name: &str) -> PathBuf {
    let mut buf = [0u16; 260];
    // SAFETY: the buffer and its length match.
    let len = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let dir = if len > 0 && len < buf.len() { from_wide(&buf[..len]) } else { r"C:\Windows\System32".to_string() };
    PathBuf::from(dir).join(name)
}

/// Today as yyyyMMdd, the form of the Apps entry's InstallDate.
pub fn local_date() -> String {
    // SAFETY: the struct is plain data, filled by the call.
    let t: SYSTEMTIME = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    format!("{:04}{:02}{:02}", t.wYear, t.wMonth, t.wDay)
}

/// The SID of the user this process runs as, from its token.
pub fn current_user_sid() -> Result<String, String> {
    let err = || "could not read the user's SID".to_string();
    // SAFETY: the token handle is closed; the buffer is sized by the first call and u64-aligned
    // for TOKEN_USER; the string ConvertSidToStringSidW allocates is freed with LocalFree.
    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(err());
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) != 0;
        CloseHandle(token);
        if !ok {
            return Err(err());
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(err());
        }
        let mut n = 0;
        while *text.add(n) != 0 {
            n += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text.cast());
        Ok(sid)
    }
}

/// Whether a process with this image name (case ignored) is running.
pub fn process_running(image: &str) -> bool {
    // SAFETY: the snapshot handle is closed; the entry is sized before the first call.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more {
            if from_wide(&entry.szExeFile).eq_ignore_ascii_case(image) {
                found = true;
                break;
            }
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        found
    }
}

pub enum RegValue {
    Sz(String),
    Dword(u32),
}

/// Creates HKCU\`key` if needed and writes the values.
pub fn reg_set_values(key: &str, values: &[(&str, RegValue)]) -> Result<(), String> {
    let mut hkey: HKEY = std::ptr::null_mut();
    // SAFETY: the strings outlive each call; the key is closed.
    unsafe {
        let rc = RegCreateKeyExW(HKEY_CURRENT_USER, wide(key).as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut hkey, std::ptr::null_mut());
        if rc != ERROR_SUCCESS {
            return Err(format!("cannot create HKCU\\{key} (error {rc})"));
        }
        let mut result = Ok(());
        for (name, value) in values {
            let rc = match value {
                RegValue::Sz(s) => {
                    let data = wide(s);
                    RegSetValueExW(hkey, wide(name).as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32)
                }
                RegValue::Dword(d) => RegSetValueExW(hkey, wide(name).as_ptr(), 0, REG_DWORD, (d as *const u32).cast(), 4),
            };
            if rc != ERROR_SUCCESS {
                result = Err(format!("cannot write {name} (error {rc})"));
                break;
            }
        }
        RegCloseKey(hkey);
        result
    }
}

/// Deletes HKCU\`key` and everything under it; a key that is not there is fine.
pub fn reg_delete_tree(key: &str) -> Result<(), String> {
    // SAFETY: the string outlives the call.
    let rc = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(key).as_ptr()) };
    if rc == ERROR_SUCCESS || rc == 2 {
        Ok(())
    } else {
        Err(format!("cannot delete HKCU\\{key} (error {rc})"))
    }
}

/// Whether HKCU\`key` holds the value `name`.
pub fn reg_has_value(key: &str, name: &str) -> bool {
    // SAFETY: a size query only: no data buffer.
    unsafe {
        let mut size = 0u32;
        RegGetValueW(HKEY_CURRENT_USER, wide(key).as_ptr(), wide(name).as_ptr(), RRF_RT_ANY, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) == ERROR_SUCCESS
    }
}
