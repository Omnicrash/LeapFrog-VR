//! `leapfrog-setup.exe`: installs the LeapFrog SteamVR driver from the folder this exe sits in,
//! registers it with SteamVR and Windows (Settings > Apps), sets up the UltraleapTracking
//! service, and takes all of it out again. No arguments: interactive. Subcommands:
//!
//!   install [--to DIR] [--in-place]            default DIR: %LOCALAPPDATA%\LeapFrog\driver
//!   uninstall [--purge-settings] [--restore-service]
//!   status
//!   common: --quiet (no output, no prompts), --pause (wait for Enter at the end)
//!
//! SteamVR's registration (vrpathreg) and the Windows entry are per user; the service step
//! needs an elevated process, so install relaunches itself elevated when it is not.

mod win;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

const SERVICE: &str = "UltraleapTracking";
const DRIVER_NAME: &str = "leapfrog";
const APP_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\LeapFrog";
/// Query status, start, stop, read control: what the driver needs from the service.
const SERVICE_ACE_RIGHTS: &str = "LCRPWPRC";

struct Options {
    command: String,
    to: Option<PathBuf>,
    in_place: bool,
    purge_settings: bool,
    restore_service: bool,
    quiet: bool,
    pause: bool,
    /// Uninstall relaunched from a temp copy: the folder to remove.
    folder: Option<PathBuf>,
    /// Elevated relaunch: the unelevated user's SID.
    user_sid: Option<String>,
}

fn main() {
    let opts = parse(std::env::args().skip(1).collect());
    let code = match run(&opts) {
        Ok(()) => 0,
        Err(e) => {
            say(&opts, &format!("Error: {e}"));
            1
        }
    };
    if opts.pause && !opts.quiet {
        println!("\nPress Enter to close.");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
    std::process::exit(code);
}

fn parse(args: Vec<String>) -> Options {
    let mut o = Options {
        command: String::new(),
        to: None,
        in_place: false,
        purge_settings: false,
        restore_service: false,
        quiet: false,
        pause: false,
        folder: None,
        user_sid: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "install" | "uninstall" | "status" => o.command = args[i].clone(),
            "--to" => {
                i += 1;
                o.to = args.get(i).map(PathBuf::from);
            }
            "--folder" => {
                i += 1;
                o.folder = args.get(i).map(PathBuf::from);
            }
            "--user-sid" => {
                i += 1;
                o.user_sid = args.get(i).cloned();
            }
            "--in-place" => o.in_place = true,
            "--purge-settings" => o.purge_settings = true,
            "--restore-service" => o.restore_service = true,
            "--quiet" => o.quiet = true,
            "--pause" => o.pause = true,
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    o
}

fn say(o: &Options, text: &str) {
    if !o.quiet {
        println!("{text}");
    }
}

fn run(o: &Options) -> Result<(), String> {
    match o.command.as_str() {
        "status" => {
            print!("{}", status_text()?);
            Ok(())
        }
        "install" => install(o),
        "uninstall" => uninstall(o),
        _ => interactive(o),
    }
}

// ---------------------------------------------------------------------------------------------
// Where things are

fn exe_dir() -> Result<PathBuf, String> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .ok_or_else(|| "own location unknown".to_string())
}

fn run_cmd(program: &str, args: &[&str]) -> Result<(bool, String), String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("could not run {program}: {e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

struct SteamVr {
    vrpathreg: PathBuf,
    config_dir: PathBuf,
}

/// SteamVR's locations from %LOCALAPPDATA%\openvr\openvrpaths.vrpath (a small JSON file).
fn steamvr() -> Result<SteamVr, String> {
    let local = std::env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set")?;
    let file = Path::new(&local).join("openvr").join("openvrpaths.vrpath");
    let text = std::fs::read_to_string(&file).map_err(|_| "SteamVR is not installed (no openvrpaths.vrpath)".to_string())?;
    let runtime = json_first_string(&text, "runtime").ok_or("openvrpaths.vrpath has no runtime")?;
    let config = json_first_string(&text, "config").ok_or("openvrpaths.vrpath has no config path")?;
    let vrpathreg = Path::new(&runtime).join("bin").join("win64").join("vrpathreg.exe");
    if !vrpathreg.exists() {
        return Err(format!("vrpathreg not found at {}", vrpathreg.display()));
    }
    Ok(SteamVr { vrpathreg, config_dir: PathBuf::from(config) })
}

/// First string of the array under `key` in a small JSON text; JSON escapes undone.
fn json_first_string(text: &str, key: &str) -> Option<String> {
    let k = format!("\"{key}\"");
    let rest = &text[text.find(&k)? + k.len()..];
    let rest = &rest[rest.find('[')? + 1..];
    let start = rest.find('"')? + 1;
    let mut out = String::new();
    let mut chars = rest[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                '"' => out.push('"'),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            c => out.push(c),
        }
    }
    None
}

impl SteamVr {
    /// The registered path of the leapfrog driver, from `vrpathreg show`.
    fn registered(&self) -> Result<Option<PathBuf>, String> {
        let (_, text) = run_cmd(&self.vrpathreg.to_string_lossy(), &["show"])?;
        Ok(text
            .lines()
            .map(str::trim)
            .filter_map(|l| l.strip_prefix(&format!("{DRIVER_NAME} : ")))
            .map(|p| PathBuf::from(p.trim()))
            .next())
    }

    fn add_driver(&self, dir: &Path) -> Result<(), String> {
        let (ok, text) = run_cmd(&self.vrpathreg.to_string_lossy(), &["adddriver", &dir.to_string_lossy()])?;
        if ok { Ok(()) } else { Err(format!("vrpathreg adddriver failed: {}", text.trim())) }
    }

    fn remove_driver(&self, dir: &Path) -> Result<(), String> {
        let (ok, text) = run_cmd(&self.vrpathreg.to_string_lossy(), &["removedriver", &dir.to_string_lossy()])?;
        if ok { Ok(()) } else { Err(format!("vrpathreg removedriver failed: {}", text.trim())) }
    }
}

fn steamvr_running() -> bool {
    run_cmd("tasklist.exe", &["/FI", "IMAGENAME eq vrserver.exe", "/NH"]).map(|(_, t)| t.to_ascii_lowercase().contains("vrserver.exe")).unwrap_or(false)
}

// ---------------------------------------------------------------------------------------------
// The tracking service

struct ServiceInfo {
    exists: bool,
    start_type: String,
    state: String,
    sddl: String,
}

fn service_info() -> Result<ServiceInfo, String> {
    let (ok, qc) = run_cmd("sc.exe", &["qc", SERVICE])?;
    if !ok {
        return Ok(ServiceInfo { exists: false, start_type: String::new(), state: String::new(), sddl: String::new() });
    }
    let field = |text: &str, name: &str| {
        text.lines()
            .find(|l| l.contains(name))
            .and_then(|l| l.split(':').nth(1))
            .map(|v| v.split_whitespace().skip(1).collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    };
    let (_, query) = run_cmd("sc.exe", &["query", SERVICE])?;
    let (_, sd) = run_cmd("sc.exe", &["sdshow", SERVICE])?;
    Ok(ServiceInfo {
        exists: true,
        start_type: field(&qc, "START_TYPE"),
        state: field(&query, "STATE"),
        sddl: sd.lines().map(str::trim).find(|l| l.starts_with("D:")).unwrap_or("").to_string(),
    })
}

fn current_user_sid() -> Result<String, String> {
    let (_, text) = run_cmd("whoami.exe", &["/user", "/fo", "csv", "/nh"])?;
    text.split(',').last().map(|s| s.trim().trim_matches('"').to_string()).filter(|s| s.starts_with("S-1-")).ok_or_else(|| "could not read the user's SID".to_string())
}

fn user_has_service_rights(sddl: &str, sid: &str) -> bool {
    sddl.contains(&format!(";;;{sid})")) && sddl.contains(&format!("(A;;{SERVICE_ACE_RIGHTS};;;{sid})"))
}

/// Manual start, and start/stop rights for `sid`. Needs an elevated process.
fn configure_service(sid: &str) -> Result<(), String> {
    let (ok, text) = run_cmd("sc.exe", &["config", SERVICE, "start=", "demand"])?;
    if !ok {
        return Err(format!("sc config failed (not elevated?): {}", text.trim()));
    }
    let info = service_info()?;
    if user_has_service_rights(&info.sddl, sid) {
        return Ok(());
    }
    // Append the user's ACE to the DACL; an SACL part (S:...) if present stays after it.
    let (dacl, rest) = match info.sddl.find("S:") {
        Some(i) => info.sddl.split_at(i),
        None => (info.sddl.as_str(), ""),
    };
    let sddl = format!("{dacl}(A;;{SERVICE_ACE_RIGHTS};;;{sid}){rest}");
    let (ok, text) = run_cmd("sc.exe", &["sdset", SERVICE, &sddl])?;
    if ok { Ok(()) } else { Err(format!("sc sdset failed: {}", text.trim())) }
}

// ---------------------------------------------------------------------------------------------
// Windows "Apps" entry

fn register_app(dir: &Path, version: &str) -> Result<(), String> {
    let exe = dir.join("leapfrog-setup.exe");
    let size_kb = dir_size(dir) / 1024;
    let date = run_cmd("powershell.exe", &["-NoProfile", "-Command", "Get-Date -Format yyyyMMdd"]).map(|(_, t)| t.trim().to_string()).unwrap_or_default();
    let values: Vec<(&str, &str, String)> = vec![
        ("DisplayName", "REG_SZ", "LeapFrog".into()),
        ("DisplayVersion", "REG_SZ", version.into()),
        ("Publisher", "REG_SZ", "Omnicrash".into()),
        ("InstallLocation", "REG_SZ", dir.to_string_lossy().into()),
        ("InstallDate", "REG_SZ", date),
        ("DisplayIcon", "REG_SZ", exe.to_string_lossy().into()),
        ("UninstallString", "REG_SZ", format!("\"{}\" uninstall --pause", exe.display())),
        ("QuietUninstallString", "REG_SZ", format!("\"{}\" uninstall --quiet", exe.display())),
        ("EstimatedSize", "REG_DWORD", size_kb.to_string()),
        ("NoModify", "REG_DWORD", "1".into()),
        ("NoRepair", "REG_DWORD", "1".into()),
    ];
    for (name, kind, value) in values {
        let (ok, text) = run_cmd("reg.exe", &["add", APP_KEY, "/v", name, "/t", kind, "/d", &value, "/f"])?;
        if !ok {
            return Err(format!("reg add {name} failed: {}", text.trim()));
        }
    }
    Ok(())
}

fn unregister_app() -> Result<(), String> {
    let _ = run_cmd("reg.exe", &["delete", APP_KEY, "/f"])?;
    Ok(())
}

fn app_registered() -> bool {
    run_cmd("reg.exe", &["query", APP_KEY, "/v", "InstallLocation"]).map(|(ok, _)| ok).unwrap_or(false)
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            total += if p.is_dir() { dir_size(&p) } else { p.metadata().map(|m| m.len()).unwrap_or(0) };
        }
    }
    total
}

// ---------------------------------------------------------------------------------------------
// Files

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("cannot create {}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("cannot read {}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| format!("cannot copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn is_dist_folder(dir: &Path) -> bool {
    dir.join("driver.vrdrivermanifest").exists() && dir.join("bin").join("win64").join("driver_leapfrog.dll").exists()
}

fn version_of(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("VERSION")).map(|v| v.trim().to_string()).unwrap_or_else(|_| "unknown".to_string())
}

/// Removes the `driver_leapfrog` section from steamvr.vrsettings, after a backup copy.
/// Drops `leapfrog-panel.vrmanifest` from SteamVR's registered application manifests
/// (`appconfig.json`, `manifest_paths`), which the panel added for the startup overlay list.
/// With SteamVR closed the file is the registry, and a stale entry would stay listed otherwise.
fn remove_overlay_manifest(config_dir: &Path) -> Result<bool, String> {
    let file = config_dir.join("appconfig.json");
    let Ok(text) = std::fs::read_to_string(&file) else { return Ok(false) };
    let mut out = String::with_capacity(text.len());
    let mut removed = false;
    for line in text.split_inclusive('\n') {
        if line.contains("leapfrog-panel.vrmanifest") {
            removed = true;
            // The entry before this one may now carry a trailing comma it must lose.
            if !line.trim_end().ends_with(',') {
                if let Some(comma) = out.trim_end().strip_suffix(',') {
                    let keep = comma.len();
                    out.truncate(keep);
                    out.push('\n');
                }
            }
            continue;
        }
        out.push_str(line);
    }
    if removed {
        std::fs::write(&file, out).map_err(|e| format!("cannot update {}: {e}", file.display()))?;
    }
    Ok(removed)
}

fn purge_settings(config_dir: &Path) -> Result<bool, String> {
    let file = config_dir.join("steamvr.vrsettings");
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(_) => return Ok(false),
    };
    let Some(key_at) = text.find("\"driver_leapfrog\"") else { return Ok(false) };
    let Some(open) = text[key_at..].find('{').map(|i| key_at + i) else { return Ok(false) };
    let mut depth = 0;
    let mut close = None;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else { return Ok(false) };
    // Take a following comma, or else a preceding one (last member).
    let mut start = key_at;
    let mut end = close;
    let after = text[close..].trim_start();
    if after.starts_with(',') {
        end = close + (text[close..].len() - after.len()) + 1;
    } else {
        let before = text[..key_at].trim_end();
        if before.ends_with(',') {
            start = before.len() - 1;
        }
    }
    std::fs::copy(&file, file.with_extension("vrsettings.leapfrog.bak")).map_err(|e| format!("backup failed: {e}"))?;
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    std::fs::write(&file, out).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Commands

fn status_text() -> Result<String, String> {
    let mut s = String::new();
    let _ = writeln!(s, "LeapFrog setup");
    match steamvr() {
        Ok(vr) => {
            let _ = writeln!(s, "  SteamVR:   {} ({})", vr.vrpathreg.parent().and_then(Path::parent).and_then(Path::parent).map(|p| p.display().to_string()).unwrap_or_default(), if steamvr_running() { "running" } else { "closed" });
            match vr.registered()? {
                Some(dir) => {
                    let _ = writeln!(s, "  Driver:    registered at {} (version {}){}", dir.display(), version_of(&dir), if dir.exists() { "" } else { ", folder missing" });
                }
                None => {
                    let _ = writeln!(s, "  Driver:    not registered");
                }
            }
        }
        Err(e) => {
            let _ = writeln!(s, "  SteamVR:   {e}");
        }
    }
    let _ = writeln!(s, "  Apps list: {}", if app_registered() { "LeapFrog is listed" } else { "not listed" });
    let info = service_info()?;
    if info.exists {
        let sid = current_user_sid().unwrap_or_default();
        let _ = writeln!(s, "  Service:   {SERVICE} {}, {}, user {} start/stop it", info.start_type.to_lowercase(), info.state.to_lowercase(), if user_has_service_rights(&info.sddl, &sid) { "may" } else { "may NOT" });
    } else {
        let _ = writeln!(s, "  Service:   {SERVICE} not installed (Ultraleap Gemini missing)");
    }
    Ok(s)
}

fn install(o: &Options) -> Result<(), String> {
    let vr = steamvr()?;
    if steamvr_running() {
        return Err("SteamVR is running; close it first (the driver file is locked while it runs)".into());
    }
    if !service_info()?.exists {
        return Err(format!("the {SERVICE} service is not installed: install Ultraleap Gemini first"));
    }
    let source = exe_dir()?;
    if !is_dist_folder(&source) {
        return Err(format!("{} is not a LeapFrog driver folder (driver.vrdrivermanifest, bin\\win64\\driver_leapfrog.dll)", source.display()));
    }
    let sid = match &o.user_sid {
        Some(s) => s.clone(),
        None => current_user_sid()?,
    };
    if !win::is_elevated() {
        say(o, "Elevating for the tracking service's settings...");
        let mut args: Vec<String> = std::env::args().skip(1).collect();
        args.push("--user-sid".into());
        args.push(sid);
        let code = win::relaunch_elevated(&args)?;
        return if code == 0 { Ok(()) } else { Err(format!("the elevated step exited with code {code}")) };
    }
    let target = if o.in_place {
        source.clone()
    } else {
        match &o.to {
            Some(t) => t.clone(),
            None => PathBuf::from(std::env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set")?).join("LeapFrog").join("driver"),
        }
    };
    let same = target.canonicalize().ok() == source.canonicalize().ok();
    if !same {
        if let Some(old) = vr.registered()? {
            if old != target && old.exists() {
                say(o, &format!("Replacing the registration of {}", old.display()));
                let _ = vr.remove_driver(&old);
            }
        }
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| format!("cannot replace {}: {e}", target.display()))?;
        }
        copy_dir(&source, &target)?;
        say(o, &format!("Copied to {}", target.display()));
    }
    if vr.registered()?.as_deref() != Some(target.as_path()) {
        vr.add_driver(&target)?;
    }
    say(o, "Registered with SteamVR");
    configure_service(&sid)?;
    say(o, &format!("{SERVICE}: manual start, start/stop allowed for this user"));
    register_app(&target, &version_of(&target))?;
    say(o, "Listed in Settings > Apps");
    say(o, "\nDone. Start SteamVR; the two Leap hands appear next to your controllers.");
    Ok(())
}

fn uninstall(o: &Options) -> Result<(), String> {
    let vr = steamvr()?;
    if steamvr_running() {
        return Err("SteamVR is running; close it first".into());
    }
    let registered = vr.registered()?;
    let folder = o.folder.clone().or(registered.clone());
    let me = exe_dir()?;
    // Running from inside the folder to delete: continue from a temp copy.
    if let Some(dir) = &folder {
        if o.folder.is_none() && me.canonicalize().ok() == dir.canonicalize().ok() {
            let temp = PathBuf::from(std::env::var("TEMP").map_err(|_| "TEMP not set")?).join("leapfrog-setup.exe");
            std::fs::copy(std::env::current_exe().map_err(|e| e.to_string())?, &temp).map_err(|e| format!("cannot copy to temp: {e}"))?;
            let mut args: Vec<String> = std::env::args().skip(1).collect();
            args.push("--folder".into());
            args.push(dir.to_string_lossy().into());
            Command::new(&temp).args(&args).spawn().map_err(|e| format!("cannot relaunch from temp: {e}"))?;
            return Ok(());
        }
    }
    if o.restore_service && !win::is_elevated() {
        say(o, "Elevating to restore the service's start type...");
        let code = win::relaunch_elevated(&std::env::args().skip(1).collect::<Vec<_>>())?;
        return if code == 0 { Ok(()) } else { Err(format!("the elevated step exited with code {code}")) };
    }
    if let Some(dir) = &registered {
        vr.remove_driver(dir)?;
        say(o, "Unregistered from SteamVR");
    }
    if let Some(dir) = &folder {
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
            say(o, &format!("Removed {}", dir.display()));
        }
    }
    if remove_overlay_manifest(&vr.config_dir)? {
        say(o, "Removed the settings panel from SteamVR's startup overlay apps");
    }
    unregister_app()?;
    say(o, "Removed from Settings > Apps");
    if o.purge_settings {
        if purge_settings(&vr.config_dir)? {
            say(o, "Removed the driver_leapfrog settings (backup: steamvr.vrsettings.leapfrog.bak)");
        }
    } else {
        say(o, "Settings kept (driver_leapfrog in steamvr.vrsettings); --purge-settings removes them");
    }
    if o.restore_service {
        let (ok, text) = run_cmd("sc.exe", &["config", SERVICE, "start=", "auto"])?;
        if !ok {
            return Err(format!("sc config failed: {}", text.trim()));
        }
        say(o, &format!("{SERVICE} back to automatic start"));
    } else {
        say(o, &format!("{SERVICE} left on manual start; --restore-service puts it back to automatic"));
    }
    say(o, "\nDone.");
    Ok(())
}

fn interactive(_o: &Options) -> Result<(), String> {
    print!("{}", status_text()?);
    let here = exe_dir()?;
    let installable = is_dist_folder(&here);
    println!();
    if installable {
        println!("  [I] Install from this folder (to %LOCALAPPDATA%\\LeapFrog\\driver)");
        println!("  [R] Register this folder in place");
    }
    println!("  [U] Uninstall");
    println!("  [Q] Quit");
    print!("> ");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    let pause = Options { pause: true, ..parse(vec![]) };
    match line.trim().to_ascii_lowercase().as_str() {
        "i" if installable => install(&Options { command: "install".into(), ..pause }),
        "r" if installable => install(&Options { command: "install".into(), in_place: true, ..pause }),
        "u" => uninstall(&Options { command: "uninstall".into(), ..pause }),
        _ => Ok(()),
    }
}
